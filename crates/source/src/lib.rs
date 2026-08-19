use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Deserialize;

mod datasource;
mod failure_kind;
mod oracle_source;
mod protocol;
mod run_history;
mod secret;
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
    RangeCheckResult, RunResponse, SourceColumn, TargetConnection, Terminal,
};
// 九行形态的推导也只有一份定义（#125）——判定式仍两端各一份。
pub use datasource::{
    Datasource, DatasourceInput, DatasourceSettings, DatasourceSettingsView, DatasourceStore,
    DatasourceView,
};
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
    /// **已退役**（ADR-0037 §10）：Oracle 凭据的真相源是数据源库。
    /// 三个字段只在**首次启动、且数据源表为空**时被迁成一条名为「默认」的数据源，
    /// 迁完打 `warn` 提示删除。留着它们是因为数据源管理屏归 #123——
    /// 硬删字段等于在界面就位之前把现存部署打成「一条数据源都没有、也建不出来」。
    #[serde(default)]
    pub oracle_connect_string: Option<String>,
    #[serde(default)]
    pub oracle_username: Option<String>,
    #[serde(default)]
    pub oracle_password: Option<String>,
    /// **不退役**（ADR-0037 §6）：ODPI-C 的 client 库一个进程只初始化一次，
    /// 做成数据源级字段时第二个值会被**静默忽略**，实际用第一个初始化的库。
    pub oracle_client_lib_dir: String,
    /// 同样是进程级：一个 sink 进程能连任意 MySQL（ADR-0037 §1），
    /// 「哪个 sink」与「哪个库」在第一版没有必须解绑的理由。
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

/// 一次 Oracle 连接要用到的全部事实：数据源那三样 + 进程级的 client 库目录。
///
/// **`client_lib_dir` 不是数据源的字段**（ADR-0037 §6），它在这里出现只是因为
/// 「开一条连接」这件事同时需要两者。构造它的地方只有一处：数据源库 + `source.toml`。
#[derive(Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct OracleAccess {
    pub connect_string: String,
    pub username: String,
    pub password: String,
    pub client_lib_dir: String,
}

/// 手写 `Debug`：派生版会把口令原样打进任何 `{:?}`，而它会经过错误路径。
impl fmt::Debug for OracleAccess {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OracleAccess")
            .field("connect_string", &self.connect_string)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("client_lib_dir", &self.client_lib_dir)
            .finish()
    }
}

/// 一次运行要用到的全部任务面事实：规格 + 两端连接 + 本次运行参数取值。
///
/// 编排进程把它落成临时 TOML（0600，跑完即删）交给 run 子进程。**规格是真相源，SQL 不在里面**
/// （ADR-0036 §2）——两端都从同一份规格现算，不存在「存下来的那份与规格对不上」这个面。
///
/// 两端连接在这里已经是**解出来的明文**：编排进程按任务上的数据源绑定去库里解一次
/// （ADR-0037 §8），子进程不碰数据源库、也不碰密钥。
///
/// **字段顺序即 TOML 里的表顺序**：全是表，没有裸标量，所以不受
/// 「值必须排在 array-of-tables 之前」那条约束——但也别往前面加标量字段。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskConfig {
    pub spec: TaskSpec,
    pub oracle: OracleAccess,
    pub target: TargetConnection,
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
