use std::sync::Arc;

use db_qbs_sink::test_support::{datetime_target_column, InMemoryDestination};
use db_qbs_sink::{
    AtomicSwapError, BatchPayload, DropStagingError, OpenRunRequest, SinkService, SourceColumn,
    TargetColumn, TargetConnection, TargetKey, WriteMode,
};

const RUN_ID: &str = "20260814091530_a3f19c";

/// 这些用例的目标表是同一张：单列 `D_BIZ datetime`，主键就是它。
///
/// `purged_rows` 与 `count_ms` 给的是**不像默认值的值**：生产里 `purged_rows` 恒为 0
/// （ADR-0035 §4），所以只有让 fake 报一个 7，才证得出这两个数是被一路带出来的、
/// 不是在出口处现编的。
fn destination() -> InMemoryDestination {
    let destination = InMemoryDestination {
        columns: vec![datetime_target_column("D_BIZ")],
        ..InMemoryDestination::default()
    };
    *destination.purged_rows.lock().unwrap() = 7;
    *destination.count_ms.lock().unwrap() = 4;
    destination
}

fn open_request() -> OpenRunRequest {
    open_request_for(RUN_ID)
}

fn open_request_for(run_id: &str) -> OpenRunRequest {
    OpenRunRequest {
        run_id: run_id.to_owned(),
        target_table: "T_POSITION".to_owned(),
        target: TargetConnection {
            host: "127.0.0.1".to_owned(),
            port: 3306,
            username: "sink".to_owned(),
            password: "change-me".to_owned(),
            database: "qbs".to_owned(),
        },
        write_mode: WriteMode::Append,
        pre_sql: None,
        primary_key: vec!["D_BIZ".to_owned()],
        source_columns: vec![SourceColumn {
            name: "D_BIZ".to_owned(),
            data_type: "DATE".to_owned(),
            precision: None,
            scale: None,
            length: None,
            fsp: None,
            support: None,
        }],
        range_check_results: None,
    }
}

fn one_row() -> BatchPayload {
    BatchPayload {
        seq: 1,
        rows: vec![vec![Some("2026-08-14 12:34:56".to_owned())]],
    }
}

#[test]
fn commit_atomically_swaps_then_exposes_a_sealed_swapped_tombstone() {
    let destination = Arc::new(destination());
    let service = SinkService::new("qbs", destination.clone());
    service.open(open_request()).unwrap();
    service.write_batch(RUN_ID, one_row()).unwrap();

    let committed = service.commit(RUN_ID, 1, 1).unwrap();

    assert_eq!(committed.source_rows, 1);
    assert_eq!(committed.staged_rows, 1);
    assert_eq!(committed.purged_rows, 7);
    assert_eq!(committed.swapped_rows, 1);
    assert_eq!(committed.count_ms, 4);
    let requests = destination.swap_requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].target_table, "T_POSITION");
    assert_eq!(requests[0].primary_key, vec!["D_BIZ".to_owned()]);
    assert_eq!(requests[0].columns, vec!["D_BIZ".to_owned()]);
    drop(requests);
    assert_eq!(destination.dropped.lock().unwrap().len(), 1);

    let status = service.get(RUN_ID).unwrap();
    let value = serde_json::to_value(status).unwrap();
    assert_eq!(value["terminal"], "SWAPPED");
    assert_eq!(value["sealed"], true);
    assert_eq!(value["purged_rows"], 7);
    assert_eq!(value["swapped_rows"], 1);

    let commit_error = service.commit(RUN_ID, 1, 1).unwrap_err();
    assert_eq!(commit_error.status, 409);
    assert_eq!(commit_error.code, "RUN_SEALED");

    let error = service
        .write_batch(
            RUN_ID,
            BatchPayload {
                seq: 2,
                rows: vec![],
            },
        )
        .unwrap_err();
    assert_eq!(error.status, 409);
    assert_eq!(error.code, "RUN_SEALED");
}

