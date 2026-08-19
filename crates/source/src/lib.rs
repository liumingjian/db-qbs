use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Deserialize;

mod failure_kind;
mod oracle_source;
mod protocol;
mod run_history;
mod sql_builder;
mod target_ddl;
mod task_spec;
mod task_store;
mod transfer;
mod web_assets;

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
pub use failure_kind::{oracle_kind, FailureKind};
pub use oracle_source::OracleRowSource;
pub use protocol::{HttpSinkClient, SinkClient, SinkError, SinkErrorKind, SinkGateDetails};
pub use run_history::{
    expired_history_indices, fold_history_lines, HistoryChange, HistoryStore, RunHistory,
    UnknownReason,
};
pub use sql_builder::{
    builder_column_query, builder_table_query, validate_builder_dblink, BuilderColumn, BuilderTable,
};
pub use target_ddl::{generate_target_ddl, TargetDdlColumnError, TargetDdlError};
pub use task_spec::{
    Comparison, Condition, Direction, OrderTerm, RunParams, TaskSpec, ValueSource, ValueType,
};
pub use task_store::{Task, TaskInput, TaskStore};
pub use transfer::{
    generate_run_id, run_transfer, RowSource, RunStage, SourceReadError, TransferEvent,
    TransferFailure, TransferRequest, TransferSummary, BATCH_BYTE_BUDGET, BATCH_ROW_LIMIT,
    FETCH_ARRAY_SIZE,
};
pub use web_assets::{embedded_web_asset, EmbeddedWebAsset};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    pub oracle_connect_string: String,
    pub oracle_username: String,
    pub oracle_password: String,
    pub oracle_client_lib_dir: String,
    pub sink_base_url: String,
    pub listen: String,
    pub data_dir: PathBuf,
    #[serde(default = "default_history_retention_days")]
    pub history_retention_days: u64,
    #[serde(default = "default_run_executable")]
    pub run_executable: PathBuf,
}

fn default_history_retention_days() -> u64 {
    90
}

fn default_run_executable() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_owned))
        .unwrap_or_default()
        .join("db-qbs-source-run")
}

/// 一次运行要用到的全部任务面事实：规格 + 本次运行参数取值。
///
/// 编排进程把它落成临时 TOML 交给 run 子进程。**规格是真相源，SQL 不在里面**
/// （ADR-0036 §2）——两端都从同一份规格现算，不存在「存下来的那份与规格对不上」这个面。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskConfig {
    pub spec: TaskSpec,
    #[serde(default)]
    pub run_params: RunParams,
}

impl TaskConfig {
    pub fn source_sql(&self) -> String {
        self.spec.source_sql()
    }

    pub fn bindings(&self) -> Result<Vec<(String, String)>, String> {
        self.spec.bindings(&self.run_params)
    }
}

/// 目标表 DDL 建议里给 `NUMBER` 补 `(p,s)` 的提示。
///
/// **它不在任务定义里**（ADR-0036 §6）：DDL 生成吃的是 describe 回来的源列，属「取列」链，
/// 这份提示随取列请求一起过来，用完即弃。
pub type ColumnPrecision = BTreeMap<String, [i64; 2]>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub kind: &'static str,
    pub path: PathBuf,
    pub detail: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "could not load {} file {}: {}",
            self.kind,
            self.path.display(),
            self.detail
        )
    }
}

impl std::error::Error for ConfigError {}

pub fn load_source_config(path: &Path) -> Result<SourceConfig, ConfigError> {
    load_toml(path, "source config")
}

pub fn load_task_config(path: &Path) -> Result<TaskConfig, ConfigError> {
    load_toml(path, "task")
}

fn load_toml<T: DeserializeOwned>(path: &Path, kind: &'static str) -> Result<T, ConfigError> {
    let text = fs::read_to_string(path).map_err(|error| ConfigError {
        kind,
        path: path.to_owned(),
        detail: error.to_string(),
    })?;

    toml::from_str(&text).map_err(|error| ConfigError {
        kind,
        path: path.to_owned(),
        detail: error.to_string(),
    })
}
