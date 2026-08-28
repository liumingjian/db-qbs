//! 目标端的真实 [`Destination`]：连接、元数据、写暂存表、切换。
//!
//! **这个文件里的三件事没有单元测试守，守它们的是 `docs/spikes/fixtures/local-rig`
//! 的验收台架**（需要一台起得了 docker MySQL 的机器）：
//!
//! - `CLIENT_FOUND_ROWS` 把「值未变的既有行记 0」抹平（#138），也就是
//!   `swap_rows_in_range` 的下界之所以能是 `staged_rows` 的前提；
//! - upsert 的 `affected_rows` 落在 `[staged, 2×staged]`（`run-v1-acceptance.sh` 的 C4②）；
//! - Connection Ritual 挂在连接池的建连钩子上，池里第**二**条连接也照做。
//!
//! 这是刻意的分工，不是遗漏：这三件事出错的方式是「MySQL 的语义变了」或「有人动了建连钩子」，
//! 不是每次改代码都会碰的东西；把它们塞进 `cargo test` 等于让「测试全绿」从此依赖 docker。
//! 拿测试替身换掉本文件时，换掉的正是这三件事——见 `test_support` 的模块说明。

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use db_qbs_shared::{swap_rows_in_range, MysqlServerInfo, RowCounts};
use mysql::prelude::Queryable;
use mysql::{
    consts::CapabilityFlags, params, Conn, Error as MysqlError, Opts, OptsBuilder, Params, TxOpts,
    Value as MysqlValue,
};

use crate::service::quote_identifier;
use crate::{
    AtomicSwapError, AtomicSwapRequest, AtomicSwapResult, ConnectedDestination, CreateStagingError,
    Destination, DestinationFactory, DropStagingError, TargetColumn, TargetConnection, TargetKey,
    WriteBatchError,
};

/// 曾经的写入台账表，随「撤销运行」一起废弃（#256）。
///
/// 名字留着只为**删**：老版本会在客户的目标库里建这张表、逐行记下每次运行写过的主键，
/// 已经装出去的实例里它还在。产品不该在客户库里留下自建的附属表，所以每次切换前
/// 顺手 `DROP TABLE IF EXISTS` 一次——不再建、只删。
const LEGACY_WRITE_LEDGER_TABLE: &str = "__db_qbs_write_ledger";

type MetadataRow = (
    String,
    String,
    String,
    Option<u64>,
    Option<u64>,
    Option<u64>,
    Option<u64>,
    String,
    Option<String>,
    u64,
    Option<String>,
    String,
);

pub struct MysqlDestination {
    database: String,
    pool: RitualPool,
    /// 建连接那一刻读到的服务端自述（#257）。**读一次就够**：一条连接活着的时候
    /// 服务端不会换版本，而 `GET /v1/agent/info` 那边一秒可能来好几次。
    server: Option<MysqlServerInfo>,
}

impl MysqlDestination {
    /// 按一份过线来的连接信息建目的地（ADR-0037 §1/§2）。
    ///
    /// **不拼 DSN 串**：口令要单独加密存放，拼串就得在两端各做一次 URL 转义，
    /// 多一层无谓的转义面。`OptsBuilder` 本来就收分量。
    pub fn connect(target: &TargetConnection) -> Result<Self, String> {
        if target.database.is_empty() {
            return Err("数据源的 database 不能为空".to_owned());
        }
        if target.host.is_empty() {
            return Err("数据源的 host 不能为空".to_owned());
        }
        let opts = OptsBuilder::new()
            .ip_or_hostname(Some(target.host.clone()))
            .tcp_port(target.port)
            .user(Some(target.username.clone()))
            .pass(Some(target.password.clone()))
            .db_name(Some(target.database.clone()))
            // `CLIENT_FOUND_ROWS` 让 MySQL 按**匹配到的行**而不是**值真变了的行**计数。
            // 少了它，`ON DUPLICATE KEY UPDATE` 对「值与目标端一模一样的既有行」记 0，
            // 一次原地重跑的 `affected_rows` 就是 0，切换段的区间断言当场把成功判成
            // `SWAP_FAILED`（#138）。带上之后这一档记 1，`[n, 2n]` 重新精确。
            .additional_capabilities(CapabilityFlags::CLIENT_FOUND_ROWS);
        let pool = RitualPool::new(Opts::from(opts))?;
        let server = read_server_info(&pool);
        Ok(Self {
            database: target.database.clone(),
            pool,
            server,
        })
    }

