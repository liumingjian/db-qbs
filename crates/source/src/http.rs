//! source 的 HTTP 面：请求进来、路由、handler、响应出去。
//!
//! **这里是 lib，不是 bin。** 整条 dispatch 从前住在 `[[bin]]` 里，于是
//! `tests/` 只能 spawn 一个进程、手搓 socket 才能碰到它——30 个 handler 因此有 5 个
//! 从没被任何测试调用过。现在唯一的入口是 `Api::handle(&Request) -> Response`：
//! 测试进程内直调，二进制那边只剩「监听 + 把 tiny_http 翻译成这两个类型」。
//!
//! 路由表是**数据**（`routes()`），不是一串 `if`。旧实现里那两条
//! 「带动作的路由必须排在前面」的注释规矩没有了：匹配分两趟，字面量样式先走一趟，
//! 带占位的样式后走一趟，所以 `/api/datasources/test-connection` 不可能被
//! `/api/datasources/{}` 吃掉，**无论表里怎么排**。

use std::collections::HashMap;
use std::fs::{self, OpenOptions, Permissions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{Local, Utc};
use db_qbs_shared::{write_log_line_with_fields, LogEvent, LogLevel, RunStage};
use rand::RngCore;
use signal_hook::consts::SIGTERM;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;

use crate::scheduler::SCHEDULE_TIME_FORMAT;
use crate::{
    cleared_cookie_header, embedded_web_asset, fetch_agent_info, generate_target_ddl,
    multipart_mail, Clock, CronSchedule, EmailTestResult, EmailTestStatus, HttpSinkClient,
    MailTransport, SinkClient,
    session_cookie_header, session_token_from_cookie_header, validate_builder_dblink,
    validate_source_sql, Agent, AgentEndpoint, AgentEvidence, AgentInput, AgentStore, AuthStore,
    ColumnPrecision, DatasourceInput, DatasourceStore, HistoryChange, HistoryStore, OracleAccess,
    OracleRowSource, Role, RowSource, RunEvidence, RunHistory, RunLogStore, RunLogWriter,
    RunParametersEvidence, RunTrigger, ScheduleRegistry,
    SourceColumn, SourceConfig, SourceEvidence, SourceReadError, TargetCheckRequest,
    TargetCheckResult, TargetConnection, TargetEvidence, Task, TaskConfig, TaskInput, TaskSpec,
    TaskStore, UnknownReason, EMAIL_LOG_PAGE_LIMIT, RUN_LOG_PAGE_LIMIT, SESSION_IDLE_SECONDS,
    USERNAME,
};

const MAX_REQUEST_BODY_BYTES: u64 = 1024 * 1024;
const DEFAULT_PREVIEW_LIMIT: usize = 10;
/// 「下次触发」一次给几个。给一个说不清 `*/n` 的取整，给一串就一目了然。
const SCHEDULE_PREVIEW_COUNT: usize = 5;
const MAX_PREVIEW_LIMIT: usize = 100;
const PREVIEW_CALL_TIMEOUT: Duration = Duration::from_secs(15);
pub(crate) const RUN_TASKS_DIRECTORY: &str = "run-tasks";

pub type RunRegistry = Arc<Mutex<RunState>>;

#[derive(Default)]
pub struct RunState {
    live_histories: HashMap<String, RunHistory>,
    active_runs: HashMap<String, ActiveRun>,
    /// 那些**没能释放掉**的目标表占用，按 `run_record_id` 记（#271）。
    ///
    /// 在飞登记只活到子进程被收尸为止，而占用泄漏这件事恰恰发生在它被摘掉的那一刻之后
    /// ——再往后问「这张目标表能不能再被写」，答得出口的只剩这张表。
    ///
    /// 键是 `run_record_id`（重试要发的是**那一次**运行的 abort，日志也落在那一支笔上），
    /// 但**判据是 [`StuckAbort::target`]**：占用是关于一张目标表的，不是关于一个任务的。
    stuck_aborts: HashMap<String, StuckAbort>,
}

impl RunState {
    /// 这个任务此刻有没有一次运行在飞。互斥键就是任务本身，见 [`ActiveRun::task_id`]。
    pub fn has_active_run(&self, task_id: &str) -> bool {
        self.active_runs.values().any(|run| run.task_id == task_id)
    }

    /// 这个任务此刻没结束的那几次运行，点名到 `run_record_id`（#270）。
    ///
    /// 与 [`Self::has_active_run`] 同一本账，只是拒绝删除时要把 ID 说出来——
    /// 只说「有运行在飞」，用户还得自己回列表里找是哪一次。
    pub fn active_run_ids(&self, task_id: &str) -> Vec<String> {
        self.active_runs
            .iter()
            .filter(|(_, run)| run.task_id == task_id)
            .map(|(run_record_id, _)| run_record_id.clone())
            .collect()
    }

    /// **这张目标表**此刻是个什么处境（#271）。`None` 是「没人占着」。
    ///
    /// 问的是表，不是运行、也不是任务：占用是 sink 按「同一台 agent 上同一个库的同一张表」
    /// 记的，所以一行运行记录能不能再跑，取决于它写的那张表被谁占着，而不是取决于
    /// 它自己那次运行怎么了。认不出目标表的行（旧记录没有证据）问不出答案，是 `None`。
    ///
    /// 两个来源合成一个答案：正在停、收尾还没走完的在飞运行是「还在释放」；
    /// 收尾时 abort 没成功、占用留在目标端的是「没释放掉」。界面拿它决定那一格
    /// 显示「停止中…」、「发起运行」还是「锁未释放，点此重试」——
    /// **服务端的判据只有这一处**，不许第二个地方各算各的。
    fn target_hold(&self, target: Option<&TargetTable>) -> Option<HoldReport> {
        let target = target?;
        if let Some((_, stuck)) = self.stuck_for_target(target) {
            // 重试正在路上时说的仍是「还在释放」：那一刻占用也许下一秒就没了，
            // 但它此刻确实还在，所以照样不许发起新运行。
            return Some(if stuck.retrying {
                HoldReport {
                    hold: TargetHold::Releasing,
                    message: None,
                }
            } else {
                HoldReport {
                    hold: TargetHold::Held,
                    message: Some(stuck.message.clone()),
                }
            });
        }
        self.active_runs
            .values()
            .find(|run| run.stop_requested.is_some() && run.target == *target)
            .map(|_| HoldReport {
                hold: TargetHold::Releasing,
                message: None,
            })
    }

    /// 占着这张目标表的那份没释放掉的占用，连它挂在哪次运行名下一起给出来。
    ///
    /// 至多一份：一张目标表同时只可能有一次运行，占用也就只有一份。
    fn stuck_for_target(&self, target: &TargetTable) -> Option<(&String, &StuckAbort)> {
        self.stuck_aborts
            .iter()
            .find(|(_, stuck)| stuck.target == *target)
    }

    /// 这个任务名下**欠着**的那几份占用，点名到 `run_record_id`（#270/#271）。
    ///
    /// 删任务那一关按它拦。它与 [`Self::target_hold`] 问的不是同一件事：那边问
    /// 「这张表能不能再被写」，这边问「删掉这个任务会不会把某份占用的重试入口一起删掉」
    /// ——重试那颗按钮长在这个任务的行上，任务没了，占用就再也没人点得到。
    pub fn held_run_ids(&self, task_id: &str) -> Vec<String> {
        self.stuck_aborts
            .iter()
            .filter(|(_, stuck)| stuck.task_id == task_id)
            .map(|(run_record_id, _)| run_record_id.clone())
            .collect()
    }

    /// 这台 agent 上此刻有几次**由本进程发起**的运行在飞（#266）。
    ///
    /// 它是并发额度那笔账的分子。分母（额度）由 agent 自报；分子在这里数，
    /// 因为目标库只经 agent 访问、而每一次访问都是本进程拉起来的——
    /// 除了它自己开的这些 run，source 侧没有第二个来源。
    pub fn in_flight_for_agent(&self, agent_id: &str) -> usize {
        self.active_runs
            .values()
            .filter(|run| run.agent_id == agent_id)
            .count()
    }
}

pub struct ActiveRun {
    /// 互斥键的**全部**：一个任务同时只许有一次运行在飞。
    /// 运行参数链退役之后，「同任务 + 同参数集」这个复合键退化成了任务本身。
    task_id: String,
    /// 这次运行发往哪台 agent。**它不是互斥键**，只用来数并发额度那笔账（#266）：
    /// 额度是**每台 agent** 的，所以分子也必须按 agent 分开数。
    agent_id: String,
    /// 这次运行写的是哪张目标表（#271）。**占用是关于它的**：停止之后那段窗口里
    /// 「还在释放」说的是这张表还没空出来，与这次运行属于哪个任务无关。
    target: TargetTable,
    child_pid: Option<u32>,
    /// 判定用的那一份，**是枚举不是字符串**：能不能取消这次运行由它一个人说了算
    /// （`RunStage::abort_allowed`）。子进程报来一个认不出的拼写时它是 `None`，
    /// 与「还没报过」同待——两端版本对不上时，唯一安全的回答是「我不知道它在做什么」。
    /// 原样的文本另有去处：运行历史那一份仍是字符串，见 `RunHistory::stage`。
    stage: Option<RunStage>,
    /// 这一刀是我们自己捅的，捅的时候是什么名义（#269/#272）。`None` 是「没捅过」。
    ///
    /// 标记打在**发信号那一刻**，不是事后推断的：子进程被信号带走时不会留下任何
    /// 「我是被停的」的痕迹，父进程唯一知道这件事的时刻就是它自己按下扳机的时刻。
    /// 终态兜底据此把这次运行记成「已由用户停止」或「服务重启，结局未知」，
    /// 而不是「进程消失」——主动停止、停服重启与被 OOM 杀掉在历史里必须分得开。
    stop_requested: Option<UnknownReason>,
}

/// 一张目标表的身份——**占用说的就是它**（#271）。
///
/// 判据与 sink 那头一个字对一个字：同一台 agent、同一个库、同一张表（表名不分大小写，
/// 见 `crates/sink/src/service.rs` 的 `admit`）。任务不在这个键里，因为任务只是
/// 「此刻谁在用它」：两个任务指同一张表、或者一个任务被删了重建，占用还是同一份，
/// 而按任务记的话，换个任务名再点一次「发起运行」就会一路跑到 sink 才撞回
/// `TARGET_TABLE_BUSY`。
#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetTable {
    agent_id: String,
    database: String,
    /// **存的时候就折成小写**：比较靠 `==`，大小写的宽严只在这一处兑现。
    table: String,
}

impl TargetTable {
    fn new(agent_id: &str, database: &str, table: &str) -> Self {
        Self {
            agent_id: agent_id.to_owned(),
            database: database.to_owned(),
            table: table.to_ascii_lowercase(),
        }
    }

    /// 一行运行记录写的是哪张目标表，从**开跑那一刻钉下的证据**里读。
    ///
    /// 读证据而不是回任务表现取：任务改了目标表、或者干脆被删了，这一行说的仍该是
    /// 当时那张表。证据不全（`evidence` 是后来才有的，旧记录没有）时是 `None`——
    /// 那样的行认不出自己占的是什么，也就报不出占用。
    fn from_history(history: &RunHistory) -> Option<Self> {
        let evidence = &history.evidence;
        Some(Self::new(
            &evidence.agent.as_ref()?.agent_id,
            &evidence.target.as_ref()?.database,
            &evidence.parameters.as_ref()?.target_table,
        ))
    }
}

/// 目标表占用**还在**的两种说法（#271）。没有第三种「已释放」——释放了就是 `None`。
///
/// 这是一个上报用的闭集，线上拼写与 [`RunStage`] 同一个规矩：前端照着它分支，
/// 改一个字就是改契约。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetHold {
    /// 停止请求已经发出，收尾还没走完：子进程也许还在死，abort 也许还在路上。
    Releasing,
    /// 收尾走完了，而占用没释放掉——abort 失败了。人手点一下才有下文。
    Held,
}

impl TargetHold {
    fn as_str(self) -> &'static str {
        match self {
            Self::Releasing => "RELEASING",
            Self::Held => "HELD",
        }
    }
}

/// 一条运行记录上报出去的那份占用处境（#271）。
struct HoldReport {
    hold: TargetHold,
    /// 没释放掉的原因，原话。「还在释放」时无话可说，所以是 `None`。
    message: Option<String>,
}

/// 一次**没能释放掉**的目标表占用（#271）。
///
/// 它是这套系统里唯一的手工补救入口的全部凭据：重试一次 abort 要的东西
/// （发给谁、说的是哪一次运行）都在这里，因为在飞投影那时早已不在了。
struct StuckAbort {
    /// 占着的是哪张目标表。**发起运行的那一关按它拦**：这张表还被占着的时候，
    /// 谁来点「发起运行」都只会换回一个 `TARGET_TABLE_BUSY`——包括另一个指着同一张表的
    /// 任务，和一个删了重建的同名任务。
    target: TargetTable,
    /// 占用是**哪个任务**的运行留下的。它不是占用的判据（判据是 `target`），
    /// 只回答一个问题：删掉这个任务，会不会把这份占用的重试入口一起删掉（#270）。
    task_id: String,
    /// sink 侧那个 21 字符的 run id：abort 认的是它。
    run_id: String,
    /// 当时那台 agent 的地址。占用在**当时那台**上，重试就得发到当时那台去。
    agent_base_url: Option<String>,
    /// 最近一次没成的原因，原样留着。
    message: String,
    /// 已经有人点了重试、这一趟还没回来。第二次点进来当场劝退，
    /// 免得两趟 abort 一起在路上。
    retrying: bool,
    /// 这条运行的日志笔。**跟着占用一起留下来**：重试再失败也要落一行
    /// `abort_failed`，而行号攥在笔身上——另起一支会把已有的行覆盖掉。
    run_log: Option<RunLogWriter>,
}

enum StartRunError {
    AlreadyRunning,
    /// 这个任务正在停，占用还没还回来（#271）。
    Stopping,
    /// 这次要写的那张目标表，占用还没还回来（#271）。硬拦在这里，而不是推过去
    /// 让目标端回一个 `TARGET_TABLE_BUSY`：本地知道的事没有理由绕一圈才说。
    /// 欠着占用的**不一定是这个任务上一次运行**——占用是按表记的。
    TargetHeld,
    Internal(String),
}

/// 一次**调度派发**的三种结局（#266）。
///
/// 三分而不是二分，是因为「这次没发出去」有两种性质完全不同的原因：额度满了是**等**，
/// 等一会儿就好；数据源解不开、agent 不在线、这个任务又被人手动跑起来了是**不发**，
/// 再等也不会自己变好。前者留在队里（界面上看得见排队原因），后者落一行历史说清楚。
pub(crate) enum DispatchOutcome {
    Started(String),
    Waiting(String),
    Refused(crate::ScheduledRefusalReason, String),
}
/// agent 注册表被后台探测线程与请求线程共用，所以它是 `Arc<Mutex<...>>`——
/// 底下是一条 SQLite 连接，`rusqlite::Connection` 不是 `Sync`。
pub type AgentRegistry = Arc<Mutex<AgentStore>>;

