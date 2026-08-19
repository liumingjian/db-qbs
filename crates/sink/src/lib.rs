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
// 九行形态的推导也只有一份定义（#125）——判定式仍两端各一份。
pub use db_qbs_shared::{
    classify_column, column_support, derive_number_shape, is_business_date_column,
    is_supported_decimal_shape, ColumnShape, ShapeRejection, TargetShape,
};
pub use http::serve;
pub use mysql_destination::{check_connection_settings, MysqlDestination};
// `precheck` 是不带主键那一支，只给「生成的表喂回预检必过」那道漂移闸用；
// 带主键那一支同样导出，因为漂移闸现在还要守「生成的 DDL 带主键，ADR-0035 §2 三条得过」。
pub use precheck::{precheck, precheck_with_primary_key};
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

/// 目标表上一条唯一性约束（`PRIMARY KEY` 或 `UNIQUE`）覆盖的列集合。
///
/// 撤掉 DELETE 之后，幂等全靠它：目标表上**没有**对应约束时，
/// `ON DUPLICATE KEY UPDATE` 不报错、写得进去、重跑就出重复行（ADR-0035 §2）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetKey {
    pub name: String,
    pub columns: Vec<String>,
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
    /// upsert 的去重键（ADR-0035 §1）。`ON DUPLICATE KEY UPDATE` 的更新列
    /// = `columns` 减去这里的列。
    pub primary_key: Vec<String>,
    pub columns: Vec<String>,
    pub source_rows: u64,
    pub source_batches: u64,
    pub received_batches: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomicSwapResult {
    pub staged_rows: u64,
    /// 恒为 0（ADR-0035 §4）——新写入模型不删任何行，字段保留。
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
    fn target_keys(&self, target_table: &str) -> Result<Vec<TargetKey>, String>;
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
    primary_key: Vec<String>,
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
