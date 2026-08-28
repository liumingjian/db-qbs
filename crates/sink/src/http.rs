//! sink 的 HTTP 面：请求进来、路由、handler、响应出去。
//!
//! 唯一的入口是 `Api::handle(&Request) -> Response`，用的是这个 crate 自己的
//! `Request`/`Response`。从前 `handle_request` 直接吃 `tiny_http::Request`，于是
//! `tests/` 想碰路由层就只能开一个真 socket 手搓 HTTP——业务那半边早就在
//! `SinkService` 接缝后面进程内直调了，只有路由这一层还够不着（#200，跟着 #198 走）。
//!
//! 路由表是**数据**（`routes()`），不是一串 `if`。匹配分两趟：字面量样式先走一趟，
//! 带占位的样式后走一趟，所以像 `/v1/runs/commit` 这样的字面量路由不可能被
//! `/v1/runs/{}` 吃掉，**无论表里怎么排**——声明顺序不承重。

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use db_qbs_shared::{write_log_line_with_fields, AgentInfo, LogEvent, LogLevel};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tiny_http::Server;

use crate::{
    load_agent_identity, ApiError, BatchPayload, CommitRequest, Destination, DestinationFactory,
    MysqlDestination, MysqlFactory, OpenRunRequest, SinkConfig, SinkService, TargetCheckRequest,
    TargetConnection,
};

const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;

/// HTTP 方法。认不出来的方法落进 `Other`，而路由表里只有前两种，所以它必然 404——
/// 与旧实现里那条 `_ => not_found()` 同一个结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Other,
}

/// 一次请求：方法、路径、头、请求体。
///
/// `body` 是 `Result` 而不是 `Vec<u8>`：读请求体可能失败，而失败必须由**读它的那个
/// handler** 报成 400，所以这个错误得一路带到 `read_json` 那里，不能在翻译层就吞掉。
pub struct Request {
    method: Method,
    path: String,
    headers: Vec<(String, String)>,
    body: Result<Vec<u8>, String>,
}