    /// 这个库里有哪些表（ADR-0038 §3）——构建器的目标表下拉喂的就是它。
    ///
    /// **不进 [`Destination`] trait**：搬运那条链一次也用不到它，进 trait 只会让
    /// 二十几个测试夹具各补一个用不上的实现。库名取连接自己的 `database`——
    /// 不做库清单端点、也不收第二个库名，否则任务就能推翻数据源的定义（ADR-0038 §3）。
    ///
    /// 表不存在这件事在这里不成立：查不到就是空清单，不是错误（ADR-0038 §9）。
    pub fn target_tables(&self) -> Result<Vec<String>, String> {
        self.pool
            .with_conn(|connection| {
                connection.exec(
                    r#"
SELECT TABLE_NAME
 FROM information_schema.TABLES
 WHERE TABLE_SCHEMA = :database
 ORDER BY TABLE_NAME
"#,
                    params! { "database" => &self.database },
                )
            })
            .map_err(|error| error.to_string())
    }

    /// 「测试连接」：建一条连接、跑一遍开连接仪式，成了就算通（ADR-0037 §9）。
    ///
    /// `RitualPool::new` 本来就是**急建**的——它当场开一条连接并跑完仪式
    /// （utf8mb4 / `sql_mode` / `max_allowed_packet` 三项回读），所以「建得起来」
    /// 与「能用」在这里是同一件事，不必另设探针查询。
    pub fn test_connection(target: &TargetConnection) -> Result<(), String> {
        Self::connect(target).map(|_| ())
    }
}

/// 生产路径的工厂：每个 run 现连一次（ADR-0037 §2）。
pub struct MysqlFactory;

impl DestinationFactory for MysqlFactory {
    type Dest = MysqlDestination;

    fn connect(
        &self,
        target: &TargetConnection,
    ) -> Result<ConnectedDestination<Self::Dest>, String> {
        Ok(ConnectedDestination {
            database: target.database.clone(),
            destination: Arc::new(MysqlDestination::connect(target)?),
        })
    }
}

impl Destination for MysqlDestination {
    fn server_info(&self) -> Option<MysqlServerInfo> {
        self.server.clone()
    }

    fn target_columns(&self, target_table: &str) -> Result<Vec<TargetColumn>, String> {
        self.pool
            .with_conn(|connection| {
                connection.exec_map(
                    r#"
SELECT COLUMN_NAME, COLUMN_TYPE, DATA_TYPE,
       NUMERIC_PRECISION, NUMERIC_SCALE, CHARACTER_MAXIMUM_LENGTH,
       DATETIME_PRECISION, IS_NULLABLE, CHARACTER_SET_NAME, ORDINAL_POSITION,
       COLUMN_DEFAULT, EXTRA
  FROM information_schema.COLUMNS
 WHERE TABLE_SCHEMA = :database AND TABLE_NAME = :target_table
 ORDER BY ORDINAL_POSITION
"#,
                    params! {
                        "database" => &self.database,
                        "target_table" => target_table,
                    },
                    |(
                        name,
                        column_type,
                        data_type,
                        precision,
                        scale,
                        length,
                        datetime_precision,
                        is_nullable,
                        character_set,
                        ordinal,
                        default_value,
                        extra,
                    ): MetadataRow| TargetColumn {
                        name,
                        column_type,
                        data_type,
                        precision,
                        scale,
                        length,
                        datetime_precision,
                        nullable: is_nullable == "YES",
                        character_set,
                        ordinal,
                        default_value,
                        extra,
                    },
                )
            })
            .map_err(|error| error.to_string())
    }

