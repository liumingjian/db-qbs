//! source 的 HTTP 面，**进程内直调**。
//!
//! 这里一条测试都不 spawn 二进制、不开 socket：`Api::handle(&Request) -> Response`
//! 就是全部入口。`source_skeleton.rs` 里那几条哨兵仍然走真进程，证的是「二进制起得来、
//! 对外真的在服务」；判断怎么回，归这里。

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use db_qbs_source::http::{routes, Api, Method, Request, Response, RunState};
use db_qbs_source::{
    AgentStore, DatasourceStore, HistoryStore, OracleAccess, OracleRowSource, SourceColumn,
    SourceConfig, SourceReadError, TaskSpec, TaskStore,
};
use serde_json::Value;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// 一台**在应答**的目标端 agent 桩：只认 `/v1/agent/info`，别的一律 503。
///
/// 与 `source_skeleton.rs` 里那台同一个形状——注册 agent、探测、目标端元数据这几条
/// 都得先过它，一台不在线的 agent 会让它们统统停在「agent 不在线」那一步。
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
        url
    })
    .as_str()
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
    runs: Arc<Mutex<RunState>>,
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
        Self {
            config_path: directory.join("source.toml"),
            tasks: TaskStore::open(&directory).unwrap(),
            datasources: DatasourceStore::open(&directory).unwrap(),
            agents: Arc::new(Mutex::new(AgentStore::open(&directory).unwrap())),
            history: HistoryStore::open(&directory).unwrap(),
            runs: Arc::new(Mutex::new(RunState::default())),
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
            runs: &self.runs,
            describe_source,
        }
    }

    fn send(&self, method: Method, url: &str, body: &str) -> Response {
        self.api()
            .handle(&Request::new(method, url, body.as_bytes().to_vec()))
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
        self.api_with_describer(describe_source).handle(&Request::new(
            Method::Post,
            url,
            body.as_bytes().to_vec(),
        ))
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
        let response = self.post(
            "/api/agents",
            &format!(r#"{{"name":"{name}","base_url":"{}"}}"#, agent_stub_url()),
        );
        assert_eq!(response.status, 201, "{}", response.body_text());
        self.json(&response)["agent_id"].as_str().unwrap().to_owned()
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
        r#"{{"name":"{name}","source_datasource_id":"{source_datasource_id}","target_datasource_id":"{target_datasource_id}","spec":{{"owner":"APP","table":"HOLDINGS","target_table":"{target_table}","columns":[{{"source":"ID","target":"ID"}},{{"source":"D_BIZ","target":"D_BIZ"}}],"primary_key":["ID"],"where_clause":"D_BIZ = DATE '2026-08-14'"}}}}"#
    )
}

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

    let started = rig.post("/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#));
    assert_eq!(started.status, 202, "{}", started.body_text());
    let run_record_id = rig.json(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();

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
            format!(r#"{{"source_datasource_id":"{source_id}","target_datasource_id":"{target_id}","target_table":"HOLDINGS","spec":{{"owner":"APP","table":"HOLDINGS","target_table":"HOLDINGS","columns":[{{"source":"ID","target":"ID"}}],"primary_key":["ID"]}}}}"#),
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
        (
            Method::Post,
            "/api/runs/{}/cleanup",
            format!("/api/runs/{run_record_id}/cleanup"),
            String::new(),
            409,
        ),
        (Method::Get, "/api/runs", "/api/runs".into(), String::new(), 200),
        (
            Method::Get,
            "/api/runs/{}",
            format!("/api/runs/{run_record_id}"),
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
            format!("/api/tasks/{task_id}"),
            String::new(),
            200,
        ),
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
    let request = Request::new(
        Method::Get,
        &format!("/api/tasks/{task_id}/curl"),
        Vec::new(),
    )
    .with_header("Host", "qbs.example.test:8443")
    .with_header("X-Forwarded-Proto", "https");

    let response = rig.api().handle(&request);

    assert_eq!(response.status, 200, "{}", response.body_text());
    assert_eq!(
        rig.json(&response)["command"],
        format!(
            "curl --request POST 'https://qbs.example.test:8443/api/runs' --header 'Content-Type: application/json' --data '{{\"task_id\":\"{task_id}\"}}'"
        )
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
    let request = Request::new(
        Method::Get,
        &format!("/api/tasks/{task_id}/curl"),
        Vec::new(),
    )
    .with_header("Host", "qbs.example.test/'bad");
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
        r#"{"source_datasource_id":"missing","spec":{"owner":"","table":"","target_table":"","primary_key":[],"columns":[]},"limit":10}"#,
    );
    assert_eq!(incomplete.status, 400);
    assert!(incomplete.body_text().contains("owner"));

    let invalid_sql = rig.post(
        "/api/builder/preview",
        r#"{"source_datasource_id":"missing","spec":{"source_sql":"DELETE FROM APP.T","owner":"","table":"","target_table":"T","primary_key":["ID"],"columns":[{"source":"ID","target":"ID"}]},"limit":10}"#,
    );
    assert_eq!(invalid_sql.status, 400);
    assert!(invalid_sql.body_text().contains("SELECT"));

    let zero = rig.post(
        "/api/builder/preview",
        r#"{"source_datasource_id":"missing","spec":{"source_sql":"SELECT ID FROM APP.T","owner":"","table":"","target_table":"T","primary_key":["ID"],"columns":[{"source":"ID","target":"ID"}]},"limit":0}"#,
    );
    assert_eq!(zero.status, 400);
    assert!(zero.body_text().contains("limit 必须大于 0"));

    let custom_sql = rig.post(
        "/api/builder/preview",
        r#"{"source_datasource_id":"missing","spec":{"source_sql":"SELECT ID FROM APP.T","owner":"","table":"","target_table":"T","primary_key":["ID"],"columns":[{"source":"ID","target":"ID"}]},"limit":1000}"#,
    );
    assert_eq!(custom_sql.status, 400);
    assert!(custom_sql.body_text().contains("数据源 missing 不存在"));
}

/// 请求体超过 1 MiB 时的那句话，判定只有一处。
#[test]
fn oversized_request_bodies_are_refused() {
    let rig = Rig::new();
    let huge = format!(r#"{{"name":"{}"}}"#, "x".repeat(1024 * 1024 + 16));
    let response = rig.post("/api/tasks", &huge);
    assert_eq!(response.status, 400);
    assert!(
        response.body_text().contains("请求体超过 1 MiB"),
        "{}",
        response.body_text()
    );
}

/// 任务定义的 CRUD：线上恰好三样，口令一个字节都不出现。
///
/// （「重启之后还在」那一半留在 `source_skeleton.rs` 的哨兵里——那要一个真进程。）
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
        r#"{"spec":{"owner":"APP","table":"HOLDINGS","target_table":"HOLDINGS","columns":[{"source":"ID","target":"ID"}],"primary_key":["ID"]}}"#,
    );
    assert_eq!(missing_name.status, 400, "{}", missing_name.body_text());
    assert_eq!(rig.json(&rig.get("/api/tasks")), serde_json::json!([]));
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
    for field in ["owner", "table", "columns", "primary_key", "where_clause"] {
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
    let task_id = rig.create_task(
        "holdings",
        "HOLDINGS",
        &(source_id.clone(), target_id.clone()),
    );

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
            "columns":[{"source":"ID","target":"ID"}],"primary_key":["MISSING"]
          }
        }"#,
    );

    assert_eq!(response.status, 400, "{}", response.body_text());
    let body = rig.json(&response);
    assert_eq!(body["kind"], "request");
    assert!(body.get("code").is_none());
    assert!(body.get("run_id").is_none());
    assert!(body["message"].as_str().unwrap().contains("MISSING"));
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
          "primary_key":["D_BIZ"],
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
            "columns":[{{"source":"ID","target":"ID"}},{{"source":"BIZ_DAY","target":"BIZ_DAY"}}],"primary_key":["ID"]
          }}
        }}"#
        ),
    );

    assert_eq!(response.status, 502, "{}", response.body_text());
    let body = rig.json(&response);
    assert_eq!(body["kind"], "oracle");
    assert!(body.get("run_id").is_none());
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
        assert_eq!(parsed["kind"], "sink");
        // 不属于任何 run：回话里没有 run_id。
        assert!(parsed.get("run_id").is_none(), "{}", response.body_text());
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
    let request = format!(r#"{{"source_datasource_id":"{source_id}","target_datasource_id":"{target_id}","target_table":"HOLDINGS","spec":{{"owner":"APP","table":"HOLDINGS","target_table":"HOLDINGS","columns":[{{"source":"ID","target":"ID"}}],"primary_key":["ID"]}}}}"#);

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
    let check_body = |target_id: &str| format!(r#"{{"source_datasource_id":"{source_id}","target_datasource_id":"{target_id}","target_table":"HOLDINGS","spec":{{"owner":"APP","table":"HOLDINGS","target_table":"HOLDINGS","columns":[{{"source":"ID","target":"ID"}}],"primary_key":["ID"]}}}}"#);
    let invalid_target = check_body(&source_id);
    let wrong_kind = rig.post_with_describer("/api/target/check", &invalid_target, described_id);
    assert_eq!(wrong_kind.status, 400, "{}", wrong_kind.body_text());

    let agent_id = rig.register_agent("拒绝检查的目标端");
    let target_id = rig.create_mysql_datasource("目标库", &agent_id);
    let sink_failure = check_body(&target_id);
    let sink = rig.post_with_describer("/api/target/check", &sink_failure, described_id);
    assert_eq!(sink.status, 502, "{}", sink.body_text());
    assert_eq!(rig.json(&sink)["kind"], "sink");

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
    assert_eq!(rig.json(&agent)["kind"], "agent");
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
    assert_eq!(parsed["kind"], "sink");
    assert!(parsed.get("run_id").is_none(), "{}", mysql_draft.body_text());

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
        assert_eq!(parsed["kind"], "agent", "{path}: {}", response.body_text());
        assert!(
            parsed["message"].as_str().unwrap().contains("不在线"),
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