/// #264：清空后导入这一档在编排上是什么样。
///
/// 三件事一起钉，因为它们必须同时为真才算这个模式成立：
///
/// 1. 写入模式**原样传到了切换请求上**——切换那一端才是真正执行 DELETE 的地方，
///    半路把它丢掉，整个模式就只剩界面上一个选项；
/// 2. 目标表里原来那些行没了，只剩本次运行推上去的那一份；
/// 3. 终态是 `REPLACED` 而不是 `SWAPPED`。这两个词不能共用：`SWAPPED` 的意思是
///    「按主键合并进目标表」，拿它描述一次整表替换，运行历史读起来就是假的。
///
/// `purged_rows` 报的是**真删掉的行数**（这里是 2），不是 fake 上那个 7——
/// 清空模式下那个旋钮被忽略，因为这个数在这条路上是有意义的事实。
#[test]
fn a_clear_then_import_run_replaces_the_target_and_says_replaced() {
    let destination = Arc::new(destination());
    // 目标表里先躺着两行旧数据，它们正是这次运行要清掉的东西。
    destination.target_rows.lock().unwrap().insert(
        (
            "T_POSITION".to_owned(),
            "[\"2026-01-01 00:00:00\"]".to_owned(),
        ),
        vec![Some("2026-01-01 00:00:00".to_owned())],
    );
    destination.target_rows.lock().unwrap().insert(
        (
            "T_POSITION".to_owned(),
            "[\"2026-01-02 00:00:00\"]".to_owned(),
        ),
        vec![Some("2026-01-02 00:00:00".to_owned())],
    );
    let service = SinkService::new("qbs", destination.clone());
    service
        .open(OpenRunRequest {
            write_mode: WriteMode::ClearThenImport,
            ..open_request()
        })
        .unwrap();
    service.write_batch(RUN_ID, one_row()).unwrap();

    let committed = service.commit(RUN_ID, 1, 1).unwrap();

    assert_eq!(
        committed.purged_rows, 2,
        "清空模式下 purged_rows 是真删掉的行数，不再是恒 0"
    );
    assert_eq!(committed.swapped_rows, 1);
    let requests = destination.swap_requests.lock().unwrap();
    assert_eq!(requests[0].write_mode, WriteMode::ClearThenImport);
    drop(requests);
    assert_eq!(
        destination.target_row_values("T_POSITION"),
        vec![vec![Some("2026-08-14 12:34:56".to_owned())]],
        "跑完之后目标表精确等于本次运行推上去的那一份"
    );

    let value = serde_json::to_value(service.get(RUN_ID).unwrap()).unwrap();
    assert_eq!(value["terminal"], "REPLACED");
    assert_eq!(value["purged_rows"], 2);
}

/// #264：追加写那一档一个字都没变——写入模式过线，但它不清空任何东西。
///
/// 这条是上面那条的对照。同一段编排、同一张目标表，只有模式不同：旧行还在，
/// 终态仍是 `SWAPPED`。
#[test]
fn an_append_run_still_leaves_what_was_there_and_says_swapped() {
    let destination = Arc::new(destination());
    destination.target_rows.lock().unwrap().insert(
        (
            "T_POSITION".to_owned(),
            "[\"2026-01-01 00:00:00\"]".to_owned(),
        ),
        vec![Some("2026-01-01 00:00:00".to_owned())],
    );
    let service = SinkService::new("qbs", destination.clone());
    service.open(open_request()).unwrap();
    service.write_batch(RUN_ID, one_row()).unwrap();

    service.commit(RUN_ID, 1, 1).unwrap();

    let requests = destination.swap_requests.lock().unwrap();
    assert_eq!(requests[0].write_mode, WriteMode::Append);
    drop(requests);
    assert_eq!(destination.target_row_values("T_POSITION").len(), 2);
    let value = serde_json::to_value(service.get(RUN_ID).unwrap()).unwrap();
    assert_eq!(value["terminal"], "SWAPPED");
}

#[test]
fn append_with_pre_sql_reports_the_cleanup_count_and_distinct_terminal() {
    let destination = Arc::new(destination());
    let service = SinkService::new("qbs", destination.clone());
    let pre_sql = "/* exact */\nDELETE FROM `qbs`.`T_POSITION` WHERE DATE(D_BIZ) < CURRENT_DATE AND D_BIZ IN (SELECT D_BIZ FROM qbs.STALE_POSITION);";
    service
        .open(OpenRunRequest {
            pre_sql: Some(pre_sql.to_owned()),
            ..open_request()
        })
        .unwrap();
    service.write_batch(RUN_ID, one_row()).unwrap();

    let committed = service.commit(RUN_ID, 1, 1).unwrap();

    assert_eq!(committed.purged_rows, 7);
    let requests = destination.swap_requests.lock().unwrap();
    assert_eq!(requests[0].pre_sql.as_deref(), Some(pre_sql));
    drop(requests);
    let value = serde_json::to_value(service.get(RUN_ID).unwrap()).unwrap();
    assert_eq!(value["terminal"], "CLEANED_AND_SWAPPED");
    assert_eq!(value["purged_rows"], 7);
}