    fn target_keys(&self, target_table: &str) -> Result<Vec<TargetKey>, String> {
        let rows: Vec<(String, String)> = self
            .pool
            .with_conn(|connection| {
                connection.exec(
                    r#"
SELECT INDEX_NAME, COLUMN_NAME
  FROM information_schema.STATISTICS
 WHERE TABLE_SCHEMA = :database AND TABLE_NAME = :target_table AND NON_UNIQUE = 0
 ORDER BY INDEX_NAME, SEQ_IN_INDEX
"#,
                    params! {
                        "database" => &self.database,
                        "target_table" => target_table,
                    },
                )
            })
            .map_err(|error| error.to_string())?;

        let mut keys: Vec<TargetKey> = Vec::new();
        for (index_name, column_name) in rows {
            match keys.last_mut() {
                Some(key) if key.name == index_name => key.columns.push(column_name),
                _ => keys.push(TargetKey {
                    name: index_name,
                    columns: vec![column_name],
                }),
            }
        }
        Ok(keys)
    }

    fn create_staging(&self, _staging_table: &str, ddl: &str) -> Result<(), CreateStagingError> {
        self.pool
            .with_conn(|connection| connection.query_drop(ddl))
            .map_err(classify_create_error)
    }

    fn write_batch(
        &self,
        staging_table: &str,
        columns: &[String],
        rows: &[Vec<Option<String>>],
        max_rows_per_insert: usize,
    ) -> Result<u64, WriteBatchError> {
        let outcome = self
            .pool
            .with_conn(|connection| {
                let mut transaction = connection.start_transaction(TxOpts::default())?;
                let mut affected_rows = 0;
                for row_chunk in rows.chunks(max_rows_per_insert) {
                    let statement = build_insert_statement(
                        &self.database,
                        staging_table,
                        columns,
                        row_chunk.len(),
                    );
                    let values = row_chunk
                        .iter()
                        .flat_map(|row| row.iter())
                        .map(|value| match value {
                            Some(value) => MysqlValue::Bytes(value.as_bytes().to_vec()),
                            None => MysqlValue::NULL,
                        })
                        .collect();
                    if let Err(error) = transaction.exec_drop(statement, Params::Positional(values))
                    {
                        return Ok(BatchWriteOutcome::Rejected(classify_write_error(
                            error, columns, row_chunk,
                        )));
                    }
                    affected_rows += transaction.affected_rows();
                    let warnings: Vec<(String, u16, String)> =
                        transaction.query("SHOW WARNINGS")?;
                    if let Some((_, code, message)) =
                        warnings.into_iter().find(|(_, code, _)| *code == 1265)
                    {
                        return Ok(BatchWriteOutcome::Rejected(classify_mysql_diagnostic(
                            code, &message, columns, row_chunk,
                        )));
                    }
                }
                transaction.commit()?;
                Ok(BatchWriteOutcome::Written(affected_rows))
            })
            .map_err(|error| WriteBatchError::Other(error.to_string()))?;

        match outcome {
            BatchWriteOutcome::Written(affected_rows) => Ok(affected_rows),
            BatchWriteOutcome::Rejected(error) => Err(error),
        }
    }

