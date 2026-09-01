//! source 的 HTTP 面，**进程内直调**。
//!
//! 这里一条测试都不 spawn 二进制、不开 socket：`Api::handle(&Request) -> Response`
//! 就是全部入口。`source_skeleton.rs` 里那几条哨兵仍然走真进程，证的是「二进制起得来、
//! 对外真的在服务」；判断怎么回，归这里。

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use db_qbs_source::http::{
    reclaim_after_restart, routes, stop_runs_for_shutdown, Access, Api, Method, Request, Response,
    RunState,
};
use db_qbs_source::{
    load_task_config, AgentStore, AlertOutboxStore, AuthStore, Clock, DatasourceStore,
    EmailAlertStore, HistoryStore, MailTransport, MailTransportError, OracleAccess,
    OracleRowSource, OutgoingMail, RunHistory, RunLogStore, ScheduleState, SourceColumn,
    SourceConfig, SourceReadError, TaskInput, TaskSpec, TaskStore, SESSION_COOKIE,
};
use serde_json::Value;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestClock(Mutex<chrono::DateTime<chrono::Utc>>);

impl TestClock {
    fn new(now: chrono::DateTime<chrono::Utc>) -> Self {
        Self(Mutex::new(now))
    }

    fn set(&self, now: chrono::DateTime<chrono::Utc>) {
        *self.0.lock().unwrap() = now;
    }
}

impl Clock for TestClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        *self.0.lock().unwrap()
    }
}

#[derive(Debug, Clone)]
struct RecordedMail {
    host: String,
    port: u16,
    security: db_qbs_source::SmtpSecurity,
    username: String,
    secret: String,
    sender_name: String,
    mail: OutgoingMail,
}

#[derive(Default)]
struct FakeMailTransport {
    sent: Mutex<Vec<RecordedMail>>,
    failure: Mutex<Option<MailTransportError>>,
}

impl MailTransport for FakeMailTransport {
    fn send(
        &self,
        settings: &db_qbs_source::EmailDeliverySettings,
        mail: &OutgoingMail,
    ) -> Result<(), MailTransportError> {
        self.sent.lock().unwrap().push(RecordedMail {
            host: settings.host.clone(),
            port: settings.port,
            security: settings.security,
            username: settings.username.clone(),
            secret: settings.secret.clone(),
            sender_name: settings.sender_name.clone(),
            mail: mail.clone(),
        });
        match *self.failure.lock().unwrap() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// 一台**在应答**的目标端 agent 桩：认 `/v1/agent/info` 与 `/v1/runs/{id}/abort`，别的一律 503。
///
/// 与 `source_skeleton.rs` 里那台同一个形状——注册 agent、探测、目标端元数据这几条
/// 都得先过它，一台不在线的 agent 会让它们统统停在「agent 不在线」那一步。
///
/// abort 也得认：子进程一死，父进程就会替它补发一次（#269）。这台桩是**全局共享**的，
/// 想对某一次 abort 下断言的用例请另起一台 [`abort_recording_agent`]。
fn agent_stub_url() -> &'static str {
    static STUB: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    STUB.get_or_init(|| {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut head = [0_u8; 1024];
                let read = stream.read(&mut head).unwrap_or(0);
                let request_line = String::from_utf8_lossy(&head[..read])
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let info = request_line.contains("/v1/agent/info");
                let aborted = aborted_run_id(&request_line);
                let body = if info {
                    r#"{"agent_id":"stub-agent","name":"桩 agent","version":"0.0.0-test","max_concurrent_runs":1}"#.to_owned()
                } else if let Some(run_id) = &aborted {
                    format!(r#"{{"run_id":"{run_id}","staging_dropped":true}}"#)
                } else {
                    r#"{"error":{"code":"BAD_REQUEST","message":"桩只认 /v1/agent/info","run_id":null,"details":{}}}"#.to_owned()
                };
                let status = if info || aborted.is_some() {
                    "200 OK"
                } else {
                    "503 Service Unavailable"
                };
                respond_and_close(stream, status, &body);
            }
        });
        url
    })
    .as_str()
}

/// 回一条响应，然后**体面地**关掉这条连接。
///
/// 写完就 drop 会在客户端还没写完请求时发出 RST，客户端于是连状态行都读不到
/// （macOS 上是 `Error encountered in the status line: Invalid argument (os error 22)`）——
/// 一条与被测行为毫无关系的偶发失败。所以：写完先半关，再把对端剩下的字节读干净，
/// 让四次挥手走完。读超时兜底，免得一条赖着不走的连接把这条单线程 accept 循环堵死。
fn respond_and_close(mut stream: TcpStream, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut drained = Vec::new();
    let _ = stream.read_to_end(&mut drained);
}

/// `POST /v1/runs/<run_id>/abort` 里的那个 run id，不是这条路就是 `None`。
fn aborted_run_id(request_line: &str) -> Option<String> {
    let path = request_line.split_whitespace().nth(1)?;
    let rest = path.strip_prefix("/v1/runs/")?;
    Some(rest.strip_suffix("/abort")?.to_owned())
}

/// 一台**只记 abort** 的目标端 agent 桩：`/v1/agent/info` 照常应答，
/// `/v1/runs/{id}/abort` 按 `abort_succeeds` 成或败，并把见到的每一个 run id 记下来。
///
/// 它回答的是 #269 那两个问题：父进程到底替子进程发了没有，以及发去的是哪一次运行。
fn abort_recording_agent(abort_succeeds: bool) -> (String, Arc<Mutex<Vec<String>>>) {
    let (url, aborts, _switch) = switchable_abort_agent(abort_succeeds);
    (url, aborts)
}

/// 同一台桩，但 abort 成不成**中途可以改**。
///
/// #271 的重试要走的正是「先失败、再成功」这条路：第一次补发失败、占用留在目标端，
/// 人点一下重试、这次成了。没有这个开关就演不出那一幕。
fn switchable_abort_agent(
    abort_succeeds: bool,
) -> (String, Arc<Mutex<Vec<String>>>, Arc<AtomicBool>) {
    let succeeds = Arc::new(AtomicBool::new(abort_succeeds));
    let switch = Arc::clone(&succeeds);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let aborts = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&aborts);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut head = [0_u8; 1024];
            let read = stream.read(&mut head).unwrap_or(0);
            let request_line = String::from_utf8_lossy(&head[..read])
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            let info = request_line.contains("/v1/agent/info");
            let aborted = aborted_run_id(&request_line);
            if let Some(run_id) = &aborted {
                recorder.lock().unwrap().push(run_id.clone());
            }
            let (status, body) = if info {
                (
                    "200 OK",
                    r#"{"agent_id":"stub-agent","name":"桩 agent","version":"0.0.0-test","max_concurrent_runs":1}"#.to_owned(),
                )
            } else if let Some(run_id) = &aborted {
                if succeeds.load(Ordering::Relaxed) {
                    (
                        "200 OK",
                        format!(r#"{{"run_id":"{run_id}","staging_dropped":true}}"#),
                    )
                } else {
                    (
                        "500 Internal Server Error",
                        r#"{"error":{"code":"SINK_ENVIRONMENT","message":"暂存表 drop 不掉","run_id":null,"details":{}}}"#.to_owned(),
                    )
                }
            } else {
                (
                    "503 Service Unavailable",
                    r#"{"error":{"code":"BAD_REQUEST","message":"桩只认 /v1/agent/info","run_id":null,"details":{}}}"#.to_owned(),
                )
            };
            respond_and_close(stream, status, &body);
        }
    });
    (url, aborts, switch)
}

fn temp_directory() -> PathBuf {
    let suffix = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "db-qbs-source-api-test-{}-{suffix}",
        std::process::id()
    ));
    fs::create_dir(&path).unwrap();
    path
}

/// 一套进程内的 source：store 全在临时目录里，`Api` 每次现借。
///
/// `Api<'a>` 借着这些 store，所以 rig 不能自己存一个 `Api`（那是自引用）——
/// 每次请求现拼一个，代价是几个指针。
struct Rig {
    directory: PathBuf,
    config: SourceConfig,
    config_path: PathBuf,
    tasks: TaskStore,
    datasources: DatasourceStore,
    agents: Arc<Mutex<AgentStore>>,
    history: HistoryStore,
    run_logs: RunLogStore,
    runs: Arc<Mutex<RunState>>,
    schedule: db_qbs_source::ScheduleRegistry,
    auth: AuthStore,
    email_alerts: EmailAlertStore,
    alert_outbox: AlertOutboxStore,
    clock: Arc<TestClock>,
    mail_transport: Arc<FakeMailTransport>,
    /// 这台 rig 的会话票据。**每条请求默认都带着它**——`/api/*` 现在整片要求登录，
    /// 不带就是 401，而这个文件里的用例问的几乎都不是「没登录会怎样」。
    /// 真要问那一句的用例走 [`Rig::send_anonymous`]。
    session: String,
}

impl Rig {
    /// 默认的假子进程只管睡：它不报任何阶段，于是「还没进入可取消阶段」这条路径是
    /// 确定的，不用跟子进程的日志赛跑。
    fn new() -> Self {
        Self::with_child("sleep 30\n")
    }

