use chrono::DateTime;
use db_qbs_shared::{write_log_line, LogLevel};
use serde_json::Value;

#[test]
fn emitter_writes_one_json_line_with_all_common_fields() {
    let mut output = Vec::new();

    write_log_line(
        &mut output,
        LogLevel::Info,
        "run_started",
        Some("20260814091530_a3f19c"),
        Some("/etc/db-qbs/tasks/nav.toml"),
    )
    .unwrap();

    let text = String::from_utf8(output).unwrap();
    assert!(text.ends_with('\n'));
    assert_eq!(text.lines().count(), 1);

    let json = text.strip_suffix('\n').unwrap();
    let line: Value = serde_json::from_str(json).unwrap();
    assert_eq!(line["level"], "info");
    assert_eq!(line["event"], "run_started");
    assert_eq!(line["run_id"], "20260814091530_a3f19c");
    assert_eq!(line["task"], "/etc/db-qbs/tasks/nav.toml");
    assert_eq!(line.as_object().unwrap().len(), 5);
    assert!(DateTime::parse_from_rfc3339(line["ts"].as_str().unwrap()).is_ok());
}

#[test]
fn emitter_serializes_absent_optional_fields_as_json_null() {
    let mut output = Vec::new();

    write_log_line(
        &mut output,
        LogLevel::Error,
        "connection_check_failed",
        None,
        None,
    )
    .unwrap();

    let line: Value = serde_json::from_slice(&output).unwrap();
    assert!(line["run_id"].is_null());
    assert!(line["task"].is_null());
}