    fn atomic_swap(
        &self,
        request: &AtomicSwapRequest,
    ) -> Result<AtomicSwapResult, AtomicSwapError> {
        let outcome = self
            .pool
            .with_conn(|connection| {
                // 台账已废（#256）：这里只把老版本留下的那张表删掉，不再建。
                // `DROP TABLE` 会隐式提交，所以必须在切换事务开始之前做。
                connection.query_drop(drop_legacy_write_ledger(&self.database))?;
                let mut transaction = connection.start_transaction(TxOpts::default())?;
                let count_statement = format!(
                    "SELECT COUNT(*) FROM {}.{}",
                    quote_identifier(&self.database),
                    quote_identifier(&request.staging_table)
                );
                let count_started = Instant::now();
                let staged_rows: u64 = transaction
                    .query_first(count_statement)?
                    .expect("SELECT COUNT(*) must return one row");
                let count_ms = count_started
                    .elapsed()
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX);
                // 判据在 `shared::verification` 里，两端读同一份：这里之所以要在事务内
                // 取 `staged_rows` 再问它，是因为「数的那份」和「切的那份」必须是同一个快照。
                let counts = RowCounts {
                    source_rows: request.source_rows,
                    staged_rows,
                    source_batches: request.source_batches,
                    received_batches: request.received_batches,
                };
                if !counts.verdict().passed() {
                    transaction.rollback()?;
                    let error = AtomicSwapError::VerifyFailed {
                        staged_rows,
                        count_ms,
                    };
                    return Ok(AtomicSwapOutcome::Failed(error));
                }

                let insert_statement = build_swap_upsert_statement(
                    &self.database,
                    &request.target_table,
                    &request.staging_table,
                    &request.columns,
                    &request.primary_key,
                );
                transaction.query_drop(insert_statement)?;
                let swapped_rows = transaction.affected_rows();
                // 区间判据（而非等值）连同它的三条理由都在 `swap_rows_in_range` 的文档里，
                // source 侧那面镜子读的也是它——上一次两端各写一份时，
                // sink 改了区间、source 的等值断言没跟上，「重跑改值」整条主路径必失败（#135 C4④）。
                // 这一档最后落到 `SWAP_FAILED` / `SinkWrite`，而 source 那面镜子同样这件事
                // 落 `Defect`（#196）——**不是同一个故障两个标签**：这里是第一手判断，
                // 数字确实不对；那里只有在本端已经放行之后才可能响，那时问题在两端判据不同。
                if !swap_rows_in_range(staged_rows, swapped_rows) {
                    transaction.rollback()?;
                    let message = format!(
                        "暂存表有 {staged_rows} 行，切换 upsert 报告影响 {swapped_rows} 行，不落在 [{staged_rows}, {}] 区间内",
                        staged_rows.saturating_mul(2)
                    );
                    return Ok(AtomicSwapOutcome::Failed(AtomicSwapError::Other(message)));
                }
                transaction.commit()?;
                Ok(AtomicSwapOutcome::Swapped(AtomicSwapResult {
                    staged_rows,
                    // 新模型不删任何行；这个 0 是事实，不是拿 0 糊弄（ADR-0035 §4）。
                    purged_rows: 0,
                    swapped_rows,
                    count_ms,
                }))
            })
            .map_err(classify_atomic_swap_error)?;

        match outcome {
            AtomicSwapOutcome::Swapped(result) => Ok(result),
            AtomicSwapOutcome::Failed(error) => Err(error),
        }
    }

    fn drop_staging(&self, staging_table: &str) -> Result<(), DropStagingError> {
        let statement = format!(
            "DROP TABLE IF EXISTS {}.{}",
            quote_identifier(&self.database),
            quote_identifier(staging_table)
        );
        self.pool
            .with_conn(|connection| connection.query_drop(statement))
            .map_err(classify_drop_error)
    }
}

fn drop_legacy_write_ledger(database: &str) -> String {
    format!(
        "DROP TABLE IF EXISTS {}.{}",
        quote_identifier(database),
        quote_identifier(LEGACY_WRITE_LEDGER_TABLE),
    )
}

fn classify_write_error(
    error: MysqlError,
    columns: &[String],
    rows: &[Vec<Option<String>>],
) -> WriteBatchError {
    match error {
        MysqlError::MySqlError(error) if matches!(error.code, 1153 | 1264 | 1292 | 1366) => {
            classify_mysql_diagnostic(error.code, &error.message, columns, rows)
        }
        error => WriteBatchError::Other(error.to_string()),
    }
}

