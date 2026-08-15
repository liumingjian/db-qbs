use std::fs::{self, OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

const DATABASE_FILE: &str = "db-qbs.sqlite3";

macro_rules! history_params {
    ($history:expr) => {
        params![
            $history.run_record_id,
            $history.run_id,
            $history.task_id,
            $history.biz_date,
            $history.staging_table,
            $history.started_at,
            $history.started_at_ms,
            $history.finished_at,
            $history.outcome,
            $history.target_table_effect,
            $history.stage,
            $history.source_rows,
            $history.staged_rows,
            $history.sink_reported_rows,
            $history.purged_rows,
            $history.source_batches,
            $history.received_batches,
            $history.fetch_ms,
            $history.push_ms,
            $history.commit_ms,
            $history.count_ms,
            $history.cursor_ms,
            $history.source_code,
            $history.sink_code,
            $history.column,
            $history.value,
            $history.message,
            $history.unknown_reason,
            $history.seq,
            $history.rows_pushed,
            $history.bytes,
            $history.ms,
            $history.last_ts,
        ]
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryChange {
    MemoryOnly,
    StageChanged,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownReason {
    ProcessDisappeared,
    ServiceRestarted,
}

impl UnknownReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::ProcessDisappeared => "PROCESS_DISAPPEARED",
            Self::ServiceRestarted => "SERVICE_RESTARTED",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::ProcessDisappeared => "进程消失，无终态日志",
            Self::ServiceRestarted => "服务重启，结局未知",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunHistory {
    pub run_record_id: String,
    pub run_id: Option<String>,
    pub task_id: String,
    pub biz_date: String,
    pub staging_table: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub outcome: Option<String>,
    pub target_table_effect: Option<String>,
    pub stage: Option<String>,
    pub source_rows: Option<u64>,
    pub staged_rows: Option<u64>,
    pub sink_reported_rows: Option<u64>,
    pub purged_rows: Option<u64>,
    pub source_batches: Option<u64>,
    pub received_batches: Option<u64>,
    pub fetch_ms: Option<u64>,
    pub push_ms: Option<u64>,
    pub commit_ms: Option<u64>,
    pub count_ms: Option<u64>,
    pub cursor_ms: Option<u64>,
    pub source_code: Option<String>,
    pub sink_code: Option<String>,
    pub column: Option<String>,
    pub value: Option<String>,
    pub message: Option<String>,
    pub unknown_reason: Option<String>,
    pub seq: u64,
    pub rows_pushed: u64,
    pub bytes: u64,
    pub ms: u64,
    pub last_ts: Option<String>,
    #[serde(skip)]
    started_at_ms: i64,
}

impl RunHistory {
    pub fn accepted(
        run_record_id: &str,
        task_id: &str,
        biz_date: &str,
        accepted_at: DateTime<Utc>,
    ) -> Self {
        Self {
            run_record_id: run_record_id.to_owned(),
            run_id: None,
            task_id: task_id.to_owned(),
            biz_date: biz_date.to_owned(),
            staging_table: None,
            started_at: accepted_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            finished_at: None,
            outcome: None,
            target_table_effect: None,
            stage: None,
            source_rows: None,
            staged_rows: None,
            sink_reported_rows: None,
            purged_rows: None,
            source_batches: None,
            received_batches: None,
            fetch_ms: None,
            push_ms: None,
            commit_ms: None,
            count_ms: None,
            cursor_ms: None,
            source_code: None,
            sink_code: None,
            column: None,
            value: None,
            message: None,
            unknown_reason: None,
            seq: 0,
            rows_pushed: 0,
            bytes: 0,
            ms: 0,
            last_ts: None,
            started_at_ms: accepted_at.timestamp_millis(),
        }
    }

    pub fn apply_log(&mut self, log: &Value) -> HistoryChange {
        let event = log.get("event").and_then(Value::as_str);
        let line_run_id = log.get("run_id").and_then(Value::as_str);
        if let (Some(existing), Some(observed)) = (self.run_id.as_deref(), line_run_id) {
            if existing != observed {
                return HistoryChange::MemoryOnly;
            }
        }
        if self.run_id.is_none() {
            self.run_id = line_run_id.map(str::to_owned);
        }
        if let Some(ts) = text(log, "ts") {
            self.last_ts = Some(ts.to_owned());
        }

        match event {
            Some("source_started") => {
                if let Some(biz_date) = text(log, "biz_date") {
                    self.biz_date = biz_date.to_owned();
                }
                if let Some(ts) = text(log, "ts") {
                    self.started_at = ts.to_owned();
                    if let Ok(parsed) = DateTime::parse_from_rfc3339(ts) {
                        self.started_at_ms = parsed.timestamp_millis();
                    }
                }
                HistoryChange::MemoryOnly
            }
            Some("stage_changed") => {
                self.stage = optional_text(log, "stage");
                HistoryChange::StageChanged
            }
            Some("run_opened") => {
                self.staging_table = optional_text(log, "staging_table");
                HistoryChange::MemoryOnly
            }
            Some("batch_pushed") => {
                if let Some(seq) = number(log, "seq") {
                    self.seq = seq;
                }
                self.rows_pushed += number(log, "rows").unwrap_or(0);
                self.bytes += number(log, "bytes").unwrap_or(0);
                self.ms += number(log, "ms").unwrap_or(0);
                HistoryChange::MemoryOnly
            }
            Some("commit_diagnosed") => {
                self.target_table_effect = match text(log, "terminal") {
                    Some("SWAPPED") => Some("SWAPPED".to_owned()),
                    Some("DISCARDED") => Some("DISCARDED".to_owned()),
                    _ => Some("UNKNOWN".to_owned()),
                };
                HistoryChange::MemoryOnly
            }
            Some("run_finished") => {
                self.finish_from_log(log);
                HistoryChange::Terminal
            }
            Some(
                "business_date_invalid"
                | "source_config_failed"
                | "task_config_failed"
                | "sql_shape_precheck_failed",
            ) => {
                self.outcome = Some("FAILED".to_owned());
                self.target_table_effect = Some("DISCARDED".to_owned());
                self.finished_at = optional_text(log, "ts");
                self.message = optional_text(log, "message");
                self.value = optional_text(log, "value");
                HistoryChange::Terminal
            }
            _ => HistoryChange::MemoryOnly,
        }
    }

    pub fn mark_unknown(&mut self, reason: UnknownReason, at: DateTime<Utc>) {
        self.outcome = Some("FAILED".to_owned());
        self.target_table_effect = None;
        self.finished_at = Some(at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        self.source_code = None;
        self.sink_code = None;
        self.column = None;
        self.value = None;
        self.message = Some(reason.message().to_owned());
        self.unknown_reason = Some(reason.as_str().to_owned());
    }

    pub fn mark_parent_failure(&mut self, message: String, at: DateTime<Utc>) {
        self.outcome = Some("FAILED".to_owned());
        self.target_table_effect = Some("DISCARDED".to_owned());
        self.finished_at = Some(at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        self.message = Some(message);
    }

    fn finish_from_log(&mut self, log: &Value) {
        self.outcome = optional_text(log, "terminal");
        self.stage = optional_text(log, "stage");
        self.finished_at = optional_text(log, "ts");
        if self.target_table_effect.is_none() {
            self.target_table_effect = match (self.outcome.as_deref(), self.stage.as_deref()) {
                (Some("SUCCEEDED"), _) => Some("SWAPPED".to_owned()),
                (Some("FAILED"), Some("COMMITTING")) => Some("UNKNOWN".to_owned()),
                (Some("FAILED"), _) => Some("DISCARDED".to_owned()),
                _ => None,
            };
        }
        self.source_rows = number(log, "source_rows");
        self.staged_rows = number(log, "staged_rows");
        self.sink_reported_rows = number(log, "sink_reported_rows");
        self.purged_rows = number(log, "purged_rows");
        self.source_batches = number(log, "source_batches");
        self.received_batches = number(log, "received_batches");
        self.fetch_ms = number(log, "fetch_ms");
        self.push_ms = number(log, "push_ms");
        self.commit_ms = number(log, "commit_ms");
        self.count_ms = number(log, "count_ms");
        self.cursor_ms = number(log, "cursor_ms");
        self.source_code = optional_text(log, "source_code");
        self.sink_code = optional_text(log, "sink_code");
        self.column = optional_text(log, "column");
        self.value = optional_text(log, "value");
        self.message = optional_text(log, "message");
    }
}

#[derive(Debug, Clone)]
pub struct HistoryStore {
    database_path: PathBuf,
}

impl HistoryStore {
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
                "CREATE TABLE IF NOT EXISTS run_history (
                    run_record_id       TEXT PRIMARY KEY NOT NULL,
                    run_id              TEXT,
                    task_id             TEXT NOT NULL,
                    biz_date            TEXT NOT NULL,
                    staging_table       TEXT,
                    started_at          TEXT NOT NULL,
                    started_at_ms       INTEGER NOT NULL,
                    finished_at         TEXT,
                    outcome             TEXT,
                    target_table_effect TEXT,
                    stage               TEXT,
                    source_rows         INTEGER,
                    staged_rows         INTEGER,
                    sink_reported_rows  INTEGER,
                    purged_rows          INTEGER,
                    source_batches      INTEGER,
                    received_batches    INTEGER,
                    fetch_ms            INTEGER,
                    push_ms             INTEGER,
                    commit_ms           INTEGER,
                    count_ms            INTEGER,
                    cursor_ms           INTEGER,
                    source_code         TEXT,
                    sink_code           TEXT,
                    [column]            TEXT,
                    [value]             TEXT,
                    message             TEXT,
                    unknown_reason      TEXT,
                    seq                 INTEGER NOT NULL,
                    rows_pushed         INTEGER NOT NULL,
                    bytes               INTEGER NOT NULL,
                    ms                  INTEGER NOT NULL,
                    last_ts             TEXT
                );
                 CREATE INDEX IF NOT EXISTS run_history_task_date
                     ON run_history(task_id, biz_date, started_at_ms);",
            )
            .map_err(|error| format!("初始化 SQLite 运行历史表失败：{error}"))?;
        Ok(store)
    }

    pub fn insert(
        &self,
        history: &RunHistory,
        now: DateTime<Utc>,
        retention_days: u64,
    ) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("开启 SQLite 运行历史事务失败：{error}"))?;
        transaction
            .execute(
                "INSERT INTO run_history (
                    run_record_id, run_id, task_id, biz_date, staging_table, started_at,
                    started_at_ms, finished_at, outcome, target_table_effect, stage,
                    source_rows, staged_rows, sink_reported_rows, purged_rows, source_batches,
                    received_batches, fetch_ms, push_ms, commit_ms, count_ms, cursor_ms,
                    source_code, sink_code, [column], [value], message, unknown_reason,
                    seq, rows_pushed, bytes, ms, last_ts
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                    ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28,
                    ?29, ?30, ?31, ?32, ?33
                 )",
                history_params!(history),
            )
            .map_err(|error| format!("插入 SQLite 运行历史失败：{error}"))?;
        cleanup_transaction(&transaction, now, retention_days)?;
        transaction
            .commit()
            .map_err(|error| format!("提交 SQLite 运行历史事务失败：{error}"))
    }

    pub fn save(
        &self,
        history: &RunHistory,
        now: DateTime<Utc>,
        retention_days: u64,
    ) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("开启 SQLite 运行历史事务失败：{error}"))?;
        transaction
            .execute(
                "UPDATE run_history SET
                    run_id=?2, task_id=?3, biz_date=?4, staging_table=?5, started_at=?6,
                    started_at_ms=?7, finished_at=?8, outcome=?9, target_table_effect=?10,
                    stage=?11, source_rows=?12, staged_rows=?13, sink_reported_rows=?14,
                    purged_rows=?15, source_batches=?16, received_batches=?17, fetch_ms=?18,
                    push_ms=?19, commit_ms=?20, count_ms=?21, cursor_ms=?22, source_code=?23,
                    sink_code=?24, [column]=?25, [value]=?26, message=?27,
                    unknown_reason=?28, seq=?29, rows_pushed=?30, bytes=?31, ms=?32,
                    last_ts=?33
                  WHERE run_record_id=?1",
                history_params!(history),
            )
            .map_err(|error| format!("更新 SQLite 运行历史失败：{error}"))?;
        cleanup_transaction(&transaction, now, retention_days)?;
        transaction
            .commit()
            .map_err(|error| format!("提交 SQLite 运行历史事务失败：{error}"))
    }

    pub fn get(&self, run_record_id: &str) -> Result<Option<RunHistory>, String> {
        self.connection()?
            .query_row(
                &format!("{HISTORY_SELECT} WHERE run_record_id = ?1"),
                [run_record_id],
                history_from_row,
            )
            .optional()
            .map_err(|error| format!("查询 SQLite 运行历史失败：{error}"))
    }

    pub fn list(
        &self,
        task_id: Option<&str>,
        biz_date: Option<&str>,
    ) -> Result<Vec<RunHistory>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(&format!(
                "{HISTORY_SELECT}
                  WHERE (?1 IS NULL OR task_id = ?1)
                    AND (?2 IS NULL OR biz_date = ?2)
               ORDER BY started_at_ms DESC, rowid DESC"
            ))
            .map_err(|error| format!("准备 SQLite 运行历史列表查询失败：{error}"))?;
        let history = statement
            .query_map(params![task_id, biz_date], history_from_row)
            .map_err(|error| format!("查询 SQLite 运行历史列表失败：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("读取 SQLite 运行历史列表失败：{error}"))?;
        Ok(history)
    }

    pub fn seal_incomplete(
        &self,
        reason: UnknownReason,
        now: DateTime<Utc>,
        retention_days: u64,
    ) -> Result<(), String> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("开启 SQLite 启动清扫事务失败：{error}"))?;
        let finished_at = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        transaction
            .execute(
                "UPDATE run_history
                    SET outcome = 'FAILED', target_table_effect = NULL, finished_at = ?1,
                        source_code = NULL, sink_code = NULL, [column] = NULL,
                        [value] = NULL, message = ?2, unknown_reason = ?3
                  WHERE outcome IS NULL",
                params![finished_at, reason.message(), reason.as_str()],
            )
            .map_err(|error| format!("封口 SQLite 非终态运行历史失败：{error}"))?;
        cleanup_transaction(&transaction, now, retention_days)?;
        transaction
            .commit()
            .map_err(|error| format!("提交 SQLite 启动清扫事务失败：{error}"))
    }

    fn connection(&self) -> Result<Connection, String> {
        let connection = Connection::open(&self.database_path)
            .map_err(|error| format!("打开 SQLite 运行历史失败：{error}"))?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|error| format!("配置 SQLite 忙等待失败：{error}"))?;
        Ok(connection)
    }
}

