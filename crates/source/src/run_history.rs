use std::fs::{self, OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use rusqlite::{named_params, params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ColumnMapping, FailureKind, RunStage, WriteMode};

const DATABASE_FILE: &str = "db-qbs.sqlite3";

macro_rules! history_params {
    ($history:expr, $mapping_issues:expr, $evidence:expr) => {
        named_params! {
            ":run_record_id": $history.run_record_id,
            ":run_id": $history.run_id,
            ":run_trigger": $history.trigger,
            ":task_id": $history.task_id,
            ":task_name": $history.task_name,
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
            ":total_rows": $history.total_rows,
            ":precount_ms": $history.precount_ms,
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
            ":evidence": $evidence,
        }
    };
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEvidence {
    #[serde(default)]
    pub source: Option<SourceEvidence>,
    #[serde(default)]
    pub target: Option<TargetEvidence>,
    #[serde(default)]
    pub agent: Option<AgentEvidence>,
    #[serde(default)]
    pub parameters: Option<RunParametersEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceEvidence {
    pub datasource_id: String,
    pub connect_string: String,
    pub username: String,
    pub client_lib_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetEvidence {
    pub datasource_id: String,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEvidence {
    pub agent_id: String,
    pub name: String,
    pub base_url: String,
    pub instance_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunParametersEvidence {
    pub target_table: String,
    pub columns: Vec<ColumnMapping>,
    pub primary_key: Vec<String>,
    /// 开跑那一刻的写入模式快照（#264）。
    ///
    /// 和 `primary_key` 一样是**当时的事实**，不是任务此刻的定义。这两份合起来才
    /// 说得出「这一次到底做了什么」：主键决定语句是 upsert 还是纯 INSERT，模式决定
    /// 之前有没有先清空整张表。回头去任务定义现取，改一次任务就会把**过去所有**
    /// 运行记录声称做过的事一起改写——与 `task_name` 同一条道理（#259）。
    ///
    /// 缺席的老历史行落到 `Append`：本字段之前，产品只有追加写这一档。
    #[serde(default)]
    pub write_mode: WriteMode,
    pub source_sql: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryChange {
    MemoryOnly,
    /// 要落盘，但**阶段没变**。开跑前计数就是这一类：它给的是分母，不是一个新阶段，
    /// 拿 `StageChanged` 顶替会顺手去改 `active_runs` 里的 stage（而且那条路上的
    /// `?` 会在 `active_runs` 缺条目时把整行日志丢掉）。
    FieldsChanged,
    StageChanged,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizeOutcome {
    Finalized,
    Replayed,
}

/// 一次运行是**谁发起的**：人按的，还是到点了调度器发的（#266）。
///
/// 两者在运行历史里必须分得开：夜里两点那次是不是自动跑的、还是有人手动补的一次，
/// 事后只有这一列答得出来。它**不是**结局的一部分，也不参与任何判定——纯粹是出处。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunTrigger {
    Manual,
    Scheduled,
}

impl RunTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "MANUAL",
            Self::Scheduled => "SCHEDULED",
        }
    }
}

/// 一次运行**没有留下终态日志**时，父进程替它写下的那句「为什么说不清」。
///
/// 三格闭集，且这三格只回答同一个问题的三种成因，不是运行阶段（[`RunStage`] 那五格
/// 一格不动）。区分它们的价值全在事后：一次被人按停的运行和一次被 OOM 杀掉的运行，
/// 在历史里长得一模一样的话，「昨晚那次到底是谁弄的」就永远没有答案。
///
/// [`RunStage`]: db_qbs_shared::RunStage
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownReason {
    ProcessDisappeared,
    ServiceRestarted,
    /// 有人在任务中心按了「停止运行」，父进程发出的 SIGTERM 把子进程带走了（#269）。
    /// 标记打在**发信号那一刻**，所以它说的是「这次死亡是我们要的」，
    /// 而不是「事后猜的」。
    StoppedByUser,
}

impl UnknownReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::ProcessDisappeared => "PROCESS_DISAPPEARED",
            Self::ServiceRestarted => "SERVICE_RESTARTED",
            Self::StoppedByUser => "STOPPED_BY_USER",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::ProcessDisappeared => "进程消失，无终态日志",
            Self::ServiceRestarted => "服务重启，结局未知",
            Self::StoppedByUser => "已由用户停止",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunHistory {
    pub run_record_id: String,
    pub run_id: Option<String>,
    /// 这一次是谁发起的（[`RunTrigger`]，#266）。落库的列名是 `run_trigger`，
    /// `trigger` 在 SQLite 里是保留字。老历史行迁移出来是 `MANUAL`——本字段之前
    /// 一次运行只可能是人按出来的。
    pub trigger: String,
    pub task_id: String,
    /// 开跑那一刻的任务名称快照。
    ///
    /// 名称在产品里只是展示标签：不唯一、不参与任何标识，随时可以在向导里改。
    /// 认领一次运行靠的是 `task_id`；名称如果每次展示都回头去任务表现取，
    /// 改一次名就会把**过去所有**运行记录上的名字一起改掉——历史于是不再是历史。
    /// 所以这一份和 `source_sql` 同一性质：当时是什么就永远是什么。
    ///
    /// 空串表示这条历史早于本字段（老库迁移后的默认值），展示层这时才回退到当前名称。
    pub task_name: String,
    /// 当次**实际执行**的源端 SQL 快照。
    ///
    /// 它与任务定义现算的那份性质根本不同：这份回答「当时执行了什么」，是审计事实，
    /// 规格改了它也不能跟着变。过滤条件是原样拼进 `WHERE` 的一段文本，所以这一份就是
    /// 执行的全文——没有另一半取值需要对照着读。
    pub source_sql: String,
    /// 开跑前从已经解出的连接与任务规格逐字段抄下的无口令快照。
    /// 空对象只表示这条历史早于快照功能，展示层不得拿当前配置补写过去。
    pub evidence: RunEvidence,
    pub staging_table: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub outcome: Option<String>,
    /// 目标表最后被怎么了。三个值：`SWAPPED`「按主键合并进目标表」、
    /// `REPLACED`「整表被替换」（清空后导入，#264）、`DISCARDED`「没被触碰」，
    /// 外加一个 `UNKNOWN`「说不清」。
    ///
    /// 和 `stage` 一样**是字符串不是枚举，且不认识的值原样搬运**：这一列是日志的
    /// 尽力投影，吞掉一个没见过的拼写等于把「子进程比父进程新」这件事藏起来。
    pub target_table_effect: Option<String>,
    /// 展示用的那一份，**是字符串不是枚举**，而且是故意的：运行历史按定义是
    /// 「日志的尽力投影」，它必须能原样搬运一个自己不认识的拼写。吞掉它，
    /// 「前端/父进程的版本落后于跑数的子进程」这件事就在屏幕上彻底消失了，
    /// 而那正是最该被看见的时候。判定不走这一份，走 `RunStage::parse`。
    pub stage: Option<String>,
    pub source_rows: Option<u64>,
    pub staged_rows: Option<u64>,
    pub sink_reported_rows: Option<u64>,
    pub purged_rows: Option<u64>,
    pub source_batches: Option<u64>,
    pub received_batches: Option<u64>,
    /// 开跑前那一次 `COUNT(*)` 拿到的**总行数**：迁移进度那一列的分母（ADR-0043 §7）。
    ///
    /// **`None` 不是错误**：计数本身失败时它缺席，而那次运行照常跑完。
    /// 它是**开跑那一刻**的事实，与随后的读取之间存在时间差——分母不是实时的。
    /// M2 之前落盘的老历史行也都是 `None`（那时还没有这一次计数）。
    pub total_rows: Option<u64>,
    /// 那一次计数**自己**的耗时。与 `count_ms`（sink 侧门禁计数）是两回事，别混；
    /// 也不并进 `fetch_ms`——揉进去之后「取数慢」会是两件事的和。
    pub precount_ms: Option<u64>,
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
        source_sql: &str,
        accepted_at: DateTime<Utc>,
    ) -> Self {
        Self {
            run_record_id: run_record_id.to_owned(),
            run_id: None,
            trigger: RunTrigger::Manual.as_str().to_owned(),
            task_id: task_id.to_owned(),
            task_name: String::new(),
            source_sql: source_sql.to_owned(),
            evidence: RunEvidence::default(),
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
            total_rows: None,
            precount_ms: None,
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
            // 开跑前计数：**落盘**而不是只留内存。进行中的那一行要靠它才有分母，
            // 而进行中的行恰恰是重启后最需要还原的那一类。
            Some("precount_finished") => {
                self.total_rows = number(log, "total_rows");
                self.precount_ms = number(log, "precount_ms");
                HistoryChange::FieldsChanged
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
                // **不认识的拼写原样搬运**，和 `stage` 那一列同一个规矩（#264）。
                // 从前这里是个把闭集外的值一律折成 `UNKNOWN` 的 `match`，于是
                // 「子进程比父进程新、多报了一个终态词」这件事在屏幕上彻底消失——
                // 而那正是最该被看见的时候。真正的「说不清」只有一种：子进程自己
                // 就没能判定，`terminal` 是 `null`。
                self.target_table_effect =
                    Some(text(log, "terminal").unwrap_or("UNKNOWN").to_owned());
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

    /// 到点了但**没发起**的那一次（#266）。
    ///
    /// 它与「预检拒绝、从未到达代理」同构：**没有运行标识**（`run_id` 一直是 `None`），
    /// 目标表一个字节都没动过。写它的理由只有一条——绝不静默丢弃一个触发时刻，
    /// 否则「月末那次到底跑没跑」没人答得上来。
    pub fn mark_skipped(&mut self, message: String, at: DateTime<Utc>) {
        self.outcome = Some("FAILED".to_owned());
        self.target_table_effect = Some("DISCARDED".to_owned());
        self.finished_at = Some(at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        self.message = Some(message);
        self.failure_kind = Some(FailureKind::Skipped.as_str().to_owned());
    }

    pub fn started_at_ms(&self) -> i64 {
        self.started_at_ms
    }

    fn finish_from_log(&mut self, log: &Value) {
        self.outcome = owned_text(log, "terminal");
        self.stage = owned_text(log, "stage");
        self.finished_at = owned_text(log, "ts");
        if self.target_table_effect.is_none() {
            // 子进程若直说了目标表遭遇了什么，就照它的（#264）——写入模式只有跑数那一端
            // 知道，「跑成功了 ⇒ 按主键合并」在清空后导入这条路上是假话。这一份同样
            // 原样搬运，不做闭集裁决。下面那套折算是**后备**：老日志里没有这个字段。
            let stated = owned_text(log, "target_table_effect");
            let stage = self.stage.as_deref().and_then(RunStage::parse);
            let folded = match (self.outcome.as_deref(), stage, text(log, "sink_code")) {
                (Some("SUCCEEDED"), _, _) => Some("SWAPPED".to_owned()),
                (Some("FAILED"), _, Some("VERIFY_FAILED")) => Some("DISCARDED".to_owned()),
                (Some("FAILED"), Some(RunStage::Committing), _) => Some("UNKNOWN".to_owned()),
                (Some("FAILED"), _, _) => Some("DISCARDED".to_owned()),
                _ => None,
            };
            self.target_table_effect = stated.or(folded);
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
                    run_trigger         TEXT NOT NULL DEFAULT 'MANUAL',
                    task_id             TEXT NOT NULL,
                    task_name           TEXT NOT NULL DEFAULT '',
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
                    total_rows          INTEGER,
                    precount_ms         INTEGER,
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
                    mapping_issues      TEXT NOT NULL DEFAULT '[]',
                    evidence            TEXT NOT NULL DEFAULT '{}'
                );
                 CREATE INDEX IF NOT EXISTS run_history_task_started
                     ON run_history(task_id, started_at_ms);
                 -- 「撤销运行」整套移除（#256），本地这张清理记录表跟着退役。
                 -- 只删不建：光把 CREATE 拿掉，已经开过库的实例里它还在。
                 DROP TABLE IF EXISTS run_cleanup;",
            )
            .map_err(|error| format!("初始化 SQLite 运行历史表失败：{error}"))?;
        // 老库里没有这一列：迁移出来的行拿空串，展示层据此回退到当前名称。
        ensure_text_column(&connection, "task_name")?;
        // 老库里没有这一列：迁移出来的行拿 `MANUAL`——本列之前一次运行只可能是人按的（#266）。
        ensure_column_with_default(&connection, "run_trigger", "MANUAL")?;
        ensure_json_column(&connection, "mapping_issues", "[]")?;
        ensure_json_column(&connection, "evidence", "{}")?;
        ensure_nullable_text_column(&connection, "failure_kind")?;
        ensure_nullable_integer_column(&connection, "total_rows")?;
        ensure_nullable_integer_column(&connection, "precount_ms")?;
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
        let mapping_issues = json_array_text(&history.mapping_issues)?;
        let evidence = json_object_text(&history.evidence)?;
        transaction
            .execute(
                "INSERT INTO run_history (
                    run_record_id, run_id, run_trigger, task_id, task_name, source_sql, staging_table,
                    started_at, started_at_ms, finished_at, outcome, target_table_effect, stage,
                    source_rows, staged_rows, sink_reported_rows, purged_rows, source_batches,
                    received_batches, total_rows, precount_ms,
                    fetch_ms, push_ms, commit_ms, count_ms, cursor_ms,
                    source_code, sink_code, [column], [value], message, unknown_reason,
                    failure_kind, seq, rows_pushed, bytes, ms, last_ts, mapping_issues, evidence
                 ) VALUES (
                    :run_record_id, :run_id, :run_trigger, :task_id, :task_name, :source_sql, :staging_table,
                    :started_at, :started_at_ms, :finished_at, :outcome, :target_table_effect,
                    :stage, :source_rows, :staged_rows, :sink_reported_rows, :purged_rows,
                    :source_batches, :received_batches, :total_rows, :precount_ms,
                    :fetch_ms, :push_ms, :commit_ms,
                    :count_ms, :cursor_ms, :source_code, :sink_code, :column, :value,
                    :message, :unknown_reason, :failure_kind, :seq, :rows_pushed, :bytes, :ms,
                    :last_ts, :mapping_issues, :evidence
                 )",
                history_params!(history, mapping_issues, evidence),
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
        let mapping_issues = json_array_text(&history.mapping_issues)?;
        let evidence = json_object_text(&history.evidence)?;
        transaction
            .execute(
                "UPDATE run_history SET
                    run_id=:run_id, run_trigger=:run_trigger, task_id=:task_id, task_name=:task_name,
                    source_sql=:source_sql,
                    staging_table=:staging_table, started_at=:started_at,
                    started_at_ms=:started_at_ms, finished_at=:finished_at, outcome=:outcome,
                    target_table_effect=:target_table_effect, stage=:stage,
                    source_rows=:source_rows, staged_rows=:staged_rows,
                    sink_reported_rows=:sink_reported_rows, purged_rows=:purged_rows,
                    source_batches=:source_batches, received_batches=:received_batches,
                    total_rows=:total_rows, precount_ms=:precount_ms,
                    fetch_ms=:fetch_ms, push_ms=:push_ms, commit_ms=:commit_ms,
                    count_ms=:count_ms, cursor_ms=:cursor_ms, source_code=:source_code,
                    sink_code=:sink_code, [column]=:column, [value]=:value, message=:message,
                    unknown_reason=:unknown_reason, failure_kind=:failure_kind, seq=:seq,
                    rows_pushed=:rows_pushed,
                    bytes=:bytes, ms=:ms, last_ts=:last_ts,
                    mapping_issues=:mapping_issues, evidence=:evidence
                  WHERE run_record_id=:run_record_id",
                history_params!(history, mapping_issues, evidence),
            )
            .map_err(|error| format!("更新 SQLite 运行历史失败：{error}"))?;
        cleanup_transaction(&transaction, now, retention_days)?;
        transaction
            .commit()
            .map_err(|error| format!("提交 SQLite 运行历史事务失败：{error}"))
    }

    /// Persist one terminal transition. Alert and delivery records join this transaction.
    pub fn finalize(
        &self,
        history: &RunHistory,
        now: DateTime<Utc>,
        retention_days: u64,
    ) -> Result<FinalizeOutcome, String> {
        if history.outcome.is_none() {
            return Err("不能封口没有结局的运行历史".to_owned());
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| format!("开启 SQLite 运行封口事务失败：{error}"))?;
        let stored_outcome = transaction
            .query_row(
                "SELECT outcome FROM run_history WHERE run_record_id = ?1",
                [&history.run_record_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|error| format!("查询 SQLite 运行封口状态失败：{error}"))?;
        if stored_outcome.flatten().is_some() {
            transaction
                .commit()
                .map_err(|error| format!("提交 SQLite 运行封口事务失败：{error}"))?;
            return Ok(FinalizeOutcome::Replayed);
        }

        let mapping_issues = json_array_text(&history.mapping_issues)?;
        let evidence = json_object_text(&history.evidence)?;
        let changed = transaction
            .execute(
                "UPDATE run_history SET
                    run_id=:run_id, run_trigger=:run_trigger, task_id=:task_id, task_name=:task_name,
                    source_sql=:source_sql, staging_table=:staging_table,
                    started_at=:started_at, started_at_ms=:started_at_ms,
                    finished_at=:finished_at, outcome=:outcome,
                    target_table_effect=:target_table_effect, stage=:stage,
                    source_rows=:source_rows, staged_rows=:staged_rows,
                    sink_reported_rows=:sink_reported_rows, purged_rows=:purged_rows,
                    source_batches=:source_batches, received_batches=:received_batches,
                    total_rows=:total_rows, precount_ms=:precount_ms,
                    fetch_ms=:fetch_ms, push_ms=:push_ms, commit_ms=:commit_ms,
                    count_ms=:count_ms, cursor_ms=:cursor_ms, source_code=:source_code,
                    sink_code=:sink_code, [column]=:column, [value]=:value, message=:message,
                    unknown_reason=:unknown_reason, failure_kind=:failure_kind, seq=:seq,
                    rows_pushed=:rows_pushed, bytes=:bytes, ms=:ms, last_ts=:last_ts,
                    mapping_issues=:mapping_issues, evidence=:evidence
                  WHERE run_record_id=:run_record_id AND outcome IS NULL",
                history_params!(history, mapping_issues, evidence),
            )
            .map_err(|error| format!("封口 SQLite 运行历史失败：{error}"))?;
        if changed == 0 {
            transaction
                .execute(
                    "INSERT INTO run_history (
                        run_record_id, run_id, run_trigger, task_id, task_name, source_sql,
                        staging_table, started_at, started_at_ms, finished_at, outcome,
                        target_table_effect, stage, source_rows, staged_rows, sink_reported_rows,
                        purged_rows, source_batches, received_batches, total_rows, precount_ms,
                        fetch_ms, push_ms, commit_ms, count_ms, cursor_ms, source_code, sink_code,
                        [column], [value], message, unknown_reason, failure_kind, seq, rows_pushed,
                        bytes, ms, last_ts, mapping_issues, evidence
                     ) VALUES (
                        :run_record_id, :run_id, :run_trigger, :task_id, :task_name, :source_sql,
                        :staging_table, :started_at, :started_at_ms, :finished_at, :outcome,
                        :target_table_effect, :stage, :source_rows, :staged_rows,
                        :sink_reported_rows, :purged_rows, :source_batches, :received_batches,
                        :total_rows, :precount_ms, :fetch_ms, :push_ms, :commit_ms, :count_ms,
                        :cursor_ms, :source_code, :sink_code, :column, :value, :message,
                        :unknown_reason, :failure_kind, :seq, :rows_pushed, :bytes, :ms,
                        :last_ts, :mapping_issues, :evidence
                     )",
                    history_params!(history, mapping_issues, evidence),
                )
                .map_err(|error| format!("插入 SQLite 终态运行历史失败：{error}"))?;
        }
        cleanup_transaction(&transaction, now, retention_days)?;
        transaction
            .commit()
            .map_err(|error| format!("提交 SQLite 运行封口事务失败：{error}"))?;
        Ok(FinalizeOutcome::Finalized)
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

    /// 进程死的那一刻还没走完的那几行（`outcome IS NULL`）。
    ///
    /// **必须在 [`Self::seal_incomplete`] 之前问**：封口那一下把它们全改成 `FAILED`，
    /// 之后再问就一行都认不出来了。问它是为了目标表占用：source 自己被重启时，
    /// 子进程连同它那一刀 abort 一起没了，而占用在目标端是纯内存的，
    /// 没人补这一刀就永远挂着（#272）。
    pub fn list_incomplete(&self) -> Result<Vec<RunHistory>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(&format!(
                "{HISTORY_SELECT}
                  WHERE outcome IS NULL
               ORDER BY started_at_ms ASC, rowid ASC"
            ))
            .map_err(|error| format!("准备 SQLite 非终态运行历史查询失败：{error}"))?;
        let history = statement
            .query_map([], history_from_row)
            .map_err(|error| format!("查询 SQLite 非终态运行历史失败：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("读取 SQLite 非终态运行历史失败：{error}"))?;
        Ok(history)
    }

    pub fn seal_incomplete(
        &self,
        reason: UnknownReason,
        now: DateTime<Utc>,
        retention_days: u64,
    ) -> Result<(), String> {
        for mut history in self.list_incomplete()? {
            history.mark_unknown(reason, now);
            self.finalize(&history, now, retention_days)?;
        }
        Ok(())
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
    source_sql: &str,
    accepted_at: DateTime<Utc>,
    lines: &[&str],
) -> Result<RunHistory, String> {
    let mut history = RunHistory::accepted(run_record_id, task_id, source_sql, accepted_at);
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

/// 老历史表整表丢弃，与任务定义同一条理由（前提是第一版尚无真实用户数据）。
///
/// 它按 `biz_date` 一列存筛选维度、按 `shape_checks` 存已退役的形状预检结果，
/// 两者在新模型里都没有对应物；把 `biz_date` 翻成一个含义不明的字段是**编事实**，
/// 而历史那份数据的全部价值就在于它是事实。
///
/// **`run_params` 那一列不在此列**：运行参数链退役之后没人再读它，而它建表时带着
/// `DEFAULT '{}'`，老库上照旧插得进去。为一列读都不读的死数据丢掉整段运行历史，
/// 代价与收益完全不成比例。
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

fn retention_cutoff(now: DateTime<Utc>, retention_days: u64) -> Option<DateTime<Utc>> {
    let days = i64::try_from(retention_days).ok()?;
    now.checked_sub_signed(TimeDelta::try_days(days)?)
}

/// 补一列非空 TEXT，老库迁移出来的行拿空串。
///
/// 空串在这里是**实话**：那些运行跑完的时候还没有这一列可记，拿当前的值补写过去
/// 才是造假。展示层看见空串就回退，看见内容就照抄。
fn ensure_text_column(connection: &Connection, name: &str) -> Result<(), String> {
    ensure_column_with_default(connection, name, "")
}

fn ensure_json_column(connection: &Connection, name: &str, default: &str) -> Result<(), String> {
    ensure_column_with_default(connection, name, default)
}

fn ensure_column_with_default(
    connection: &Connection,
    name: &str,
    default: &str,
) -> Result<(), String> {
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
            &format!("ALTER TABLE run_history ADD COLUMN {name} TEXT NOT NULL DEFAULT '{default}'"),
            [],
        )
        .map_err(|error| format!("迁移 SQLite 运行历史列 {name} 失败：{error}"))?;
    Ok(())
}

/// 补一列可空 INTEGER。老库里没有这一列，老历史行补出来就是 `NULL`——
/// 那正是实话：那些运行跑的时候还没有「开跑前计数」这回事。
fn ensure_nullable_integer_column(connection: &Connection, name: &str) -> Result<(), String> {
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
            &format!("ALTER TABLE run_history ADD COLUMN {name} INTEGER"),
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

fn json_object_text(value: &RunEvidence) -> Result<String, String> {
    serde_json::to_string(value).map_err(|error| format!("序列化运行证据失败：{error}"))
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
    run_record_id, run_id, run_trigger, task_id, task_name, source_sql, staging_table, started_at,
    started_at_ms, finished_at, outcome, target_table_effect, stage, source_rows, staged_rows,
    sink_reported_rows, purged_rows, source_batches, received_batches, fetch_ms, push_ms,
    total_rows, precount_ms,
    commit_ms, count_ms, cursor_ms, source_code, sink_code, [column], [value],
    message, unknown_reason, failure_kind, seq, rows_pushed, bytes, ms, last_ts,
    mapping_issues, evidence
  FROM run_history";

fn history_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunHistory> {
    Ok(RunHistory {
        run_record_id: row.get("run_record_id")?,
        run_id: row.get("run_id")?,
        trigger: row.get("run_trigger")?,
        task_id: row.get("task_id")?,
        task_name: row.get("task_name")?,
        source_sql: row.get("source_sql")?,
        evidence: json_object_from_row(row, "evidence")?,
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
        total_rows: row.get("total_rows")?,
        precount_ms: row.get("precount_ms")?,
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

fn json_object_from_row(row: &rusqlite::Row<'_>, name: &str) -> rusqlite::Result<RunEvidence> {
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
