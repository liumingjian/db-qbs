use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{TimeDelta, TimeZone, Utc};
use db_qbs_source::{
    expired_history_indices, fold_history_lines, HistoryStore, RunHistory, UnknownReason,
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[test]
fn retention_uses_injected_now_and_a_time_boundary() {
    let now = Utc.with_ymd_and_hms(2026, 8, 15, 12, 0, 0).unwrap();
    let timestamps = [
        Utc.with_ymd_and_hms(2026, 5, 17, 11, 59, 59).unwrap(),
        Utc.with_ymd_and_hms(2026, 5, 17, 12, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 8, 15, 11, 59, 59).unwrap(),
    ];

    assert_eq!(expired_history_indices(&timestamps, now, 90), vec![0]);
}

#[test]
fn json_lines_fold_into_one_history_row_with_event_scoped_terminals() {
    let accepted_at = Utc.with_ymd_and_hms(2026, 8, 15, 9, 59, 59).unwrap();
    let lines = [
        r#"{"ts":"2026-08-15T10:00:00.000Z","event":"source_started","run_id":null,"biz_date":"2026-08-14"}"#,
        r#"{"ts":"2026-08-15T10:00:01.000Z","event":"stage_changed","run_id":"run-7","stage":"PREPARING"}"#,
        r#"{"ts":"2026-08-15T10:00:02.000Z","event":"run_opened","run_id":"run-7","staging_table":"STG_7"}"#,
        r#"{"ts":"2026-08-15T10:00:03.000Z","event":"stage_changed","run_id":"run-7","stage":"STREAMING"}"#,
        r#"{"ts":"2026-08-15T10:00:04.000Z","event":"batch_pushed","run_id":"run-7","seq":1,"rows":3,"bytes":100,"ms":10}"#,
        r#"{"ts":"2026-08-15T10:00:05.000Z","event":"stage_changed","run_id":"run-7","stage":"COMMITTING"}"#,
        r#"{"ts":"2026-08-15T10:00:06.000Z","event":"commit_diagnosed","run_id":"run-7","terminal":"DISCARDED","message":"sink tombstone"}"#,
        r#"{"ts":"2026-08-15T10:00:07.000Z","event":"run_finished","run_id":"run-7","terminal":"FAILED","stage":"COMMITTING","source_code":null,"sink_code":"VERIFY_FAILED","column":"AMOUNT","value":"secret","message":"counts differ","source_rows":3,"source_batches":1,"staged_rows":3,"received_batches":1,"sink_reported_rows":2,"purged_rows":0,"fetch_ms":4,"push_ms":10,"commit_ms":6,"count_ms":2,"cursor_ms":1}"#,
    ];

    let history =
        fold_history_lines("record-1", "task-1", "2026-08-14", accepted_at, &lines).unwrap();

    assert_eq!(history.run_record_id, "record-1");
    assert_eq!(history.task_id, "task-1");
    assert_eq!(history.run_id.as_deref(), Some("run-7"));
    assert_eq!(history.biz_date, "2026-08-14");
    assert_eq!(history.staging_table.as_deref(), Some("STG_7"));
    assert_eq!(history.outcome.as_deref(), Some("FAILED"));
    assert_eq!(history.target_table_effect.as_deref(), Some("DISCARDED"));
    assert_eq!(history.stage.as_deref(), Some("COMMITTING"));
    assert_eq!(history.source_rows, Some(3));
    assert_eq!(history.staged_rows, Some(3));
    assert_eq!(history.sink_reported_rows, Some(2));
    assert_eq!(history.purged_rows, Some(0));
    assert_eq!(history.source_batches, Some(1));
    assert_eq!(history.received_batches, Some(1));
    assert_eq!(history.fetch_ms, Some(4));
    assert_eq!(history.push_ms, Some(10));
    assert_eq!(history.commit_ms, Some(6));
    assert_eq!(history.count_ms, Some(2));
    assert_eq!(history.cursor_ms, Some(1));
    assert_eq!(history.source_code, None);
    assert_eq!(history.sink_code.as_deref(), Some("VERIFY_FAILED"));
    assert_eq!(history.column.as_deref(), Some("AMOUNT"));
    assert_eq!(history.value.as_deref(), Some("secret"));
    assert_eq!(history.message.as_deref(), Some("counts differ"));
    assert_eq!(history.seq, 1);
    assert_eq!(history.rows_pushed, 3);
    assert_eq!(history.bytes, 100);
    assert_eq!(history.ms, 10);
    assert_eq!(history.started_at, "2026-08-15T10:00:00.000Z");
    assert_eq!(
        history.finished_at.as_deref(),
        Some("2026-08-15T10:00:07.000Z")
    );
}

#[test]
fn mapping_precheck_diagnostics_survive_the_terminal_fold() {
    let accepted_at = Utc.with_ymd_and_hms(2026, 8, 15, 9, 59, 59).unwrap();
    let lines = [
        r#"{"ts":"2026-08-15T10:00:01.000Z","event":"mapping_precheck_failed","run_id":"run-7","column":"V_TEXT","source":"VARCHAR2(200)","target":"missing","rule":"目标表缺列"}"#,
        r#"{"ts":"2026-08-15T10:00:02.000Z","event":"mapping_precheck_failed","run_id":"run-7","column":"D_BIZ","source":"DATE","target":"VARCHAR(20)","rule":"类型不兼容"}"#,
        r#"{"ts":"2026-08-15T10:00:03.000Z","event":"stage_changed","run_id":"run-7","stage":"FAILED"}"#,
        r#"{"ts":"2026-08-15T10:00:04.000Z","event":"run_finished","run_id":"run-7","terminal":"FAILED","stage":"PREPARING","source_code":null,"sink_code":"PRECHECK_FAILED","column":null,"value":null,"message":"映射预检未通过：一次发现 2 项问题","source_rows":0,"source_batches":0,"staged_rows":null,"received_batches":null,"sink_reported_rows":null,"purged_rows":null,"fetch_ms":0,"push_ms":0,"commit_ms":0,"count_ms":null,"cursor_ms":1}"#,
    ];

    let history =
        fold_history_lines("record-1", "task-1", "2026-08-14", accepted_at, &lines).unwrap();

    assert_eq!(history.mapping_issues.len(), 2);
    assert_eq!(history.mapping_issues[0]["column"], "V_TEXT");
    assert_eq!(history.mapping_issues[1]["rule"], "类型不兼容");
}

#[test]
fn explicit_verification_failure_is_known_to_be_discarded() {
    let accepted_at = Utc.with_ymd_and_hms(2026, 8, 15, 9, 59, 59).unwrap();
    let lines = [
        r#"{"ts":"2026-08-15T10:00:01.000Z","event":"stage_changed","run_id":"run-7","stage":"COMMITTING"}"#,
        r#"{"ts":"2026-08-15T10:00:02.000Z","event":"run_finished","run_id":"run-7","terminal":"FAILED","stage":"COMMITTING","source_code":null,"sink_code":"VERIFY_FAILED","column":null,"value":null,"message":"counts differ","source_rows":3,"source_batches":1,"staged_rows":2,"received_batches":1,"sink_reported_rows":3,"purged_rows":0,"fetch_ms":1,"push_ms":1,"commit_ms":1,"count_ms":1,"cursor_ms":1}"#,
    ];

    let history =
        fold_history_lines("record-1", "task-1", "2026-08-14", accepted_at, &lines).unwrap();

    assert_eq!(history.target_table_effect.as_deref(), Some("DISCARDED"));
}

#[test]
fn shape_precheck_failure_keeps_parent_identity_without_inventing_a_run_id() {
    let accepted_at = Utc.with_ymd_and_hms(2026, 8, 15, 9, 59, 59).unwrap();
    let lines = [
        r#"{"ts":"2026-08-15T10:00:00.000Z","event":"source_started","run_id":null,"biz_date":"2026-08-14"}"#,
        r#"{"ts":"2026-08-15T10:00:01.000Z","event":"sql_shape_precheck_failed","run_id":null,"message":"two checks failed"}"#,
    ];

    let history =
        fold_history_lines("record-2", "task-1", "2026-08-14", accepted_at, &lines).unwrap();

    assert_eq!(history.run_record_id, "record-2");
    assert_eq!(history.run_id, None);
    assert_eq!(history.outcome.as_deref(), Some("FAILED"));
    assert_eq!(history.target_table_effect.as_deref(), Some("DISCARDED"));
    assert_eq!(history.message.as_deref(), Some("two checks failed"));
    assert_eq!(history.source_code, None);
    assert_eq!(history.sink_code, None);
}

#[test]
fn sqlite_writes_lazily_remove_expired_rows_and_startup_seals_incomplete_rows() {
    let suffix = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "db-qbs-run-history-test-{}-{suffix}",
        std::process::id()
    ));
    fs::create_dir(&directory).unwrap();
    let store = HistoryStore::open(&directory).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 8, 15, 12, 0, 0).unwrap();
    let old_at = now - TimeDelta::days(91);
    let old = RunHistory::accepted("old", "task-1", "2026-05-16", old_at);
    store.insert(&old, old_at, 90).unwrap();
    assert!(store.get("old").unwrap().is_some());

    let mut current = RunHistory::accepted("current", "task-1", "2026-08-15", now);
    current.shape_checks = vec![serde_json::json!({
        "rule": "business_date_range",
        "passed": true,
        "message": "ok",
    })];
    current.mapping_issues = vec![serde_json::json!({
        "column": "V_TEXT",
        "source": "VARCHAR2(200)",
        "target": "missing",
        "rule": "目标表缺列",
        "message": null,
    })];
    store.insert(&current, now, 90).unwrap();
    assert!(store.get("old").unwrap().is_none());
    let stored = store.get("current").unwrap().unwrap();
    assert_eq!(stored.shape_checks, current.shape_checks);
    assert_eq!(stored.mapping_issues, current.mapping_issues);

    store
        .seal_incomplete(UnknownReason::ServiceRestarted, now, 90)
        .unwrap();
    let sealed = store.get("current").unwrap().unwrap();
    assert_eq!(sealed.outcome.as_deref(), Some("FAILED"));
    assert_eq!(sealed.unknown_reason.as_deref(), Some("SERVICE_RESTARTED"));
    assert_eq!(sealed.source_code, None);
    assert_eq!(sealed.sink_code, None);
    assert_eq!(sealed.target_table_effect, None);

    fs::remove_dir_all(directory).unwrap();
}
