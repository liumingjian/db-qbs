use std::fmt;
use std::sync::Mutex;

use mysql::prelude::Queryable;
use mysql::{params, Conn, Error as MysqlError, Opts, OptsBuilder};

use crate::service::quote_identifier;
use crate::{CreateStagingError, Destination, DropStagingError, SinkConfig, TargetColumn};

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
