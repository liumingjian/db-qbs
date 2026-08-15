use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

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
    let files_before = directory_entries(&directory);
    let mut child = start_source(&config);
    wait_for_tasks(port, &mut child);

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
    let kill_status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
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
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
            .as_bytes(),
    )?;
    read_response(&mut stream)
}

fn post(port: u16, path: &str, body: &str) -> std::io::Result<HttpResponse> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.write_all(
        format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    )?;
    read_response(&mut stream)
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
