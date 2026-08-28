mod agent;
pub mod http;
mod mysql_destination;
mod precheck;
mod service;
pub mod test_support;

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::Value;

// 报文形状的唯一定义在 `db-qbs-shared`（#124）。这里只保留门面，
// crate 内部与既有测试的引用路径一个字不变。
pub use db_qbs_shared::{
    AbortResponse, BatchPayload, BatchResponse, ColumnSupport, CommitRequest, CommitResponse,
    ErrorBody, ErrorEnvelope, MysqlServerInfo, OpenOutcome, OpenRunRequest, OpenRunResponse,
    PrecheckIssue, RangeCheckColumn, RangeCheckResult, RunResponse, SourceColumn,
    TargetCheckFinding, TargetCheckKind, TargetCheckRequest, TargetCheckResult, TargetConnection,
    Terminal, WriteMode, WriteStatement,
};
// 九行形态的推导也只有一份定义（#125）——判定式仍两端各一份。
pub use agent::load_or_create as load_agent_identity;
pub use db_qbs_shared::{
    classify_column, column_support, derive_number_shape, is_business_date_column,
    is_supported_decimal_shape, ColumnShape, ShapeRejection, TargetShape,
};
pub use http::serve;
pub use mysql_destination::{
    check_connection_settings, MysqlDestination, MysqlFactory, MIN_PACKET, PACKET_REMEDY,
};
// `precheck` 是不带主键那一支，只给「生成的表喂回预检必过」那道漂移闸用；
// 带主键那一支同样导出，因为漂移闸现在还要守「生成的 DDL 带主键，ADR-0035 §2 三条得过」。
pub use precheck::{
    precheck, precheck_conclusions, precheck_with_primary_key, target_check_findings,
    APPEND_ONLY_CONCLUSION,
};
pub use service::build_staging_ddl;

const MAX_PREPARED_STATEMENT_PLACEHOLDERS: usize = 65_535;
const TOMBSTONE_LIMIT: usize = 32;

/// 一台 agent 上同时允许在飞的 run 数上限，`max_concurrent_runs` 不写时取它（#260）。
///
/// 4 不是算出来的，是**保守的默认**：每个 run 各持一份自己的目标端连接池
/// （[`MysqlDestination`]），4 个 run 在最坏情况下也就是十几条 MySQL 连接。
/// 现场要更高就在 `sink.toml` 里写明——但那是一次**明示**的决定，
/// 不该由「谁先把请求发过来谁就占住」来隐式决定。
pub const DEFAULT_MAX_CONCURRENT_RUNS: usize = 4;