/// 发起一次运行需要的**全部**输入：跑哪个任务。
///
/// `deny_unknown_fields` 在这里是有牙齿的：老界面（或老脚本）送来的 `run_params`
/// 会被当场拒掉，而不是被静默忽略、让人以为参数生效了。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartRunInput {
    task_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuilderLinkInput {
    /// 取哪个 Oracle 数据源的表清单（ADR-0037 §1）。构建器不再吃进程级凭据。
    datasource_id: String,
    dblink: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuilderDatasourceInput {
    datasource_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuilderSqlInput {
    datasource_id: String,
    source_sql: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuilderPreviewInput {
    source_datasource_id: String,
    spec: TaskSpec,
    limit: Option<usize>,
}

/// 「下次触发」读数的请求体（#265）。
///
/// `cron` 缺席或为空是**合法的一次提问**——界面刚打开、还没人写表达式时就是这样，
/// 而那一刻正是它最需要知道「服务器现在是哪个时区」的时候。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SchedulePreviewInput {
    #[serde(default)]
    cron: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct PreviewResult {
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
    truncated: bool,
    elapsed_ms: u64,
}

/// 草稿测连的请求体（ADR-0039 §3）：吃的是**表单里当前填的那组值**，不是库里存的那条。
///
/// `datasource_id` 只有编辑态才有，用途单一——口令留空时去库里取那一份
/// （与保存的「留空 = 不改」是同一条解释规则，两处不许分岔）。新建态没有它。
///
/// **不加 `deny_unknown_fields`**：`DatasourceInput` 内部用了 `flatten`，两者不兼容
/// （与 `DatasourceInput` 自己的注释同一条理由）。
#[derive(Deserialize)]
struct DatasourceTestInput {
    #[serde(default)]
    datasource_id: Option<String>,
    #[serde(flatten)]
    draft: DatasourceInput,
}

/// 目标端元数据面的两个代理入口（ADR-0038 §3）：界面只报**数据源 id**，
/// 凭据由 source 在这里解一次再过线——与「测试连接」同一条路径（ADR-0037 §1/§8）。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetMetadataInput {
    datasource_id: String,
    /// 只有取列面要它；取表清单不带。
    #[serde(default)]
    target_table: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetCheckInput {
    // The task spec has names but no Oracle type/length metadata; target comparison must describe
    // the selected source columns through the bound source datasource first.
    source_datasource_id: String,
    target_datasource_id: String,
    target_table: String,
    spec: TaskSpec,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuilderColumnsInput {
    datasource_id: String,
    dblink: Option<String>,
    owner: String,
    table: String,
}

/// 取列请求。`column_precision` **不在任务定义里**（ADR-0036 §6）：目标表 DDL 生成吃的是
/// describe 回来的源列，属「取列」链，这份精度提示随请求一起来、用完即弃。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ColumnFetchInput {
    datasource_id: String,
    spec: TaskSpec,
    #[serde(default)]
    column_precision: Option<ColumnPrecision>,
    /// 目标端数据源。**可选**：这个端点只描述源端的列，取列本身不需要目标端。
    /// 给了就顺带把建表语句的字符序按那台 agent 上报的值生成（#257）；
    /// 不给就不写 `COLLATE`，与本票之前一个字不差。
    #[serde(default)]
    target_datasource_id: Option<String>,
}

/// 一次请求能碰到的**全部**进程状态。`Api::handle` 是这个 crate 对外的唯一 HTTP 入口。
///
/// 借用形态是刻意的：这些 store 的所有权归 `serve()`，`Api` 只在一次服务期内借着用，
/// 测试里同样可以就地拼一个出来。
pub struct Api<'a> {
    pub config: &'a SourceConfig,
    pub config_path: &'a Path,
    pub tasks: &'a TaskStore,
    pub datasources: &'a DatasourceStore,
    pub agents: &'a AgentRegistry,
    pub history: &'a HistoryStore,
    /// 子进程吐出的**原始**日志行。运行历史是它折叠之后的结果，两者同库不同表：
    /// 折叠会丢掉认不出来的事件，而排障要的恰恰是折叠之前的那份原文。
    pub run_logs: &'a RunLogStore,
    pub runs: &'a RunRegistry,
    /// 调度器此刻的状态（#266）。HTTP 面**只读**它——写它的是那条调度线程。
    /// 它在这里的唯一理由是「排队中的任务在界面上看得见」：队列活在一条后台线程的
    /// 内存里，不摆到 `Api` 上就没有任何一个请求答得出「它在等什么」。
    pub schedule: &'a ScheduleRegistry,
    /// 登录、会话与口令。**只护得到这个进程的 HTTP 面**——sink 那半边仍然没有鉴权。
    pub auth: &'a AuthStore,
    pub email_alerts: &'a crate::EmailAlertStore,
    pub alert_outbox: &'a crate::AlertOutboxStore,
    pub clock: Arc<dyn Clock>,
    pub mail_transport: Arc<dyn MailTransport>,
    pub describe_source: fn(&OracleAccess, &TaskSpec) -> Result<Vec<SourceColumn>, SourceReadError>,
}

/// HTTP 方法。认不出来的方法落进 `Other`，而路由表里只有前四种，所以它必然 404——
/// 与旧实现里那一串 `_ => not_found()` 同一个结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
    Other,
}

/// 一次请求：方法、路径、查询串、请求体。
///
/// `body` 是 `Result` 而不是 `Vec<u8>`：读请求体可能失败，而失败必须由**读它的那个
/// handler** 报成 400（不读请求体的 handler 从前就不受影响），所以这个错误得一路带到
/// `read_json_body` 那里，不能在翻译层就地吞掉。
pub struct Request {
    method: Method,
    path: String,
    query: Option<String>,
    headers: Vec<(String, String)>,
    body: Result<Vec<u8>, String>,
}

impl Request {
    /// 测试用的构造：URL 里带不带 `?query` 都行。
    pub fn new(method: Method, url: &str, body: Vec<u8>) -> Self {
        let (path, query) = match url.split_once('?') {
            Some((path, query)) => (path.to_owned(), Some(query.to_owned())),
            None => (url.to_owned(), None),
        };
        Self {
            method,
            path,
            query,
            headers: Vec::new(),
            body: Ok(body),
        }
    }

    /// 测试与桥接层共用：header 名大小写不敏感，与 HTTP 语义一致。
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn method(&self) -> Method {
        self.method
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// 请求体的字节。读失败时是 `Err`，超长时是一段比上限多一个字节的 `Ok`——
    /// 「超过 1 MiB」这个判定归 `read_json_body`，与从前同一处。
    pub fn body(&self) -> Result<&[u8], String> {
        match &self.body {
            Ok(body) => Ok(body),
            Err(error) => Err(error.clone()),
        }
    }
}

/// 一次响应。状态码 + 头 + 字节，没有别的。
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    /// 响应体当 UTF-8 文本读；测试里断言 JSON 用得上。
    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

type Handler = fn(&Api<'_>, &Request, &str) -> HttpResponse;

/// 一条路由归谁进。**它是路由表上的一列，不是 handler 里的一句 `if`**——
/// 判定因此只有一处，而 `every_route_reaches_its_handler` 会连这一列一起对账：
/// 新加一条路由却不声明它归哪一档，测试当场就红。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// 没有会话也进得去。**只有三条**：登录、退出、问一句「我登着吗」。
    Public,
    /// 要一张活着的会话票据，没有就是 401。
    Session,
    /// 只有管理员能进；操作员带着有效票据也只会得到稳定的 403。
    Administrator,
}

/// 一条路由。`pattern` 里最多有一个 `{}`，代表一段资源 id。
pub struct Route {
    pub method: Method,
    pub pattern: &'static str,
    pub access: Access,
    handler: Handler,
}

impl Route {
    /// 要登录才进得去的那一档。**绝大多数路由走这个构造**，
    /// 所以它叫 `new`：让「忘了想」落到更安全的一边。
    fn new(method: Method, pattern: &'static str, handler: Handler) -> Self {
        Self {
            method,
            pattern,
            access: Access::Session,
            handler,
        }
    }

    fn public(method: Method, pattern: &'static str, handler: Handler) -> Self {
        Self {
            method,
            pattern,
            access: Access::Public,
            handler,
        }
    }

    fn administrator(method: Method, pattern: &'static str, handler: Handler) -> Self {
        Self { method, pattern, access: Access::Administrator, handler }
    }

    /// 带占位的样式比字面量样式**后**匹配，见 `route_api`。
    pub fn has_placeholder(&self) -> bool {
        self.pattern.contains("{}")
    }
}

/// 把请求路径按样式对一遍：对上了就回样式里 `{}` 那一段，没有占位时回空串。
///
/// 这一个函数取代了从前那四个几乎一样的 path parser——`resource_id_from_path`、
/// `agent_action_id_from_path`、`test_connection_id_from_path`、`cancel_run_id_from_path`
/// 都是同一段逻辑把前后缀内联了一遍。id 里不许有 `/`、不许为空，与从前一字不差。
fn match_pattern<'a>(pattern: &str, path: &'a str) -> Option<&'a str> {
    let Some((prefix, suffix)) = pattern.split_once("{}") else {
        return (pattern == path).then_some("");
    };
    let rest = path.strip_prefix(prefix)?;
    let captured = rest.strip_suffix(suffix)?;
    if captured.is_empty() || captured.contains('/') {
        return None;
    }
    Some(captured)
}

/// 全部 API 路由。**表里的先后不承重**——`route_api` 分两趟走，字面量永远压过占位。
pub fn routes() -> &'static [Route] {
    static ROUTES: OnceLock<Vec<Route>> = OnceLock::new();
    ROUTES.get_or_init(|| {
        use Method::{Delete, Get, Post, Put};
        vec![
            // 会话那三条是**全表仅有的公开路由**：没有它们，登录本身也会撞 401。
            Route::public(Get, "/api/session", |state, request, _id| {
                handle_session_state(request, state)
            }),
            Route::public(Post, "/api/session", |state, request, _id| {
                handle_login(request, state)
            }),
            Route::public(Delete, "/api/session", |state, request, _id| {
                handle_logout(request, state)
            }),
            Route::new(Put, "/api/password", |state, request, _id| {
                handle_change_password(request, state)
            }),
            Route::administrator(Get, "/api/operator-account", |state, _request, _id| {
                handle_get_operator_account(state)
            }),
            Route::administrator(Put, "/api/operator-account", |state, request, _id| {
                handle_update_operator_account(request, state)
            }),
            Route::administrator(Get, "/api/email-alert-settings", |state, _request, _id| {
                handle_get_email_alert_settings(state)
            }),
            Route::administrator(Put, "/api/email-alert-settings", |state, request, _id| {
                handle_update_email_alert_settings(request, state)
            }),
            Route::administrator(Post, "/api/email-alert-settings/test", |state, _request, _id| {
                handle_test_email_alert_settings(state)
            }),
            Route::administrator(Get, "/api/email-deliveries", |state, request, _id| {
                handle_list_email_deliveries(state, request.query())
            }),
            Route::administrator(Get, "/api/email-logs", |state, request, _id| {
                handle_list_email_logs(state, request.query())
            }),
            Route::administrator(
                Post,
                "/api/email-deliveries/{}/retry",
                |state, _request, id| handle_retry_email_delivery(state, id),
            ),
            Route::new(Post, "/api/columns", |state, request, _id| {
                handle_column_fetch(request, state)
            }),
            Route::new(Post, "/api/builder/tables", |state, request, _id| {
                handle_builder_tables(request, state)
            }),
            Route::new(Post, "/api/builder/dblinks", |state, request, _id| {
                handle_builder_dblinks(request, state)
            }),
            Route::new(Post, "/api/builder/sql-columns", |state, request, _id| {
                handle_builder_sql_columns(request, state)
            }),
            Route::new(Post, "/api/builder/columns", |state, request, _id| {
                handle_builder_columns(request, state)
            }),
            Route::new(Post, "/api/builder/sql", |_state, request, _id| {
                handle_builder_sql(request)
            }),
            Route::new(Post, "/api/builder/preview", |state, request, _id| {
                handle_builder_preview(request, state)
            }),
            // 「这条 cron 下一次什么时候响」——纯算，不碰任何存储，因此 `_state`。
            // 归在 `builder/` 下面是因为它服务的是**草稿**：任务还没保存，人已经想知道
            // 自己写的那一行是不是他以为的那个意思。
            Route::new(Post, "/api/builder/schedule", |_state, request, _id| {
                handle_schedule_preview(request)
            }),
            Route::new(Get, "/api/schedule", |state, _request, _id| {
                handle_schedule_state(state)
            }),
            Route::new(Get, "/api/agents", |state, _request, _id| {
                handle_list_agents(state)
            }),
            Route::administrator(Post, "/api/agents", |state, request, _id| {
                handle_register_agent(request, state)
            }),
            Route::administrator(Post, "/api/agents/{}/probe", |state, _request, id| {
                handle_probe_agent(state, id)
            }),
            Route::administrator(Put, "/api/agents/{}", |state, request, id| {
                handle_update_agent(request, state, id)
            }),
            Route::administrator(Delete, "/api/agents/{}", |state, _request, id| {
                handle_delete_agent(state, id)
            }),
            Route::new(Get, "/api/datasources", |state, _request, _id| {
                handle_list_datasources(state.datasources)
            }),
            Route::administrator(Post, "/api/datasources", |state, request, _id| {
                handle_create_datasource(request, state)
            }),
            Route::administrator(Post, "/api/datasources/test-connection", |state, request, _id| {
                handle_test_datasource_draft(request, state)
            }),
            Route::administrator(
                Post,
                "/api/datasources/{}/test-connection",
                |state, _request, id| handle_test_datasource(state, id),
            ),
            Route::new(Get, "/api/datasources/{}", |state, _request, id| {
                handle_get_datasource(state.datasources, id)
            }),
            Route::administrator(Put, "/api/datasources/{}", |state, request, id| {
                handle_update_datasource(request, state, id)
            }),
            Route::administrator(Delete, "/api/datasources/{}", |state, _request, id| {
                handle_delete_datasource(state, id)
            }),
            Route::new(Post, "/api/target/tables", |state, request, _id| {
                handle_target_tables(request, state)
            }),
            Route::new(Post, "/api/target/columns", |state, request, _id| {
                handle_target_columns(request, state)
            }),
            Route::new(Post, "/api/target/check", |state, request, _id| {
                handle_target_check(request, state)
            }),
            Route::new(Post, "/api/runs", |state, request, _id| {
                handle_start_run(request, state)
            }),
            Route::new(Post, "/api/runs/{}/cancel", |state, _request, id| {
                handle_cancel_run(state.runs, id)
            }),
            // 重试一次没能释放掉的目标表占用（#271）。**这是全系统唯一的手工补救入口**：
            // 在它之前，占用泄漏之后的唯一出路是运维自己去调目标端的 abort 接口。
            Route::new(Post, "/api/runs/{}/release", |state, _request, id| {
                handle_release_target(state, id)
            }),
            Route::new(Get, "/api/runs", |state, request, _id| {
                handle_list_history(state.runs, state.history, state.alert_outbox, request.query())
            }),
            Route::new(Get, "/api/runs/{}", |state, _request, id| {
                handle_get_run(state.runs, state.history, state.alert_outbox, id)
            }),
            // 原始日志行的**游标增量**取用。放在 `/api/runs/{}` 之后不承重：
            // 两条的段数不同（4 段 vs 3 段），匹配上互不干扰。
            Route::new(Get, "/api/runs/{}/logs", |state, request, id| {
                handle_run_logs(state, id, request.query())
            }),
            Route::new(Get, "/api/tasks", |state, _request, _id| {
                handle_list_tasks(state.tasks)
            }),
            Route::new(Post, "/api/tasks", |state, request, _id| {
                handle_create_task(request, state)
            }),
            Route::new(Get, "/api/tasks/{}", |state, _request, id| {
                handle_get_task(state.tasks, id)
            }),
            Route::new(Get, "/api/tasks/{}/curl", |state, request, id| {
                handle_task_curl(request, state, id)
            }),
            Route::new(Put, "/api/tasks/{}", |state, request, id| {
                handle_update_task(request, state, id)
            }),
            Route::new(Delete, "/api/tasks/{}", |state, _request, id| {
                handle_delete_task(state, id)
            }),
        ]
    })
}

impl Api<'_> {
    /// 这个 crate 的 HTTP 面**唯一**的入口。
    pub fn handle(&self, request: &Request) -> Response {
        let path = request.path();
        if path == "/api" || path.starts_with("/api/") {
            return self.route_api(request);
        }
        if request.method() == Method::Get {
            if let Some(asset) = embedded_web_asset(path) {
                return embedded_response(asset.body.into_owned(), asset.content_type, path);
            }
        }
        not_found()
    }

    /// 两趟匹配：先字面量样式，再带占位的样式。
    ///
    /// 这就是「顺序不承重」的全部机制。从前 `/api/datasources/test-connection` 要靠
    /// 排在按 id 那条前面才不被吃掉，`/api/agents/{}/probe` 同理；现在即便把表倒过来写，
    /// 结果也一个字节不变。
    ///
    /// **鉴权就拦在这里，在分发之前。** 它不在任何一个 handler 里，也不在 `serve()` 里：
    /// 放进 handler 就会有第 31 个 handler 忘了写，放进 `serve()` 则整个 `tests/api.rs`
    /// 都碰不到它（那半边不开 socket）。
    fn route_api(&self, request: &Request) -> Response {
        let token = request
            .header("Cookie")
            .and_then(session_token_from_cookie_header);
        // 认一次就顺手把滑动窗口往前推了，所以它必须只发生一次，不能每个 handler 各来一遍。
        let session = match token {
            Some(token) => match self.auth.resolve_session(token, self.clock.now()) {
                Ok(Some(identity)) => Some((token, identity)),
                Ok(None) => None,
                Err(error) => return internal_error(error),
            },
            None => None,
        };
        for placeholders in [false, true] {
            for route in routes() {
                if route.has_placeholder() != placeholders || route.method != request.method() {
                    continue;
                }
                let Some(resource_id) = match_pattern(route.pattern, request.path()) else {
                    continue;
                };
                if route.access == Access::Public {
                    return (route.handler)(self, request, resource_id);
                }
                let Some((token, identity)) = &session else {
                    return unauthorized();
                };
                if route.access == Access::Administrator && identity.role != Role::Admin {
                    let mut response = forbidden();
                    response.headers.push((
                        "Set-Cookie".to_owned(),
                        session_cookie_header(token, SESSION_IDLE_SECONDS),
                    ));
                    return response;
                }
                let mut response = (route.handler)(self, request, resource_id);
                // 服务端那份窗口刚被推过了，cookie 也得跟着续期——否则浏览器会在
                // **登录满 8 小时**那一刻把票据丢掉，而服务端那一份其实还活着，
                // 于是「闲置 8 小时才踢」在用户那边变成了「登录 8 小时后必被踢」。
                response.headers.push((
                    "Set-Cookie".to_owned(),
                    session_cookie_header(token, SESSION_IDLE_SECONDS),
                ));
                return response;
            }
        }
        // 一条都没匹配上。**没登录的一律回 401，不回 404**：两者的差别足以让门外的人
        // 把整张路由表枚举出来，而那正是这道门要拦的第一步。
        if session.is_some() {
            not_found()
        } else {
            unauthorized()
        }
    }
}

