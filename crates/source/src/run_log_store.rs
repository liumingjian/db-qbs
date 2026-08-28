//! 原始运行日志行的落库与增量取用。
//!
//! 子进程吐出的每一行都会先被父进程折叠进一行运行历史，然后**当场丢掉**。
//! 折叠是有损的：认不出来的事件按定义被忽略，一次运行卡在哪一步、上一句说了什么，
//! 折叠完的那一行答不出来。这个模块存的就是折叠之前的那份原文。
//!
//! 几条刻意的取舍：
//!
//! * **原文照存**。存进去的是子进程写出来的那一行字符串，不是解析后的结构，
//!   也不是渲染成人话的句子。进程间的 JSON Lines 契约不动（那是父子两端的协议），
//!   翻成人话是展示层的事。连解析不出 JSON 的行也照存——「来什么显什么，不吞」。
//! * **与运行历史同库**（`db-qbs.sqlite3`，同一份 0600），按 `run_record_id` 相关联。
//!   同库的理由是运行日志与运行历史生死与共：历史行没了，日志行也就没有主语了。
//! * **连接每次现开**，与 [`crate::HistoryStore`] 同一条路子而不是 `Mutex<Connection>`：
//!   写日志的是每条运行各自的监督线程，读日志的是 HTTP 工作线程，两边都要并发；
//!   一把进程级的锁会让一条运行的日志洪流卡住别人的查询。
//! * **随写清理**，不起后台任务。清理与插入同一个事务，理由与运行历史那一份一样：
//!   一个只在写的时候才醒的程序不需要一个永远醒着的线程来收垃圾。

use std::fs::{self, OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;

const DATABASE_FILE: &str = "db-qbs.sqlite3";

/// 原始日志行按天保留的上限。**比运行历史（默认 90 天）严得多，是故意的**：
/// 历史那一行是台账，原文是排障用的草稿纸；草稿纸的价值几天之内就掉没了，
/// 而它携带的业务值一天不删就一天在盘上。
pub const RUN_LOG_RETENTION_DAYS: u64 = 7;

/// 每条任务保留原始日志的运行次数上限。与 7 天**两者取严**：
/// 一条一天跑几十次的任务，7 天能堆出上千次运行的原文；
/// 一条一月跑一次的任务，光靠次数则会把半年前的原文一直留着。
pub const RUN_LOG_RETENTION_RUNS_PER_TASK: u64 = 10;

/// 业务值落库前截断到的字符数。**足以判断是哪一列出了问题，不足以充当一份数据副本**。
pub const BUSINESS_VALUE_MAX_CHARS: usize = 64;

/// 一次增量查询最多返回多少行。超出的部分靠客户端带着新游标再来一次——
/// 这是**游标增量轮询**，不是长连接：后端是同步阻塞栈，一条长连接会占死一个工作线程。
pub const RUN_LOG_PAGE_LIMIT: usize = 500;

/// 一条落库的原始日志行。`seq` 是这条运行内部从 1 开始的序号，也就是游标本身。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunLogLine {
    pub seq: i64,
    pub line: String,
}

/// 原始运行日志表的门。
#[derive(Debug, Clone)]
pub struct RunLogStore {
    database_path: PathBuf,
}

impl RunLogStore {
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
        connection
            .execute_batch(
                // `started_at_ms` 与 `task_id` 是从运行历史那边抄过来的冗余：
                // 保留期两条规则都按「运行」为单位裁，而运行历史的保留期（90 天）
                // 比这里长得多，去 join 它等于把两份互不相干的保留期绑死。
                "CREATE TABLE IF NOT EXISTS run_log_lines (
                    run_record_id  TEXT NOT NULL,
                    seq            INTEGER NOT NULL,
                    task_id        TEXT NOT NULL,
                    started_at_ms  INTEGER NOT NULL,
                    line           TEXT NOT NULL,
                    PRIMARY KEY (run_record_id, seq)
                 );
                 CREATE INDEX IF NOT EXISTS run_log_lines_task_started
                     ON run_log_lines(task_id, started_at_ms);",
            )
            .map_err(|error| format!("初始化 SQLite 运行日志表失败：{error}"))?;
        Ok(store)
    }

    /// 追加一行原文，并在**同一个事务里**把过期的行清掉。
    ///
    /// `seq` 由调用方给：一条运行只有一个监督线程在写，本地计数器比
    /// `SELECT MAX(seq)+1` 少一次查询，也不会因为清理而重号。
    pub fn append(
        &self,
        run_record_id: &str,
        task_id: &str,
        started_at_ms: i64,
        seq: i64,
        line: &str,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        let stored = truncate_business_values(line);
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("开启 SQLite 运行日志事务失败：{error}"))?;
        transaction
            .execute(
                "INSERT OR REPLACE INTO run_log_lines
                    (run_record_id, seq, task_id, started_at_ms, line)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![run_record_id, seq, task_id, started_at_ms, stored],
            )
            .map_err(|error| format!("插入 SQLite 运行日志失败：{error}"))?;
        cleanup_transaction(&transaction, now)?;
        transaction
            .commit()
            .map_err(|error| format!("提交 SQLite 运行日志事务失败：{error}"))
    }

    /// 取 `after` 之后的行，最多 [`RUN_LOG_PAGE_LIMIT`] 条，按序号升序。
    ///
    /// `after = 0` 就是「从头开始」。运行进行中与已结束走的是同一条路：
    /// 这里只认表里的行，不问那条运行是不是还活着。
    pub fn lines_after(&self, run_record_id: &str, after: i64) -> Result<Vec<RunLogLine>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT seq, line FROM run_log_lines
                  WHERE run_record_id = ?1 AND seq > ?2
               ORDER BY seq ASC
                  LIMIT ?3",
            )
            .map_err(|error| format!("准备 SQLite 运行日志查询失败：{error}"))?;
        let lines = statement
            .query_map(
                params![run_record_id, after, RUN_LOG_PAGE_LIMIT as i64],
                |row| {
                    Ok(RunLogLine {
                        seq: row.get("seq")?,
                        line: row.get("line")?,
                    })
                },
            )
            .map_err(|error| format!("查询 SQLite 运行日志失败：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("读取 SQLite 运行日志失败：{error}"))?;
        Ok(lines)
    }

    fn connection(&self) -> Result<Connection, String> {
        let connection = Connection::open(&self.database_path)
            .map_err(|error| format!("打开 SQLite 运行日志失败：{error}"))?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|error| format!("配置 SQLite 忙等待失败：{error}"))?;
        Ok(connection)
    }
}