fn default_max_concurrent_runs() -> usize {
    DEFAULT_MAX_CONCURRENT_RUNS
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SinkConfig {
    /// **已退役**（ADR-0037 §2）：目标端凭据随每个 run 的请求过线，sink 不再持有自己的那一份。
    /// 字段留着只为让现存部署与三份验收台架的 `sink.toml` 仍能解析——启动时打 `warn` 提示删除，
    /// 值本身**不被任何代码读取**。硬报错换来的只是一次无谓的中断。
    #[serde(default)]
    pub mysql_dsn: Option<String>,
    /// 已退役，同 `mysql_dsn`。库名现在随 [`TargetConnection`] 过来，逐 run 取值。
    #[serde(default)]
    pub database: Option<String>,
    pub listen: String,
    /// 这台 agent 给人看的名字（ADR-0044 §2），留空取主机名。**不作判据**，只进 source 的界面。
    #[serde(default)]
    pub agent_name: Option<String>,
    /// agent 身份文件的位置。默认是 `sink.toml` 同目录下的 `agent-id`——
    /// 不需要人来准备，没有就现生成（见 [`crate::load_agent_identity`]）。
    #[serde(default)]
    pub agent_id_file: Option<PathBuf>,
    /// 这台 agent 同时允许在飞多少个 run（#260），不写取 [`DEFAULT_MAX_CONCURRENT_RUNS`]。
    ///
    /// 超限时 `POST /v1/runs` 当场拒，**不排队**：排队等于让 source 那边挂着一条
    /// 看不出为什么不动的连接，而「额度满了，等在跑的那几个结束再来」是句能读懂的话。
    #[serde(default = "default_max_concurrent_runs")]
    pub max_concurrent_runs: usize,
    /// `sink.toml` 所在目录，**不来自配置文件本身**：`agent_id_file` 留空时的默认位置按它算。
    /// [`SinkConfig::parse`] 出来的配置里它是空的，那条路径只有测试在走。
    #[serde(skip)]
    pub config_dir: PathBuf,
}

impl SinkConfig {
    pub fn parse(input: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(input)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let input = fs::read_to_string(path)
            .map_err(|error| format!("读取 sink 配置 {} 失败：{error}", path.display()))?;
        let mut config = Self::parse(&input)
            .map_err(|error| format!("解析 sink 配置 {} 失败：{error}", path.display()))?;
        config.config_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        Ok(config)
    }

    /// 身份文件的最终位置：配置里指了就用那个，没指就落在 `sink.toml` 隔壁。
    pub fn agent_id_path(&self) -> PathBuf {
        self.agent_id_file
            .clone()
            .unwrap_or_else(|| self.config_dir.join("agent-id"))
    }
}

/// 目标表上一条唯一性约束（`PRIMARY KEY` 或 `UNIQUE`）覆盖的列集合。
///
/// 撤掉 DELETE 之后，幂等全靠它：目标表上**没有**对应约束时，
/// `ON DUPLICATE KEY UPDATE` 不报错、写得进去、重跑就出重复行（ADR-0035 §2）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TargetKey {
    pub name: String,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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
    /// `information_schema.COLUMNS.COLUMN_DEFAULT`——没有默认值时是 `NULL`。
    ///
    /// 它与 [`Self::extra`] 是 ADR-0038 §5 第 3 分支的判据：未被映射的列
    /// `NOT NULL` 且**无默认值且非 auto_increment** 才拒，否则放行。
    /// 跟着原来那条 `information_schema.COLUMNS` 查询一起取，不多一次来回。
    pub default_value: Option<String>,
    /// `information_schema.COLUMNS.EXTRA`——`auto_increment` / `DEFAULT_GENERATED` 之类，
    /// 没有就是空串（这一列在 MySQL 里 `NOT NULL`）。
    pub extra: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomicSwapRequest {
    pub run_id: String,
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
    /// 这条连接背后那台 MySQL 的自述（版本 + utf8mb4 默认字符序，见 [`MysqlServerInfo`]）。
    ///
    /// **建连接的时候就读掉**，之后只是把读到的那一份交出来：`GET /v1/agent/info` 手上
    /// 没有任何凭据（ADR-0037 §2），临时去连是连不上的。读不到就是 `None`——
    /// 一台连得上、但连自己的字符集表都查不了的 MySQL 不该因此让整条搬运链停摆。
    fn server_info(&self) -> Option<MysqlServerInfo>;

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

/// 一条按 run 建起来的目标端连接：库名 + 目的地。
///
/// 库名与连接一起出自工厂，**不是**从请求里另取一份：`FixedDestination`（测试与既有夹具）
/// 要的是「固定库名 + 固定目的地」，[`MysqlFactory`] 要的是「按请求里的库名连」。
/// 让工厂一次给全，调用方就没有第二处能把这两者配错。
pub struct ConnectedDestination<D> {
    pub database: String,
    pub destination: Arc<D>,
}

/// 按 run 建目标端连接（ADR-0037 §2）。sink 启动不再连 MySQL，
/// 连不上的失败点从进程启动挪到 `POST /v1/runs`。
pub trait DestinationFactory: Send + Sync {
    type Dest: Destination;

    fn connect(
        &self,
        target: &TargetConnection,
    ) -> Result<ConnectedDestination<Self::Dest>, String>;
}

/// 固定一份目的地的工厂：忽略请求里的连接信息，永远给同一个。
///
/// 它是**测试与夹具用**的（`SinkService::new` 就构造它），也是既有那二十几处
/// `SinkService::new("qbs", destination)` 一个字都不用改的原因。生产路径走 [`MysqlFactory`]。
pub struct FixedDestination<D> {
    database: String,
    destination: Arc<D>,
}

impl<D> FixedDestination<D> {
    pub fn new(database: impl Into<String>, destination: Arc<D>) -> Self {
        Self {
            database: database.into(),
            destination,
        }
    }
}

impl<D: Destination> DestinationFactory for FixedDestination<D> {
    type Dest = D;

    fn connect(
        &self,
        _target: &TargetConnection,
    ) -> Result<ConnectedDestination<Self::Dest>, String> {
        Ok(ConnectedDestination {
            database: self.database.clone(),
            destination: Arc::clone(&self.destination),
        })
    }
}

struct ActiveRun<D> {
    run_id: String,
    /// 本次 run 写的是哪个库——互斥键的前半截（#260）。
    /// 取自工厂给的 [`ConnectedDestination::database`]，不从请求里另取一份。
    database: String,
    staging_table: String,
    source_columns: Vec<String>,
    swap_columns: Vec<String>,
    target_table: String,
    primary_key: Vec<String>,
    max_rows_per_insert: usize,
    next_seq: u64,
    rows_written: u64,
    sealed: bool,
    /// 本次 run 的目标端连接（ADR-0037 §2）。**每个 run 各持一份**——
    /// 进程里不再有「那个 MySQL 连接」这种东西。
    destination: Arc<D>,
}

// 手写 `Clone`：派生版会给 `D` 加上 `D: Clone` 约束，而目的地本来就只按 `Arc` 共享。
impl<D> Clone for ActiveRun<D> {
    fn clone(&self) -> Self {
        Self {
            run_id: self.run_id.clone(),
            database: self.database.clone(),
            staging_table: self.staging_table.clone(),
            source_columns: self.source_columns.clone(),
            swap_columns: self.swap_columns.clone(),
            target_table: self.target_table.clone(),
            primary_key: self.primary_key.clone(),
            max_rows_per_insert: self.max_rows_per_insert,
            next_seq: self.next_seq,
            rows_written: self.rows_written,
            sealed: self.sealed,
            destination: Arc::clone(&self.destination),
        }
    }
}

/// 一个已受理但尚未进 `active_runs` 的 run 占的位子（#260）。
pub(crate) struct RunAdmission {
    pub(crate) database: String,
    pub(crate) target_table: String,
}

pub struct SinkService<F: DestinationFactory> {
    factory: F,
    /// 同时在飞的 run 数上限（#260），来自 `sink.toml` 的 `max_concurrent_runs`。
    max_concurrent_runs: usize,
    active_runs: Mutex<HashMap<String, ActiveRun<F::Dest>>>,
    /// 已经受理、但还没走完「连库 → 读元数据 → 预检 → 建暂存表」的 run（#260）。
    ///
    /// 为什么不能只看 `active_runs`：那张表要到 `open` 的**最后一行**才有条目，
    /// 而在它之前是好几秒的 MySQL 往返。只看它的话，两个指向同一张目标表的 run
    /// 会双双通过判定、双双开跑，互斥键等于没设。受理的那一刻先在这里占个位，
    /// 无论 `open` 成功还是中途出错，位子都由 `AdmissionGuard` 的 `Drop` 归还。
    admitting: Mutex<HashMap<String, RunAdmission>>,
    tombstones: Mutex<VecDeque<RunResponse>>,
    /// 最近一次连上目标端时读到的 MySQL 自述（#257）。
    ///
    /// **这是缓存，不是配置。** sink 启动时不连 MySQL，`GET /v1/agent/info` 也拿不到凭据，
    /// 所以「这台 agent 连的是哪个版本」只能在**手上有凭据的那些路径**上顺手记一笔：
    /// 目标端检查（`POST /v1/target/check`）与开跑（`POST /v1/runs`）。在那之前是
    /// `None`，信息接口照实报「未知」——猜一个 8.0 出来，正是本票要避免的那种静默。
    observed_mysql: Mutex<Option<MysqlServerInfo>>,
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
