//! 失败分类闭集 —— V1 成功标准第 4 条「错误信息直接指向原因」的分类那一半。
//!
//! 措辞那一半早就到位（ADR-0017 §4 点名到列与值），缺的是**分类**：失败原因此前只能靠读
//! `sink_code` 与人话反推，`SinkErrorKind` 又只有 `Transport` / `Response` 两档。
//! M4 那条要求区分的六类（Oracle 连接 / dblink / 类型映射 / 网络 / MySQL 写入 /
//! 校验）在本闭集里各占一个值，且**全部由已有信号确定性推导，不做人话文字匹配**。
//!
//! 决策与推导规则见 ADR-0029。

/// 一次失败的成因分类。闭集，**只增不删不改义**（与 ADR-0017 §5 的日志字段同一口径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// 配置或启动参数不合法：CLI 参数、`source.toml`、任务定义、业务日期。
    Config,
    /// 父进程没能把这次运行拉起来（物化任务文件失败、子进程 spawn 失败）。
    Orchestrator,
    /// 源端 SQL 形状预检未通过（source 本地成文，ADR-0016 §4）。
    ///
    /// **已退役、不再产生**：形状预检整段随 ADR-0036 §5 取消。保留这个值是因为本闭集
    /// 「只增不删不改义」——删掉它等于改写一个已发布的分类表。
    ShapePrecheck,
    /// 连不上 Oracle：Instant Client 初始化、监听器、登录。
    SourceConnect,
    /// 会话已建立后撞上远端库——即 dblink 不可用。
    SourceDblink,
    /// 源端查询或读取失败，含 `ORA-01555`。
    SourceQuery,
    /// 源端值过不了规范形式（ADR-0003），带列名与值。
    SourceValue,
    /// 目标端映射预检未通过（`PRECHECK_FAILED`）。
    MappingPrecheck,
    /// 与目标端的 HTTP 连接中断（`SinkErrorKind::Transport`）。
    Network,
    /// 目标端写 MySQL 失败：建暂存表、写批次、切换。
    SinkWrite,
    /// 目标端把某个业务值拒了（MySQL 1264 / 1292 / 1366 一类）。
    DataRejected,
    /// 目标端环境配置错（`max_allowed_packet` 一类），不是数据问题。
    SinkEnvironment,
    /// 目标端此刻腾不出手：目标表正被另一个 run 的切换事务占着（`SWAP_TARGET_BUSY`），
    /// 或目标表已被另一次运行占住、或 agent 的并发额度满了（`TARGET_TABLE_BUSY` /
    /// `RUN_QUOTA_EXCEEDED`，#260）。三者的处置是同一条：**等一等重跑**，不必查数据。
    TargetBusy,
    /// 校验门禁未通过（`VERIFY_FAILED`）。**只由目标端判**：门禁开在切换事务里，
    /// 那是唯一能把「数的那份」和「切的那份」钉成同一个快照的地方。
    ///
    /// source 本地对 commit 响应的复核**不落这一档**（#196）：同样这几件事目标端已经
    /// 先判过并放行了，本端还能判不过就只可能是两端判据不一致，那是 [`FailureKind::Defect`]。
    VerifyFailed,
    /// 程序缺陷，不是运行故障：`INTERNAL_*`、协议断言、序号/封口错，
    /// 以及 source 复核 commit 响应时发现两端判据不一致（#196）。
    Defect,
    /// 结局未知：进程消失、服务重启、commit 断连后仍判不出。
    Unknown,
    /// 到点了但没发起：上一次还没结束，本次跳过（#266）。
    ///
    /// 它是本闭集里唯一一个**什么都没做**的值：没连过库、没到过代理、目标表未被改动。
    /// 落成一行历史是为了让那个触发时刻有答案，不是为了报告一次故障。
    Skipped,
}

impl FailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Config => "CONFIG",
            Self::Orchestrator => "ORCHESTRATOR",
            Self::ShapePrecheck => "SHAPE_PRECHECK",
            Self::SourceConnect => "SOURCE_CONNECT",
            Self::SourceDblink => "SOURCE_DBLINK",
            Self::SourceQuery => "SOURCE_QUERY",
            Self::SourceValue => "SOURCE_VALUE",
            Self::MappingPrecheck => "MAPPING_PRECHECK",
            Self::Network => "NETWORK",
            Self::SinkWrite => "SINK_WRITE",
            Self::DataRejected => "DATA_REJECTED",
            Self::SinkEnvironment => "SINK_ENVIRONMENT",
            Self::TargetBusy => "TARGET_BUSY",
            Self::VerifyFailed => "VERIFY_FAILED",
            Self::Defect => "DEFECT",
            Self::Unknown => "UNKNOWN",
            Self::Skipped => "SKIPPED",
        }
    }

    /// 目标端错误码 → 分类。`code` 是 ADR-0010 的闭集之一。
    ///
    /// 未知码落 [`FailureKind::Defect`]：闭集只增不删，出现闭集外的码本身就是缺陷。
    pub fn from_sink_code(code: &str) -> Self {
        match code {
            "PRECHECK_FAILED" => Self::MappingPrecheck,
            "VERIFY_FAILED" => Self::VerifyFailed,
            "SWAP_TARGET_BUSY" | "TARGET_TABLE_BUSY" | "RUN_QUOTA_EXCEEDED" => Self::TargetBusy,
            "DATA_REJECTED" => Self::DataRejected,
            "SINK_ENVIRONMENT" => Self::SinkEnvironment,
            "STAGING_CREATE_FAILED" | "BATCH_WRITE_FAILED" | "SWAP_FAILED" => Self::SinkWrite,
            "SEQ_MISMATCH"
            | "RUN_SEALED"
            | "RUN_UNKNOWN"
            | "PAYLOAD_TOO_LARGE"
            | "BAD_REQUEST"
            | "INTERNAL_PRECHECK_ESCAPE"
            | "INTERNAL_ASSERTION_FAILED" => Self::Defect,
            _ => Self::Defect,
        }
    }
}

/// 会话已建立后，这些 Oracle 码指向的是**远端库**，也就是 dblink。
///
/// 同一批码出现在建连接那一步时指的是本地库，因此判定是**看在哪一步撞上的**，
/// 不是看码本身——见 [`oracle_kind`]。`ORA-02019` / `02020` / `02068` 只可能来自 dblink，
/// 但仍按同一条规则走，不为它们开特例。
const REMOTE_LINK_CODES: &[i32] = &[2019, 2020, 2068, 12154, 12170, 12203, 12514, 12541];

/// 按「在哪一步撞上的」把一个 Oracle 错误分类。
///
/// `connecting` 为真表示还在建本地会话；为假表示会话已经建起来了，正在 prepare / fetch。
pub fn oracle_kind(oracle_code: Option<i32>, connecting: bool) -> FailureKind {
    if connecting {
        return FailureKind::SourceConnect;
    }
    match oracle_code {
        Some(code) if REMOTE_LINK_CODES.contains(&code) => FailureKind::SourceDblink,
        _ => FailureKind::SourceQuery,
    }
}