    fn with_child(child_body: &str) -> Self {
        let directory = temp_directory();
        let run_executable = directory.join("fake-source-run.sh");
        fs::write(&run_executable, format!("#!/bin/sh\nset -eu\n{child_body}")).unwrap();
        fs::set_permissions(&run_executable, fs::Permissions::from_mode(0o700)).unwrap();

        let config = SourceConfig {
            oracle_connect_string: None,
            oracle_username: None,
            oracle_password: None,
            oracle_client_lib_dir: "/db-qbs-missing-oracle-client".to_owned(),
            sink_base_url: None,
            listen: "127.0.0.1:0".to_owned(),
            data_dir: directory.clone(),
            history_retention_days: 90,
            run_executable,
        };
        let clock = Arc::new(TestClock::new(
            chrono::DateTime::parse_from_rfc3339("2026-08-31T10:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        ));
        let auth = AuthStore::open(&directory).unwrap();
        let session = auth.issue_session("admin", clock.now()).unwrap().token;
        Self {
            config_path: directory.join("source.toml"),
            tasks: TaskStore::open(&directory).unwrap(),
            datasources: DatasourceStore::open(&directory).unwrap(),
            agents: Arc::new(Mutex::new(AgentStore::open(&directory).unwrap())),
            history: HistoryStore::open(&directory).unwrap(),
            run_logs: RunLogStore::open(&directory).unwrap(),
            runs: Arc::new(Mutex::new(RunState::default())),
            schedule: Arc::new(Mutex::new(ScheduleState::default())),
            auth,
            email_alerts: EmailAlertStore::open(&directory).unwrap(),
            alert_outbox: AlertOutboxStore::open(&directory).unwrap(),
            clock,
            mail_transport: Arc::new(FakeMailTransport::default()),
            session,
            config,
            directory,
        }
    }

    fn api(&self) -> Api<'_> {
        self.api_with_describer(OracleRowSource::describe)
    }

    fn api_with_describer(
        &self,
        describe_source: fn(
            &OracleAccess,
            &TaskSpec,
        ) -> Result<Vec<SourceColumn>, SourceReadError>,
    ) -> Api<'_> {
        Api {
            config: &self.config,
            config_path: &self.config_path,
            tasks: &self.tasks,
            datasources: &self.datasources,
            agents: &self.agents,
            history: &self.history,
            run_logs: &self.run_logs,
            runs: &self.runs,
            schedule: &self.schedule,
            auth: &self.auth,
            email_alerts: &self.email_alerts,
            alert_outbox: &self.alert_outbox,
            clock: self.clock.clone(),
            mail_transport: self.mail_transport.clone(),
            describe_source,
        }
    }

    /// 同一份数据目录上的**第二条命**（#272）：进程被杀掉、又被拉起来是什么样，
    /// 这台 rig 就是什么样——磁盘上那几行没走完的历史照旧在，而在飞登记是纯内存的，
    /// 跟着上一条命一起没了，所以它是空的。
    ///
    /// 上一条命的假子进程还在睡，没人管它：真进程里那些孤儿也是这样，
    /// 而这条用例问的是**新进程看见了什么**。
    fn second_life(&self) -> Self {
        let directory = self.directory.clone();
        Self {
            config: self.config.clone(),
            config_path: self.config_path.clone(),
            tasks: TaskStore::open(&directory).unwrap(),
            datasources: DatasourceStore::open(&directory).unwrap(),
            agents: Arc::new(Mutex::new(AgentStore::open(&directory).unwrap())),
            history: HistoryStore::open(&directory).unwrap(),
            run_logs: RunLogStore::open(&directory).unwrap(),
            runs: Arc::new(Mutex::new(RunState::default())),
            schedule: Arc::new(Mutex::new(ScheduleState::default())),
            auth: AuthStore::open(&directory).unwrap(),
            email_alerts: EmailAlertStore::open(&directory).unwrap(),
            alert_outbox: AlertOutboxStore::open(&directory).unwrap(),
            clock: self.clock.clone(),
            mail_transport: self.mail_transport.clone(),
            session: self.session.clone(),
            directory,
        }
    }

    /// 给一条请求挂上这台 rig 的会话 cookie。手搓 `Request` 的用例也得过这一道。
    fn authorized(&self, request: Request) -> Request {
        request.with_header("Cookie", format!("{SESSION_COOKIE}={}", self.session))
    }

    fn send(&self, method: Method, url: &str, body: &str) -> Response {
        self.api()
            .handle(&self.authorized(Request::new(method, url, body.as_bytes().to_vec())))
    }

    /// 不带任何 cookie 的一次请求：问的就是「门外的人看到什么」。
    fn send_anonymous(&self, method: Method, url: &str, body: &str) -> Response {
        self.api()
            .handle(&Request::new(method, url, body.as_bytes().to_vec()))
    }

    fn send_with_session(&self, session: &str, method: Method, url: &str, body: &str) -> Response {
        self.api().handle(
            &Request::new(method, url, body.as_bytes().to_vec())
                .with_header("Cookie", format!("{SESSION_COOKIE}={session}")),
        )
    }

    fn get(&self, url: &str) -> Response {
        self.send(Method::Get, url, "")
    }

    fn post(&self, url: &str, body: &str) -> Response {
        self.send(Method::Post, url, body)
    }

    fn post_with_describer(
        &self,
        url: &str,
        body: &str,
        describe_source: fn(
            &OracleAccess,
            &TaskSpec,
        ) -> Result<Vec<SourceColumn>, SourceReadError>,
    ) -> Response {
        self.api_with_describer(describe_source)
            .handle(&self.authorized(Request::new(
                Method::Post,
                url,
                body.as_bytes().to_vec(),
            )))
    }

    fn put(&self, url: &str, body: &str) -> Response {
        self.send(Method::Put, url, body)
    }

    fn delete(&self, url: &str) -> Response {
        self.send(Method::Delete, url, "")
    }

    fn json(&self, response: &Response) -> Value {
        serde_json::from_slice(&response.body)
            .unwrap_or_else(|_| panic!("响应不是 JSON：{}", response.body_text()))
    }

    /// 注册一台 agent，返回它的 id。
    fn register_agent(&self, name: &str) -> String {
        self.register_agent_at(name, agent_stub_url())
    }

    /// 注册一台**指定地址**的 agent：要对这台桩收到了什么下断言的用例走这条。
    fn register_agent_at(&self, name: &str, base_url: &str) -> String {
        let response = self.post(
            "/api/agents",
            &format!(r#"{{"name":"{name}","base_url":"{base_url}"}}"#),
        );
        assert_eq!(response.status, 201, "{}", response.body_text());
        self.json(&response)["agent_id"].as_str().unwrap().to_owned()
    }

    /// 等到这个任务名下**再没有在飞的运行**。
    ///
    /// 「投影不再是活的」不等于「这个任务可以再跑了」：子进程死掉之后，父进程还要替它
    /// 向目标端补发一次 abort（#269），那一小段里在飞登记仍然在——界面上就是「停止中…」。
    /// 收尾之后紧接着再起一次的用例，等的必须是这一条，不是 `live == false`。
    fn wait_until_idle(&self, task_id: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if !self.runs.lock().unwrap().has_active_run(task_id) {
                return;
            }
            assert!(Instant::now() < deadline, "等在飞运行释放超时");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn create_oracle_datasource(&self, name: &str) -> String {
        let response = self.post(
            "/api/datasources",
            &format!(
                r#"{{"name":"{name}","kind":"oracle","connect_string":"//oracle:1521/XE","username":"source","password":"secret"}}"#
            ),
        );
        assert_eq!(response.status, 201, "{}", response.body_text());
        self.json(&response)["datasource_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn create_mysql_datasource(&self, name: &str, agent_id: &str) -> String {
        let response = self.post("/api/datasources", &mysql_datasource_json(name, agent_id));
        assert_eq!(response.status, 201, "{}", response.body_text());
        self.json(&response)["datasource_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// 两端数据源 + 一台 agent 一次备齐，返回 `(agent_id, source_id, target_id)`。
    fn seed(&self) -> (String, String, String) {
        let agent_id = self.register_agent("目标端");
        let source_id = self.create_oracle_datasource("源库");
        let target_id = self.create_mysql_datasource("目标库", &agent_id);
        (agent_id, source_id, target_id)
    }

    fn create_task(&self, name: &str, target_table: &str, datasources: &(String, String)) -> String {
        let response = self.post("/api/tasks", &task_json(name, target_table, datasources));
        assert_eq!(response.status, 201, "{}", response.body_text());
        self.json(&response)["task_id"].as_str().unwrap().to_owned()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn mysql_datasource_json(name: &str, agent_id: &str) -> String {
    format!(
        r#"{{"name":"{name}","kind":"mysql","agent_id":"{agent_id}","host":"127.0.0.1","port":3306,"username":"sink","password":"change-me","database":"qbs"}}"#
    )
}

fn task_json(name: &str, target_table: &str, datasources: &(String, String)) -> String {
    let (source_datasource_id, target_datasource_id) = datasources;
    format!(
        r#"{{"name":"{name}","source_datasource_id":"{source_datasource_id}","target_datasource_id":"{target_datasource_id}","spec":{{"owner":"APP","table":"HOLDINGS","target_table":"{target_table}","columns":[{{"source":"ID","target":"ID"}},{{"source":"D_BIZ","target":"D_BIZ"}}],"write_mode":"APPEND","schedule_enabled":false,"primary_key":["ID"],"where_clause":"D_BIZ = DATE '2026-08-14'"}}}}"#
    )
}

fn task_json_with_pre_sql(
    name: &str,
    target_table: &str,
    datasources: &(String, String),
    pre_sql: &str,
) -> String {
    let mut task: Value = serde_json::from_str(&task_json(name, target_table, datasources)).unwrap();
    task["spec"]["pre_sql"] = Value::String(pre_sql.to_owned());
    serde_json::to_string(&task).unwrap()
}

fn custom_task_json_with_pre_sql(
    name: &str,
    target_table: &str,
    datasources: &(String, String),
    source_sql: &str,
    pre_sql: &str,
) -> String {
    let mut task: Value =
        serde_json::from_str(&task_json_with_pre_sql(name, target_table, datasources, pre_sql))
            .unwrap();
    task["spec"]["source_sql"] = Value::String(source_sql.to_owned());
    task["spec"].as_object_mut().unwrap().remove("where_clause");
    serde_json::to_string(&task).unwrap()
}

const REPRESENTATIVE_PRE_SQL: &str = "/* exact */\nDELETE FROM `qbs`.`HOLDINGS` WHERE DATE(D_BIZ) < CURRENT_DATE AND ID IN (SELECT ID FROM qbs.STALE_HOLDINGS);";

/// 上面那份规格现算出来的源端 SQL。父子两端算的是同一份，历史里钉的也是它。
const EXPECTED_SOURCE_SQL: &str = "SELECT a.ID AS ID,\n       a.D_BIZ AS D_BIZ\n  FROM APP.HOLDINGS a\n WHERE D_BIZ = DATE '2026-08-14'";

/// 任务定义在线上恰好三样：名字、结构化规格、身份。SQL 不在里面。
const TASK_FIELDS: [&str; 5] = [
    "name",
    "source_datasource_id",
    "spec",
    "target_datasource_id",
    "task_id",
];

fn assert_task_fields(task: &Value) {
    let mut fields: Vec<_> = task
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    assert_eq!(fields, TASK_FIELDS);
}

fn wait_for_json(rig: &Rig, path: &str, predicate: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = rig.get(path);
        if response.status == 200 {
            let body = rig.json(&response);
            if predicate(&body) {
                return body;
            }
        }
        assert!(Instant::now() < deadline, "等 {path} 超时");
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_empty_directory(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if fs::read_dir(path).unwrap().next().is_none() {
            return;
        }
        assert!(Instant::now() < deadline, "等清扫超时");
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_file_text(path: &Path, predicate: impl Fn(&str) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(text) = fs::read_to_string(path) {
            if predicate(&text) {
                return;
            }
        }
        assert!(Instant::now() < deadline, "等 {path:?} 超时");
        thread::sleep(Duration::from_millis(20));
    }
}

fn directory_entries(directory: &Path) -> Vec<PathBuf> {
    let mut entries = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

/// 通用 404 的那句话。判断「路由压根没匹配上」就靠它。
const UNROUTED: &str = "请求的 source API 资源不存在";

/// 每条路由都到得了它的 handler——**一条都不许漏**。
///
/// 从前这 29 条路由住在 `[[bin]]` 里，`tests/` 够不着，于是其中 5 条从没被任何测试
/// 调用过。这条测试把路由表整张走一遍，并在最后跟 `routes()` 对账：**新加一条路由却
/// 不在这张表里，这里就红**。
#[test]
fn every_route_reaches_its_handler() {
    let rig = Rig::new();
    let agent_id = rig.register_agent("目标端");
    let doomed_agent_id = rig.register_agent("待删 agent");
    let source_id = rig.create_oracle_datasource("源库");
    let target_id = rig.create_mysql_datasource("目标库", &agent_id);
    let doomed_id = rig.create_mysql_datasource("待删数据源", &agent_id);
    let task_id = rig.create_task("搬一次", "HOLDINGS", &(source_id.clone(), target_id.clone()));
    // 删任务那一格另开一个任务：`task_id` 下面要起一次运行、而且这条假子进程会一直睡到
    // 这条用例跑完，删它只会被 #270 那道拒绝挡回来（409），走不到 handler 的 200 分支。
    let doomed_task_id =
        rig.create_task("待删任务", "HOLDINGS_DOOMED", &(source_id.clone(), target_id.clone()));

    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(started.status, 202, "{}", started.body_text());
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        rig.put(
            "/api/email-alert-settings",
            &email_alert_settings_json(true, "route-test-secret", 0),
        )
        .status,
        200
    );
    let now = rig.clock.now();
    let mut failed = RunHistory::accepted("route-alert", "route-task", "SELECT 1", now);
    failed.outcome = Some("FAILED".to_owned());
    failed.finished_at = Some(now.to_rfc3339());
    failed.failure_kind = Some("NETWORK".to_owned());
    rig.history.finalize(&failed, now, 90).unwrap();
    let pending_delivery_id = rig
        .alert_outbox
        .delivery_history(Some("route-alert"))
        .unwrap()[0]
        .delivery_id
        .clone();

    // (方法, 路由样式, 实打实的 URL, 请求体, 期望状态码)
    let checks: Vec<(Method, &str, String, String, u16)> = vec![
        (Method::Post, "/api/columns", "/api/columns".into(), "{}".into(), 400),
        (Method::Post, "/api/builder/tables", "/api/builder/tables".into(), "{}".into(), 400),
        (
            Method::Post,
            "/api/builder/dblinks",
            "/api/builder/dblinks".into(),
            r#"{"datasource_id":"no-such-datasource"}"#.into(),
            400,
        ),
        (Method::Post, "/api/builder/sql-columns", "/api/builder/sql-columns".into(), "{}".into(), 400),
        (
            Method::Post,
            "/api/builder/columns",
            "/api/builder/columns".into(),
            r#"{"datasource_id":"no-such-datasource","dblink":null,"owner":"","table":""}"#.into(),
            400,
        ),
        (
            Method::Post,
            "/api/builder/sql",
            "/api/builder/sql".into(),
            r#"{"datasource_id":"x","source_sql":"select 1 from dual"}"#.into(),
            400,
        ),
        (
            Method::Post,
            "/api/builder/preview",
            "/api/builder/preview".into(),
            r#"{"source_datasource_id":"missing","spec":{},"limit":10}"#.into(),
            400,
        ),
        (
            Method::Post,
            "/api/builder/schedule",
            "/api/builder/schedule".into(),
            r#"{"cron":"0 2 * * *"}"#.into(),
            200,
        ),
        (
            Method::Get,
            "/api/schedule",
            "/api/schedule".into(),
            String::new(),
            200,
        ),
        (Method::Get, "/api/agents", "/api/agents".into(), String::new(), 200),
        (Method::Post, "/api/agents", "/api/agents".into(), "{}".into(), 400),
        (
            Method::Post,
            "/api/agents/{}/probe",
            format!("/api/agents/{agent_id}/probe"),
            String::new(),
            200,
        ),
        (
            Method::Put,
            "/api/agents/{}",
            format!("/api/agents/{agent_id}"),
            format!(r#"{{"name":"改过名的目标端","base_url":"{}"}}"#, agent_stub_url()),
            200,
        ),
        (
            Method::Delete,
            "/api/agents/{}",
            format!("/api/agents/{doomed_agent_id}"),
            String::new(),
            200,
        ),
        (Method::Get, "/api/datasources", "/api/datasources".into(), String::new(), 200),
        (Method::Post, "/api/datasources", "/api/datasources".into(), "{}".into(), 400),
        (
            Method::Post,
            "/api/datasources/test-connection",
            "/api/datasources/test-connection".into(),
            "{}".into(),
            400,
        ),
        (
            Method::Post,
            "/api/datasources/{}/test-connection",
            format!("/api/datasources/{target_id}/test-connection"),
            String::new(),
            502,
        ),
        (
            Method::Get,
            "/api/datasources/{}",
            format!("/api/datasources/{target_id}"),
            String::new(),
            200,
        ),
        (
            Method::Put,
            "/api/datasources/{}",
            format!("/api/datasources/{target_id}"),
            mysql_datasource_json("目标库（改）", &agent_id),
            200,
        ),
        (
            Method::Delete,
            "/api/datasources/{}",
            format!("/api/datasources/{doomed_id}"),
            String::new(),
            200,
        ),
        (
            Method::Post,
            "/api/target/tables",
            "/api/target/tables".into(),
            format!(r#"{{"datasource_id":"{target_id}"}}"#),
            502,
        ),
        (
            Method::Post,
            "/api/target/columns",
            "/api/target/columns".into(),
            format!(r#"{{"datasource_id":"{target_id}","target_table":"HOLDINGS"}}"#),
            502,
        ),
        (
            Method::Post,
            "/api/target/check",
            "/api/target/check".into(),
            format!(r#"{{"source_datasource_id":"{source_id}","target_datasource_id":"{target_id}","target_table":"HOLDINGS","spec":{{"owner":"APP","table":"HOLDINGS","target_table":"HOLDINGS","columns":[{{"source":"ID","target":"ID"}}],"write_mode":"APPEND","schedule_enabled":false,"primary_key":["ID"]}}}}"#),
            502,
        ),
        (
            Method::Post,
            "/api/runs",
            "/api/runs".into(),
            format!(r#"{{"task_id":"{task_id}"}}"#),
            409,
        ),
        (
            Method::Post,
            "/api/runs/{}/cancel",
            format!("/api/runs/{run_record_id}/cancel"),
            String::new(),
            409,
        ),
        // 这次运行没欠着占用，所以重试释放这条 404——那是它认得这条路的证据（#271）。
        (
            Method::Post,
            "/api/runs/{}/release",
            format!("/api/runs/{run_record_id}/release"),
            String::new(),
            404,
        ),
        (Method::Get, "/api/runs", "/api/runs".into(), String::new(), 200),
        (
            Method::Get,
            "/api/runs/{}",
            format!("/api/runs/{run_record_id}"),
            String::new(),
            200,
        ),
        (
            Method::Get,
            "/api/runs/{}/logs",
            format!("/api/runs/{run_record_id}/logs"),
            String::new(),
            200,
        ),
        (Method::Get, "/api/tasks", "/api/tasks".into(), String::new(), 200),
        (Method::Post, "/api/tasks", "/api/tasks".into(), "{}".into(), 400),
        (
            Method::Get,
            "/api/tasks/{}",
            format!("/api/tasks/{task_id}"),
            String::new(),
            200,
        ),
        (
            Method::Get,
            "/api/tasks/{}/curl",
            format!("/api/tasks/{task_id}/curl"),
            String::new(),
            200,
        ),
        (
            Method::Put,
            "/api/tasks/{}",
            format!("/api/tasks/{task_id}"),
            task_json("改过名的任务", "HOLDINGS", &(source_id.clone(), target_id.clone())),
            200,
        ),
        (
            Method::Delete,
            "/api/tasks/{}",
            format!("/api/tasks/{doomed_task_id}"),
            String::new(),
            200,
        ),
        (
            Method::Put,
            "/api/password",
            "/api/password".into(),
            r#"{"current_password":"admin","new_password":"admin"}"#.into(),
            200,
        ),
        (
            Method::Get,
            "/api/operator-account",
            "/api/operator-account".into(),
            String::new(),
            200,
        ),
        (
            Method::Put,
            "/api/operator-account",
            "/api/operator-account".into(),
            r#"{"enabled":false}"#.into(),
            200,
        ),
        (
            Method::Get,
            "/api/email-alert-settings",
            "/api/email-alert-settings".into(),
            String::new(),
            200,
        ),
        (
            Method::Put,
            "/api/email-alert-settings",
            "/api/email-alert-settings".into(),
            email_alert_settings_json(false, "", 24),
            200,
        ),
        (
            Method::Post,
            "/api/email-alert-settings/test",
            "/api/email-alert-settings/test".into(),
            String::new(),
            200,
        ),
        (
            Method::Get,
            "/api/email-deliveries",
            "/api/email-deliveries".into(),
            String::new(),
            200,
        ),
        (
            Method::Post,
            "/api/email-deliveries/{}/retry",
            format!("/api/email-deliveries/{pending_delivery_id}/retry"),
            String::new(),
            400,
        ),
        // 会话那三条**排在最末，而且退出排最后**：`DELETE /api/session` 会把 rig
        // 自己那张票销掉，排在中间会让它后面每一行都变成 401。
        (Method::Get, "/api/session", "/api/session".into(), String::new(), 200),
        (
            Method::Post,
            "/api/session",
            "/api/session".into(),
            r#"{"username":"admin","password":"admin"}"#.into(),
            200,
        ),
        (Method::Delete, "/api/session", "/api/session".into(), String::new(), 200),
    ];

    for (method, pattern, url, body, expected) in &checks {
        let response = rig.send(*method, url, body);
        assert!(
            !response.body_text().contains(UNROUTED),
            "{method:?} {pattern} 没有匹配上任何路由"
        );
        assert_eq!(
            response.status, *expected,
            "{method:?} {url} 回的是 {}：{}",
            response.status,
            response.body_text()
        );
    }

    let covered: Vec<(Method, &str)> = checks
        .iter()
        .map(|(method, pattern, ..)| (*method, *pattern))
        .collect();
    for route in routes() {
        assert!(
            covered.contains(&(route.method, route.pattern)),
            "路由 {:?} {} 没有测试走过它——每条路由都得在上面那张表里有一行",
            route.method,
            route.pattern
        );
    }
    assert_eq!(covered.len(), routes().len(), "表里有路由表上没有的行");
}

#[test]
fn task_curl_is_complete_server_assembled_and_uses_the_public_request_origin() {
    let rig = Rig::new();
    let task_id = rig.create_task(
        "搬一次",
        "HOLDINGS",
        &("source-id".to_owned(), "target-id".to_owned()),
    );
    let request = rig.authorized(
        Request::new(
            Method::Get,
            &format!("/api/tasks/{task_id}/curl"),
            Vec::new(),
        )
        .with_header("Host", "qbs.example.test:8443")
        .with_header("X-Forwarded-Proto", "https"),
    );

    let response = rig.api().handle(&request);

    assert_eq!(response.status, 200, "{}", response.body_text());
    // **两条命令**：`/api/runs` 现在要会话，所以先登录换 cookie 再发起。
    // 口令是占位符——这个接口不发票据，也从不回读口令。
    assert_eq!(
        rig.json(&response)["command"],
        format!(
            "curl --silent --show-error --cookie-jar '/tmp/db-qbs-session-{task_id}.cookie' --request POST 'https://qbs.example.test:8443/api/session' --header 'Content-Type: application/json' --data '{{\"username\":\"admin\",\"password\":\"改成你的口令\"}}' > /dev/null && curl --cookie '/tmp/db-qbs-session-{task_id}.cookie' --request POST 'https://qbs.example.test:8443/api/runs' --header 'Content-Type: application/json' --data '{{\"task_id\":\"{task_id}\"}}'; rm -f '/tmp/db-qbs-session-{task_id}.cookie'"
        )
    );

    rig.auth.update_operator(true, Some("operator-secret")).unwrap();
    let operator = rig
        .auth
        .issue_session("operator", chrono::Utc::now())
        .unwrap()
        .token;
    let operator_response = rig.api().handle(
        &Request::new(
            Method::Get,
            &format!("/api/tasks/{task_id}/curl"),
            Vec::new(),
        )
        .with_header("Cookie", format!("{SESSION_COOKIE}={operator}")),
    );
    assert_eq!(operator_response.status, 200);
    assert!(
        rig.json(&operator_response)["command"]
            .as_str()
            .unwrap()
            .contains(r#""username":"operator""#)
    );
}

#[test]
fn task_curl_rejects_unknown_tasks_and_untrusted_origin_shapes() {
    let rig = Rig::new();
    assert_eq!(rig.get("/api/tasks/missing/curl").status, 404);

    let task_id = rig.create_task(
        "搬一次",
        "HOLDINGS",
        &("source-id".to_owned(), "target-id".to_owned()),
    );
    let request = rig.authorized(
        Request::new(
            Method::Get,
            &format!("/api/tasks/{task_id}/curl"),
            Vec::new(),
        )
        .with_header("Host", "qbs.example.test/'bad"),
    );
    assert_eq!(rig.api().handle(&request).status, 400);
}

/// 表里的先后**不承重**：字面量样式永远压过带占位的样式。
///
/// 从前这两条要靠 `route_api_request` 里的书写顺序保住，规矩只写在注释里
/// （「带动作的路由必须排在前面」）。现在它是匹配规则本身的性质。
#[test]
fn literal_patterns_win_over_placeholder_patterns() {
    let rig = Rig::new();

    // `test-connection` 从前会被按 id 的那条分支当成一个数据源 id 吃掉，落成 404。
    let draft = rig.post("/api/datasources/test-connection", "{}");
    assert_eq!(draft.status, 400, "{}", draft.body_text());
    assert!(!draft.body_text().contains(UNROUTED));

    // `/api/agents/{}/probe` 同理：从前靠排在按 id 的那条前面才不被吃掉。
    let agent_id = rig.register_agent("目标端");
    let probe = rig.post(&format!("/api/agents/{agent_id}/probe"), "");
    assert_eq!(probe.status, 200, "{}", probe.body_text());
}

/// 路由表里不许有两行同方法同样式——重复的那条永远是死的。
#[test]
fn route_table_declares_each_method_and_pattern_once() {
    let mut seen: Vec<(Method, &str)> = Vec::new();
    for route in routes() {
        let key = (route.method, route.pattern);
        assert!(!seen.contains(&key), "路由重复：{:?} {}", key.0, key.1);
        seen.push(key);
    }
}

/// 一段资源 id 里不许有 `/`，也不许为空——这条规矩从前抄在四个 path parser 里，
/// 现在只有 `match_pattern` 一处。
#[test]
fn resource_ids_are_a_single_path_segment() {
    let rig = Rig::new();
    for path in [
        "/api/tasks/",
        "/api/tasks/a/b",
        "/api/datasources/",
        "/api/agents//probe",
        "/api/runs//cancel",
    ] {
        let response = rig.get(path);
        assert_eq!(response.status, 404, "{path} 不该匹配上任何路由");
    }
}

/// 认不出的方法、认不出的路径，回的是同一句 404。
#[test]
fn unknown_method_and_unknown_path_share_one_404() {
    let rig = Rig::new();
    let unknown_path = rig.get("/api/nope");
    assert_eq!(unknown_path.status, 404);
    assert!(unknown_path.body_text().contains(UNROUTED));

    let wrong_method = rig.put("/api/tasks", "{}");
    assert_eq!(wrong_method.status, 404);
    assert!(wrong_method.body_text().contains(UNROUTED));

    let bare = rig.get("/api");
    assert_eq!(bare.status, 404);
}

/// 非 `/api` 的 GET 走内嵌静态资源；别的一律 404。
///
/// 这条分支从前也只有 spawn 进程才碰得到（`tests/web_assets.rs` 测的是
/// `embedded_web_asset` 函数本身，不过 HTTP）。
#[test]
fn static_assets_are_served_off_the_api_tree() {
    let rig = Rig::new();

    let index = rig.get("/");
    assert_eq!(index.status, 200);
    assert_eq!(index.header("Content-Type"), Some("text/html; charset=utf-8"));
    assert_eq!(index.header("Cache-Control"), Some("no-cache"));
    assert!(index.body.starts_with(b"<!doctype html>"));

    // 前端路由由 index 兜底是**前端**的事，服务端不认这些路径。
    assert_eq!(rig.get("/tasks").status, 404);
    assert_eq!(rig.get("/assets/not-built.js").status, 404);
    // 静态资源只认 GET。
    assert_eq!(rig.post("/", "").status, 404);
}

/// 数据源那条 id 链：取一条、改一条、删一条。三个 handler 从前一条测试都没有。
#[test]
fn datasource_by_id_round_trips() {
    let rig = Rig::new();
    let agent_id = rig.register_agent("目标端");
    let datasource_id = rig.create_mysql_datasource("目标库", &agent_id);

    let fetched = rig.get(&format!("/api/datasources/{datasource_id}"));
    assert_eq!(fetched.status, 200, "{}", fetched.body_text());
    assert_eq!(rig.json(&fetched)["name"], "目标库");
    // 口令连密文都不回。
    assert!(!fetched.body_text().contains("change-me"));

    let updated = rig.put(
        &format!("/api/datasources/{datasource_id}"),
        &mysql_datasource_json("改过名的目标库", &agent_id),
    );
    assert_eq!(updated.status, 200, "{}", updated.body_text());
    assert_eq!(rig.json(&updated)["name"], "改过名的目标库");

    // 绑一台没注册过的 agent，当场拒。
    let refused = rig.put(
        &format!("/api/datasources/{datasource_id}"),
        &mysql_datasource_json("乱绑", "no-such-agent"),
    );
    assert_eq!(refused.status, 400, "{}", refused.body_text());

    assert_eq!(rig.delete(&format!("/api/datasources/{datasource_id}")).status, 200);
    assert_eq!(rig.get(&format!("/api/datasources/{datasource_id}")).status, 404);
    assert_eq!(rig.put(&format!("/api/datasources/{datasource_id}"), &mysql_datasource_json("已删", &agent_id)).status, 404);
    assert_eq!(rig.delete(&format!("/api/datasources/{datasource_id}")).status, 404);
}

/// 还被任务引着的数据源删不掉，措辞里点名是哪几个任务。
#[test]
fn a_datasource_a_task_still_points_at_cannot_be_deleted() {
    let rig = Rig::new();
    let agent_id = rig.register_agent("目标端");
    let source_id = rig.create_oracle_datasource("源库");
    let target_id = rig.create_mysql_datasource("目标库", &agent_id);
    rig.create_task("搬一次", "HOLDINGS", &(source_id, target_id.clone()));

    let refused = rig.delete(&format!("/api/datasources/{target_id}"));
    assert_eq!(refused.status, 409, "{}", refused.body_text());
    assert!(refused.body_text().contains("搬一次"));
}

/// 改一台 agent 的名字和地址。这个 handler 从前一条测试都没有。
#[test]
fn agent_update_repins_identity() {
    let rig = Rig::new();
    let agent_id = rig.register_agent("目标端");

    let renamed = rig.put(
        &format!("/api/agents/{agent_id}"),
        &format!(r#"{{"name":"改过名的目标端","base_url":"{}"}}"#, agent_stub_url()),
    );
    assert_eq!(renamed.status, 200, "{}", renamed.body_text());
    assert_eq!(rig.json(&renamed)["name"], "改过名的目标端");

    // 打不通的地址：当场报 agent 不在线，不落库。
    let unreachable = rig.put(
        &format!("/api/agents/{agent_id}"),
        r#"{"name":"搬走了","base_url":"http://127.0.0.1:1"}"#,
    );
    assert!(
        unreachable.status >= 400,
        "打不通的 agent 不该当成改好了：{}",
        unreachable.body_text()
    );
    let still = rig.get("/api/agents");
    assert!(rig.json(&still)[0]["name"] == "改过名的目标端");

    assert_eq!(rig.put("/api/agents/no-such-agent", &format!(r#"{{"name":"没有","base_url":"{}"}}"#, agent_stub_url())).status, 404);
}

/// 构建器那两条从没被测过的入口：请求体先过一遍，数据源不认就 400，
/// 一步都到不了 Oracle。
#[test]
fn builder_dblinks_and_columns_reject_before_reaching_oracle() {
    let rig = Rig::new();

    let dblinks = rig.post(
        "/api/builder/dblinks",
        r#"{"datasource_id":"no-such-datasource"}"#,
    );
    assert_eq!(dblinks.status, 400, "{}", dblinks.body_text());

    let missing_owner = rig.post(
        "/api/builder/columns",
        r#"{"datasource_id":"no-such-datasource","dblink":null,"owner":" ","table":"HOLDINGS"}"#,
    );
    assert_eq!(missing_owner.status, 400, "{}", missing_owner.body_text());
    assert!(missing_owner.body_text().contains("owner and table are required"));

    let bad_dblink = rig.post(
        "/api/builder/columns",
        r#"{"datasource_id":"no-such-datasource","dblink":"bad link","owner":"APP","table":"HOLDINGS"}"#,
    );
    assert_eq!(bad_dblink.status, 400, "{}", bad_dblink.body_text());
}

#[test]
fn builder_preview_validates_spec_and_limit_before_reaching_oracle() {
    let rig = Rig::new();
    let incomplete = rig.post(
        "/api/builder/preview",
        r#"{"source_datasource_id":"missing","spec":{"owner":"","table":"","target_table":"","write_mode":"APPEND","schedule_enabled":false,"primary_key":[],"columns":[]},"limit":10}"#,
    );
    assert_eq!(incomplete.status, 400);
    assert!(incomplete.body_text().contains("owner"));

    let invalid_sql = rig.post(
        "/api/builder/preview",
        r#"{"source_datasource_id":"missing","spec":{"source_sql":"DELETE FROM APP.T","owner":"","table":"","target_table":"T","write_mode":"APPEND","schedule_enabled":false,"primary_key":["ID"],"columns":[{"source":"ID","target":"ID"}]},"limit":10}"#,
    );
    assert_eq!(invalid_sql.status, 400);
    assert!(invalid_sql.body_text().contains("SELECT"));

    let zero = rig.post(
        "/api/builder/preview",
        r#"{"source_datasource_id":"missing","spec":{"source_sql":"SELECT ID FROM APP.T","owner":"","table":"","target_table":"T","write_mode":"APPEND","schedule_enabled":false,"primary_key":["ID"],"columns":[{"source":"ID","target":"ID"}]},"limit":0}"#,
    );
    assert_eq!(zero.status, 400);
    assert!(zero.body_text().contains("limit 必须大于 0"));

    let custom_sql = rig.post(
        "/api/builder/preview",
        r#"{"source_datasource_id":"missing","spec":{"source_sql":"SELECT ID FROM APP.T","owner":"","table":"","target_table":"T","write_mode":"APPEND","schedule_enabled":false,"primary_key":["ID"],"columns":[{"source":"ID","target":"ID"}]},"limit":1000}"#,
    );
    assert_eq!(custom_sql.status, 400);
    assert!(custom_sql.body_text().contains("数据源 missing 不存在"));
}

/// 请求体超过 1 MiB 时的那句话，判定只有一处——`/api/columns` 也归它管，
/// 那里从前自己重做了一遍读 body，于是这条上限对它形同不存在（#199）。
#[test]
fn oversized_request_bodies_are_refused() {
    let rig = Rig::new();
    let huge = format!(r#"{{"name":"{}"}}"#, "x".repeat(1024 * 1024 + 16));
    for path in ["/api/tasks", "/api/columns"] {
        let response = rig.post(path, &huge);
        assert_eq!(response.status, 400, "{path}: {}", response.body_text());
        assert!(
            response.body_text().contains("请求体超过 1 MiB"),
            "{path}: {}",
            response.body_text()
        );
    }
}

/// 任务定义的 CRUD：线上恰好三样，口令一个字节都不出现。
///
/// （「重启之后还在」那一半留在 `source_skeleton.rs` 的哨兵里——那要一个真进程。）
/// 改名不追写历史：运行记录上的名字是开跑那一刻的快照（#259）。
///
/// 任务名只是展示标签，向导里随时可以改；一次运行认领的是 `task_id`。名字若在展示时
/// 回头到任务上现取，改一次名就会把**过去每一次**运行都改成新名字，而那些运行当时
/// 并不叫这个名字——历史于是不再是历史。
#[test]
fn renaming_a_task_leaves_earlier_runs_carrying_the_old_name() {
    let rig = Rig::new();
    let (_agent_id, source_id, target_id) = rig.seed();
    let datasources = (source_id, target_id);
    let task_id = rig.create_task("持仓明细", "HOLDINGS", &datasources);

    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(started.status, 202, "{}", started.body_text());
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let detail = rig.json(&rig.get(&format!("/api/runs/{run_record_id}")));
    assert_eq!(detail["task_name"], "持仓明细");

    let renamed = rig.put(
        &format!("/api/tasks/{task_id}"),
        &task_json("持仓明细（日更）", "HOLDINGS", &datasources),
    );
    assert_eq!(renamed.status, 200, "{}", renamed.body_text());
    assert_eq!(rig.json(&renamed)["name"], "持仓明细（日更）");

    let listed = rig.json(&rig.get(&format!("/api/runs?task_id={task_id}")));
    assert_eq!(listed[0]["task_id"], task_id);
    assert_eq!(listed[0]["task_name"], "持仓明细");
    let detail = rig.json(&rig.get(&format!("/api/runs/{run_record_id}")));
    assert_eq!(detail["task_name"], "持仓明细");
}

#[test]
fn the_in_process_api_uses_the_rig_clock_when_accepting_a_run() {
    let rig = Rig::new();
    let (_agent_id, source_id, target_id) = rig.seed();
    let task_id = rig.create_task("clocked", "CLOCKED", &(source_id, target_id));
    let now = chrono::DateTime::parse_from_rfc3339("2026-08-31T10:05:06.789Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    rig.clock.set(now);

    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(started.status, 202, "{}", started.body_text());
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let detail = rig.json(&rig.get(&format!("/api/runs/{run_record_id}")));
    assert_eq!(detail["started_at"], "2026-08-31T10:05:06.789Z");
}

#[test]
fn task_crud_persists_stable_identity_without_exposing_credentials() {
    let rig = Rig::new();
    let (_agent_id, source_id, target_id) = rig.seed();
    let datasources = (source_id, target_id);

    let created = rig.post("/api/tasks", &task_json("持仓明细", "HOLDINGS", &datasources));
    assert_eq!(created.status, 201, "{}", created.body_text());
    let created = rig.json(&created);
    assert_task_fields(&created);
    let task_id = created["task_id"].as_str().unwrap().to_owned();
    assert!(!task_id.is_empty());

    let listed = rig.get("/api/tasks");
    assert_eq!(listed.status, 200);
    assert_eq!(rig.json(&listed), serde_json::json!([created]));

    let detail = rig.get(&format!("/api/tasks/{task_id}"));
    assert_eq!(detail.status, 200);
    assert_eq!(rig.json(&detail), created);

    let updated = rig.put(
        &format!("/api/tasks/{task_id}"),
        &task_json("持仓日明细", "HOLDINGS_DAILY", &datasources),
    );
    assert_eq!(updated.status, 200, "{}", updated.body_text());
    let updated = rig.json(&updated);
    assert_task_fields(&updated);
    assert_eq!(updated["task_id"], task_id);
    assert_eq!(updated["name"], "持仓日明细");
    assert_eq!(updated["spec"]["target_table"], "HOLDINGS_DAILY");

    let no_config_endpoint = rig.get("/api/config");
    assert_eq!(no_config_endpoint.status, 404);
    for response in [&listed, &no_config_endpoint] {
        assert!(!response.body_text().contains("secret"));
        assert!(!response.body_text().contains("oracle_password"));
    }

    let deleted = rig.delete(&format!("/api/tasks/{task_id}"));
    assert_eq!(deleted.status, 200, "{}", deleted.body_text());
    assert_eq!(rig.json(&deleted), updated);
    assert_eq!(rig.get(&format!("/api/tasks/{task_id}")).status, 404);
    assert_eq!(rig.json(&rig.get("/api/tasks")), serde_json::json!([]));
}

#[test]
fn table_task_pre_sql_is_validated_persisted_and_updated_against_the_bound_target() {
    let rig = Rig::new();
    let (_agent_id, source_id, target_id) = rig.seed();
    let datasources = (source_id, target_id);
    let original = REPRESENTATIVE_PRE_SQL;

    let created = rig.post(
        "/api/tasks",
        &task_json_with_pre_sql("持仓清理", "HOLDINGS", &datasources, original),
    );
    assert_eq!(created.status, 201, "{}", created.body_text());
    let created = rig.json(&created);
    let task_id = created["task_id"].as_str().unwrap();
    assert_eq!(created["spec"]["pre_sql"], original);
    assert_eq!(
        rig.json(&rig.get(&format!("/api/tasks/{task_id}")))["spec"]["pre_sql"],
        original
    );

    let updated_sql = "DELETE FROM HOLDINGS WHERE D_BIZ = CURRENT_DATE";
    let updated = rig.put(
        &format!("/api/tasks/{task_id}"),
        &task_json_with_pre_sql("持仓清理", "HOLDINGS", &datasources, updated_sql),
    );
    assert_eq!(updated.status, 200, "{}", updated.body_text());
    assert_eq!(rig.json(&updated)["spec"]["pre_sql"], updated_sql);

    let wrong_target = rig.post(
        "/api/tasks",
        &task_json_with_pre_sql(
            "越界清理",
            "HOLDINGS",
            &datasources,
            "DELETE FROM other.HOLDINGS WHERE ID = 1",
        ),
    );
    assert_eq!(wrong_target.status, 400, "{}", wrong_target.body_text());
    assert!(wrong_target.body_text().contains("当前任务的目标表"));
}

#[test]
fn custom_source_task_preserves_pre_sql_through_save_and_manual_run() {
    let rig = Rig::new();
    let (_agent_id, source_id, target_id) = rig.seed();
    let custom_source = "SELECT ID, D_BIZ FROM APP.HOLDINGS WHERE STATUS = 'READY'";
    let created = rig.post(
        "/api/tasks",
        &custom_task_json_with_pre_sql(
            "自定义源清理",
            "HOLDINGS",
            &(source_id, target_id),
            custom_source,
            REPRESENTATIVE_PRE_SQL,
        ),
    );
    assert_eq!(created.status, 201, "{}", created.body_text());
    let created = rig.json(&created);
    let task_id = created["task_id"].as_str().unwrap();
    assert_eq!(created["spec"]["source_sql"], custom_source);
    assert_eq!(created["spec"]["pre_sql"], REPRESENTATIVE_PRE_SQL);
    assert_eq!(
        rig.json(&rig.get(&format!("/api/tasks/{task_id}")))["spec"]["pre_sql"],
        REPRESENTATIVE_PRE_SQL
    );

    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(started.status, 202, "{}", started.body_text());
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let accepted = rig.json(&rig.get(&format!("/api/runs/{run_record_id}")));
    assert_eq!(accepted["trigger"], "MANUAL");
    assert_eq!(
        accepted["evidence"]["parameters"]["pre_sql"],
        REPRESENTATIVE_PRE_SQL
    );
    assert!(accepted["source_sql"]
        .as_str()
        .is_some_and(|sql| sql.contains(custom_source)));

    let task_files = directory_entries(&rig.directory.join("run-tasks"));
    assert_eq!(task_files.len(), 1);
    let materialized = load_task_config(&task_files[0]).unwrap();
    assert_eq!(
        materialized.spec.pre_sql.as_deref(),
        Some(REPRESENTATIVE_PRE_SQL)
    );
    assert_eq!(materialized.spec.source_sql.as_deref(), Some(custom_source));
}

#[test]
fn task_writes_reject_client_identity_and_incomplete_definitions() {
    let rig = Rig::new();
    let (_agent_id, source_id, target_id) = rig.seed();
    let datasources = (source_id, target_id);

    let client_identity = rig.post(
        "/api/tasks",
        &format!(
            r#"{{"task_id":"chosen-by-client",{}"#,
            &task_json("持仓明细", "HOLDINGS", &datasources)[1..]
        ),
    );
    assert_eq!(client_identity.status, 400, "{}", client_identity.body_text());

    let missing_name = rig.post(
        "/api/tasks",
        r#"{"spec":{"owner":"APP","table":"HOLDINGS","target_table":"HOLDINGS","columns":[{"source":"ID","target":"ID"}],"write_mode":"APPEND","schedule_enabled":false,"primary_key":["ID"]}}"#,
    );
    assert_eq!(missing_name.status, 400, "{}", missing_name.body_text());
    assert_eq!(rig.json(&rig.get("/api/tasks")), serde_json::json!([]));
}

/// 调度这一半（#265）：两个字段存得下读得回，无效表达式在**保存时**被拒且理由能读，
/// 「下次触发」读数拿的是服务器本地时区。
#[test]
fn scheduling_fields_round_trip_and_a_bad_cron_is_refused_at_save_time() {
    let rig = Rig::new();
    let (_agent_id, source_id, target_id) = rig.seed();

    let body = |cron: &str, enabled: bool| {
        format!(
            r#"{{"name":"每日两点","source_datasource_id":"{source_id}","target_datasource_id":"{target_id}","spec":{{"owner":"APP","table":"HOLDINGS","target_table":"HOLDINGS","columns":[{{"source":"ID","target":"ID"}}],"write_mode":"APPEND","schedule_cron":"{cron}","schedule_enabled":{enabled},"primary_key":["ID"]}}}}"#
        )
    };

    // 无效的表达式在保存这一刻就被拒，理由是解析器那句原话——不是「参数错误」四个字。
    let refused = rig.post("/api/tasks", &body("0 25 * * *", true));
    assert_eq!(refused.status, 400, "{}", refused.body_text());
    assert_eq!(
        rig.json(&refused)["error"]["message"],
        "小时字段的 25 超出取值范围 0-23"
    );
    assert_eq!(rig.json(&rig.get("/api/tasks")), serde_json::json!([]));

    // 开着开关却没有表达式，同样在保存时被拒。
    let contradiction = rig.post(
        "/api/tasks",
        &format!(
            r#"{{"name":"没写表达式","source_datasource_id":"{source_id}","target_datasource_id":"{target_id}","spec":{{"owner":"APP","table":"HOLDINGS","target_table":"HOLDINGS","columns":[{{"source":"ID","target":"ID"}}],"write_mode":"APPEND","schedule_enabled":true,"primary_key":["ID"]}}}}"#
        ),
    );
    assert_eq!(contradiction.status, 400, "{}", contradiction.body_text());
    assert_eq!(
        rig.json(&contradiction)["error"]["message"],
        "启用了周期调度就必须写一条 cron 表达式"
    );

    // 合法的存得下，读回来是**原文**。
    let created = rig.post("/api/tasks", &body("0 2 * * *", true));
    assert_eq!(created.status, 201, "{}", created.body_text());
    let task_id = rig.json(&created)["task_id"].as_str().unwrap().to_owned();
    let read_back = rig.json(&rig.get(&format!("/api/tasks/{task_id}")));
    assert_eq!(read_back["spec"]["schedule_cron"], "0 2 * * *");
    assert_eq!(read_back["spec"]["schedule_enabled"], true);
}

/// 「下次触发」读数（#265）：时区是**服务器本地时区**并且明写出来，触发时刻按它算。
///
/// 这个端点是解析器的端到端出口——界面上那句「下次 2026-08-29 02:00」就是它答的。
#[test]
fn the_schedule_preview_states_the_server_timezone_and_the_next_fire_times() {
    let rig = Rig::new();

    let answer = rig.json(&rig.post("/api/builder/schedule", r#"{"cron":"*/15 * * * *"}"#));
    let times = answer["next_fire_times"].as_array().unwrap();
    assert_eq!(times.len(), 5, "{answer}");
    for time in times {
        let text = time.as_str().unwrap();
        assert_eq!(text.len(), 16, "呈现格式是 YYYY-MM-DD HH:MM：{text}");
        let minute: u32 = text[14..].parse().unwrap();
        assert_eq!(minute % 15, 0, "*/15 只落在四个一刻钟上：{text}");
    }
    // 时区不是可选的装饰：不写出来，「凌晨两点」就没人能对账。
    let local = chrono::Local::now();
    assert_eq!(answer["utc_offset"], local.format("%:z").to_string());
    assert_eq!(answer["timezone"], local.format("%Z").to_string());

    // 还没写表达式也要答得出时区——界面刚打开的那一刻正是最需要它的时候。
    let empty = rig.json(&rig.post("/api/builder/schedule", r#"{"cron":null}"#));
    assert_eq!(empty["next_fire_times"], serde_json::json!([]));
    assert_eq!(empty["utc_offset"], local.format("%:z").to_string());

    // 表达式不合法就是 400，理由与保存被拒时一字不差。
    let refused = rig.post("/api/builder/schedule", r#"{"cron":"5/10 * * * *"}"#);
    assert_eq!(refused.status, 400, "{}", refused.body_text());
    assert_eq!(
        rig.json(&refused)["error"]["message"],
        "分钟字段的步长只能跟在 * 或 a-b 后面：5/10"
    );
}

/// 发起一次运行：临时任务文件、子进程参数、活投影、终态历史，一条链走到底。
#[test]
fn run_launch_materializes_task_and_aggregates_child_output_until_exit() {
    let directory = temp_directory();
    let release = directory.join("release-child");
    let invocation = directory.join("child-args");
    let rig = Rig::with_child(&format!(
        r#"printf '%s\n' "$@" > '{}'
printf '%s\n' '{{"ts":"2026-08-15T10:00:00.000Z","level":"info","event":"source_started","run_id":null,"task":null,"message":"started"}}'
printf '%s\n' '{{"ts":"2026-08-15T10:00:01.000Z","level":"info","event":"stage_changed","run_id":"run-7","task":null,"stage":"PREPARING","message":"preparing"}}'
printf '%s\n' '{{"ts":"2026-08-15T10:00:02.000Z","level":"info","event":"run_opened","run_id":"run-7","task":null,"staging_table":"STG_7","columns_checked":2,"message":"opened"}}'
printf '%s\n' '{{"ts":"2026-08-15T10:00:03.000Z","level":"info","event":"stage_changed","run_id":"run-7","task":null,"stage":"STREAMING","message":"streaming"}}'
printf '%s\n' '{{"ts":"2026-08-15T10:00:04.000Z","level":"info","event":"batch_pushed","run_id":"run-7","task":null,"seq":1,"rows":3,"source_rows":3,"bytes":100,"written":3,"ms":10}}'
printf '%s\n' '{{"ts":"2026-08-15T10:00:05.000Z","level":"info","event":"batch_pushed","run_id":"run-7","task":null,"seq":2,"rows":4,"source_rows":7,"bytes":120,"written":4,"ms":12}}'
while [ ! -f '{}' ]; do sleep 0.02; done
printf '%s\n' '{{"ts":"2026-08-15T10:00:06.000Z","level":"info","event":"stage_changed","run_id":"run-7","task":null,"stage":"COMMITTING","message":"committing"}}'
printf '%s\n' '{{"ts":"2026-08-15T10:00:07.000Z","level":"info","event":"run_finished","run_id":"run-7","task":null,"terminal":"SUCCEEDED","stage":"SUCCEEDED","message":"done","source_code":null,"sink_code":null,"column":null,"value":null,"source_rows":7,"source_batches":2,"staged_rows":7,"received_batches":2,"sink_reported_rows":7,"purged_rows":1,"fetch_ms":4,"push_ms":22,"commit_ms":6,"count_ms":2,"cursor_ms":1}}'
"#,
        invocation.display(),
        release.display(),
    ));
    let (_agent_id, source_id, target_id) = rig.seed();
    let task_id = rig.create_task("holdings", "HOLDINGS", &(source_id, target_id));

    let audit = rusqlite::Connection::open(rig.directory.join("db-qbs.sqlite3")).unwrap();
    audit
        .execute_batch(
            "CREATE TABLE history_write_audit (operation TEXT NOT NULL);
             CREATE TRIGGER audit_history_insert AFTER INSERT ON run_history
             BEGIN INSERT INTO history_write_audit VALUES ('INSERT'); END;
             CREATE TRIGGER audit_history_update AFTER UPDATE ON run_history
             BEGIN INSERT INTO history_write_audit VALUES ('UPDATE'); END;",
        )
        .unwrap();
    drop(audit);

    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(started.status, 202, "{}", started.body_text());
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let mut detail = wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
        body["rows_pushed"] == 7
    });
    assert!(detail["evidence"].is_object());
    detail.as_object_mut().unwrap().remove("evidence");
    // 发起时刻是跑的时候生成的，断不出字面量，只断它在且是个时间戳。
    assert!(detail["started_at"].as_str().is_some_and(|at| at.ends_with('Z')));
    detail.as_object_mut().unwrap().remove("started_at");
    assert_eq!(
        detail,
        serde_json::json!({
            "run_record_id": run_record_id,
            "run_id": "run-7",
            // 开跑那一刻的任务名快照，跟着 live 投影一起出来（#259）。
            "task_name": "holdings",
            "trigger": "MANUAL",
            "source_sql": EXPECTED_SOURCE_SQL,
            "staging_table": "STG_7",
            "stage": "STREAMING",
            "total_rows": null,
            "precount_ms": null,
            "seq": 2,
            "rows_pushed": 7,
            "bytes": 220,
            "ms": 22,
            "last_ts": "2026-08-15T10:00:05.000Z",
            "live": true,
            // 在飞的这一行也报占用（#271）：没人按停止，所以两格都是空的。
            "target_hold": null,
            "target_hold_message": null,
        })
    );
    let live_list = rig.json(&rig.get(&format!("/api/runs?task_id={task_id}")));
    assert_eq!(live_list.as_array().unwrap().len(), 1);
    assert_eq!(live_list[0]["run_record_id"], run_record_id);
    assert_eq!(live_list[0]["rows_pushed"], 7);
    assert_eq!(live_list[0]["finished_at"], Value::Null);

    let task_files = directory_entries(&rig.directory.join("run-tasks"));
    assert_eq!(task_files.len(), 1);
    assert!(task_files[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains(&run_record_id));
    assert_eq!(
        fs::metadata(&task_files[0]).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let task_toml = fs::read_to_string(&task_files[0]).unwrap();
    for field in [
        "owner",
        "table",
        "columns",
        "write_mode",
        "schedule_enabled",
        "primary_key",
        "where_clause",
    ] {
        assert!(task_toml.contains(field), "{task_toml}");
    }
    // SQL 不落进任务文件：子进程从同一份规格现算。
    assert!(!task_toml.contains("SELECT"), "{task_toml}");
    // 任务身份仍不落进去——子进程按规格干活，不需要知道自己是哪条任务。
    for absent in ["holdings", "task_id"] {
        assert!(!task_toml.contains(absent), "{task_toml}");
    }
    // 两端凭据与目标端 agent 都落进去：编排进程解一次，子进程不碰数据源库、也不碰密钥。
    for present in [
        "[oracle]",
        "[target]",
        "client_lib_dir",
        "[agent]",
        "instance_id",
    ] {
        assert!(task_toml.contains(present), "{task_toml}");
    }

    let args = fs::read_to_string(invocation).unwrap();
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        [
            "--config",
            rig.config_path.to_str().unwrap(),
            "--task",
            task_files[0].to_str().unwrap(),
        ]
    );

    fs::write(release, "").unwrap();
    let history = wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
        body["live"] == false
    });
    assert_eq!(history["run_record_id"], run_record_id);
    assert_eq!(history["run_id"], "run-7");
    assert_eq!(history["task_id"], task_id);
    assert_eq!(history["source_sql"], EXPECTED_SOURCE_SQL);
    assert_eq!(history["outcome"], "SUCCEEDED");
    assert_eq!(history["target_table_effect"], "SWAPPED");
    assert_eq!(history["source_rows"], 7);
    assert_eq!(history["source_batches"], 2);
    assert_eq!(history["rows_pushed"], 7);
    assert_eq!(history["seq"], 2);
    assert_eq!(history["bytes"], 220);
    assert_eq!(history["ms"], 22);
    assert_eq!(history["source_code"], Value::Null);
    assert_eq!(history["sink_code"], Value::Null);

    let by_task = rig.json(&rig.get(&format!("/api/runs?task_id={task_id}")));
    assert_eq!(by_task.as_array().unwrap().len(), 1);
    assert_eq!(by_task[0]["run_record_id"], run_record_id);
    // 筛选只剩任务这一维。
    let no_match = rig.json(&rig.get("/api/runs?task_id=nonexistent"));
    assert_eq!(no_match, serde_json::json!([]));
    let audit = rusqlite::Connection::open(rig.directory.join("db-qbs.sqlite3")).unwrap();
    let history_writes: u64 = audit
        .query_row("SELECT COUNT(*) FROM history_write_audit", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(history_writes, 5);
    drop(audit);
    wait_for_empty_directory(&rig.directory.join("run-tasks"));

    let _ = fs::remove_dir_all(directory);
}

#[test]
fn silent_child_disappearance_evicts_projection_and_removes_task_file() {
    let rig = Rig::with_child("exit 1\n");
    let (_agent_id, source_id, target_id) = rig.seed();
    let task_id = rig.create_task("holdings", "HOLDINGS", &(source_id, target_id));

    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(started.status, 202, "{}", started.body_text());
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let history = wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
        body["live"] == false
    });
    assert_eq!(history["outcome"], "FAILED");
    assert_eq!(history["unknown_reason"], "PROCESS_DISAPPEARED");
    assert_eq!(history["message"], "进程消失，无终态日志");
    assert_eq!(history["source_code"], Value::Null);
    assert_eq!(history["sink_code"], Value::Null);
    assert_eq!(history["target_table_effect"], Value::Null);
    wait_for_empty_directory(&rig.directory.join("run-tasks"));
}

/// 子进程卡在半路：投影仍是「活的」，而「已受理」不等于 PREPARING。
#[test]
fn child_hanging_mid_run_remains_live_and_accepted_is_not_preparing() {
    let directory = temp_directory();
    let emit = directory.join("emit-lines");
    let release = directory.join("release-child");
    let rig = Rig::with_child(&format!(
        r#"while [ ! -f '{}' ]; do sleep 0.02; done
printf '%s\n' '{{"ts":"2026-08-15T11:00:00.000Z","level":"info","event":"source_started","run_id":null,"task":null,"message":"started"}}'
printf '%s\n' '{{"ts":"2026-08-15T11:00:01.000Z","level":"info","event":"stage_changed","run_id":"run-hanging","task":null,"stage":"PREPARING","message":"preparing"}}'
printf '%s\n' '{{"ts":"2026-08-15T11:00:02.000Z","level":"info","event":"batch_pushed","run_id":"run-hanging","task":null,"seq":1,"rows":5,"source_rows":5,"bytes":64,"written":5,"ms":9}}'
while [ ! -f '{}' ]; do sleep 0.02; done
"#,
        emit.display(),
        release.display(),
    ));
    let (agent_id, source_id, target_id) = rig.seed();
    let pre_sql = "/* snapshot */\nDELETE FROM qbs.HOLDINGS WHERE D_BIZ < CURRENT_DATE;";
    let created = rig.post(
        "/api/tasks",
        &task_json_with_pre_sql(
            "holdings",
            "HOLDINGS",
            &(source_id.clone(), target_id.clone()),
            pre_sql,
        ),
    );
    assert_eq!(created.status, 201, "{}", created.body_text());
    let task_id = rig.json(&created)["task_id"].as_str().unwrap().to_owned();

    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(started.status, 202, "{}", started.body_text());
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let accepted = rig.json(&rig.get(&format!("/api/runs/{run_record_id}")));
    assert_eq!(accepted["stage"], Value::Null);
    assert_eq!(accepted["run_id"], Value::Null);
    assert_eq!(accepted["source_sql"], EXPECTED_SOURCE_SQL);
    assert_eq!(accepted["evidence"]["source"]["datasource_id"], source_id);
    assert_eq!(
        accepted["evidence"]["source"]["connect_string"],
        "//oracle:1521/XE"
    );
    assert_eq!(accepted["evidence"]["source"]["username"], "source");
    assert_eq!(
        accepted["evidence"]["source"]["client_lib_dir"],
        "/db-qbs-missing-oracle-client"
    );
    assert_eq!(accepted["evidence"]["target"]["datasource_id"], target_id);
    assert_eq!(accepted["evidence"]["target"]["host"], "127.0.0.1");
    assert_eq!(accepted["evidence"]["target"]["port"], 3306);
    assert_eq!(accepted["evidence"]["target"]["database"], "qbs");
    assert_eq!(accepted["evidence"]["target"]["username"], "sink");
    assert_eq!(accepted["evidence"]["agent"]["agent_id"], agent_id);
    assert_eq!(accepted["evidence"]["agent"]["name"], "目标端");
    assert_eq!(accepted["evidence"]["agent"]["base_url"], agent_stub_url());
    assert_eq!(accepted["evidence"]["agent"]["instance_id"], "stub-agent");
    assert_eq!(accepted["evidence"]["parameters"]["target_table"], "HOLDINGS");
    assert_eq!(accepted["evidence"]["parameters"]["primary_key"], serde_json::json!(["ID"]));
    assert_eq!(accepted["evidence"]["parameters"]["source_sql"], EXPECTED_SOURCE_SQL);
    assert_eq!(accepted["evidence"]["parameters"]["pre_sql"], pre_sql);
    assert!(!serde_json::to_string(&accepted).unwrap().contains("change-me"));
    assert!(!serde_json::to_string(&accepted).unwrap().contains("secret"));
    assert_eq!(accepted["seq"], 0);
    assert_eq!(accepted["rows_pushed"], 0);
    assert_eq!(accepted["bytes"], 0);
    assert_eq!(accepted["ms"], 0);
    assert_eq!(accepted["last_ts"], Value::Null);
    assert_eq!(accepted["live"], true);

    let changed = rig.put(
        &format!("/api/datasources/{source_id}"),
        r#"{"name":"源库（改）","kind":"oracle","connect_string":"//changed:1521/NEW","username":"changed","password":"new-secret"}"#,
    );
    assert_eq!(changed.status, 200, "{}", changed.body_text());
    let still_original = rig.json(&rig.get(&format!("/api/runs/{run_record_id}")));
    assert_eq!(
        still_original["evidence"]["source"]["connect_string"],
        "//oracle:1521/XE"
    );
    assert_eq!(still_original["evidence"]["source"]["username"], "source");

    fs::write(emit, "").unwrap();
    let partial = wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
        body["rows_pushed"] == 5
    });
    assert_eq!(partial["stage"], "PREPARING");
    assert_eq!(partial["run_id"], "run-hanging");
    assert_eq!(partial["seq"], 1);
    assert_eq!(partial["bytes"], 64);
    assert_eq!(partial["ms"], 9);
    assert_eq!(partial["live"], true);

    thread::sleep(Duration::from_millis(100));
    assert_eq!(rig.get(&format!("/api/runs/{run_record_id}")).status, 200);
    fs::write(release, "").unwrap();
    let history = wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
        body["live"] == false
    });
    assert_eq!(history["unknown_reason"], "PROCESS_DISAPPEARED");

    let _ = fs::remove_dir_all(directory);
}

/// 互斥键就是任务本身：同一个任务起不来第二次，别的任务照样起得来。
#[test]
fn run_launch_rejects_a_second_run_of_the_same_task_until_child_reap() {
    let directory = temp_directory();
    let release = directory.join("release-children");
    let rig = Rig::with_child(&format!(
        "while [ ! -f '{}' ]; do sleep 0.02; done\nexit 1\n",
        release.display()
    ));
    let (_agent_id, source_id, target_id) = rig.seed();
    let datasources = (source_id, target_id);
    let first_task_id = rig.create_task("holdings", "HOLDINGS", &datasources);
    let second_task_id = rig.create_task("holdings-2", "HOLDINGS_2", &datasources);

    let start = |task_id: &str| {
        let response = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
        assert_eq!(response.status, 202, "{}", response.body_text());
        rig.json(&response)["run_record_id"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    let first = start(&first_task_id);
    let duplicate = rig.post("/api/runs", &format!(r#"{{"task_id":"{first_task_id}"}}"#));
    assert_eq!(duplicate.status, 409, "{}", duplicate.body_text());
    assert!(rig.json(&duplicate)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("已有一次运行进行中"));

    let other_task = start(&second_task_id);

    // 老界面（或老脚本）送来的运行参数**当场拒**，不是静默忽略。
    let with_run_params = rig.post(
        "/api/runs",
        &format!(r#"{{"task_id":"{second_task_id}","run_params":{{"d_biz":"2026-08-14"}}}}"#),
    );
    assert_eq!(with_run_params.status, 400, "{}", with_run_params.body_text());

    fs::write(release, "").unwrap();
    for run_record_id in [&first, &other_task] {
        wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
            body["live"] == false
        });
    }
    wait_for_empty_directory(&rig.directory.join("run-tasks"));
    rig.wait_until_idle(&first_task_id);

    let relaunched = start(&first_task_id);
    wait_for_json(&rig, &format!("/api/runs/{relaunched}"), |body| {
        body["live"] == false
    });

    let _ = fs::remove_dir_all(directory);
}

/// 封口点不变量：PREPARING / STREAMING 停得了，COMMITTING 停不了。
#[test]
fn cancel_signals_preparing_and_streaming_but_rejects_committing() {
    let directory = temp_directory();
    let canceled = directory.join("canceled-stages");
    let counter = directory.join("run-counter");
    let release_committing = directory.join("release-committing");
    let rig = Rig::with_child(&format!(
        r#"count=$(cat '{}' 2>/dev/null || echo 0)
count=$((count + 1))
printf '%s' "$count" > '{}'
case "$count" in
  1) stage=PREPARING ;;
  2) stage=STREAMING ;;
  *) stage=COMMITTING ;;
esac
trap 'printf "%s\n" "$stage" >> "{}"; exit 0' TERM
printf '%s\n' "{{\"ts\":\"2026-08-15T13:00:00.000Z\",\"level\":\"info\",\"event\":\"stage_changed\",\"run_id\":\"run-$count\",\"task\":null,\"stage\":\"$stage\"}}"
if [ "$stage" = COMMITTING ]; then
  while [ ! -f '{}' ]; do sleep 0.02; done
else
  while :; do sleep 0.02; done
fi
"#,
        counter.display(),
        counter.display(),
        canceled.display(),
        release_committing.display(),
    ));
    let (_agent_id, source_id, target_id) = rig.seed();
    let task_id = rig.create_task("holdings", "HOLDINGS", &(source_id, target_id));

    let start = |task_id: &str| {
        let response = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
        assert_eq!(response.status, 202, "{}", response.body_text());
        rig.json(&response)["run_record_id"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    for stage in ["PREPARING", "STREAMING"] {
        let run_record_id = start(&task_id);
        wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
            body["stage"] == stage
        });
        let canceled_response = rig.post(&format!("/api/runs/{run_record_id}/cancel"), "");
        assert_eq!(canceled_response.status, 202, "{}", canceled_response.body_text());
        wait_for_file_text(&canceled, |text| text.lines().any(|line| line == stage));
        wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
            body["live"] == false
        });
        // 收尾还要替子进程补发一次 abort，在飞登记要等那一步走完才摘（#269）。
        rig.wait_until_idle(&task_id);
    }

    let committing = start(&task_id);
    wait_for_json(&rig, &format!("/api/runs/{committing}"), |body| {
        body["stage"] == "COMMITTING"
    });
    let rejected_response = rig.post(&format!("/api/runs/{committing}/cancel"), "");
    assert_eq!(rejected_response.status, 409, "{}", rejected_response.body_text());
    let rejected_body = rig.json(&rejected_response);
    assert_eq!(rejected_body["error"]["message"], "已过封口点，停不了");
    assert!(rejected_body.get("code").is_none());
    assert!(rejected_body["error"].get("code").is_none());
    assert!(!fs::read_to_string(&canceled)
        .unwrap()
        .lines()
        .any(|line| line == "COMMITTING"));

    fs::write(release_committing, "").unwrap();
    wait_for_json(&rig, &format!("/api/runs/{committing}"), |body| {
        body["live"] == false
    });

    let _ = fs::remove_dir_all(directory);
}

/// 停止运行的完整一趟（#269）：SIGTERM → 确认子进程退出 → 替它向目标端补发 abort。
///
/// 三件事一起证：顺序是硬的（子进程还活着时**一个字节都没发过去**）、abort 发的是
/// 这一次运行的 run_id、以及占用释放之后同一个任务立刻能再跑。
#[test]
fn stopping_a_run_aborts_on_the_child_s_behalf_only_after_it_has_exited() {
    let directory = temp_directory();
    let stopped = directory.join("child-caught-term");
    let release = directory.join("release-dying-child");
    let rig = Rig::with_child(&format!(
        r#"trap 'printf "%s\n" caught >> "{}"; while [ ! -f "{}" ]; do sleep 0.02; done; exit 0' TERM
printf '%s\n' '{{"ts":"2026-08-15T14:00:00.000Z","level":"info","event":"stage_changed","run_id":"run-stopped","task":null,"stage":"STREAMING"}}'
while :; do sleep 0.02; done
"#,
        stopped.display(),
        release.display(),
    ));
    let (agent_url, aborts) = abort_recording_agent(true);
    let agent_id = rig.register_agent_at("目标端", &agent_url);
    let source_id = rig.create_oracle_datasource("源库");
    let target_id = rig.create_mysql_datasource("目标库", &agent_id);
    let task_id = rig.create_task("holdings", "HOLDINGS", &(source_id, target_id));

    let start = || {
        let response = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
        assert_eq!(response.status, 202, "{}", response.body_text());
        rig.json(&response)["run_record_id"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    let run_record_id = start();
    wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
        body["stage"] == "STREAMING"
    });
    // 还没人按停止：目标表占用这一栏是空的，那一格上写的是「发起运行」。
    let running = rig.json(&rig.get(&format!("/api/runs/{run_record_id}")));
    assert_eq!(running["target_hold"], Value::Null);

    let canceled = rig.post(&format!("/api/runs/{run_record_id}/cancel"), "");
    assert_eq!(canceled.status, 202, "{}", canceled.body_text());

    // 信号到了，子进程**还没死**（它卡在 trap 里）：这一刻暂存表一动都不能动。
    wait_for_file_text(&stopped, |text| text.contains("caught"));
    thread::sleep(Duration::from_millis(100));
    let too_early = aborts.lock().unwrap().clone();
    assert!(too_early.is_empty(), "子进程还活着就发了 abort：{too_early:?}");

    // 这段窗口界面上写的是「停止中…」，而且**发起运行这条路是关着的**（#271）：
    // 占用还在目标端挂着，这时候放一次新运行进去只会撞回一个 `TARGET_TABLE_BUSY`。
    let stopping = rig.json(&rig.get(&format!("/api/runs/{run_record_id}")));
    assert_eq!(stopping["target_hold"], "RELEASING");
    assert_eq!(stopping["live"], true);
    let refused = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(refused.status, 409, "{}", refused.body_text());
    assert_eq!(
        rig.json(&refused)["error"]["message"],
        "该任务正在停止，等目标表占用释放后才能再跑"
    );
    // 清单那一头说的是同一句话：作业中心读的是这条路，不是运行详情那条。
    let listed = rig.json(&rig.get("/api/runs"));
    let row = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["run_record_id"] == run_record_id.as_str())
        .expect("清单里得有这次运行");
    assert_eq!(row["target_hold"], "RELEASING");

    fs::write(&release, "").unwrap();
    rig.wait_until_idle(&task_id);
    assert_eq!(aborts.lock().unwrap().clone(), vec!["run-stopped"]);

    // 占用真的还回来了：那一栏空了，界面这才敢把「发起运行」放出来。
    let released = rig.json(&rig.get(&format!("/api/runs/{run_record_id}")));
    assert_eq!(released["target_hold"], Value::Null);
    assert_eq!(released["target_hold_message"], Value::Null);

    // 主动停止在历史里说得出自己是主动停止的，不再和 OOM、崩溃混作一谈。
    let history = rig.json(&rig.get(&format!("/api/runs/{run_record_id}")));
    assert_eq!(history["live"], false);
    assert_eq!(history["outcome"], "FAILED");
    assert_eq!(history["unknown_reason"], "STOPPED_BY_USER");
    assert_eq!(history["message"], "已由用户停止");
    assert_eq!(history["target_table_effect"], Value::Null);

    // 占用一还，同一张目标表马上能再跑一次。
    let again = start();
    wait_for_json(&rig, &format!("/api/runs/{again}"), |body| {
        body["stage"] == "STREAMING"
    });
    assert_eq!(
        rig.post(&format!("/api/runs/{again}/cancel"), "").status,
        202
    );
    rig.wait_until_idle(&task_id);

    let _ = fs::remove_dir_all(directory);
}

/// 补发的 abort 失败了**不吞**：失败事实落成一行 `abort_failed` 运行日志（#269）。
///
/// 目标表这时仍被占着，怎么把这件事摆到用户面前是后续票的事；这里守的是
/// 「它至少被记下来了」。
#[test]
fn a_failed_abort_after_a_stop_is_written_into_the_run_log() {
    let rig = Rig::with_child(
        r#"trap 'exit 0' TERM
printf '%s\n' '{"ts":"2026-08-15T14:10:00.000Z","level":"info","event":"stage_changed","run_id":"run-unabortable","task":null,"stage":"STREAMING"}'
while :; do sleep 0.02; done
"#,
    );
    let (agent_url, aborts) = abort_recording_agent(false);
    let agent_id = rig.register_agent_at("目标端", &agent_url);
    let source_id = rig.create_oracle_datasource("源库");
    let target_id = rig.create_mysql_datasource("目标库", &agent_id);
    let task_id = rig.create_task("holdings", "HOLDINGS", &(source_id, target_id));

    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(started.status, 202, "{}", started.body_text());
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
        body["stage"] == "STREAMING"
    });
    assert_eq!(
        rig.post(&format!("/api/runs/{run_record_id}/cancel"), "").status,
        202
    );
    rig.wait_until_idle(&task_id);
    assert_eq!(aborts.lock().unwrap().clone(), vec!["run-unabortable"]);

    let logs = wait_for_json(&rig, &format!("/api/runs/{run_record_id}/logs"), |body| {
        body["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line["line"].as_str().unwrap().contains("abort_failed"))
    });
    let failed = logs["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line["line"].as_str().unwrap()).ok())
        .find(|line| line["event"] == "abort_failed")
        .expect("abort 失败要留下一行日志");
    assert_eq!(failed["level"], "warn");
    assert_eq!(failed["run_id"], "run-unabortable");
    assert_eq!(failed["message"], "暂存表 drop 不掉");
    // 停止这件事本身照旧记成「已由用户停止」——abort 成没成功是另一件事。
    let history = rig.json(&rig.get(&format!("/api/runs/{run_record_id}")));
    assert_eq!(history["unknown_reason"], "STOPPED_BY_USER");
}

/// 补发的 abort 没成时，占用留在目标端——界面上那一格就得说「锁未释放」，
/// 并且**发起运行必须是关着的**；点一下重试，成了才放行（#271）。
///
/// 这条用例走的是整条路：失败 → 拦住新运行 → 重试仍失败（照旧拦住、照旧落日志）→
/// 目标端缓过来后重试成功 → 占用还回来 → 同一个任务这才跑得起来。
#[test]
fn a_stuck_target_hold_blocks_new_runs_until_a_retry_releases_it() {
    let rig = Rig::with_child(
        r#"trap 'exit 0' TERM
printf '%s\n' '{"ts":"2026-08-15T14:30:00.000Z","level":"info","event":"stage_changed","run_id":"run-held","task":null,"stage":"STREAMING"}'
while :; do sleep 0.02; done
"#,
    );
    let (agent_url, aborts, succeeds) = switchable_abort_agent(false);
    let agent_id = rig.register_agent_at("目标端", &agent_url);
    let source_id = rig.create_oracle_datasource("源库");
    let target_id = rig.create_mysql_datasource("目标库", &agent_id);
    let task_id = rig.create_task("holdings", "HOLDINGS", &(source_id, target_id));

    let start = || rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    let started = start();
    assert_eq!(started.status, 202, "{}", started.body_text());
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
        body["stage"] == "STREAMING"
    });
    assert_eq!(
        rig.post(&format!("/api/runs/{run_record_id}/cancel"), "")
            .status,
        202
    );
    rig.wait_until_idle(&task_id);

    // 补发那一刀没成：这条运行如实说自己欠着一份占用，连原因的原话一起带出来。
    let held = rig.json(&rig.get(&format!("/api/runs/{run_record_id}")));
    assert_eq!(held["live"], false);
    assert_eq!(held["target_hold"], "HELD");
    assert_eq!(held["target_hold_message"], "暂存表 drop 不掉");
    // 停止本身照旧记成「已由用户停止」——abort 成没成是另一件事（#269）。
    assert_eq!(held["unknown_reason"], "STOPPED_BY_USER");

    // **占用还在的时候，发起运行是关着的**：这是那条铁律的服务端一半。
    let refused = start();
    assert_eq!(refused.status, 409, "{}", refused.body_text());
    assert_eq!(
        rig.json(&refused)["error"]["message"],
        "这张目标表的占用还没释放，先把它释放掉再发起"
    );

    // 重试一次，目标端还是不行：如实回 502，占用照旧挂着，失败照旧落一行日志。
    let retried = rig.post(&format!("/api/runs/{run_record_id}/release"), "");
    assert_eq!(retried.status, 502, "{}", retried.body_text());
    let retried_body = rig.json(&retried);
    assert_eq!(retried_body["error"]["message"], "暂存表 drop 不掉");
    assert_eq!(retried_body["error"]["kind"], "sink");
    assert_eq!(
        rig.json(&rig.get(&format!("/api/runs/{run_record_id}")))["target_hold"],
        "HELD"
    );
    let logs = wait_for_json(&rig, &format!("/api/runs/{run_record_id}/logs"), |body| {
        abort_failures(body) == 2
    });
    assert_eq!(abort_failures(&logs), 2, "重试失败也得留下一行");

    // 目标端缓过来了，人再点一次：这一次占用真的还回来了。
    succeeds.store(true, Ordering::Relaxed);
    let released = rig.post(&format!("/api/runs/{run_record_id}/release"), "");
    assert_eq!(released.status, 200, "{}", released.body_text());
    assert_eq!(rig.json(&released)["message"], "目标表占用已释放");
    let freed = rig.json(&rig.get(&format!("/api/runs/{run_record_id}")));
    assert_eq!(freed["target_hold"], Value::Null);
    assert_eq!(freed["target_hold_message"], Value::Null);
    // 三次都发给了同一次运行：补发一次，重试两次。
    assert_eq!(
        aborts.lock().unwrap().clone(),
        vec!["run-held", "run-held", "run-held"]
    );
    // 释放干净了，同一个任务这才跑得起来。
    let again = start();
    assert_eq!(again.status, 202, "{}", again.body_text());
    let again_id = rig.json(&again)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    // 收摊前先等它报出阶段：没报阶段的 run 停不了（「尚未进入可取消阶段」），
    // 那一句 409 与这条用例要证的事毫无关系。
    wait_for_json(&rig, &format!("/api/runs/{again_id}"), |body| {
        body["stage"] == "STREAMING"
    });
    assert_eq!(
        rig.post(&format!("/api/runs/{again_id}/cancel"), "").status,
        202
    );
    rig.wait_until_idle(&task_id);

    // 已经释放过的运行再点一次重试：404，说的是「没有待释放的占用」，
    // 而不是含混的「找不到」——那两件事在界面上要走不同的路。
    let nothing_to_do = rig.post(&format!("/api/runs/{run_record_id}/release"), "");
    assert_eq!(nothing_to_do.status, 404, "{}", nothing_to_do.body_text());
    assert_eq!(
        rig.json(&nothing_to_do)["error"]["message"],
        "这次运行没有待释放的目标表占用"
    );
}

/// 子进程自己那一刀 abort 没砍下去时，占用同样留在目标端（#271）。
///
/// 这条路径父进程**不补 abort**（子进程已经走完终态，再发只会在已封口的 run 上换回
/// 409），但占用还挂着这件事一模一样——界面照样不许显示成可以重跑，重试那颗照样在。
#[test]
fn a_child_reported_abort_failure_holds_the_target_table_too() {
    let rig = Rig::with_child(
        r#"printf '%s\n' '{"ts":"2026-08-15T15:00:00.000Z","level":"info","event":"stage_changed","run_id":"run-child-abort","task":null,"stage":"STREAMING"}'
printf '%s\n' '{"ts":"2026-08-15T15:00:03.000Z","level":"warn","event":"abort_failed","run_id":"run-child-abort","task":null,"message":"暂存表 drop 不掉"}'
printf '%s\n' '{"ts":"2026-08-15T15:00:04.000Z","level":"error","event":"run_finished","run_id":"run-child-abort","task":null,"terminal":"FAILED","stage":"FAILED","message":"目标端拒绝","failure_kind":"SINK_WRITE","source_code":null,"sink_code":"WRITE_FAILED","column":null,"value":null,"source_rows":1,"source_batches":1,"staged_rows":0,"received_batches":1,"sink_reported_rows":0,"purged_rows":0,"fetch_ms":1,"push_ms":1,"commit_ms":0,"count_ms":0,"cursor_ms":0}'
"#,
    );
    let (agent_url, aborts) = abort_recording_agent(true);
    let agent_id = rig.register_agent_at("目标端", &agent_url);
    let source_id = rig.create_oracle_datasource("源库");
    let target_id = rig.create_mysql_datasource("目标库", &agent_id);
    let task_id = rig.create_task("holdings", "HOLDINGS", &(source_id, target_id));

    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(started.status, 202, "{}", started.body_text());
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    rig.wait_until_idle(&task_id);

    // 父进程一个字节都没往目标端发——它信子进程自己报的终态。
    assert!(
        aborts.lock().unwrap().is_empty(),
        "报了终态的运行不该被父进程补一刀"
    );
    // 但占用还挂着，所以这一行如实说「没释放掉」，发起运行也被拦住。
    let held = rig.json(&rig.get(&format!("/api/runs/{run_record_id}")));
    assert_eq!(held["outcome"], "FAILED");
    assert_eq!(held["target_hold"], "HELD");
    assert_eq!(held["target_hold_message"], "暂存表 drop 不掉");
    assert_eq!(
        rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#))
            .status,
        409
    );

    // 点一下重试，这一次成了：占用还回来，任务重新跑得起来。
    let released = rig.post(&format!("/api/runs/{run_record_id}/release"), "");
    assert_eq!(released.status, 200, "{}", released.body_text());
    assert_eq!(aborts.lock().unwrap().clone(), vec!["run-child-abort"]);
    assert_eq!(
        rig.json(&rig.get(&format!("/api/runs/{run_record_id}")))["target_hold"],
        Value::Null
    );
    assert_eq!(
        rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#))
            .status,
        202
    );
    rig.wait_until_idle(&task_id);
}

/// 占用是**一张目标表**的事，不是一个任务的事（#271）。
///
/// 指着同一张表的另一个任务，跑不起来、在自己那一行上看得见「锁未释放」、也点得动重试。
/// 而删任务那一关拦的是另一件事：欠着占用的那个任务删不掉，因为重试的入口就长在它
/// 那一行上，删了它占用就再没人点得到（#270）。
#[test]
fn a_stuck_hold_is_about_the_target_table_not_the_task_that_left_it() {
    let rig = Rig::with_child(
        r#"trap 'exit 0' TERM
printf '%s\n' '{"ts":"2026-08-15T16:00:00.000Z","level":"info","event":"stage_changed","run_id":"run-shared-table","task":null,"stage":"STREAMING"}'
while :; do sleep 0.02; done
"#,
    );
    let (agent_url, _aborts, succeeds) = switchable_abort_agent(true);
    let agent_id = rig.register_agent_at("目标端", &agent_url);
    let source_id = rig.create_oracle_datasource("源库");
    let target_id = rig.create_mysql_datasource("目标库", &agent_id);
    let datasources = (source_id, target_id);
    // 两个任务，同一台 agent 上的同一张目标表——占用记在表上，于是它们共用同一份。
    let neighbour = rig.create_task("邻居", "HOLDINGS", &datasources);
    let holder = rig.create_task("欠着占用的", "HOLDINGS", &datasources);

    let start = |task_id: &str| rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    let stop_and_settle = |task_id: &str| -> String {
        let started = start(task_id);
        assert_eq!(started.status, 202, "{}", started.body_text());
        let run_record_id = rig.json(&started)["run_record_id"]
            .as_str()
            .unwrap()
            .to_owned();
        wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
            body["stage"] == "STREAMING"
        });
        assert_eq!(
            rig.post(&format!("/api/runs/{run_record_id}/cancel"), "")
                .status,
            202
        );
        rig.wait_until_idle(task_id);
        run_record_id
    };

    // 邻居先跑一次、停掉，补发的 abort 成了：它那一行干干净净。
    let neighbour_run = stop_and_settle(&neighbour);
    assert_eq!(
        rig.json(&rig.get(&format!("/api/runs/{neighbour_run}")))["target_hold"],
        Value::Null
    );

    // 这一次 abort 不成了：占用留在目标端，欠着它的是另一个任务的运行。
    succeeds.store(false, Ordering::Relaxed);
    let holder_run = stop_and_settle(&holder);
    assert_eq!(
        rig.json(&rig.get(&format!("/api/runs/{holder_run}")))["target_hold"],
        "HELD"
    );

    // **邻居那一行也如实说「锁未释放」**：占用挂在同一张表上，这一行照样不能重跑。
    let neighbour_row = rig.json(&rig.get(&format!("/api/runs/{neighbour_run}")));
    assert_eq!(neighbour_row["target_hold"], "HELD");
    assert_eq!(neighbour_row["target_hold_message"], "暂存表 drop 不掉");
    // 而且它真的发不起来——拦在 source，不必一路跑到目标端才撞回 `TARGET_TABLE_BUSY`。
    let refused = start(&neighbour);
    assert_eq!(refused.status, 409, "{}", refused.body_text());
    assert_eq!(
        rig.json(&refused)["error"]["message"],
        "这张目标表的占用还没释放，先把它释放掉再发起"
    );

    // 欠着占用的那个任务删不掉：删了它，重试那颗按钮就跟着没了，占用永远留在目标端。
    let refused_delete = rig.delete(&format!("/api/tasks/{holder}"));
    assert_eq!(refused_delete.status, 409, "{}", refused_delete.body_text());
    let refusal = rig.json(&refused_delete);
    let message = refusal["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("目标表占用还没释放") && message.contains(&holder_run),
        "{message}"
    );
    assert_eq!(refusal["error"]["runs"], serde_json::json!([holder_run]));
    assert_eq!(rig.get(&format!("/api/tasks/{holder}")).status, 200);

    // 重试也是按表算的：从**邻居那一行**点下去，释放的是同一张表上欠着的那份占用。
    succeeds.store(true, Ordering::Relaxed);
    let released = rig.post(&format!("/api/runs/{neighbour_run}/release"), "");
    assert_eq!(released.status, 200, "{}", released.body_text());
    assert_eq!(
        rig.json(&rig.get(&format!("/api/runs/{holder_run}")))["target_hold"],
        Value::Null
    );

    // 占用还清了，两边一起恢复：任务删得掉，运行也发得起来。
    assert_eq!(rig.delete(&format!("/api/tasks/{holder}")).status, 200);
    let again = start(&neighbour);
    assert_eq!(again.status, 202, "{}", again.body_text());
    let again_id = rig.json(&again)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    // 同上：先等它报出阶段，再收摊。
    wait_for_json(&rig, &format!("/api/runs/{again_id}"), |body| {
        body["stage"] == "STREAMING"
    });
    assert_eq!(
        rig.post(&format!("/api/runs/{again_id}/cancel"), "").status,
        202
    );
    rig.wait_until_idle(&neighbour);
}

/// 已过封口点的运行死掉时，父进程既不补 abort，**也不记一份占用**（#271）。
///
/// 那时 sink 的 `commit` 已经在跑，它的每一条出口都会自己把占用摘掉；而对着一个已封口的
/// run 发 abort 只换回一个 409 `RUN_SEALED`，记下来只会在界面上立起一颗永远点不成的
/// 「锁未释放，点此重试」。所以这条路径上正确的行为是**什么都不做**。
#[test]
fn a_child_that_died_after_the_point_of_no_return_leaves_no_hold_behind() {
    let rig = Rig::with_child(
        r#"printf '%s\n' '{"ts":"2026-08-15T17:00:00.000Z","level":"info","event":"stage_changed","run_id":"run-committing","task":null,"stage":"COMMITTING"}'
sleep 0.2
"#,
    );
    let (agent_url, aborts) = abort_recording_agent(true);
    let agent_id = rig.register_agent_at("目标端", &agent_url);
    let source_id = rig.create_oracle_datasource("源库");
    let target_id = rig.create_mysql_datasource("目标库", &agent_id);
    let task_id = rig.create_task("holdings", "HOLDINGS", &(source_id, target_id));

    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(started.status, 202, "{}", started.body_text());
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
        body["stage"] == "COMMITTING"
    });
    rig.wait_until_idle(&task_id);

    // 一个字节都没发过去：封口点之后 abort 权已经不在 source 手上。
    let seen = aborts.lock().unwrap().clone();
    assert!(seen.is_empty(), "已封口的运行不该被补一刀 abort：{seen:?}");
    // 也没有留下一份占用：那一格照旧是「发起运行」，下一次运行发得起来。
    let history = rig.json(&rig.get(&format!("/api/runs/{run_record_id}")));
    assert_eq!(history["live"], false);
    assert_eq!(history["unknown_reason"], "PROCESS_DISAPPEARED");
    assert_eq!(history["target_hold"], Value::Null);
    let again = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(again.status, 202, "{}", again.body_text());
    rig.wait_until_idle(&task_id);
}

/// source 自己被杀掉时，在飞那次运行的目标表占用留在了目标端——**下一条命起来就得
/// 把它记上账**，而不是把这张表显示成可以重跑（#272）。
///
/// 这是「占用还在就绝不让它看起来能重跑」那条铁律在**进程边界**上的一半：
/// 子进程被父进程收了尸的那一种早就补过 abort 了，这里说的是父进程自己没了的那一种。
#[test]
fn a_restart_books_the_hold_left_behind_by_a_run_nobody_could_abort() {
    let rig = Rig::with_child(
        r#"printf '%s\n' '{"ts":"2026-08-31T06:35:16.000Z","level":"info","event":"stage_changed","run_id":"run-orphan","task":null,"stage":"STREAMING"}'
while :; do sleep 0.02; done
"#,
    );
    let (agent_url, aborts) = abort_recording_agent(false);
    let agent_id = rig.register_agent_at("目标端", &agent_url);
    let source_id = rig.create_oracle_datasource("源库");
    let target_id = rig.create_mysql_datasource("目标库", &agent_id);
    let task_id = rig.create_task("holdings", "HOLDINGS", &(source_id, target_id));

    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(started.status, 202, "{}", started.body_text());
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
        body["stage"] == "STREAMING"
    });

    // 进程在这一刻被 `kill -9`：没有收尾、没有 abort，磁盘上留下一行没走完的历史。
    let restarted = rig.second_life();
    reclaim_after_restart(
        &restarted.history,
        &restarted.run_logs,
        &restarted.runs,
        90,
        restarted.clock.as_ref(),
    )
    .unwrap();

    // 补发那一趟没成（目标端 drop 不掉暂存表），于是占用如实挂着，原话照抄。
    let held = wait_for_json(&restarted, &format!("/api/runs/{run_record_id}"), |body| {
        body["target_hold"] == "HELD"
    });
    assert_eq!(held["target_hold_message"], "暂存表 drop 不掉");
    assert_eq!(held["unknown_reason"], "PROCESS_DISAPPEARED");
    assert_eq!(aborts.lock().unwrap().clone(), vec!["run-orphan"]);

    // 这张表这时候一次新运行都放不进去——放进去也只会撞回目标端的 `TARGET_TABLE_BUSY`。
    let refused = restarted.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(refused.status, 409, "{}", refused.body_text());
    assert_eq!(
        restarted.json(&refused)["error"]["message"],
        "这张目标表的占用还没释放，先把它释放掉再发起"
    );

    // 补发失败照旧落一行 `abort_failed`，而且**接在上一条命写的那行后面**：
    // 新起一支从 0 数的笔会把这条运行原有的日志逐行盖掉。
    let logs = restarted.json(&restarted.get(&format!("/api/runs/{run_record_id}/logs")));
    let lines = logs["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0]["line"].as_str().unwrap().contains("stage_changed"));
    assert_eq!(abort_failures(&logs), 1);
}

/// 补发那一趟成了：占用还回来，这张表当场又能跑（#272）。
#[test]
fn a_restart_releases_the_hold_and_the_table_is_free_again() {
    let rig = Rig::with_child(
        r#"printf '%s\n' '{"ts":"2026-08-31T06:35:16.000Z","level":"info","event":"stage_changed","run_id":"run-orphan","task":null,"stage":"STREAMING"}'
while :; do sleep 0.02; done
"#,
    );
    let (agent_url, aborts) = abort_recording_agent(true);
    let agent_id = rig.register_agent_at("目标端", &agent_url);
    let source_id = rig.create_oracle_datasource("源库");
    let target_id = rig.create_mysql_datasource("目标库", &agent_id);
    let task_id = rig.create_task("holdings", "HOLDINGS", &(source_id, target_id));

    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
        body["stage"] == "STREAMING"
    });

    let restarted = rig.second_life();
    reclaim_after_restart(
        &restarted.history,
        &restarted.run_logs,
        &restarted.runs,
        90,
        restarted.clock.as_ref(),
    )
    .unwrap();

    let released = wait_for_json(&restarted, &format!("/api/runs/{run_record_id}"), |body| {
        body["target_hold"] == Value::Null
    });
    assert_eq!(released["target_hold_message"], Value::Null);
    // 发的是**那一次**运行的 run_id：目标端认的是它。
    assert_eq!(aborts.lock().unwrap().clone(), vec!["run-orphan"]);
    let again = restarted.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(again.status, 202, "{}", again.body_text());
}

/// 优雅停服也得先收摊（#272）：在飞的运行停掉、abort 补上、占用还回去，**然后**才封口。
///
/// 这一半不能指望下一条命：退出前那一下封口会把没走完的那几行全填上 `outcome`，
/// 下一条命起来时一行都看不见——`reclaim_after_restart` 于是无从下手。
/// 记的名义也不是「已由用户停止」：没人按过那颗按钮。
#[test]
fn a_graceful_shutdown_stops_in_flight_runs_and_hands_the_hold_back() {
    let rig = Rig::with_child(
        r#"trap 'exit 0' TERM
printf '%s\n' '{"ts":"2026-08-31T07:00:00.000Z","level":"info","event":"stage_changed","run_id":"run-shutdown","task":null,"stage":"STREAMING"}'
while :; do sleep 0.02; done
"#,
    );
    let (agent_url, aborts) = abort_recording_agent(true);
    let agent_id = rig.register_agent_at("目标端", &agent_url);
    let source_id = rig.create_oracle_datasource("源库");
    let target_id = rig.create_mysql_datasource("目标库", &agent_id);
    let task_id = rig.create_task("holdings", "HOLDINGS", &(source_id, target_id));

    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
        body["stage"] == "STREAMING"
    });

    stop_runs_for_shutdown(&rig.runs, Duration::from_secs(5));

    // 收摊回来时在飞登记已经空了：等的就是那一刀砍没砍下去。
    assert!(!rig.runs.lock().unwrap().has_active_run(&task_id));
    assert_eq!(aborts.lock().unwrap().clone(), vec!["run-shutdown"]);
    let history = rig.json(&rig.get(&format!("/api/runs/{run_record_id}")));
    assert_eq!(history["unknown_reason"], "SERVICE_RESTARTED");
    assert_eq!(history["message"], "服务重启，结局未知");
    assert_eq!(history["target_hold"], Value::Null);
}

/// 这条运行的日志里有几行 `abort_failed`。
fn abort_failures(logs: &Value) -> usize {
    logs["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line["line"].as_str().unwrap()).ok())
        .filter(|line| line["event"] == "abort_failed")
        .count()
}

/// 子进程自己跑完了就**不补 abort**：那时暂存表要么已经换进目标表、要么它自己收拾过了，
/// 再发一次只会在已封口的 run 上换回一个 409，平白造出一条「abort 失败」。
#[test]
fn a_run_that_reported_its_own_terminal_is_not_aborted_by_the_parent() {
    let rig = Rig::with_child(
        r#"printf '%s\n' '{"ts":"2026-08-15T14:20:00.000Z","level":"info","event":"stage_changed","run_id":"run-complete","task":null,"stage":"STREAMING"}'
printf '%s\n' '{"ts":"2026-08-15T14:20:07.000Z","level":"info","event":"run_finished","run_id":"run-complete","task":null,"terminal":"SUCCEEDED","stage":"SUCCEEDED","message":"done","source_code":null,"sink_code":null,"column":null,"value":null,"source_rows":7,"source_batches":2,"staged_rows":7,"received_batches":2,"sink_reported_rows":7,"purged_rows":0,"fetch_ms":4,"push_ms":22,"commit_ms":6,"count_ms":2,"cursor_ms":1}'
"#,
    );
    let (agent_url, aborts) = abort_recording_agent(true);
    let agent_id = rig.register_agent_at("目标端", &agent_url);
    let source_id = rig.create_oracle_datasource("源库");
    let target_id = rig.create_mysql_datasource("目标库", &agent_id);
    let task_id = rig.create_task("holdings", "HOLDINGS", &(source_id, target_id));

    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(started.status, 202, "{}", started.body_text());
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let history = wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
        body["live"] == false
    });
    assert_eq!(history["outcome"], "SUCCEEDED");
    rig.wait_until_idle(&task_id);
    thread::sleep(Duration::from_millis(100));
    let seen = aborts.lock().unwrap().clone();
    assert!(seen.is_empty(), "跑完的运行不该被补一刀 abort：{seen:?}");
}

/// 任务还有运行没结束时删任务被拒（#270）；停止之后再删同一个任务就成功。
///
/// 拒绝而不是「自动停止再删除」：删除不可逆，顺手终止一次可能正在写数据的运行，
/// 风险大于便利。
#[test]
fn delete_task_is_refused_while_a_run_is_in_flight_and_succeeds_after_the_stop() {
    let rig = Rig::with_child(
        r#"trap 'exit 0' TERM
printf '%s\n' "{\"ts\":\"2026-08-15T13:00:00.000Z\",\"level\":\"info\",\"event\":\"stage_changed\",\"run_id\":\"run-1\",\"task\":null,\"stage\":\"STREAMING\"}"
while :; do sleep 0.02; done
"#,
    );
    let (_agent_id, source_id, target_id) = rig.seed();
    let task_id = rig.create_task("holdings", "HOLDINGS", &(source_id, target_id));

    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(started.status, 202, "{}", started.body_text());
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
        body["stage"] == "STREAMING"
    });

    let refused = rig.delete(&format!("/api/tasks/{task_id}"));
    assert_eq!(refused.status, 409, "{}", refused.body_text());
    let refused_body = rig.json(&refused);
    let message = refused_body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("请先停止") && message.contains(&run_record_id),
        "{message}"
    );
    assert_eq!(
        refused_body["error"]["runs"],
        serde_json::json!([run_record_id])
    );
    // 拒了就是一个字节都没删：任务还在，运行也还在飞。
    assert_eq!(rig.get(&format!("/api/tasks/{task_id}")).status, 200);
    assert_eq!(
        rig.json(&rig.get(&format!("/api/runs/{run_record_id}")))["live"],
        true
    );