impl Request {
    pub fn new(method: Method, path: &str, body: Vec<u8>) -> Self {
        Self {
            method,
            path: path.to_owned(),
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

    /// 一份同名头**里的任意一份**取到这个值。
    ///
    /// 不是「第一份是不是它」：同名头可以来好几份，而从前那道 Content-Type 判定
    /// 走的就是 `any`。收窄成看第一份，会让一份合法的请求变成 415。
    pub fn header_matches(&self, name: &str, value: &str) -> bool {
        self.headers.iter().any(|(header, existing)| {
            header.eq_ignore_ascii_case(name) && existing.eq_ignore_ascii_case(value)
        })
    }

    /// 请求体的字节。读失败时是 `Err`，超长时是一段比上限多一个字节的 `Ok`——
    /// 「超过 64 MiB」这个判定归 `read_json`，与从前同一处。
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

/// 一次请求能碰到的**全部**进程状态：一个服务实例加这台 agent 的身份。
///
/// 借用形态是刻意的：两者的所有权归 `serve()`，`Api` 只在一次服务期内借着用，
/// 测试里同样可以就地拼一个出来。
pub struct Api<'a, F: DestinationFactory> {
    pub service: &'a SinkService<F>,
    pub agent: &'a AgentInfo,
}

type Handler<F> = fn(&Api<'_, F>, &Request, &str) -> Response;

/// 一条路由。`pattern` 里最多有一个 `{}`，代表一段 run id。
pub struct Route<F: DestinationFactory> {
    pub method: Method,
    pub pattern: &'static str,
    handler: Handler<F>,
}

impl<F: DestinationFactory> Route<F> {
    fn new(method: Method, pattern: &'static str, handler: Handler<F>) -> Self {
        Self {
            method,
            pattern,
            handler,
        }
    }

    /// 带占位的样式比字面量样式**后**匹配，见 `Api::handle`。
    pub fn has_placeholder(&self) -> bool {
        self.pattern.contains("{}")
    }
}

/// 把请求路径按样式对一遍：对上了就回样式里 `{}` 那一段，没有占位时回空串。
///
/// 这一个函数取代了从前的 `run_resource` 与 `run_action`——两者是同一段逻辑
/// 把前后缀内联了一遍。run id 里不许有 `/`、不许为空，与从前一字不差。
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

/// 全部 v1 路由。**表里的先后不承重**——`Api::handle` 分两趟走，字面量永远压过占位。
///
/// 表按 `F` 现造：handler 是 `fn(&Api<'_, F>, ..)`，泛型函数里放不下一份
/// `static`。十条路由的一次 `Vec`，与它后面那次 MySQL 往返比可以忽略。
pub fn routes<F: DestinationFactory>() -> Vec<Route<F>> {
    use Method::{Get, Post};
    vec![
        Route::new(Get, "/v1/agent/info", |api, _request, _run_id| {
            json_response(200, &api.agent_info())
        }),
        Route::new(Post, "/v1/runs", |api, request, _run_id| {
            handle_open(request, api.service)
        }),
        Route::new(
            Post,
            "/v1/target/test-connection",
            |_api, request, _run_id| handle_test_connection(request),
        ),
        Route::new(Post, "/v1/target/tables", |_api, request, _run_id| {
            handle_target_tables(request)
        }),
        Route::new(Post, "/v1/target/columns", |_api, request, _run_id| {
            handle_target_columns(request)
        }),
        Route::new(Post, "/v1/target/check", |api, request, _run_id| {
            handle_target_check(request, api.service)
        }),
        Route::new(Post, "/v1/runs/{}/batches", |api, request, run_id| {
            handle_batch(request, api.service, run_id)
        }),
        Route::new(Post, "/v1/runs/{}/commit", |api, request, run_id| {
            handle_commit(request, api.service, run_id)
        }),
        Route::new(Post, "/v1/runs/{}/abort", |api, request, run_id| {
            handle_abort(request, api.service, run_id)
        }),
        Route::new(Get, "/v1/runs/{}", |api, _request, run_id| {
            handle_get(api.service, run_id)
        }),
    ]
}

impl<F: DestinationFactory> Api<'_, F> {
    /// 这台 agent 的身份自述 + 它最近一次观察到的 MySQL（#257）。
    ///
    /// 身份那三个字段在进程起来之前就定了（`load_agent_identity` 先于监听），
    /// 而 MySQL 那一份**要等到手上有凭据的请求来过一次才有**——sink 自己不持有目标端
    /// 凭据（ADR-0037 §2）。所以这一份是每次现拼的：静态身份 + 服务里的那份缓存。
    /// 没观察过就不带 `mysql` 字段，读的一方按「未知」处理，不许当成 8.0。
    pub fn agent_info(&self) -> AgentInfo {
        AgentInfo {
            mysql: self.service.observed_mysql(),
            ..self.agent.clone()
        }
    }

    /// 这个 crate 的 HTTP 面**唯一**的入口。
    ///
    /// 两趟匹配：先字面量样式，再带占位的样式。这就是「顺序不承重」的全部机制——
    /// 从前 `/v1/runs/` 下的字面量路由要靠写在按 run id 分发的那一支之前才不被吃掉，
    /// 现在即便把表倒过来写，结果也一个字节不变。
    pub fn handle(&self, request: &Request) -> Response {
        let routes = routes::<F>();
        for placeholders in [false, true] {
            for route in &routes {
                if route.has_placeholder() != placeholders || route.method != request.method() {
                    continue;
                }
                let Some(run_id) = match_pattern(route.pattern, request.path()) else {
                    continue;
                };
                return (route.handler)(self, request, run_id);
            }
        }
        error_response(not_found())
    }
}

pub fn serve(config: SinkConfig) -> Result<(), String> {
    if config.listen.is_empty() {
        return Err("sink 配置 listen 不能为空".to_owned());
    }
    {
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        let _ = write_log_line_with_fields(
            &mut writer,
            LogLevel::Warn,
            LogEvent::SinkStarted,
            None,
            None,
            json!({
                "listen": &config.listen,
                "message": format!(
                    "本服务无鉴权，能连上者可用调用方给的凭据清空并重写任意暂存表与目标表；当前监听地址：{}",
                    config.listen,
                ),
            }),
        );
    }
    // 退役字段仍能解析，但一个字都不读（ADR-0037 §2）。留一条 warn，
    // 否则部署者会以为 `sink.toml` 里那份凭据仍然是生效的那一份。
    if config.mysql_dsn.is_some() || config.database.is_some() {
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        let _ = write_log_line_with_fields(
            &mut writer,
            LogLevel::Warn,
            LogEvent::SinkStarted,
            None,
            None,
            json!({
                "message": "sink.toml 的 mysql_dsn / database 已退役且不再被读取（ADR-0037 §2）：目标端凭据随每个 run 的请求过线，请从配置文件里删掉这两个字段",
            }),
        );
    }
    // 身份先于监听：起不来就别开门（ADR-0044 §2）。id 文件写不下去时 source 那侧的
    // 「注册」会在下一次重启后认到另一个身份，那正是本票要挡的静默——所以这里硬失败。
    let agent = load_agent_identity(&config.agent_id_path(), config.agent_name.as_deref())?;
    {
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        let _ = write_log_line_with_fields(
            &mut writer,
            LogLevel::Info,
            LogEvent::SinkStarted,
            None,
            None,
            json!({
                "agent_id": &agent.agent_id,
                "agent_name": &agent.name,
                "version": &agent.version,
                "message": "本进程即目标端 agent；请在 source 的「目标端 Agent」屏用这个地址注册它（ADR-0044 §3）",
            }),
        );
    }
    // sink 启动**不再连 MySQL**：连接按 run 建，连不上的失败点在 POST /v1/runs。
    let service = SinkService::with_factory(MysqlFactory)
        .with_max_concurrent_runs(config.max_concurrent_runs);
    let server = Server::http(&config.listen)
        .map_err(|error| format!("监听 {} 失败：{error}", config.listen))?;

    let api = Api {
        service: &service,
        agent: &agent,
    };
    // 多线程 accept：HTTP_WORKER_THREADS 条工作线程**共用同一个监听器**（#260）。
    //
    // 在这之前只有一条线程，一个请求全程处理完才回头取下一个——而这台服务上每个请求
    // 都是一次同步的 MySQL 往返：建暂存表、写批次、切换。一次切换（整表 upsert，
    // 分钟级）期间，**所有**其他任务的批次推送都堵在门外，「同一时刻只有一个任务在跑」
    // 因此是进程结构决定的，跟任务那层的互斥键没关系。
    //
    // 用 `thread::scope` 而不是 `thread::spawn`：`Api` 借着栈上的 `service` 与 `agent`，
    // 作用域线程让借用照旧成立，不必把这两份状态搬进 `Arc`。
    //
    // 线程数取固定值，不按核数算：这里等的是**阻塞 IO**，不是 CPU。它也**不是**并发额度——
    // 同时能有几个 run 在飞由 `max_concurrent_runs` 说了算，工作线程只决定同一瞬间
    // 能有几个 HTTP 请求在处理。线程比额度多几条是故意的：额度满时那句拒绝、
    // 以及 `GET /v1/runs/{}` 这类只读接口，得能在几个慢写入在飞的时候照样答得出来。
    let terminated = AtomicBool::new(false);
    let workers: Vec<_> = thread::scope(|scope| {
        let handles: Vec<_> = (0..HTTP_WORKER_THREADS)
            .map(|_| scope.spawn(|| accept_loop(&server, &api, &terminated)))
            .collect();
        handles
            .into_iter()
            // 一条工作线程 panic 了，剩下几条照常服务——但退出时得说出来，
            // 否则「服务半死不活」会以退出码 0 收场。
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err("HTTP 工作线程 panic 退出".to_owned()))
            })
            .collect()
    });
    for outcome in workers {
        outcome?;
    }
    Ok(())
}