/// 登录接口的输入。用户名这一栏留着不是因为有第二个账号，而是因为
/// 一个只有口令框的登录表单会让人以为自己走错了页面。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginInput {
    username: String,
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PasswordChangeInput {
    current_password: String,
    new_password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorAccountInput {
    enabled: bool,
    #[serde(default)]
    password: Option<String>,
}

/// 「我登着吗」。**它是公开的**：让还没登录的人问这一句，前端才能在首屏决定
/// 摆登录页还是摆应用，而不必先撞一个 401 再从错误里反推。
fn handle_session_state(request: &Request, state: &Api<'_>) -> HttpResponse {
    let account = match request
        .header("Cookie")
        .and_then(session_token_from_cookie_header)
    {
        Some(token) => match state.auth.resolve_session(token, state.clock.now()) {
            Ok(account) => account,
            Err(error) => return internal_error(error),
        },
        None => None,
    };
    json_response(
        200,
        &json!({
            "authenticated": account.is_some(),
            "username": account.as_ref().map(|account| account.username.as_str()),
            "role": account.as_ref().map(|account| account.role),
        }),
    )
}

/// 登录。**失败不限速、不冷却、不锁定**（所有者裁定）：出厂口令长期有效，
/// 所以这道门的实际强度是「能连上端口的人试两次即可进入」。这句话不写在界面上，
/// 但也不能不写在代码里。
fn handle_login(request: &Request, state: &Api<'_>) -> HttpResponse {
    let input: LoginInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    match state.auth.verify_password(&input.username, &input.password) {
        Ok(true) => {}
        // 账号错与口令错**回同一句话**：分开报只会告诉试口令的人账号叫什么。
        Ok(false) => return login_refused(),
        Err(error) => return internal_error(error),
    }
    let issued = match state
        .auth
        .issue_session(&input.username, state.clock.now())
    {
        Ok(issued) => issued,
        // 校验通过到发票之间若管理员恰好禁用了操作员，对外仍是同一种登录失败。
        Err(error) if error == "账号未启用" => return login_refused(),
        Err(error) => return internal_error(error),
    };
    let role = if input.username == USERNAME { Role::Admin } else { Role::Operator };
    let mut response = json_response(200, &json!({
        "authenticated": true,
        "username": input.username,
        "role": role,
    }));
    response.headers.push((
        "Set-Cookie".to_owned(),
        session_cookie_header(&issued.token, issued.max_age_seconds),
    ));
    response
}

fn login_refused() -> HttpResponse {
    json_response(401, &json!({ "error": { "message": "账号或口令不正确" } }))
}

/// 退出登录。**销的只有这一张票**——别处登着的同一个账号不受影响。
///
/// 没带票据也回 200：退出是个幂等动作，「你本来就没登着」不是一次失败。
fn handle_logout(request: &Request, state: &Api<'_>) -> HttpResponse {
    if let Some(token) = request
        .header("Cookie")
        .and_then(session_token_from_cookie_header)
    {
        if let Err(error) = state.auth.forget(token) {
            return internal_error(error);
        }
    }
    let mut response = json_response(200, &json!({ "authenticated": false }));
    response
        .headers
        .push(("Set-Cookie".to_owned(), cleared_cookie_header()));
    response
}

/// 改口令。要先输当前口令，改完**除了这一张之外的会话全部失效**。
///
/// 这条路由归 `Access::Session`，所以走到这里时票据必然是活的——
/// 从 cookie 里再取一次只是为了知道「留哪一张」。
fn handle_change_password(request: &Request, state: &Api<'_>) -> HttpResponse {
    let input: PasswordChangeInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    let keep = request
        .header("Cookie")
        .and_then(session_token_from_cookie_header)
        .unwrap_or_default();
    let account = match state.auth.session_identity(keep) {
        Ok(Some(account)) => account,
        Ok(None) => return unauthorized(),
        Err(error) => return internal_error(error),
    };
    match state.auth.change_password(
        &account.username,
        &input.current_password,
        &input.new_password,
        keep,
    )
    {
        Ok(()) => json_response(200, &json!({ "message": "口令已修改" })),
        Err(error) => bad_request(error),
    }
}

fn handle_get_operator_account(state: &Api<'_>) -> HttpResponse {
    match state.auth.operator_account() {
        Ok(account) => json_response(200, &account),
        Err(error) => internal_error(error),
    }
}

fn handle_update_operator_account(request: &Request, state: &Api<'_>) -> HttpResponse {
    let input: OperatorAccountInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    match state.auth.update_operator(input.enabled, input.password.as_deref()) {
        Ok(()) => handle_get_operator_account(state),
        Err(error) => bad_request(error),
    }
}

fn handle_get_email_alert_settings(state: &Api<'_>) -> HttpResponse {
    match state.email_alerts.get() {
        Ok(settings) => json_response(200, &settings),
        Err(error) => internal_error(error),
    }
}

fn handle_update_email_alert_settings(request: &Request, state: &Api<'_>) -> HttpResponse {
    let input: crate::EmailAlertSettingsInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    match state.email_alerts.update(input) {
        Ok(settings) => json_response(200, &settings),
        Err(error) => bad_request(error),
    }
}

fn handle_test_email_alert_settings(state: &Api<'_>) -> HttpResponse {
    let settings = match state.email_alerts.get() {
        Ok(settings) => settings,
        Err(error) => return internal_error(error),
    };
    record_email_log(
        state,
        LogLevel::Info,
        LogEvent::EmailTestStarted,
        None,
        None,
        json!({
            "enabled": settings.enabled,
            "provider_preset": settings.provider_preset,
            "smtp_host": &settings.smtp_host,
            "smtp_port": settings.smtp_port,
            "smtp_security": settings.smtp_security,
            "smtp_username": &settings.smtp_username,
            "has_smtp_secret": settings.has_smtp_secret,
            "sender_address": &settings.sender_address,
            "recipient_count": settings.recipients.len(),
        }),
    );
    let delivery = match state.email_alerts.test_delivery_settings() {
        Ok(delivery) => delivery,
        Err(error) => {
            record_email_log(
                state,
                LogLevel::Error,
                LogEvent::EmailTestCompleted,
                None,
                None,
                json!({
                    "status": "FAILED",
                    "recipient_count": settings.recipients.len(),
                    "success_count": 0,
                    "failure_count": 0,
                    "error": &error,
                }),
            );
            return persist_test_result(state, EmailTestStatus::Failed, Some(error));
        }
    };

    let subject = format!("[db-qbs][{}][测试] 邮件配置验证", settings.instance_name);
    let mut latest_error = None;
    let mut success_count = 0;
    let mut failure_count = 0;
    for recipient in &settings.recipients {
        let plain = format!(
            "db-qbs 测试邮件\n\n实例：{}\n收件人：{}\n\n收到此邮件表示已保存的 SMTP 配置可以发送邮件。",
            settings.instance_name, recipient
        );
        let html = format!(
            "<!doctype html><html><body><h1>db-qbs 测试邮件</h1><p>实例：{}</p><p>收件人：{}</p><p>收到此邮件表示已保存的 SMTP 配置可以发送邮件。</p></body></html>",
            escape_html(&settings.instance_name),
            escape_html(recipient),
        );
        let mail = match multipart_mail(
            &delivery.sender_address,
            &delivery.sender_name,
            recipient,
            &subject,
            plain,
            html,
        ) {
            Ok(mail) => mail,
            Err(_) => {
                failure_count += 1;
                let error = "生成测试邮件失败".to_owned();
                latest_error = Some(error.clone());
                record_email_log(
                    state,
                    LogLevel::Error,
                    LogEvent::EmailTestRecipientCompleted,
                    None,
                    None,
                    json!({
                        "recipient": recipient,
                        "status": "FAILED",
                        "error": error,
                    }),
                );
                continue;
            }
        };
        match state.mail_transport.send(&delivery, &mail) {
            Ok(()) => {
                success_count += 1;
                record_email_log(
                    state,
                    LogLevel::Info,
                    LogEvent::EmailTestRecipientCompleted,
                    None,
                    None,
                    json!({
                        "recipient": recipient,
                        "status": "SUCCESS",
                        "error": null,
                    }),
                );
            }
            Err(error) => {
                failure_count += 1;
                let code = error.code();
                let message = error.sanitized_message().to_owned();
                latest_error = Some(message.clone());
                record_email_log(
                    state,
                    LogLevel::Error,
                    LogEvent::EmailTestRecipientCompleted,
                    None,
                    None,
                    json!({
                        "recipient": recipient,
                        "status": "FAILED",
                        "error_code": code,
                        "error": message,
                    }),
                );
            }
        }
    }

    let status = if latest_error.is_some() {
        EmailTestStatus::Failed
    } else {
        EmailTestStatus::Success
    };
    record_email_log(
        state,
        if status == EmailTestStatus::Success {
            LogLevel::Info
        } else {
            LogLevel::Error
        },
        LogEvent::EmailTestCompleted,
        None,
        None,
        json!({
            "status": match status {
                EmailTestStatus::Success => "SUCCESS",
                EmailTestStatus::Failed => "FAILED",
            },
            "recipient_count": settings.recipients.len(),
            "success_count": success_count,
            "failure_count": failure_count,
            "error": &latest_error,
        }),
    );
    persist_test_result(state, status, latest_error)
}

fn handle_list_email_deliveries(state: &Api<'_>, query: Option<&str>) -> HttpResponse {
    let run_record_id = url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .find(|(key, _)| key == "run_record_id")
        .map(|(_, value)| value.into_owned());
    match state
        .alert_outbox
        .delivery_history(run_record_id.as_deref())
    {
        Ok(deliveries) => json_response(200, &deliveries),
        Err(error) => internal_error(error),
    }
}

fn handle_list_email_logs(state: &Api<'_>, query: Option<&str>) -> HttpResponse {
    let mut after: i64 = 0;
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        if key.as_ref() != "after" {
            continue;
        }
        match value.parse::<i64>() {
            Ok(parsed) if parsed >= 0 => after = parsed,
            _ => return bad_request("查询参数 after 必须是非负整数".to_owned()),
        }
    }
    let lines = match state.alert_outbox.email_logs().lines_after(after) {
        Ok(lines) => lines,
        Err(error) => return internal_error(error),
    };
    let has_more = lines.len() >= EMAIL_LOG_PAGE_LIMIT;
    let next_after = lines.last().map_or(after, |line| line.seq);
    json_response(
        200,
        &json!({
            "after": after,
            "next_after": next_after,
            "has_more": has_more,
            "lines": lines,
        }),
    )
}

fn handle_retry_email_delivery(state: &Api<'_>, delivery_id: &str) -> HttpResponse {
    match state
        .alert_outbox
        .manual_retry(delivery_id, state.clock.now(), &state.email_alerts)
    {
        Ok(crate::ManualRetryOutcome::Retried(delivery)) => json_response(200, &delivery),
        Ok(crate::ManualRetryOutcome::NotFound) => not_found(),
        Ok(crate::ManualRetryOutcome::Ineligible) => {
            bad_request("只有已耗尽重试窗口的失败投递才能手动重试".to_owned())
        }
        Err(error) => internal_error(error),
    }
}

fn persist_test_result(
    state: &Api<'_>,
    status: EmailTestStatus,
    error: Option<String>,
) -> HttpResponse {
    let result = EmailTestResult {
        status,
        tested_at: state.clock.now().to_rfc3339(),
        error,
    };
    match state.email_alerts.record_test_result(&result) {
        Ok(()) => json_response(200, &result),
        Err(error) => internal_error(error),
    }
}

fn record_email_log(
    state: &Api<'_>,
    level: LogLevel,
    event: LogEvent,
    run_id: Option<&str>,
    task: Option<&str>,
    fields: serde_json::Value,
) {
    let _ = state
        .alert_outbox
        .email_logs()
        .append(level, event, run_id, task, fields);
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn handle_start_run(request: &Request, state: &Api<'_>) -> HttpResponse {
    let input: StartRunInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    let task = match state.tasks.get(&input.task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return not_found(),
        Err(error) => return internal_error(error),
    };
    // 两端连接在发起时解一次（ADR-0037 §8）：解不开就当场拒，不要等到子进程起来了才炸。
    let access = match oracle_access(state, &task.source_datasource_id) {
        Ok(access) => access,
        Err(error) => return bad_request(error),
    };
    let target = match state
        .datasources
        .target_connection(&task.target_datasource_id)
    {
        Ok(target) => target,
        Err(error) => return bad_request(error),
    };
    // 目标端 agent 也在这里解一次并当场核对（ADR-0044 §4）。**它比其余三条链路更要紧**：
    // 一次搬运会往目标库写，而「写得进去」曾经与「那台 agent 活着」毫无关系——
    // 现场把 agent 停掉、同步照跑，正是这条缺失造成的。
    let agent = match resolve_target_agent(state, &task.target_datasource_id) {
        Ok(agent) => agent,
        Err(error) => return agent_failure(error),
    };

    match start_run(
        state.config,
        state.config_path,
        &task,
        access,
        target,
        agent_endpoint(&agent),
        state.history,
        state.run_logs,
        state.runs,
        Arc::clone(&state.clock),
        RunTrigger::Manual,
    ) {
        Ok(run_record_id) => json_response(202, &json!({ "run_record_id": run_record_id })),
        Err(StartRunError::AlreadyRunning) => json_response(
            409,
            &json!({ "error": { "message": "该任务已有一次运行进行中" } }),
        ),
        // 「正在停」与「已经在跑」分成两句（#271）：停止是异步的，点完停止到占用真的
        // 还回来之间有一段窗口，那段窗口里回一句「已有一次运行进行中」会让人以为
        // 自己那一下停止没生效。
        Err(StartRunError::Stopping) => json_response(
            409,
            &json!({ "error": { "message": "该任务正在停止，等目标表占用释放后才能再跑" } }),
        ),
        Err(StartRunError::TargetHeld) => json_response(
            409,
            &json!({ "error": { "message": "这张目标表的占用还没释放，先把它释放掉再发起" } }),
        ),
        Err(StartRunError::Internal(error)) => internal_error(error),
    }
}

fn handle_cancel_run(runs: &RunRegistry, run_record_id: &str) -> HttpResponse {
    // 锁只用来**抄一份**这条 run 此刻的阶段与 PID，随即松手（#255）。判阶段、发信号、
    // 拼回话都在锁外做：`kill(2)` 本身不阻塞，但攥着这把锁的每一微秒，
    // 别的线程的 `/api/runs` 与运行详情都在排队。
    let snapshot = match runs.lock() {
        Ok(registry) => registry
            .active_runs
            .get(run_record_id)
            .map(|run| (run.stage, run.child_pid)),
        Err(_) => return internal_error("run 控制锁已损坏".to_owned()),
    };
    let Some((run_stage, child_pid)) = snapshot else {
        return not_found();
    };
    let refused = |message: &str| json_response(409, &json!({ "error": { "message": message } }));
    // 停不停得了只由 `RunStage::abort_allowed` 一个人说了算——它就是 CONTEXT.md
    // 那条封口点不变量。**理由**另说：拒绝的原因分三种，所以按变体挑话，
    // 而不是对 `abort_allowed` 取反。这里不写通配分支，于是将来往闭集里加一格
    // 会在这儿变成编译错误，而不是悄悄落进「说不清为什么」的那一句。
    let Some(stage) = run_stage else {
        return refused("run 尚未进入可取消阶段");
    };
    if !stage.abort_allowed() {
        return refused(match stage {
            // 权限没了：暂存表的处置权已整个交给 sink。
            RunStage::Committing => "已过封口点，停不了",
            // 对象没了：进程早已退出。
            RunStage::Succeeded | RunStage::Failed => "run 已经结束，没有可停的进程",
            // `abort_allowed` 对这两格为真，走不到这里；写全只为不留通配。
            RunStage::Preparing | RunStage::Streaming => "run 尚未进入可取消阶段",
        });
    }
    let Some(pid) = child_pid else {
        return internal_error("run 子进程尚未登记".to_owned());
    };
    // 标记打在**发信号之前**（#269）。信号一旦发出，子进程可能下一微秒就没了，而监督线程
    // 紧接着就要读这个标记来决定这次运行记成「已由用户停止」还是「进程消失」——
    // 先 kill 后标记就是在跟一次进程死亡赛跑，输了的那次会被记成意外死亡。
    // 加锁与发信号仍是**两趟**：`kill(2)` 留在锁外（见函数开头那段）。
    mark_stop_requested(runs, run_record_id, UnknownReason::StoppedByUser);
    match send_sigterm(pid) {
        Ok(()) => json_response(202, &json!({ "message": "已发送 SIGTERM" })),
        Err(error) => {
            // 信号没发出去，这次运行还在跑：标记得撤回去，否则它将来真的意外死掉时
            // 会顶着一句「已由用户停止」，而那不是事实。
            unmark_stop_requested(runs, run_record_id);
            internal_error(error)
        }
    }
}

/// `POST /api/runs/{}/release` —— 重试一次没能释放掉的目标表占用（#271）。
///
/// 路径上的是**哪一行按下了这一下**，释放的却是**那一行写的那张目标表**上欠着的占用
/// ——两者通常是同一次运行，但不必是：占用是按表记的，同一张表上另一次运行欠下的占用，
/// 在这一行上照样显示成「锁未释放」，那就得在这一行上点得动。
///
/// 只对**确实欠着占用**的表开门：没有欠着就是 404，因为那种情况下没有已知的占用可释放，
/// 凭空发一次 abort 只是朝目标端乱开枪。
///
/// 幂等性由 sink 那头保证（未知的 run 回 200）。这里不自动重试、也不定时重试——
/// 「abort 不承诺可靠性」那条没变，变的只是**人现在有地方点这一下**。
fn handle_release_target(state: &Api<'_>, run_record_id: &str) -> HttpResponse {
    let Some(target) = run_target_table(state, run_record_id) else {
        return not_found();
    };
    // 锁只用来抄一份重试要用的东西，并把「有人正在重试」挂上去，随即松手：
    // abort 是一次最长 30 秒的 HTTP 往返，攥着这把锁做它，整个界面都要陪着等。
    let taken = match state.runs.lock() {
        Ok(mut registry) => {
            let Some(holder) = registry
                .stuck_for_target(&target)
                .map(|(holder, _)| holder.clone())
            else {
                // **不是通用 404**：这条运行认得，只是它写的那张表没有欠着的占用。
                // 凭空发一次 abort 是朝目标端乱开枪，而把这句话说清楚，人才知道自己
                // 看到的是「已经释放了」而不是「这个接口坏了」。
                return json_response(
                    404,
                    &json!({ "error": { "message": "这次运行没有待释放的目标表占用" } }),
                );
            };
            let stuck = registry
                .stuck_aborts
                .get_mut(&holder)
                .expect("上一句刚在同一把锁下找到它");
            if stuck.retrying {
                return json_response(
                    409,
                    &json!({ "error": { "message": "正在重试释放目标表占用，请稍候" } }),
                );
            }
            stuck.retrying = true;
            (holder, stuck.run_id.clone(), stuck.agent_base_url.clone())
        }
        Err(_) => return internal_error("run 控制锁已损坏".to_owned()),
    };
    // 往下动的是**欠着占用的那次运行**，不一定是路径上的那一次。
    let (holder, run_id, agent_base_url) = taken;
    let outcome = match agent_base_url.as_deref() {
        Some(base_url) => release_target_hold(base_url, &run_id),
        None => Err(MISSING_AGENT_URL_MESSAGE.to_owned()),
    };
    // 收尾与重启补发那一趟共用一处：成了整条摘掉，没成就把原因留在占用上，
    // 并落一行 `abort_failed`——与第一次失败同一个形状。
    match settle_stuck_abort(state.runs, &holder, &run_id, outcome) {
        Ok(()) => json_response(200, &json!({ "message": "目标表占用已释放" })),
        Err(message) => sink_failure(message),
    }
}

/// 这一行运行记录写的是哪张目标表。
///
/// 在飞的那份与落库的那份读的是同一样东西（开跑时钉下的证据），所以先问投影、
/// 再问库，两条路答出来的是同一个键。这条运行根本不存在时是 `None`。
fn run_target_table(state: &Api<'_>, run_record_id: &str) -> Option<TargetTable> {
    if let Ok(registry) = state.runs.lock() {
        if let Some(history) = registry.live_histories.get(run_record_id) {
            return TargetTable::from_history(history);
        }
    }
    TargetTable::from_history(&state.history.get(run_record_id).ok()??)
}

/// 记下「这一刀是我们自己捅的」，并连**什么名义**一起记下（#269/#272）：
/// 人按的是「已由用户停止」，停服收摊时捅的是「服务重启，结局未知」。
fn mark_stop_requested(runs: &RunRegistry, run_record_id: &str, reason: UnknownReason) {
    set_stop_requested(runs, run_record_id, Some(reason));
}

/// 把上一句撤回去：信号根本没发出去，这次运行还在跑。
///
/// **两个名字而不是一个参数**：`mark_stop_requested(runs, id, None)` 在调用处
/// 读起来像「标记停止」，而它做的事正相反——这是一次回滚，那就该在名字上说出来。
fn unmark_stop_requested(runs: &RunRegistry, run_record_id: &str) {
    set_stop_requested(runs, run_record_id, None);
}

fn set_stop_requested(runs: &RunRegistry, run_record_id: &str, reason: Option<UnknownReason>) {
    if let Ok(mut registry) = runs.lock() {
        if let Some(run) = registry.active_runs.get_mut(run_record_id) {
            run.stop_requested = reason;
        }
    }
}

fn send_sigterm(pid: u32) -> Result<(), String> {
    let pid = i32::try_from(pid).map_err(|_| format!("run 子进程 PID 超出范围：{pid}"))?;
    // SAFETY: libc::kill has no memory-safety preconditions; pid fits the platform's pid_t.
    if unsafe { libc::kill(pid, SIGTERM) } == 0 {
        Ok(())
    } else {
        Err(format!(
            "向 run 子进程发送 SIGTERM 失败：{}",
            io::Error::last_os_error()
        ))
    }
}

fn handle_get_run(
    runs: &RunRegistry,
    history_store: &HistoryStore,
    alert_outbox: &crate::AlertOutboxStore,
    run_record_id: &str,
) -> HttpResponse {
    // 「在飞投影」与「占用处境」**一把锁读完**：分两次拿锁，答出来的会是两个时刻的
    // 拼接——「已经不在飞了，可占用还在释放」这种自相矛盾的回答就是那么来的。
    let live = match runs.lock() {
        Ok(registry) => registry
            .live_histories
            .get(run_record_id)
            .cloned()
            .map(|record| {
                let hold = registry.target_hold(TargetTable::from_history(&record).as_ref());
                (record, hold)
            }),
        Err(_) => return internal_error("run 投影锁已损坏".to_owned()),
    };
    if let Some((record, hold)) = live {
        return json_response(
            200,
            &json!({
                "run_record_id": run_record_id,
                "run_id": record.run_id,
                "task_name": record.task_name,
                // 手动还是调度，**在飞的时候就要分得开**（#266）：一次半夜自己跑起来的
                // 运行，最该问「这谁发起的」的时刻正是它还在跑的时候。
                "trigger": record.trigger,
                "source_sql": record.source_sql,
                "evidence": record.evidence,
                "staging_table": record.staging_table,
                // 发起时刻。界面上的「已用时」要的是**墙钟**，而 `ms` 是批次耗时的累加：
                // 开跑前计数的那几十秒里一个批次都还没有，`ms` 因此是 0，
                // 于是一次真的在跑的运行会先自称「已用时 00:00」将近一分钟（UX 评审 P1-8）。
                "started_at": record.started_at,
                "stage": record.stage,
                "total_rows": record.total_rows,
                "precount_ms": record.precount_ms,
                "seq": record.seq,
                "rows_pushed": record.rows_pushed,
                "bytes": record.bytes,
                "ms": record.ms,
                "last_ts": record.last_ts,
                // 目标表占用还在不在（#271）。在飞的时候它只有一种非空取值：
                // 有人按过停止、这次运行正在收尾。
                "target_hold": hold.as_ref().map(|report| report.hold.as_str()),
                "target_hold_message": hold.as_ref().and_then(|report| report.message.clone()),
                "live": true,
            }),
        );
    }
    // 落库的那一行：它已经定死了，所以占用另拿一次锁去问不会拼出自相矛盾的回答。
    // 问的是**这一行写的那张目标表**，答案可能来自别人——同一张表上另一次运行欠下的
    // 占用，一样让这一行不能重跑。
    match history_store.get(run_record_id) {
        Ok(Some(history)) => {
            let hold = match runs.lock() {
                Ok(registry) => registry.target_hold(TargetTable::from_history(&history).as_ref()),
                Err(_) => return internal_error("run 投影锁已损坏".to_owned()),
            };
            let alert = match alert_outbox.summary_for_run(run_record_id) {
                Ok(alert) => alert,
                Err(error) => return internal_error(error),
            };
            history_response(&history, hold.as_ref(), alert)
        }
        Ok(None) => not_found(),
        Err(error) => internal_error(error),
    }
}

/// `GET /api/runs/{}/logs?after=<序号>` —— 原始日志行的增量取用。
///
/// **游标增量轮询，不是 SSE、不是长连接**：这套后端是同步阻塞栈，没有异步运行时，
/// 一条挂着不放的连接会整根占死一个工作线程；而运行日志的自然节奏（界面每秒问一次）
/// 用游标就够了。`after` 是上一次拿到的最后一个 `seq`，不给就是从头开始。
///
/// 运行进行中与已结束走的是**同一条路**：这里只读表，不问那条运行是不是还活着。
/// `live` 只是顺带告诉调用方还该不该接着轮询，它不改变返回哪些行。
fn handle_run_logs(state: &Api<'_>, run_record_id: &str, query: Option<&str>) -> HttpResponse {
    let mut after: i64 = 0;
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        if key.as_ref() != "after" {
            continue;
        }
        match value.parse::<i64>() {
            Ok(parsed) if parsed >= 0 => after = parsed,
            _ => return bad_request("查询参数 after 必须是非负整数".to_owned()),
        }
    }
    // 认不认得这条运行由**运行历史**说了算，不由日志表说了算：日志的保留期（7 天）
    // 比历史（默认 90 天）短得多，一条老运行的原文早已清掉，但它本身还在——
    // 那种情况的正确答案是「200，一行都没有」，不是 404。
    //
    // 两个问题（认不认得、还在不在跑）**一把锁读完**：分两次拿锁的话，中间隔着一次
    // 状态变化，答出来的会是两个时刻的组合——「不认得，但正在跑」这种自相矛盾的回答
    // 就是这么来的。
    let (known, live) = match state.runs.lock() {
        Ok(registry) => (
            registry.live_histories.contains_key(run_record_id),
            registry.active_runs.contains_key(run_record_id),
        ),
        Err(_) => return internal_error("run 投影锁已损坏".to_owned()),
    };
    if !known {
        match state.history.get(run_record_id) {
            Ok(Some(_)) => {}
            Ok(None) => return not_found(),
            Err(error) => return internal_error(error),
        }
    }
    let lines = match state.run_logs.lines_after(run_record_id, after) {
        Ok(lines) => lines,
        Err(error) => return internal_error(error),
    };
    // 一页取完了并不代表没有更多：`has_more` 让调用方立刻带着新游标再来一次，
    // 而不必等到下一个轮询周期才把积压的行取完。
    let has_more = lines.len() >= RUN_LOG_PAGE_LIMIT;
    let next_after = lines.last().map_or(after, |line| line.seq);
    json_response(
        200,
        &json!({
            "run_record_id": run_record_id,
            "after": after,
            "next_after": next_after,
            "has_more": has_more,
            "live": live,
            "lines": lines,
        }),
    )
}