fn classify_mysql_diagnostic(
    code: u16,
    message: &str,
    columns: &[String],
    rows: &[Vec<Option<String>>],
) -> WriteBatchError {
    if code == 1153 {
        return WriteBatchError::Environment { mysql_code: code };
    }

    let (column, value) = mysql_error_location(message, columns, rows)
        .map(|(column, value)| (Some(column), value))
        .unwrap_or((None, None));
    if code == 1265 {
        return WriteBatchError::PrecheckEscape {
            mysql_code: code,
            column,
            value,
        };
    }

    WriteBatchError::DataValue {
        mysql_code: code,
        column: column.unwrap_or_else(|| "<无法从 MySQL 错误定位列>".to_owned()),
        value,
    }
}

fn mysql_error_location(
    message: &str,
    columns: &[String],
    rows: &[Vec<Option<String>>],
) -> Option<(String, Option<String>)> {
    let (_, location) = message.rsplit_once(" for column '")?;
    let (reported_column, row) = location.split_once("' at row ")?;
    let row = row
        .trim_end_matches(|character: char| !character.is_ascii_digit())
        .parse::<usize>()
        .ok()?;
    let short_column = reported_column
        .rsplit('.')
        .next()
        .unwrap_or(reported_column)
        .trim_matches('`');
    let column_index = columns
        .iter()
        .position(|column| column.eq_ignore_ascii_case(short_column))?;
    let value = rows
        .get(row.checked_sub(1)?)
        .and_then(|values| values.get(column_index))
        .cloned()
        .flatten();
    Some((columns[column_index].clone(), value))
}

enum BatchWriteOutcome {
    Written(u64),
    Rejected(WriteBatchError),
}

enum AtomicSwapOutcome {
    Swapped(AtomicSwapResult),
    Failed(AtomicSwapError),
}

fn build_insert_statement(
    database: &str,
    staging_table: &str,
    columns: &[String],
    row_count: usize,
) -> String {
    let column_count = columns.len();
    let quoted_columns = columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let row_placeholders = format!("({})", vec!["?"; column_count].join(","));
    let value_placeholders = vec![row_placeholders; row_count].join(",");
    format!(
        "INSERT INTO {}.{} ({quoted_columns}) VALUES {value_placeholders}",
        quote_identifier(database),
        quote_identifier(staging_table)
    )
}

/// 切换段：`INSERT ... SELECT ... ON DUPLICATE KEY UPDATE`，**不再 DELETE**（ADR-0035 §1）。
///
/// 更新列 = 全部选中列**排除主键列本身**，语义是「同一主键的行，以本次源端数据为准」。
/// 不做部分更新：没有对应需求，且会引入「这列这次没更新是有意还是漏了」的排查成本。
fn build_swap_upsert_statement(
    database: &str,
    target_table: &str,
    staging_table: &str,
    columns: &[String],
    primary_key: &[String],
) -> String {
    let quoted_columns = columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let updates = columns
        .iter()
        .filter(|column| {
            !primary_key
                .iter()
                .any(|key| key.eq_ignore_ascii_case(column))
        })
        .map(|column| {
            let quoted = quote_identifier(column);
            format!("{quoted} = VALUES({quoted})")
        })
        .collect::<Vec<_>>();
    let mut statement = format!(
        "INSERT INTO {}.{} ({quoted_columns}) SELECT {quoted_columns} FROM {}.{}",
        quote_identifier(database),
        quote_identifier(target_table),
        quote_identifier(database),
        quote_identifier(staging_table)
    );
    // 全部列都是主键时没有可更新的列，但仍必须带上这段：否则重跑会撞 `ERROR 1062`
    // 而不是安静地幂等。拿主键列做一次无操作赋值即可。
    let assignments = if updates.is_empty() {
        let quoted = quote_identifier(&primary_key[0]);
        format!("{quoted} = {quoted}")
    } else {
        updates.join(", ")
    };
    statement.push_str(&format!(" ON DUPLICATE KEY UPDATE {assignments}"));
    statement
}

