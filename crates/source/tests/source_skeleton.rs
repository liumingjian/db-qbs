//! source 长驻进程的**哨兵**：只证「二进制真的起得来、对外真的在服务」。
//!
//! 判断怎么回话——路由、30 个 handler、发起运行那条链——全在 `tests/api.rs` 里
//! 进程内直调 `Api::handle` 证完了，那边 27 条测试跑不到 2 秒。留在这里的，
//! 是**只有一个真进程才有意义**的那几样：监听与端口释放、SIGTERM、
//! 启动期的配置迁移、重启时对未完成运行的封存。

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use db_qbs_source::{
    AlertOutboxStore, EmailAlertSettingsInput, EmailAlertStore, EmailDeliveryState,
    EmailProviderPreset, HistoryStore, RunHistory, RunTrigger, SmtpSecurity,
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// 一台**真的在应答**的目标端 agent 桩（ADR-0044）。
///
/// 本文件里几乎每条链路都要先过 agent：目标端测连、取表、取列、发起运行，
/// 一台不在线的 agent 会让它们统统停在「agent 不在线」这一步，测不到后面的东西。
/// 所以给整个测试二进制起**一台**桩，跑完随进程退出。
///
/// 它只认 `/v1/agent/info`，别的路径一律 503——目标端那些端点归 sink 台架去证，
/// 这里要的只是「agent 活着」这一个事实；顺带让「agent 活着但目标库不通」
/// 这条路径仍然测得到（那时候回的是 502 kind=sink）。
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
                let body = if request_line.contains("/v1/agent/info") {
                    r#"{"agent_id":"stub-agent","name":"桩 agent","version":"0.0.0-test"}"#
                } else {
                    r#"{"error":{"code":"BAD_REQUEST","message":"桩只认 /v1/agent/info","run_id":null,"details":{}}}"#
                };
                let status = if request_line.contains("/v1/agent/info") {
                    "200 OK"
                } else {
                    "503 Service Unavailable"
                };
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

/// 一台**连得上、永不回话**的 agent 桩：TCP 握得上手，读那一头一直空着，
/// 于是 `fetch_agent_info` 会一路撑到它 5 秒的读超时（`agent.rs`）。
///
/// 它是本文件里唯一一处「能把一条请求按住好几秒」的手段，而这正是
/// 多线程 accept（#255）那条哨兵需要的东西。
static BLACK_HOLE_ACCEPTED: AtomicU64 = AtomicU64::new(0);

fn black_hole_agent_url() -> &'static str {
    static STUB: OnceLock<String> = OnceLock::new();
    STUB.get_or_init(|| {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        thread::spawn(move || {
            // 攥着不放：不读、不写、不关。一关连接对端就立刻拿到 EOF，按不住了。
            let mut held = Vec::new();
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                BLACK_HOLE_ACCEPTED.fetch_add(1, Ordering::SeqCst);
                held.push(stream);
            }
        });
        url
    })
    .as_str()
}

fn black_hole_smtp() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

fn wait_for_smtp_connection(listener: &TcpListener) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "outbox worker did not connect to SMTP"
                );
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("accepting SMTP connection failed: {error}"),
        }
    }
}