pub fn expired_history_indices(
    timestamps: &[DateTime<Utc>],
    now: DateTime<Utc>,
    retention_days: u64,
) -> Vec<usize> {
    let Some(cutoff) = retention_cutoff(now, retention_days) else {
        return Vec::new();
    };
    timestamps
        .iter()
        .enumerate()
        .filter_map(|(index, timestamp)| (*timestamp < cutoff).then_some(index))
        .collect()
}

pub fn fold_history_lines(
    run_record_id: &str,
    task_id: &str,
    biz_date: &str,
    accepted_at: DateTime<Utc>,
    lines: &[&str],
) -> Result<RunHistory, String> {
    let mut history = RunHistory::accepted(run_record_id, task_id, biz_date, accepted_at);
    for line in lines {
        let log: Value =
            serde_json::from_str(line).map_err(|error| format!("运行日志 JSON 无效：{error}"))?;
        history.apply_log(&log);
    }
    Ok(history)
}

fn cleanup_transaction(
    transaction: &rusqlite::Transaction<'_>,
    now: DateTime<Utc>,
    retention_days: u64,
) -> Result<(), String> {
    let Some(cutoff) = retention_cutoff(now, retention_days) else {
        return Ok(());
    };
    transaction
        .execute(
            "DELETE FROM run_history WHERE started_at_ms < ?1",
            [cutoff.timestamp_millis()],
        )
        .map_err(|error| format!("清理过期 SQLite 运行历史失败：{error}"))?;
    Ok(())
}

