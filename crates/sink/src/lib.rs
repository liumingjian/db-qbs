mod http;
mod mysql_destination;
mod precheck;
mod service;

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use db_qbs_shared::BatchPayload;
pub use http::serve;
pub use mysql_destination::{check_connection_settings, MysqlDestination};
pub use precheck::precheck;
pub use service::build_staging_ddl;

const MAX_PREPARED_STATEMENT_PLACEHOLDERS: usize = 65_535;
const TOMBSTONE_LIMIT: usize = 32;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SinkConfig {
    pub mysql_dsn: String,
    pub database: String,
    pub listen: String,
}

impl SinkConfig {
    pub fn parse(input: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(input)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let input = fs::read_to_string(path)
            .map_err(|error| format!("读取 sink 配置 {} 失败：{error}", path.display()))?;
        Self::parse(&input)
            .map_err(|error| format!("解析 sink 配置 {} 失败：{error}", path.display()))
    }
}

/// 取列面的三档支持标记（ADR-0010 2026-08-16 增补二 §2）。
///
/// **`sink` 不得读它做任何判定。** 它随 `POST /runs` 的 `source_columns` 一起过线，
/// 只是因为两端共用同一个结构形状；逐列类型判定按 ADR-0010 §3.1 集中在 sink，
/// 由 sink 自己按 `type` / `precision` / `scale` / `length` / `fsp` 判。
/// 一旦被当成判定输入，判定就悄悄搬回 source 侧了。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnSupport {
    Ok,
    NeedsPrecision,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: String,
    pub precision: Option<i64>,
    pub scale: Option<i64>,
    pub length: Option<u64>,
    /// `TIMESTAMP(n)` 的 `n`。非 `TIMESTAMP` 列不带它（ADR-0010 2026-08-16 增补一）。
    /// **永久可选**，不设收紧成必填的计划（#106 裁定 Q14）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fsp: Option<u32>,
    /// 见 [`ColumnSupport`]——展示提示，**不是预检裁决**。同样永久可选。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support: Option<ColumnSupport>,
}

/// 3.5 步值域校核：sink 跑完 1–3 步后告诉 source「哪几列要校核 + 推导出的目标形状」。
///
/// `precision` / `scale` 是**推导出的目标形状** `(p', s')`，不是源端 `(p, s)`；
/// 推导形状的标度恒非负，故用无符号（#106「预检顺序加 3.5 步」）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RangeCheckColumn {
    pub column: String,
    pub precision: u32,
    pub scale: u32,
}

/// 3.5 步值域校核：source 执行完聚合 SQL 后回发的每列不合规行数。
/// **判定仍由 sink 做**（ADR-0010 §3.1）——source 只回事实，不回结论。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RangeCheckResult {
    pub column: String,
    pub invalid_rows: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetColumn {
    pub name: String,
    pub column_type: String,
    pub data_type: String,
    pub precision: Option<u64>,
    pub scale: Option<u64>,
    pub length: Option<u64>,
    pub datetime_precision: Option<u64>,
    pub nullable: bool,
    pub character_set: Option<String>,
    pub ordinal: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrecheckIssue {
    pub column: String,
    pub source: String,
    pub target: String,
    pub rule: String,
    /// 动作型建议，**由 sink 侧算**（ADR-0010 2026-08-16 增补二 §1）——
    /// web 不得把判定式复制进 TypeScript 重算一遍。永久可选。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRunRequest {
    pub run_id: String,
    pub target_table: String,
    pub target_date_col: String,
    pub biz_date: String,
    pub source_columns: Vec<SourceColumn>,
    /// 3.5 步：source 回发的值域校核结果。永久可选（#106 裁定 Q14/Q15）。
    #[serde(default)]
    pub range_check_results: Option<Vec<RangeCheckResult>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenRunResponse {
    pub run_id: String,
    pub staging_table: String,
    pub columns_checked: usize,
    /// 3.5 步：sink 告诉 source「哪几列要跑值域校核」。永久可选（#106 裁定 Q14/Q15）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_check_columns: Option<Vec<RangeCheckColumn>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AbortResponse {
    pub run_id: String,
    pub staging_dropped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BatchResponse {
    pub seq: u64,
    pub rows_written: u64,
    pub next_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitRequest {
    pub total_batches: u64,
    pub total_rows: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommitResponse {
    pub source_rows: u64,
    pub staged_rows: u64,
    pub purged_rows: u64,
    pub swapped_rows: u64,
    pub count_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Terminal {
    Swapped,
    Discarded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunResponse {
    pub run_id: String,
    pub staging_table: String,
    pub batches_received: u64,
    pub rows_written: u64,
    pub sealed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal: Option<Terminal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purged_rows: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swapped_rows: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomicSwapRequest {
    pub staging_table: String,
    pub target_table: String,
    pub target_date_col: String,
    pub biz_date_start: String,
    pub biz_date_end: String,
    pub columns: Vec<String>,
    pub source_rows: u64,
    pub source_batches: u64,
    pub received_batches: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomicSwapResult {
    pub staged_rows: u64,
    pub purged_rows: u64,
    pub swapped_rows: u64,
    pub count_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtomicSwapError {
    VerifyFailed { staged_rows: u64, count_ms: u64 },
    TargetBusy { errno: u16 },
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateStagingError {
    TableExists,
    PermissionDenied,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropStagingError {
    PermissionDenied,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteBatchError {
    DataValue {
        mysql_code: u16,
        column: String,
        value: Option<String>,
    },
    PrecheckEscape {
        mysql_code: u16,
        column: Option<String>,
        value: Option<String>,
    },
    Environment {
        mysql_code: u16,
    },
    Other(String),
}

pub trait Destination: Send + Sync {
    fn target_columns(&self, target_table: &str) -> Result<Vec<TargetColumn>, String>;
    fn create_staging(&self, staging_table: &str, ddl: &str) -> Result<(), CreateStagingError>;
    fn write_batch(
        &self,
        staging_table: &str,
        columns: &[String],
        rows: &[Vec<Option<String>>],
        max_rows_per_insert: usize,
    ) -> Result<u64, WriteBatchError>;
    fn atomic_swap(&self, request: &AtomicSwapRequest)
        -> Result<AtomicSwapResult, AtomicSwapError>;
    fn drop_staging(&self, staging_table: &str) -> Result<(), DropStagingError>;
}

#[derive(Clone)]
struct ActiveRun {
    staging_table: String,
    source_columns: Vec<String>,
    swap_columns: Vec<String>,
    target_table: String,
    target_date_col: String,
    biz_date_start: String,
    biz_date_end: String,
    max_rows_per_insert: usize,
    next_seq: u64,
    rows_written: u64,
    sealed: bool,
}

pub struct SinkService<D: Destination> {
    database: String,
    destination: Arc<D>,
    active_runs: Mutex<HashMap<String, ActiveRun>>,
    tombstones: Mutex<VecDeque<RunResponse>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApiError {
    #[serde(skip)]
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    pub run_id: Option<String>,
    pub details: Value,
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}