// Connections enter this pool only after the creation hook has completed.
struct RitualPool {
    opts: Opts,
    idle: Mutex<Vec<Conn>>,
}

impl RitualPool {
    fn new(opts: Opts) -> Result<Self, String> {
        let pool = Self {
            opts,
            idle: Mutex::new(Vec::new()),
        };
        let connection = pool.connect()?;
        pool.idle
            .lock()
            .expect("MySQL pool mutex poisoned")
            .push(connection);
        Ok(pool)
    }

    fn connect(&self) -> Result<Conn, String> {
        let mut connection =
            Conn::new(self.opts.clone()).map_err(|error| format!("连接 MySQL 失败：{error}"))?;
        run_connection_ritual(&mut connection)?;
        Ok(connection)
    }

    fn with_conn<T>(
        &self,
        operation: impl FnOnce(&mut Conn) -> Result<T, MysqlError>,
    ) -> Result<T, PoolError> {
        let pooled = self.idle.lock().expect("MySQL pool mutex poisoned").pop();
        let mut connection = match pooled {
            Some(mut connection) => {
                if connection.ping().is_ok() {
                    connection
                } else {
                    self.connect().map_err(PoolError::ConnectionRitual)?
                }
            }
            None => self.connect().map_err(PoolError::ConnectionRitual)?,
        };

        let result = operation(&mut connection).map_err(PoolError::Mysql);
        self.idle
            .lock()
            .expect("MySQL pool mutex poisoned")
            .push(connection);
        result
    }
}

/// 读一次服务端自述：`@@version` 与 utf8mb4 的默认字符序（#257）。
///
/// **字符序是查出来的，不是按版本号推出来的**：8.0 给 `utf8mb4_0900_ai_ci`、
/// 5.7 给 `utf8mb4_general_ci`，但部署方完全可以把 `collation-server` 改成别的，
/// 而这份值最终会写进 source 生成的建表语句里。推一份出来只会在建完表之后才暴露错误。
///
/// **读失败不是错误**：回 `None`，调用方当「没观察到」处理。这两项只喂建表语句的
/// 字符序与界面上的版本一列，够不着搬运链的正确性；为它让 `POST /v1/runs` 失败，
/// 换来的是一次本来能跑完的同步跑不起来。
fn read_server_info(pool: &RitualPool) -> Option<MysqlServerInfo> {
    let row: Option<(String, Option<String>)> = pool
        .with_conn(|connection| {
            connection.query_first(
                "SELECT @@version, \
                        (SELECT DEFAULT_COLLATE_NAME FROM information_schema.CHARACTER_SETS \
                          WHERE CHARACTER_SET_NAME = 'utf8mb4')",
            )
        })
        .ok()?;
    let (version, utf8mb4_collation) = row?;
    Some(MysqlServerInfo {
        version,
        utf8mb4_collation: utf8mb4_collation?,
    })
}

fn run_connection_ritual(connection: &mut Conn) -> Result<(), String> {
    connection
        .query_drop("SET NAMES utf8mb4")
        .map_err(|error| format!("开连接仪式设置 utf8mb4 失败：{error}"))?;
    connection
        .query_drop("SET SESSION sql_mode = 'STRICT_ALL_TABLES'")
        .map_err(|error| format!("开连接仪式设置 sql_mode 失败：{error}"))?;

    let settings: Option<(String, String, String, String, u64)> = connection
        .query_first(
            "SELECT @@character_set_client, @@character_set_connection, \
                    @@character_set_results, @@SESSION.sql_mode, @@max_allowed_packet",
        )
        .map_err(|error| format!("开连接仪式回读会话变量失败：{error}"))?;
    let Some((client, connection_charset, results, sql_mode, max_allowed_packet)) = settings else {
        return Err("开连接仪式回读会话变量没有返回结果，整个 sink 不可用".to_owned());
    };

    check_connection_settings(
        &client,
        &connection_charset,
        &results,
        &sql_mode,
        max_allowed_packet,
    )
    .map_err(|message| format!("开连接仪式失败，整个 sink 不可用：{message}"))
}

