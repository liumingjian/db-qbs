use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[test]
fn an_unusable_task_file_fails_before_any_network_access() {
    // SQL 形状预检整段取消后（ADR-0036 §5），子进程开跑前的本地闸只剩「任务文件读得动」这一条。
    // 它仍必须在**碰网络之前**判完：形状不过时不发请求那条口径，换了对象没换性质。
    let directory = temp_directory();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let config = write_source_config(&directory, listener.local_addr().unwrap().port());
    let task = directory.join("task.toml");
    fs::write(&task, "[spec]\nowner = \"APP\"\ngranularity = \"DAY\"\n").unwrap();

    let output = run_source(&config, &task);

    assert_eq!(output.status.code(), Some(1));
    assert!(listener.accept().is_err(), "config failure sent a request");
    let lines = json_lines(&output.stdout);
    assert_common_fields(&lines, &fs::canonicalize(&task).unwrap());
    assert!(lines.iter().all(|line| line["run_id"].is_null()));
    let failure = lines
        .iter()
        .find(|line| line["event"] == "task_config_failed")
        .unwrap();
    assert_eq!(failure["failure_kind"], "CONFIG");
    assert!(failure["message"].as_str().unwrap().contains("granularity"));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_usable_spec_attempts_oracle_describe_before_sink() {
    let directory = temp_directory();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let config = write_source_config(&directory, listener.local_addr().unwrap().port());
    let task = write_task(&directory);

    let output = run_source(&config, &task);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        listener.accept().is_err(),
        "source contacted sink before Oracle describe"
    );
    let lines = json_lines(&output.stdout);
    assert_common_fields(&lines, &fs::canonicalize(&task).unwrap());
    assert!(lines
        .iter()
        .any(|line| line["event"] == "run_finished" && line["stage"] == "PREPARING"));
    let terminal = lines
        .iter()
        .find(|line| line["event"] == "run_finished")
        .unwrap();
    // 分类必须在终态行上，排障不该靠读人话反推是哪一侧坏了（V1 成功标准第 4 条）。
    // Oracle 客户端在台架外根本起不来，这一步撞的必然是「连不上 Oracle」。
    assert_eq!(terminal["failure_kind"], "SOURCE_CONNECT");
    for field in [
        "failure_kind",
        "source_rows",
        "source_batches",
        "staged_rows",
        "received_batches",
        "sink_reported_rows",
        "purged_rows",
        "fetch_ms",
        "push_ms",
        "commit_ms",
        "count_ms",
        "cursor_ms",
    ] {
        assert!(terminal.get(field).is_some(), "missing {field}");
    }
    assert!(terminal["run_id"].is_string());

    fs::remove_dir_all(directory).unwrap();
}

fn run_source(config: &Path, task: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_db-qbs-source-run"))
        .args(["--config"])
        .arg(config)
        .args(["--task"])
        .arg(task)
        .output()
        .unwrap()
}

fn write_source_config(directory: &Path, port: u16) -> PathBuf {
    let path = directory.join("source.toml");
    fs::write(
        &path,
        format!(
            "oracle_connect_string = \"//oracle:1521/XE\"\n\
             oracle_username = \"source\"\n\
             oracle_password = \"secret\"\n\
             oracle_client_lib_dir = \"/opt/oracle\"\n\
             sink_base_url = \"http://127.0.0.1:{port}\"\n\
             listen = \"127.0.0.1:18088\"\n\
             data_dir = \"{}\"\n",
            directory.display()
        ),
    )
    .unwrap();
    path
}

/// 一份可用的任务文件：结构化规格 + 本次运行参数，与父进程物化出来的那份同形。
fn write_task(directory: &Path) -> PathBuf {
    let path = directory.join("task.toml");
    fs::write(
        &path,
        "[spec]\n\
         owner = \"APP\"\n\
         table = \"ORDERS\"\n\
         target_table = \"ORDERS\"\n\
         columns = [\n\
         { source = \"ID\", target = \"ID\" },\n\
         { source = \"BIZ_DAY\", target = \"BIZ_DAY\" },\n\
         ]\n\
         primary_key = [\"ID\"]\n\
         \n\
         [[spec.conditions]]\n\
         column = \"BIZ_DAY\"\n\
         operator = \"eq\"\n\
         value_type = \"date\"\n\
         parameter = \"biz_day\"\n\
         value_source = \"runtime\"\n\
         constant = \"\"\n\
         \n\
         [oracle]\n\
         connect_string = \"//oracle:1521/XE\"\n\
         username = \"source\"\n\
         password = \"secret\"\n\
         client_lib_dir = \"/db-qbs-missing-oracle-client\"\n\
         \n\
         [target]\n\
         host = \"127.0.0.1\"\n\
         port = 3306\n\
         username = \"sink\"\n\
         password = \"change-me\"\n\
         database = \"qbs\"\n\
         \n\
         [run_params]\n\
         biz_day = \"2026-08-14\"\n",
    )
    .unwrap();
    path
}

fn json_lines(stdout: &[u8]) -> Vec<Value> {
    String::from_utf8(stdout.to_vec())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn assert_common_fields(lines: &[Value], task: &Path) {
    assert!(!lines.is_empty());
    for line in lines {
        assert!(line.get("ts").unwrap().is_string());
        assert!(line.get("level").unwrap().is_string());
        assert!(line.get("event").unwrap().is_string());
        assert!(line.get("run_id").is_some());
        assert_eq!(line["task"], task.to_string_lossy().as_ref());
    }
    assert_eq!(lines[0]["event"], "source_started");
}

fn temp_directory() -> PathBuf {
    let suffix = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "db-qbs-source-test-{}-{suffix}",
        std::process::id()
    ));
    fs::create_dir(&path).unwrap();
    path
}