fn handle_list_history(
    runs: &RunRegistry,
    history_store: &HistoryStore,
    alert_outbox: &crate::AlertOutboxStore,
    query: Option<&str>,
) -> HttpResponse {
    let mut task_id = None;
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        if key.as_ref() == "task_id" {
            task_id = Some(value.into_owned());
        }
    }
    match history_store.list(task_id.as_deref()) {
        Ok(history) => match merge_live_history(runs, history, task_id.as_deref()) {
            Ok(merged) => {
                // 占用处境挨条补上（#271）。作业中心那一格读的就是这个字段：
                // 一行任务只看它最近那次运行，而「能不能再跑」正写在那一行上。
                let holds = match runs.lock() {
                    Ok(registry) => merged
                        .iter()
                        .map(|row| registry.target_hold(TargetTable::from_history(row).as_ref()))
                        .collect::<Vec<_>>(),
                    Err(_) => return internal_error("run 投影锁已损坏".to_owned()),
                };
                let values = merged
                    .iter()
                    .zip(&holds)
                    .map(|(history, hold)| {
                        alert_outbox
                            .summary_for_run(&history.run_record_id)
                            .map(|alert| history_value(history, hold.as_ref(), alert))
                    })
                    .collect::<Result<Vec<_>, _>>();
                let values = match values {
                    Ok(values) => values,
                    Err(error) => return internal_error(error),
                };
                json_response(200, &values)
            }
            Err(error) => internal_error(error),
        },
        Err(error) => internal_error(error),
    }
}

fn merge_live_history(
    runs: &RunRegistry,
    mut history: Vec<RunHistory>,
    task_id: Option<&str>,
) -> Result<Vec<RunHistory>, String> {
    let live = runs
        .lock()
        .map_err(|_| "run 投影锁已损坏".to_owned())?
        .live_histories
        .values()
        .filter(|row| task_id.is_none_or(|wanted| row.task_id == wanted))
        .cloned()
        .collect::<Vec<_>>();
    for row in live {
        match history
            .iter()
            .position(|candidate| candidate.run_record_id == row.run_record_id)
        {
            Some(index) => history[index] = row,
            None => history.push(row),
        }
    }
    history.sort_by(|left, right| {
        right
            .started_at_ms()
            .cmp(&left.started_at_ms())
            .then_with(|| right.run_record_id.cmp(&left.run_record_id))
    });
    Ok(history)
}

fn history_response(
    history: &RunHistory,
    hold: Option<&HoldReport>,
    alert: Option<crate::RunAlertSummary>,
) -> HttpResponse {
    json_response(200, &history_value(history, hold, alert))
}

/// 落库的那一行，加上**只有内存里才知道**的两件事：这条运行还活着没有，
/// 以及它占的目标表还回来了没有（#271）。两者都不进 SQLite——
/// 进程一死它们就不成立了，存下来只会在重启之后骗人。
fn history_value(
    history: &RunHistory,
    hold: Option<&HoldReport>,
    alert: Option<crate::RunAlertSummary>,
) -> Value {
    let mut value =
        serde_json::to_value(history).expect("serializing a run history response must succeed");
    let object = value
        .as_object_mut()
        .expect("run history serializes as an object");
    object.insert("live".to_owned(), Value::Bool(false));
    object.insert(
        "target_hold".to_owned(),
        hold.map_or(Value::Null, |report| json!(report.hold.as_str())),
    );
    object.insert(
        "target_hold_message".to_owned(),
        hold.and_then(|report| report.message.clone())
            .map_or(Value::Null, Value::String),
    );
    object.insert(
        "alert".to_owned(),
        serde_json::to_value(alert).expect("serializing alert summary must succeed"),
    );
    value
}

/// 把注册表里那条记录压成子进程要用的端点（ADR-0044 §4）。
/// **`instance_id` 一起钉进去**：子进程开跑前还要再核一次身份，
/// 从「发起」到「真的开始写」之间那段时间里 agent 被换掉，仍然要被抓住。
fn agent_endpoint(agent: &Agent) -> AgentEndpoint {
    AgentEndpoint {
        agent_id: agent.agent_id.clone(),
        name: agent.name.clone(),
        base_url: agent.base_url.clone(),
        instance_id: agent.instance_id.clone(),
    }
}

/// 调度器派一次活（#266）。**与「立即运行」那颗按钮走同一条路**——解两端连接、
/// 当场核对 agent、再 `start_run`——只在两处不同：结局是三分的
/// [`DispatchOutcome`]（要能表达「排队等着」），以及运行历史上记的是
/// [`RunTrigger::Scheduled`]。
///
/// 额度那一关**开在这里，不在 agent 那一头**：额度满时不发请求、留在队里，
/// 而不是推过去吃一个 `RUN_QUOTA_EXCEEDED`。分母是 agent 自报的那份额度，
/// 没自报（旧 agent）就按**一次一个**——那是唯一一个绝不会被拒的取值。
pub(crate) fn dispatch_scheduled_run(state: &Api<'_>, task: &Task) -> DispatchOutcome {
    let access = match oracle_access(state, &task.source_datasource_id) {
        Ok(access) => access,
        Err(error) => {
            return DispatchOutcome::Refused(
                crate::ScheduledRefusalReason::SourceDatasourceUnavailable,
                error,
            );
        }
    };
    let target = match state
        .datasources
        .target_connection(&task.target_datasource_id)
    {
        Ok(target) => target,
        Err(error) => {
            return DispatchOutcome::Refused(
                crate::ScheduledRefusalReason::TargetDatasourceUnavailable,
                error,
            );
        }
    };
    let agent = match resolve_target_agent(state, &task.target_datasource_id) {
        Ok(agent) => agent,
        Err(error) => {
            return DispatchOutcome::Refused(
                crate::ScheduledRefusalReason::TargetAgentUnavailable,
                error,
            );
        }
    };
    // 没自报额度的 agent 按**一次一个**算，而这 1 不是保守的猜测，是那台 agent 的实情：
    // #260 之前的 sink 是 `for request in server.incoming_requests()`，一个请求一个请求地
    // 处理，同一时刻只跑得动一个 run。不带 `max_concurrent_runs` 字段的 agent 恰恰就是那批。
    // 所以这里既不该改成 4（sink.toml 的默认值——那是**配了新版本的人**选的数，不是这台老
    // agent 的能力），也不该改成任何别的数：1 是唯一一个既不撞 `RUN_QUOTA_EXCEEDED`、
    // 又没有白白空着额度的取值。
    let quota = agent.max_concurrent_runs.unwrap_or(1) as usize;
    let in_flight = match state.runs.lock() {
        Ok(runs) => runs.in_flight_for_agent(&agent.agent_id),
        Err(_) => {
            return DispatchOutcome::Refused(
                crate::ScheduledRefusalReason::Internal,
                "run 控制锁已损坏".to_owned(),
            );
        }
    };
    if in_flight >= quota {
        return DispatchOutcome::Waiting(format!(
            "目标端 agent「{}」的并发额度已满（在飞 {in_flight}，上限 {quota}），排队等待",
            agent.name
        ));
    }
    match start_run(
        state.config,
        state.config_path,
        task,
        access,
        target,
        agent_endpoint(&agent),
        state.history,
        state.run_logs,
        state.runs,
        Arc::clone(&state.clock),
        RunTrigger::Scheduled,
    ) {
        Ok(run_record_id) => DispatchOutcome::Started(run_record_id),
        Err(StartRunError::AlreadyRunning) => DispatchOutcome::Refused(
            crate::ScheduledRefusalReason::PreviousRunActive,
            "上次尚未结束，本次跳过".to_owned(),
        ),
        Err(StartRunError::Stopping) => DispatchOutcome::Refused(
            crate::ScheduledRefusalReason::PreviousRunStopping,
            "上次正在停止，本次跳过".to_owned(),
        ),
        Err(StartRunError::TargetHeld) => DispatchOutcome::Refused(
            crate::ScheduledRefusalReason::TargetHeld,
            "目标表的占用还没释放，本次跳过".to_owned(),
        ),
        Err(StartRunError::Internal(error)) => {
            DispatchOutcome::Refused(crate::ScheduledRefusalReason::Internal, error)
        }
    }
}

fn start_run(
    config: &SourceConfig,
    config_path: &Path,
    task: &Task,
    access: OracleAccess,
    target: TargetConnection,
    agent: AgentEndpoint,
    history_store: &HistoryStore,
    run_log_store: &RunLogStore,
    runs: &RunRegistry,
    clock: Arc<dyn Clock>,
    trigger: RunTrigger,
) -> Result<String, StartRunError> {
    let run_record_id = generate_run_record_id();
    // 历史里钉的是**当次实际执行**的语句文本：规格以后改了它也不跟着变。
    let mut history = RunHistory::accepted(
        &run_record_id,
        &task.task_id,
        &task.spec.source_sql(),
        clock.now(),
    );
    // 名字也钉在这一行上：它是展示标签，改名随时可能发生，而这条记录说的是
    // 「当时那次运行」。回头去任务表现取，改一次名就会把过去所有运行记录的名字
    // 一起改写（#259）。
    history.task_name = task.name.clone();
    // 谁发起的也钉在这一行上（#266）：夜里那次是自动跑的还是有人手动补的，
    // 事后只有这一列答得出来。
    history.trigger = trigger.as_str().to_owned();
    history.evidence = RunEvidence {
        source: Some(SourceEvidence {
            datasource_id: task.source_datasource_id.clone(),
            connect_string: access.connect_string.clone(),
            username: access.username.clone(),
            client_lib_dir: access.client_lib_dir.clone(),
        }),
        target: Some(TargetEvidence {
            datasource_id: task.target_datasource_id.clone(),
            host: target.host.clone(),
            port: target.port,
            database: target.database.clone(),
            username: target.username.clone(),
        }),
        agent: Some(AgentEvidence {
            agent_id: agent.agent_id.clone(),
            name: agent.name.clone(),
            base_url: agent.base_url.clone(),
            instance_id: agent.instance_id.clone(),
        }),
        parameters: Some(RunParametersEvidence {
            target_table: task.spec.target_table.clone(),
            columns: task.spec.columns.clone(),
            primary_key: task.spec.primary_key.clone(),
            write_mode: task.spec.write_mode,
            pre_sql: task.spec.pre_sql.clone(),
            source_sql: task.spec.source_sql(),
        }),
    };
    // 占用的键在这里就凑齐：agent、库、表三样都在手边，而它们各自的来源
    // （注册表、数据源、任务定义）此后都可能变，唯有开跑这一刻的取值算数。
    let target_table =
        TargetTable::new(&agent.agent_id, &target.database, &task.spec.target_table);
    register_active_run(runs, &run_record_id, &task.task_id, &agent.agent_id, target_table)?;
    if let Err(error) = history_store.insert(&history, clock.now(), config.history_retention_days) {
        remove_active_run(runs, &run_record_id);
        return Err(StartRunError::Internal(error));
    }
    runs.lock()
        .map_err(|_| StartRunError::Internal("run 投影锁已损坏".to_owned()))?
        .live_histories
        .insert(run_record_id.clone(), history.clone());

    let task_path = match materialize_task(config, task, access, target, agent, &run_record_id) {
        Ok(path) => path,
        Err(error) => {
            let now = clock.now();
            history.mark_parent_failure(error.clone(), now);
            let _ = history_store.finalize(&history, now, config.history_retention_days);
            remove_live_history(runs, &run_record_id);
            remove_active_run(runs, &run_record_id);
            return Err(StartRunError::Internal(error));
        }
    };
    let mut child = match Command::new(&config.run_executable)
        .arg("--config")
        .arg(config_path)
        .arg("--task")
        .arg(&task_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = fs::remove_file(&task_path);
            let message = format!("启动 run 子进程失败：{error}");
            let now = clock.now();
            history.mark_parent_failure(message.clone(), now);
            let _ = history_store.finalize(&history, now, config.history_retention_days);
            remove_live_history(runs, &run_record_id);
            remove_active_run(runs, &run_record_id);
            return Err(StartRunError::Internal(message));
        }
    };
    runs.lock()
        .map_err(|_| StartRunError::Internal("run 控制锁已损坏".to_owned()))?
        .active_runs
        .get_mut(&run_record_id)
        .expect("an active run remains registered until child reap")
        .child_pid = Some(child.id());
    let stdout = child
        .stdout
        .take()
        .expect("stdout is available after configuring it as piped");

    let worker_runs = Arc::clone(runs);
    let worker_record_id = run_record_id.clone();
    let worker_history_store = history_store.clone();
    let run_log_writer = RunLogWriter::new(
        run_log_store.clone(),
        run_record_id.clone(),
        task.task_id.clone(),
        history.started_at_ms(),
    );
    let retention_days = config.history_retention_days;
    let worker_clock = Arc::clone(&clock);
    thread::spawn(move || {
        supervise_run(
            child,
            stdout,
            task_path,
            worker_record_id,
            worker_history_store,
            run_log_writer,
            retention_days,
            worker_runs,
            worker_clock,
        )
    });
    Ok(run_record_id)
}