    let canceled = rig.post(&format!("/api/runs/{run_record_id}/cancel"), "");
    assert_eq!(canceled.status, 202, "{}", canceled.body_text());

    // 「在飞」的账在监督线程里销，而销账要等父进程替子进程补发完那一次 abort（#269），
    // 比 `live` 转 false 晚一点点——判据是「停完了终究删得掉」，不是「下一个请求就删得掉」。
    rig.wait_until_idle(&task_id);
    let deleted = rig.delete(&format!("/api/tasks/{task_id}"));
    assert_eq!(deleted.status, 200, "{}", deleted.body_text());
    assert_eq!(rig.get(&format!("/api/tasks/{task_id}")).status, 404);
}

/// 一台**记账的** agent 桩：照常应答 `/v1/agent/info`，别的 503，
/// 并把看到的每一行请求行记下来，好证明「某条链路一个字节都没过线」。
fn recording_agent() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut head = [0_u8; 1024];
            let read = stream.read(&mut head).unwrap_or(0);
            let request_line = String::from_utf8_lossy(&head[..read])
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            let info = request_line.contains("/v1/agent/info");
            recorder.lock().unwrap().push(request_line);
            let body = if info {
                r#"{"agent_id":"stub-agent","name":"桩 agent","version":"0.0.0-test"}"#
            } else {
                r#"{"error":{"code":"BAD_REQUEST","message":"桩只认 /v1/agent/info","run_id":null,"details":{}}}"#
            };
            let status = if info { "200 OK" } else { "503 Service Unavailable" };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.flush();
        }
    });
    (url, seen)
}

