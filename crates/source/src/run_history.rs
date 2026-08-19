use std::fs::{self, OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use rusqlite::{named_params, params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

use crate::{FailureKind, RunParams};

const DATABASE_FILE: &str = "db-qbs.sqlite3";

macro_rules! history_params {
    ($history:expr, $run_params:expr, $mapping_issues:expr) => {
        named_params! {
            ":run_record_id": $history.run_record_id,
            ":run_id": $history.run_id,
            ":task_id": $history.task_id,
            ":run_params": $run_params,
            ":staging_table": $history.staging_table,
            ":started_at": $history.started_at,
            ":started_at_ms": $history.started_at_ms,
            ":finished_at": $history.finished_at,
            ":outcome": $history.outcome,
            ":target_table_effect": $history.target_table_effect,
            ":stage": $history.stage,
            ":source_rows": $history.source_rows,
            ":staged_rows": $history.staged_rows,
            ":sink_reported_rows": $history.sink_reported_rows,
            ":purged_rows": $history.purged_rows,
            ":source_batches": $history.source_batches,
            ":received_batches": $history.received_batches,
            ":fetch_ms": $history.fetch_ms,
            ":push_ms": $history.push_ms,
            ":commit_ms": $history.commit_ms,
            ":count_ms": $history.count_ms,
            ":cursor_ms": $history.cursor_ms,
            ":source_code": $history.source_code,
            ":sink_code": $history.sink_code,
            ":column": $history.column,
            ":value": $history.value,
            ":message": $history.message,
            ":unknown_reason": $history.unknown_reason,
            ":failure_kind": $history.failure_kind,
            ":seq": $history.seq,
            ":rows_pushed": $history.rows_pushed,
            ":bytes": $history.bytes,
            ":ms": $history.ms,
            ":last_ts": $history.last_ts,
            ":source_sql": $history.source_sql,
            ":mapping_issues": $mapping_issues,
        }
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
    /// 本次运行的参数集：参数名 → 值，按参数名排序（ADR-0036 §7 的规范形式）。
    /// 它同时是并发互斥键的后半段与历史里那一列的展示内容；无参数时是空 map，
    /// 互斥键随之退化成「同任务不许并跑」。
    pub run_params: RunParams,
    /// 当次**实际执行**的源端 SQL 快照（ADR-0036 §2）。
    ///
    /// 它与任务定义现算的那份性质根本不同：这份回答「当时执行了什么」，是审计事实，
    /// 规格改了它也不能跟着变。存的是**未绑定的语句文本**，参数值不内联——
    /// 值由 `run_params` 那一列负责展示，内联会让同一份事实在两处重复。
    pub source_sql: String,
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
    /// 失败分类（[`crate::FailureKind`] 的 `as_str`）。成功、进行中、以及 M2 之前落盘的
    /// 老历史行都是 `None`——消费者读到缺席不得报错（与 ADR-0017 §2 `component` 同一口径）。
    pub failure_kind: Option<String>,
    pub seq: u64,
    pub rows_pushed: u64,
    pub bytes: u64,
    pub ms: u64,
    pub last_ts: Option<String>,
    pub mapping_issues: Vec<Value>,
    #[serde(skip)]
    started_at_ms: i64,
}

impl RunHistory {
    pub fn accepted(
        run_record_id: &str,
        task_id: &str,
        run_params: RunParams,
        source_sql: &str,
        accepted_at: DateTime<Utc>,
    ) -> Self {
        Self {
            run_record_id: run_record_id.to_owned(),
            run_id: None,
            task_id: task_id.to_owned(),
            run_params,
            source_sql: source_sql.to_owned(),
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
            failure_kind: None,
            seq: 0,
            rows_pushed: 0,
            bytes: 0,
            ms: 0,
            last_ts: None,
            mapping_issues: Vec::new(),
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
                if let Some(ts) = text(log, "ts") {
                    self.started_at = ts.to_owned();
                    if let Ok(parsed) = DateTime::parse_from_rfc3339(ts) {
                        self.started_at_ms = parsed.timestamp_millis();
                    }
                }
                HistoryChange::MemoryOnly
            }
            Some("stage_changed") => {
                self.stage = owned_text(log, "stage");
                HistoryChange::StageChanged
            }
            Some("run_opened") => {
                self.staging_table = owned_text(log, "staging_table");
                HistoryChange::MemoryOnly
            }
            Some("mapping_precheck_failed") => {
                self.mapping_issues.push(serde_json::json!({
                    "column": log.get("column").cloned().unwrap_or(Value::Null),
                    "source": log.get("source").cloned().unwrap_or(Value::Null),
                    "target": log.get("target").cloned().unwrap_or(Value::Null),
                    "rule": log.get("rule").cloned().unwrap_or(Value::Null),
                    "message": log.get("message").cloned().unwrap_or(Value::Null),
                    "suggestion": log.get("suggestion").cloned().unwrap_or(Value::Null),
                }));
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
                self.finished_at = owned_text(log, "ts");
                self.message = owned_text(log, "message");
                self.value = owned_text(log, "value");
                self.failure_kind = owned_text(log, "failure_kind");
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
        self.failure_kind = Some(FailureKind::Unknown.as_str().to_owned());
    }

    pub fn mark_parent_failure(&mut self, message: String, at: DateTime<Utc>) {
        self.outcome = Some("FAILED".to_owned());
        self.target_table_effect = Some("DISCARDED".to_owned());
        self.finished_at = Some(at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        self.message = Some(message);
        self.failure_kind = Some(FailureKind::Orchestrator.as_str().to_owned());
    }

    fn finish_from_log(&mut self, log: &Value) {
        self.outcome = owned_text(log, "terminal");
        self.stage = owned_text(log, "stage");
        self.finished_at = owned_text(log, "ts");
        if self.target_table_effect.is_none() {
            self.target_table_effect = match (
                self.outcome.as_deref(),
                self.stage.as_deref(),
                text(log, "sink_code"),
            ) {
                (Some("SUCCEEDED"), _, _) => Some("SWAPPED".to_owned()),
                (Some("FAILED"), _, Some("VERIFY_FAILED")) => Some("DISCARDED".to_owned()),
                (Some("FAILED"), Some("COMMITTING"), _) => Some("UNKNOWN".to_owned()),
                (Some("FAILED"), _, _) => Some("DISCARDED".to_owned()),
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
        self.source_code = owned_text(log, "source_code");
        self.sink_code = owned_text(log, "sink_code");
        self.column = owned_text(log, "column");
        self.value = owned_text(log, "value");
        self.message = owned_text(log, "message");
        self.failure_kind = owned_text(log, "failure_kind");
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
        drop_legacy_history_table(&connection)?;
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS run_history (
                    run_record_id       TEXT PRIMARY KEY NOT NULL,
                    run_id              TEXT,
                    task_id             TEXT NOT NULL,
                    run_params          TEXT NOT NULL DEFAULT '{}',
                    source_sql          TEXT NOT NULL DEFAULT '',
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
                    failure_kind        TEXT,
                    seq                 INTEGER NOT NULL,
                    rows_pushed         INTEGER NOT NULL,
                    bytes               INTEGER NOT NULL,
                    ms                  INTEGER NOT NULL,
                    last_ts             TEXT,
                    mapping_issues      TEXT NOT NULL DEFAULT '[]'
                );
                 CREATE INDEX IF NOT EXISTS run_history_task_started
                     ON run_history(task_id, started_at_ms);",
            )
            .map_err(|error| format!("初始化 SQLite 运行历史表失败：{error}"))?;
        ensure_json_column(&connection, "mapping_issues")?;
        ensure_nullable_text_column(&connection, "failure_kind")?;
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
        let run_params = run_params_text(&history.run_params)?;
        let mapping_issues = json_array_text(&history.mapping_issues)?;
        transaction
            .execute(
                "INSERT INTO run_history (
                    run_record_id, run_id, task_id, run_params, source_sql, staging_table,
                    started_at, started_at_ms, finished_at, outcome, target_table_effect, stage,
                    source_rows, staged_rows, sink_reported_rows, purged_rows, source_batches,
                    received_batches, fetch_ms, push_ms, commit_ms, count_ms, cursor_ms,
                    source_code, sink_code, [column], [value], message, unknown_reason,
                    failure_kind, seq, rows_pushed, bytes, ms, last_ts, mapping_issues
                 ) VALUES (
                    :run_record_id, :run_id, :task_id, :run_params, :source_sql, :staging_table,
                    :started_at, :started_at_ms, :finished_at, :outcome, :target_table_effect,
                    :stage, :source_rows, :staged_rows, :sink_reported_rows, :purged_rows,
                    :source_batches, :received_batches, :fetch_ms, :push_ms, :commit_ms,
                    :count_ms, :cursor_ms, :source_code, :sink_code, :column, :value,
                    :message, :unknown_reason, :failure_kind, :seq, :rows_pushed, :bytes, :ms,
                    :last_ts, :mapping_issues
                 )",
                history_params!(history, run_params, mapping_issues),
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
        let run_params = run_params_text(&history.run_params)?;
        let mapping_issues = json_array_text(&history.mapping_issues)?;
        transaction
            .execute(
                "UPDATE run_history SET
                    run_id=:run_id, task_id=:task_id, run_params=:run_params,
                    source_sql=:source_sql,
                    staging_table=:staging_table, started_at=:started_at,
                    started_at_ms=:started_at_ms, finished_at=:finished_at, outcome=:outcome,
                    target_table_effect=:target_table_effect, stage=:stage,
                    source_rows=:source_rows, staged_rows=:staged_rows,
                    sink_reported_rows=:sink_reported_rows, purged_rows=:purged_rows,
                    source_batches=:source_batches, received_batches=:received_batches,
                    fetch_ms=:fetch_ms, push_ms=:push_ms, commit_ms=:commit_ms,
                    count_ms=:count_ms, cursor_ms=:cursor_ms, source_code=:source_code,
                    sink_code=:sink_code, [column]=:column, [value]=:value, message=:message,
                    unknown_reason=:unknown_reason, failure_kind=:failure_kind, seq=:seq,
                    rows_pushed=:rows_pushed,
                    bytes=:bytes, ms=:ms, last_ts=:last_ts,
                    mapping_issues=:mapping_issues
                  WHERE run_record_id=:run_record_id",
                history_params!(history, run_params, mapping_issues),
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

    /// 按业务日期筛选随「业务日期」这个一等概念一起退役（ADR-0035 §3）：
    /// 运行参数是任务自定义的名字，筛不出一个通用维度来。只留按任务筛。
    pub fn list(&self, task_id: Option<&str>) -> Result<Vec<RunHistory>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(&format!(
                "{HISTORY_SELECT}
                  WHERE (?1 IS NULL OR task_id = ?1)
               ORDER BY started_at_ms DESC, rowid DESC"
            ))
            .map_err(|error| format!("准备 SQLite 运行历史列表查询失败：{error}"))?;
        let history = statement
            .query_map(params![task_id], history_from_row)
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
                        [value] = NULL, message = ?2, unknown_reason = ?3,
                        failure_kind = ?4
                  WHERE outcome IS NULL",
                params![
                    finished_at,
                    reason.message(),
                    reason.as_str(),
                    FailureKind::Unknown.as_str()
                ],
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
    run_params: RunParams,
    source_sql: &str,
    accepted_at: DateTime<Utc>,
    lines: &[&str],
) -> Result<RunHistory, String> {
    let mut history =
        RunHistory::accepted(run_record_id, task_id, run_params, source_sql, accepted_at);
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

/// 老历史表整表丢弃，与任务定义同一条理由（ADR-0036 §4，前提是第一版尚无真实用户数据）。
///
/// 它按 `biz_date` 一列存筛选维度、按 `shape_checks` 存已退役的形状预检结果，
/// 两者在新模型里都没有对应物；把 `biz_date` 翻成一个参数名不明的运行参数是**编事实**，
/// 而历史那份数据的全部价值就在于它是事实。
fn drop_legacy_history_table(connection: &Connection) -> Result<(), String> {
    let has_biz_date: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('run_history') WHERE name = 'biz_date')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("检查 SQLite 运行历史表失败：{error}"))?;
    if !has_biz_date {
        return Ok(());
    }
    connection
        .execute("DROP TABLE run_history", [])
        .map_err(|error| format!("丢弃旧 SQLite 运行历史表失败：{error}"))?;
    Ok(())
}

fn run_params_text(run_params: &RunParams) -> Result<String, String> {
    serde_json::to_string(run_params).map_err(|error| format!("序列化运行参数失败：{error}"))
}

fn decode_run_params(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunParams> {
    let encoded: String = row.get("run_params")?;
    serde_json::from_str(&encoded).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            row.as_ref()
                .column_index("run_params")
                .expect("the run_params column was just read"),
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn retention_cutoff(now: DateTime<Utc>, retention_days: u64) -> Option<DateTime<Utc>> {
    let days = i64::try_from(retention_days).ok()?;
    now.checked_sub_signed(TimeDelta::try_days(days)?)
}

fn ensure_json_column(connection: &Connection, name: &str) -> Result<(), String> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('run_history') WHERE name = ?1)",
            [name],
            |row| row.get(0),
        )
        .map_err(|error| format!("检查 SQLite 运行历史列 {name} 失败：{error}"))?;
    if exists {
        return Ok(());
    }
    connection
        .execute(
            &format!("ALTER TABLE run_history ADD COLUMN {name} TEXT NOT NULL DEFAULT '[]'"),
            [],
        )
        .map_err(|error| format!("迁移 SQLite 运行历史列 {name} 失败：{error}"))?;
    Ok(())
}

/// 补一列可空 TEXT。M2 之前建的库没有这一列，老历史行补出来就是 `NULL`——
/// 与 `ensure_json_column` 同一条路子，区别只在没有默认值可给：分类是**当时没记**，不是空集。
fn ensure_nullable_text_column(connection: &Connection, name: &str) -> Result<(), String> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('run_history') WHERE name = ?1)",
            [name],
            |row| row.get(0),
        )
        .map_err(|error| format!("检查 SQLite 运行历史列 {name} 失败：{error}"))?;
    if exists {
        return Ok(());
    }
    connection
        .execute(
            &format!("ALTER TABLE run_history ADD COLUMN {name} TEXT"),
            [],
        )
        .map_err(|error| format!("迁移 SQLite 运行历史列 {name} 失败：{error}"))?;
    Ok(())
}

fn json_array_text(values: &[Value]) -> Result<String, String> {
    serde_json::to_string(values).map_err(|error| format!("序列化运行历史诊断失败：{error}"))
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn owned_text(value: &Value, key: &str) -> Option<String> {
    text(value, key).map(str::to_owned)
}

fn number(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

const HISTORY_SELECT: &str = "SELECT
    run_record_id, run_id, task_id, run_params, source_sql, staging_table, started_at,
    started_at_ms, finished_at, outcome, target_table_effect, stage, source_rows, staged_rows,
    sink_reported_rows, purged_rows, source_batches, received_batches, fetch_ms, push_ms,
    commit_ms, count_ms, cursor_ms, source_code, sink_code, [column], [value],
    message, unknown_reason, failure_kind, seq, rows_pushed, bytes, ms, last_ts,
    mapping_issues
  FROM run_history";

fn history_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunHistory> {
    Ok(RunHistory {
        run_record_id: row.get("run_record_id")?,
        run_id: row.get("run_id")?,
        task_id: row.get("task_id")?,
        run_params: decode_run_params(row)?,
        source_sql: row.get("source_sql")?,
        staging_table: row.get("staging_table")?,
        started_at: row.get("started_at")?,
        started_at_ms: row.get("started_at_ms")?,
        finished_at: row.get("finished_at")?,
        outcome: row.get("outcome")?,
        target_table_effect: row.get("target_table_effect")?,
        stage: row.get("stage")?,
        source_rows: row.get("source_rows")?,
        staged_rows: row.get("staged_rows")?,
        sink_reported_rows: row.get("sink_reported_rows")?,
        purged_rows: row.get("purged_rows")?,
        source_batches: row.get("source_batches")?,
        received_batches: row.get("received_batches")?,
        fetch_ms: row.get("fetch_ms")?,
        push_ms: row.get("push_ms")?,
        commit_ms: row.get("commit_ms")?,
        count_ms: row.get("count_ms")?,
        cursor_ms: row.get("cursor_ms")?,
        source_code: row.get("source_code")?,
        sink_code: row.get("sink_code")?,
        column: row.get("column")?,
        value: row.get("value")?,
        message: row.get("message")?,
        unknown_reason: row.get("unknown_reason")?,
        failure_kind: row.get("failure_kind")?,
        seq: row.get("seq")?,
        rows_pushed: row.get("rows_pushed")?,
        bytes: row.get("bytes")?,
        ms: row.get("ms")?,
        last_ts: row.get("last_ts")?,
        mapping_issues: json_array_from_row(row, "mapping_issues")?,
    })
}

fn json_array_from_row(row: &rusqlite::Row<'_>, name: &str) -> rusqlite::Result<Vec<Value>> {
    let encoded: String = row.get(name)?;
    let column_index = row.as_ref().column_index(name)?;
    serde_json::from_str(&encoded).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column_index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}