/// 共用监听器的工作线程数。见 `serve` 里那段说明。
const HTTP_WORKER_THREADS: usize = 8;

/// 一条工作线程的一辈子：取一个请求、处理完、回头再取。
///
/// `recv_timeout` 在多条线程上同时调是 tiny_http 支持的（`Server: Send + Sync`，
/// 内部自带队列）。用带超时的轮询而不是无限阻塞的 `recv()`，是为了让**任何一条线程
/// 取请求失败时，其余几条也走得掉**：否则 `serve` 会卡在 `join` 上，进程既不服务
/// 也不退出。sink 没有 SIGTERM 处理，收到信号按默认语义直接结束进程。
fn accept_loop<F: DestinationFactory>(
    server: &Server,
    api: &Api<'_, F>,
    terminated: &AtomicBool,
) -> Result<(), String> {
    while !terminated.load(Ordering::Relaxed) {
        match server.recv_timeout(Duration::from_millis(100)) {
            Ok(Some(request)) => handle_request(request, api),
            Ok(None) => {}
            Err(error) => {
                terminated.store(true, Ordering::Relaxed);
                return Err(format!("接收 HTTP 请求失败：{error}"));
            }
        }
    }
    Ok(())
}

/// 一个 tiny_http 请求的全程：翻译进来、交给 `Api::handle`、翻译回去。
///
/// 认识 `tiny_http` 的只有三处：`serve` 的监听循环、这个函数、底下的 `bridge`。
/// 判断怎么回，全在 `Api::handle` 里。
fn handle_request<F: DestinationFactory>(mut request: tiny_http::Request, api: &Api<'_, F>) {
    let parsed = Request::from_tiny_http(&mut request);
    let run_id = run_id_in_path::<F>(parsed.path());
    let response = api.handle(&parsed).into_tiny_http();

    if let Err(error) = request.respond(response) {
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        let _ = write_log_line_with_fields(
            &mut writer,
            LogLevel::Error,
            LogEvent::HttpResponseFailed,
            run_id.as_deref(),
            None,
            json!({ "message": format!("HTTP 响应写入失败：{error}") }),
        );
    }
}

