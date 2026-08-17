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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fsp: Option<u32>,
    // Display metadata from source; precheck deliberately does not read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support: Option<ColumnSupport>,
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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRunRequest {
    pub run_id: String,
    pub target_table: String,
    pub target_date_col: String,
    pub biz_date: String,
    pub source_columns: Vec<SourceColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenRunResponse {
    pub run_id: String,
    pub staging_table: String,
    pub columns_checked: usize,
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
