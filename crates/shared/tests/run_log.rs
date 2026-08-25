use chrono::DateTime;
use db_qbs_shared::{write_log_line, write_log_line_with_fields, LogEvent, LogLevel};
use serde_json::{json, Value};

#[test]
fn emitter_writes_one_json_line_with_all_common_fields() {
    let mut output = Vec::new();

    write_log_line(
        &mut output,
        LogLevel::Info,
        LogEvent::SourceStarted,
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
    assert_eq!(line["event"], "source_started");
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
        LogEvent::SinkUnavailable,
        None,
        None,
    )
    .unwrap();

    let line: Value = serde_json::from_slice(&output).unwrap();
    assert!(line["run_id"].is_null());
    assert!(line["task"].is_null());
}

#[test]
fn range_check_event_serializes_its_scan_facts() {
    let mut output = Vec::new();

    write_log_line_with_fields(
        &mut output,
        LogLevel::Info,
        LogEvent::RangeCheckExecuted,
        Some("20260814091530_a3f19c"),
        None,
        json!({
            "columns": ["N_RAW", "N_EXPR"],
            "scanned_rows": 37,
            "ms": 12,
        }),
    )
    .unwrap();

    let line: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(line["event"], "range_check_executed");
    assert_eq!(line["columns"], json!(["N_RAW", "N_EXPR"]));
    assert_eq!(line["scanned_rows"], 37);
    assert_eq!(line["ms"], 12);
}

#[test]
fn event_vocabulary_is_closed_and_stable() {
    assert_eq!(
        LogEvent::ALL.map(LogEvent::as_str),
        [
            "cli_failed",
            "source_started",
            "business_date_invalid",
            "source_config_failed",
            "task_config_failed",
            "sql_shape_precheck_failed",
            "sql_shape_precheck_passed",
            "stage_changed",
            "mapping_precheck_failed",
            "range_check_executed",
            "precount_finished",
            "run_opened",
            "batch_pushed",
            "commit_diagnosed",
            "abort_failed",
            "run_finished",
            "sink_started",
            "sink_unavailable",
            "http_response_failed",
        ]
    );
}