fn target_check_agent(check_body: Value) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(&mut stream);
            let mut request_line = String::new();
            let _ = reader.read_line(&mut request_line);
            let mut content_length = 0;
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                    break;
                }
                if let Some((name, value)) = header.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        content_length = value.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut request_body = vec![0; content_length];
            let _ = reader.read_exact(&mut request_body);
            drop(reader);
            let body = if request_line.contains("/v1/agent/info") {
                r#"{"agent_id":"check-agent","name":"检查桩","version":"0.0.0-test"}"#.to_owned()
            } else if request_line.contains("/v1/target/check") {
                serde_json::to_string(&check_body).unwrap()
            } else {
                r#"{"error":{"code":"BAD_REQUEST","message":"unexpected path","run_id":null,"details":{}}}"#.to_owned()
            };
            let status = if request_line.contains("/v1/agent/info")
                || request_line.contains("/v1/target/check")
            {
                "200 OK"
            } else {
                "404 Not Found"
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.flush();
        }
    });
    url
}

fn described_id(
    _access: &OracleAccess,
    _spec: &TaskSpec,
) -> Result<Vec<SourceColumn>, SourceReadError> {
    Ok(vec![SourceColumn {
        name: "ID".to_owned(),
        data_type: "NUMBER".to_owned(),
        precision: Some(10),
        scale: Some(0),
        length: None,
        fsp: None,
        support: None,
    }])
}

