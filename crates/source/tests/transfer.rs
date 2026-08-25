use db_qbs_source::{
    generate_run_id, run_transfer, BatchPayload, BatchResponse, ColumnSupport, CommitResponse,
    FailureKind, OpenOutcome, OpenRunRequest, PrecheckIssue, RangeCheckColumn, RangeCheckResult,
    RowSource, RunResponse, SinkClient, SinkError, SinkErrorKind, SourceColumn, SourceReadError,
    TargetConnection, Terminal, TransferEvent, TransferRequest, BATCH_BYTE_BUDGET,
};

/// 目标端连接（ADR-0037 §1）。搬运骨架不读它，只是原样塞进 `OpenRunRequest`——
/// 这些用例照旧只钉搬运行为。
fn target() -> TargetConnection {
    TargetConnection {
        host: "127.0.0.1".to_owned(),
        port: 3306,
        username: "sink".to_owned(),
        password: "change-me".to_owned(),
        database: "qbs".to_owned(),
    }
}

const RUN_ID: &str = "20260814153000_a3f19c";

#[test]
fn streams_rows_in_order_then_commits_the_fetch_accumulator() {
    let rows = (0..5_001)
        .map(|value| vec![Some(value.to_string())])
        .collect();
    let mut source = FakeSource::new(rows);
    let mut sink = RecordingSink::default();

    let summary = run_transfer(
        &mut source,
        &mut sink,
        TransferRequest {
            run_id: RUN_ID.to_owned(),
            target_table: "ORDERS".to_owned(),
            target: target(),
            primary_key: vec!["ID".to_owned()],
        },
        |_| {},
    )
    .unwrap();

    assert_eq!(sink.calls, vec!["open", "batch:1", "batch:2", "commit"]);
    assert_eq!(sink.batch_rows, vec![5_000, 1]);
    assert_eq!(sink.commit_counts, Some((2, 5_001)));
    assert_eq!(summary.source_rows, 5_001);
    assert_eq!(summary.total_batches, 2);
}

#[test]
fn batch_events_include_the_cumulative_source_row_count() {
    let rows = (0..5_001)
        .map(|value| vec![Some(value.to_string())])
        .collect();
    let mut source = FakeSource::new(rows);
    let mut sink = RecordingSink::default();
    let mut events = Vec::new();

    run_transfer(
        &mut source,
        &mut sink,
        TransferRequest {
            run_id: RUN_ID.to_owned(),
            target_table: "ORDERS".to_owned(),
            target: target(),
            primary_key: vec!["ID".to_owned()],
        },
        |event| events.push(event),
    )
    .unwrap();

    let cumulative_rows: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            TransferEvent::BatchPushed { source_rows, .. } => Some(*source_rows),
            _ => None,
        })
        .collect();
    assert_eq!(cumulative_rows, [5_000, 5_001]);
}

#[test]
fn ora_1555_is_translated_without_losing_the_source_code() {
    let error = SourceReadError::new("ORA-01555: snapshot too old", Some(1555));

    assert_eq!(
        error.user_message(),
        "源端结果集在读取过程中失效，通常是运行时间过长且源表有大量并发写入，请缩小业务日期范围或联系 DBA 调大 undo 保留"
    );
    assert_eq!(error.oracle_code, Some(1555));
}

#[test]
fn open_failure_does_not_abort_before_staging_exists() {
    let mut source = FakeSource::new(vec![]);
    let mut sink = RejectingOpenSink::default();
    let mut events = Vec::new();

    let failure = run_transfer(
        &mut source,
        &mut sink,
        TransferRequest {
            run_id: RUN_ID.to_owned(),
            target_table: "ORDERS".to_owned(),
            target: target(),
            primary_key: vec!["ID".to_owned()],
        },
        |event| events.push(event),
    )
    .unwrap_err();

    assert_eq!(failure.stage, db_qbs_source::RunStage::Preparing);
    assert_eq!(sink.calls, vec!["open"]);
    assert!(events.iter().any(|event| matches!(
        event,
        TransferEvent::MappingPrecheckFailed {
            column,
            rule,
            suggestion,
            ..
        } if column == "AMOUNT"
            && rule == "precision differs"
            && suggestion.as_deref() == Some("改为 DECIMAL(8,0)")
    )));
    assert!(matches!(
        events.last(),
        Some(TransferEvent::StageChanged(db_qbs_source::RunStage::Failed))
    ));
}

