mod http;
mod mysql_destination;
mod precheck;
mod service;

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::Value;

// 报文形状的唯一定义在 `db-qbs-shared`（#124）。这里只保留门面，
// crate 内部与既有测试的引用路径一个字不变。
pub use db_qbs_shared::{
    AbortResponse, BatchPayload, BatchResponse, ColumnSupport, CommitRequest, CommitResponse,
    ErrorBody, ErrorEnvelope, OpenRunRequest, OpenRunResponse, PrecheckIssue, RangeCheckColumn,
    RangeCheckResult, RunResponse, SourceColumn, Terminal,
};
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

/// sink 内部的错误模型。**它不是线上形状**——过线的是 [`ErrorEnvelope`]，
/// 由 [`ApiError::into_envelope`] 在 HTTP 边界上转一次（#124）。
/// 那一处转换是编译器盯着的唯一接缝：信封加字段，这里就编译不过。
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    pub run_id: Option<String>,
    pub details: Value,
}

impl ApiError {
    /// 字段顺序即线上字节顺序，与 [`ErrorBody`] 的声明顺序一致，**不要重排**。
    pub fn into_envelope(self) -> ErrorEnvelope {
        ErrorEnvelope {
            error: ErrorBody {
                code: self.code.to_owned(),
                message: self.message,
                run_id: self.run_id,
                details: self.details,
            },
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}