/// **accept 循环是多线程的**（#255）：一个客户端卡在 5 秒的 agent 探测里，
/// 另一个客户端拉任务列表照样秒回。
///
/// 这条哨兵只有真进程才成立——`tests/api.rs` 那边直调 `Api::handle`，
/// 证得了「锁不跨越阻塞 IO」，证不了「监听器后面站着几条线程」。
/// 改回单线程 accept 会让它在「列表等了 5 秒」上失败，而不是悄悄退化。
#[test]
fn a_client_stuck_on_an_unresponsive_agent_does_not_freeze_another_client() {
    let directory = temp_directory();
    let black_hole = black_hole_agent_url().to_owned();
    let (port, _config, source, _ready) = start_source_ready(|port| {
        write_config_with_oracle(
            &directory,
            &format!("127.0.0.1:{port}"),
            &black_hole,
            "/opt/oracle",
        )
    });
    // 迁移出来的那台 agent 指着黑洞；迁移本身不探测，所以启动没被它拖住。
    let agent_id = migrated_agent_id(port);

    let before = BLACK_HOLE_ACCEPTED.load(Ordering::SeqCst);
    let probing = thread::spawn(move || {
        let started = Instant::now();
        let response = post(port, &format!("/api/agents/{agent_id}/probe"), "").unwrap();
        (response.status, started.elapsed())
    });
    // 等探测那一发真的落到黑洞上再计时，否则量到的只是线程还没起来。
    let deadline = Instant::now() + Duration::from_secs(5);
    while BLACK_HOLE_ACCEPTED.load(Ordering::SeqCst) == before {
        assert!(Instant::now() < deadline, "探测请求一直没发出去");
        thread::sleep(Duration::from_millis(10));
    }

    let started = Instant::now();
    let listed = get(port, "/api/tasks").unwrap();
    let elapsed = started.elapsed();
    assert_eq!(listed.status, 200, "{}", listed.body);
    assert!(
        elapsed < Duration::from_secs(2),
        "探测把任务列表一起冻住了：列表等了 {elapsed:?}"
    );

    let (status, probe_elapsed) = probing.join().unwrap();
    assert_eq!(status, 200);
    assert!(
        probe_elapsed >= Duration::from_secs(4),
        "探测根本没被按住（{probe_elapsed:?}），这条哨兵没在证它该证的事"
    );

    assert_success(&terminate(source));
    let _ = fs::remove_dir_all(directory);
}