/// 一台**可以停掉**的 agent 桩：注册时它活着，`stop()` 之后端口上没人应答。
struct StoppableAgent {
    url: String,
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl StoppableAgent {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        let mut head = [0_u8; 1024];
                        let _ = stream.read(&mut head);
                        let body = r#"{"agent_id":"stub-agent","name":"桩 agent","version":"0.0.0-test"}"#;
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.flush();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            }
            // 线程退出即丢掉 listener：这台 agent 从此「停了」。
        });
        Self {
            url,
            stop,
            handle: Some(handle),
        }
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap();
        }
    }
}

/// 取列面读请求体走的是与全片同一个 `read_json_body`：同一句中文、同一条上限（#199）。
#[test]
fn column_fetch_reads_its_body_through_the_shared_reader() {
    let rig = Rig::new();
    let response = rig.post("/api/columns", "{ 这不是 JSON");

    assert_eq!(response.status, 400, "{}", response.body_text());
    let body = rig.json(&response);
    assert_eq!(body["error"]["kind"], "request");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("JSON 请求体无效"),
        "{}",
        response.body_text()
    );
}

/// SQL 形状预检整段取消后，取列前的本地闸只剩规格自身的合法性。它仍必须在**连 Oracle 之前**判完。
#[test]
fn column_fetch_rejects_an_invalid_spec_before_reaching_oracle() {
    let rig = Rig::new();
    let response = rig.post(
        "/api/columns",
        r#"{
          "datasource_id":"unused-the-spec-gate-runs-first",
          "spec":{
            "owner":"APP","table":"ORDERS","target_table":"ORDERS",
            "columns":[{"source":"ID","target":"ID"}],"write_mode":"APPEND","schedule_enabled":false,"primary_key":["MISSING"]
          }
        }"#,
    );

    assert_eq!(response.status, 400, "{}", response.body_text());
    let body = rig.json(&response);
    // 壳只有一种（#199）：`kind` 是信封里的字段，不是与 `message` 并排的第二种形状。
    assert_eq!(body["error"]["kind"], "request");
    assert!(body.get("kind").is_none());
    assert!(body.get("message").is_none());
    assert!(body["error"].get("code").is_none());
    assert!(body["error"].get("run_id").is_none());
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("MISSING"));
}

#[test]
fn builder_sql_is_derived_from_the_spec_and_never_travels_back() {
    let rig = Rig::new();
    let generated = rig.post(
        "/api/builder/sql",
        r#"{
          "dblink":"FA",
          "owner":"HTBR45",
          "table":"T_R_FR_ASTSTAT",
          "target_table":"T_POSITION",
          "columns":[{"source":"N_VA_PRICE","target":"N_VA_PRICE"},{"source":"D_BIZ","target":"D_BIZ"}],
          "write_mode":"APPEND","schedule_enabled":false,"primary_key":["D_BIZ"],
          "where_clause":"D_BIZ >= DATE '2026-08-01' AND STATUS IN ('OK','WARN')"
        }"#,
    );

    assert_eq!(generated.status, 200, "{}", generated.body_text());
    let derived = rig.json(&generated);
    // 派生面只剩一样：现算的源端 SQL。
    assert_eq!(derived.as_object().unwrap().len(), 1);
    let sql = derived["source_sql"].as_str().unwrap();
    assert!(sql.contains("T_R_FR_ASTSTAT@FA"), "{sql}");
    assert!(
        sql.ends_with(" WHERE D_BIZ >= DATE '2026-08-01' AND STATUS IN ('OK','WARN')"),
        "{sql}"
    );

    // 形状预检那个端点整段没了，不是改了语义。
    assert_eq!(rig.post("/api/sql-shape", &generated.body_text()).status, 404);

    let tasks = rig.get("/api/tasks");
    assert_eq!(tasks.status, 200);
    assert_eq!(rig.json(&tasks), Value::Array(Vec::new()));
}

#[test]
fn builder_rejects_an_invalid_dblink_before_connecting_to_oracle() {
    let rig = Rig::new();
    let response = rig.post(
        "/api/builder/tables",
        r#"{"datasource_id":"unused-the-dblink-gate-runs-first","dblink":"FA WHERE 1=1"}"#,
    );
    assert_eq!(response.status, 400);
    assert!(rig.json(&response)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("dblink"));
}

#[test]
fn builder_rejects_non_select_custom_sql_before_connecting_to_oracle() {
    let rig = Rig::new();
    let response = rig.post(
        "/api/builder/sql-columns",
        r#"{"datasource_id":"unused","source_sql":"UPDATE T SET C = 1"}"#,
    );
    assert_eq!(response.status, 400);
    assert!(rig.json(&response)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("SELECT"));
}

/// 取列时 Oracle 连不上：不建 run、不碰 sink 的业务端点、不落任何文件。
#[test]
fn column_fetch_oracle_failure_does_not_create_a_run_touch_sink_or_write_storage() {
    let rig = Rig::new();
    let (agent_url, seen) = recording_agent();
    let registered = rig.post(
        "/api/agents",
        &format!(r#"{{"name":"目标端","base_url":"{agent_url}"}}"#),
    );
    assert_eq!(registered.status, 201, "{}", registered.body_text());
    let source_datasource_id = rig.create_oracle_datasource("源库");
    seen.lock().unwrap().clear();
    let files_before = directory_entries(&rig.directory);

    let response = rig.post(
        "/api/columns",
        &format!(
            r#"{{
          "datasource_id":"{source_datasource_id}",
          "spec":{{
            "owner":"APP","table":"MISSING_ORDERS","target_table":"ORDERS",
            "columns":[{{"source":"ID","target":"ID"}},{{"source":"BIZ_DAY","target":"BIZ_DAY"}}],"write_mode":"APPEND","schedule_enabled":false,"primary_key":["ID"]
          }}
        }}"#
        ),
    );

    assert_eq!(response.status, 502, "{}", response.body_text());
    let body = rig.json(&response);
    assert_eq!(body["error"]["kind"], "oracle");
    assert!(body.get("kind").is_none());
    assert!(body["error"].get("run_id").is_none());
    assert_eq!(directory_entries(&rig.directory), files_before);
    // 进程内没有后台探测线程（那归 `serve()`），所以这里的判据是最硬的那条：
    // 取列失败之后，agent 那条地址上**一个字节都没过线**。
    assert!(
        seen.lock().unwrap().is_empty(),
        "取列失败不该碰 sink：{:?}",
        seen.lock().unwrap()
    );
}