/// 响应写不出去时，日志里那个 run id。**只给日志用**——分发不看它，分发看的是路由表。
///
/// 它自己也读那张表：带占位的路由多一条，这里就多认一条，不必有人记得来改。
fn run_id_in_path<F: DestinationFactory>(path: &str) -> Option<String> {
    routes::<F>()
        .iter()
        .filter(|route| route.has_placeholder())
        .find_map(|route| match_pattern(route.pattern, path))
        .map(str::to_owned)
}

fn handle_open(request: &Request, service: &SinkService<impl DestinationFactory>) -> Response {
    if !has_json_content_type(request) {
        return error_response(unsupported_media_type(None));
    }
    let request: OpenRunRequest = match read_json(request) {
        Ok(request) => request,
        Err(error) => return error_response(error),
    };
    match service.open(request) {
        // 两种 outcome 都是 200，区别写在报文里——那份暗号由 `OpenOutcome` 一处编码。
        Ok(outcome) => json_response(200, &outcome.into_response()),
        Err(error) => error_response(error),
    }
}

fn handle_batch(
    request: &Request,
    service: &SinkService<impl DestinationFactory>,
    run_id: &str,
) -> Response {
    let payload: BatchPayload = match read_run_json(request, run_id) {
        Ok(payload) => payload,
        Err(error) => return error_response(error),
    };
    match service.write_batch(run_id, payload) {
        Ok(response) => json_response(200, &response),
        Err(error) => error_response(error),
    }
}

fn handle_abort(
    request: &Request,
    service: &SinkService<impl DestinationFactory>,
    run_id: &str,
) -> Response {
    if !has_json_content_type(request) {
        return error_response(unsupported_media_type(Some(run_id)));
    }
    if let Err(error) = read_json::<EmptyBody>(request) {
        return error_response(error);
    }
    match service.abort(run_id) {
        Ok(response) => json_response(200, &response),
        Err(error) => error_response(error),
    }
}

fn handle_commit(
    request: &Request,
    service: &SinkService<impl DestinationFactory>,
    run_id: &str,
) -> Response {
    let payload: CommitRequest = match read_run_json(request, run_id) {
        Ok(payload) => payload,
        Err(error) => return error_response(error),
    };
    match service.commit(run_id, payload.total_batches, payload.total_rows) {
        Ok(response) => json_response(200, &response),
        Err(error) => error_response(error),
    }
}