#[test]
fn commit_transport_failure_gets_once_and_never_aborts() {
    let mut source = FakeSource::new(vec![]);
    let mut sink = CommitDisconnectSink::default();
    let mut events = Vec::new();

    let failure = run_transfer(
        &mut source,
        &mut sink,
        TransferRequest {
            run_id: RUN_ID.to_owned(),
            target_table: "ORDERS".to_owned(),
            target: target(),
            primary_key: vec!["ID".to_owned()],
        },
        |event| events.push(event),
    )
    .unwrap_err();

    assert_eq!(sink.calls, vec!["open", "commit", "get"]);
    // 传输层断了就是网络那一类，不因为它发生在 commit 上就算成目标端写入失败。
    assert_eq!(failure.kind, FailureKind::Network);
    assert_eq!(
        failure.commit_diagnostic.as_deref(),
        Some("目标端报告该 run 已切换成功（swapped_rows=0），目标表已是新数据，重跑前请先确认")
    );
    assert!(events.iter().any(|event| matches!(
        event,
        TransferEvent::CommitDiagnosed {
            terminal: Some(Terminal::Swapped),
            ..
        }
    )));
}

#[test]
fn empty_result_commits_without_sending_a_batch() {
    let mut source = FakeSource::new(vec![]);
    let mut sink = RecordingSink::default();

    let summary = run_transfer(
        &mut source,
        &mut sink,
        TransferRequest {
            run_id: RUN_ID.to_owned(),
            target_table: "ORDERS".to_owned(),
            target: target(),
            primary_key: vec!["ID".to_owned()],
        },
        |_| {},
    )
    .unwrap();

    assert_eq!(sink.calls, vec!["open", "commit"]);
    assert_eq!(sink.commit_counts, Some((0, 0)));
    assert_eq!(summary.source_rows, 0);
    assert_eq!(summary.total_batches, 0);
}

#[test]
fn range_check_runs_between_two_open_requests_and_emits_scan_event() {
    let mut source = FakeSource::new(vec![]);
    source.range_check = Some((
        vec![RangeCheckResult {
            column: "ID".to_owned(),
            invalid_rows: 0,
        }],
        7,
    ));
    let mut sink = RecordingSink {
        range_check_columns: Some(vec![RangeCheckColumn {
            column: "ID".to_owned(),
            precision: 8,
            scale: 0,
        }]),
        ..RecordingSink::default()
    };
    let mut events = Vec::new();

    run_transfer(
        &mut source,
        &mut sink,
        TransferRequest {
            run_id: RUN_ID.to_owned(),
            target_table: "ORDERS".to_owned(),
            target: target(),
            primary_key: vec!["ID".to_owned()],
        },
        |event| events.push(event),
    )
    .unwrap();

    assert_eq!(sink.calls, vec!["open", "open", "commit"]);
    assert_eq!(sink.range_check_requests.len(), 2);
    assert!(sink.range_check_requests[0].is_none());
    assert_eq!(
        sink.range_check_requests[1],
        Some(vec![RangeCheckResult {
            column: "ID".to_owned(),
            invalid_rows: 0,
        }])
    );
    assert_eq!(
        source.range_check_requests,
        vec![vec![RangeCheckColumn {
            column: "ID".to_owned(),
            precision: 8,
            scale: 0,
        }]]
    );
    assert!(events.iter().any(|event| matches!(
        event,
        TransferEvent::RangeCheckExecuted {
            columns,
            scanned_rows: 7,
            ..
        } if columns == &vec!["ID".to_owned()]
    )));
}

