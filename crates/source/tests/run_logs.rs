//! 原始运行日志行的落库、截断与保留期。
//!
//! 接口那一层（`GET /api/runs/{}/logs`）的用例在 `api.rs` 里，走的是真的
//! `Api::handle`；这里问的是存储本身：存进去的是不是原文、业务值有没有被截、
//! 到期的行会不会在下一次写入时被顺手清掉。

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{TimeDelta, TimeZone, Utc};
use db_qbs_source::{
    truncate_business_values, RunLogStore, BUSINESS_VALUE_MAX_CHARS, RUN_LOG_PAGE_LIMIT,
    RUN_LOG_RETENTION_DAYS, RUN_LOG_RETENTION_RUNS_PER_TASK,
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

fn temp_directory() -> std::path::PathBuf {
    let suffix = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "db-qbs-source-run-logs-test-{}-{suffix}",
        std::process::id()
    ));
    fs::create_dir(&path).unwrap();
    path
}

fn store() -> (std::path::PathBuf, RunLogStore) {
    let directory = temp_directory();
    let store = RunLogStore::open(&directory).unwrap();
    (directory, store)
}

fn line(event: &str) -> String {
    format!(
        r#"{{"ts":"2026-08-15T10:00:00.000Z","level":"info","event":"{event}","run_id":"run-1","task":null,"component":"source-run"}}"#
    )
}

#[test]
fn a_stored_line_comes_back_byte_for_byte() {
    let (_directory, store) = store();
    let now = Utc::now();
    let started_at_ms = now.timestamp_millis();
    let original = line("source_started");
    store
        .append("record-1", "task-1", started_at_ms, 1, &original, now)
        .unwrap();

    let lines = store.lines_after("record-1", 0).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].seq, 1);
    // 存的是**原文**，不是解析后的结构，也不是渲染过的句子。
    assert_eq!(lines[0].line, original);
}

#[test]
fn a_line_that_is_not_json_is_stored_verbatim_too() {
    let (_directory, store) = store();
    let now = Utc::now();
    store
        .append(
            "record-1",
            "task-1",
            now.timestamp_millis(),
            1,
            "Segmentation fault (core dumped)",
            now,
        )
        .unwrap();

    let lines = store.lines_after("record-1", 0).unwrap();
    assert_eq!(lines[0].line, "Segmentation fault (core dumped)");
}

