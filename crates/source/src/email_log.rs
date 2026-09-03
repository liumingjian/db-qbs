//! 邮件事件日志的落库与增量取用。
//!
//! 日志正文沿用 source stdout 的 JSON Lines 约定：`ts`、`level`、`event`、可选的
//! `run_id`/`task`，以及事件自己的结构化字段。磁盘上保存的是同一行 JSON，管理员界面
//! 因而可以看到与后端日志一致的事实，而不是另一套重新拼出来的人话。

use std::fs::{self, OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{TimeDelta, Utc};
use db_qbs_shared::{write_log_line_with_fields, LogEvent, LogLevel};
use rusqlite::{params, Connection};
use serde::Serialize;

const DATABASE_FILE: &str = "db-qbs.sqlite3";
const EMAIL_LOG_RETENTION_DAYS: i64 = 30;
pub const EMAIL_LOG_PAGE_LIMIT: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmailLogLine {
    pub seq: i64,
    pub line: String,
}

#[derive(Debug, Clone)]
pub struct EmailLogStore {
    database_path: PathBuf,
}

impl EmailLogStore {
    pub fn open(data_dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(data_dir)
            .map_err(|error| format!("创建 source 数据目录失败：{error}"))?;
        let database_path = data_dir.join(DATABASE_FILE);
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&database_path)
            .map_err(|error| format!("创建 SQLite 库文件失败：{error}"))?;
        fs::set_permissions(&database_path, Permissions::from_mode(0o600))
            .map_err(|error| format!("设置 SQLite 库文件权限失败：{error}"))?;

        let store = Self { database_path };
        let connection = store.connection()?;
        initialize_email_log_table(&connection)?;
        Ok(store)
    }

    pub fn append<T: Serialize>(
        &self,
        level: LogLevel,
        event: LogEvent,
        run_id: Option<&str>,
        task: Option<&str>,
        fields: T,
    ) -> Result<i64, String> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("开启 SQLite 邮件日志事务失败：{error}"))?;
        let seq = insert_log_in_transaction(&transaction, level, event, run_id, task, fields)?;
        cleanup_email_log_transaction(&transaction)?;
        transaction
            .commit()
            .map_err(|error| format!("提交 SQLite 邮件日志事务失败：{error}"))?;
        Ok(seq)
    }

    pub fn lines_after(&self, after: i64) -> Result<Vec<EmailLogLine>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT seq, line FROM email_log_lines
                  WHERE seq > ?1
               ORDER BY seq ASC
                  LIMIT ?2",
            )
            .map_err(|error| format!("准备 SQLite 邮件日志查询失败：{error}"))?;
        let lines = statement
            .query_map(params![after, EMAIL_LOG_PAGE_LIMIT as i64], |row| {
                Ok(EmailLogLine {
                    seq: row.get("seq")?,
                    line: row.get("line")?,
                })
            })
            .map_err(|error| format!("查询 SQLite 邮件日志失败：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("读取 SQLite 邮件日志失败：{error}"));
        lines
    }

    fn connection(&self) -> Result<Connection, String> {
        let connection = Connection::open(&self.database_path)
            .map_err(|error| format!("打开 SQLite 邮件日志失败：{error}"))?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|error| format!("配置 SQLite 邮件日志忙等待失败：{error}"))?;
        Ok(connection)
    }
}

pub(crate) fn initialize_email_log_table(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS email_log_lines (
                seq         INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at  TEXT NOT NULL,
                line        TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS email_log_lines_created_at
                 ON email_log_lines(created_at);",
        )
        .map_err(|error| format!("初始化 SQLite 邮件日志表失败：{error}"))
}

pub(crate) fn insert_log_in_transaction<T: Serialize>(
    transaction: &rusqlite::Transaction<'_>,
    level: LogLevel,
    event: LogEvent,
    run_id: Option<&str>,
    task: Option<&str>,
    fields: T,
) -> Result<i64, String> {
    let mut bytes = Vec::new();
    write_log_line_with_fields(&mut bytes, level, event, run_id, task, fields)
        .map_err(|error| format!("生成邮件日志 JSON 失败：{error}"))?;
    let line = String::from_utf8(bytes)
        .map_err(|error| format!("生成邮件日志文本失败：{error}"))?
        .trim_end_matches('\n')
        .to_owned();
    transaction
        .execute(
            "INSERT INTO email_log_lines (created_at, line) VALUES (?1, ?2)",
            params![Utc::now().to_rfc3339(), line],
        )
        .map_err(|error| format!("写入 SQLite 邮件日志失败：{error}"))?;
    Ok(transaction.last_insert_rowid())
}

fn cleanup_email_log_transaction(transaction: &rusqlite::Transaction<'_>) -> Result<(), String> {
    let cutoff = Utc::now() - TimeDelta::days(EMAIL_LOG_RETENTION_DAYS);
    transaction
        .execute(
            "DELETE FROM email_log_lines WHERE created_at < ?1",
            [cutoff.to_rfc3339()],
        )
        .map_err(|error| format!("清理过期 SQLite 邮件日志失败：{error}"))?;
    Ok(())
}
