//! source ↔ sink 的线上报文形状。
//!
//! **这里只放形状，不放判定。** ADR-0027 §1 / A5 认下了「生成侧与判定侧两份实现」
//! 并把兜底定成 ADR-0014 的边界套件在两端各钉一遍——那条裁定管的是**判定实现**，
//! 与本模块无关：报文形状不含任何规则、不做任何裁决，只描述「这段 JSON 有哪些字段」。
//! 逐列类型判定仍按 ADR-0010 §3.1 集中在 sink，**一个字都不许搬进来**（#124）。
//!
//! 属性口径（#124）：
//! - `deny_unknown_fields` 只在**收**的时候生效，取两端并集 = 收方今天的行为。
//! - `skip_serializing_if` 只在**发**的时候生效，取发方今天的那一份 = 线上字节不变。
//!
//! 每个报文的正反两向都由 `crates/shared/tests/protocol_golden.rs` 钉死。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 一批行数据。`rows` 的每一行按 `source_columns` 的顺序给值，`None` 表示 NULL。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchPayload {
    pub seq: u64,
    pub rows: Vec<Vec<Option<String>>>,
}

/// 取列面的三档支持标记（ADR-0010 2026-08-16 增补二 §2）。
///
/// 由 source 侧 describe 时产出，`/api/columns` 直接序列化给 web 承载三档标记。
/// **`sink` 不得读它做任何判定**——它是 describe 面的展示提示，不是预检裁决。
/// 它随 `POST /runs` 的 `source_columns` 一起过线，只是因为两端共用同一个结构形状；
/// 逐列类型判定按 ADR-0010 §3.1 集中在 sink，由 sink 自己按
/// `type` / `precision` / `scale` / `length` / `fsp` 判。
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

/// 预检问题一条。**由 sink 侧算**（ADR-0010 2026-08-16 增补二 §1）——
/// web 不得把判定式复制进 TypeScript 重算一遍。
///
/// 它经由错误响应的 `details.issues` 过线，source 侧读它只为呈现。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrecheckIssue {
    pub column: String,
    pub source: String,
    pub target: String,
    pub rule: String,
    /// 动作型建议。永久可选。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
    /// Typed target-check presentation. It is deliberately not part of the
    /// legacy run-precheck wire shape.
    #[serde(skip)]
    pub check: Option<TargetCheckFinding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetCheckKind {
    MissingColumn,
    NullabilityMismatch,
    InsufficientLengthOrPrecision,
    PrimaryKeyMismatch,
    TypeNotWhitelisted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetCheckFinding {
    pub column: Option<String>,
    pub kind: TargetCheckKind,
    pub expected: String,
    pub actual: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetCheckRequest {
    pub target: TargetConnection,
    pub target_table: String,
    pub source_columns: Vec<SourceColumn>,
    pub primary_key: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetCheckResult {
    pub ok: bool,
    pub findings: Vec<TargetCheckFinding>,
    pub suggested_ddl: Option<String>,
}

/// 目标端 MySQL 的连接信息。**由 source 侧的数据源库读出、随请求过线**（ADR-0037 §1）——
/// sink 不再持有自己的那一份凭据。
///
/// **口令在这里是明文**，因为它已经过了线：静态加密只管落盘那一段（ADR-0037 §3），
/// 过线这一段靠的是部署前提（ADR-0037 §4：通道必须可信）。
/// **不许把它打进日志、历史或任何错误明细**——那是 ADR-0037 §1 认下的唯一流出路径之外的第二条。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetConnection {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub database: String,
}

/// 手写 `Debug`：派生版会把口令原样打进任何 `{:?}`，而这个结构会经过错误路径。
impl std::fmt::Debug for TargetConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TargetConnection")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("database", &self.database)
            .finish()
    }
}

/// `POST /v1/runs` 的请求体。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRunRequest {
    pub run_id: String,
    pub target_table: String,
    /// 本次运行的目标端连接（ADR-0037 §1/§2）。`sink.toml` 的 `mysql_dsn` / `database` 已退役，
    /// 库名也随本字段过来——暂存表 DDL 的库名限定按它取。
    pub target: TargetConnection,
    /// upsert 的去重键（ADR-0035 §2）。用户在构建器里勾，sink 侧预检去目标表核对
    /// 「约束确有、列在选中列里、列 NOT NULL」三条——缺约束时
    /// `ON DUPLICATE KEY UPDATE` 会**静默退化成纯 INSERT**，重跑就出重复行。
    /// 撤掉 DELETE 之后，那条预检是唯一挡住静默重复的东西。
    pub primary_key: Vec<String>,
    pub source_columns: Vec<SourceColumn>,
    /// 3.5 步：source 回发的值域校核结果。永久可选（#106 裁定 Q14/Q15）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_check_results: Option<Vec<RangeCheckResult>>,
}

/// `POST /v1/runs` 的响应体。**它有两种含义，别直接读字段**——
/// 「已开成」与「先去跑值域校核」都走 200，靠 `staging_table` 空串 + `range_check_columns`
/// 有值这一对暗号区分。暗号只在 [`crate::OpenOutcome`] 里认一次，两端都经由它构造与读取。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRunResponse {
    pub run_id: String,
    pub staging_table: String,
    pub columns_checked: usize,
    /// 3.5 步：sink 告诉 source「哪几列要跑值域校核」。永久可选（#106 裁定 Q14/Q15）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_check_columns: Option<Vec<RangeCheckColumn>>,
}