/// 目标端元数据面：凭据由 datasource_id 解、请求确实过线给 sink、**一个字节都不落盘**。
#[test]
fn the_target_metadata_proxy_resolves_credentials_and_writes_nothing() {
    let rig = Rig::new();
    let (_agent_id, source_datasource_id, target_datasource_id) = rig.seed();
    let files_before = directory_entries(&rig.directory);

    // Oracle 数据源上没有目标端连接——按名字拒，不编一份出来。
    let wrong_kind = rig.post(
        "/api/target/tables",
        &format!(r#"{{"datasource_id":"{source_datasource_id}"}}"#),
    );
    assert_eq!(wrong_kind.status, 400, "{}", wrong_kind.body_text());

    // 取列面必须点名一张表：不给库清单端点、也不替用户猜表。
    let missing_table = rig.post(
        "/api/target/columns",
        &format!(r#"{{"datasource_id":"{target_datasource_id}"}}"#),
    );
    assert_eq!(missing_table.status, 400, "{}", missing_table.body_text());
    assert!(
        missing_table.body_text().contains("target_table"),
        "{}",
        missing_table.body_text()
    );

    for (path, body) in [
        (
            "/api/target/tables",
            format!(r#"{{"datasource_id":"{target_datasource_id}"}}"#),
        ),
        (
            "/api/target/columns",
            format!(r#"{{"datasource_id":"{target_datasource_id}","target_table":"T_POSITION"}}"#),
        ),
    ] {
        let response = rig.post(path, &body);
        assert_eq!(response.status, 502, "{}", response.body_text());
        let parsed = rig.json(&response);
        assert_eq!(parsed["error"]["kind"], "sink");
        // 不属于任何 run：回话里没有 run_id。
        assert!(
            parsed["error"].get("run_id").is_none(),
            "{}",
            response.body_text()
        );
    }

    // 不进任务定义、不进 SQLite、不留临时文件——目录里一个新条目都没有。
    assert_eq!(directory_entries(&rig.directory), files_before);
}

#[test]
fn target_check_proxies_every_typed_kind_and_attaches_ddl_only_when_failed() {
    let rig = Rig::new();
    let source_id = rig.create_oracle_datasource("源库");
    let findings = [
        "missing_column",
        "nullability_mismatch",
        "insufficient_length_or_precision",
        "primary_key_mismatch",
        "type_not_whitelisted",
    ]
    .into_iter()
    .map(|kind| {
        serde_json::json!({
            "column": "ID",
            "kind": kind,
            "expected": "DECIMAL(10,0)",
            "actual": "<missing>",
            "message": format!("{kind} finding"),
        })
    })
    .collect::<Vec<_>>();
    let agent_url = target_check_agent(serde_json::json!({
        "ok": false,
        "findings": findings,
        "suggested_ddl": null,
    }));
    let registered = rig.post(
        "/api/agents",
        &format!(r#"{{"name":"目标检查","base_url":"{agent_url}"}}"#),
    );
    let agent_id = rig.json(&registered)["agent_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let target_id = rig.create_mysql_datasource("目标库", &agent_id);
    let request = format!(r#"{{"source_datasource_id":"{source_id}","target_datasource_id":"{target_id}","target_table":"HOLDINGS","spec":{{"owner":"APP","table":"HOLDINGS","target_table":"HOLDINGS","columns":[{{"source":"ID","target":"ID"}}],"write_mode":"APPEND","schedule_enabled":false,"primary_key":["ID"]}}}}"#);

    let response = rig.post_with_describer("/api/target/check", &request, described_id);
    assert_eq!(response.status, 200, "{}", response.body_text());
    let body = rig.json(&response);
    assert_eq!(body["ok"], false);
    assert_eq!(body["findings"].as_array().unwrap().len(), 5);
    for kind in [
        "missing_column",
        "nullability_mismatch",
        "insufficient_length_or_precision",
        "primary_key_mismatch",
        "type_not_whitelisted",
    ] {
        assert!(body["findings"].as_array().unwrap().iter().any(|finding| finding["kind"] == kind));
    }
    assert!(body["suggested_ddl"]
        .as_str()
        .unwrap()
        .contains("CREATE TABLE `HOLDINGS`"));

    let ok_url = target_check_agent(serde_json::json!({
        "ok": true,
        "findings": [],
        "suggested_ddl": "must be discarded",
    }));
    let ok_agent = rig.post(
        "/api/agents",
        &format!(r#"{{"name":"目标检查通过","base_url":"{ok_url}"}}"#),
    );
    let ok_agent_id = rig.json(&ok_agent)["agent_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ok_target_id = rig.create_mysql_datasource("目标库通过", &ok_agent_id);
    let ok_request = request.replace(&target_id, &ok_target_id);
    let ok = rig.post_with_describer("/api/target/check", &ok_request, described_id);
    assert_eq!(ok.status, 200, "{}", ok.body_text());
    assert_eq!(rig.json(&ok)["suggested_ddl"], Value::Null);
}

#[test]
fn target_check_maps_request_datasource_agent_and_sink_failures_at_their_boundaries() {
    let rig = Rig::new();
    assert_eq!(rig.post("/api/target/check", "{}").status, 400);

    let source_id = rig.create_oracle_datasource("源库");
    let check_body = |target_id: &str| format!(r#"{{"source_datasource_id":"{source_id}","target_datasource_id":"{target_id}","target_table":"HOLDINGS","spec":{{"owner":"APP","table":"HOLDINGS","target_table":"HOLDINGS","columns":[{{"source":"ID","target":"ID"}}],"write_mode":"APPEND","schedule_enabled":false,"primary_key":["ID"]}}}}"#);
    let invalid_target = check_body(&source_id);
    let wrong_kind = rig.post_with_describer("/api/target/check", &invalid_target, described_id);
    assert_eq!(wrong_kind.status, 400, "{}", wrong_kind.body_text());

    let agent_id = rig.register_agent("拒绝检查的目标端");
    let target_id = rig.create_mysql_datasource("目标库", &agent_id);
    let sink_failure = check_body(&target_id);
    let sink = rig.post_with_describer("/api/target/check", &sink_failure, described_id);
    assert_eq!(sink.status, 502, "{}", sink.body_text());
    assert_eq!(rig.json(&sink)["error"]["kind"], "sink");

    let mut stopped = StoppableAgent::start();
    let registered = rig.post(
        "/api/agents",
        &format!(r#"{{"name":"会停的目标端","base_url":"{}"}}"#, stopped.url),
    );
    let stopped_id = rig.json(&registered)["agent_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let stopped_target = rig.create_mysql_datasource("已离线目标库", &stopped_id);
    stopped.stop();
    let agent_failure = check_body(&stopped_target);
    let agent = rig.post_with_describer("/api/target/check", &agent_failure, described_id);
    assert_eq!(agent.status, 502, "{}", agent.body_text());
    assert_eq!(rig.json(&agent)["error"]["kind"], "agent");
}

/// 草稿测连吃的是**表单里当前填的那组值**，不是库里存的那条，而且一个字节都不落盘。
#[test]
fn the_draft_test_connection_reads_the_form_values_and_writes_nothing() {
    let rig = Rig::new();
    let agent_id = rig.register_agent("目标端");
    let datasources_before = rig.json(&rig.get("/api/datasources"));
    let files_before = directory_entries(&rig.directory);

    // 路由：`test-connection` 那一截不许被当成数据源 id 吃掉。
    let malformed = rig.post("/api/datasources/test-connection", "{}");
    assert_eq!(malformed.status, 400, "{}", malformed.body_text());

    // 字段不全按字段判，不按 id 判。
    let empty_host = rig.post(
        "/api/datasources/test-connection",
        &format!(
            r#"{{"name":"草稿","kind":"mysql","agent_id":"{agent_id}","host":"","port":3306,"username":"u","password":"p","database":"dw"}}"#
        ),
    );
    assert_eq!(empty_host.status, 400, "{}", empty_host.body_text());
    assert!(empty_host.body_text().contains("host"), "{}", empty_host.body_text());

    // 没选 agent 的草稿：连测都不该测。
    let no_agent = rig.post(
        "/api/datasources/test-connection",
        r#"{"name":"草稿","kind":"mysql","host":"127.0.0.1","port":3306,"username":"u","password":"p","database":"dw_stage"}"#,
    );
    assert_eq!(no_agent.status, 400, "{}", no_agent.body_text());
    assert!(no_agent.body_text().contains("agent"), "{}", no_agent.body_text());

    // 目标端草稿：source 不建 MySQL 连接，测连经 agent 转给 sink。
    // agent 活着但不认这个端点 → 502。
    let mysql_draft = rig.post(
        "/api/datasources/test-connection",
        &format!(
            r#"{{"name":"草稿","kind":"mysql","agent_id":"{agent_id}","host":"127.0.0.1","port":3306,"username":"u","password":"p","database":"dw_stage"}}"#
        ),
    );
    assert_eq!(mysql_draft.status, 502, "{}", mysql_draft.body_text());
    let parsed = rig.json(&mysql_draft);
    assert_eq!(parsed["error"]["kind"], "sink");
    assert!(
        parsed["error"].get("run_id").is_none(),
        "{}",
        mysql_draft.body_text()
    );

    // Oracle 草稿：客户端库路径是假的，连不上——但它同样不该落盘。
    let oracle_draft = rig.post(
        "/api/datasources/test-connection",
        r#"{"name":"草稿","kind":"oracle","connect_string":"//127.0.0.1:1521/NOPE","username":"u","password":"p"}"#,
    );
    assert_ne!(oracle_draft.status, 200, "{}", oracle_draft.body_text());

    // 三次测连之后：数据源清单逐字未变、目录里一个新条目都没有。
    assert_eq!(rig.json(&rig.get("/api/datasources")), datasources_before);
    assert_eq!(directory_entries(&rig.directory), files_before);
}

/// agent 注册表的写入面：注册要求对方活着、身份钉在记录上、探测按需重跑、
/// 被数据源引用的 agent 删不掉。
#[test]
fn agent_registration_requires_a_live_agent_and_pins_its_identity() {
    let rig = Rig::new();

    // 注册一台不存在的：**不落库**。
    let dead = TcpListener::bind("127.0.0.1:0").unwrap();
    let dead_url = format!("http://{}", dead.local_addr().unwrap());
    drop(dead);
    let refused = rig.post(
        "/api/agents",
        &format!(r#"{{"name":"死的","base_url":"{dead_url}"}}"#),
    );
    assert_eq!(refused.status, 502, "{}", refused.body_text());
    assert_eq!(
        rig.json(&rig.get("/api/agents")).as_array().unwrap().len(),
        0,
        "注册失败不许在库里留下痕迹"
    );

    // https / 带 query 的地址在打网络之前就拒。
    let bad_scheme = rig.post(
        "/api/agents",
        r#"{"name":"错的","base_url":"https://target:8080"}"#,
    );
    assert_eq!(bad_scheme.status, 400, "{}", bad_scheme.body_text());

    // 注册一台活的：身份、版本、最近可见时间一起落下来。
    let registered = rig.post(
        "/api/agents",
        &format!(r#"{{"name":"目标端 A","base_url":"{}"}}"#, agent_stub_url()),
    );
    assert_eq!(registered.status, 201, "{}", registered.body_text());
    let registered = rig.json(&registered);
    assert_eq!(registered["instance_id"], "stub-agent");
    assert_eq!(registered["status"], "online");
    assert_eq!(registered["version"], "0.0.0-test");
    assert!(registered["last_seen_at"].is_string(), "{registered}");
    let agent_id = registered["agent_id"].as_str().unwrap().to_owned();

    // 手动探测：结果本身是信息，**失败也回 200**。
    let probed = rig.post(&format!("/api/agents/{agent_id}/probe"), "{}");
    assert_eq!(probed.status, 200, "{}", probed.body_text());
    assert_eq!(rig.json(&probed)["status"], "online");

    // 被数据源引用就删不掉。
    let bound = rig.post("/api/datasources", &mysql_datasource_json("目标库", &agent_id));
    assert_eq!(bound.status, 201, "{}", bound.body_text());
    assert_eq!(rig.json(&bound)["agent_id"], agent_id);
    let refused_delete = rig.delete(&format!("/api/agents/{agent_id}"));
    assert_eq!(refused_delete.status, 409, "{}", refused_delete.body_text());
    assert!(
        refused_delete.body_text().contains("目标库"),
        "{}",
        refused_delete.body_text()
    );

    // 绑一台不在注册表里的 agent：写入面当场拒。
    let dangling = rig.post("/api/datasources", &mysql_datasource_json("野的", "nonexistent"));
    assert_eq!(dangling.status, 400, "{}", dangling.body_text());
}

/// 目标端 agent 停掉之后，四条目标端链路必须**全部当场断**，
/// 而且断在「agent 不在线」这一句上。
#[test]
fn a_stopped_agent_breaks_every_target_side_link() {
    let rig = Rig::with_child("sleep 5\n");
    let mut agent = StoppableAgent::start();

    let registered = rig.post(
        "/api/agents",
        &format!(r#"{{"name":"目标端","base_url":"{}"}}"#, agent.url),
    );
    assert_eq!(registered.status, 201, "{}", registered.body_text());
    let agent_id = rig.json(&registered)["agent_id"].as_str().unwrap().to_owned();
    let source_datasource_id = rig.create_oracle_datasource("源库");
    let target_datasource_id = rig.create_mysql_datasource("目标库", &agent_id);
    let task_id = rig.create_task(
        "holdings",
        "HOLDINGS",
        &(source_datasource_id, target_datasource_id.clone()),
    );

    agent.stop(); // 这台 agent 从此停了。

    for (method, path, body) in [
        (
            Method::Post,
            format!("/api/datasources/{target_datasource_id}/test-connection"),
            String::from("{}"),
        ),
        (
            Method::Post,
            "/api/target/tables".to_owned(),
            format!(r#"{{"datasource_id":"{target_datasource_id}"}}"#),
        ),
        (
            Method::Post,
            "/api/target/columns".to_owned(),
            format!(r#"{{"datasource_id":"{target_datasource_id}","target_table":"T_POSITION"}}"#),
        ),
        (
            Method::Post,
            "/api/runs".to_owned(),
            format!(r#"{{"task_id":"{task_id}"}}"#),
        ),
    ] {
        let response = rig.send(method, &path, &body);
        assert_eq!(response.status, 502, "{path}: {}", response.body_text());
        let parsed = rig.json(&response);
        assert_eq!(parsed["error"]["kind"], "agent", "{path}: {}", response.body_text());
        assert!(
            parsed["error"]["message"].as_str().unwrap().contains("不在线"),
            "{path}: {}",
            response.body_text()
        );
    }

    // 一次运行都没起来：没有临时任务文件、历史是空的。
    assert_eq!(rig.json(&rig.get("/api/runs")), serde_json::json!([]));
    assert!(
        !rig.directory.join("run-tasks").exists()
            || directory_entries(&rig.directory.join("run-tasks")).is_empty()
    );

    // 注册表那一列也如实变红，人不必去猜。
    let agents = rig.json(&rig.get("/api/agents"));
    assert_eq!(agents[0]["status"], "offline", "{agents}");
    assert!(agents[0]["last_error"].is_string(), "{agents}");
}

// ---------------------------------------------------------------- 登录与会话
//
// 这一段问的全是「门」的事。它护住的是 **source 的 HTTP 面**——sink 那半边仍然
// 没有鉴权，本文件一个字也证明不了那半边的安全。

/// 每条路由都得声明自己归哪一档。**这张表和 `routes()` 两头对账**：
/// 新加一条路由却不在这里写明它公开还是要登录，这条测试当场就红。
#[test]
fn every_route_declares_its_access() {
    // 公开的**只有三条**，全是会话本身：没有它们，登录也会撞 401。
    let public: [(Method, &str); 3] = [
        (Method::Get, "/api/session"),
        (Method::Post, "/api/session"),
        (Method::Delete, "/api/session"),
    ];
    for (method, pattern) in &public {
        let route = routes()
            .iter()
            .find(|route| route.method == *method && route.pattern == *pattern)
            .unwrap_or_else(|| panic!("路由表里没有 {method:?} {pattern}"));
        assert_eq!(
            route.access,
            Access::Public,
            "{method:?} {pattern} 应当是公开的"
        );
    }
    let administrator: [(Method, &str); 16] = [
        (Method::Get, "/api/operator-account"),
        (Method::Put, "/api/operator-account"),
        (Method::Get, "/api/email-alert-settings"),
        (Method::Put, "/api/email-alert-settings"),
        (Method::Post, "/api/email-alert-settings/test"),
        (Method::Get, "/api/email-deliveries"),
        (Method::Post, "/api/email-deliveries/{}/retry"),
        (Method::Post, "/api/agents"),
        (Method::Post, "/api/agents/{}/probe"),
        (Method::Put, "/api/agents/{}"),
        (Method::Delete, "/api/agents/{}"),
        (Method::Post, "/api/datasources"),
        (Method::Post, "/api/datasources/test-connection"),
        (Method::Post, "/api/datasources/{}/test-connection"),
        (Method::Put, "/api/datasources/{}"),
        (Method::Delete, "/api/datasources/{}"),
    ];
    for route in routes() {
        if public.contains(&(route.method, route.pattern)) {
            continue;
        }
        let expected = if administrator.contains(&(route.method, route.pattern)) {
            Access::Administrator
        } else {
            Access::Session
        };
        assert_eq!(route.access, expected, "{:?} {} 权限档位不对", route.method, route.pattern);
    }
}

/// 没登录的人碰**每一条**要登录的路由，一律 401。
///
/// 判据直接从 `routes()` 现拿，不另抄一份清单：抄一份就会有漏掉的那一条，
/// 而漏掉的那一条正是会被人走进来的那一条。
#[test]
fn every_session_route_refuses_an_anonymous_request() {
    let rig = Rig::new();
    for route in routes() {
        if route.access == Access::Public {
            continue;
        }
        let url = route.pattern.replace("{}", "some-id");
        let response = rig.send_anonymous(route.method, &url, "{}");
        assert_eq!(
            response.status,
            401,
            "{:?} {} 放进了一个没登录的请求：{}",
            route.method,
            route.pattern,
            response.body_text()
        );
    }
}

/// 没匹配上任何路由时，**没登录的人看到的也是 401，不是 404**。
///
/// 两者的差别足以让门外的人把整张路由表枚举出来——一条 404 就等于回答了
/// 「这个路径存在吗」。登录之后照旧是 404，那才是它本来的意思。
#[test]
fn an_unknown_api_path_looks_the_same_as_a_real_one_from_outside() {
    let rig = Rig::new();
    assert_eq!(rig.send_anonymous(Method::Get, "/api/nope", "").status, 401);
    let known = rig.send_anonymous(Method::Get, "/api/tasks", "");
    assert_eq!(known.status, 401);
    assert_eq!(rig.get("/api/nope").status, 404);
}

/// 出厂口令进得去，错口令进不去，而**两种失败回同一句话**。
#[test]
fn the_default_credentials_open_the_door_and_a_wrong_one_does_not() {
    let rig = Rig::new();

    let refused = rig.send_anonymous(
        Method::Post,
        "/api/session",
        r#"{"username":"admin","password":"nope"}"#,
    );
    assert_eq!(refused.status, 401);
    assert_eq!(rig.json(&refused)["error"]["message"], "账号或口令不正确");
    assert!(refused.header("Set-Cookie").is_none(), "被拒的登录不许发票据");

    // 账号不存在与口令不对**一字不差**：分开报只会告诉试口令的人账号叫什么。
    let unknown_account = rig.send_anonymous(
        Method::Post,
        "/api/session",
        r#"{"username":"root","password":"admin"}"#,
    );
    assert_eq!(unknown_account.status, 401);
    assert_eq!(
        rig.json(&unknown_account)["error"]["message"],
        rig.json(&refused)["error"]["message"]
    );
    let disabled_operator = rig.send_anonymous(
        Method::Post,
        "/api/session",
        r#"{"username":"operator","password":"admin"}"#,
    );
    assert_eq!(disabled_operator.status, 401);
    assert_eq!(
        rig.json(&disabled_operator)["error"]["message"],
        rig.json(&refused)["error"]["message"]
    );

    let accepted = rig.send_anonymous(
        Method::Post,
        "/api/session",
        r#"{"username":"admin","password":"admin"}"#,
    );
    assert_eq!(accepted.status, 200, "{}", accepted.body_text());
    let cookie = accepted.header("Set-Cookie").unwrap().to_owned();
    assert!(cookie.starts_with("db_qbs_session="), "{cookie}");
    // 三条属性缺一不可：`HttpOnly` 挡住脚本读票据，`SameSite=Strict` 挡住跨站带票，
    // `Path=/` 让退出时那条 `Max-Age=0` 对得上、真的删得掉。
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    assert!(cookie.contains("SameSite=Strict"), "{cookie}");
    assert!(cookie.contains("Path=/"), "{cookie}");
    // **没有 `Secure`**：现场是明文 HTTP，带上它 cookie 根本存不下来。
    assert!(!cookie.contains("Secure"), "{cookie}");
}

/// 「我登着吗」这一句**没登录也问得出来**——首屏靠它决定摆登录页还是摆应用。
#[test]
fn the_session_probe_answers_from_outside_the_door() {
    let rig = Rig::new();

    let outside = rig.send_anonymous(Method::Get, "/api/session", "");
    assert_eq!(outside.status, 200, "{}", outside.body_text());
    assert_eq!(rig.json(&outside)["authenticated"], false);
    assert!(rig.json(&outside)["username"].is_null());

    let inside = rig.get("/api/session");
    assert_eq!(inside.status, 200);
    assert_eq!(rig.json(&inside)["authenticated"], true);
    assert_eq!(rig.json(&inside)["username"], "admin");
    assert_eq!(rig.json(&inside)["role"], "ADMIN");
}

#[test]
fn existing_admin_hash_migrates_unchanged_and_operator_starts_disabled() {
    let bootstrap = temp_directory();
    let first = AuthStore::open(&bootstrap).unwrap();
    drop(first);
    let bootstrap_database = rusqlite::Connection::open(bootstrap.join("db-qbs.sqlite3")).unwrap();
    let legacy: String = bootstrap_database
        .query_row("SELECT password_hash FROM credentials WHERE id = 1", [], |row| row.get(0))
        .unwrap();
    drop(bootstrap_database);
    fs::remove_dir_all(bootstrap).unwrap();

    let directory = temp_directory();
    let database = rusqlite::Connection::open(directory.join("db-qbs.sqlite3")).unwrap();
    database
        .execute_batch(
            "CREATE TABLE credentials (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                password_hash TEXT NOT NULL
             );
             CREATE TABLE sessions (
                token TEXT PRIMARY KEY NOT NULL,
                created_at INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL
             );",
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO credentials (id, password_hash) VALUES (1, ?1)",
            [&legacy],
        )
        .unwrap();
    let now = chrono::Utc::now();
    database
        .execute(
            "INSERT INTO sessions (token, created_at, last_seen_at) VALUES ('legacy-session', ?1, ?1)",
            [now.timestamp()],
        )
        .unwrap();
    drop(database);

    let reopened = AuthStore::open(&directory).unwrap();
    let database = rusqlite::Connection::open(directory.join("db-qbs.sqlite3")).unwrap();
    let migrated: String = database
        .query_row("SELECT password_hash FROM accounts WHERE username = 'admin'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(migrated, legacy, "迁移不该重新散列管理员口令");
    drop(database);

    assert!(reopened.verify_password("admin", "admin").unwrap());
    assert_eq!(
        reopened
            .resolve_session("legacy-session", now)
            .unwrap()
            .unwrap()
            .username,
        "admin"
    );
    assert!(!reopened.verify_password("operator", "admin").unwrap());
    assert!(!reopened.verify_password("nobody", "admin").unwrap());
    let operator = reopened.operator_account().unwrap();
    assert!(!operator.enabled);
    assert!(!operator.has_password);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn administrator_activates_operator_and_session_reports_the_fixed_identity() {
    let rig = Rig::new();
    let before = rig.get("/api/operator-account");
    assert_eq!(before.status, 200);
    assert_eq!(rig.json(&before)["enabled"], false);
    assert_eq!(rig.json(&before)["has_password"], false);
    assert_eq!(rig.json(&before)["role"], "OPERATOR");

    let incomplete = rig.put("/api/operator-account", r#"{"enabled":true}"#);
    assert_eq!(incomplete.status, 400);

    let enabled = rig.put(
        "/api/operator-account",
        r#"{"enabled":true,"password":"operator-secret"}"#,
    );
    assert_eq!(enabled.status, 200, "{}", enabled.body_text());
    assert_eq!(rig.json(&enabled)["enabled"], true);
    assert_eq!(rig.json(&enabled)["has_password"], true);
    assert!(!enabled.body_text().contains("operator-secret"));

    let login = rig.send_anonymous(
        Method::Post,
        "/api/session",
        r#"{"username":"operator","password":"operator-secret"}"#,
    );
    assert_eq!(login.status, 200, "{}", login.body_text());
    assert_eq!(rig.json(&login)["username"], "operator");
    assert_eq!(rig.json(&login)["role"], "OPERATOR");
    let cookie = login.header("Set-Cookie").unwrap();
    let token = cookie
        .split(';')
        .next()
        .unwrap()
        .split_once('=')
        .unwrap()
        .1;
    let state = rig.send_with_session(token, Method::Get, "/api/session", "");
    assert_eq!(rig.json(&state)["username"], "operator");
    assert_eq!(rig.json(&state)["role"], "OPERATOR");
}

#[test]
fn operator_can_use_daily_work_routes_but_admin_routes_return_stable_403() {
    let rig = Rig::new();
    rig.auth.update_operator(true, Some("operator-secret")).unwrap();
    let token = rig
        .auth
        .issue_session("operator", chrono::Utc::now())
        .unwrap()
        .token;

    for route in routes() {
        if route.access == Access::Public {
            continue;
        }
        let url = route.pattern.replace("{}", "some-id");
        let response = rig.send_with_session(&token, route.method, &url, "{}");
        if route.access == Access::Administrator {
            assert_eq!(response.status, 403, "{:?} {}", route.method, route.pattern);
            assert_eq!(rig.json(&response)["error"]["code"], "FORBIDDEN");
        } else {
            assert_ne!(response.status, 401, "{:?} {}", route.method, route.pattern);
            assert_ne!(response.status, 403, "{:?} {}", route.method, route.pattern);
        }
    }
}

#[test]
fn email_alert_settings_default_to_editable_tencent_values() {
    let rig = Rig::new();

    let response = rig.get("/api/email-alert-settings");

    assert_eq!(response.status, 200, "{}", response.body_text());
    assert_eq!(
        rig.json(&response),
        serde_json::json!({
            "enabled": false,
            "provider_preset": "TENCENT_EXMAIL",
            "smtp_host": "smtp.exmail.qq.com",
            "smtp_port": 465,
            "smtp_security": "IMPLICIT_TLS",
            "smtp_username": "",
            "has_smtp_secret": false,
            "sender_address": "",
            "sender_name": "",
            "recipients": [],
            "max_retry_hours": 24,
            "instance_name": "db-qbs",
            "external_base_url": null,
            "latest_test_result": null,
        })
    );
}

#[test]
fn email_alert_settings_persist_encrypted_with_write_only_blank_preserve() {
    let rig = Rig::new();
    let saved = rig.put(
        "/api/email-alert-settings",
        &email_alert_settings_json(false, "SMTP-secret-marker", 12),
    );
    assert_eq!(saved.status, 200, "{}", saved.body_text());
    let saved_json = rig.json(&saved);
    assert_eq!(saved_json["provider_preset"], "GENERIC");
    assert_eq!(saved_json["smtp_security"], "STARTTLS");
    assert_eq!(saved_json["recipients"], serde_json::json!(["Ops@example.com", "audit@example.org"]));
    assert_eq!(saved_json["external_base_url"], "https://qbs.example.com");
    assert_eq!(saved_json["has_smtp_secret"], true);
    assert!(saved_json.get("smtp_secret").is_none());
    assert!(rig.mail_transport.sent.lock().unwrap().is_empty());

    let database = fs::read(rig.directory.join("db-qbs.sqlite3")).unwrap();
    assert!(!String::from_utf8_lossy(&database).contains("SMTP-secret-marker"));

    let reopened = rig.second_life();
    let reopened_json = reopened.json(&reopened.get("/api/email-alert-settings"));
    assert_eq!(reopened_json, saved_json);
    let preserved = reopened.put(
        "/api/email-alert-settings",
        &email_alert_settings_json(true, "", 12),
    );
    assert_eq!(preserved.status, 200, "{}", preserved.body_text());
    assert_eq!(reopened.json(&preserved)["has_smtp_secret"], true);
    let delivery = reopened.email_alerts.delivery_settings().unwrap().unwrap();
    assert_eq!(delivery.secret, "SMTP-secret-marker");
    assert_eq!(delivery.host, "mail.example.com");
}

#[test]
fn email_alert_settings_validate_shape_without_smtp_io() {
    let rig = Rig::new();

    for (name, body) in [
        ("plaintext security", email_alert_settings_json(false, "secret", 24).replace("STARTTLS", "PLAINTEXT")),
        ("invalid recipient", email_alert_settings_json(false, "secret", 24).replace("audit@example.org", "not-an-email")),
        ("retry too long", email_alert_settings_json(false, "secret", 169)),
        ("URL path", email_alert_settings_json(false, "secret", 24).replace("https://qbs.example.com", "https://qbs.example.com/app")),
    ] {
        let response = rig.put("/api/email-alert-settings", &body);
        assert_eq!(response.status, 400, "{name}: {}", response.body_text());
    }

    let recipients: Vec<_> = (0..51).map(|index| format!("ops{index}@example.com")).collect();
    let too_many = email_alert_settings_json(false, "secret", 24)
        .replace(
            r#"["Ops@example.com"," ops@example.com ","audit@example.org"]"#,
            &serde_json::to_string(&recipients).unwrap(),
        );
    let response = rig.put("/api/email-alert-settings", &too_many);
    assert_eq!(response.status, 400, "{}", response.body_text());

    let incomplete = email_alert_settings_json(true, "", 24).replace("mail.example.com", "");
    let response = rig.put("/api/email-alert-settings", &incomplete);
    assert_eq!(response.status, 400, "{}", response.body_text());
    assert!(rig.mail_transport.sent.lock().unwrap().is_empty());
}

#[test]
fn test_email_uses_saved_settings_sends_each_recipient_and_persists_success() {
    let rig = Rig::new();
    let saved = rig.put(
        "/api/email-alert-settings",
        &email_alert_settings_json(false, "SMTP-secret-marker", 24),
    );
    assert_eq!(saved.status, 200, "{}", saved.body_text());

    let response = rig.post("/api/email-alert-settings/test", "");

    assert_eq!(response.status, 200, "{}", response.body_text());
    assert_eq!(rig.json(&response)["status"], "SUCCESS");
    assert_eq!(rig.json(&response)["tested_at"], "2026-08-31T10:00:00+00:00");
    assert_eq!(rig.json(&response)["error"], Value::Null);
    let sent = rig.mail_transport.sent.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].host, "mail.example.com");
    assert_eq!(sent[0].port, 587);
    assert_eq!(sent[0].security, db_qbs_source::SmtpSecurity::Starttls);
    assert_eq!(sent[0].username, "mailer");
    assert_eq!(sent[0].secret, "SMTP-secret-marker");
    assert_eq!(sent[0].sender_name, "db-qbs alerts");
    assert_eq!(sent[0].mail.envelope_from, "alerts@example.com");
    assert_eq!(sent[0].mail.envelope_to, "Ops@example.com");
    assert_eq!(sent[1].mail.envelope_to, "audit@example.org");
    let message = String::from_utf8_lossy(&sent[0].mail.message);
    assert!(message.contains("multipart/alternative"));
    assert!(message.contains("text/plain"));
    assert!(message.contains("text/html"));
    assert!(message.contains("Subject:"));
    drop(sent);

    assert!(rig.history.list(None).unwrap().is_empty());
    let reopened = rig.second_life();
    let settings = reopened.json(&reopened.get("/api/email-alert-settings"));
    assert_eq!(settings["latest_test_result"]["status"], "SUCCESS");
    assert_eq!(
        settings["latest_test_result"]["tested_at"],
        "2026-08-31T10:00:00+00:00"
    );
}

#[test]
fn test_email_persists_only_a_sanitized_failure() {
    let rig = Rig::new();
    assert_eq!(
        rig.put(
            "/api/email-alert-settings",
            &email_alert_settings_json(false, "SMTP-secret-marker", 24),
        )
        .status,
        200
    );
    *rig.mail_transport.failure.lock().unwrap() = Some(MailTransportError::Timeout);

    let response = rig.post("/api/email-alert-settings/test", "");

    assert_eq!(response.status, 200, "{}", response.body_text());
    assert_eq!(rig.json(&response)["status"], "FAILED");
    assert_eq!(rig.json(&response)["error"], "SMTP 连接或响应超时");
    assert!(!response.body_text().contains("SMTP-secret-marker"));
    assert_eq!(rig.mail_transport.sent.lock().unwrap().len(), 2);

    let reopened = rig.second_life();
    let settings = reopened.json(&reopened.get("/api/email-alert-settings"));
    assert_eq!(settings["latest_test_result"]["status"], "FAILED");
    assert_eq!(settings["latest_test_result"]["error"], "SMTP 连接或响应超时");
    assert!(!settings.to_string().contains("SMTP-secret-marker"));
    assert!(reopened.history.list(None).unwrap().is_empty());
}

#[test]
fn run_details_expose_only_the_same_aggregate_alert_state_to_both_roles() {
    let rig = Rig::new();
    assert_eq!(
        rig.put(
            "/api/email-alert-settings",
            &email_alert_settings_json(true, "SMTP-secret-marker", 24),
        )
        .status,
        200
    );
    let now = rig.clock.now();
    let mut failed = RunHistory::accepted(
        "aggregate-alert-run",
        "task-aggregate",
        "SELECT sensitive_sql FROM source",
        now,
    );
    failed.task_name = "汇总状态".to_owned();
    failed.outcome = Some("FAILED".to_owned());
    failed.finished_at = Some(now.to_rfc3339());
    failed.failure_kind = Some("NETWORK".to_owned());
    failed.message = Some("raw SMTP server response".to_owned());
    rig.history.finalize(&failed, now, 90).unwrap();

    rig.auth
        .update_operator(true, Some("operator-secret"))
        .unwrap();
    let operator = rig.auth.issue_session("operator", now).unwrap().token;
    let path = "/api/runs/aggregate-alert-run";
    let admin_response = rig.get(path);
    let operator_response = rig.send_with_session(&operator, Method::Get, path, "");
    assert_eq!(admin_response.status, 200);
    assert_eq!(operator_response.status, 200);
    let expected = serde_json::json!({
        "alert_id": "alert-aggregate-alert-run",
        "delivery_state": "PENDING",
    });
    assert_eq!(rig.json(&admin_response)["alert"], expected);
    assert_eq!(rig.json(&operator_response)["alert"], expected);
    for response in [admin_response, operator_response] {
        let body = response.body_text();
        for private in [
            "Ops@example.com",
            "audit@example.org",
            "SMTP-secret-marker",
            "attempt_count",
            "last_error",
        ] {
            assert!(!body.contains(private), "run projection leaked {private}");
        }
    }
}

#[test]
fn disabled_and_incomplete_alerts_are_final_not_sent_without_fake_recipients() {
    let rig = Rig::new();
    let now = rig.clock.now();
    let mut failed = RunHistory::accepted("disabled-alert", "task-disabled", "SELECT secret", now);
    failed.outcome = Some("FAILED".to_owned());
    failed.finished_at = Some(now.to_rfc3339());
    failed.failure_kind = Some("NETWORK".to_owned());
    rig.history.finalize(&failed, now, 90).unwrap();

    let run = rig.get("/api/runs/disabled-alert");
    assert_eq!(run.status, 200);
    assert_eq!(rig.json(&run)["alert"]["delivery_state"], "NOT_SENT");
    for private in ["recipient", "attempt_count", "last_error", "retry_deadline_at"] {
        assert!(!run.body_text().contains(private));
    }
    assert_eq!(
        rig.json(&rig.get("/api/email-deliveries?run_record_id=disabled-alert")),
        serde_json::json!([]),
        "an Alert-level disposition represents zero recipients without a fake address"
    );

    assert_eq!(
        rig.put(
            "/api/email-alert-settings",
            &email_alert_settings_json(false, "SMTP-secret-marker", 24),
        )
        .status,
        200
    );
    let mut configured_disabled =
        RunHistory::accepted("configured-disabled", "task-disabled", "SELECT secret", now);
    configured_disabled.outcome = Some("FAILED".to_owned());
    configured_disabled.finished_at = Some(now.to_rfc3339());
    configured_disabled.failure_kind = Some("NETWORK".to_owned());
    rig.history.finalize(&configured_disabled, now, 90).unwrap();
    let deliveries = rig.json(
        &rig.get("/api/email-deliveries?run_record_id=configured-disabled"),
    );
    assert_eq!(deliveries.as_array().unwrap().len(), 2);
    assert!(deliveries
        .as_array()
        .unwrap()
        .iter()
        .all(|delivery| delivery["state"] == "NOT_SENT"));
    let retry = format!(
        "/api/email-deliveries/{}/retry",
        deliveries[0]["delivery_id"].as_str().unwrap()
    );
    assert_eq!(rig.post(&retry, "").status, 400);
    let suppressed_id = deliveries[1]["delivery_id"].as_str().unwrap();
    rusqlite::Connection::open(rig.directory.join("db-qbs.sqlite3"))
        .unwrap()
        .execute(
            "UPDATE email_deliveries SET state = 'SUPPRESSED' WHERE delivery_id = ?1",
            [suppressed_id],
        )
        .unwrap();
    assert_eq!(
        rig.post(
            &format!("/api/email-deliveries/{suppressed_id}/retry"),
            "",
        )
        .status,
        400
    );
}

#[test]
fn disabling_terminates_pending_work_and_reenabling_never_backfills_it() {
    let rig = Rig::new();
    assert_eq!(
        rig.put(
            "/api/email-alert-settings",
            &email_alert_settings_json(true, "SMTP-secret-marker", 24),
        )
        .status,
        200
    );
    let now = rig.clock.now();
    let mut failed = RunHistory::accepted("terminated-alert", "task-disabled", "SELECT 1", now);
    failed.outcome = Some("FAILED".to_owned());
    failed.finished_at = Some(now.to_rfc3339());
    failed.failure_kind = Some("NETWORK".to_owned());
    rig.history.finalize(&failed, now, 90).unwrap();
    *rig.mail_transport.failure.lock().unwrap() = Some(MailTransportError::Timeout);
    assert_eq!(
        rig.alert_outbox
            .run_due_attempts(
                &rig.email_alerts,
                rig.mail_transport.as_ref(),
                rig.clock.as_ref(),
            )
            .unwrap(),
        2
    );

    let disabled = rig.put(
        "/api/email-alert-settings",
        &email_alert_settings_json(false, "", 24),
    );
    assert_eq!(disabled.status, 200, "{}", disabled.body_text());
    let terminated = rig.json(
        &rig.get("/api/email-deliveries?run_record_id=terminated-alert"),
    );
    assert!(terminated.as_array().unwrap().iter().all(|delivery| {
        delivery["state"] == "NOT_SENT"
            && delivery["last_error"] == "管理员已停用邮件告警"
            && delivery["next_attempt_at"].is_null()
    }));
    assert_eq!(
        rig.json(&rig.get("/api/runs/terminated-alert"))["alert"]["delivery_state"],
        "NOT_SENT"
    );

    let enabled = rig.put(
        "/api/email-alert-settings",
        &email_alert_settings_json(true, "", 24),
    );
    assert_eq!(enabled.status, 200, "{}", enabled.body_text());
    *rig.mail_transport.failure.lock().unwrap() = None;
    assert_eq!(
        rig.alert_outbox
            .run_due_attempts(
                &rig.email_alerts,
                rig.mail_transport.as_ref(),
                rig.clock.as_ref(),
            )
            .unwrap(),
        0
    );
    let still_terminated = rig.json(
        &rig.get("/api/email-deliveries?run_record_id=terminated-alert"),
    );
    assert!(still_terminated
        .as_array()
        .unwrap()
        .iter()
        .all(|delivery| delivery["state"] == "NOT_SENT"));
    let retry = format!(
        "/api/email-deliveries/{}/retry",
        still_terminated[0]["delivery_id"].as_str().unwrap()
    );
    assert_eq!(rig.post(&retry, "").status, 400);
}

#[test]
fn only_administrators_can_diagnose_and_retry_exhausted_email_deliveries() {
    let rig = Rig::new();
    assert_eq!(
        rig.put(
            "/api/email-alert-settings",
            &email_alert_settings_json(true, "SMTP-secret-marker", 0),
        )
        .status,
        200
    );
    let now = rig.clock.now();
    let mut failed = RunHistory::accepted(
        "delivery-diagnostics",
        "task-delivery",
        "SELECT private_marker FROM source",
        now,
    );
    failed.task_name = "投递诊断".to_owned();
    failed.outcome = Some("FAILED".to_owned());
    failed.finished_at = Some(now.to_rfc3339());
    failed.failure_kind = Some("NETWORK".to_owned());
    rig.history.finalize(&failed, now, 90).unwrap();
    *rig.mail_transport.failure.lock().unwrap() = Some(MailTransportError::Timeout);
    assert_eq!(
        rig.alert_outbox
            .run_due_attempts(
                &rig.email_alerts,
                rig.mail_transport.as_ref(),
                rig.clock.as_ref(),
            )
            .unwrap(),
        2
    );

    rig.auth
        .update_operator(true, Some("operator-secret"))
        .unwrap();
    let operator = rig
        .auth
        .issue_session("operator", now)
        .unwrap()
        .token;
    let private_path = "/api/email-deliveries?run_record_id=delivery-diagnostics";
    let refused = rig.send_with_session(&operator, Method::Get, private_path, "");
    assert_eq!(refused.status, 403);
    assert!(!refused.body_text().contains("Ops@example.com"));
    assert!(!refused.body_text().contains("attempt_count"));

    let history_response = rig.get(private_path);
    assert_eq!(history_response.status, 200);
    let history = rig.json(&history_response);
    assert_eq!(history.as_array().unwrap().len(), 2);
    assert_eq!(history[0]["state"], "FAILED");
    assert_eq!(history[0]["attempt_count"], 1);
    assert_eq!(history[0]["last_error"], "SMTP 连接或响应超时");
    assert_eq!(history[0]["alert_id"], "alert-delivery-diagnostics");
    assert!(history[0]["recipient"].is_string());
    let delivery_id = history[0]["delivery_id"].as_str().unwrap();

    rig.clock.set(now + chrono::Duration::hours(1));
    let retry_path = format!("/api/email-deliveries/{delivery_id}/retry");

    assert_eq!(
        rig.put(
            "/api/email-alert-settings",
            &email_alert_settings_json(false, "", 0),
        )
        .status,
        200
    );
    let disabled_retry = rig.post(&retry_path, "");
    assert_eq!(disabled_retry.status, 400);
    assert_eq!(rig.json(&disabled_retry)["error"]["kind"], "request");
    assert_eq!(
        rig.json(&disabled_retry)["error"]["message"],
        "只有已耗尽重试窗口的失败投递才能手动重试"
    );
    assert_eq!(
        rig.alert_outbox
            .run_due_attempts(
                &rig.email_alerts,
                rig.mail_transport.as_ref(),
                rig.clock.as_ref(),
            )
            .unwrap(),
        0,
        "disabled delivery must not revive an exhausted delivery"
    );

    rusqlite::Connection::open(rig.directory.join("db-qbs.sqlite3"))
        .unwrap()
        .execute(
            "UPDATE email_alert_settings SET enabled = 1, smtp_host = '' WHERE singleton_id = 1",
            [],
        )
        .unwrap();
    let incomplete_retry = rig.post(&retry_path, "");
    assert_eq!(incomplete_retry.status, 400);
    assert_eq!(
        rig.json(&incomplete_retry)["error"],
        rig.json(&disabled_retry)["error"],
        "disabled and incomplete settings expose the same stable rejection"
    );
    assert_eq!(
        rig.alert_outbox
            .delivery_history(Some("delivery-diagnostics"))
            .unwrap()[0]
            .state,
        db_qbs_source::EmailDeliveryState::Failed,
        "incomplete settings must leave exhausted delivery final"
    );

    assert_eq!(
        rig.put(
            "/api/email-alert-settings",
            &email_alert_settings_json(true, "", 0),
        )
        .status,
        200
    );
    *rig.mail_transport.failure.lock().unwrap() = None;
    assert_eq!(
        rig.alert_outbox
            .run_due_attempts(
                &rig.email_alerts,
                rig.mail_transport.as_ref(),
                rig.clock.as_ref(),
            )
            .unwrap(),
        0,
        "re-enabling delivery must not backfill a rejected manual retry"
    );

    let retried = rig.post(&retry_path, "");
    assert_eq!(retried.status, 200, "{}", retried.body_text());
    let retried = rig.json(&retried);
    assert_eq!(retried["state"], "PENDING");
    assert_eq!(
        retried["attempt_count"], 1,
        "manual retry keeps lifetime count"
    );
    assert_eq!(retried["next_attempt_at"], rig.clock.now().to_rfc3339());
    assert_eq!(rig.post(&retry_path, "").status, 400);

    let operator_retry = rig.send_with_session(&operator, Method::Post, &retry_path, "");
    assert_eq!(operator_retry.status, 403);
    assert!(!operator_retry.body_text().contains("Ops@example.com"));
}

#[test]
fn accepted_run_failure_finishes_before_the_outbox_first_attempt() {
    let rig = Rig::with_child(
        r#"printf '%s\n' '{"ts":"2026-08-31T10:00:01.000Z","event":"run_finished","run_id":"run-alert","terminal":"FAILED","stage":"FAILED","message":"raw failure must stay local","failure_kind":"SOURCE_QUERY","source_code":"ORA-marker","sink_code":null,"column":null,"value":"sample-marker","source_rows":0,"source_batches":0,"staged_rows":0,"received_batches":0,"sink_reported_rows":0,"purged_rows":0,"fetch_ms":1,"push_ms":0,"commit_ms":0,"count_ms":0,"cursor_ms":0}'
"#,
    );
    assert_eq!(
        rig.put(
            "/api/email-alert-settings",
            &email_alert_settings_json(true, "SMTP-secret-marker", 24),
        )
        .status,
        200
    );
    let (_agent_id, source_id, target_id) = rig.seed();
    let task_id = rig.create_task("源查询失败", "ALERT_TARGET", &(source_id, target_id));

    let accepted = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(accepted.status, 202, "{}", accepted.body_text());
    let run_record_id = rig.json(&accepted)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let finished = wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
        body["live"] == false
    });
    assert_eq!(finished["outcome"], "FAILED");
    assert_eq!(finished["alert"]["delivery_state"], "PENDING");
    assert!(rig.mail_transport.sent.lock().unwrap().is_empty());

    assert_eq!(
        rig.alert_outbox
            .run_due_attempts(
                &rig.email_alerts,
                rig.mail_transport.as_ref(),
                rig.clock.as_ref(),
            )
            .unwrap(),
        2
    );
    let sent = rig.json(&rig.get(&format!("/api/runs/{run_record_id}")));
    assert_eq!(sent["outcome"], "FAILED");
    assert_eq!(sent["alert"]["delivery_state"], "SENT");
}

fn email_alert_settings_json(enabled: bool, secret: &str, retry_hours: u8) -> String {
    serde_json::json!({
        "enabled": enabled,
        "provider_preset": "GENERIC",
        "smtp_host": "mail.example.com",
        "smtp_port": 587,
        "smtp_security": "STARTTLS",
        "smtp_username": "mailer",
        "smtp_secret": secret,
        "sender_address": "alerts@example.com",
        "sender_name": "db-qbs alerts",
        "recipients": ["Ops@example.com", " ops@example.com ", "audit@example.org"],
        "max_retry_hours": retry_hours,
        "instance_name": "production",
        "external_base_url": "https://qbs.example.com",
    })
    .to_string()
}

#[test]
fn operator_password_reset_and_disable_invalidate_only_operator_sessions() {
    let rig = Rig::new();
    rig.auth.update_operator(true, Some("first")).unwrap();
    let current = rig.auth.issue_session("operator", chrono::Utc::now()).unwrap().token;
    let elsewhere = rig.auth.issue_session("operator", chrono::Utc::now()).unwrap().token;
    let admin = rig.auth.issue_session("admin", chrono::Utc::now()).unwrap().token;

    let changed = rig.send_with_session(
        &current,
        Method::Put,
        "/api/password",
        r#"{"current_password":"first","new_password":"second"}"#,
    );
    assert_eq!(changed.status, 200, "{}", changed.body_text());
    assert_eq!(rig.send_with_session(&current, Method::Get, "/api/tasks", "").status, 200);
    assert_eq!(rig.send_with_session(&elsewhere, Method::Get, "/api/tasks", "").status, 401);
    assert_eq!(rig.send_with_session(&admin, Method::Get, "/api/tasks", "").status, 200);

    let reset = rig.put(
        "/api/operator-account",
        r#"{"enabled":true,"password":"third"}"#,
    );
    assert_eq!(reset.status, 200);
    assert_eq!(rig.send_with_session(&current, Method::Get, "/api/tasks", "").status, 401);
    assert!(rig.auth.verify_password("operator", "third").unwrap());

    let active = rig.auth.issue_session("operator", chrono::Utc::now()).unwrap().token;
    assert_eq!(rig.put("/api/operator-account", r#"{"enabled":false}"#).status, 200);
    assert_eq!(rig.send_with_session(&active, Method::Get, "/api/tasks", "").status, 401);
    assert!(!rig.auth.verify_password("operator", "third").unwrap());
}

/// 退出销的是**这一张票**，别处登着的同一个账号不受影响。
#[test]
fn logging_out_burns_one_ticket_and_leaves_the_others_alone() {
    let rig = Rig::new();
    let elsewhere = rig.auth.issue_session("admin", chrono::Utc::now()).unwrap().token;
    let elsewhere_cookie = format!("db_qbs_session={elsewhere}");

    let goodbye = rig.delete("/api/session");
    assert_eq!(goodbye.status, 200, "{}", goodbye.body_text());
    assert!(goodbye.header("Set-Cookie").unwrap().contains("Max-Age=0"));

    assert_eq!(rig.get("/api/tasks").status, 401, "退出之后这张票还认");
    let still_in = rig.api().handle(
        &Request::new(Method::Get, "/api/tasks", Vec::new())
            .with_header("Cookie", elsewhere_cookie),
    );
    assert_eq!(still_in.status, 200, "另一处的登录被这次退出连坐了");
}

/// 退出是**幂等**的：没带票据也回 200。「你本来就没登着」不是一次失败。
#[test]
fn logging_out_twice_is_not_an_error() {
    let rig = Rig::new();
    assert_eq!(rig.delete("/api/session").status, 200);
    assert_eq!(rig.send_anonymous(Method::Delete, "/api/session", "").status, 200);
}

/// 每一次带票据的请求都把 cookie 续一次期。
///
/// 服务端那份滑动窗口已经往前推了；cookie 不跟着续，浏览器就会在**登录满 8 小时**
/// 那一刻把票据丢掉，于是「闲置 8 小时才踢」在用户那边变成「登录 8 小时必被踢」。
#[test]
fn every_authenticated_request_slides_the_cookie_forward() {
    let rig = Rig::new();
    let response = rig.get("/api/tasks");
    assert_eq!(response.status, 200);
    let cookie = response.header("Set-Cookie").unwrap();
    assert!(cookie.contains(&format!("Max-Age={}", 8 * 60 * 60)), "{cookie}");
}

/// 改口令：要先输当前口令，改完**除了这一张之外的会话全部失效**。
#[test]
fn changing_the_password_keeps_this_session_and_burns_the_rest() {
    let rig = Rig::new();
    let elsewhere = rig.auth.issue_session("admin", chrono::Utc::now()).unwrap().token;
    rig.auth.update_operator(true, Some("operator-secret")).unwrap();
    let operator = rig
        .auth
        .issue_session("operator", chrono::Utc::now())
        .unwrap()
        .token;

    let wrong = rig.put(
        "/api/password",
        r#"{"current_password":"nope","new_password":"新口令"}"#,
    );
    assert_eq!(wrong.status, 400);
    assert_eq!(rig.json(&wrong)["error"]["message"], "当前口令不正确");

    let empty = rig.put(
        "/api/password",
        r#"{"current_password":"admin","new_password":""}"#,
    );
    assert_eq!(empty.status, 400);
    assert_eq!(rig.json(&empty)["error"]["message"], "新口令不能为空");

    let changed = rig.put(
        "/api/password",
        r#"{"current_password":"admin","new_password":"新口令"}"#,
    );
    assert_eq!(changed.status, 200, "{}", changed.body_text());

    // 改密的常见动机就是「我怀疑别处有人登着」，所以别处那张票必须当场作废。
    let stale = rig.api().handle(
        &Request::new(Method::Get, "/api/tasks", Vec::new())
            .with_header("Cookie", format!("db_qbs_session={elsewhere}")),
    );
    assert_eq!(stale.status, 401, "改完口令，别处那张票还认");
    // 而**发起这次改密的这一张留着**：改完口令立刻被自己踢出去毫无道理。
    assert_eq!(rig.get("/api/tasks").status, 200);
    assert_eq!(
        rig.send_with_session(&operator, Method::Get, "/api/tasks", "").status,
        200,
        "管理员改密不该清掉操作员会话"
    );

    assert_eq!(
        rig.send_anonymous(
            Method::Post,
            "/api/session",
            r#"{"username":"admin","password":"admin"}"#
        )
        .status,
        401,
        "旧口令改完还能登"
    );
    assert_eq!(
        rig.send_anonymous(
            Method::Post,
            "/api/session",
            r#"{"username":"admin","password":"新口令"}"#
        )
        .status,
        200
    );
}

/// 闲置**满 8 小时**才过期，而且窗口是**滑动**的：期间有请求就一直不过期。
///
/// 走 store 而不是走 HTTP：过期与否取决于「现在几点」，而 HTTP 那一层的现在
/// 只能是真的现在。这条判据本身没有第二份实现。
#[test]
fn a_session_expires_only_after_eight_idle_hours() {
    let rig = Rig::new();
    let start = chrono::Utc::now();
    let token = rig.auth.issue_session("admin", start).unwrap().token;
    let hours = |n: i64| start + chrono::Duration::hours(n);

    assert!(rig.auth.authenticate(&token, hours(7)).unwrap(), "7 小时就被踢了");
    // 上一句把窗口推到了第 7 小时，所以第 14 小时仍在窗口内——这就是「滑动」。
    assert!(rig.auth.authenticate(&token, hours(14)).unwrap(), "窗口没有跟着往前滑");
    assert!(
        !rig.auth.authenticate(&token, hours(23)).unwrap(),
        "闲置超过 8 小时还认"
    );
    // 过期的票据当场删掉：留着只会让这张表长成一个没人清的坟场。
    assert!(!rig.auth.authenticate(&token, hours(23)).unwrap());
}

/// `reset-password` 的那一半：口令回到出厂值，**所有**会话一并作废。
#[test]
fn resetting_the_password_returns_to_the_factory_default_and_burns_every_session() {
    let rig = Rig::new();
    rig.auth.change_password("admin", "admin", "新口令", "").unwrap();
    let token = rig.auth.issue_session("admin", chrono::Utc::now()).unwrap().token;

    rig.auth.reset_password().unwrap();

    assert!(rig.auth.verify_password("admin", "admin").unwrap());
    assert!(
        !rig.auth
            .authenticate(&token, chrono::Utc::now())
            .unwrap(),
        "重置之后旧会话还认——而跑这条命令的人正是进不去的那个"
    );
}

// ---------------------------------------------------------------------------
// 并发（#255）
//
// source 的 accept 循环已经是多条工作线程共用一个监听器，所以下面这几条问的是
// **同一份 `Api` 被几条线程同时使唤时会怎样**。它们仍然走 `Api::handle` 这一个入口，
// 不开新的验证面：并发是这一层的性质，不是一台新机器。
// ---------------------------------------------------------------------------

/// 多线程 accept 的编译期前提：`Api` 和它借着的每一份状态都得是 `Sync`。
///
/// 这条看着像废话，但它是**唯一**会在有人把裸 `rusqlite::Connection`
/// （`Send` 而非 `Sync`）放回 store 里时当场喊停的地方——否则下一个发现的人
/// 是 `server.rs` 里那段 `thread::scope`，报错位置离肇事处十万八千里。
#[test]
fn every_piece_of_shared_state_is_shareable_across_threads() {
    fn assert_sync<T: Sync>() {}
    assert_sync::<Api<'static>>();
    assert_sync::<TaskStore>();
    assert_sync::<DatasourceStore>();
    assert_sync::<AuthStore>();
    assert_sync::<HistoryStore>();
    assert_sync::<Arc<Mutex<AgentStore>>>();
    assert_sync::<Arc<Mutex<RunState>>>();
}

/// 一条线程正卡在慢 Oracle 取数里，另一条线程拉任务列表**不等它**。
///
/// 这是这一票交付的那句话本身。判据是时间：取数固定睡 2 秒，列表那一发必须在
/// 它睡醒之前就回来了。慢的那一头走 `/api/target/check`，因为它**先摸数据源库、
/// 再做阻塞取数**——数据源那把锁若跨过了取数，这条会当场超时。
#[test]
fn a_slow_oracle_fetch_does_not_block_another_client_listing_tasks() {
    static DESCRIBING: AtomicU64 = AtomicU64::new(0);
    const SLOW_DESCRIBE: Duration = Duration::from_secs(2);

    fn slow_describe(
        access: &OracleAccess,
        spec: &TaskSpec,
    ) -> Result<Vec<SourceColumn>, SourceReadError> {
        DESCRIBING.fetch_add(1, Ordering::SeqCst);
        thread::sleep(SLOW_DESCRIBE);
        described_id(access, spec)
    }

    let rig = Rig::new();
    // 作用域线程只借，不搬：`Rig` 得活到 `thread::scope` 之外（`Drop` 要清临时目录）。
    let rig = &rig;
    let (_agent_id, source_id, target_id) = rig.seed();
    rig.create_task("holdings", "HOLDINGS", &(source_id.clone(), target_id.clone()));
    let check = format!(
        r#"{{"source_datasource_id":"{source_id}","target_datasource_id":"{target_id}","target_table":"HOLDINGS","spec":{{"owner":"APP","table":"HOLDINGS","target_table":"HOLDINGS","columns":[{{"source":"ID","target":"ID"}}],"write_mode":"APPEND","schedule_enabled":false,"primary_key":["ID"]}}}}"#
    );

    thread::scope(|scope| {
        let slow = scope.spawn(|| rig.post_with_describer("/api/target/check", &check, slow_describe));

        // 等它真的进到取数里，再计时——否则量到的可能只是线程还没起来。
        let deadline = Instant::now() + Duration::from_secs(5);
        while DESCRIBING.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "慢取数一直没开始");
            thread::sleep(Duration::from_millis(5));
        }

        let started = Instant::now();
        let listed = rig.get("/api/tasks");
        let elapsed = started.elapsed();
        assert_eq!(listed.status, 200, "{}", listed.body_text());
        assert_eq!(rig.json(&listed).as_array().unwrap().len(), 1);
        assert!(
            elapsed < SLOW_DESCRIBE / 2,
            "取数把任务列表一起冻住了：列表等了 {elapsed:?}"
        );

        // 慢的那一头照旧要走完；它撞在桩 agent 的 503 上，那不是这条测试问的事。
        let _ = slow.join().unwrap();
    });
}

/// 几条线程同时读写任务表：不丢写、不重 id、不死锁。
///
/// 每条写线程建 5 条，读线程全程不停地拉列表。判据有两条——
/// 末了库里不多不少 40 条且 id 两两不同（写没丢、也没互相盖掉），
/// 读线程每一发都是 200（读没被写打断成半张表）。
#[test]
fn concurrent_task_writes_and_reads_neither_lose_a_write_nor_deadlock() {
    const WRITERS: usize = 8;
    const PER_WRITER: usize = 5;
    const READERS: usize = 4;

    let rig = Rig::new();
    let rig = &rig;
    let (_agent_id, source_id, target_id) = rig.seed();
    let datasources = (source_id, target_id);
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let created: Vec<String> = thread::scope(|scope| {
        let readers: Vec<_> = (0..READERS)
            .map(|_| {
                let done = Arc::clone(&done);
                scope.spawn(move || {
                    let mut rounds = 0_usize;
                    while !done.load(Ordering::SeqCst) {
                        let listed = rig.get("/api/tasks");
                        assert_eq!(listed.status, 200, "{}", listed.body_text());
                        assert!(rig.json(&listed).is_array());
                        rounds += 1;
                    }
                    rounds
                })
            })
            .collect();
        let writers: Vec<_> = (0..WRITERS)
            .map(|writer| {
                let datasources = datasources.clone();
                scope.spawn(move || {
                    (0..PER_WRITER)
                        .map(|index| {
                            rig.create_task(
                                &format!("并发任务-{writer}-{index}"),
                                "HOLDINGS",
                                &datasources,
                            )
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();

        let created: Vec<String> = writers
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect();
        done.store(true, Ordering::SeqCst);
        for reader in readers {
            assert!(reader.join().unwrap() > 0, "读线程一发都没跑成");
        }
        created
    });

    assert_eq!(created.len(), WRITERS * PER_WRITER);
    let unique: std::collections::HashSet<&String> = created.iter().collect();
    assert_eq!(unique.len(), created.len(), "并发建任务撞出了重复 task_id");

    let listed = rig.json(&rig.get("/api/tasks"));
    let stored: std::collections::HashSet<String> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["task_id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(stored.len(), created.len(), "并发写丢了任务");
    for task_id in &created {
        assert!(stored.contains(task_id), "{task_id} 没落库");
    }
}

/// 同一条任务被几条线程同时发起：**恰好一个** 202，其余全是 409。
///
/// 「一条任务同时只跑一次」这条互斥原来只有一条 accept 线程护着，它是不是真的互斥
/// 从来没被问过。现在问了。
#[test]
fn concurrent_starts_of_one_task_elect_exactly_one_winner() {
    const STARTERS: usize = 8;

    let directory = temp_directory();
    let release = directory.join("release-children");
    let rig = Rig::with_child(&format!(
        "while [ ! -f '{}' ]; do sleep 0.02; done\nexit 0\n",
        release.display()
    ));
    let rig = &rig;
    let (_agent_id, source_id, target_id) = rig.seed();
    let task_id = rig.create_task("holdings", "HOLDINGS", &(source_id, target_id));
    let body = format!(r#"{{"task_id":"{task_id}"}}"#);

    let statuses: Vec<u16> = thread::scope(|scope| {
        let handles: Vec<_> = (0..STARTERS)
            .map(|_| {
                let body = body.clone();
                scope.spawn(move || rig.post("/api/runs", &body))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap().status)
            .collect()
    });

    let accepted = statuses.iter().filter(|status| **status == 202).count();
    let rejected = statuses.iter().filter(|status| **status == 409).count();
    assert_eq!(accepted, 1, "同一条任务并发发起被放进去 {accepted} 次：{statuses:?}");
    assert_eq!(rejected, STARTERS - 1, "{statuses:?}");

    fs::write(&release, "").unwrap();
    let running = rig.json(&rig.get("/api/runs"));
    let run_record_id = running.as_array().unwrap()[0]["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_json(rig, &format!("/api/runs/{run_record_id}"), |body| {
        body["live"] == false
    });
    wait_for_empty_directory(&rig.directory.join("run-tasks"));
    let _ = fs::remove_dir_all(directory);
}

/// 并发登录 / 认票据：会话表在多线程下不串号、也不把请求卡住。
///
/// `authenticate()` 落在**每一个** `/api/*` 上，是多线程之后最热的一处竞争。
#[test]
fn concurrent_sessions_are_issued_and_authenticated_without_crossing_wires() {
    const CLIENTS: usize = 8;

    let rig = Rig::new();
    let rig = &rig;
    let tokens: Vec<String> = thread::scope(|scope| {
        let handles: Vec<_> = (0..CLIENTS)
            .map(|_| {
                scope.spawn(|| {
                    let response = rig.send_anonymous(
                        Method::Post,
                        "/api/session",
                        r#"{"username":"admin","password":"admin"}"#,
                    );
                    assert_eq!(response.status, 200, "{}", response.body_text());
                    let cookie = response
                        .headers
                        .iter()
                        .find(|(name, _)| name.eq_ignore_ascii_case("Set-Cookie"))
                        .map(|(_, value)| value.clone())
                        .expect("登录没有下发 cookie");
                    cookie
                        .split(';')
                        .next()
                        .unwrap()
                        .trim_start_matches(&format!("{SESSION_COOKIE}="))
                        .to_owned()
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });

    let unique: std::collections::HashSet<&String> = tokens.iter().collect();
    assert_eq!(unique.len(), CLIENTS, "并发登录发出了重复票据");
    thread::scope(|scope| {
        for token in &tokens {
            scope.spawn(move || {
                let response = rig.api().handle(
                    &Request::new(Method::Get, "/api/tasks", Vec::new())
                        .with_header("Cookie", format!("{SESSION_COOKIE}={token}")),
                );
                assert_eq!(response.status, 200, "{}", response.body_text());
            });
        }
    });
}

/// 运行**进行中**与**已结束**两种情况下，游标接口都得答得出来。
#[test]
fn run_logs_are_served_incrementally_while_the_run_is_live_and_after_it_ends() {
    let directory = temp_directory();
    let release = directory.join("release-child");
    let long_value = "值".repeat(200);
    let rig = Rig::with_child(&format!(
        r#"printf '%s\n' '{{"ts":"2026-08-15T10:00:00.000Z","level":"info","event":"source_started","run_id":null,"task":null,"message":"started"}}'
printf '%s\n' '{{"ts":"2026-08-15T10:00:01.000Z","level":"info","event":"stage_changed","run_id":"run-9","task":null,"stage":"STREAMING","message":"streaming"}}'
printf '%s\n' '这一行不是 JSON'
while [ ! -f '{}' ]; do sleep 0.02; done
printf '%s\n' '{{"ts":"2026-08-15T10:00:07.000Z","level":"error","event":"run_finished","run_id":"run-9","task":null,"terminal":"FAILED","stage":"FAILED","message":"目标端拒绝","failure_kind":"TARGET_REJECTED","source_code":null,"sink_code":"WRITE_FAILED","column":"AMOUNT","value":"{}","source_rows":1,"source_batches":1,"staged_rows":0,"received_batches":1,"sink_reported_rows":0,"purged_rows":0,"fetch_ms":1,"push_ms":1,"commit_ms":0,"count_ms":0,"cursor_ms":0}}'
"#,
        release.display(),
        long_value,
    ));
    let (_agent_id, source_id, target_id) = rig.seed();
    let task_id = rig.create_task("holdings", "HOLDINGS", &(source_id, target_id));

    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(started.status, 202, "{}", started.body_text());
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // 运行还活着的时候就能一段一段取。
    let live = wait_for_json(&rig, &format!("/api/runs/{run_record_id}/logs"), |body| {
        body["lines"].as_array().unwrap().len() >= 3
    });
    assert_eq!(live["live"], true);
    assert_eq!(live["after"], 0);
    assert_eq!(live["next_after"], 3);
    assert_eq!(live["has_more"], false);
    let lines = live["lines"].as_array().unwrap();
    assert_eq!(lines[0]["seq"], 1);
    let first: Value = serde_json::from_str(lines[0]["line"].as_str().unwrap()).unwrap();
    // 原文照存：进程间那份 JSON Lines 契约一个字都没改。
    assert_eq!(first["event"], "source_started");
    // 解析不出 JSON 的那一行也照存——来什么显什么，不吞。
    assert_eq!(lines[2]["line"], "这一行不是 JSON");

    // 带上游标只拿新的那一段。
    let after_two = rig.json(&rig.get(&format!("/api/runs/{run_record_id}/logs?after=2")));
    let tail = after_two["lines"].as_array().unwrap();
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0]["seq"], 3);
    // 游标停在末尾就是空段，不是错。
    let caught_up = rig.json(&rig.get(&format!("/api/runs/{run_record_id}/logs?after=3")));
    assert_eq!(caught_up["lines"].as_array().unwrap().len(), 0);
    assert_eq!(caught_up["next_after"], 3);

    fs::write(release, "").unwrap();
    wait_for_json(&rig, &format!("/api/runs/{run_record_id}"), |body| {
        body["live"] == false
    });

    // 结束之后同一条路照旧走得通，终态那一行也在里面。
    let finished = wait_for_json(&rig, &format!("/api/runs/{run_record_id}/logs?after=3"), |body| {
        !body["lines"].as_array().unwrap().is_empty()
    });
    assert_eq!(finished["live"], false);
    let terminal: Value =
        serde_json::from_str(finished["lines"][0]["line"].as_str().unwrap()).unwrap();
    assert_eq!(terminal["event"], "run_finished");
    assert_eq!(terminal["column"], "AMOUNT");
    // 业务值落库前截到 64 个字符：够判断是哪一列坏了，不够当数据副本。
    assert_eq!(terminal["value"].as_str().unwrap().chars().count(), 64);
    assert_eq!(terminal["value_truncated"], true);
    // 折叠出来的那一行运行历史照旧成立，格式没变过。
    let history = rig.json(&rig.get(&format!("/api/runs/{run_record_id}")));
    assert_eq!(history["outcome"], "FAILED");
    assert_eq!(history["column"], "AMOUNT");
}

#[test]
fn run_logs_refuse_an_unknown_run_and_a_nonsense_cursor() {
    let rig = Rig::new();
    assert_eq!(rig.get("/api/runs/no-such-run/logs").status, 404);

    let (_agent_id, source_id, target_id) = rig.seed();
    let task_id = rig.create_task("holdings", "HOLDINGS", &(source_id, target_id));
    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();
    for bad in ["abc", "-1", ""] {
        let response = rig.get(&format!("/api/runs/{run_record_id}/logs?after={bad}"));
        assert_eq!(response.status, 400, "after={bad} 本该被拒");
    }
}

// ------------------------------------------------------------------ 调度派活
//
// 「不补跑」「开关关着不触发」这两条是 `ScheduleState::observe` 的纯函数性质，
// 用例在 `scheduler.rs` 里。这里守的是另外三条**必须有 store 和子进程才成立**的：
// 到点真的发得出去、上一次没结束时留下的那行历史长什么样、额度满时排在哪儿。
//
// 时钟是参数（`run_scheduler_pass` 的 `now`），所以这三条一秒都不用等。

/// 调度器的一轮，时钟由用例给。
fn scheduler_pass(rig: &Rig, now: &str) {
    let terminated = std::sync::atomic::AtomicBool::new(false);
    db_qbs_source::run_scheduler_pass(
        &rig.api(),
        &rig.schedule,
        chrono::NaiveDateTime::parse_from_str(now, "%Y-%m-%d %H:%M").unwrap(),
        &terminated,
    );
}

fn scheduled_task_json(
    name: &str,
    target_table: &str,
    datasources: &(String, String),
    cron: &str,
) -> String {
    let (source_datasource_id, target_datasource_id) = datasources;
    format!(
        r#"{{"name":"{name}","source_datasource_id":"{source_datasource_id}","target_datasource_id":"{target_datasource_id}","spec":{{"owner":"APP","table":"HOLDINGS","target_table":"{target_table}","columns":[{{"source":"ID","target":"ID"}},{{"source":"D_BIZ","target":"D_BIZ"}}],"write_mode":"APPEND","schedule_cron":"{cron}","schedule_enabled":true,"primary_key":["ID"],"where_clause":"D_BIZ = DATE '2026-08-14'"}}}}"#
    )
}

fn scheduled_task_json_with_pre_sql(
    name: &str,
    target_table: &str,
    datasources: &(String, String),
    cron: &str,
    pre_sql: &str,
) -> String {
    let mut task: Value =
        serde_json::from_str(&scheduled_task_json(name, target_table, datasources, cron)).unwrap();
    task["spec"]["pre_sql"] = Value::String(pre_sql.to_owned());
    serde_json::to_string(&task).unwrap()
}

fn create_scheduled_task(
    rig: &Rig,
    name: &str,
    target_table: &str,
    datasources: &(String, String),
    cron: &str,
) -> String {
    let response = rig.post(
        "/api/tasks",
        &scheduled_task_json(name, target_table, datasources, cron),
    );
    assert_eq!(response.status, 201, "{}", response.body_text());
    rig.json(&response)["task_id"].as_str().unwrap().to_owned()
}

/// 到点了就自己跑起来，而且历史上认得出这是**调度发起的**，不是有人按的。
///
/// 起服务那一刻不算触发（不补跑）：第一轮只把下一个时刻算出来，一次运行都不该发出去。
#[test]
fn the_cron_instant_starts_a_run_that_history_marks_as_scheduled() {
    let rig = Rig::new();
    let (_agent_id, source_id, target_id) = rig.seed();
    let datasources = (source_id, target_id);
    let created = rig.post(
        "/api/tasks",
        &scheduled_task_json_with_pre_sql(
            "每分钟",
            "HOLDINGS",
            &datasources,
            "* * * * *",
            REPRESENTATIVE_PRE_SQL,
        ),
    );
    assert_eq!(created.status, 201, "{}", created.body_text());
    let task_id = rig.json(&created)["task_id"].as_str().unwrap().to_owned();

    scheduler_pass(&rig, "2026-08-28 03:10");
    let quiet = rig.json(&rig.get(&format!("/api/runs?task_id={task_id}")));
    assert!(
        quiet.as_array().unwrap().is_empty(),
        "刚看到这个任务的那一轮不该触发任何运行：{quiet}"
    );

    scheduler_pass(&rig, "2026-08-28 03:11");
    let listed = rig.json(&rig.get(&format!("/api/runs?task_id={task_id}")));
    assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed[0]["trigger"], "SCHEDULED");
    assert_eq!(listed[0]["task_name"], "每分钟");
    assert_eq!(
        listed[0]["evidence"]["parameters"]["pre_sql"],
        REPRESENTATIVE_PRE_SQL
    );
    // 真发出去了：有运行标识那一半由子进程补，但临时任务文件此刻已经在磁盘上。
    assert!(listed[0]["run_record_id"].as_str().is_some_and(|id| !id.is_empty()));
    let task_files = directory_entries(&rig.directory.join("run-tasks"));
    assert_eq!(task_files.len(), 1);
    assert_eq!(
        load_task_config(&task_files[0])
            .unwrap()
            .spec
            .pre_sql
            .as_deref(),
        Some(REPRESENTATIVE_PRE_SQL)
    );

    // 同一分钟内再评估一次不会再触发一回（`next_after` 是严格之后）。
    scheduler_pass(&rig, "2026-08-28 03:11");
    let again = rig.json(&rig.get(&format!("/api/runs?task_id={task_id}")));
    assert_eq!(again.as_array().unwrap().len(), 1, "{again}");
}

/// 上一次还没结束，本次跳过——**但那个触发时刻在历史里留了一行**。
///
/// 这行与「预检拒绝、从未到达代理」同构：`run_id` 是空的，目标表一个字节都没动过。
/// 它存在的唯一理由是「月末那次到底跑没跑」得有答案。
#[test]
fn an_occurrence_that_collides_with_a_live_run_is_recorded_without_a_run_id() {
    let rig = Rig::new();
    assert_eq!(
        rig.put(
            "/api/email-alert-settings",
            &email_alert_settings_json(true, "schedule-secret", 24),
        )
        .status,
        200
    );
    let (_agent_id, source_id, target_id) = rig.seed();
    let datasources = (source_id, target_id);
    let task_id = create_scheduled_task(&rig, "每分钟", "HOLDINGS", &datasources, "* * * * *");

    // 有人手动跑了一次，而假子进程睡着不动——到点时它还在飞。
    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(started.status, 202, "{}", started.body_text());
    let manual_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();

    scheduler_pass(&rig, "2026-08-28 03:10");
    scheduler_pass(&rig, "2026-08-28 03:11");

    let listed = rig.json(&rig.get(&format!("/api/runs?task_id={task_id}")));
    let rows = listed.as_array().unwrap();
    assert_eq!(rows.len(), 2, "被跳过的那一次也要留一行：{listed}");
    let skipped = rows
        .iter()
        .find(|row| row["run_record_id"] != manual_id.as_str())
        .unwrap();
    assert_eq!(skipped["run_id"], Value::Null, "跳过的那一次没有运行标识");
    assert_eq!(skipped["trigger"], "SCHEDULED");
    assert_eq!(skipped["failure_kind"], "SKIPPED");
    assert_eq!(skipped["scheduled_refusal_reason"], "PREVIOUS_RUN_ACTIVE");
    assert_eq!(skipped["message"], "上次尚未结束，本次跳过");
    assert_eq!(skipped["outcome"], "FAILED");
    assert_eq!(skipped["target_table_effect"], "DISCARDED");
    assert_eq!(skipped["task_name"], "每分钟");
    assert_eq!(skipped["live"], false);
    assert_eq!(skipped["alert"]["delivery_state"], "PENDING");
}

#[test]
fn a_scheduled_refusal_for_a_missing_datasource_is_alertable() {
    let rig = Rig::new();
    assert_eq!(
        rig.put(
            "/api/email-alert-settings",
            &email_alert_settings_json(true, "schedule-secret", 24),
        )
        .status,
        200
    );
    let input: TaskInput = serde_json::from_str(&scheduled_task_json(
        "悬空源库",
        "HOLDINGS",
        &("missing-source".to_owned(), "missing-target".to_owned()),
        "* * * * *",
    ))
    .unwrap();
    let task = rig.tasks.create(input).unwrap();

    scheduler_pass(&rig, "2026-08-28 03:10");
    scheduler_pass(&rig, "2026-08-28 03:11");

    let listed = rig.json(&rig.get(&format!("/api/runs?task_id={}", task.task_id)));
    assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed[0]["run_id"], Value::Null);
    assert_eq!(listed[0]["trigger"], "SCHEDULED");
    assert_eq!(listed[0]["outcome"], "FAILED");
    assert_eq!(listed[0]["failure_kind"], "SKIPPED");
    assert_eq!(
        listed[0]["scheduled_refusal_reason"],
        "SOURCE_DATASOURCE_UNAVAILABLE"
    );
    assert_eq!(listed[0]["alert"]["delivery_state"], "PENDING");
}

#[test]
fn repeated_busy_schedule_alerts_expose_fixed_window_suppression() {
    let rig = Rig::new();
    assert_eq!(
        rig.put(
            "/api/email-alert-settings",
            &email_alert_settings_json(true, "schedule-secret", 24),
        )
        .status,
        200
    );
    let (_agent_id, source_id, target_id) = rig.seed();
    let datasources = (source_id, target_id);
    let task_id = create_scheduled_task(&rig, "高频任务", "HOLDINGS", &datasources, "* * * * *");
    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(started.status, 202, "{}", started.body_text());

    scheduler_pass(&rig, "2026-08-28 03:10");
    let first_at = chrono::DateTime::parse_from_rfc3339("2026-08-31T10:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    for (minute, at) in [
        (11, first_at),
        (12, first_at + chrono::Duration::minutes(30)),
        (13, first_at + chrono::Duration::hours(1)),
        (
            14,
            first_at + chrono::Duration::hours(1) + chrono::Duration::seconds(1),
        ),
    ] {
        rig.clock.set(at);
        scheduler_pass(&rig, &format!("2026-08-28 03:{minute}"));
    }

    let listed = rig.json(&rig.get(&format!("/api/runs?task_id={task_id}")));
    let mut skipped = listed
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["scheduled_refusal_reason"] == "PREVIOUS_RUN_ACTIVE")
        .collect::<Vec<_>>();
    skipped.sort_by_key(|row| row["started_at"].as_str().unwrap());
    assert_eq!(skipped.len(), 4, "{listed}");
    assert_eq!(
        skipped
            .iter()
            .map(|row| row["alert"]["delivery_state"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["PENDING", "SUPPRESSED", "PENDING", "SUPPRESSED"],
        "a candidate exactly one hour later is deliverable and starts a new window"
    );
    let alert_ids = skipped
        .iter()
        .map(|row| row["alert"]["alert_id"].as_str().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(alert_ids.len(), 4, "each occurrence needs distinct Alert evidence");
    assert!(skipped.iter().all(|row| row.get("recipient").is_none()));
    assert_eq!(
        rig.alert_outbox
            .run_due_attempts(
                &rig.email_alerts,
                rig.mail_transport.as_ref(),
                rig.clock.as_ref(),
            )
            .unwrap(),
        4,
        "only the two non-suppressed occurrences, each with two recipients, are mailed"
    );
    assert_eq!(rig.mail_transport.sent.lock().unwrap().len(), 4);
}

/// 额度满时**在 source 侧排队**，不推给代理去吃 `RUN_QUOTA_EXCEEDED`——
/// 而且排着的那一条在界面上看得见它在等什么（`GET /api/schedule`）。
#[test]
fn an_occurrence_over_the_agent_quota_waits_in_a_queue_the_interface_can_see() {
    let rig = Rig::new();
    let (_agent_id, source_id, target_id) = rig.seed();
    let datasources = (source_id, target_id);
    // 桩 agent 自报的额度是 1，所以第一个任务一开跑，第二个就只能等。
    let busy = rig.create_task("占额度的", "BUSY", &datasources);
    let waiting = create_scheduled_task(&rig, "排队的", "HOLDINGS", &datasources, "* * * * *");

    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{busy}"}}"#));
    assert_eq!(started.status, 202, "{}", started.body_text());

    scheduler_pass(&rig, "2026-08-28 03:10");
    scheduler_pass(&rig, "2026-08-28 03:11");

    // 没被推给代理：这个任务一条运行历史都没有。
    let listed = rig.json(&rig.get(&format!("/api/runs?task_id={waiting}")));
    assert!(
        listed.as_array().unwrap().is_empty(),
        "额度满时不该发出去，也不该记成一次失败：{listed}"
    );

    let schedule = rig.json(&rig.get("/api/schedule"));
    let queued = schedule["queued"].as_array().unwrap();
    assert_eq!(queued.len(), 1, "{schedule}");
    assert_eq!(queued[0]["task_id"], waiting);
    assert_eq!(queued[0]["task_name"], "排队的");
    assert_eq!(queued[0]["due_at"], "2026-08-28 03:11");
    assert!(
        queued[0]["waiting_reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("并发额度已满")),
        "排队的那一条要说得出它在等什么：{schedule}"
    );
    // 时区写出来，与「下次触发」预览同一份口径。
    assert!(schedule["utc_offset"].as_str().is_some());
    assert!(schedule["next_fire_times"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["task_id"] == waiting.as_str()));
}