fn materialize_task(
    config: &SourceConfig,
    task: &Task,
    access: OracleAccess,
    target: TargetConnection,
    agent: AgentEndpoint,
    run_record_id: &str,
) -> Result<PathBuf, String> {
    let directory = config.data_dir.join(RUN_TASKS_DIRECTORY);
    fs::create_dir_all(&directory).map_err(|error| format!("创建临时任务目录失败：{error}"))?;
    let path = directory.join(format!("task-{run_record_id}.toml"));
    let task_config = task_config_from_task(task, access, target, agent);
    let contents = toml::to_string(&task_config)
        .map_err(|error| format!("序列化临时任务定义失败：{error}"))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|error| format!("创建临时任务定义失败：{error}"))?;
    file.write_all(contents.as_bytes())
        .map_err(|error| format!("写入临时任务定义失败：{error}"))?;
    fs::set_permissions(&path, Permissions::from_mode(0o600))
        .map_err(|error| format!("设置临时任务定义权限失败：{error}"))?;
    Ok(path)
}

/// 临时任务文件里带着**解出来的明文凭据**（ADR-0037 §1/§8）。
/// 落盘形态由 [`materialize_task`] 保证：0600、跑完即删、启动与退出各扫一次。
fn task_config_from_task(
    task: &Task,
    access: OracleAccess,
    target: TargetConnection,
    agent: AgentEndpoint,
) -> TaskConfig {
    TaskConfig {
        spec: task.spec.clone(),
        oracle: access,
        target,
        agent,
    }
}

fn supervise_run(
    mut child: Child,
    stdout: impl Read,
    task_path: PathBuf,
    run_record_id: String,
    history_store: HistoryStore,
    mut run_log: RunLogWriter,
    retention_days: u64,
    runs: RunRegistry,
    clock: Arc<dyn Clock>,
) {
    let mut terminal_observed = false;
    // 子进程自己那一刀 abort 也可能没砍下去（#271）。砍不下去时它写一行 `abort_failed`，
    // 而那一行是父进程**唯一**能知道这件事的地方：占用因此留在了目标端，后果与父进程
    // 补发失败一模一样，界面上就该是同一句话。凭据在见到那一行时当场抄下来——
    // 终态一到，在飞投影当场被摘掉，事后再抄什么也没有了。
    let mut child_abort_failure: Option<(RunWrapup, String)> = None;
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else {
            break;
        };
        {
            let stdout = io::stdout();
            let mut writer = stdout.lock();
            let _ = writeln!(writer, "{line}");
        }
        // 落库在解析**之前**：解析不出 JSON 的行也照存。运行历史那边可以忽略它，
        // 排障那边不行——「来什么显什么，不吞」。
        run_log.write(&line);
        let Ok(log) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        // 只有失败路径上才会有这一行：子进程的 `abort_best_effort` 是它自己收拾
        // 暂存表的那一下，跑成了的运行从不发 abort。
        if log["event"] == "abort_failed" {
            let message = log["message"]
                .as_str()
                .unwrap_or("目标端 abort 失败")
                .to_owned();
            child_abort_failure = Some((wrapup_snapshot(&runs, &run_record_id), message));
        }
        let Some((change, history)) = apply_log_line(&runs, &run_record_id, &log) else {
            continue;
        };
        let is_terminal = change == HistoryChange::Terminal;
        let requires_persistence = change != HistoryChange::MemoryOnly;
        terminal_observed |= is_terminal;
        if requires_persistence {
            let persisted = if is_terminal {
                history_store
                    .finalize(&history, clock.now(), retention_days)
                    .map(|_| ())
            } else {
                history_store.save(&history, clock.now(), retention_days)
            };
            if persisted.is_err() {
                continue;
            }
        }
        if is_terminal {
            remove_live_history(&runs, &run_record_id);
        }
    }
    let _ = child.wait();
    let _ = fs::remove_file(task_path);
    if !terminal_observed {
        // 收尾要用的那几样东西**只住在这条在飞投影里**，而它下面就要被摘掉，所以先抄一份。
        let wrapup = wrapup_snapshot(&runs, &run_record_id);
        // 「进程消失」与「已由用户停止」是同一个事实的两种成因：子进程被信号带走时
        // 什么都留不下，唯一知道这是不是我们自己要的，是发信号时打下的那个标记。
        let reason = wrapup
            .stop_requested
            .unwrap_or(UnknownReason::ProcessDisappeared);
        let history = runs.lock().ok().and_then(|mut registry| {
            let history = registry.live_histories.get_mut(&run_record_id)?;
            history.mark_unknown(reason, clock.now());
            Some(history.clone())
        });
        if let Some(history) = history {
            let _ = history_store.finalize(&history, clock.now(), retention_days);
        }
        // 子进程没来得及说完的那句 abort，父进程替它说（#269）。**必须在 `child.wait()`
        // 之后**：暂存表要等发起写入的那个进程死透了才动得。
        if let Some(message) = abort_on_behalf_of_child(&wrapup, &mut run_log) {
            // 没释放掉：这件事必须活得比在飞登记更久（#271）。登记下一行就摘了，
            // 而占用还在目标端挂着——不记下来，界面下一秒就会把这个任务显示成可以重跑，
            // 而那是**假的**：再发起一次只会撞回一个 `TARGET_TABLE_BUSY`。
            record_stuck_abort(&runs, &run_record_id, &wrapup, message, run_log);
        }
    } else if let Some((wrapup, message)) = child_abort_failure {
        // 子进程自己走完了终态，所以父进程不补 abort（那只会在已封口的 run 上换回 409）；
        // 可它路上那一刀没砍下去，占用照样挂着。**占用还在就不许显示成可以重跑**，
        // 这一条不分是谁那一刀没成（#271）。
        record_stuck_abort(&runs, &run_record_id, &wrapup, message, run_log);
    }
    remove_live_history(&runs, &run_record_id);
    remove_active_run(&runs, &run_record_id);
}

/// 子进程退出后，父进程收尾要用到的全部事实。
struct RunWrapup {
    /// 这次运行是谁的。占用没释放掉时，重试那颗按钮长在**这个任务**的行上，
    /// 所以删这个任务的那一关也要认它（#270）。
    task_id: String,
    /// 这次运行写的是哪张目标表。占用没释放掉时，拦住的是**这张表**的下一次运行（#271）。
    target: Option<TargetTable>,
    /// 这次死亡是不是我们自己要的，以及什么名义（发信号那一刻打的标记）。
    stop_requested: Option<UnknownReason>,
    /// sink 侧那个 21 字符的 run id。`None` 表示子进程一行日志都没发出来就没了，
    /// sink 从来不知道有过这次运行——没有占用可释放，也就不该发 abort。
    run_id: Option<String>,
    /// 目标端 agent 的地址，取自开跑那一刻的证据快照。agent 后来被改了地址也不影响：
    /// 暂存表在**当时那台**上，abort 就得发到当时那台去。
    agent_base_url: Option<String>,
    /// 最后一次听说的阶段：abort 权限挂在它上面，且只有这一份实现。
    stage: Option<RunStage>,
}

fn wrapup_snapshot(runs: &RunRegistry, run_record_id: &str) -> RunWrapup {
    let Ok(registry) = runs.lock() else {
        return RunWrapup {
            task_id: String::new(),
            target: None,
            stop_requested: None,
            run_id: None,
            agent_base_url: None,
            stage: None,
        };
    };
    let history = registry.live_histories.get(run_record_id);
    RunWrapup {
        task_id: registry
            .active_runs
            .get(run_record_id)
            .map(|run| run.task_id.clone())
            .unwrap_or_default(),
        target: registry
            .active_runs
            .get(run_record_id)
            .map(|run| run.target.clone()),
        stop_requested: registry
            .active_runs
            .get(run_record_id)
            .and_then(|run| run.stop_requested),
        run_id: history.and_then(|history| history.run_id.clone()),
        agent_base_url: history
            .and_then(|history| history.evidence.agent.as_ref())
            .map(|agent| agent.base_url.clone()),
        stage: registry
            .active_runs
            .get(run_record_id)
            .and_then(|run| run.stage),
    }
}

/// 替死掉的子进程向 sink 补发一次 abort：释放目标表占用，并把暂存表 drop 掉。
///
/// 为什么非得父进程来做：停止运行走的是 SIGTERM，而子进程没有信号处理，被内核直接
/// 终止，来不及调用它自己那条 `abort_best_effort`。sink 那边的目标表占用是纯内存的，
/// 只在提交或收到 abort 时删除——没人补这一刀，同一张目标表就再也开不了第二次运行
/// （`TARGET_TABLE_BUSY`）。
///
/// 三种情况不发：
/// * 子进程自己走完了终态（调用方已判）——那时 sink 侧要么已提交、要么子进程自己
///   abort 过了，再发一次只会在 `sealed` 的 run 上换回一个 409，平白造出一条「失败」。
/// * 没有 run_id 或没有 agent 地址：sink 根本不知道这次运行。
/// * 阶段已过封口点：暂存表的处置权整个归 sink 了，source 永久放弃 abort 权。
///   **这一条不漏占用**：`COMMITTING` 是 sink 的 `commit` 正在跑，而它的每一条出口
///   （换表成了、校验没过、目标端报错）都会走 `finish_run` 把占用摘掉——那时没有占用
///   需要 source 去释放。何况已封口的 run 上发 abort 只会换回一个 409 `RUN_SEALED`，
///   记下来只会在界面上立起一颗永远点不成的「锁未释放，点此重试」。
///
/// 失败**不吞**：落成一行 `abort_failed` 运行日志，和子进程自己 abort 失败时写的那行
/// 同一个形状，并把失败的原因**回给调用方**——占用还在这件事要上报到界面上（#271）。
/// 这里仍旧不自动重试：「abort 不承诺可靠性」那条没有变，变的是人有地方点重试了。
///
/// 返回 `None` 有两种意思：这次不必发（三种不发的情况），或者发了并且成了。
/// 两者对占用而言是同一件事——目标表是空闲的。
fn abort_on_behalf_of_child(wrapup: &RunWrapup, run_log: &mut RunLogWriter) -> Option<String> {
    let run_id = wrapup.run_id.as_deref()?;
    // 认不出的阶段拼写（子进程比父进程新）按「可能还没封口」办：多发一次 abort 是幂等的，
    // 少发一次却会把目标表永久锁住。
    if wrapup.stage.is_some_and(|stage| !stage.abort_allowed()) {
        return None;
    }
    let Some(base_url) = wrapup.agent_base_url.as_deref() else {
        let message = "运行证据里没有目标端 agent 地址".to_owned();
        log_abort_failed(run_log, run_id, message.clone());
        return Some(message);
    };
    match release_target_hold(base_url, run_id) {
        Ok(()) => None,
        Err(message) => {
            log_abort_failed(run_log, run_id, message.clone());
            Some(message)
        }
    }
}

/// source **自己**重启时，上一条命没走完的那几次运行留下的目标表占用（#272）。
///
/// 为什么非补不可：子进程的死法有两种，父进程收得了尸的那一种已经由
/// [`abort_on_behalf_of_child`] 补过 abort 了；另一种是父进程跟它一起没了
/// ——升级重部署、机器重启、`kill` 掉 source 都算。那一刀因此谁也没砍，
/// 而 sink 那边的目标表占用是纯内存的，只在提交或收到 abort 时删除：占用于是
/// **永远**挂在目标端，这张表再也开不了第二次运行，界面上却什么都看不出来
/// ——重启后的 source 把这几行历史封了口就忘干净了，下一次「发起运行」照放，
/// 一路跑到 sink 才撞回一个 `TARGET_TABLE_BUSY`。
///
/// 补的方式与人点「锁未释放，点此重试」是同一条路（[`release_target_hold`]），
/// 只是这一趟由启动本身发起。**先把占用如实记上，再去释放**：登记是同步的，
/// 释放是后台的一趟 HTTP——反过来的话，从开门到 abort 回来的那几秒里，
/// 这张表会被显示成可以重跑，而那是假的。占用挂着的这段时间它显示「停止中…」
/// （`retrying` 为真），释放成了就整条摘掉，没释放掉就落回「锁未释放，点此重试」。
///
/// 哪几行不补，与子进程那一路一个判据：没有 run_id（sink 从不知道有过这次运行）、
/// 认不出目标表（老历史没有证据）、阶段已过封口点（处置权归 sink，它自己会摘掉占用）。
pub fn reclaim_after_restart(
    history_store: &HistoryStore,
    run_logs: &RunLogStore,
    runs: &RunRegistry,
    retention_days: u64,
    clock: &dyn Clock,
) -> Result<(), String> {
    // 抄在封口**之前**：封口把这几行的 `outcome` 全填上，之后就再也认不出
    // 哪几次运行可能还在目标端占着表。
    let incomplete = history_store.list_incomplete()?;
    history_store.seal_incomplete(
        UnknownReason::ProcessDisappeared,
        clock.now(),
        retention_days,
    )?;
    let mut pending = Vec::new();
    {
        let Ok(mut registry) = runs.lock() else {
            return Ok(());
        };
        for history in &incomplete {
            let (Some(run_id), Some(target)) =
                (history.run_id.clone(), TargetTable::from_history(history))
            else {
                continue;
            };
            // 认不出的阶段拼写按「可能还没封口」办，与 `abort_on_behalf_of_child` 同理：
            // 多发一次 abort 是幂等的，少发一次却把目标表永久锁住。
            if history
                .stage
                .as_deref()
                .and_then(RunStage::parse)
                .is_some_and(|stage| !stage.abort_allowed())
            {
                continue;
            }
            let agent_base_url = history
                .evidence
                .agent
                .as_ref()
                .map(|agent| agent.base_url.clone());
            registry.stuck_aborts.insert(
                history.run_record_id.clone(),
                StuckAbort {
                    target,
                    task_id: history.task_id.clone(),
                    run_id: run_id.clone(),
                    agent_base_url: agent_base_url.clone(),
                    message: ORPHANED_HOLD_MESSAGE.to_owned(),
                    // 这一趟 abort 已经在路上了：这段窗口里别人再点重试只会让两趟
                    // abort 一起飞。
                    retrying: true,
                    // 补写的那支笔要**接着**上一条命的行号写，否则一行 `abort_failed`
                    // 会把这条运行原有的日志第一行盖掉。
                    run_log: Some(RunLogWriter::resuming(
                        run_logs.clone(),
                        history.run_record_id.clone(),
                        history.task_id.clone(),
                        history.started_at_ms(),
                    )),
                },
            );
            pending.push((history.run_record_id.clone(), run_id, agent_base_url));
        }
    }
    if pending.is_empty() {
        return Ok(());
    }
    // 后台一趟，不挡启动：每次 abort 都是一趟最长 30 秒的 HTTP 往返，
    // 串在启动路径上等于让整个界面陪着等目标端。
    let runs = Arc::clone(runs);
    thread::spawn(move || {
        for (run_record_id, run_id, agent_base_url) in pending {
            let outcome = match agent_base_url.as_deref() {
                Some(base_url) => release_target_hold(base_url, &run_id),
                None => Err(MISSING_AGENT_URL_MESSAGE.to_owned()),
            };
            let _ = settle_stuck_abort(&runs, &run_record_id, &run_id, outcome);
        }
    });
    Ok(())
}