enum PoolError {
    Mysql(MysqlError),
    ConnectionRitual(String),
}

impl fmt::Display for PoolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mysql(error) => error.fmt(formatter),
            Self::ConnectionRitual(message) => formatter.write_str(message),
        }
    }
}

fn classify_create_error(error: PoolError) -> CreateStagingError {
    match error {
        PoolError::Mysql(MysqlError::MySqlError(error)) if error.code == 1050 => {
            CreateStagingError::TableExists
        }
        PoolError::Mysql(MysqlError::MySqlError(error)) if is_permission_error(error.code) => {
            CreateStagingError::PermissionDenied
        }
        other => CreateStagingError::Other(other.to_string()),
    }
}

fn classify_drop_error(error: PoolError) -> DropStagingError {
    match error {
        PoolError::Mysql(MysqlError::MySqlError(error)) if is_permission_error(error.code) => {
            DropStagingError::PermissionDenied
        }
        other => DropStagingError::Other(other.to_string()),
    }
}

fn classify_atomic_swap_error(error: PoolError) -> AtomicSwapError {
    match error {
        PoolError::Mysql(MysqlError::MySqlError(error)) if matches!(error.code, 1205 | 1213) => {
            AtomicSwapError::TargetBusy { errno: error.code }
        }
        other => AtomicSwapError::Other(other.to_string()),
    }
}

fn is_permission_error(code: u16) -> bool {
    matches!(code, 1044 | 1045 | 1142 | 1143 | 1227)
}

/// 开连接仪式对 `max_allowed_packet` 的下界，64 MiB（ADR-0024）。
///
/// **不放宽**（#262）：它守的是一整批 `INSERT` 被 MySQL 按包长截断——截断在协议层
/// 就把语句砍成半截，报出来的是语法错，排查的人会去翻业务数据，而数据是好的。
pub const MIN_PACKET: u64 = 64 * 1024 * 1024;

/// 把 `max_allowed_packet` 调够的照做指令——开连接仪式与运行期两处报文共用这一份。
///
/// 5.7 的默认值是 4 MiB，8.0 的默认值才是 64 MiB，所以**每一台没调过参的 5.7 都会在
/// 第一次开连接仪式上撞到这道门**（#262）。门不放宽，能改的只有报文：从「一次神秘的
/// 环境故障」变成「照抄这两行」。
///
/// 两条都给，缺一不可：`SET GLOBAL` 当场生效但重启就没了；my.cnf 要重启才生效，
/// 但能留到下一次重启之后。只给前者，客户重启一次数据库就退回原样；只给后者，
/// 得先停一次库才能开工。
pub const PACKET_REMEDY: &str = "请在目标库上以有 SUPER 权限的账号执行 `SET GLOBAL max_allowed_packet = 67108864;`（当场生效，但只对之后新建的连接生效，改完请重跑），并在 my.cnf 的 `[mysqld]` 段写上 `max_allowed_packet = 64M`，让它在 MySQL 重启之后仍然成立；MySQL 5.7 的默认值是 4 MiB，未调参的 5.7 实例都会停在这里。这是目标端环境配置，不要排查业务数据";

