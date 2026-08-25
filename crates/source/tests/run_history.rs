use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{TimeDelta, TimeZone, Utc};
use db_qbs_source::{
    expired_history_indices, fold_history_lines, HistoryStore, RunHistory, UnknownReason,
};

const SOURCE_SQL: &str = "SELECT a.ID AS ID\n  FROM APP.ORDERS a\n WHERE D_BIZ = DATE '2026-08-14'";

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
        r#"{"ts":"2026-08-15T10:00:00.000Z","event":"source_started","run_id":null,"message":"source one-shot process started; the task spec is the only source of truth"}"#,
        r#"{"ts":"2026-08-15T10:00:01.000Z","event":"stage_changed","run_id":"run-7","stage":"PREPARING"}"#,
        r#"{"ts":"2026-08-15T10:00:02.000Z","event":"run_opened","run_id":"run-7","staging_table":"STG_7"}"#,
        r#"{"ts":"2026-08-15T10:00:03.000Z","event":"stage_changed","run_id":"run-7","stage":"STREAMING"}"#,
        r#"{"ts":"2026-08-15T10:00:04.000Z","event":"batch_pushed","run_id":"run-7","seq":1,"rows":3,"bytes":100,"ms":10}"#,
        r#"{"ts":"2026-08-15T10:00:05.000Z","event":"stage_changed","run_id":"run-7","stage":"COMMITTING"}"#,
        r#"{"ts":"2026-08-15T10:00:06.000Z","event":"commit_diagnosed","run_id":"run-7","terminal":"DISCARDED","message":"sink tombstone"}"#,
        r#"{"ts":"2026-08-15T10:00:07.000Z","event":"run_finished","run_id":"run-7","terminal":"FAILED","stage":"COMMITTING","source_code":null,"sink_code":"VERIFY_FAILED","column":"AMOUNT","value":"secret","message":"counts differ","failure_kind":"VERIFY_FAILED","source_rows":3,"source_batches":1,"staged_rows":3,"received_batches":1,"sink_reported_rows":2,"purged_rows":0,"fetch_ms":4,"push_ms":10,"commit_ms":6,"count_ms":2,"cursor_ms":1}"#,
    ];

    let history =
        fold_history_lines("record-1", "task-1", SOURCE_SQL, accepted_at, &lines).unwrap();

    assert_eq!(history.run_record_id, "record-1");
    assert_eq!(history.task_id, "task-1");
    assert_eq!(history.run_id.as_deref(), Some("run-7"));
    // 当次执行的 SQL 快照原样钉住：它回答「当时执行了什么」，规格之后怎么改都不动它。
    assert_eq!(history.source_sql, SOURCE_SQL);
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
    assert_eq!(history.failure_kind.as_deref(), Some("VERIFY_FAILED"));
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
        r#"{"ts":"2026-08-15T10:00:01.000Z","event":"mapping_precheck_failed","run_id":"run-7","column":"V_TEXT","source":"VARCHAR2(200)","target":"missing","rule":"目标表缺列","suggestion":"在目标表加列 VARCHAR(200) NULL"}"#,
        r#"{"ts":"2026-08-15T10:00:02.000Z","event":"mapping_precheck_failed","run_id":"run-7","column":"D_BIZ","source":"DATE","target":"VARCHAR(20)","rule":"类型不兼容"}"#,
        r#"{"ts":"2026-08-15T10:00:03.000Z","event":"stage_changed","run_id":"run-7","stage":"FAILED"}"#,
        r#"{"ts":"2026-08-15T10:00:04.000Z","event":"run_finished","run_id":"run-7","terminal":"FAILED","stage":"PREPARING","source_code":null,"sink_code":"PRECHECK_FAILED","column":null,"value":null,"message":"映射预检未通过：一次发现 2 项问题","source_rows":0,"source_batches":0,"staged_rows":null,"received_batches":null,"sink_reported_rows":null,"purged_rows":null,"fetch_ms":0,"push_ms":0,"commit_ms":0,"count_ms":null,"cursor_ms":1}"#,
    ];

    let history =
        fold_history_lines("record-1", "task-1", SOURCE_SQL, accepted_at, &lines).unwrap();

    assert_eq!(history.mapping_issues.len(), 2);
    assert_eq!(history.mapping_issues[0]["column"], "V_TEXT");
    assert_eq!(
        history.mapping_issues[0]["suggestion"],
        "在目标表加列 VARCHAR(200) NULL"
    );
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
        fold_history_lines("record-1", "task-1", SOURCE_SQL, accepted_at, &lines).unwrap();

    assert_eq!(history.target_table_effect.as_deref(), Some("DISCARDED"));
}