/// 停服收摊：把在飞的运行停掉，**并等它们的收尾走完**（#272）。
///
/// 这是 [`reclaim_after_restart`] 的另一半，管的是同一件事：目标表占用在 sink 那边
/// 是纯内存的，只在提交或收到 abort 时删除。source 一走了之，占用就永远挂在目标端，
/// 那张表再也开不了第二次运行。而收摊这一半必须在**本进程里**做完——重启后那一半
/// 看的是「没走完的历史行」，可优雅退出会在最后把它们全封了口，下一条命于是一行也看不见。
///
/// 做法就是替每一次在飞运行按一次「停止运行」，名义是**服务重启**而不是用户停止：
/// 两者在历史里必须分得开，昨晚那次到底是谁弄的，事后只有这一列答得出来。
/// 随后等在飞登记空掉——真正发 abort 的是各自的监督线程，进程一退它们就没了，
/// 所以这里等的不是礼貌，是那一刀砍没砍下去。等不到就算了，剩下的交给下一条命。
///
/// 已过封口点的那几次**不发信号**：暂存表的处置权已整个归 sink，它自己会摘掉占用；
/// 这时候把子进程打断，只会让一次已经在换表的运行在历史里落个「结局未知」。
/// 它们照样在等待之列——收尾走完了才轮到封口。
///
/// 不自己落日志：这一趟的每一个结果都已经有地方说了——每次运行的历史行上写着
/// 「服务重启，结局未知」，abort 没砍下去的那几次在各自的运行日志里落着 `abort_failed`。
pub fn stop_runs_for_shutdown(runs: &RunRegistry, grace: Duration) {
    let victims: Vec<(String, Option<u32>, Option<RunStage>)> = match runs.lock() {
        Ok(registry) => registry
            .active_runs
            .iter()
            .map(|(run_record_id, run)| (run_record_id.clone(), run.child_pid, run.stage))
            .collect(),
        Err(_) => return,
    };
    if victims.is_empty() {
        return;
    }
    for (run_record_id, child_pid, stage) in &victims {
        if stage.is_some_and(|stage| !stage.abort_allowed()) {
            continue;
        }
        let Some(pid) = child_pid else {
            continue;
        };
        // 标记打在发信号之前，与 `handle_cancel_run` 同一个理由：先 kill 后标记
        // 就是在跟一次进程死亡赛跑。
        mark_stop_requested(runs, run_record_id, UnknownReason::ServiceRestarted);
        if send_sigterm(*pid).is_err() {
            // 信号没发出去，这次运行还在跑：标记撤回去，否则它将来真的死掉时
            // 会顶着一句「服务重启」，而那不是事实。
            unmark_stop_requested(runs, run_record_id);
        }
    }
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        match runs.lock() {
            Ok(registry) if registry.active_runs.is_empty() => return,
            Err(_) => return,
            Ok(_) => {}
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// 一趟 abort 回来之后，那份占用记录该怎么办（#271/#272）。
///
/// 两个发起处（人点的重试、重启后的自动补发）共用这一处：成了就整条摘掉，
/// 没成就把原因原样留在上面、把「正在重试」放下来，并落一行 `abort_failed`。
/// 原样把 `outcome` 回给调用方，接口那边照它挑回话。
fn settle_stuck_abort(
    runs: &RunRegistry,
    run_record_id: &str,
    run_id: &str,
    outcome: Result<(), String>,
) -> Result<(), String> {
    let Err(message) = outcome else {
        if let Ok(mut registry) = runs.lock() {
            registry.stuck_aborts.remove(run_record_id);
        }
        return Ok(());
    };
    // 笔先取出来，日志落在**锁外**：写它是一次 SQLite 往返，攥着 run 控制锁做它，
    // 整个界面都在排队。
    let mut run_log = runs
        .lock()
        .ok()
        .and_then(|mut registry| registry.stuck_aborts.get_mut(run_record_id)?.run_log.take());
    if let Some(run_log) = run_log.as_mut() {
        log_abort_failed(run_log, run_id, message.clone());
    }
    if let Ok(mut registry) = runs.lock() {
        if let Some(stuck) = registry.stuck_aborts.get_mut(run_record_id) {
            stuck.retrying = false;
            stuck.message.clone_from(&message);
            stuck.run_log = run_log;
        }
    }
    Err(message)
}

/// 重启补发时挂在占用上的那句话。补发这一趟还没回来时界面并不显示它
/// （那时是「停止中…」），它是补发**失败**之前的占位。
const ORPHANED_HOLD_MESSAGE: &str =
    "source 重启前这次运行没有走完，目标表占用可能还留在目标端；正在补发一次 abort";

/// 开跑时就没记下 agent 地址（实际上到不了这里）：说清楚这次只能人到目标端去清，
/// 别装作重试过了。
const MISSING_AGENT_URL_MESSAGE: &str = "运行证据里没有目标端 agent 地址，这次占用只能在目标端手工清";

/// 向目标端发一次 abort：目标表占用与暂存表一起了结。
///
/// **补发与重试走的是同一条路**（#271）：一条是子进程死后父进程自动补的那一刀，
/// 一条是人在界面上点出来的那一刀，除了发起的人不同，别的一个字都不该不一样。
fn release_target_hold(base_url: &str, run_id: &str) -> Result<(), String> {
    let mut sink = HttpSinkClient::new(base_url)?;
    sink.abort(run_id).map(|_| ()).map_err(|error| error.message)
}

/// 把一次没能释放掉的占用挂到登记表上（#271）。
///
/// 连日志笔一起交过去：重试再失败也要落一行 `abort_failed`，而行号攥在笔身上，
/// 另起一支会把这条运行已有的日志行覆盖掉。
fn record_stuck_abort(
    runs: &RunRegistry,
    run_record_id: &str,
    wrapup: &RunWrapup,
    message: String,
    run_log: RunLogWriter,
) {
    // 没有 run_id 就走不到这里（`abort_on_behalf_of_child` 头一行就退了），
    // 目标表也是开跑时就登记好的，走到这里必有。两条写全只为不留说不清的分支：
    // 记不下占用的键，这份占用就再也拦不住任何东西，那时宁可不记。
    let (Some(run_id), Some(target)) = (wrapup.run_id.clone(), wrapup.target.clone()) else {
        return;
    };
    if let Ok(mut registry) = runs.lock() {
        registry.stuck_aborts.insert(
            run_record_id.to_owned(),
            StuckAbort {
                target,
                task_id: wrapup.task_id.clone(),
                run_id,
                agent_base_url: wrapup.agent_base_url.clone(),
                message,
                retrying: false,
                run_log: Some(run_log),
            },
        );
    }
}

fn log_abort_failed(run_log: &mut RunLogWriter, run_id: &str, message: String) {
    let mut line = Vec::new();
    if write_log_line_with_fields(
        &mut line,
        LogLevel::Warn,
        LogEvent::AbortFailed,
        Some(run_id),
        None,
        json!({ "message": message }),
    )
    .is_err()
    {
        return;
    }
    let line = String::from_utf8_lossy(&line).trim_end().to_owned();
    {
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        let _ = writeln!(writer, "{line}");
    }
    run_log.write(&line);
}

fn apply_log_line(
    runs: &RunRegistry,
    run_record_id: &str,
    log: &Value,
) -> Option<(HistoryChange, RunHistory)> {
    let Ok(mut registry) = runs.lock() else {
        return None;
    };
    let (change, history) = {
        let record = registry.live_histories.get_mut(run_record_id)?;
        let change = record.apply_log(log);
        (change, record.clone())
    };
    if change == HistoryChange::StageChanged {
        registry.active_runs.get_mut(run_record_id)?.stage =
            history.stage.as_deref().and_then(RunStage::parse);
    }
    Some((change, history))
}

fn register_active_run(
    runs: &RunRegistry,
    run_record_id: &str,
    task_id: &str,
    agent_id: &str,
    target: TargetTable,
) -> Result<(), StartRunError> {
    let mut runs = runs
        .lock()
        .map_err(|_| StartRunError::Internal("run 控制锁已损坏".to_owned()))?;
    if let Some(active) = runs.active_runs.values().find(|run| run.task_id == task_id) {
        return Err(if active.stop_requested.is_some() {
            StartRunError::Stopping
        } else {
            StartRunError::AlreadyRunning
        });
    }
    // 这张目标表的占用还挂在目标端：**这一关就是「占用还在时绝不让它看起来能重跑」
    // 那条铁律的服务端一半**（#271）。前端那颗按钮是另一半，两半各自成立。
    //
    // 拦的判据是**表**不是任务：占用本来就是 sink 按表记的，按任务拦的话，另一个指着
    // 同一张表的任务照样点得动「发起运行」，一路跑到 sink 才撞回 `TARGET_TABLE_BUSY`。
    if runs.stuck_for_target(&target).is_some() {
        return Err(StartRunError::TargetHeld);
    }
    runs.active_runs.insert(
        run_record_id.to_owned(),
        ActiveRun {
            task_id: task_id.to_owned(),
            agent_id: agent_id.to_owned(),
            target,
            child_pid: None,
            stage: None,
            stop_requested: None,
        },
    );
    Ok(())
}

fn remove_active_run(runs: &RunRegistry, run_record_id: &str) {
    if let Ok(mut runs) = runs.lock() {
        runs.active_runs.remove(run_record_id);
    }
}

fn remove_live_history(runs: &RunRegistry, run_record_id: &str) {
    if let Ok(mut registry) = runs.lock() {
        registry.live_histories.remove(run_record_id);
    }
}

pub(crate) fn generate_run_record_id() -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut id = String::with_capacity(32);
    for byte in bytes {
        id.push(HEX[(byte >> 4) as usize] as char);
        id.push(HEX[(byte & 0x0f) as usize] as char);
    }
    id
}

fn handle_list_tasks(store: &TaskStore) -> HttpResponse {
    match store.list() {
        Ok(tasks) => json_response(200, &tasks),
        Err(error) => internal_error(error),
    }
}

fn handle_create_task(request: &Request, state: &Api<'_>) -> HttpResponse {
    let mut input: TaskInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    // 校验先判一次，好让它落在 400 上。`store.create` 里还会再判一次——那一次是存储层
    // 自己的门，不依赖任何调用方记得先问；这一次只为把「你写错了」和「服务端坏了」
    // 分成两个状态码。500 会让人去看服务端日志找一个根本不在那里的故障。
    if let Err(error) = validate_task_input_for_target(state, &mut input) {
        return bad_request(error);
    }
    match state.tasks.create(input) {
        Ok(task) => json_response(201, &task),
        Err(error) => internal_error(error),
    }
}

fn handle_get_task(store: &TaskStore, task_id: &str) -> HttpResponse {
    match store.get(task_id) {
        Ok(Some(task)) => json_response(200, &task),
        Ok(None) => not_found(),
        Err(error) => internal_error(error),
    }
}

fn handle_task_curl(request: &Request, state: &Api<'_>, task_id: &str) -> HttpResponse {
    match state.tasks.get(task_id) {
        Ok(Some(_)) => {}
        Ok(None) => return not_found(),
        Err(error) => return internal_error(error),
    }

    let origin = match request_origin(request, &state.config.listen) {
        Ok(origin) => origin,
        Err(error) => return bad_request(error),
    };
    let body = serde_json::to_string(&json!({ "task_id": task_id }))
        .expect("serializing a task identity must succeed");
    let token = request
        .header("Cookie")
        .and_then(session_token_from_cookie_header)
        .unwrap_or_default();
    let account = match state.auth.session_identity(token) {
        Ok(Some(account)) => account,
        Ok(None) => return unauthorized(),
        Err(error) => return internal_error(error),
    };
    // 手拼而不是走 `json!`：`serde_json` 的 map 是有序的**字典序**，于是
    // `password` 会排到 `username` 前面——功能上无所谓，但这段命令是给人读的。
    // 用户名来自两个固定账号之一，不含引号，没有转义面。
    let credentials = format!(
        r#"{{"username":"{}","password":"改成你的口令"}}"#,
        account.username
    );
    // **两条命令，不是一条。** `/api/runs` 现在要一张会话票据，所以先登录换 cookie、
    // 再拿着 cookie 发起。少了前半条，这段命令会稳定地撞回 401——
    // 那不是「脚本写错了」，是这段命令自己不完整。
    //
    // 口令是个占位符，不是真值：这个接口不发票据，也不回读口令（ADR-0037 §5 的负面条款
    // 一字不改）。cookie 落在 `/tmp` 下一个按任务命名的文件里，**用完自己删**。
    let jar = format!("/tmp/db-qbs-session-{task_id}.cookie");
    let command = format!(
        "curl --silent --show-error --cookie-jar '{jar}' --request POST '{origin}/api/session' --header 'Content-Type: application/json' --data '{credentials}' > /dev/null && curl --cookie '{jar}' --request POST '{origin}/api/runs' --header 'Content-Type: application/json' --data '{body}'; rm -f '{jar}'"
    );
    json_response(200, &json!({ "command": command }))
}

fn request_origin(request: &Request, fallback_listen: &str) -> Result<String, String> {
    let scheme = request.header("X-Forwarded-Proto").unwrap_or("http");
    if !matches!(scheme, "http" | "https") {
        return Err("X-Forwarded-Proto 只允许 http 或 https".to_owned());
    }
    let authority = request.header("Host").unwrap_or(fallback_listen);
    let origin = Url::parse(&format!("{scheme}://{authority}"))
        .map_err(|_| "请求 Host 不是有效的 HTTP 地址".to_owned())?;
    if origin.host_str().is_none()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
        || !origin.username().is_empty()
        || origin.password().is_some()
    {
        return Err("请求 Host 不是有效的 HTTP 地址".to_owned());
    }
    Ok(origin.as_str().trim_end_matches('/').to_owned())
}

fn handle_update_task(request: &Request, state: &Api<'_>, task_id: &str) -> HttpResponse {
    let mut input: TaskInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    if let Err(error) = validate_task_input_for_target(state, &mut input) {
        return bad_request(error);
    }
    match state.tasks.update(task_id, input) {
        Ok(Some(task)) => json_response(200, &task),
        Ok(None) => not_found(),
        Err(error) => internal_error(error),
    }
}

fn validate_task_input_for_target(state: &Api<'_>, input: &mut TaskInput) -> Result<(), String> {
    input.validate()?;
    let Some(pre_sql) = input
        .spec
        .pre_sql
        .as_deref()
        .filter(|sql| !sql.trim().is_empty())
    else {
        input.spec.pre_sql = None;
        return Ok(());
    };
    let target = state
        .datasources
        .target_connection(&input.target_datasource_id)?;
    let pre_sql = db_qbs_shared::validate_pre_sql(
        Some(pre_sql),
        &target.database,
        &input.spec.target_table,
        input.spec.write_mode,
    )
    .map_err(|error| error.to_string())?;
    debug_assert!(pre_sql.is_some());
    Ok(())
}

/// 删任务：还有运行没结束就拒（#270），与删数据源 / 删 agent 同一形态的 409。
///
/// 不做「自动停止再删除」：删除不可逆，让它顺手终止一次可能正在往目标库写数据的运行，
/// 风险大于便利。正确的顺序是用户先停止、等这次运行收尾，再删任务。
///
/// 拦两种情况，因为**这个任务名下有两种东西会被删任务变成孤儿**：
///
/// * 还没结束的运行（`active_run_ids`）——任务一没，停止它的入口就没了。
/// * 还没还回来的目标表占用（`held_run_ids`，#271）——占用是纯内存的，重试释放的那颗
///   按钮长在这个任务的行上；任务一没，占用还挂在目标端，而界面上再没有一处点得到它。
///
/// 这两条与「能不能再发起一次」**不是同一个判据**，而且不该是：发起那一关拦的是
/// 「这张目标表被占着」（可能被别的任务占着），删除这一关拦的是「这个任务欠着东西」。
/// 两者在这个任务自己的占用上重合，所以「停完了、释放干净了就删得掉」仍然成立；
/// 别的任务占着同一张表时删得掉但跑不起来，那不是自相矛盾——两句话说的本就是两件事。
fn handle_delete_task(state: &Api<'_>, task_id: &str) -> HttpResponse {
    let (active, held) = match state.runs.lock() {
        Ok(registry) => (
            registry.active_run_ids(task_id),
            registry.held_run_ids(task_id),
        ),
        Err(_) => return internal_error("run 控制锁已损坏".to_owned()),
    };
    // 点名到 run_record_id：一个任务至多一次在飞，但报文形状跟着删数据源那条走
    // （复数名词的数组 + 一句自己就能读懂的 message），前端拿不到数组时原样显示它。
    if !active.is_empty() {
        return delete_task_refused(
            "RUN_IN_FLIGHT",
            format!(
                "任务还有运行没结束（{}）；请先停止这次运行，等它收尾后再删除任务",
                active.join("、")
            ),
            active,
        );
    }
    if !held.is_empty() {
        return delete_task_refused(
            "TARGET_HELD",
            format!(
                "任务上一次运行的目标表占用还没释放（{}）；请先在这一行点「锁未释放，点此重试」，释放成功后再删除任务",
                held.join("、")
            ),
            held,
        );
    }
    match state.tasks.delete(task_id) {
        Ok(Some(task)) => json_response(200, &task),
        Ok(None) => not_found(),
        Err(error) => internal_error(error),
    }
}

/// 删任务被拒时的那一份 409：两种拦法**同一个形状**，前端只认一种报文。
///
/// `reason` 是给机器读的那一格（`RUN_IN_FLIGHT` / `TARGET_HELD`）：两种拦法要人做的事
/// 不同（去停止 vs 去重试释放），而界面上那句话不该靠**猜服务端的中文**来分辨是哪一种。
/// `message` 仍旧自己就能读懂，给不认这一格的调用方（curl、旧界面）留着。
fn delete_task_refused(reason: &str, message: String, runs: Vec<String>) -> HttpResponse {
    json_response(
        409,
        &json!({
            "error": {
                "message": message,
                "reason": reason,
                "runs": runs,
            }
        }),
    )
}

fn handle_list_datasources(store: &DatasourceStore) -> HttpResponse {
    match store.list() {
        // 只出 view：口令连密文都不回（ADR-0037 §5）。
        Ok(datasources) => json_response(
            200,
            &datasources
                .iter()
                .map(|datasource| datasource.view())
                .collect::<Vec<_>>(),
        ),
        Err(error) => internal_error(error),
    }
}

fn handle_create_datasource(request: &Request, state: &Api<'_>) -> HttpResponse {
    let input: DatasourceInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    if let Err(error) = check_bound_agent_exists(state, &input) {
        return bad_request(error);
    }
    match state.datasources.create(input) {
        Ok(datasource) => json_response(201, &datasource.view()),
        Err(error) => bad_request(error),
    }
}

/// MySQL 数据源绑的那台 agent 必须**真在注册表里**（ADR-0044 §3）。
///
/// 只检存在、**不在这里检在线**：一台临时掉线的 agent 不该让人连数据源的库名都改不了。
/// 「在线」是用它的那一刻的判据（[`resolve_target_agent`]），不是编辑表单的判据。
fn check_bound_agent_exists(
    state: &Api<'_>,
    input: &DatasourceInput,
) -> Result<(), String> {
    let crate::DatasourceSettings::Mysql { agent_id, .. } = &input.settings else {
        return Ok(());
    };
    if agent_id.trim().is_empty() {
        // 空绑定的措辞归 `DatasourceSettings::validate`，那里那句更贴近表单。
        return Ok(());
    }
    match with_agents(state.agents, |store| store.get(agent_id))? {
        Some(_) => Ok(()),
        None => Err(format!(
            "目标端 agent（{agent_id}）不在注册表里，请先在「目标端 Agent」屏注册它"
        )),
    }
}

fn handle_get_datasource(store: &DatasourceStore, datasource_id: &str) -> HttpResponse {
    match store.get(datasource_id) {
        Ok(Some(datasource)) => json_response(200, &datasource.view()),
        Ok(None) => not_found(),
        Err(error) => internal_error(error),
    }
}

fn handle_update_datasource(
    request: &Request,
    state: &Api<'_>,
    datasource_id: &str,
) -> HttpResponse {
    let input: DatasourceInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    if let Err(error) = check_bound_agent_exists(state, &input) {
        return bad_request(error);
    }
    match state.datasources.update(datasource_id, input) {
        Ok(Some(datasource)) => json_response(200, &datasource.view()),
        Ok(None) => not_found(),
        Err(error) => bad_request(error),
    }
}

/// 删数据源：还有任务引着就拒（ADR-0037 §7），措辞点名是哪几个任务——
/// 只说「有任务在用」会让用户挨个点开去找。
fn handle_delete_datasource(state: &Api<'_>, datasource_id: &str) -> HttpResponse {
    match state.tasks.names_referencing(datasource_id) {
        Ok(names) if !names.is_empty() => {
            return json_response(
                409,
                &json!({
                    "error": {
                        "message": format!("数据源仍被 {} 个任务引用：{}；请先改这些任务的数据源", names.len(), names.join("、")),
                        "tasks": names,
                    }
                }),
            )
        }
        Ok(_) => {}
        Err(error) => return internal_error(error),
    }
    match state.datasources.delete(datasource_id) {
        Ok(Some(datasource)) => json_response(200, &datasource.view()),
        Ok(None) => not_found(),
        Err(error) => internal_error(error),
    }
}

/// 目标端 agent 的注册表（ADR-0044 §3）。列表是**读库**，不逐台现探——
/// 后台探测线程 15 秒一轮已经在维护状态了，进屏时再串行探一遍只会把首屏拖到超时长度。
fn handle_list_agents(state: &Api<'_>) -> HttpResponse {
    match with_agents(state.agents, AgentStore::list) {
        Ok(agents) => json_response(200, &agents),
        Err(error) => internal_error(error),
    }
}

/// 注册一台 agent。**当场探一次，探不通就不落库**（ADR-0044 §3）：
/// 库里放一条从没连通过的记录，只会让人在数据源那一屏选到一台并不存在的 agent。
fn handle_register_agent(request: &Request, state: &Api<'_>) -> HttpResponse {
    let input: AgentInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    let base_url = match crate::normalize_base_url(&input.base_url) {
        Ok(base_url) => base_url,
        Err(error) => return bad_request(error),
    };
    let info = match fetch_agent_info(&base_url) {
        Ok(info) => info,
        Err(error) => return agent_unreachable(&input.base_url, &error),
    };
    let now = now_rfc3339();
    match with_agents(state.agents, |store| store.register(&input, &info, &now)) {
        Ok(agent) => json_response(201, &agent),
        Err(error) => internal_error(error),
    }
}

/// 改名 / 改址。改址会**重新钉身份**——换机器是人明确发起的动作（ADR-0044 §3）。
fn handle_update_agent(
    request: &Request,
    state: &Api<'_>,
    agent_id: &str,
) -> HttpResponse {
    let input: AgentInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    let base_url = match crate::normalize_base_url(&input.base_url) {
        Ok(base_url) => base_url,
        Err(error) => return bad_request(error),
    };
    let info = match fetch_agent_info(&base_url) {
        Ok(info) => info,
        Err(error) => return agent_unreachable(&input.base_url, &error),
    };
    let now = now_rfc3339();
    match with_agents(state.agents, |store| {
        store.update(agent_id, &input, &info, &now)
    }) {
        Ok(Some(agent)) => json_response(200, &agent),
        Ok(None) => not_found(),
        Err(error) => internal_error(error),
    }
}

/// 手动探一台（列表上那个「探测」按钮）。**失败也回 200**：探测的结果本身就是信息，
/// 界面要拿它把那一行标红并显示原因，报 5xx 会让前端把它当成请求失败。
fn handle_probe_agent(state: &Api<'_>, agent_id: &str) -> HttpResponse {
    match probe_and_record(state.agents, agent_id) {
        Ok(Some(agent)) => json_response(200, &agent),
        Ok(None) => not_found(),
        Err(error) => internal_error(error),
    }
}

/// 删一台 agent。**被数据源引用就拒**，与数据源被任务引用那条 409 同一形态（ADR-0039 §4）：
/// 删掉之后那些数据源会立刻变成「绑着一台不存在的 agent」，那是个没人看得懂的状态。
fn handle_delete_agent(state: &Api<'_>, agent_id: &str) -> HttpResponse {
    let referencing = match state.datasources.list() {
        Ok(datasources) => datasources
            .into_iter()
            .filter(|datasource| {
                matches!(
                    &datasource.settings,
                    crate::DatasourceSettings::Mysql { agent_id: bound, .. }
                        if bound == agent_id
                )
            })
            .map(|datasource| datasource.name)
            .collect::<Vec<_>>(),
        Err(error) => return internal_error(error),
    };
    if !referencing.is_empty() {
        return json_response(
            409,
            &json!({
                "error": {
                    "message": format!(
                        "这台 agent 仍被 {} 条数据源引用；请先改这些数据源绑定的 agent",
                        referencing.len()
                    ),
                    "datasources": referencing,
                }
            }),
        );
    }
    match with_agents(state.agents, |store| store.delete(agent_id)) {
        Ok(Some(agent)) => json_response(200, &agent),
        Ok(None) => not_found(),
        Err(error) => internal_error(error),
    }
}

fn with_agents<T>(
    agents: &AgentRegistry,
    action: impl FnOnce(&AgentStore) -> Result<T, String>,
) -> Result<T, String> {
    let store = agents
        .lock()
        .map_err(|_| "agent 注册表锁已损坏".to_owned())?;
    action(&store)
}

/// 探一台并把结果记进库。**探测在锁外做**：一台掉线的 agent 要等满连接超时，
/// 攥着锁等于让同时进来的每个请求陪它一起卡住。
fn probe_and_record(agents: &AgentRegistry, agent_id: &str) -> Result<Option<Agent>, String> {
    let Some(agent) = with_agents(agents, |store| store.get(agent_id))? else {
        return Ok(None);
    };
    let probed = fetch_agent_info(&agent.base_url);
    let now = now_rfc3339();
    with_agents(agents, |store| store.record_probe(agent_id, &probed, &now))
}

/// 「这条 MySQL 数据源该走哪台 agent，而且它现在能用吗」——**每一条目标端链路的入口**
/// （ADR-0044 §4）：测连、取表、取列、发起运行，四处走的都是它。
///
/// 它当场探一次并核身份，因此「agent 停了」在**这一刻**就有后果，
/// 不必等后台那一轮（最长 15 秒）。返回错误时给的是可以直接摆到界面上的人话。
fn resolve_target_agent(state: &Api<'_>, datasource_id: &str) -> Result<Agent, String> {
    resolve_agent(state, &state.datasources.target_agent_id(datasource_id)?)
}

/// 同上，但直接按 agent id 解——草稿测连走这一条：那组值还没存进库，
/// 「这条数据源绑的是谁」只能从表单里当前选的那台取（ADR-0039 §3 同一条解释规则）。
fn resolve_agent(state: &Api<'_>, agent_id: &str) -> Result<Agent, String> {
    let agent = probe_and_record(state.agents, agent_id)?.ok_or_else(|| {
        format!("目标端 agent（{agent_id}）不在注册表里，请先在「目标端 Agent」屏注册它")
    })?;
    match agent.status {
        crate::AgentStatus::Online => Ok(agent),
        crate::AgentStatus::Mismatch => Err(format!(
            "目标端 agent「{}」身份不符：{}。目标库只能经它访问，在核对清楚之前这条链路不放行",
            agent.name,
            agent.last_error.unwrap_or_default()
        )),
        crate::AgentStatus::Offline => Err(format!(
            "目标端 agent「{}」不在线（{}）：{}。目标库只能经它访问，请先把它起起来",
            agent.name,
            agent.base_url,
            agent.last_error.unwrap_or_default()
        )),
    }
}

fn agent_unreachable(base_url: &str, error: &str) -> HttpResponse {
    agent_failure(format!(
        "连不上这个地址上的目标端 agent（{base_url}）：{error}"
    ))
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// 草稿「测试连接」（ADR-0039 §3）：用**表单里当前填的值**测，不是库里存的那份。
///
/// 存在的理由是「测通才让存」这条门槛（所有者 2026-08-19 裁定 2）——新建的数据源
/// 库里根本还没有，按 id 测在新建态上无从谈起；改了口令的编辑态按 id 测的也是旧口令。
///
/// **不写任何存储**：解出来的连接用完即弃，与两个目标端元数据入口同一处置（ADR-0038 §3）。
/// 回报里带 `elapsed_ms` 与 `label`，`.inline-result` 那一行纯文字要用（ADR-0039 §3）。
fn handle_test_datasource_draft(request: &Request, state: &Api<'_>) -> HttpResponse {
    let input: DatasourceTestInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    let datasource_id = input.datasource_id.as_deref().filter(|id| !id.is_empty());
    let started = std::time::Instant::now();
    match &input.draft.settings {
        crate::DatasourceSettings::Oracle { connect_string, .. } => {
            let label = connect_string.clone();
            let access = match state.datasources.draft_oracle_access(
                datasource_id,
                &input.draft.settings,
                &state.config.oracle_client_lib_dir,
            ) {
                Ok(access) => access,
                Err(error) => return bad_request(error),
            };
            match OracleRowSource::test_connection(&access) {
                Ok(()) => test_connection_ok(started, label),
                Err(error) => oracle_failure(error),
            }
        }
        crate::DatasourceSettings::Mysql {
            agent_id, database, ..
        } => {
            let label = database.clone();
            // 先判表单本身（字段齐不齐、有没有选 agent），再判 agent 在不在线：
            // 顺序反过来的话，一份「什么都没填」的草稿会先撞上「agent 不在注册表里」，
            // 那句话对着一个空表单说毫无意义。
            let target = match state
                .datasources
                .draft_target_connection(datasource_id, &input.draft.settings)
            {
                Ok(target) => target,
                Err(error) => return bad_request(error),
            };
            // 目标端一律经 agent（ADR-0044 §1）：解出这条草稿选的那台并当场核一次，
            // 它不在线就到此为止——底下那句「连不上 sink」说不清是库不通还是 agent 不通。
            let agent = match resolve_agent(state, agent_id) {
                Ok(agent) => agent,
                Err(error) => return agent_failure(error),
            };
            match test_target_connection(&agent.base_url, &target) {
                Ok(()) => test_connection_ok(started, label),
                Err(error) => sink_failure(error),
            }
        }
    }
}

/// 成功那一格：耗时与库名/连接串一起回去，界面拼成「连接成功 · 186 ms · dw_stage」。
fn test_connection_ok(started: std::time::Instant, label: String) -> HttpResponse {
    json_response(
        200,
        &json!({
            "ok": true,
            "elapsed_ms": started.elapsed().as_millis() as u64,
            "label": label,
        }),
    )
}

/// 「测试连接」（ADR-0037 §9）：Oracle 在本机直连，MySQL 走 sink 的新端点——
/// source 仍不建 MySQL 连接，`CONTEXT.md` 那条不对称在这一点上保留。
fn handle_test_datasource(state: &Api<'_>, datasource_id: &str) -> HttpResponse {
    let datasource = match state.datasources.get(datasource_id) {
        Ok(Some(datasource)) => datasource,
        Ok(None) => return not_found(),
        Err(error) => return internal_error(error),
    };
    match datasource.settings {
        crate::DatasourceSettings::Oracle { .. } => {
            let access = match oracle_access(state, datasource_id) {
                Ok(access) => access,
                Err(error) => return bad_request(error),
            };
            match OracleRowSource::test_connection(&access) {
                Ok(()) => json_response(200, &json!({ "ok": true })),
                Err(error) => oracle_failure(error),
            }
        }
        crate::DatasourceSettings::Mysql { .. } => {
            let target = match state.datasources.target_connection(datasource_id) {
                Ok(target) => target,
                Err(error) => return bad_request(error),
            };
            let agent = match resolve_target_agent(state, datasource_id) {
                Ok(agent) => agent,
                Err(error) => return agent_failure(error),
            };
            match test_target_connection(&agent.base_url, &target) {
                Ok(()) => json_response(200, &json!({ "ok": true })),
                Err(error) => sink_failure(error),
            }
        }
    }
}

/// 目标库的表清单（ADR-0038 §3）。source 仍**不建 MySQL 连接**——查询在 sink 那侧跑，
/// `CONTEXT.md` 那条不对称原样保留。结果纯瞬态：不进任务定义、不进 SQLite（ADR-0038 §8）。
fn handle_target_tables(request: &Request, state: &Api<'_>) -> HttpResponse {
    let input: TargetMetadataInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    // 顺序：先解连接（数据源不存在 / 类型不对是 400，属请求本身的错），再解 agent
    // （不在线是 502，属环境的错）。反过来的话，「拿一条 Oracle 数据源去要目标端表清单」
    // 会先撞上 agent 那一关，报出来的是一句与真实成因无关的话。
    let target = match state.datasources.target_connection(&input.datasource_id) {
        Ok(target) => target,
        Err(error) => return bad_request(error),
    };
    let agent = match resolve_target_agent(state, &input.datasource_id) {
        Ok(agent) => agent,
        Err(error) => return agent_failure(error),
    };
    match post_to_sink(
        &agent.base_url,
        "/v1/target/tables",
        &serde_json::to_value(&target).expect("target connection must serialize"),
    ) {
        Ok(body) => json_response(200, &body),
        Err(error) => sink_failure(error),
    }
}

/// 一张目标表的列清单与唯一性约束（ADR-0038 §3）。**表不存在回空清单，不是错误**（§9）。
fn handle_target_columns(request: &Request, state: &Api<'_>) -> HttpResponse {
    let input: TargetMetadataInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    if input.target_table.trim().is_empty() {
        return bad_request("target_table 不能为空".to_owned());
    }
    // 顺序同取表清单：数据源面的错 400，agent 面的错 502。
    let target = match state.datasources.target_connection(&input.datasource_id) {
        Ok(target) => target,
        Err(error) => return bad_request(error),
    };
    let agent = match resolve_target_agent(state, &input.datasource_id) {
        Ok(agent) => agent,
        Err(error) => return agent_failure(error),
    };
    match post_to_sink(
        &agent.base_url,
        "/v1/target/columns",
        &json!({ "target": target, "target_table": input.target_table }),
    ) {
        Ok(body) => json_response(200, &body),
        Err(error) => sink_failure(error),
    }
}

fn handle_target_check(request: &Request, state: &Api<'_>) -> HttpResponse {
    let input: TargetCheckInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    if let Err(error) = input.spec.validate() {
        return bad_request(error);
    }
    if input.target_table.trim().is_empty() {
        return bad_request("target_table 不能为空".to_owned());
    }
    if !input
        .target_table
        .eq_ignore_ascii_case(&input.spec.target_table)
    {
        return bad_request("target_table 必须与 spec.target_table 一致".to_owned());
    }

    let access = match oracle_access(state, &input.source_datasource_id) {
        Ok(access) => access,
        Err(error) => return bad_request(error),
    };
    let source_columns = match (state.describe_source)(&access, &input.spec) {
        Ok(columns) => columns,
        Err(error) => return oracle_failure(error),
    };
    let target = match state
        .datasources
        .target_connection(&input.target_datasource_id)
    {
        Ok(target) => target,
        Err(error) => return bad_request(error),
    };
    let agent = match resolve_target_agent(state, &input.target_datasource_id) {
        Ok(agent) => agent,
        Err(error) => return agent_failure(error),
    };
    let sink_body = match post_to_sink(
        &agent.base_url,
        "/v1/target/check",
        &serde_json::to_value(TargetCheckRequest {
            target,
            target_table: input.target_table.clone(),
            source_columns: source_columns.clone(),
            primary_key: input.spec.primary_key.clone(),
        })
        .expect("target check request must serialize"),
    ) {
        Ok(body) => body,
        Err(error) => return sink_failure(error),
    };
    let mut result: TargetCheckResult = match serde_json::from_value(sink_body) {
        Ok(result) => result,
        Err(error) => return sink_failure(format!("目标端检查回话形状无效：{error}")),
    };
    result.suggested_ddl = if result.ok {
        None
    } else {
        // 字符序取这台 agent 上报的那一份（#257）：source 不连 MySQL，这是唯一的信息源。
        // 旧版本 agent 报不出来就是 `None`，生成的语句照旧不写 `COLLATE`。
        generate_target_ddl(
            &source_columns,
            &input.target_table,
            &input.spec.primary_key,
            None,
            agent.mysql_collation.as_deref(),
        )
        .ok()
    };
    json_response(200, &result)
}

/// 往 sink 发一个「不属于任何 run」的元数据请求，把 JSON 回话原样带回来。
///
/// 与 [`test_target_connection`] 同一条通道、同一条部署前提（ADR-0037 §4：通道必须可信），
/// 只是多一个「把响应体读回来」——`test-connection` 只关心成没成。
fn post_to_sink(agent_base_url: &str, path: &str, body: &Value) -> Result<Value, String> {
    let url = format!("{}{path}", agent_base_url.trim_end_matches('/'));
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .redirects(0)
        .build();
    match agent.post(&url).send_json(body.clone()) {
        Ok(response) => response
            .into_json::<Value>()
            .map_err(|error| format!("目标端回话不是 JSON：{error}")),
        Err(ureq::Error::Status(_, response)) => Err(response
            .into_json::<Value>()
            .ok()
            .and_then(|body| {
                body.get("error")?
                    .get("message")?
                    .as_str()
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "目标端拒绝了元数据请求".to_owned())),
        Err(ureq::Error::Transport(error)) => Err(format!("连不上 sink：{error}")),
    }
}

/// 把连接信息交给 sink 试一把。**口令在这里过线**——这是 ADR-0037 §1 认下的那条路径，
/// 与发起运行走同一个通道、同一条部署前提（§4：通道必须可信）。
fn test_target_connection(agent_base_url: &str, target: &TargetConnection) -> Result<(), String> {
    let url = format!(
        "{}/v1/target/test-connection",
        agent_base_url.trim_end_matches('/')
    );
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .redirects(0)
        .build();
    match agent
        .post(&url)
        .send_json(serde_json::to_value(target).expect("target connection must serialize"))
    {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(_, response)) => Err(response
            .into_json::<Value>()
            .ok()
            .and_then(|body| {
                body.get("error")?
                    .get("message")?
                    .as_str()
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "目标端拒绝了测试连接请求".to_owned())),
        Err(ureq::Error::Transport(error)) => Err(format!("连不上 sink：{error}")),
    }
}

fn read_json_body<T: DeserializeOwned>(request: &Request) -> Result<T, String> {
    let body = request
        .body()
        .map_err(|error| format!("读取请求体失败：{error}"))?;
    if body.len() as u64 > MAX_REQUEST_BODY_BYTES {
        return Err("请求体超过 1 MiB".to_owned());
    }
    serde_json::from_slice(body).map_err(|error| format!("JSON 请求体无效：{error}"))
}

type HttpResponse = Response;

fn not_found() -> HttpResponse {
    json_response(
        404,
        &json!({ "error": { "message": "请求的 source API 资源不存在" } }),
    )
}

/// 没有活着的会话时的唯一回答。**过期与从未登录不分开报**：对调用方它们是同一件事
/// ——回登录页去。
fn unauthorized() -> HttpResponse {
    json_response(401, &json!({ "error": { "message": "请先登录" } }))
}

fn forbidden() -> HttpResponse {
    json_response(
        403,
        &json!({ "error": { "code": "FORBIDDEN", "message": "需要管理员权限" } }),
    )
}

/// **失败的正文只有一种壳**（#199）：`{"error": {"message": ...}}`，`kind` 是壳里的
/// 一个可选字段，不是与它并排的第二种形状。
///
/// `kind` 答的是「下一步该找谁」——改自己的输入（`request`）、找 DBA（`oracle`）、
/// 还是去看目标端那台机器（`agent` / `sink`）。需要归属的屏去读它，不需要的当它不存在。
fn error_response(status: u16, kind: &str, message: String) -> HttpResponse {
    error_response_with(status, kind, message, json!({}))
}

/// 同一只信封，外加这次失败自己带的那几个字段（Oracle 的错误码、判废的列）。
/// **附加字段也在信封里边**，没有第二层——拼信封的地方只有这一处。
fn error_response_with(
    status: u16,
    kind: &str,
    message: String,
    extra: serde_json::Value,
) -> HttpResponse {
    let mut detail = serde_json::Map::new();
    detail.insert("message".to_owned(), json!(message));
    detail.insert("kind".to_owned(), json!(kind));
    if let serde_json::Value::Object(fields) = extra {
        detail.extend(fields);
    }
    json_response(status, &json!({ "error": detail }))
}

/// 请求本身的错：改输入的人是调用方自己。
fn bad_request(message: String) -> HttpResponse {
    error_response(400, "request", message)
}

/// 目标端 agent 那一段断了——这条链路上没有回退，只能去看那台机器。
fn agent_failure(message: String) -> HttpResponse {
    error_response(502, "agent", message)
}

/// agent 活着，但它背后的 sink/目标库不接受这次请求。
fn sink_failure(message: String) -> HttpResponse {
    error_response(502, "sink", message)
}

fn internal_error(message: String) -> HttpResponse {
    json_response(500, &json!({ "error": { "message": message } }))
}

fn json_response(status: u16, value: &impl Serialize) -> HttpResponse {
    let body = serde_json::to_vec(value).expect("serializing an HTTP response must succeed");
    Response {
        status,
        headers: vec![(
            "Content-Type".to_owned(),
            "application/json; charset=utf-8".to_owned(),
        )],
        body,
    }
}

fn embedded_response(body: Vec<u8>, content_type: &str, path: &str) -> HttpResponse {
    let request_path = path.split('?').next().unwrap_or(path);
    let cache_control_value = if matches!(request_path, "/" | "/index.html") {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    Response {
        status: 200,
        headers: vec![
            ("Content-Type".to_owned(), content_type.to_owned()),
            ("Cache-Control".to_owned(), cache_control_value.to_owned()),
        ],
        body,
    }
}

/// 解出某个 Oracle 数据源的连接信息：数据源那三样 + 进程级的 client 库目录（ADR-0037 §6）。
fn oracle_access(state: &Api<'_>, datasource_id: &str) -> Result<OracleAccess, String> {
    state
        .datasources
        .oracle_access(datasource_id, &state.config.oracle_client_lib_dir)
}

fn handle_builder_tables(request: &Request, state: &Api<'_>) -> HttpResponse {
    let input: BuilderLinkInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    if let Err(error) = validate_builder_dblink(input.dblink.as_deref()) {
        return bad_request(error);
    }
    let access = match oracle_access(state, &input.datasource_id) {
        Ok(access) => access,
        Err(error) => return bad_request(error),
    };
    match OracleRowSource::list_builder_tables(&access, input.dblink.as_deref()) {
        Ok(tables) => json_response(200, &tables),
        Err(error) => oracle_failure(error),
    }
}

fn handle_builder_dblinks(request: &Request, state: &Api<'_>) -> HttpResponse {
    let input: BuilderDatasourceInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    let access = match oracle_access(state, &input.datasource_id) {
        Ok(access) => access,
        Err(error) => return bad_request(error),
    };
    match OracleRowSource::list_builder_dblinks(&access) {
        Ok(dblinks) => json_response(200, &dblinks),
        Err(error) => oracle_failure(error),
    }
}

fn handle_builder_sql_columns(request: &Request, state: &Api<'_>) -> HttpResponse {
    let input: BuilderSqlInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    if let Err(error) = validate_source_sql(&input.source_sql) {
        return bad_request(error);
    }
    let access = match oracle_access(state, &input.datasource_id) {
        Ok(access) => access,
        Err(error) => return bad_request(error),
    };
    match OracleRowSource::describe_source_sql(&access, &input.source_sql) {
        Ok(columns) => json_response(200, &columns),
        Err(error) => oracle_failure(error),
    }
}

fn handle_builder_columns(request: &Request, state: &Api<'_>) -> HttpResponse {
    let input: BuilderColumnsInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    if input.owner.trim().is_empty() || input.table.trim().is_empty() {
        return bad_request("owner and table are required".to_owned());
    }
    if let Err(error) = validate_builder_dblink(input.dblink.as_deref()) {
        return bad_request(error);
    }
    let access = match oracle_access(state, &input.datasource_id) {
        Ok(access) => access,
        Err(error) => return bad_request(error),
    };
    match OracleRowSource::list_builder_columns(
        &access,
        input.dblink.as_deref(),
        &input.owner,
        &input.table,
    ) {
        Ok(columns) => json_response(200, &columns),
        Err(error) => oracle_failure(error),
    }
}

/// 规格的派生面：源端 SQL。
///
/// **只读**——界面上没有编辑入口，所以这里只出不进：web 拿规格来换一份现算的 SQL 展示，
/// 不存在「web 改了 SQL 再传回来」这条路。
fn handle_builder_sql(request: &Request) -> HttpResponse {
    let spec: TaskSpec = match read_json_body(request) {
        Ok(spec) => spec,
        Err(error) => return bad_request(error),
    };
    if let Err(error) = spec.validate() {
        return bad_request(error);
    }
    json_response(200, &json!({ "source_sql": spec.source_sql() }))
}

/// 把一条 cron 表达式翻译成人话：**服务器本地时区**是哪个，接下来几次什么时候触发。
///
/// 时区在这里被钉死，而且**必须显示出来**：任务定义里存的是一行没有时区的文本，
/// 「凌晨两点」到底是哪个两点全靠这一层回答。答案是运行 `source` 的那台机器的本地时区，
/// 不是浏览器的、也不是 UTC——那台机器才是将来真正到点发起运行的地方。
/// 界面上不写出来的话，跨时区办公的人会拿自己的表去对一个别人的两点。
///
/// 表达式不合法就是 400，理由原样来自 [`CronSchedule::parse`]：这条路径与保存时的那道
/// 校验读的是同一份解析器，所以界面上先看到的那句话，和保存被拒时的那句话一字不差。
fn handle_schedule_preview(request: &Request) -> HttpResponse {
    let input: SchedulePreviewInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    let now = Local::now();
    let expression = input
        .cron
        .as_deref()
        .map(str::trim)
        .filter(|expression| !expression.is_empty());
    // 没给表达式不是错，只是没有可算的东西：时区照答，触发时刻是空的一串。
    let upcoming = match expression {
        Some(expression) => match CronSchedule::parse(expression) {
            Ok(schedule) => schedule.upcoming(now.naive_local(), SCHEDULE_PREVIEW_COUNT),
            Err(error) => return bad_request(error),
        },
        None => Vec::new(),
    };
    json_response(
        200,
        &json!({
            "timezone": now.format("%Z").to_string(),
            "utc_offset": now.format("%:z").to_string(),
            "now": now.format(SCHEDULE_TIME_FORMAT).to_string(),
            "next_fire_times": upcoming
                .iter()
                .map(|fire| fire.format(SCHEDULE_TIME_FORMAT).to_string())
                .collect::<Vec<_>>(),
        }),
    )
}

/// 调度器此刻在干什么（#266）：每个任务的下一次触发时刻，以及**排队中的那些**。
///
/// 排队这一段是本票的验收项之一——额度满时任务在 source 侧等着，如果只活在后台线程的
/// 内存里，界面上就只剩「什么都没发生」。`waiting_reason` 是给人看的一句话，
/// 直接来自上一次派发尝试。
///
/// 时区与「下次触发」预览同一份口径：`source` 那台机器的本地时区，而且写出来。
fn handle_schedule_state(state: &Api<'_>) -> HttpResponse {
    let now = Local::now();
    let (queued, next_fires) = match state.schedule.lock() {
        Ok(schedule) => (schedule.queue_view(), schedule.next_fires()),
        Err(_) => return internal_error("调度器状态锁已损坏".to_owned()),
    };
    json_response(
        200,
        &json!({
            "timezone": now.format("%Z").to_string(),
            "utc_offset": now.format("%:z").to_string(),
            "now": now.format(SCHEDULE_TIME_FORMAT).to_string(),
            "queued": queued,
            "next_fire_times": next_fires
                .into_iter()
                .map(|(task_id, next_fire_time)| json!({
                    "task_id": task_id,
                    "next_fire_time": next_fire_time,
                }))
                .collect::<Vec<_>>(),
        }),
    )
}

fn handle_builder_preview(request: &Request, state: &Api<'_>) -> HttpResponse {
    let input: BuilderPreviewInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    if let Err(error) = input.spec.validate() {
        return bad_request(error);
    }
    let limit = match preview_limit(input.limit) {
        Ok(limit) => limit,
        Err(error) => return bad_request(error),
    };
    let access = match oracle_access(state, &input.source_datasource_id) {
        Ok(access) => access,
        Err(error) => return bad_request(error),
    };
    preview_response(collect_preview(&input.spec, limit, |source_sql| {
        OracleRowSource::preview(&access, source_sql, PREVIEW_CALL_TIMEOUT)
    }))
}

fn preview_response(result: Result<PreviewResult, SourceReadError>) -> HttpResponse {
    match result {
        Ok(preview) => json_response(200, &preview),
        Err(error) if error.timed_out => {
            error_response(504, "oracle", "源端数据预览超时".to_owned())
        }
        Err(error) => oracle_failure(error),
    }
}

fn preview_limit(requested: Option<usize>) -> Result<usize, String> {
    match requested {
        Some(0) => Err("limit 必须大于 0".to_owned()),
        Some(limit) => Ok(limit.min(MAX_PREVIEW_LIMIT)),
        None => Ok(DEFAULT_PREVIEW_LIMIT),
    }
}

/// Generate once through `TaskSpec`, then feed that exact SQL to the reader.
/// The extra read is only a truncation probe and is never returned.
fn collect_preview<S, F>(
    spec: &TaskSpec,
    limit: usize,
    open: F,
) -> Result<PreviewResult, SourceReadError>
where
    S: RowSource,
    F: FnOnce(&str) -> Result<S, SourceReadError>,
{
    let source_sql = spec.source_sql();
    let started = Instant::now();
    let mut source = open(&source_sql)?;
    let columns = source
        .columns()
        .iter()
        .map(|column| column.name.clone())
        .collect();
    let mut rows = Vec::with_capacity(limit);
    let mut truncated = false;
    for index in 0..=limit {
        let Some(row) = source.next_row()? else { break };
        if index == limit {
            truncated = true;
            break;
        }
        rows.push(row);
    }
    Ok(PreviewResult {
        columns,
        rows,
        truncated,
        elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
    })
}

fn oracle_failure(error: crate::SourceReadError) -> HttpResponse {
    error_response_with(
        502,
        "oracle",
        error.user_message(),
        json!({
            "oracle_code": error.oracle_code,
            // 取列失败不是一次 run，进不了运行历史；分类仍照实给出，
            // 否则「连不上 Oracle」与「dblink 不可用」在这个面上又只能靠人话反推。
            "failure_kind": error.kind.as_str(),
        }),
    )
}

fn handle_column_fetch(request: &Request, state: &Api<'_>) -> HttpResponse {
    let input: ColumnFetchInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    if let Err(error) = input.spec.validate() {
        return bad_request(error);
    }

    let access = match oracle_access(state, &input.datasource_id) {
        Ok(access) => access,
        Err(error) => return bad_request(error),
    };
    let columns = match OracleRowSource::describe(&access, &input.spec) {
        Ok(columns) => columns,
        Err(error) => return oracle_failure(error),
    };
    // 目标端可给可不给（见 `ColumnFetchInput`）。给了但那台 agent 不在线，也不该让取列
    // 失败——取列问的是源端，目标端只影响建表语句里那一段字符序。
    let target_collation = input
        .target_datasource_id
        .as_deref()
        .and_then(|datasource_id| resolve_target_agent(state, datasource_id).ok())
        .and_then(|agent| agent.mysql_collation);
    match generate_target_ddl(
        &columns,
        &input.spec.target_table,
        &input.spec.primary_key,
        input.column_precision.as_ref(),
        target_collation.as_deref(),
    ) {
        Ok(target_ddl) => json_response(
            200,
            &json!({ "columns": columns, "target_ddl": target_ddl }),
        ),
        Err(error) => {
            let message = error.to_string();
            let issues = error.columns;
            error_response_with(
                422,
                "target_ddl",
                message,
                json!({ "columns": issues, "described_columns": columns }),
            )
        }
    }
}

pub(crate) fn emit(level: LogLevel, event: LogEvent, fields: serde_json::Value) {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    let _ = write_log_line_with_fields(&mut writer, level, event, None, None, fields);
}

/// tiny_http 与这个模块之间的翻译层。二进制那边除了监听循环，剩下的就只有这两个函数。
mod bridge {
    use std::io::Read;

    use super::{Method, Request, Response, MAX_REQUEST_BODY_BYTES};

    impl From<&tiny_http::Method> for Method {
        fn from(method: &tiny_http::Method) -> Self {
            match method {
                tiny_http::Method::Get => Method::Get,
                tiny_http::Method::Post => Method::Post,
                tiny_http::Method::Put => Method::Put,
                tiny_http::Method::Delete => Method::Delete,
                _ => Method::Other,
            }
        }
    }

    impl Request {
        /// 从 tiny_http 的请求里把方法、URL 和请求体取出来。
        ///
        /// 读到上限**再多一个字节**为止：多出来的那一个字节就是「超长」的判据，
        /// 留给 `read_json_body` 去认——判定只有一处。
        pub fn from_tiny_http(request: &mut tiny_http::Request) -> Self {
            let method = Method::from(request.method());
            let url = request.url().to_owned();
            let headers = request
                .headers()
                .iter()
                .map(|header| {
                    (
                        header.field.as_str().as_str().to_owned(),
                        header.value.as_str().to_owned(),
                    )
                })
                .collect();
            let mut body = Vec::new();
            let read = request
                .as_reader()
                .take(MAX_REQUEST_BODY_BYTES + 1)
                .read_to_end(&mut body)
                .map(|_| body)
                .map_err(|error| error.to_string());
            let mut parsed = Request::new(method, &url, Vec::new());
            parsed.headers = headers;
            parsed.body = read;
            parsed
        }
    }

    impl Response {
        /// 交回给 tiny_http 去写线。
        pub fn into_tiny_http(self) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
            let mut response = tiny_http::Response::from_data(self.body)
                .with_status_code(tiny_http::StatusCode(self.status));
            for (name, value) in &self.headers {
                let header = tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
                    .expect("响应头是本进程自己造的，必然合法");
                response = response.with_header(header);
            }
            response
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ColumnMapping, FailureKind, SourceColumn, WriteMode};

    struct FakeSource {
        columns: Vec<SourceColumn>,
        rows: std::vec::IntoIter<Vec<Option<String>>>,
    }

    impl RowSource for FakeSource {
        fn columns(&self) -> &[SourceColumn] {
            &self.columns
        }

        fn next_row(&mut self) -> Result<Option<Vec<Option<String>>>, SourceReadError> {
            Ok(self.rows.next())
        }
    }

    fn source(rows: usize) -> FakeSource {
        FakeSource {
            columns: vec![SourceColumn {
                name: "BIZ_ID".to_owned(),
                data_type: "NUMBER".to_owned(),
                precision: Some(10),
                scale: Some(0),
                length: None,
                fsp: None,
                support: None,
            }],
            rows: (0..rows)
                .map(|value| vec![Some(value.to_string())])
                .collect::<Vec<_>>()
                .into_iter(),
        }
    }

    fn spec(source_sql: Option<&str>, where_clause: &str) -> TaskSpec {
        TaskSpec {
            source_sql: source_sql.map(str::to_owned),
            dblink: None,
            owner: if source_sql.is_some() { "" } else { "APP" }.to_owned(),
            table: if source_sql.is_some() { "" } else { "ORDERS" }.to_owned(),
            target_table: "orders".to_owned(),
            where_clause: Some(where_clause.to_owned()),
            write_mode: WriteMode::Append,
            pre_sql: None,
            schedule_cron: None,
            schedule_enabled: false,
            primary_key: vec!["BIZ_ID".to_owned()],
            columns: vec![ColumnMapping {
                source: "ID".to_owned(),
                target: "BIZ_ID".to_owned(),
            }],
        }
    }

    #[test]
    fn preview_defaults_caps_and_uses_one_extra_row_for_truncation() {
        assert_eq!(preview_limit(None), Ok(10));
        assert_eq!(preview_limit(Some(500)), Ok(100));
        assert!(preview_limit(Some(0)).is_err());

        let preview = collect_preview(&spec(None, ""), 2, |_| Ok(source(3))).unwrap();
        assert_eq!(preview.columns, vec!["BIZ_ID"]);
        assert_eq!(
            preview.rows,
            vec![vec![Some("0".to_owned())], vec![Some("1".to_owned())]]
        );
        assert!(preview.truncated);

        let complete = collect_preview(&spec(None, ""), 2, |_| Ok(source(2))).unwrap();
        assert!(!complete.truncated);
    }

    #[test]
    fn preview_reader_receives_task_specs_exact_generated_sql() {
        let table = spec(None, "STATUS = 1");
        let expected_table = table.source_sql();
        collect_preview(&table, 1, |actual| {
            assert_eq!(actual, expected_table);
            Ok(source(0))
        })
        .unwrap();

        let custom = spec(Some("SELECT ID FROM APP.ORDERS;"), "");
        let expected_custom = custom.source_sql();
        assert!(expected_custom.contains("FROM ("));
        collect_preview(&custom, 1, |actual| {
            assert_eq!(actual, expected_custom);
            Ok(source(0))
        })
        .unwrap();
    }

    #[test]
    fn preview_timeout_is_504_and_other_source_errors_keep_failure_classification() {
        let mut timeout = SourceReadError::new("call timed out", Some(1067));
        timeout.timed_out = true;
        assert_eq!(preview_response(Err(timeout)).status, 504);

        let source_error = SourceReadError::with_kind(
            "table does not exist",
            Some(942),
            FailureKind::SourceQuery,
        );
        let response = preview_response(Err(source_error));
        assert_eq!(response.status, 502);
        assert!(response.body_text().contains("SOURCE_QUERY"));
    }
}