/// 目标端收下校核结果之后再要一次，就是它的状态机坏了。以前这里**静默放行**：
/// 第二答的 `staging_table` 是空串，搬运照常往下走，第一批就撞上一个不存在的 run 的 404。
#[test]
fn a_sink_that_asks_for_a_range_check_twice_is_a_defect_and_no_batch_is_pushed() {
    let mut source = FakeSource::new(vec![vec![Some("1".to_owned())]]);
    source.range_check = Some((
        vec![RangeCheckResult {
            column: "ID".to_owned(),
            invalid_rows: 0,
        }],
        7,
    ));
    let mut sink = AlwaysRangeCheckSink::default();

    let failure = run_transfer(
        &mut source,
        &mut sink,
        TransferRequest {
            run_id: RUN_ID.to_owned(),
            target_table: "ORDERS".to_owned(),
            target: target(),
            primary_key: vec!["ID".to_owned()],
        },
        |_| {},
    )
    .unwrap_err();

    assert_eq!(failure.kind, FailureKind::Defect);
    assert_eq!(failure.stage, db_qbs_source::RunStage::Preparing);
    assert_eq!(sink.calls, vec!["open", "open", "abort"]);
}

#[test]
fn fetch_failure_aborts_and_does_not_commit() {
    let mut source = FailingSource {
        inner: FakeSource::new(vec![vec![Some("1".to_owned())]]),
        failed: false,
    };
    let mut sink = RecordingSink::default();

    let failure = run_transfer(
        &mut source,
        &mut sink,
        TransferRequest {
            run_id: RUN_ID.to_owned(),
            target_table: "ORDERS".to_owned(),
            target: target(),
            primary_key: vec!["ID".to_owned()],
        },
        |_| {},
    )
    .unwrap_err();

    assert_eq!(sink.calls, vec!["open", "abort"]);
    // ORA-03113 不在远端链路码表里，取数阶段撞上它算源端查询失败。
    assert_eq!(failure.kind, FailureKind::SourceQuery);
    assert_eq!(failure.source_rows, 1);
    assert_eq!(failure.total_batches, 0);
}

#[test]
fn byte_budget_splits_wide_rows_before_the_row_limit() {
    let wide_value = "x".repeat(BATCH_BYTE_BUDGET / 2 + 1);
    let mut source = FakeSource::new(vec![vec![Some(wide_value.clone())], vec![Some(wide_value)]]);
    let mut sink = RecordingSink::default();

    let summary = run_transfer(
        &mut source,
        &mut sink,
        TransferRequest {
            run_id: RUN_ID.to_owned(),
            target_table: "ORDERS".to_owned(),
            target: target(),
            primary_key: vec!["ID".to_owned()],
        },
        |_| {},
    )
    .unwrap();

    assert_eq!(sink.batch_rows, vec![1, 1]);
    assert_eq!(summary.total_batches, 2);
}

#[test]
fn commit_diagnostic_distinguishes_discarded_and_unknown() {
    for (diagnostic, expected) in [
        (
            Diagnostic::Discarded,
            "目标端报告暂存表已丢弃，目标表未被触碰，可直接重跑",
        ),
        (Diagnostic::Unknown, "无法确定目标表是否已被切换"),
    ] {
        let mut source = FakeSource::new(vec![]);
        let mut sink = CommitDisconnectSink::new(diagnostic);

        let failure = run_transfer(
            &mut source,
            &mut sink,
            TransferRequest {
                run_id: RUN_ID.to_owned(),
                target_table: "ORDERS".to_owned(),
                target: target(),
                primary_key: vec!["ID".to_owned()],
            },
            |_| {},
        )
        .unwrap_err();

        assert_eq!(sink.calls, vec!["open", "commit", "get"]);
        assert_eq!(failure.commit_diagnostic.as_deref(), Some(expected));
    }
}

