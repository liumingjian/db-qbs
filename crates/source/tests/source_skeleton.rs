use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

const TASK_FIELDS: [&str; 6] = [
    "name",
    "source_date_col",
    "source_sql",
    "target_date_col",
    "target_table",
    "task_id",
];

#[test]
fn task_crud_persists_stable_identity_without_exposing_credentials() {
    let directory = temp_directory();
    let port = unused_port();
    let config = write_config(&directory, &format!("127.0.0.1:{port}"));
    let mut child = start_source(&config);
    wait_for_tasks(port, &mut child);

    let created = request(
        port,
        "POST",
        "/api/tasks",
        Some(
            r#"{"name":"持仓明细","source_sql":"SELECT ID, D_BIZ FROM HOLDINGS WHERE D_BIZ >= :biz_date AND D_BIZ < :biz_date + 1","source_date_col":"D_BIZ","target_table":"HOLDINGS","target_date_col":"D_BIZ"}"#,
        ),
    )
    .unwrap();
    assert_eq!(created.status, 201, "{}", created.body);
    let created = json_body(&created);
    assert_task_fields(&created);
    let task_id = created["task_id"].as_str().unwrap().to_owned();
    assert!(!task_id.is_empty());

    let listed = get(port, "/api/tasks").unwrap();
    assert_eq!(listed.status, 200);
    assert_eq!(json_body(&listed), serde_json::json!([created]));

    let detail = get(port, &format!("/api/tasks/{task_id}")).unwrap();
    assert_eq!(detail.status, 200);
    assert_eq!(json_body(&detail), created);

    let updated = request(
        port,
        "PUT",
        &format!("/api/tasks/{task_id}"),
        Some(
            r#"{"name":"持仓日明细","source_sql":"SELECT ID, AMOUNT, D_BIZ FROM HOLDINGS WHERE D_BIZ >= :biz_date AND D_BIZ < :biz_date + 1","source_date_col":"D_BIZ","target_table":"HOLDINGS_DAILY","target_date_col":"D_BIZ"}"#,
        ),
    )
    .unwrap();
    assert_eq!(updated.status, 200, "{}", updated.body);
    let updated = json_body(&updated);
    assert_task_fields(&updated);
    assert_eq!(updated["task_id"], task_id);
    assert_eq!(updated["name"], "持仓日明细");
    assert_eq!(updated["target_table"], "HOLDINGS_DAILY");

    let first_output = terminate(child);
    assert_success(&first_output);
    let database = directory.join("db-qbs.sqlite3");
    assert_eq!(
        fs::metadata(&database).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let mut restarted = start_source(&config);
    let listed = wait_for_tasks(port, &mut restarted);
    assert_eq!(listed.status, 200);
    assert_eq!(json_body(&listed), serde_json::json!([updated]));

    let no_config_endpoint = get(port, "/api/config").unwrap();
    assert_eq!(no_config_endpoint.status, 404);
    for response in [&listed, &no_config_endpoint] {
        assert!(!response.body.contains("secret"));
        assert!(!response.body.contains("oracle_password"));
    }

    let deleted = request(port, "DELETE", &format!("/api/tasks/{task_id}"), None).unwrap();
    assert_eq!(deleted.status, 200, "{}", deleted.body);
    assert_eq!(json_body(&deleted), updated);
    assert_eq!(
        get(port, &format!("/api/tasks/{task_id}")).unwrap().status,
        404
    );
    assert_eq!(
        json_body(&get(port, "/api/tasks").unwrap()),
        serde_json::json!([])
    );

    assert_success(&terminate(restarted));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn task_writes_reject_client_identity_and_incomplete_definitions() {
    let directory = temp_directory();
    let port = unused_port();
    let config = write_config(&directory, &format!("127.0.0.1:{port}"));
    let mut child = start_source(&config);
    wait_for_tasks(port, &mut child);

    let client_identity = request(
        port,
        "POST",
        "/api/tasks",
        Some(
            r#"{"task_id":"chosen-by-client","name":"持仓明细","source_sql":"SELECT ID FROM HOLDINGS","source_date_col":"D_BIZ","target_table":"HOLDINGS","target_date_col":"D_BIZ"}"#,
        ),
    )
    .unwrap();
    assert_eq!(client_identity.status, 400, "{}", client_identity.body);

    let missing_name = request(
        port,
        "POST",
        "/api/tasks",
        Some(
            r#"{"source_sql":"SELECT ID FROM HOLDINGS","source_date_col":"D_BIZ","target_table":"HOLDINGS","target_date_col":"D_BIZ"}"#,
        ),
    )
    .unwrap();
    assert_eq!(missing_name.status, 400, "{}", missing_name.body);
    assert_eq!(
        json_body(&get(port, "/api/tasks").unwrap()),
        serde_json::json!([])
    );

    assert_success(&terminate(child));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn run_launch_materializes_task_and_aggregates_child_output_until_exit() {
    let directory = temp_directory();
    let port = unused_port();
    let release = directory.join("release-child");
    let invocation = directory.join("child-args");
    let fake_child = write_fake_child(
        &directory,
        &format!(
            r#"printf '%s\n' "$@" > '{}'
printf '%s\n' '{{"ts":"2026-08-15T10:00:00.000Z","level":"info","event":"source_started","run_id":null,"task":null,"biz_date":"2026-08-14","message":"started"}}'
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
        ),
    );
    let config = write_run_config(&directory, port, &fake_child);
    let mut source = start_source(&config);
    wait_for_tasks(port, &mut source);
    let task_id = create_task(port);
    let audit = rusqlite::Connection::open(directory.join("db-qbs.sqlite3")).unwrap();
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

    let started = post(
        port,
        "/api/runs",
        &format!(r#"{{"task_id":"{task_id}","biz_date":"2026-08-14"}}"#),
    )
    .unwrap();
    assert_eq!(started.status, 202, "{}", started.body);
    let run_record_id = json_body(&started)["run_record_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let detail = wait_for_json(port, &format!("/api/runs/{run_record_id}"), |body| {
        body["rows_pushed"] == 7
    });
    assert_eq!(
        detail,
        serde_json::json!({
            "run_record_id": run_record_id,
            "run_id": "run-7",
            "biz_date": "2026-08-14",
            "staging_table": "STG_7",
            "stage": "STREAMING",
            "seq": 2,
            "rows_pushed": 7,
            "bytes": 220,
            "ms": 22,
            "last_ts": "2026-08-15T10:00:05.000Z",
            "live": true,
        })
    );

    let task_files = directory_entries(&directory.join("run-tasks"));
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
        "source_sql",
        "source_date_col",
        "target_table",
        "target_date_col",
    ] {
        assert!(task_toml.contains(field), "{task_toml}");
    }
    for secret in ["name", "task_id", "oracle_password", "secret"] {
        assert!(!task_toml.contains(secret), "{task_toml}");
    }

    let args = fs::read_to_string(invocation).unwrap();
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        [
            "--config",
            config.to_str().unwrap(),
            "--task",
            task_files[0].to_str().unwrap(),
            "--biz-date",
            "2026-08-14",
        ]
    );

    fs::write(release, "").unwrap();
    let history = wait_for_json(port, &format!("/api/runs/{run_record_id}"), |body| {
        body["live"] == false
    });
    assert_eq!(history["run_record_id"], run_record_id);
    assert_eq!(history["run_id"], "run-7");
    assert_eq!(history["task_id"], task_id);
    assert_eq!(history["biz_date"], "2026-08-14");
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

    let by_task = json_body(&get(port, &format!("/api/runs?task_id={task_id}")).unwrap());
    assert_eq!(by_task.as_array().unwrap().len(), 1);
    assert_eq!(by_task[0]["run_record_id"], run_record_id);
    let by_date = json_body(&get(port, "/api/runs?biz_date=2026-08-14").unwrap());
    assert_eq!(by_date.as_array().unwrap().len(), 1);
    let no_match = json_body(&get(port, "/api/runs?biz_date=2026-08-13").unwrap());
    assert_eq!(no_match, serde_json::json!([]));
    let audit = rusqlite::Connection::open(directory.join("db-qbs.sqlite3")).unwrap();
    let history_writes: u64 = audit
        .query_row("SELECT COUNT(*) FROM history_write_audit", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(history_writes, 5);
    wait_for_empty_directory(&directory.join("run-tasks"));

    assert_success(&terminate(source));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn silent_child_disappearance_evicts_projection_and_removes_task_file() {
    let directory = temp_directory();
    let port = unused_port();
    let fake_child = write_fake_child(&directory, "exit 1\n");
    let config = write_run_config(&directory, port, &fake_child);
    let mut source = start_source(&config);
    wait_for_tasks(port, &mut source);
    let task_id = create_task(port);

    let run_record_id = start_run(port, &task_id);
    let history = wait_for_json(port, &format!("/api/runs/{run_record_id}"), |body| {
        body["live"] == false
    });
    assert_eq!(history["outcome"], "FAILED");
    assert_eq!(history["unknown_reason"], "PROCESS_DISAPPEARED");
    assert_eq!(history["message"], "进程消失，无终态日志");
    assert_eq!(history["source_code"], Value::Null);
    assert_eq!(history["sink_code"], Value::Null);
    assert_eq!(history["target_table_effect"], Value::Null);
    wait_for_empty_directory(&directory.join("run-tasks"));

    assert_success(&terminate(source));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn restart_cleanup_distinguishes_graceful_restart_from_process_disappearance() {
    let directory = temp_directory();
    let port = unused_port();
    let fake_child = write_fake_child(
        &directory,
        r#"printf '%s\n' '{"ts":"2026-08-15T12:00:00.000Z","level":"info","event":"source_started","run_id":null,"task":null,"biz_date":"2026-08-14"}'
printf '%s\n' '{"ts":"2026-08-15T12:00:01.000Z","level":"info","event":"stage_changed","run_id":"run-restart","task":null,"stage":"PREPARING"}'
sleep 2
"#,
    );
    let config = write_run_config(&directory, port, &fake_child);
    let mut source = start_source(&config);
    wait_for_tasks(port, &mut source);
    let task_id = create_task(port);

    let graceful_record_id = start_run(port, &task_id);
    wait_for_json(port, &format!("/api/runs/{graceful_record_id}"), |body| {
        body["stage"] == "PREPARING"
    });
    assert_success(&terminate(source));

    let mut restarted = start_source(&config);
    wait_for_tasks(port, &mut restarted);
    let graceful = json_body(&get(port, &format!("/api/runs/{graceful_record_id}")).unwrap());
    assert_eq!(graceful["live"], false);
    assert_eq!(graceful["unknown_reason"], "SERVICE_RESTARTED");
    assert_eq!(graceful["message"], "服务重启，结局未知");
    assert_eq!(graceful["source_code"], Value::Null);
    assert_eq!(graceful["target_table_effect"], Value::Null);

    let disappeared_record_id = start_run(port, &task_id);
    wait_for_json(
        port,
        &format!("/api/runs/{disappeared_record_id}"),
        |body| body["stage"] == "PREPARING",
    );
    let killed = kill_source(restarted, "-KILL");
    assert!(!killed.status.success(), "{}", output_text(&killed));

    let mut after_kill = start_source(&config);
    wait_for_tasks(port, &mut after_kill);
    let disappeared = json_body(&get(port, &format!("/api/runs/{disappeared_record_id}")).unwrap());
    assert_eq!(disappeared["live"], false);
    assert_eq!(disappeared["unknown_reason"], "PROCESS_DISAPPEARED");
    assert_eq!(disappeared["message"], "进程消失，无终态日志");
    assert_eq!(disappeared["sink_code"], Value::Null);
    assert_eq!(disappeared["target_table_effect"], Value::Null);
    wait_for_empty_directory(&directory.join("run-tasks"));

    assert_success(&terminate(after_kill));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn child_hanging_mid_run_remains_live_and_accepted_is_not_preparing() {
    let directory = temp_directory();
    let port = unused_port();
    let emit = directory.join("emit-lines");
    let release = directory.join("release-child");
    let fake_child = write_fake_child(
        &directory,
        &format!(
            r#"while [ ! -f '{}' ]; do sleep 0.02; done
printf '%s\n' '{{"ts":"2026-08-15T11:00:00.000Z","level":"info","event":"source_started","run_id":null,"task":null,"biz_date":"2026-08-14","message":"started"}}'
printf '%s\n' '{{"ts":"2026-08-15T11:00:01.000Z","level":"info","event":"stage_changed","run_id":"run-hanging","task":null,"stage":"PREPARING","message":"preparing"}}'
printf '%s\n' '{{"ts":"2026-08-15T11:00:02.000Z","level":"info","event":"batch_pushed","run_id":"run-hanging","task":null,"seq":1,"rows":5,"source_rows":5,"bytes":64,"written":5,"ms":9}}'
while [ ! -f '{}' ]; do sleep 0.02; done
"#,
            emit.display(),
            release.display(),
        ),
    );
    let config = write_run_config(&directory, port, &fake_child);
    let mut source = start_source(&config);
    wait_for_tasks(port, &mut source);
    let task_id = create_task(port);

    let run_record_id = start_run(port, &task_id);
    let accepted = json_body(&get(port, &format!("/api/runs/{run_record_id}")).unwrap());
    assert_eq!(accepted["stage"], Value::Null);
    assert_eq!(accepted["run_id"], Value::Null);
    assert_eq!(accepted["biz_date"], Value::Null);
    assert_eq!(accepted["seq"], 0);
    assert_eq!(accepted["rows_pushed"], 0);
    assert_eq!(accepted["bytes"], 0);
    assert_eq!(accepted["ms"], 0);
    assert_eq!(accepted["last_ts"], Value::Null);
    assert_eq!(accepted["live"], true);

    fs::write(emit, "").unwrap();
    let partial = wait_for_json(port, &format!("/api/runs/{run_record_id}"), |body| {
        body["rows_pushed"] == 5
    });
    assert_eq!(partial["stage"], "PREPARING");
    assert_eq!(partial["run_id"], "run-hanging");
    assert_eq!(partial["biz_date"], "2026-08-14");
    assert_eq!(partial["seq"], 1);
    assert_eq!(partial["bytes"], 64);
    assert_eq!(partial["ms"], 9);
    assert_eq!(partial["live"], true);

    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        get(port, &format!("/api/runs/{run_record_id}"))
            .unwrap()
            .status,
        200
    );
    fs::write(release, "").unwrap();
    let history = wait_for_json(port, &format!("/api/runs/{run_record_id}"), |body| {
        body["live"] == false
    });
    assert_eq!(history["unknown_reason"], "PROCESS_DISAPPEARED");

    assert_success(&terminate(source));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn tasks_endpoint_is_ready_and_sigterm_allows_same_port_restart() {
    let directory = temp_directory();
    let port = unused_port();
    let config = write_config(&directory, &format!("127.0.0.1:{port}"));

    let mut first = start_source(&config);
    let response = wait_for_tasks(port, &mut first);
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
    let second_output = terminate(second);
    assert_success(&second_output);

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
    let port = unused_port();
    let config = write_config(&directory, &format!("0.0.0.0:{port}"));

    let mut child = start_source(&config);
    let response = wait_for_tasks(port, &mut child);
    assert_eq!(response.status, 200);
    let output = terminate(child);
    let lines = json_lines(&output.stdout);
    let warning = lines.iter().find(|line| line["level"] == "warn").unwrap();
    let message = warning["message"].as_str().unwrap();
    let address = format!("0.0.0.0:{port}");
    for phrase in [
        "无鉴权",
        "源库跑任意 SQL",
        "清空重写目标表",
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

#[test]
fn column_fetch_shape_failure_reports_all_checks_without_a_run_code() {
    let directory = temp_directory();
    let port = unused_port();
    let config = write_config(&directory, &format!("127.0.0.1:{port}"));
    let mut child = start_source(&config);
    wait_for_tasks(port, &mut child);

    let response = post(
        port,
        "/api/columns",
        r#"{
          "source_sql":"SELECT a.id AS ID, a.biz_day AS BIZ_DAY FROM orders a WHERE a.biz_day >= TO_DATE(:biz_date,'YYYY-MM-DD') AND a.biz_day < TO_DATE(:biz_date,'YYYY-MM-DD') + 1",
          "source_date_col":"BIZ_DAY",
          "target_table":"ORDERS",
          "target_date_col":"OTHER_DAY"
        }"#,
    )
    .unwrap();

    assert_eq!(response.status, 422);
    let body: Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(body["kind"], "sql_shape");
    assert!(body.get("code").is_none());
    assert!(body.get("run_id").is_none());
    let checks = body["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 6);
    assert_eq!(
        checks
            .iter()
            .filter(|check| check["passed"] == true)
            .count(),
        5
    );
    assert!(checks.iter().all(|check| check.get("code").is_none()));

    assert_success(&terminate(child));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn column_fetch_oracle_failure_does_not_create_a_run_touch_sink_or_write_storage() {
    let directory = temp_directory();
    let port = unused_port();
    let sink = TcpListener::bind("127.0.0.1:0").unwrap();
    sink.set_nonblocking(true).unwrap();
    let sink_url = format!("http://{}", sink.local_addr().unwrap());
    let config = write_config_with_oracle(
        &directory,
        &format!("127.0.0.1:{port}"),
        &sink_url,
        "/db-qbs-missing-oracle-client",
    );
    let mut child = start_source(&config);
    wait_for_tasks(port, &mut child);
    let files_before = directory_entries(&directory);

    let response = post(
        port,
        "/api/columns",
        r#"{
          "source_sql":"SELECT a.id AS ID, a.biz_day AS BIZ_DAY FROM missing_orders a WHERE a.biz_day >= TO_DATE(:biz_date,'YYYY-MM-DD') AND a.biz_day < TO_DATE(:biz_date,'YYYY-MM-DD') + 1",
          "source_date_col":"BIZ_DAY",
          "target_table":"ORDERS",
          "target_date_col":"BIZ_DAY"
        }"#,
    )
    .unwrap();

    assert_eq!(response.status, 502);
    let body: Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(body["kind"], "oracle");
    assert!(body.get("run_id").is_none());
    assert_eq!(directory_entries(&directory), files_before);
    let sink_error = sink.accept().unwrap_err();
    assert_eq!(sink_error.kind(), std::io::ErrorKind::WouldBlock);

    assert_success(&terminate(child));
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

fn wait_for_tasks(port: u16, child: &mut Child) -> HttpResponse {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("source exited before readiness with {status}");
        }
        if let Ok(response) = get(port, "/api/tasks") {
            return response;
        }
        assert!(Instant::now() < deadline, "source did not become ready");
        thread::sleep(Duration::from_millis(20));
    }
}

fn get(port: u16, path: &str) -> std::io::Result<HttpResponse> {
    request(port, "GET", path, None)
}

fn request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> std::io::Result<HttpResponse> {
    let body = body.unwrap_or("");
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.write_all(
        format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    )?;
    read_response(&mut stream)
}

fn post(port: u16, path: &str, body: &str) -> std::io::Result<HttpResponse> {
    request(port, "POST", path, Some(body))
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
    Ok(HttpResponse {
        status,
        body: body.to_owned(),
    })
}

fn json_body(response: &HttpResponse) -> Value {
    serde_json::from_str(&response.body).unwrap()
}

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

fn create_task(port: u16) -> String {
    let response = post(
        port,
        "/api/tasks",
        r#"{"name":"holdings","source_sql":"SELECT ID, D_BIZ FROM HOLDINGS WHERE D_BIZ >= :biz_date AND D_BIZ < :biz_date + 1","source_date_col":"D_BIZ","target_table":"HOLDINGS","target_date_col":"D_BIZ"}"#,
    )
    .unwrap();
    assert_eq!(response.status, 201, "{}", response.body);
    json_body(&response)["task_id"].as_str().unwrap().to_owned()
}

fn start_run(port: u16, task_id: &str) -> String {
    let response = post(
        port,
        "/api/runs",
        &format!(r#"{{"task_id":"{task_id}","biz_date":"2026-08-14"}}"#),
    )
    .unwrap();
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
             sink_base_url = \"http://127.0.0.1:18080\"\n\
             listen = \"127.0.0.1:{port}\"\n\
             data_dir = \"{}\"\n\
             run_executable = \"{}\"\n",
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
    write_config_with_oracle(directory, listen, "http://127.0.0.1:18080", "/opt/oracle")
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

fn directory_entries(directory: &Path) -> Vec<PathBuf> {
    let mut entries = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    entries
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
}