/// `POST /v1/runs/{run_id}/batches` 的响应体。请求体是 [`BatchPayload`]。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchResponse {
    pub seq: u64,
    pub rows_written: u64,
    pub next_seq: u64,
}

/// `POST /v1/runs/{run_id}/commit` 的请求体。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitRequest {
    pub total_batches: u64,
    pub total_rows: u64,
}

/// `POST /v1/runs/{run_id}/commit` 的响应体。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitResponse {
    pub source_rows: u64,
    pub staged_rows: u64,
    /// 新写入模型下**恒为 0**，字段保留（ADR-0035 §4）。这句话是真的：确实没删任何行。
    pub purged_rows: u64,
    pub swapped_rows: u64,
    pub count_ms: u64,
}

/// Delete the target rows whose latest successful writer is this run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupRunRequest {
    pub run_id: String,
    pub target_table: String,
    pub target: TargetConnection,
    pub primary_key: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupRunResponse {
    pub run_id: String,
    pub deleted_rows: u64,
}

/// `POST /v1/runs/{run_id}/abort` 的响应体。请求体是空对象 `{}`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbortResponse {
    pub run_id: String,
    pub staging_dropped: bool,
}

/// run 的终态（ADR-0012）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Terminal {
    Swapped,
    Discarded,
}

impl Terminal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Swapped => "SWAPPED",
            Self::Discarded => "DISCARDED",
        }
    }
}

/// `GET /v1/runs/{run_id}` 的响应体。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunResponse {
    pub run_id: String,
    pub staging_table: String,
    pub batches_received: u64,
    pub rows_written: u64,
    pub sealed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<Terminal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purged_rows: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swapped_rows: Option<u64>,
}

/// 错误响应的**外壳**：任何状态码非 2xx 的响应都是 `{"error": {...}}`。
///
/// **`details` 刻意保持无类型**（#124 Q13）：给它定型要回答「错误明细一共有几种形状」，
/// 那是失败分类（ADR-0029）的地盘，是设计不是搬家。source 侧靠字符串键去掏它，
/// 掏不到就静默降级——这条已知缺陷留档在 #124「本票明确不做」第 2 条。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

/// 见 [`ErrorEnvelope`]。字段顺序即线上字节顺序，**不要重排**。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    pub run_id: Option<String>,
    pub details: Value,
}

/// `GET /v1/agent/info` 的响应体（ADR-0044 §2）。
///
/// 它是**目标端 agent 的身份自述**：source 侧的 agent 注册、在线探测、以及每次运行开跑前的
/// 身份核对，读的都是这一份。三个字段各有各的用处，都不许省：
///
/// - `agent_id`：agent **跨重启稳定**的身份。注册时钉进 source 侧那条记录，之后每次探测
///   都拿它比一次——比不上就说明「同一个地址上换了一个 agent 应答」，那正是
///   ADR-0044 §1 要抓的那种静默（把地址指到别处、或另起一个 sink 顶上）。
/// - `name`：给人看的名字，agent 自报（默认取主机名）。**不作判据**，只进界面。
/// - `version`：agent 的构建版本，排障时用来判「两端是不是同一批二进制」。
///
/// - `mysql`：这台 agent **实际连到的那台 MySQL** 的自述，见 [`MysqlServerInfo`]。
///   与上面三个不同，它**是 `Option`**：agent 启动时并不连 MySQL（ADR-0037 §2，凭据随
///   每个 run 过线），所以进程刚起来时它就是 `None`——那是「还没观察到」，不是「8.0」。
///   一次目标端检查或一次开跑之后它才有值，之后一直报最近一次观察到的那一份。
///
/// **没有凭据字段，也永远不许加**：这个端点是未鉴权面（ADR-0024），任何进得来的人都读得到。
/// `mysql` 里的两项都是服务端自述的公开信息（`@@version` 与字符序），不是凭据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentInfo {
    pub agent_id: String,
    pub name: String,
    pub version: String,
    /// 见上。**旧版本 agent 不带这个字段**，所以是 `default` + `skip_serializing_if`：
    /// 新 source 读旧 agent 得到 `None`，与「新 agent 还没观察过」同一档处理。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mysql: Option<MysqlServerInfo>,
}

/// 目标端 MySQL 的自述：版本，以及生成建表语句要用的那一项字符序。
///
/// **两项都是从服务器上读回来的，不是推出来的。** 支持矩阵里 MySQL 8.0 与 5.7 的
/// utf8mb4 默认字符序不同（`utf8mb4_0900_ai_ci` / `utf8mb4_general_ci`），而生成建表语句的
/// 那一端（source）**一条 MySQL 连接都不建**，除 agent 上报之外没有第二个信息源。
///
/// 既然要上报，就顺手把字符序本身读回来，而不是在 source 侧按版本号做一次
/// 「5.7 就用 general_ci」的映射：那等于把 MySQL 的默认值抄一份进我们的代码，
/// 抄错、或者部署方把 `collation-server` 改过，都只会在建完表之后才暴露。
/// 版本号照样上报——它是给人看的排障信息，界面上那一列读的就是它。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MysqlServerInfo {
    /// `@@version` 原样，例如 `8.0.36` / `5.7.44-log`。
    pub version: String,
    /// `information_schema.CHARACTER_SETS` 里 utf8mb4 的 `DEFAULT_COLLATE_NAME`。
    pub utf8mb4_collation: String,
}