fn handle_get(service: &SinkService<impl DestinationFactory>, run_id: &str) -> Response {
    match service.get(run_id) {
        Ok(response) => json_response(200, &response),
        Err(error) => error_response(error),
    }
}

/// 「测试连接」（ADR-0037 §9）——**不属于任何 run**，所以它不进 run 注册表、
/// 不留 tombstone，也不需要服务实例。source 侧的数据源管理面靠它验 MySQL 那一侧。
fn handle_test_connection(request: &Request) -> Response {
    if !has_json_content_type(request) {
        return error_response(unsupported_media_type(None));
    }
    let target: TargetConnection = match read_json(request) {
        Ok(target) => target,
        Err(error) => return error_response(error),
    };
    match MysqlDestination::test_connection(&target) {
        Ok(()) => json_response(200, &json!({ "ok": true })),
        // 码闭集不增：连不上是目标端环境故障（ADR-0037 §9）。
        Err(message) => error_response(ApiError {
            status: 500,
            code: "SINK_ENVIRONMENT",
            message: format!("连接目标端失败：{message}"),
            run_id: None,
            details: json!({ "kind": "OTHER" }),
        }),
    }
}

/// `POST /v1/target/columns` 的请求体。
///
/// 连接**嵌在 `target` 里**，不 flatten 进顶层：`OpenRunRequest` 已经是这个形状，
/// 而 serde 的 `flatten` 与 `deny_unknown_fields` 不能共存——拼字段名的错就会静默通过。
/// `/v1/target/tables` 没有第二个字段，所以它原样收一个 `TargetConnection`，
/// 与 `/v1/target/test-connection` 一致。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetColumnsRequest {
    target: TargetConnection,
    target_table: String,
}

/// 目标端元数据面（ADR-0038 §3）：ADR-0027 §3 那道封条到这里完整解除。
///
/// 与 `test-connection` 同属「不属于任何 run 的端点」——**不产生 `run_id`、不进 run 注册表、
/// 不留 tombstone、不写任何存储**，连接按请求建、用完即弃（`MysqlDestination` 出作用域即断）。
/// 它喂的是**选择面**，不是判定面：拦截层仍然只有映射预检一处（ADR-0009 增补 §3 一字不改）。
fn handle_target_tables(request: &Request) -> Response {
    if !has_json_content_type(request) {
        return error_response(unsupported_media_type(None));
    }
    let target: TargetConnection = match read_json(request) {
        Ok(target) => target,
        Err(error) => return error_response(error),
    };
    let destination = match MysqlDestination::connect(&target) {
        Ok(destination) => destination,
        Err(message) => return error_response(target_environment(message)),
    };
    match destination.target_tables() {
        Ok(tables) => json_response(200, &json!({ "tables": tables })),
        Err(message) => error_response(target_environment(message)),
    }
}

/// 一张目标表的列清单与唯一性约束（ADR-0038 §3）。
///
/// **表不存在不是错误**：`information_schema` 查不到就是空清单（ADR-0038 §9）。
/// 构建器只亮不判——「这张表能不能用」的结论归映射预检出。
fn handle_target_columns(request: &Request) -> Response {
    if !has_json_content_type(request) {
        return error_response(unsupported_media_type(None));
    }
    let payload: TargetColumnsRequest = match read_json(request) {
        Ok(payload) => payload,
        Err(error) => return error_response(error),
    };
    let destination = match MysqlDestination::connect(&payload.target) {
        Ok(destination) => destination,
        Err(message) => return error_response(target_environment(message)),
    };
    let columns = match destination.target_columns(&payload.target_table) {
        Ok(columns) => columns,
        Err(message) => return error_response(target_environment(message)),
    };
    let keys = match destination.target_keys(&payload.target_table) {
        Ok(keys) => keys,
        Err(message) => return error_response(target_environment(message)),
    };
    json_response(200, &json!({ "columns": columns, "keys": keys }))
}

fn handle_target_check<F: DestinationFactory>(
    request: &Request,
    service: &SinkService<F>,
) -> Response {
    if !has_json_content_type(request) {
        return error_response(unsupported_media_type(None));
    }
    let payload: TargetCheckRequest = match read_json(request) {
        Ok(payload) => payload,
        Err(error) => return error_response(error),
    };
    match service.check_target(payload) {
        Ok(result) => json_response(200, &result),
        Err(error) => error_response(error),
    }
}

