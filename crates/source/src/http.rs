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
    CronSchedule, HttpSinkClient, SinkClient,
    session_cookie_header, session_token_from_cookie_header, validate_builder_dblink,
    validate_source_sql, Agent, AgentEndpoint, AgentEvidence, AgentInput, AgentStore, AuthStore,
    ColumnPrecision, DatasourceInput, DatasourceStore, HistoryChange, HistoryStore, OracleAccess,
    OracleRowSource, RowSource, RunEvidence, RunHistory, RunLogStore, RunLogWriter,
    RunParametersEvidence, RunTrigger, ScheduleRegistry,
    SourceColumn, SourceConfig, SourceEvidence, SourceReadError, TargetCheckRequest,
    TargetCheckResult, TargetConnection, TargetEvidence, Task, TaskConfig, TaskInput, TaskSpec,
    TaskStore, UnknownReason, RUN_LOG_PAGE_LIMIT, SESSION_IDLE_SECONDS, USERNAME,
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
    child_pid: Option<u32>,
    /// 判定用的那一份，**是枚举不是字符串**：能不能取消这次运行由它一个人说了算
    /// （`RunStage::abort_allowed`）。子进程报来一个认不出的拼写时它是 `None`，
    /// 与「还没报过」同待——两端版本对不上时，唯一安全的回答是「我不知道它在做什么」。
    /// 原样的文本另有去处：运行历史那一份仍是字符串，见 `RunHistory::stage`。
    stage: Option<RunStage>,
    /// 有人按过「停止运行」，且 SIGTERM 确实发出去了（#269）。
    ///
    /// 标记打在**发信号那一刻**，不是事后推断的：子进程被信号带走时不会留下任何
    /// 「我是被停的」的痕迹，父进程唯一知道这件事的时刻就是它自己按下扳机的时刻。
    /// 终态兜底据此把这次运行记成「已由用户停止」而不是「进程消失」——
    /// 主动停止与被 OOM 杀掉在历史里必须分得开。
    stop_requested: bool,
}