pub fn check_connection_settings(
    character_set_client: &str,
    character_set_connection: &str,
    character_set_results: &str,
    sql_mode: &str,
    max_allowed_packet: u64,
) -> Result<(), String> {
    const EXPECTED_CHARSET: &str = "utf8mb4";
    const EXPECTED_SQL_MODE: &str = "STRICT_ALL_TABLES";

    let mut problems = Vec::new();
    for (name, actual) in [
        ("character_set_client", character_set_client),
        ("character_set_connection", character_set_connection),
        ("character_set_results", character_set_results),
    ] {
        if actual != EXPECTED_CHARSET {
            problems.push(format!(
                "环境配置错误：{name} 期望 {EXPECTED_CHARSET}，实际 {actual}"
            ));
        }
    }
    if sql_mode != EXPECTED_SQL_MODE {
        problems.push(format!(
            "环境配置错误：sql_mode 期望完整值 {EXPECTED_SQL_MODE}，实际 {sql_mode}"
        ));
    }
    if max_allowed_packet < MIN_PACKET {
        problems.push(format!(
            "环境配置错误：max_allowed_packet 期望至少 {MIN_PACKET} 字节（64 MiB），实际 {max_allowed_packet} 字节；{PACKET_REMEDY}"
        ));
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("；"))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_insert_statement, build_swap_upsert_statement, classify_atomic_swap_error,
        classify_mysql_diagnostic, PoolError,
    };
    use crate::{AtomicSwapError, WriteBatchError};
    use mysql::{Error as MysqlError, MySqlError};

    #[test]
    fn mysql_diagnostics_recover_the_original_column_and_value() {
        let columns = vec!["N_AMOUNT".to_owned(), "D_BIZ".to_owned()];
        let rows = vec![vec![
            Some("999999".to_owned()),
            Some("10000-01-01 00:00:00".to_owned()),
        ]];

        let error = classify_mysql_diagnostic(
            1292,
            "Incorrect datetime value for column 'D_BIZ' at row 1",
            &columns,
            &rows,
        );

        assert_eq!(
            error,
            WriteBatchError::DataValue {
                mysql_code: 1292,
                column: "D_BIZ".to_owned(),
                value: Some("10000-01-01 00:00:00".to_owned()),
            }
        );
    }

    #[test]
    fn mysql_lock_errors_are_target_busy() {
        for errno in [1205, 1213] {
            let error =
                classify_atomic_swap_error(PoolError::Mysql(MysqlError::MySqlError(MySqlError {
                    state: "HY000".to_owned(),
                    message: "target is locked".to_owned(),
                    code: errno,
                })));

            assert_eq!(error, AtomicSwapError::TargetBusy { errno });
        }
    }

    #[test]
    fn insert_statement_has_explicit_source_order_and_one_placeholder_per_value() {
        let statement = build_insert_statement(
            "qbs",
            "T_POSITION__stg_run",
            &["C_SECOND".to_owned(), "C_FIRST".to_owned()],
            3,
        );

        assert_eq!(
            statement,
            "INSERT INTO `qbs`.`T_POSITION__stg_run` (`C_SECOND`, `C_FIRST`) VALUES (?,?),(?,?),(?,?)"
        );
        assert_eq!(statement.matches('?').count(), 6);
    }

    #[test]
    fn the_swap_upserts_on_the_primary_key_and_updates_every_other_column() {
        assert_eq!(
            build_swap_upsert_statement(
                "qbs",
                "T_POSITION",
                "T_POSITION__stg_run",
                &["C_SECOND".to_owned(), "C_FIRST".to_owned()],
                &["c_first".to_owned()],
            ),
            concat!(
                "INSERT INTO `qbs`.`T_POSITION` (`C_SECOND`, `C_FIRST`) ",
                "SELECT `C_SECOND`, `C_FIRST` FROM `qbs`.`T_POSITION__stg_run` ",
                "ON DUPLICATE KEY UPDATE `C_SECOND` = VALUES(`C_SECOND`)"
            )
        );
    }

    #[test]
    fn an_all_key_table_still_gets_an_upsert_clause_so_a_rerun_stays_idempotent() {
        assert_eq!(
            build_swap_upsert_statement(
                "qbs",
                "T_POSITION",
                "T_POSITION__stg_run",
                &["C_ONLY".to_owned()],
                &["C_ONLY".to_owned()],
            ),
            concat!(
                "INSERT INTO `qbs`.`T_POSITION` (`C_ONLY`) ",
                "SELECT `C_ONLY` FROM `qbs`.`T_POSITION__stg_run` ",
                "ON DUPLICATE KEY UPDATE `C_ONLY` = `C_ONLY`"
            )
        );
    }
}