#[test]
fn run_id_has_the_single_wire_shape() {
    let run_id = generate_run_id();
    let (timestamp, random) = run_id.split_once('_').unwrap();

    assert_eq!(timestamp.len(), 14);
    assert!(timestamp.bytes().all(|byte| byte.is_ascii_digit()));
    assert_eq!(random.len(), 6);
    assert!(random
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
}

#[test]
fn rows_written_mismatch_aborts_before_commit() {
    let mut source = FakeSource::new(vec![vec![Some("1".to_owned())]]);
    let mut sink = RecordingSink {
        wrong_batch_count: true,
        ..RecordingSink::default()
    };

    let failure = run_transfer(
        &mut source,
        &mut sink,
        TransferRequest {
            run_id: RUN_ID.to_owned(),
            target_table: "ORDERS".to_owned(),
            target: target(),
            primary_key: vec!["ID".to_owned()],
        },
        |_| {},
    )
    .unwrap_err();

    assert_eq!(sink.calls, vec!["open", "batch:1", "abort"]);
    assert_eq!(failure.stage, db_qbs_source::RunStage::Streaming);
    assert_eq!(
        failure.sink_code.as_deref(),
        Some("INTERNAL_ASSERTION_FAILED")
    );
}

#[test]
fn abort_failure_is_reported_before_the_failed_stage() {
    let mut source = FakeSource::new(vec![Vec::new()]);
    let mut sink = RecordingSink {
        abort_error: true,
        ..RecordingSink::default()
    };
    let mut events = Vec::new();

    run_transfer(
        &mut source,
        &mut sink,
        TransferRequest {
            run_id: RUN_ID.to_owned(),
            target_table: "ORDERS".to_owned(),
            target: target(),
            primary_key: vec!["ID".to_owned()],
        },
        |event| events.push(event),
    )
    .unwrap_err();

    assert_eq!(sink.calls, vec!["open", "abort"]);
    assert!(matches!(
        events.as_slice(),
        [
            ..,
            TransferEvent::AbortFailed { message },
            TransferEvent::StageChanged(db_qbs_source::RunStage::Failed),
        ] if message == "abort failed"
    ));
}

struct FakeSource {
    columns: Vec<SourceColumn>,
    rows: std::vec::IntoIter<Vec<Option<String>>>,
    range_check: Option<(Vec<RangeCheckResult>, u64)>,
    range_check_requests: Vec<Vec<RangeCheckColumn>>,
}

impl FakeSource {
    fn new(rows: Vec<Vec<Option<String>>>) -> Self {
        Self {
            columns: vec![SourceColumn {
                name: "ID".to_owned(),
                data_type: "NUMBER".to_owned(),
                precision: Some(8),
                scale: Some(0),
                length: None,
                fsp: None,
                support: Some(ColumnSupport::Ok),
            }],
            rows: rows.into_iter(),
            range_check: None,
            range_check_requests: Vec::new(),
        }
    }
}

impl RowSource for FakeSource {
    fn columns(&self) -> &[SourceColumn] {
        &self.columns
    }

    fn next_row(&mut self) -> Result<Option<Vec<Option<String>>>, SourceReadError> {
        Ok(self.rows.next())
    }

    fn range_check(
        &mut self,
        columns: &[RangeCheckColumn],
    ) -> Result<(Vec<RangeCheckResult>, u64), SourceReadError> {
        self.range_check_requests.push(columns.to_vec());
        self.range_check.clone().ok_or_else(|| {
            SourceReadError::with_kind("range check was not configured", None, FailureKind::Defect)
        })
    }
}

struct FailingSource {
    inner: FakeSource,
    failed: bool,
}

impl RowSource for FailingSource {
    fn columns(&self) -> &[SourceColumn] {
        self.inner.columns()
    }

    fn next_row(&mut self) -> Result<Option<Vec<Option<String>>>, SourceReadError> {
        if let Some(row) = self.inner.rows.next() {
            return Ok(Some(row));
        }
        if !self.failed {
            self.failed = true;
            return Err(SourceReadError::new("fetch failed", Some(3113)));
        }
        Ok(None)
    }
}

fn opened_run() -> OpenOutcome {
    OpenOutcome::Opened {
        run_id: RUN_ID.to_owned(),
        staging_table: format!("ORDERS__stg_{RUN_ID}"),
        columns_checked: 1,
    }
}

#[derive(Default)]
struct RecordingSink {
    calls: Vec<&'static str>,
    batch_rows: Vec<usize>,
    commit_counts: Option<(u64, u64)>,
    range_check_columns: Option<Vec<RangeCheckColumn>>,
    range_check_requests: Vec<Option<Vec<RangeCheckResult>>>,
    wrong_batch_count: bool,
    abort_error: bool,
}

impl SinkClient for RecordingSink {
    fn open_attempt(&mut self, request: &OpenRunRequest) -> Result<OpenOutcome, SinkError> {
        self.calls.push("open");
        self.range_check_requests
            .push(request.range_check_results.clone());
        if request.range_check_results.is_none() {
            if let Some(range_check_columns) = &self.range_check_columns {
                return Ok(OpenOutcome::RangeCheckNeeded {
                    run_id: RUN_ID.to_owned(),
                    columns_checked: 1,
                    columns: range_check_columns.clone(),
                });
            }
        }
        Ok(opened_run())
    }

    fn push_batch(
        &mut self,
        _run_id: &str,
        payload: &BatchPayload,
    ) -> Result<BatchResponse, SinkError> {
        self.calls.push(if payload.seq == 1 {
            "batch:1"
        } else {
            "batch:2"
        });
        self.batch_rows.push(payload.rows.len());
        let rows_written = if self.wrong_batch_count {
            payload.rows.len().saturating_sub(1) as u64
        } else {
            payload.rows.len() as u64
        };
        Ok(BatchResponse {
            seq: payload.seq,
            rows_written,
            next_seq: payload.seq + 1,
        })
    }

    fn commit(
        &mut self,
        _run_id: &str,
        total_batches: u64,
        total_rows: u64,
    ) -> Result<CommitResponse, SinkError> {
        self.calls.push("commit");
        self.commit_counts = Some((total_batches, total_rows));
        Ok(CommitResponse {
            source_rows: total_rows,
            staged_rows: total_rows,
            purged_rows: 0,
            swapped_rows: total_rows,
            count_ms: 4,
        })
    }

    fn get(&mut self, _run_id: &str) -> Result<RunResponse, SinkError> {
        unreachable!()
    }

    fn abort(&mut self, _run_id: &str) -> Result<bool, SinkError> {
        self.calls.push("abort");
        if self.abort_error {
            Err(SinkError::response(None, "abort failed"))
        } else {
            Ok(true)
        }
    }
}

/// 一个坏掉的目标端：拿到校核结果之后照旧要求再核一遍。
#[derive(Default)]
struct AlwaysRangeCheckSink {
    calls: Vec<&'static str>,
}

impl SinkClient for AlwaysRangeCheckSink {
    fn open_attempt(&mut self, _request: &OpenRunRequest) -> Result<OpenOutcome, SinkError> {
        self.calls.push("open");
        Ok(OpenOutcome::RangeCheckNeeded {
            run_id: RUN_ID.to_owned(),
            columns_checked: 1,
            columns: vec![RangeCheckColumn {
                column: "ID".to_owned(),
                precision: 8,
                scale: 0,
            }],
        })
    }

    fn push_batch(
        &mut self,
        _run_id: &str,
        _payload: &BatchPayload,
    ) -> Result<BatchResponse, SinkError> {
        unreachable!("the run never opened, so no batch may be pushed")
    }

    fn commit(
        &mut self,
        _run_id: &str,
        _total_batches: u64,
        _total_rows: u64,
    ) -> Result<CommitResponse, SinkError> {
        unreachable!("the run never opened, so it cannot commit")
    }

    fn get(&mut self, _run_id: &str) -> Result<RunResponse, SinkError> {
        unreachable!()
    }

    fn abort(&mut self, _run_id: &str) -> Result<bool, SinkError> {
        self.calls.push("abort");
        Ok(true)
    }
}

#[derive(Default)]
struct RejectingOpenSink {
    calls: Vec<&'static str>,
}

impl SinkClient for RejectingOpenSink {
    fn open_attempt(&mut self, _request: &OpenRunRequest) -> Result<OpenOutcome, SinkError> {
        self.calls.push("open");
        let mut error = SinkError::response(Some("PRECHECK_FAILED".to_owned()), "mapping rejected");
        error.precheck_issues = Box::new(vec![PrecheckIssue {
            column: "AMOUNT".to_owned(),
            source: "NUMBER(8,0)".to_owned(),
            target: "decimal(7,0)".to_owned(),
            rule: "precision differs".to_owned(),
            suggestion: Some("改为 DECIMAL(8,0)".to_owned()),
        }]);
        Err(error)
    }

    fn push_batch(
        &mut self,
        _run_id: &str,
        _payload: &BatchPayload,
    ) -> Result<BatchResponse, SinkError> {
        unreachable!()
    }

    fn commit(
        &mut self,
        _run_id: &str,
        _total_batches: u64,
        _total_rows: u64,
    ) -> Result<CommitResponse, SinkError> {
        unreachable!()
    }

    fn get(&mut self, _run_id: &str) -> Result<RunResponse, SinkError> {
        unreachable!()
    }

    fn abort(&mut self, _run_id: &str) -> Result<bool, SinkError> {
        self.calls.push("abort");
        Ok(false)
    }
}

struct CommitDisconnectSink {
    calls: Vec<&'static str>,
    diagnostic: Diagnostic,
}

#[derive(Clone, Copy)]
enum Diagnostic {
    Swapped,
    Discarded,
    Unknown,
}

impl Default for CommitDisconnectSink {
    fn default() -> Self {
        Self::new(Diagnostic::Swapped)
    }
}

impl CommitDisconnectSink {
    fn new(diagnostic: Diagnostic) -> Self {
        Self {
            calls: Vec::new(),
            diagnostic,
        }
    }
}

impl SinkClient for CommitDisconnectSink {
    fn open_attempt(&mut self, _request: &OpenRunRequest) -> Result<OpenOutcome, SinkError> {
        self.calls.push("open");
        Ok(opened_run())
    }

    fn push_batch(
        &mut self,
        _run_id: &str,
        _payload: &BatchPayload,
    ) -> Result<BatchResponse, SinkError> {
        unreachable!()
    }

    fn commit(
        &mut self,
        _run_id: &str,
        _total_batches: u64,
        _total_rows: u64,
    ) -> Result<CommitResponse, SinkError> {
        self.calls.push("commit");
        Err(SinkError {
            kind: SinkErrorKind::Transport,
            code: None,
            message: "connection closed".to_owned(),
            column: None,
            value: None,
            precheck_issues: Box::new(Vec::new()),
            gate: None,
        })
    }

    fn get(&mut self, _run_id: &str) -> Result<RunResponse, SinkError> {
        self.calls.push("get");
        if matches!(self.diagnostic, Diagnostic::Unknown) {
            return Err(SinkError::response(
                Some("RUN_UNKNOWN".to_owned()),
                "unknown run",
            ));
        }
        Ok(RunResponse {
            run_id: RUN_ID.to_owned(),
            staging_table: format!("ORDERS__stg_{RUN_ID}"),
            batches_received: 0,
            rows_written: 0,
            sealed: true,
            terminal: Some(match self.diagnostic {
                Diagnostic::Swapped => Terminal::Swapped,
                Diagnostic::Discarded => Terminal::Discarded,
                Diagnostic::Unknown => unreachable!(),
            }),
            purged_rows: Some(4),
            swapped_rows: Some(0),
        })
    }

    fn abort(&mut self, _run_id: &str) -> Result<bool, SinkError> {
        self.calls.push("abort");
        Ok(false)
    }
}