#[test]
fn open_revalidates_pre_sql_before_creating_staging() {
    let destination = Arc::new(destination());
    let service = SinkService::new("qbs", destination.clone());

    let error = service
        .open(OpenRunRequest {
            pre_sql: Some("DELETE FROM qbs.OTHER WHERE D_BIZ < CURRENT_DATE".to_owned()),
            ..open_request()
        })
        .unwrap_err();

    assert_eq!(error.status, 400);
    assert_eq!(error.code, "BAD_REQUEST");
    assert!(destination.created.lock().unwrap().is_empty());
}

/// #264：清空是**导入的一部分**，不是它前面的一步。
///
/// 行数门禁没过，切换整个不发生——包括清空。替身把这件事做成了顺序上的事实
/// （先判、再删、再插，一起返回），真目的地把它做成事务：两边说的是同一句话，
/// 而这条用例是替身那一半的守卫。目标表还剩那两行旧数据。
#[test]
fn a_clear_then_import_that_fails_its_gate_does_not_clear_anything() {
    let destination = Arc::new(destination());
    destination.target_rows.lock().unwrap().insert(
        (
            "T_POSITION".to_owned(),
            "[\"2026-01-01 00:00:00\"]".to_owned(),
        ),
        vec![Some("2026-01-01 00:00:00".to_owned())],
    );
    let service = SinkService::new("qbs", destination.clone());
    service
        .open(OpenRunRequest {
            write_mode: WriteMode::ClearThenImport,
            ..open_request()
        })
        .unwrap();
    service.write_batch(RUN_ID, one_row()).unwrap();
    destination.lose_staged_rows(1);

    let error = service.commit(RUN_ID, 1, 1).unwrap_err();

    assert_eq!(error.code, "VERIFY_FAILED");
    assert_eq!(
        destination.target_row_values("T_POSITION").len(),
        1,
        "导入没发生，清空也就没发生：目标表原样不动"
    );
    let value = serde_json::to_value(service.get(RUN_ID).unwrap()).unwrap();
    assert_eq!(value["terminal"], "DISCARDED");
}

#[test]
fn verification_failure_reports_database_loss_and_discards_staging() {
    let destination = Arc::new(destination());
    let service = SinkService::new("qbs", destination.clone());
    service.open(open_request()).unwrap();
    service.write_batch(RUN_ID, one_row()).unwrap();
    // 写入说了「收下了」，行却没在暂存表里——门禁存在的理由就是这一幕。
    destination.lose_staged_rows(1);

    let error = service.commit(RUN_ID, 1, 1).unwrap_err();

    assert_eq!(error.status, 409);
    assert_eq!(error.code, "VERIFY_FAILED");
    assert_eq!(error.details["source_rows"], 1);
    assert_eq!(error.details["staged_rows"], 0);
    assert_eq!(error.details["source_batches"], 1);
    assert_eq!(error.details["received_batches"], 1);
    assert_eq!(error.details["sink_reported_rows"], 1);
    assert_eq!(error.details["count_ms"], 4);
    assert!(error.message.contains("数据在写入 MySQL 的过程中丢失"));
    assert!(error
        .message
        .ends_with("目标表未被触碰，可直接重跑；重跑仍失败请报 issue。"));
    assert_eq!(destination.dropped.lock().unwrap().len(), 1);
    assert_eq!(
        serde_json::to_value(service.get(RUN_ID).unwrap()).unwrap()["terminal"],
        "DISCARDED"
    );
}

#[test]
fn verification_failure_reports_a_missing_batch_and_discards_staging() {
    let destination = Arc::new(destination());
    let service = SinkService::new("qbs", destination.clone());
    service.open(open_request()).unwrap();

    let error = service.commit(RUN_ID, 1, 1).unwrap_err();

    assert_eq!(error.code, "VERIFY_FAILED");
    assert_eq!(error.details["source_rows"], 1);
    assert_eq!(error.details["staged_rows"], 0);
    assert_eq!(error.details["source_batches"], 1);
    assert_eq!(error.details["received_batches"], 0);
    assert_eq!(error.details["sink_reported_rows"], 0);
    assert_eq!(error.details["count_ms"], 4);
    assert!(error.message.contains("有批次未送达"));
    assert!(error.message.ends_with("目标表未被触碰，可直接重跑。"));
    assert_eq!(destination.dropped.lock().unwrap().len(), 1);
}