/// 注册表里那台由 `sink_base_url` 迁移出来的 agent（ADR-0044 §5）。
/// 本文件的 `source.toml` 都带着那个字段，所以首启之后它一定在。
fn migrated_agent_id(port: u16) -> String {
    let agents = json_body(&get(port, "/api/agents").unwrap());
    agents.as_array().unwrap()[0]["agent_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// 建任务前先备好两端数据源（ADR-0037 §8）。Oracle 那条由 `source.toml` 的退役字段
/// 首启迁移出来（§10）；MySQL 那条这里现建——它不必连得上，本文件不跑真 MySQL，
/// 但**必须绑一台已注册的 agent**（ADR-0044 §1），绑的就是迁移出来的那条。
fn seed_datasources(port: u16) -> (String, String) {
    let listed = json_body(&get(port, "/api/datasources").unwrap());
    let source_id = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|datasource| datasource["kind"] == "oracle")
        .expect("首启迁移必须留下一条 Oracle 数据源")["datasource_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let agent_id = migrated_agent_id(port);
    let created = post(
        port,
        "/api/datasources",
        &format!(
            r#"{{"name":"目标库","kind":"mysql","agent_id":"{agent_id}","host":"127.0.0.1","port":3306,"username":"sink","password":"change-me","database":"qbs"}}"#
        ),
    )
    .unwrap();
    assert_eq!(created.status, 201, "{}", created.body);
    let target_id = json_body(&created)["datasource_id"]
        .as_str()
        .unwrap()
        .to_owned();
    (source_id, target_id)
}

/// 一份任务定义的请求体。规格是唯一真相源，过滤是一段自由 WHERE 文本。
fn task_json(name: &str, target_table: &str, datasources: &(String, String)) -> String {
    let (source_datasource_id, target_datasource_id) = datasources;
    format!(
        r#"{{"name":"{name}","source_datasource_id":"{source_datasource_id}","target_datasource_id":"{target_datasource_id}","spec":{{"owner":"APP","table":"HOLDINGS","target_table":"{target_table}","columns":[{{"source":"ID","target":"ID"}},{{"source":"D_BIZ","target":"D_BIZ"}}],"write_mode":"APPEND","schedule_enabled":false,"primary_key":["ID"],"where_clause":"D_BIZ = DATE '2026-08-14'"}}}}"#
    )
}

#[test]
fn restart_cleanup_distinguishes_graceful_restart_from_process_disappearance() {
    let directory = temp_directory();
    let fake_child = write_fake_child(
        &directory,
        r#"printf '%s\n' '{"ts":"2026-08-15T12:00:00.000Z","level":"info","event":"source_started","run_id":null,"task":null}'
printf '%s\n' '{"ts":"2026-08-15T12:00:01.000Z","level":"info","event":"stage_changed","run_id":"run-restart","task":null,"stage":"PREPARING"}'
sleep 2
"#,
    );
    let (port, config, source, _ready) =
        start_source_ready(|port| write_run_config(&directory, port, &fake_child));
    let task_id = create_task(port);
    let settings = email_alert_settings(false, 1, "restart-alerts@example.com");
    assert_eq!(
        put(port, "/api/email-alert-settings", &settings)
            .unwrap()
            .status,
        200
    );

    let graceful_record_id = start_run(port, &task_id);
    wait_for_json(port, &format!("/api/runs/{graceful_record_id}"), |body| {
        body["stage"] == "PREPARING"
    });
    assert_success(&terminate(source));

    let restarted = restart_source(&config, port);
    let graceful = json_body(&get(port, &format!("/api/runs/{graceful_record_id}")).unwrap());
    assert_eq!(graceful["live"], false);
    assert_eq!(graceful["unknown_reason"], "SERVICE_RESTARTED");
    assert_eq!(graceful["message"], "服务重启，结局未知");
    assert_eq!(graceful["source_code"], Value::Null);
    assert_eq!(graceful["target_table_effect"], Value::Null);
    assert_eq!(
        graceful["alert"]["alert_id"],
        format!("alert-{graceful_record_id}")
    );
    assert_eq!(graceful["alert"]["delivery_state"], "NOT_SENT");
    let graceful_deliveries = AlertOutboxStore::open(&directory)
        .unwrap()
        .delivery_history(Some(&graceful_record_id))
        .unwrap();
    assert_eq!(graceful_deliveries.len(), 1);
    assert_eq!(
        graceful_deliveries[0].recipient,
        "restart-alerts@example.com"
    );
    assert_eq!(graceful_deliveries[0].state, EmailDeliveryState::NotSent);

    let disappeared_record_id = start_run(port, &task_id);
    wait_for_json(
        port,
        &format!("/api/runs/{disappeared_record_id}"),
        |body| body["stage"] == "PREPARING",
    );
    let killed = kill_source(restarted, "-KILL");
    assert!(!killed.status.success(), "{}", output_text(&killed));

    let after_kill = restart_source(&config, port);
    let disappeared = json_body(&get(port, &format!("/api/runs/{disappeared_record_id}")).unwrap());
    assert_eq!(disappeared["live"], false);
    assert_eq!(disappeared["unknown_reason"], "PROCESS_DISAPPEARED");
    assert_eq!(disappeared["message"], "进程消失，无终态日志");
    assert_eq!(disappeared["sink_code"], Value::Null);
    assert_eq!(disappeared["target_table_effect"], Value::Null);
    assert_eq!(
        disappeared["alert"]["alert_id"],
        format!("alert-{disappeared_record_id}")
    );
    assert_eq!(disappeared["alert"]["delivery_state"], "NOT_SENT");
    let disappeared_deliveries = AlertOutboxStore::open(&directory)
        .unwrap()
        .delivery_history(Some(&disappeared_record_id))
        .unwrap();
    assert_eq!(disappeared_deliveries.len(), 1);
    assert_eq!(
        disappeared_deliveries[0].recipient,
        "restart-alerts@example.com"
    );
    assert_eq!(disappeared_deliveries[0].state, EmailDeliveryState::NotSent);
    wait_for_empty_directory(&directory.join("run-tasks"));

    assert_success(&terminate(after_kill));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn pending_delivery_resumes_after_process_restart_and_blocked_smtp_does_not_delay_sigterm() {
    let directory = temp_directory();
    let (smtp_listener, smtp_port) = black_hole_smtp();
    EmailAlertStore::open(&directory)
        .unwrap()
        .update(email_alert_settings_input(
            true,
            smtp_port,
            "snapshotted@example.com",
        ))
        .unwrap();
    let accepted_at = chrono::Utc::now();
    let mut failed = RunHistory::accepted(
        "persisted-delivery",
        "restart-task",
        "SELECT sensitive_marker FROM payroll",
        accepted_at,
    );
    failed.task_name = "Restart recovery".to_owned();
    failed.trigger = RunTrigger::Manual.as_str().to_owned();
    failed.outcome = Some("FAILED".to_owned());
    failed.finished_at = Some(accepted_at.to_rfc3339());
    failed.failure_kind = Some("NETWORK".to_owned());
    HistoryStore::open(&directory)
        .unwrap()
        .finalize(&failed, accepted_at, 90)
        .unwrap();

    let (port, config, first, _ready) =
        start_source_ready(|port| write_config(&directory, &format!("127.0.0.1:{port}")));
    let first_connection = wait_for_smtp_connection(&smtp_listener);
    let first_recipient = AlertOutboxStore::open(&directory)
        .unwrap()
        .delivery_history(Some("persisted-delivery"))
        .unwrap()
        .remove(0);
    assert_eq!(first_recipient.alert_id, "alert-persisted-delivery");
    assert_eq!(first_recipient.recipient, "snapshotted@example.com");
    assert_eq!(first_recipient.state, EmailDeliveryState::Pending);

    let killed = kill_source(first, "-KILL");
    assert!(!killed.status.success(), "{}", output_text(&killed));
    drop(first_connection);

    let restarted = restart_source(&config, port);
    let second_connection = wait_for_smtp_connection(&smtp_listener);
    let resumed = AlertOutboxStore::open(&directory)
        .unwrap()
        .delivery_history(Some("persisted-delivery"))
        .unwrap()
        .remove(0);
    assert_eq!(resumed.alert_id, "alert-persisted-delivery");
    assert_eq!(resumed.recipient, "snapshotted@example.com");
    assert_eq!(resumed.attempt_count, 0);

    let started = Instant::now();
    let output = terminate(restarted);
    assert_success(&output);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "blocked SMTP delayed graceful shutdown for {:?}: {}",
        started.elapsed(),
        output_text(&output)
    );
    drop(second_connection);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn tasks_endpoint_is_ready_and_sigterm_allows_same_port_restart() {
    let directory = temp_directory();
    let (port, config, first, response) =
        start_source_ready(|port| write_config(&directory, &format!("127.0.0.1:{port}")));
    assert_eq!(response.status, 200);
    assert_eq!(response.body, "[]");
    assert_eq!(get(port, "/health").unwrap().status, 404);
    let first_output = terminate(first);
    assert_success(&first_output);
    let first_lines = json_lines(&first_output.stdout);
    assert!(first_lines.iter().any(|line| {
        line["level"] == "info"
            && line["event"] == "source_started"
            && line["listen"] == format!("127.0.0.1:{port}")
    }));

    let mut second = start_source(&config);
    let response = wait_for_tasks(port, &mut second);
    assert_eq!(response.status, 200);
    assert_eq!(response.body, "[]");

    // 任务定义**熬得过一次重启**，而且库文件只有属主读得了。
    let datasources = seed_datasources(port);
    let created = post(
        port,
        "/api/tasks",
        &task_json("持仓明细", "HOLDINGS", &datasources),
    )
    .unwrap();
    assert_eq!(created.status, 201, "{}", created.body);
    let created = json_body(&created);
    assert_success(&terminate(second));
    let database = directory.join("db-qbs.sqlite3");
    assert_eq!(
        fs::metadata(&database).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let mut third = start_source(&config);
    let listed = wait_for_tasks(port, &mut third);
    assert_eq!(json_body(&listed), serde_json::json!([created]));
    assert!(!listed.body.contains("secret"));
    assert_success(&terminate(third));

    fs::remove_dir_all(directory).unwrap();
}

/// `source.toml` 里那两组退役字段的**首启迁移**（ADR-0037 §10 / ADR-0044 §5）。
///
/// 它只在 `serve()` 起来的那一刻跑一次，`Api` 上没有这条路径——所以它是哨兵，
/// 不是进程内测试。
#[test]
fn first_boot_migrates_the_retired_config_fields() {
    let directory = temp_directory();
    let (port, _config, child, _ready) =
        start_source_ready(|port| write_config(&directory, &format!("127.0.0.1:{port}")));

    // `sink_base_url` 迁成一台名为「默认」的 agent，身份还空着（没探过）。
    let agents = json_body(&get(port, "/api/agents").unwrap());
    assert_eq!(agents.as_array().unwrap().len(), 1, "{agents}");
    assert_eq!(agents[0]["name"], "默认");
    assert_eq!(agents[0]["base_url"], agent_stub_url());

    // `oracle_*` 那三样迁成一条名为「默认」的 Oracle 数据源，口令不出现在线上。
    let datasources = json_body(&get(port, "/api/datasources").unwrap());
    let oracle = datasources
        .as_array()
        .unwrap()
        .iter()
        .find(|datasource| datasource["kind"] == "oracle")
        .expect("首启迁移必须留下一条 Oracle 数据源");
    assert_eq!(oracle["connect_string"], "//oracle:1521/XE");
    assert!(!datasources.to_string().contains("secret"));

    assert_success(&terminate(child));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn empty_listen_fails_with_a_readable_reason() {
    let directory = temp_directory();
    let config = write_config(&directory, "");

    let output = start_source(&config).wait_with_output().unwrap();

    assert_eq!(output.status.code(), Some(1));
    let output_text = output_text(&output);
    assert!(output_text.contains("listen"), "{output_text}");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn non_loopback_listen_emits_the_required_warning() {
    let directory = temp_directory();
    let (port, _config, child, response) =
        start_source_ready(|port| write_config(&directory, &format!("0.0.0.0:{port}")));
    assert_eq!(response.status, 200);
    let output = terminate(child);
    let lines = json_lines(&output.stdout);
    let address = format!("0.0.0.0:{port}");
    // 按 `listen` 字段挑，不能按「第一条 warn」挑：退役字段的首启迁移告警（ADR-0037 §10）
    // 排在这条之前，且它不带 `listen`。用告警自己的措辞去挑则是循环论证。
    let warning = lines
        .iter()
        .find(|line| line["level"] == "warn" && line["listen"] == address)
        .unwrap_or_else(|| panic!("没有带 listen={address} 的 warn 行：{lines:?}"));
    let message = warning["message"].as_str().unwrap();
    // 措辞按 ADR-0037 §5 ③ 加码：暴露面从「该 source 的」放大到「全部已配置数据源」，
    // 这句话本身是那条裁定的唯一可观测物，必须钉住，连带两个「任一」的量词。
    //
    // 装上登录之后这条**没有作废**，只是多了两个前提：一是全新部署仍在用出厂口令
    // （所以「能连上」与「进得来」之间只隔两次输入），二是 sink 那半边根本没有门。
    // 两句都得在，否则这条警告会变成一句「已经有鉴权了」的假安慰。
    for phrase in [
        "已要求登录",
        "出厂口令",
        "全部已配置数据源",
        "任一源库跑任意 SQL",
        "清空重写任一目标表",
        "sink 仍然无鉴权",
        "运行历史含源库真实业务值",
        address.as_str(),
    ] {
        assert!(
            message.contains(phrase),
            "missing {phrase:?} in {message:?}"
        );
    }

    fs::remove_dir_all(directory).unwrap();
}


fn start_source(config: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_db-qbs-source"))
        .args(["--config"])
        .arg(config)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn terminate(child: Child) -> Output {
    kill_source(child, "-TERM")
}

fn kill_source(child: Child, signal: &str) -> Output {
    let kill_status = Command::new("kill")
        .args([signal, &child.id().to_string()])
        .status()
        .unwrap();
    assert!(kill_status.success());
    child.wait_with_output().unwrap()
}

/// 起一个 source 并等它就绪；端口被别人占走就换个号从头再来。
///
/// 端口没法在测试里真正预定：`unused_port()` 把探测用的 listener 交还，到 source 走完启动前的
/// 磁盘活（开两个 SQLite store、seal_incomplete、clean_run_tasks）真正 bind，中间是一段空窗。
/// 空窗里抢号的有三路：同一个测试二进制里并行跑的其它测试发出的 HTTP 请求（`TcpStream::connect`
/// 的本地端口同样从临时端口池里取）、上一轮跑残下来还在监听的 source 进程、以及被 SIGKILL 的
/// source 留下的、还没释放干净的监听套接字。三路的症状是同一句
/// 「监听 … 失败：Address already in use」、退出码 1，不是「5 秒没等到」——所以修法是换号重来，
/// 拉长等待没有用。
fn start_source_ready(
    build_config: impl Fn(u16) -> PathBuf,
) -> (u16, PathBuf, Child, HttpResponse) {
    for attempt in 0..8 {
        let port = unused_port();
        let config = build_config(port);
        let mut child = start_source(&config);
        match try_wait_for_tasks(port, &mut child) {
            Ok(response) => return (port, config, child, response),
            Err(error) if error.contains("Address already in use") => assert!(
                attempt < 7,
                "连续 8 个端口都在 source bind 之前被占走：{error}"
            ),
            Err(error) => panic!("{error}"),
        }
    }
    unreachable!()
}

/// 同端口重启 source。被 SIGKILL 的上一个 source 偶尔会把监听套接字多留一会儿，
/// 这里只等它释放，不换端口——换了端口就测不到「重启后接着读同一份历史」这件事。
fn restart_source(config: &Path, port: u16) -> Child {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut child = start_source(config);
        match try_wait_for_tasks(port, &mut child) {
            Ok(_) => return child,
            Err(error) if error.contains("Address already in use") => assert!(
                Instant::now() < deadline,
                "端口迟迟没释放，同端口重启失败：{error}"
            ),
            Err(error) => panic!("{error}"),
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_tasks(port: u16, child: &mut Child) -> HttpResponse {
    match try_wait_for_tasks(port, child) {
        Ok(response) => response,
        Err(error) => panic!("{error}"),
    }
}

/// source 起来没有；起不来时把它的诊断信息带回去，好让调用方决定换号重试还是直接判失败。
fn try_wait_for_tasks(port: u16, child: &mut Child) -> Result<HttpResponse, String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            // source 的失败原因走 emit()，落在 stdout 上，stderr 通常是空的。
            let mut stdout = String::new();
            let mut stderr = String::new();
            if let Some(mut pipe) = child.stdout.take() {
                let _ = pipe.read_to_string(&mut stdout);
            }
            if let Some(mut pipe) = child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            return Err(format!(
                "source exited before readiness with {status}; stdout={stdout} stderr={stderr}"
            ));
        }
        if let Ok(response) = get(port, "/api/tasks") {
            return Ok(response);
        }
        if Instant::now() >= deadline {
            return Err(format!("source did not become ready on port {port}"));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn get(port: u16, path: &str) -> std::io::Result<HttpResponse> {
    request(port, "GET", path, None)
}

/// 一次请求的三段（连、写、读）各自的上限。
///
/// 取 15 秒的依据：本文件里最慢的一档是 `/api/target/*` 与 `/api/columns`——它们要往 sink 或
/// Oracle 发连接，而这两处在测试里都是打不通的（sink 端口空着、Oracle 客户端库路径是假的），
/// 失败在秒级内就回；其余端点是纯本地 SQLite，亚秒级。15 秒明显大于正常值，又明显小于 CI 的
/// 外层耐心（分钟级），所以卡住时是本进程自己报出「哪条请求超时」，而不是被外层砍掉。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 一次**带会话**的请求。`/api/*` 现在整片要求登录，所以这里先换一张票再发。
///
/// 票据按端口缓存：本文件里一个端口就是一台 source，而会话是落库的，
/// 重启同一个端口上的 source 之后那张票仍然认得。撞上 401 就把缓存丢掉重来一次——
/// 端口在测试之间会被回收，缓存里那张票可能属于上一台 source。
fn request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> std::io::Result<HttpResponse> {
    let cookie = session_cookie(port)?;
    let response = raw_request(port, method, path, body, Some(&cookie))?;
    if response.status != 401 {
        return Ok(response);
    }
    forget_session_cookie(port);
    let cookie = session_cookie(port)?;
    raw_request(port, method, path, body, Some(&cookie))
}

fn session_cache() -> &'static Mutex<HashMap<u16, String>> {
    static CACHE: OnceLock<Mutex<HashMap<u16, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn forget_session_cookie(port: u16) {
    session_cache().lock().unwrap().remove(&port);
}

/// 登录换票。**出厂口令是 `admin / admin`**，而且它长期有效——
/// 这里直接用它，正是因为产品不强制改。
fn session_cookie(port: u16) -> std::io::Result<String> {
    if let Some(cookie) = session_cache().lock().unwrap().get(&port) {
        return Ok(cookie.clone());
    }
    let response = raw_request(
        port,
        "POST",
        "/api/session",
        Some(r#"{"username":"admin","password":"admin"}"#),
        None,
    )?;
    if response.status != 200 {
        return Err(std::io::Error::other(format!(
            "登录 source 失败（HTTP {}）：{}",
            response.status, response.body
        )));
    }
    let cookie = response
        .set_cookie
        .as_deref()
        .and_then(|header| header.split(';').next())
        .ok_or_else(|| std::io::Error::other("登录成功却没有发回会话 cookie"))?
        .to_owned();
    session_cache()
        .lock()
        .unwrap()
        .insert(port, cookie.clone());
    Ok(cookie)
}

fn raw_request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    cookie: Option<&str>,
) -> std::io::Result<HttpResponse> {
    let body = body.unwrap_or("");
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    // 三段都要设：被测的 source 在某条路径上不回响应、也不关连接时，无超时的 `read_to_string`
    // 会永久阻塞——挂死的不是这一条用例，是整个测试二进制。
    let mut stream = TcpStream::connect_timeout(&address, REQUEST_TIMEOUT)
        .map_err(|error| annotate(method, path, "连接", error))?;
    stream
        .set_write_timeout(Some(REQUEST_TIMEOUT))
        .and_then(|()| stream.set_read_timeout(Some(REQUEST_TIMEOUT)))
        .map_err(|error| annotate(method, path, "设超时", error))?;
    let cookie_header = match cookie {
        Some(cookie) => format!("Cookie: {cookie}\r\n"),
        None => String::new(),
    };
    stream
        .write_all(
            format!(
                "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n{cookie_header}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .map_err(|error| annotate(method, path, "发请求", error))?;
    read_response(&mut stream).map_err(|error| annotate(method, path, "读回话", error))
}

/// 超时（以及任何 I/O 失败）的信息里必须点名是哪条 `method path` 的哪一段——
/// 否则挂死只是换成了一句「某条请求超时」，排障省不下多少。
fn annotate(method: &str, path: &str, stage: &str, error: std::io::Error) -> std::io::Error {
    std::io::Error::new(
        error.kind(),
        format!("{method} {path} 的{stage}阶段失败（上限 {REQUEST_TIMEOUT:?}）：{error}"),
    )
}

fn post(port: u16, path: &str, body: &str) -> std::io::Result<HttpResponse> {
    request(port, "POST", path, Some(body))
}

fn put(port: u16, path: &str, body: &str) -> std::io::Result<HttpResponse> {
    request(port, "PUT", path, Some(body))
}

fn read_response(stream: &mut TcpStream) -> std::io::Result<HttpResponse> {
    let mut raw = String::new();
    stream.read_to_string(&mut raw)?;
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let status = head
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let set_cookie = head
        .lines()
        .find_map(|line| line.strip_prefix("Set-Cookie: "))
        .map(str::to_owned);
    Ok(HttpResponse {
        status,
        body: body.to_owned(),
        set_cookie,
    })
}

fn json_body(response: &HttpResponse) -> Value {
    serde_json::from_str(&response.body).unwrap()
}


fn create_task(port: u16) -> String {
    let datasources = seed_datasources(port);
    let response = post(
        port,
        "/api/tasks",
        &task_json("holdings", "HOLDINGS", &datasources),
    )
    .unwrap();
    assert_eq!(response.status, 201, "{}", response.body);
    json_body(&response)["task_id"].as_str().unwrap().to_owned()
}

/// 发起一次运行。**任务身份就是全部输入**——没有对话框，也没有参数。
fn start_run(port: u16, task_id: &str) -> String {
    let response = post(port, "/api/runs", &format!(r#"{{"task_id":"{task_id}"}}"#)).unwrap();
    assert_eq!(response.status, 202, "{}", response.body);
    json_body(&response)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn write_fake_child(directory: &Path, body: &str) -> PathBuf {
    let path = directory.join("fake-source-run.sh");
    fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}")).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

fn write_run_config(directory: &Path, port: u16, run_executable: &Path) -> PathBuf {
    let path = directory.join("source.toml");
    fs::write(
        &path,
        format!(
            "oracle_connect_string = \"//oracle:1521/XE\"\n\
             oracle_username = \"source\"\n\
             oracle_password = \"secret\"\n\
             oracle_client_lib_dir = \"/opt/oracle\"\n\
             sink_base_url = \"{}\"\n\
             listen = \"127.0.0.1:{port}\"\n\
             data_dir = \"{}\"\n\
             run_executable = \"{}\"\n",
            agent_stub_url(),
            directory.display(),
            run_executable.display(),
        ),
    )
    .unwrap();
    path
}

fn wait_for_json(port: u16, path: &str, predicate: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = get(port, path).unwrap();
        if response.status == 200 {
            let body = json_body(&response);
            if predicate(&body) {
                return body;
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for {path}");
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_empty_directory(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if fs::read_dir(path).unwrap().next().is_none() {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for cleanup");
        thread::sleep(Duration::from_millis(20));
    }
}


fn write_config(directory: &Path, listen: &str) -> PathBuf {
    write_config_with_oracle(directory, listen, agent_stub_url(), "/opt/oracle")
}

fn email_alert_settings(enabled: bool, smtp_port: u16, recipient: &str) -> String {
    serde_json::json!({
        "enabled": enabled,
        "provider_preset": "GENERIC",
        "smtp_host": "127.0.0.1",
        "smtp_port": smtp_port,
        "smtp_security": "IMPLICIT_TLS",
        "smtp_username": "mailer",
        "smtp_secret": "test-only-secret",
        "sender_address": "alerts@example.com",
        "sender_name": "db-qbs alerts",
        "recipients": [recipient],
        "max_retry_hours": 24,
        "instance_name": "test",
        "external_base_url": null,
    })
    .to_string()
}

fn email_alert_settings_input(
    enabled: bool,
    smtp_port: u16,
    recipient: &str,
) -> EmailAlertSettingsInput {
    EmailAlertSettingsInput {
        enabled,
        provider_preset: EmailProviderPreset::Generic,
        smtp_host: "127.0.0.1".to_owned(),
        smtp_port,
        smtp_security: SmtpSecurity::ImplicitTls,
        smtp_username: "mailer".to_owned(),
        smtp_secret: "test-only-secret".to_owned(),
        sender_address: "alerts@example.com".to_owned(),
        sender_name: "db-qbs alerts".to_owned(),
        recipients: vec![recipient.to_owned()],
        max_retry_hours: 24,
        instance_name: "test".to_owned(),
        external_base_url: None,
    }
}

fn write_config_with_oracle(
    directory: &Path,
    listen: &str,
    sink_base_url: &str,
    oracle_client_lib_dir: &str,
) -> PathBuf {
    let path = directory.join("source.toml");
    fs::write(
        &path,
        format!(
            "oracle_connect_string = \"//oracle:1521/XE\"\n\
             oracle_username = \"source\"\n\
             oracle_password = \"secret\"\n\
             oracle_client_lib_dir = \"{oracle_client_lib_dir}\"\n\
             sink_base_url = \"{sink_base_url}\"\n\
             listen = \"{listen}\"\n\
             data_dir = \"{}\"\n",
            directory.display(),
        ),
    )
    .unwrap();
    path
}


fn unused_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "{}", output_text(output));
}

fn json_lines(stdout: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn temp_directory() -> PathBuf {
    let suffix = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "db-qbs-source-skeleton-test-{}-{suffix}",
        std::process::id()
    ));
    fs::create_dir(&path).unwrap();
    path
}

struct HttpResponse {
    status: u16,
    body: String,
    /// 登录那一条回话里的票据。别的响应上也有（每次请求都续期），这里只有登录用得着。
    set_cookie: Option<String>,
}
