use std::fmt;
use std::sync::Mutex;
use std::time::Instant;

use mysql::prelude::Queryable;
use mysql::{
    params, Conn, Error as MysqlError, Opts, OptsBuilder, Params, TxOpts, Value as MysqlValue,
};

use crate::service::quote_identifier;
use crate::{
    AtomicSwapError, AtomicSwapRequest, AtomicSwapResult, CreateStagingError, Destination,
    DropStagingError, SinkConfig, TargetColumn, WriteBatchError,
};

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
);

pub struct MysqlDestination {
    database: String,
    pool: RitualPool,
}

impl MysqlDestination {
    pub fn new(config: &SinkConfig) -> Result<Self, String> {
        if config.database.is_empty() {
            return Err("sink 配置 database 不能为空".to_owned());
        }
        let base = Opts::from_url(&config.mysql_dsn)
            .map_err(|error| format!("sink 配置 mysql_dsn 无效：{error}"))?;
        let opts = OptsBuilder::from_opts(base).db_name(Some(config.database.clone()));
        let pool = RitualPool::new(Opts::from(opts))?;
        Ok(Self {
            database: config.database.clone(),
            pool,
        })
    }
}

impl Destination for MysqlDestination {
    fn target_columns(&self, target_table: &str) -> Result<Vec<TargetColumn>, String> {
        self.pool
            .with_conn(|connection| {
                connection.exec_map(
                    r#"
SELECT COLUMN_NAME, COLUMN_TYPE, DATA_TYPE,
       NUMERIC_PRECISION, NUMERIC_SCALE, CHARACTER_MAXIMUM_LENGTH,
       DATETIME_PRECISION, IS_NULLABLE, CHARACTER_SET_NAME, ORDINAL_POSITION
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
                    },
                )
            })
            .map_err(|error| error.to_string())
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
                if staged_rows != request.source_rows
                    || request.received_batches != request.source_batches
                {
                    transaction.rollback()?;
                    let error = AtomicSwapError::VerifyFailed {
                        staged_rows,
                        count_ms,
                    };
                    return Ok(AtomicSwapOutcome::Failed(error));
                }

                let delete_statement = build_delete_statement(
                    &self.database,
                    &request.target_table,
                    &request.target_date_col,
                );
                transaction.exec_drop(
                    delete_statement,
                    (&request.biz_date_start, &request.biz_date_end),
                )?;
                let purged_rows = transaction.affected_rows();

                let insert_statement = build_swap_insert_statement(
                    &self.database,
                    &request.target_table,
                    &request.staging_table,
                    &request.columns,
                );
                transaction.query_drop(insert_statement)?;
                let swapped_rows = transaction.affected_rows();
                if swapped_rows != staged_rows {
                    transaction.rollback()?;
                    let message =
                        format!("暂存表有 {staged_rows} 行，切换 INSERT 只写入 {swapped_rows} 行");
                    return Ok(AtomicSwapOutcome::Failed(AtomicSwapError::Other(message)));
                }
                transaction.commit()?;
                Ok(AtomicSwapOutcome::Swapped(AtomicSwapResult {
                    staged_rows,
                    purged_rows,
                    swapped_rows,
                    count_ms,
                }))
            })
            .map_err(|error| AtomicSwapError::Other(error.to_string()))?;

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

fn build_delete_statement(database: &str, target_table: &str, target_date_col: &str) -> String {
    let quoted_date_column = quote_identifier(target_date_col);
    format!(
        "DELETE FROM {}.{} WHERE {} >= ? AND {} < ?",
        quote_identifier(database),
        quote_identifier(target_table),
        quoted_date_column,
        quoted_date_column
    )
}

fn build_swap_insert_statement(
    database: &str,
    target_table: &str,
    staging_table: &str,
    columns: &[String],
) -> String {
    let quoted_columns = columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSERT INTO {}.{} ({quoted_columns}) SELECT {quoted_columns} FROM {}.{}",
        quote_identifier(database),
        quote_identifier(target_table),
        quote_identifier(database),
        quote_identifier(staging_table)
    )
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

fn is_permission_error(code: u16) -> bool {
    matches!(code, 1044 | 1045 | 1142 | 1143 | 1227)
}

pub fn check_connection_settings(
    character_set_client: &str,
    character_set_connection: &str,
    character_set_results: &str,
    sql_mode: &str,
    max_allowed_packet: u64,
) -> Result<(), String> {
    const EXPECTED_CHARSET: &str = "utf8mb4";
    const EXPECTED_SQL_MODE: &str = "STRICT_ALL_TABLES";
    const MIN_PACKET: u64 = 64 * 1024 * 1024;

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
            "环境配置错误：max_allowed_packet 期望至少 {MIN_PACKET} 字节，实际 {max_allowed_packet} 字节；请调整 MySQL 配置，不要排查业务数据"
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
        build_delete_statement, build_insert_statement, build_swap_insert_statement,
        classify_mysql_diagnostic,
    };
    use crate::WriteBatchError;

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
    fn swap_statements_use_a_half_open_date_range_and_explicit_columns() {
        assert_eq!(
            build_delete_statement("qbs", "T_POSITION", "D_BIZ"),
            "DELETE FROM `qbs`.`T_POSITION` WHERE `D_BIZ` >= ? AND `D_BIZ` < ?"
        );
        assert_eq!(
            build_swap_insert_statement(
                "qbs",
                "T_POSITION",
                "T_POSITION__stg_run",
                &["C_SECOND".to_owned(), "C_FIRST".to_owned()],
            ),
            "INSERT INTO `qbs`.`T_POSITION` (`C_SECOND`, `C_FIRST`) SELECT `C_SECOND`, `C_FIRST` FROM `qbs`.`T_POSITION__stg_run`"
        );
    }
}