#[test]
fn zero_row_commit_is_valid_and_returns_the_actual_purge_count() {
    let destination = Arc::new(destination());
    *destination.purged_rows.lock().unwrap() = 9;
    let service = SinkService::new("qbs", destination);
    service.open(open_request()).unwrap();

    let committed = service.commit(RUN_ID, 0, 0).unwrap();

    assert_eq!(committed.source_rows, 0);
    assert_eq!(committed.staged_rows, 0);
    assert_eq!(committed.purged_rows, 9);
    assert_eq!(committed.swapped_rows, 0);
}

#[test]
fn swap_failure_discards_staging_and_records_a_discarded_tombstone() {
    let destination = Arc::new(destination());
    *destination.swap_error.lock().unwrap() =
        Some(AtomicSwapError::Other("duplicate target key".to_owned()));
    let service = SinkService::new("qbs", destination.clone());
    service.open(open_request()).unwrap();
    service.write_batch(RUN_ID, one_row()).unwrap();

    let error = service.commit(RUN_ID, 1, 1).unwrap_err();

    assert_eq!(error.status, 500);
    assert_eq!(error.code, "SWAP_FAILED");
    assert!(error.message.contains("目标表未被触碰，可直接重跑"));
    assert_eq!(destination.dropped.lock().unwrap().len(), 1);
    assert_eq!(
        serde_json::to_value(service.get(RUN_ID).unwrap()).unwrap()["terminal"],
        "DISCARDED"
    );
}

#[test]
fn lock_wait_timeout_and_deadlock_report_target_busy() {
    for errno in [1205, 1213] {
        let destination = Arc::new(destination());
        *destination.swap_error.lock().unwrap() = Some(AtomicSwapError::TargetBusy { errno });
        let service = SinkService::new("qbs", destination.clone());
        service.open(open_request()).unwrap();
        service.write_batch(RUN_ID, one_row()).unwrap();

        let error = service.commit(RUN_ID, 1, 1).unwrap_err();

        assert_eq!((error.status, error.code), (409, "SWAP_TARGET_BUSY"));
        assert_eq!(error.details["target_table"], "T_POSITION");
        assert_eq!(error.details["errno"], errno);
        assert!(error.message.contains("另一个 run"), "{}", error.message);
        assert!(error.message.contains("重跑即可"), "{}", error.message);
        assert_eq!(destination.dropped.lock().unwrap().len(), 1);
        assert_eq!(
            serde_json::to_value(service.get(RUN_ID).unwrap()).unwrap()["terminal"],
            "DISCARDED"
        );
    }
}

