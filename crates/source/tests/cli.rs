use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[test]
fn invalid_shape_reports_all_problems_without_network_access() {
    let directory = temp_directory();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let config = write_source_config(&directory, listener.local_addr().unwrap().port());
    let task = write_task(
        &directory,
        "SELECT *, amount * 2 FROM orders WHERE biz_day = SYSDATE AND status = 'OPEN'",
        "BIZ_DAY",
        "OTHER_DAY",
    );

    let output = run_source(&config, &task, "2026-08-14");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        listener.accept().is_err(),
        "precheck failure sent a request"
    );
    let lines = json_lines(&output.stdout);
    assert_common_fields(&lines, &fs::canonicalize(&task).unwrap());
    assert!(lines.iter().all(|line| line["run_id"].is_null()));
    let failure = lines
        .iter()
        .find(|line| line["event"] == "sql_shape_precheck_failed")
        .unwrap();
    let codes: Vec<_> = failure["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|problem| problem["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&"relative_time_function"));
    assert!(codes.contains(&"invalid_date_predicate"));
    assert!(codes.contains(&"additional_where_predicate"));
    assert!(codes.contains(&"unnamed_projection"));
    assert!(codes.contains(&"date_column_mismatch"));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn valid_shape_attempts_oracle_describe_before_sink() {
    let directory = temp_directory();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let config = write_source_config(&directory, listener.local_addr().unwrap().port());
    let task = write_task(
        &directory,
        "SELECT a.id AS ID, a.biz_day AS BIZ_DAY FROM orders a \
         WHERE a.biz_day >= TO_DATE(:biz_date,'YYYY-MM-DD') \
         AND a.biz_day < TO_DATE(:biz_date,'YYYY-MM-DD') + 1",
        "BIZ_DAY",
        "biz_day",
    );
    let output = run_source(&config, &task, "2026-08-14");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        listener.accept().is_err(),
        "source contacted sink before Oracle describe"
    );
    let lines = json_lines(&output.stdout);
    assert_common_fields(&lines, &fs::canonicalize(&task).unwrap());
    assert!(lines
        .iter()
        .any(|line| line["event"] == "sql_shape_precheck_passed"));
    assert!(lines
        .iter()
        .any(|line| line["event"] == "run_finished" && line["stage"] == "PREPARING"));
    let terminal = lines
        .iter()
        .find(|line| line["event"] == "run_finished")
        .unwrap();
    for field in [
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

fn run_source(config: &Path, task: &Path, biz_date: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_db-qbs-source-run"))
        .args(["--config"])
        .arg(config)
        .args(["--task"])
        .arg(task)
        .args(["--biz-date", biz_date])
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

fn write_task(
    directory: &Path,
    sql: &str,
    source_date_col: &str,
    target_date_col: &str,
) -> PathBuf {
    let path = directory.join("task.toml");
    fs::write(
        &path,
        format!(
            "source_sql = '''{sql}'''\n\
             source_date_col = \"{source_date_col}\"\n\
             target_table = \"ORDERS\"\n\
             target_date_col = \"{target_date_col}\"\n"
        ),
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
