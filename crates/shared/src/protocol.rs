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

/// `POST /v1/runs` 的响应体。
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