#[test]
fn drop_failure_after_successful_swap_still_records_a_swapped_tombstone() {
    let destination = Arc::new(destination());
    *destination.drop_error.lock().unwrap() = Some(DropStagingError::PermissionDenied);
    let service = SinkService::new("qbs", destination.clone());
    service.open(open_request()).unwrap();
    service.write_batch(RUN_ID, one_row()).unwrap();

    let error = service.commit(RUN_ID, 1, 1).unwrap_err();

    assert_eq!(error.status, 500);
    assert_eq!(error.code, "SWAP_FAILED");
    assert!(
        error.message.contains("目标表已完成切换"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("缺少 DROP 权限"),
        "{}",
        error.message
    );

    let status = serde_json::to_value(service.get(RUN_ID).unwrap()).unwrap();
    assert_eq!(status["terminal"], "SWAPPED");
    assert_eq!(status["sealed"], true);

    let retry = service.commit(RUN_ID, 1, 1).unwrap_err();
    assert_eq!(retry.code, "RUN_SEALED");
    assert_eq!(destination.swap_requests.lock().unwrap().len(), 1);
}

#[test]
fn drop_failure_after_verify_failure_keeps_the_gate_numbers_and_discards_the_run() {
    let destination = Arc::new(destination());
    *destination.drop_error.lock().unwrap() =
        Some(DropStagingError::Other("connection lost".to_owned()));
    let service = SinkService::new("qbs", destination.clone());
    service.open(open_request()).unwrap();
    service.write_batch(RUN_ID, one_row()).unwrap();
    destination.lose_staged_rows(1);

    let error = service.commit(RUN_ID, 1, 1).unwrap_err();

    assert_eq!(error.status, 409);
    assert_eq!(error.code, "VERIFY_FAILED");
    assert_eq!(error.details["source_rows"], 1);
    assert_eq!(error.details["staged_rows"], 0);
    assert_eq!(error.details["source_batches"], 1);
    assert_eq!(error.details["received_batches"], 1);
    assert_eq!(error.details["sink_reported_rows"], 1);
    assert_eq!(error.details["count_ms"], 4);
    assert!(error.message.contains("清理失败"), "{}", error.message);
    assert!(error.message.contains("需手工 DROP"), "{}", error.message);
    assert_eq!(
        serde_json::to_value(service.get(RUN_ID).unwrap()).unwrap()["terminal"],
        "DISCARDED"
    );
}

#[test]
fn drop_failure_after_swap_failure_keeps_swap_failed_and_discards_the_run() {
    let destination = Arc::new(destination());
    *destination.swap_error.lock().unwrap() =
        Some(AtomicSwapError::Other("duplicate target key".to_owned()));
    *destination.drop_error.lock().unwrap() =
        Some(DropStagingError::Other("connection lost".to_owned()));
    let service = SinkService::new("qbs", destination.clone());
    service.open(open_request()).unwrap();
    service.write_batch(RUN_ID, one_row()).unwrap();

    let error = service.commit(RUN_ID, 1, 1).unwrap_err();

    assert_eq!(error.status, 500);
    assert_eq!(error.code, "SWAP_FAILED");
    assert!(
        error.message.contains("duplicate target key"),
        "{}",
        error.message
    );
    assert!(error.message.contains("清理失败"), "{}", error.message);
    assert_eq!(
        serde_json::to_value(service.get(RUN_ID).unwrap()).unwrap()["terminal"],
        "DISCARDED"
    );
}

#[test]
fn abort_records_discarded_tombstones_and_the_33rd_evicts_the_oldest() {
    let destination = Arc::new(destination());
    let service = SinkService::new("qbs", destination);
    let run_ids = (0..33)
        .map(|index| format!("20260814{index:06}_{index:06x}"))
        .collect::<Vec<_>>();

    for run_id in &run_ids {
        service.open(open_request_for(run_id)).unwrap();
        assert!(service.abort(run_id).unwrap().staging_dropped);
    }

    assert_eq!(service.get(&run_ids[0]).unwrap_err().status, 404);
    let retained = serde_json::to_value(service.get(&run_ids[1]).unwrap()).unwrap();
    assert_eq!(retained["terminal"], "DISCARDED");
    assert_eq!(retained["sealed"], false);
    let batch_error = service.write_batch(&run_ids[1], one_row()).unwrap_err();
    assert_eq!(batch_error.status, 404);
    assert_eq!(batch_error.code, "RUN_UNKNOWN");
}

// ---------------------------------------------------------------------------
// 无主键目标表：纯追加写（#261）
// ---------------------------------------------------------------------------

/// 一张**没有任何唯一约束**的目标表，单列 `D_BIZ datetime NULL`。
///
/// `nullable: true` 不是无所谓的细节：没有主键就没有哪一列能豁免「目标列必须可空」
/// 那条预检，一张 NOT NULL 的无主键表本来就该被拦下来。
fn keyless_destination() -> InMemoryDestination {
    InMemoryDestination {
        columns: vec![TargetColumn {
            nullable: true,
            ..datetime_target_column("D_BIZ")
        }],
        keys: Vec::new(),
        ..InMemoryDestination::default()
    }
}

fn keyless_open_request(run_id: &str) -> OpenRunRequest {
    OpenRunRequest {
        primary_key: Vec::new(),
        ..open_request_for(run_id)
    }
}

fn row_at(seq: u64, value: &str) -> BatchPayload {
    BatchPayload {
        seq,
        rows: vec![vec![Some(value.to_owned())]],
    }
}

#[test]
fn a_target_table_without_a_primary_key_opens_and_commits_as_a_plain_append() {
    let destination = Arc::new(keyless_destination());
    let service = SinkService::new("qbs", destination.clone());

    service.open(keyless_open_request(RUN_ID)).unwrap();
    service.write_batch(RUN_ID, one_row()).unwrap();
    let committed = service.commit(RUN_ID, 1, 1).unwrap();

    // 严格相等，不是区间：纯 INSERT 没有更新那条腿（#261）。
    assert_eq!(committed.staged_rows, committed.swapped_rows);
    let requests = destination.swap_requests.lock().unwrap();
    assert!(
        requests[0].primary_key.is_empty(),
        "空主键必须原样传到切换请求上——语句形状就是从它派生的"
    );
    drop(requests);
    assert_eq!(
        serde_json::to_value(service.get(RUN_ID).unwrap()).unwrap()["terminal"],
        "SWAPPED"
    );
}

#[test]
fn re_running_a_keyless_task_doubles_the_target_table_and_that_is_the_known_cost() {
    // 这个产品第一次接受「同一任务定义跑两次，目标表状态不同」。它不是缺陷，
    // 是这条路的定义——所以它被钉在这里，而不是靠人记得。
    let destination = Arc::new(keyless_destination());
    let service = SinkService::new("qbs", destination.clone());

    for run_id in ["20260814091530_a3f19c", "20260814091531_b4e28d"] {
        service.open(keyless_open_request(run_id)).unwrap();
        service
            .write_batch(run_id, row_at(1, "2026-08-14 12:34:56"))
            .unwrap();
        service.commit(run_id, 1, 1).unwrap();
    }

    let rows = destination.target_row_values("T_POSITION");
    assert_eq!(rows.len(), 2, "两次跑同一批数据，目标表里就是两份");
    assert_eq!(rows[0], rows[1]);
}

#[test]
fn a_task_recorded_as_keyless_is_refused_when_the_target_table_has_grown_a_unique_key() {
    // 写法绝不静默切换：任务定义记的是「无主键」，目标表却有唯一约束——
    // 这时改走 upsert 等于同一份任务定义在没人改过它的情况下换了语义。
    let destination = Arc::new(InMemoryDestination {
        keys: vec![TargetKey {
            name: "PRIMARY".to_owned(),
            columns: vec!["D_BIZ".to_owned()],
        }],
        ..keyless_destination()
    });
    let service = SinkService::new("qbs", destination.clone());

    let error = service.open(keyless_open_request(RUN_ID)).unwrap_err();

    // 拦截点仍然只有映射预检那一个。
    assert_eq!(error.status, 422);
    assert_eq!(error.code, "PRECHECK_FAILED");
    let issues = error.details["issues"].as_array().unwrap();
    assert!(
        issues
            .iter()
            .any(|issue| issue["rule"].as_str().unwrap().contains("写法不会静默切换")),
        "{issues:?}"
    );
    assert!(
        destination.created.lock().unwrap().is_empty(),
        "拒跑就是什么都没建"
    );
}

#[test]
fn the_no_primary_key_conclusion_comes_back_from_the_target_check() {
    use db_qbs_sink::{TargetCheckRequest, APPEND_ONLY_CONCLUSION};

    let destination = Arc::new(keyless_destination());
    let service = SinkService::new("qbs", destination);

    let result = service
        .check_target(TargetCheckRequest {
            target: open_request().target,
            target_table: "T_POSITION".to_owned(),
            source_columns: open_request().source_columns,
            primary_key: Vec::new(),
        })
        .unwrap();

    // 结论不是 finding：检查是**通过**的，通过之后还有一句话必须被读到（#261）。
    assert!(result.ok);
    assert!(result.findings.is_empty());
    assert_eq!(result.notes, vec![APPEND_ONLY_CONCLUSION.to_owned()]);
}

#[test]
fn a_target_table_with_a_primary_key_says_nothing_extra() {
    use db_qbs_sink::TargetCheckRequest;

    let destination = Arc::new(destination());
    let service = SinkService::new("qbs", destination);

    let result = service
        .check_target(TargetCheckRequest {
            target: open_request().target,
            target_table: "T_POSITION".to_owned(),
            source_columns: open_request().source_columns,
            primary_key: vec!["D_BIZ".to_owned()],
        })
        .unwrap();

    assert!(result.ok);
    assert!(result.notes.is_empty(), "有主键就是今天的行为，一个字不多");
}
