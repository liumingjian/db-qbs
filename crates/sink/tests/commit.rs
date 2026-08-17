use std::sync::{Arc, Mutex};

use db_qbs_sink::{
    AtomicSwapError, AtomicSwapRequest, AtomicSwapResult, BatchPayload, CreateStagingError,
    Destination, DropStagingError, OpenRunRequest, SinkService, SourceColumn, TargetColumn,
    WriteBatchError,
};

const RUN_ID: &str = "20260814091530_a3f19c";

struct FakeDestination {
    swap_requests: Mutex<Vec<AtomicSwapRequest>>,
    staged_rows: Mutex<u64>,
    purged_rows: Mutex<u64>,
    swap_error: Mutex<Option<AtomicSwapError>>,
    drop_error: Mutex<Option<DropStagingError>>,
    dropped: Mutex<Vec<String>>,
}

impl FakeDestination {
    fn new() -> Self {
        Self {
            swap_requests: Mutex::new(Vec::new()),
            staged_rows: Mutex::new(1),
            purged_rows: Mutex::new(7),
            swap_error: Mutex::new(None),
            drop_error: Mutex::new(None),
            dropped: Mutex::new(Vec::new()),
        }
    }
}

impl Destination for FakeDestination {
    fn target_columns(&self, _target_table: &str) -> Result<Vec<TargetColumn>, String> {
        Ok(vec![TargetColumn {
            name: "D_BIZ".to_owned(),
            column_type: "datetime".to_owned(),
            data_type: "datetime".to_owned(),
            precision: None,
            scale: None,
            length: None,
            datetime_precision: Some(0),
            nullable: true,
            character_set: None,
            ordinal: 1,
        }])
    }

    fn create_staging(&self, _staging_table: &str, _ddl: &str) -> Result<(), CreateStagingError> {
        Ok(())
    }

    fn write_batch(
        &self,
        _staging_table: &str,
        _columns: &[String],
        rows: &[Vec<Option<String>>],
        _max_rows_per_insert: usize,
    ) -> Result<u64, WriteBatchError> {
        Ok(rows.len() as u64)
    }

    fn atomic_swap(
        &self,
        request: &AtomicSwapRequest,
    ) -> Result<AtomicSwapResult, AtomicSwapError> {
        self.swap_requests.lock().unwrap().push(request.clone());
        if let Some(error) = self.swap_error.lock().unwrap().take() {
            return Err(error);
        }
        let staged_rows = *self.staged_rows.lock().unwrap();
        if staged_rows != request.source_rows || request.received_batches != request.source_batches
        {
            return Err(AtomicSwapError::VerifyFailed {
                staged_rows,
                count_ms: 4,
            });
        }
        Ok(AtomicSwapResult {
            staged_rows,
            purged_rows: *self.purged_rows.lock().unwrap(),
            swapped_rows: staged_rows,
            count_ms: 4,
        })
    }

    fn drop_staging(&self, staging_table: &str) -> Result<(), DropStagingError> {
        if let Some(error) = self.drop_error.lock().unwrap().take() {
            return Err(error);
        }
        self.dropped.lock().unwrap().push(staging_table.to_owned());
        Ok(())
    }
}

fn open_request() -> OpenRunRequest {
    open_request_for(RUN_ID)
}

fn open_request_for(run_id: &str) -> OpenRunRequest {
    OpenRunRequest {
        run_id: run_id.to_owned(),
        target_table: "T_POSITION".to_owned(),
        target_date_col: "D_BIZ".to_owned(),
        biz_date: "2026-08-14".to_owned(),
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
    let destination = Arc::new(FakeDestination::new());
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
    assert_eq!(requests[0].target_date_col, "D_BIZ");
    assert_eq!(requests[0].biz_date_start, "2026-08-14");
    assert_eq!(requests[0].biz_date_end, "2026-08-15");
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

#[test]
fn verification_failure_reports_database_loss_and_discards_staging() {
    let destination = Arc::new(FakeDestination::new());
    *destination.staged_rows.lock().unwrap() = 0;
    let service = SinkService::new("qbs", destination.clone());
    service.open(open_request()).unwrap();
    service.write_batch(RUN_ID, one_row()).unwrap();

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
    let destination = Arc::new(FakeDestination::new());
    *destination.staged_rows.lock().unwrap() = 0;
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
    let destination = Arc::new(FakeDestination::new());
    *destination.staged_rows.lock().unwrap() = 0;
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
    let destination = Arc::new(FakeDestination::new());
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
        let destination = Arc::new(FakeDestination::new());
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
    let destination = Arc::new(FakeDestination::new());
    *destination.drop_error.lock().unwrap() = Some(DropStagingError::PermissionDenied);
    let service = SinkService::new("qbs", destination.clone());
    service.open(open_request()).unwrap();
    service.write_batch(RUN_ID, one_row()).unwrap();

    let error = service.commit(RUN_ID, 1, 1).unwrap_err();

    assert_eq!(error.status, 500);
    assert_eq!(error.code, "SWAP_FAILED");
    assert!(error.message.contains("目标表已完成切换"), "{}", error.message);
    assert!(error.message.contains("缺少 DROP 权限"), "{}", error.message);

    let status = serde_json::to_value(service.get(RUN_ID).unwrap()).unwrap();
    assert_eq!(status["terminal"], "SWAPPED");
    assert_eq!(status["sealed"], true);

    let retry = service.commit(RUN_ID, 1, 1).unwrap_err();
    assert_eq!(retry.code, "RUN_SEALED");
    assert_eq!(destination.swap_requests.lock().unwrap().len(), 1);
}

#[test]
fn drop_failure_after_verify_failure_keeps_the_gate_numbers_and_discards_the_run() {
    let destination = Arc::new(FakeDestination::new());
    *destination.staged_rows.lock().unwrap() = 0;
    *destination.drop_error.lock().unwrap() =
        Some(DropStagingError::Other("connection lost".to_owned()));
    let service = SinkService::new("qbs", destination.clone());
    service.open(open_request()).unwrap();
    service.write_batch(RUN_ID, one_row()).unwrap();

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
    let destination = Arc::new(FakeDestination::new());
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
    assert!(error.message.contains("duplicate target key"), "{}", error.message);
    assert!(error.message.contains("清理失败"), "{}", error.message);
    assert_eq!(
        serde_json::to_value(service.get(RUN_ID).unwrap()).unwrap()["terminal"],
        "DISCARDED"
    );
}

#[test]
fn abort_records_discarded_tombstones_and_the_33rd_evicts_the_oldest() {
    let destination = Arc::new(FakeDestination::new());
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