enum StartRunError {
    AlreadyRunning,
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
    Refused(String),
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
            Route::new(Post, "/api/agents", |state, request, _id| {
                handle_register_agent(request, state)
            }),
            Route::new(Post, "/api/agents/{}/probe", |state, _request, id| {
                handle_probe_agent(state, id)
            }),
            Route::new(Put, "/api/agents/{}", |state, request, id| {
                handle_update_agent(request, state, id)
            }),
            Route::new(Delete, "/api/agents/{}", |state, _request, id| {
                handle_delete_agent(state, id)
            }),
            Route::new(Get, "/api/datasources", |state, _request, _id| {
                handle_list_datasources(state.datasources)
            }),
            Route::new(Post, "/api/datasources", |state, request, _id| {
                handle_create_datasource(request, state)
            }),
            Route::new(Post, "/api/datasources/test-connection", |state, request, _id| {
                handle_test_datasource_draft(request, state)
            }),
            Route::new(
                Post,
                "/api/datasources/{}/test-connection",
                |state, _request, id| handle_test_datasource(state, id),
            ),
            Route::new(Get, "/api/datasources/{}", |state, _request, id| {
                handle_get_datasource(state.datasources, id)
            }),
            Route::new(Put, "/api/datasources/{}", |state, request, id| {
                handle_update_datasource(request, state, id)
            }),
            Route::new(Delete, "/api/datasources/{}", |state, _request, id| {
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
            Route::new(Get, "/api/runs", |state, request, _id| {
                handle_list_history(state.runs, state.history, request.query())
            }),
            Route::new(Get, "/api/runs/{}", |state, _request, id| {
                handle_get_run(state.runs, state.history, id)
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
                handle_create_task(request, state.tasks)
            }),
            Route::new(Get, "/api/tasks/{}", |state, _request, id| {
                handle_get_task(state.tasks, id)
            }),
            Route::new(Get, "/api/tasks/{}/curl", |state, request, id| {
                handle_task_curl(request, state, id)
            }),
            Route::new(Put, "/api/tasks/{}", |state, request, id| {
                handle_update_task(request, state.tasks, id)
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
            Some(token) => match self.auth.authenticate(token, Utc::now()) {
                Ok(true) => Some(token),
                Ok(false) => None,
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
                let Some(token) = session else {
                    return unauthorized();
                };
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

/// 「我登着吗」。**它是公开的**：让还没登录的人问这一句，前端才能在首屏决定
/// 摆登录页还是摆应用，而不必先撞一个 401 再从错误里反推。
fn handle_session_state(request: &Request, state: &Api<'_>) -> HttpResponse {
    let authenticated = match request
        .header("Cookie")
        .and_then(session_token_from_cookie_header)
    {
        Some(token) => match state.auth.authenticate(token, Utc::now()) {
            Ok(live) => live,
            Err(error) => return internal_error(error),
        },
        None => false,
    };
    json_response(
        200,
        &json!({
            "authenticated": authenticated,
            "username": if authenticated { Some(USERNAME) } else { None },
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
        Ok(false) => {
            return json_response(401, &json!({ "error": { "message": "账号或口令不正确" } }))
        }
        Err(error) => return internal_error(error),
    }
    let issued = match state.auth.issue_session(Utc::now()) {
        Ok(issued) => issued,
        Err(error) => return internal_error(error),
    };
    let mut response = json_response(200, &json!({ "authenticated": true, "username": USERNAME }));
    response.headers.push((
        "Set-Cookie".to_owned(),
        session_cookie_header(&issued.token, issued.max_age_seconds),
    ));
    response
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
    match state
        .auth
        .change_password(&input.current_password, &input.new_password, keep)
    {
        Ok(()) => json_response(200, &json!({ "message": "口令已修改" })),
        Err(error) => bad_request(error),
    }
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
        RunTrigger::Manual,
    ) {
        Ok(run_record_id) => json_response(202, &json!({ "run_record_id": run_record_id })),
        Err(StartRunError::AlreadyRunning) => json_response(
            409,
            &json!({ "error": { "message": "该任务已有一次运行进行中" } }),
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
    mark_stop_requested(runs, run_record_id, true);
    match send_sigterm(pid) {
        Ok(()) => json_response(202, &json!({ "message": "已发送 SIGTERM" })),
        Err(error) => {
            // 信号没发出去，这次运行还在跑：标记得撤回去，否则它将来真的意外死掉时
            // 会顶着一句「已由用户停止」，而那不是事实。
            mark_stop_requested(runs, run_record_id, false);
            internal_error(error)
        }
    }
}

fn mark_stop_requested(runs: &RunRegistry, run_record_id: &str, requested: bool) {
    if let Ok(mut registry) = runs.lock() {
        if let Some(run) = registry.active_runs.get_mut(run_record_id) {
            run.stop_requested = requested;
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
    run_record_id: &str,
) -> HttpResponse {
    let record = match runs.lock() {
        Ok(registry) => registry.live_histories.get(run_record_id).cloned(),
        Err(_) => return internal_error("run 投影锁已损坏".to_owned()),
    };
    if let Some(record) = record {
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
                "live": true,
            }),
        );
    }
    match history_store.get(run_record_id) {
        Ok(Some(history)) => history_response(&history),
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
                let values = merged.iter().map(history_value).collect::<Vec<_>>();
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

fn history_response(history: &RunHistory) -> HttpResponse {
    json_response(200, &history_value(history))
}

fn history_value(history: &RunHistory) -> Value {
    let mut value =
        serde_json::to_value(history).expect("serializing a run history response must succeed");
    let object = value
        .as_object_mut()
        .expect("run history serializes as an object");
    object.insert("live".to_owned(), Value::Bool(false));
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
        Err(error) => return DispatchOutcome::Refused(error),
    };
    let target = match state
        .datasources
        .target_connection(&task.target_datasource_id)
    {
        Ok(target) => target,
        Err(error) => return DispatchOutcome::Refused(error),
    };
    let agent = match resolve_target_agent(state, &task.target_datasource_id) {
        Ok(agent) => agent,
        Err(error) => return DispatchOutcome::Refused(error),
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
        Err(_) => return DispatchOutcome::Refused("run 控制锁已损坏".to_owned()),
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
        RunTrigger::Scheduled,
    ) {
        Ok(run_record_id) => DispatchOutcome::Started(run_record_id),
        Err(StartRunError::AlreadyRunning) => {
            DispatchOutcome::Refused("上次尚未结束，本次跳过".to_owned())
        }
        Err(StartRunError::Internal(error)) => DispatchOutcome::Refused(error),
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
    trigger: RunTrigger,
) -> Result<String, StartRunError> {
    let run_record_id = generate_run_record_id();
    // 历史里钉的是**当次实际执行**的语句文本：规格以后改了它也不跟着变。
    let mut history = RunHistory::accepted(
        &run_record_id,
        &task.task_id,
        &task.spec.source_sql(),
        Utc::now(),
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
            source_sql: task.spec.source_sql(),
        }),
    };
    register_active_run(runs, &run_record_id, &task.task_id, &agent.agent_id)?;
    if let Err(error) = history_store.insert(&history, Utc::now(), config.history_retention_days) {
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
            history.mark_parent_failure(error.clone(), Utc::now());
            let _ = history_store.save(&history, Utc::now(), config.history_retention_days);
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
            history.mark_parent_failure(message.clone(), Utc::now());
            let _ = history_store.save(&history, Utc::now(), config.history_retention_days);
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
) {
    let mut terminal_observed = false;
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
        let Some((change, history)) = apply_log_line(&runs, &run_record_id, &log) else {
            continue;
        };
        let is_terminal = change == HistoryChange::Terminal;
        let requires_persistence = change != HistoryChange::MemoryOnly;
        terminal_observed |= is_terminal;
        if requires_persistence
            && history_store
                .save(&history, Utc::now(), retention_days)
                .is_err()
        {
            continue;
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
        let reason = if wrapup.stop_requested {
            UnknownReason::StoppedByUser
        } else {
            UnknownReason::ProcessDisappeared
        };
        let history = runs.lock().ok().and_then(|mut registry| {
            let history = registry.live_histories.get_mut(&run_record_id)?;
            history.mark_unknown(reason, Utc::now());
            Some(history.clone())
        });
        if let Some(history) = history {
            let _ = history_store.save(&history, Utc::now(), retention_days);
        }
        // 子进程没来得及说完的那句 abort，父进程替它说（#269）。**必须在 `child.wait()`
        // 之后**：暂存表要等发起写入的那个进程死透了才动得。
        abort_on_behalf_of_child(&wrapup, &mut run_log);
    }
    remove_live_history(&runs, &run_record_id);
    remove_active_run(&runs, &run_record_id);
}

/// 子进程退出后，父进程收尾要用到的全部事实。
struct RunWrapup {
    /// 这次死亡是不是我们自己要的（`handle_cancel_run` 发信号时打的标记）。
    stop_requested: bool,
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
            stop_requested: false,
            run_id: None,
            agent_base_url: None,
            stage: None,
        };
    };
    let history = registry.live_histories.get(run_record_id);
    RunWrapup {
        stop_requested: registry
            .active_runs
            .get(run_record_id)
            .is_some_and(|run| run.stop_requested),
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
///
/// 失败**不吞**：落成一行 `abort_failed` 运行日志，和子进程自己 abort 失败时写的那行
/// 同一个形状。这里不重试——「abort 不承诺可靠性」那条没有变。
fn abort_on_behalf_of_child(wrapup: &RunWrapup, run_log: &mut RunLogWriter) {
    let Some(run_id) = wrapup.run_id.as_deref() else {
        return;
    };
    // 认不出的阶段拼写（子进程比父进程新）按「可能还没封口」办：多发一次 abort 是幂等的，
    // 少发一次却会把目标表永久锁住。
    if wrapup.stage.is_some_and(|stage| !stage.abort_allowed()) {
        return;
    }
    let Some(base_url) = wrapup.agent_base_url.as_deref() else {
        log_abort_failed(run_log, run_id, "运行证据里没有目标端 agent 地址".to_owned());
        return;
    };
    let mut sink = match HttpSinkClient::new(base_url) {
        Ok(sink) => sink,
        Err(error) => {
            log_abort_failed(run_log, run_id, error);
            return;
        }
    };
    if let Err(error) = sink.abort(run_id) {
        log_abort_failed(run_log, run_id, error.message);
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
) -> Result<(), StartRunError> {
    let mut runs = runs
        .lock()
        .map_err(|_| StartRunError::Internal("run 控制锁已损坏".to_owned()))?;
    if runs.active_runs.values().any(|run| run.task_id == task_id) {
        return Err(StartRunError::AlreadyRunning);
    }
    runs.active_runs.insert(
        run_record_id.to_owned(),
        ActiveRun {
            task_id: task_id.to_owned(),
            agent_id: agent_id.to_owned(),
            child_pid: None,
            stage: None,
            stop_requested: false,
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

fn handle_create_task(request: &Request, store: &TaskStore) -> HttpResponse {
    let input: TaskInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    // 校验先判一次，好让它落在 400 上。`store.create` 里还会再判一次——那一次是存储层
    // 自己的门，不依赖任何调用方记得先问；这一次只为把「你写错了」和「服务端坏了」
    // 分成两个状态码。500 会让人去看服务端日志找一个根本不在那里的故障。
    if let Err(error) = input.validate() {
        return bad_request(error);
    }
    match store.create(input) {
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
    // 手拼而不是走 `json!`：`serde_json` 的 map 是有序的**字典序**，于是
    // `password` 会排到 `username` 前面——功能上无所谓，但这段命令是给人读的。
    // 两个值都是常量，都不含引号，没有转义面。
    let credentials = format!(r#"{{"username":"{USERNAME}","password":"改成你的口令"}}"#);
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

fn handle_update_task(request: &Request, store: &TaskStore, task_id: &str) -> HttpResponse {
    let input: TaskInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    if let Err(error) = input.validate() {
        return bad_request(error);
    }
    match store.update(task_id, input) {
        Ok(Some(task)) => json_response(200, &task),
        Ok(None) => not_found(),
        Err(error) => internal_error(error),
    }
}

/// 删任务：还有运行没结束就拒（#270），与删数据源 / 删 agent 同一形态的 409。
///
/// 不做「自动停止再删除」：删除不可逆，让它顺手终止一次可能正在往目标库写数据的运行，
/// 风险大于便利。正确的顺序是用户先停止、等这次运行收尾，再删任务。
///
/// 「还没结束」的判据就是 `has_active_run` 那把互斥键（任务本身）——与「能不能再发起
/// 一次」完全同一个判据，所以「停完了就删得掉」和「停完了就能再跑」在同一刻成立，
/// 用户不会看见两处互相矛盾的说法。
fn handle_delete_task(state: &Api<'_>, task_id: &str) -> HttpResponse {
    let active = match state.runs.lock() {
        Ok(registry) => registry.active_run_ids(task_id),
        Err(_) => return internal_error("run 控制锁已损坏".to_owned()),
    };
    if !active.is_empty() {
        // 点名到 run_record_id：一个任务至多一次在飞，但报文形状跟着删数据源那条走
        // （复数名词的数组 + 一句自己就能读懂的 message），前端拿不到数组时原样显示它。
        return json_response(
            409,
            &json!({
                "error": {
                    "message": format!(
                        "任务还有运行没结束（{}）；请先停止这次运行，等它收尾后再删除任务",
                        active.join("、")
                    ),
                    "runs": active,
                }
            }),
        );
    }
    match state.tasks.delete(task_id) {
        Ok(Some(task)) => json_response(200, &task),
        Ok(None) => not_found(),
        Err(error) => internal_error(error),
    }
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