#[test]
fn a_retired_early_failure_event_still_folds_without_inventing_a_run_id() {
    let accepted_at = Utc.with_ymd_and_hms(2026, 8, 15, 9, 59, 59).unwrap();
    let lines = [
        r#"{"ts":"2026-08-15T10:00:00.000Z","event":"source_started","run_id":null,"message":"source one-shot process started; the task spec is the only source of truth"}"#,
        // `sql_shape_precheck_failed` 已随 ADR-0036 §5 退役、不再产生；事件闭集只增不删，
        // 折叠器仍认它，这条用例守的是「早失败事件不编造 run_id」这个口径本身。
        r#"{"ts":"2026-08-15T10:00:01.000Z","event":"sql_shape_precheck_failed","run_id":null,"message":"two checks failed","failure_kind":"SHAPE_PRECHECK"}"#,
    ];

    let history =
        fold_history_lines("record-2", "task-1", SOURCE_SQL, accepted_at, &lines).unwrap();

    assert_eq!(history.run_record_id, "record-2");
    assert_eq!(history.run_id, None);
    assert_eq!(history.outcome.as_deref(), Some("FAILED"));
    assert_eq!(history.target_table_effect.as_deref(), Some("DISCARDED"));
    assert_eq!(history.message.as_deref(), Some("two checks failed"));
    assert_eq!(history.source_code, None);
    assert_eq!(history.sink_code, None);
    assert_eq!(history.failure_kind.as_deref(), Some("SHAPE_PRECHECK"));
}

#[test]
fn a_log_line_without_a_category_leaves_the_history_row_unclassified() {
    // 本功能之前落盘的历史行没有 failure_kind；读到缺席是「当时没记」，不是错误。
    let accepted_at = Utc.with_ymd_and_hms(2026, 8, 15, 9, 59, 59).unwrap();
    let lines = [
        r#"{"ts":"2026-08-15T10:00:02.000Z","event":"run_finished","run_id":"run-9","terminal":"FAILED","stage":"COMMITTING","source_code":null,"sink_code":"VERIFY_FAILED","column":null,"value":null,"message":"counts differ","source_rows":3,"source_batches":1,"staged_rows":2,"received_batches":1,"sink_reported_rows":3,"purged_rows":0,"fetch_ms":1,"push_ms":1,"commit_ms":1,"count_ms":1,"cursor_ms":1}"#,
    ];

    let history =
        fold_history_lines("record-9", "task-1", SOURCE_SQL, accepted_at, &lines).unwrap();

    assert_eq!(history.failure_kind, None);
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
    let old = RunHistory::accepted("old", "task-1", SOURCE_SQL, old_at);
    store.insert(&old, old_at, 90).unwrap();
    assert!(store.get("old").unwrap().is_some());

    let mut current = RunHistory::accepted("current", "task-1", SOURCE_SQL, now);
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
    assert_eq!(stored.mapping_issues, current.mapping_issues);
    // 运行参数与 SQL 快照都要经得起落盘再读回来——它们是历史行的事实那一半。
    assert_eq!(stored.source_sql, SOURCE_SQL);

    store
        .seal_incomplete(UnknownReason::ServiceRestarted, now, 90)
        .unwrap();
    let sealed = store.get("current").unwrap().unwrap();
    assert_eq!(sealed.outcome.as_deref(), Some("FAILED"));
    assert_eq!(sealed.unknown_reason.as_deref(), Some("SERVICE_RESTARTED"));
    // 启动清扫封的是「结局未知」，分类必须跟着落，否则历史行上只有人话说得清这一点。
    assert_eq!(sealed.failure_kind.as_deref(), Some("UNKNOWN"));
    assert_eq!(sealed.source_code, None);
    assert_eq!(sealed.sink_code, None);
    assert_eq!(sealed.target_table_effect, None);

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn cleanup_metadata_persists_the_run_target_and_becomes_single_use() {
    let suffix = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let directory = std::env::temp_dir().join(format!(
        "db-qbs-run-cleanup-test-{}-{suffix}",
        std::process::id()
    ));
    fs::create_dir(&directory).unwrap();
    let store = HistoryStore::open(&directory).unwrap();

    store
        .register_cleanup(
            "record-1",
            "target-ds",
            "ORDERS",
            &["ID".to_owned(), "TENANT".to_owned()],
        )
        .unwrap();
    let pending = store.cleanup("record-1").unwrap().unwrap();
    assert_eq!(pending.status, "pending");
    assert_eq!(pending.primary_key, vec!["ID", "TENANT"]);

    store.mark_cleanup_available("record-1").unwrap();
    assert_eq!(
        store.cleanup("record-1").unwrap().unwrap().status,
        "available"
    );
    store.mark_cleaned("record-1", 7).unwrap();
    let cleaned = store.cleanup("record-1").unwrap().unwrap();
    assert_eq!(cleaned.status, "cleaned");
    assert_eq!(cleaned.deleted_rows, Some(7));

    fs::remove_dir_all(directory).unwrap();
}