/// 错误码闭集不增（ADR-0010 十五码，ADR-0038 §9）：目标端连不上或查不动，
/// 都是目标端环境故障，与 `test-connection` 同一个码、同一个 `details.kind`。
fn target_environment(message: String) -> ApiError {
    ApiError {
        status: 500,
        code: "SINK_ENVIRONMENT",
        message: format!("读取目标端元数据失败：{message}"),
        run_id: None,
        details: json!({ "kind": "OTHER" }),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyBody {}

fn read_run_json<T: DeserializeOwned>(request: &Request, run_id: &str) -> Result<T, ApiError> {
    if !has_json_content_type(request) {
        return Err(unsupported_media_type(Some(run_id)));
    }
    read_json(request).map_err(|mut error| {
        error.run_id = Some(run_id.to_owned());
        error
    })
}

fn read_json<T: DeserializeOwned>(request: &Request) -> Result<T, ApiError> {
    let body = request
        .body()
        .map_err(|error| bad_request(format!("读取请求体失败：{error}")))?;
    if body.len() as u64 > MAX_BODY_BYTES {
        return Err(ApiError {
            status: 413,
            code: "PAYLOAD_TOO_LARGE",
            message:
                "请求体超过 64 MiB 断路器；这是批次预算逻辑缺陷，不是数据或环境问题，请报 issue"
                    .to_owned(),
            run_id: None,
            details: json!({ "max_bytes": MAX_BODY_BYTES }),
        });
    }
    serde_json::from_slice(body).map_err(|error| bad_request(format!("JSON 请求体无效：{error}")))
}

fn has_json_content_type(request: &Request) -> bool {
    request.header_matches("Content-Type", "application/json")
}

fn unsupported_media_type(run_id: Option<&str>) -> ApiError {
    ApiError {
        status: 415,
        code: "BAD_REQUEST",
        message: "Content-Type 必须是 application/json".to_owned(),
        run_id: run_id.map(str::to_owned),
        details: json!({}),
    }
}

fn bad_request(message: String) -> ApiError {
    ApiError {
        status: 400,
        code: "BAD_REQUEST",
        message,
        run_id: None,
        details: json!({}),
    }
}

fn not_found() -> ApiError {
    ApiError {
        status: 404,
        code: "RUN_UNKNOWN",
        message: "请求的 sink v1 资源不存在".to_owned(),
        run_id: None,
        details: json!({}),
    }
}

fn error_response(error: ApiError) -> Response {
    let status = error.status;
    json_response(status, &error.into_envelope())
}

fn json_response(status: u16, value: &impl Serialize) -> Response {
    let body = serde_json::to_vec(value).expect("serializing an HTTP response must succeed");
    Response {
        status,
        headers: vec![("Content-Type".to_owned(), "application/json".to_owned())],
        body,
    }
}

/// tiny_http 与这个模块之间的翻译层。`serve` 的监听循环之外，只剩这两个函数还认识它。
mod bridge {
    use std::io::Read;

    use super::{Method, Request, Response, MAX_BODY_BYTES};

    impl From<&tiny_http::Method> for Method {
        fn from(method: &tiny_http::Method) -> Self {
            match method {
                tiny_http::Method::Get => Method::Get,
                tiny_http::Method::Post => Method::Post,
                _ => Method::Other,
            }
        }
    }

    impl Request {
        /// 从 tiny_http 的请求里把方法、URL 和请求体取出来。
        ///
        /// 读到上限**再多一个字节**为止：多出来的那一个字节就是「超长」的判据，
        /// 留给 `read_json` 去认——判定只有一处。
        ///
        /// 请求体在这里**一律读**，包括 GET 与没匹配上任何路由的那些。从前它只在
        /// handler 里读，所以打到不存在的路径上的一个大 body 会被直接丢掉；
        /// 代价是那种请求现在也要先缓冲到 64 MiB 上限才答 404，状态码一个字没变。
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
                .take(MAX_BODY_BYTES + 1)
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