/// 一条运行的**日志笔**：这条运行是谁、下一行该是第几号，都攥在它自己手里。
///
/// 存在的理由有两条。一是行号：一条运行只有一个监督线程在写，把计数器放在写的人身上，
/// 比每写一行回表问一次 `MAX(seq)` 既准又便宜。二是**错误在这里被吞掉**——
/// 排障用的原文存不进去，不能反过来连累这次搬运本身；这个判断只在这一个地方做，
/// 监督线程那边不必每行都写一遍 `let _ =`。
pub struct RunLogWriter {
    store: RunLogStore,
    run_record_id: String,
    task_id: String,
    started_at_ms: i64,
    seq: i64,
}

impl RunLogWriter {
    pub fn new(
        store: RunLogStore,
        run_record_id: String,
        task_id: String,
        started_at_ms: i64,
    ) -> Self {
        Self {
            store,
            run_record_id,
            task_id,
            started_at_ms,
            seq: 0,
        }
    }

    /// 记下一行原文。行号从 1 起，就是接口那边的游标。
    pub fn write(&mut self, line: &str) {
        self.seq += 1;
        let _ = self.store.append(
            &self.run_record_id,
            &self.task_id,
            self.started_at_ms,
            self.seq,
            line,
            Utc::now(),
        );
    }
}

/// 把一行原文里的业务值截到 [`BUSINESS_VALUE_MAX_CHARS`] 个字符。
///
/// 只动 `value` 一个字段：失败行上的 `column` 是列名、`source` / `target` 是类型，
/// 都不是从源库里采出来的数据，只有 `value` 是。截断过的行会多带一个
/// `value_truncated: true`——**宁可多说一句「这里被截过」，也不要让展示层
/// 把半截值当成完整值念出来**。字段只增不减，加一个标记不违反那条契约。
///
/// 解析不出 JSON 的行原样返回：那种行也照存，判断它是什么不是这里的职责。
pub fn truncate_business_values(line: &str) -> String {
    let Ok(mut parsed) = serde_json::from_str::<Value>(line) else {
        return line.to_owned();
    };
    let Some(object) = parsed.as_object_mut() else {
        return line.to_owned();
    };
    let Some(value) = object.get("value").and_then(Value::as_str) else {
        return line.to_owned();
    };
    if value.chars().count() <= BUSINESS_VALUE_MAX_CHARS {
        return line.to_owned();
    }
    let truncated: String = value.chars().take(BUSINESS_VALUE_MAX_CHARS).collect();
    object.insert("value".to_owned(), Value::String(truncated));
    object.insert("value_truncated".to_owned(), Value::Bool(true));
    serde_json::to_string(&parsed).unwrap_or_else(|_| line.to_owned())
}

/// 保留期按「7 天」与「每任务最近 10 次运行」**两者取严**，两条各写成一条 DELETE。
fn cleanup_transaction(
    transaction: &rusqlite::Transaction<'_>,
    now: DateTime<Utc>,
) -> Result<(), String> {
    if let Some(cutoff) = retention_cutoff(now, RUN_LOG_RETENTION_DAYS) {
        transaction
            .execute(
                "DELETE FROM run_log_lines WHERE started_at_ms < ?1",
                [cutoff.timestamp_millis()],
            )
            .map_err(|error| format!("清理过期 SQLite 运行日志失败：{error}"))?;
    }
    // 写成 `IN (排在第 10 名之后的运行)` 而不是 `NOT IN (前 10 名)`：
    // 后者在子查询为空时会把整张表删光，而这里的正确答案恰恰是「一行都不删」。
    transaction
        .execute(
            "DELETE FROM run_log_lines
              WHERE run_record_id IN (
                SELECT run_record_id FROM (
                  SELECT run_record_id,
                         ROW_NUMBER() OVER (
                           PARTITION BY task_id
                           ORDER BY started_at_ms DESC, run_record_id DESC
                         ) AS position
                    FROM (SELECT DISTINCT run_record_id, task_id, started_at_ms
                            FROM run_log_lines)
                ) WHERE position > ?1
              )",
            [RUN_LOG_RETENTION_RUNS_PER_TASK as i64],
        )
        .map_err(|error| format!("清理超量 SQLite 运行日志失败：{error}"))?;
    Ok(())
}

fn retention_cutoff(now: DateTime<Utc>, retention_days: u64) -> Option<DateTime<Utc>> {
    let days = i64::try_from(retention_days).ok()?;
    now.checked_sub_signed(TimeDelta::try_days(days)?)
}