#[test]
fn the_cursor_returns_only_lines_after_it() {
    let (_directory, store) = store();
    let now = Utc::now();
    let started_at_ms = now.timestamp_millis();
    for seq in 1..=5 {
        store
            .append(
                "record-1",
                "task-1",
                started_at_ms,
                seq,
                &line(&format!("event_{seq}")),
                now,
            )
            .unwrap();
    }

    let all = store.lines_after("record-1", 0).unwrap();
    assert_eq!(
        all.iter().map(|line| line.seq).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
    let tail = store.lines_after("record-1", 3).unwrap();
    assert_eq!(tail.iter().map(|line| line.seq).collect::<Vec<_>>(), [4, 5]);
    // 游标停在末尾就是「没有新行」，不是错。
    assert!(store.lines_after("record-1", 5).unwrap().is_empty());
    // 另一条运行的行不会串进来。
    assert!(store.lines_after("record-2", 0).unwrap().is_empty());
}

#[test]
fn one_page_is_capped_and_the_next_cursor_picks_up_where_it_stopped() {
    let (_directory, store) = store();
    let now = Utc::now();
    let started_at_ms = now.timestamp_millis();
    let total = RUN_LOG_PAGE_LIMIT as i64 + 7;
    for seq in 1..=total {
        store
            .append("record-1", "task-1", started_at_ms, seq, &line("tick"), now)
            .unwrap();
    }

    let first = store.lines_after("record-1", 0).unwrap();
    assert_eq!(first.len(), RUN_LOG_PAGE_LIMIT);
    let rest = store
        .lines_after("record-1", first.last().unwrap().seq)
        .unwrap();
    assert_eq!(rest.len(), 7);
    assert_eq!(rest.last().unwrap().seq, total);
}

#[test]
fn a_long_business_value_is_truncated_before_it_reaches_the_database() {
    let (_directory, store) = store();
    let now = Utc::now();
    let value = "值".repeat(200);
    let failure = format!(
        r#"{{"ts":"2026-08-15T10:00:07.000Z","level":"error","event":"run_finished","run_id":"run-1","task":null,"terminal":"FAILED","column":"AMOUNT","value":"{value}","message":"目标端拒绝"}}"#
    );
    store
        .append(
            "record-1",
            "task-1",
            now.timestamp_millis(),
            1,
            &failure,
            now,
        )
        .unwrap();

    let stored: serde_json::Value =
        serde_json::from_str(&store.lines_after("record-1", 0).unwrap()[0].line).unwrap();
    assert_eq!(
        stored["value"].as_str().unwrap().chars().count(),
        BUSINESS_VALUE_MAX_CHARS
    );
    assert_eq!(stored["value_truncated"], true);
    // 列名照留：判断「是哪一列出的问题」全靠它，它也不是业务数据。
    assert_eq!(stored["column"], "AMOUNT");
    assert_eq!(stored["message"], "目标端拒绝");
}

#[test]
fn a_short_business_value_is_left_alone() {
    let short = r#"{"event":"run_finished","column":"AMOUNT","value":"12.34"}"#;
    assert_eq!(truncate_business_values(short), short);
    // 截断按**字符**数，不按字节：多字节值不能被切成半个字符。
    let boundary = format!(r#"{{"value":"{}"}}"#, "值".repeat(BUSINESS_VALUE_MAX_CHARS));
    assert_eq!(truncate_business_values(&boundary), boundary);
    // 非 JSON 与非字符串的 value 都原样通过。
    assert_eq!(truncate_business_values("not json"), "not json");
    let numeric = r#"{"value":123}"#;
    assert_eq!(truncate_business_values(numeric), numeric);
}

#[test]
fn lines_older_than_the_retention_window_are_purged_on_the_next_write() {
    let (_directory, store) = store();
    let now = Utc.with_ymd_and_hms(2026, 8, 15, 10, 0, 0).unwrap();
    let stale = now - TimeDelta::try_days(RUN_LOG_RETENTION_DAYS as i64 + 1).unwrap();
    store
        .append(
            "old-record",
            "task-1",
            stale.timestamp_millis(),
            1,
            &line("source_started"),
            stale,
        )
        .unwrap();
    assert_eq!(store.lines_after("old-record", 0).unwrap().len(), 1);

    // 一次新的写入就是清理的时机——没有后台任务，也不需要一个。
    store
        .append(
            "fresh-record",
            "task-2",
            now.timestamp_millis(),
            1,
            &line("source_started"),
            now,
        )
        .unwrap();
    assert!(store.lines_after("old-record", 0).unwrap().is_empty());
    assert_eq!(store.lines_after("fresh-record", 0).unwrap().len(), 1);
}

#[test]
fn only_the_most_recent_runs_of_a_task_keep_their_lines() {
    let (_directory, store) = store();
    let now = Utc.with_ymd_and_hms(2026, 8, 15, 10, 0, 0).unwrap();
    let keep = RUN_LOG_RETENTION_RUNS_PER_TASK as i64;
    // 全都在 7 天之内，于是裁掉的只可能是「每任务 10 次」那一条规则。
    for index in 0..keep + 5 {
        let started = now - TimeDelta::try_minutes(keep + 5 - index).unwrap();
        store
            .append(
                &format!("record-{index}"),
                "task-1",
                started.timestamp_millis(),
                1,
                &line("source_started"),
                now,
            )
            .unwrap();
    }

    for index in 0..5 {
        assert!(
            store
                .lines_after(&format!("record-{index}"), 0)
                .unwrap()
                .is_empty(),
            "第 {index} 次运行的原文本该被挤掉"
        );
    }
    for index in 5..keep + 5 {
        assert_eq!(
            store
                .lines_after(&format!("record-{index}"), 0)
                .unwrap()
                .len(),
            1,
            "第 {index} 次运行的原文本该还在"
        );
    }

    // 另一条任务的次数自己算，不受这条任务刷屏的影响。
    store
        .append(
            "other-task-record",
            "task-2",
            now.timestamp_millis(),
            1,
            &line("source_started"),
            now,
        )
        .unwrap();
    assert_eq!(store.lines_after("other-task-record", 0).unwrap().len(), 1);
}