fn retention_cutoff(now: DateTime<Utc>, retention_days: u64) -> Option<DateTime<Utc>> {
    let days = i64::try_from(retention_days).ok()?;
    now.checked_sub_signed(TimeDelta::try_days(days)?)
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn optional_text(value: &Value, key: &str) -> Option<String> {
    text(value, key).map(str::to_owned)
}

fn number(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

const HISTORY_SELECT: &str = "SELECT
    run_record_id, run_id, task_id, biz_date, staging_table, started_at, started_at_ms,
    finished_at, outcome, target_table_effect, stage, source_rows, staged_rows,
    sink_reported_rows, purged_rows, source_batches, received_batches, fetch_ms, push_ms,
    commit_ms, count_ms, cursor_ms, source_code, sink_code, [column], [value],
    message, unknown_reason, seq, rows_pushed, bytes, ms, last_ts
  FROM run_history";

fn history_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunHistory> {
    Ok(RunHistory {
        run_record_id: row.get(0)?,
        run_id: row.get(1)?,
        task_id: row.get(2)?,
        biz_date: row.get(3)?,
        staging_table: row.get(4)?,
        started_at: row.get(5)?,
        started_at_ms: row.get(6)?,
        finished_at: row.get(7)?,
        outcome: row.get(8)?,
        target_table_effect: row.get(9)?,
        stage: row.get(10)?,
        source_rows: row.get(11)?,
        staged_rows: row.get(12)?,
        sink_reported_rows: row.get(13)?,
        purged_rows: row.get(14)?,
        source_batches: row.get(15)?,
        received_batches: row.get(16)?,
        fetch_ms: row.get(17)?,
        push_ms: row.get(18)?,
        commit_ms: row.get(19)?,
        count_ms: row.get(20)?,
        cursor_ms: row.get(21)?,
        source_code: row.get(22)?,
        sink_code: row.get(23)?,
        column: row.get(24)?,
        value: row.get(25)?,
        message: row.get(26)?,
        unknown_reason: row.get(27)?,
        seq: row.get(28)?,
        rows_pushed: row.get(29)?,
        bytes: row.get(30)?,
        ms: row.get(31)?,
        last_ts: row.get(32)?,
    })
}
