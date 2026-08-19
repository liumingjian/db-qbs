use std::fmt;
use std::time::{Duration, Instant};

use chrono::Utc;
use rand::RngCore;

use crate::{
    BatchPayload, FailureKind, OpenRunRequest, RangeCheckColumn, RangeCheckResult, RunResponse,
    SinkClient, SinkError, SinkErrorKind, SourceColumn, Terminal,
};

pub const FETCH_ARRAY_SIZE: u32 = 100;
pub const BATCH_ROW_LIMIT: usize = 5_000;
pub const BATCH_BYTE_BUDGET: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStage {
    Preparing,
    Streaming,
    Committing,
    Succeeded,
    Failed,
}

impl RunStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preparing => "PREPARING",
            Self::Streaming => "STREAMING",
            Self::Committing => "COMMITTING",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceReadError {
    pub message: String,
    pub oracle_code: Option<i32>,
    pub column: Option<String>,
    pub value: Option<String>,
    /// 成因分类。**在出错的那一步定下**，不靠事后从人话或错误码反推——
    /// 同一个 Oracle 码在建连接与取数两步上指的不是同一件事（ADR-0029 §2）。
    pub kind: FailureKind,
}

impl SourceReadError {
    /// 会话已建立之后的读取失败。dblink 与本地查询由 [`crate::oracle_kind`] 按码区分。
    pub fn new(message: impl Into<String>, oracle_code: Option<i32>) -> Self {
        Self::with_kind(message, oracle_code, crate::oracle_kind(oracle_code, false))
    }

    /// 建连接那一步的失败：Instant Client 初始化、监听器、登录。
    pub fn connecting(message: impl Into<String>, oracle_code: Option<i32>) -> Self {
        Self::with_kind(message, oracle_code, crate::oracle_kind(oracle_code, true))
    }

    pub fn with_kind(
        message: impl Into<String>,
        oracle_code: Option<i32>,
        kind: FailureKind,
    ) -> Self {
        Self {
            message: message.into(),
            oracle_code,
            column: None,
            value: None,
            kind,
        }
    }

    pub fn user_message(&self) -> String {
        if self.oracle_code == Some(1555) {
            "源端结果集在读取过程中失效，通常是运行时间过长且源表有大量并发写入，请缩小业务日期范围或联系 DBA 调大 undo 保留"
                .to_owned()
        } else {
            format!("源端：{}", self.message)
        }
    }
}

pub trait RowSource {
    fn columns(&self) -> &[SourceColumn];
    fn next_row(&mut self) -> Result<Option<Vec<Option<String>>>, SourceReadError>;

    fn range_check(
        &mut self,
        _columns: &[RangeCheckColumn],
    ) -> Result<(Vec<RangeCheckResult>, u64), SourceReadError> {
        Err(SourceReadError::with_kind(
            "source row source cannot execute a range check",
            None,
            FailureKind::Defect,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferRequest {
    pub run_id: String,
    pub target_table: String,
    /// upsert 的去重键（ADR-0035 §2），原样过线交给 sink 侧预检核对。
    pub primary_key: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferSummary {
    pub source_rows: u64,
    pub total_batches: u64,
    pub staged_rows: u64,
    pub purged_rows: u64,
    pub swapped_rows: u64,
    pub fetch_ms: u64,
    pub push_ms: u64,
    pub commit_ms: u64,
    pub count_ms: u64,
    pub cursor_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferEvent {
    StageChanged(RunStage),
    RunOpened {
        staging_table: String,
        columns_checked: usize,
    },
    BatchPushed {
        seq: u64,
        rows: usize,
        source_rows: u64,
        bytes: usize,
        written: u64,
        ms: u64,
    },
    MappingPrecheckFailed {
        column: String,
        source: String,
        target: String,
        rule: String,
        suggestion: Option<String>,
    },
    RangeCheckExecuted {
        columns: Vec<String>,
        scanned_rows: u64,
        ms: u64,
    },
    CommitDiagnosed {
        terminal: Option<Terminal>,
        message: String,
    },
    AbortFailed {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferFailure {
    pub stage: RunStage,
    /// 成因分类，见 [`FailureKind`]。每个构造点都必须显式给出——没有「默认档」。
    pub kind: FailureKind,
    pub message: String,
    pub source_code: Option<i32>,
    pub sink_code: Option<String>,
    pub column: Option<String>,
    pub value: Option<String>,
    pub commit_diagnostic: Option<String>,
    pub source_rows: u64,
    pub total_batches: u64,
    pub staged_rows: Option<u64>,
    pub received_batches: Option<u64>,
    pub sink_reported_rows: Option<u64>,
    pub purged_rows: Option<u64>,
    pub fetch_ms: u64,
    pub push_ms: u64,
    pub commit_ms: u64,
    pub count_ms: Option<u64>,
    pub cursor_ms: u64,
}

impl fmt::Display for TransferFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TransferFailure {}

impl TransferFailure {
    pub fn new(
        stage: RunStage,
        kind: FailureKind,
        message: impl Into<String>,
        source_rows: u64,
        total_batches: u64,
    ) -> Self {
        Self {
            stage,
            kind,
            message: message.into(),
            source_code: None,
            sink_code: None,
            column: None,
            value: None,
            commit_diagnostic: None,
            source_rows,
            total_batches,
            staged_rows: None,
            received_batches: None,
            sink_reported_rows: None,
            purged_rows: None,
            fetch_ms: 0,
            push_ms: 0,
            commit_ms: 0,
            count_ms: None,
            cursor_ms: 0,
        }
    }

    pub fn from_source_error(
        stage: RunStage,
        error: SourceReadError,
        source_rows: u64,
        total_batches: u64,
    ) -> Self {
        let mut failure = Self::new(
            stage,
            error.kind,
            error.user_message(),
            source_rows,
            total_batches,
        );
        failure.source_code = error.oracle_code;
        failure.column = error.column;
        failure.value = error.value;
        failure
    }

    fn with_timings(
        mut self,
        fetch: Duration,
        push: Duration,
        commit: Duration,
        cursor: Duration,
    ) -> Self {
        self.fetch_ms = elapsed_ms(fetch);
        self.push_ms = elapsed_ms(push);
        self.commit_ms = elapsed_ms(commit);
        self.cursor_ms = elapsed_ms(cursor);
        self
    }
}

pub fn generate_run_id() -> String {
    let mut random = [0u8; 3];
    rand::thread_rng().fill_bytes(&mut random);
    format!(
        "{}_{:02x}{:02x}{:02x}",
        Utc::now().format("%Y%m%d%H%M%S"),
        random[0],
        random[1],
        random[2]
    )
}

pub fn run_transfer(
    source: &mut impl RowSource,
    sink: &mut impl SinkClient,
    request: TransferRequest,
    mut observe: impl FnMut(TransferEvent),
) -> Result<TransferSummary, Box<TransferFailure>> {
    let cursor_started = Instant::now();
    observe(TransferEvent::StageChanged(RunStage::Preparing));

    let mut open_request = OpenRunRequest {
        run_id: request.run_id.clone(),
        target_table: request.target_table,
        primary_key: request.primary_key,
        source_columns: source.columns().to_vec(),
        range_check_results: None,
    };
    let opened = match sink.open(&open_request) {
        Ok(opened) => opened,
        Err(error) => {
            for issue in error.precheck_issues.iter() {
                observe(TransferEvent::MappingPrecheckFailed {
                    column: issue.column.clone(),
                    source: issue.source.clone(),
                    target: issue.target.clone(),
                    rule: issue.rule.clone(),
                    suggestion: issue.suggestion.clone(),
                });
            }
            observe(TransferEvent::StageChanged(RunStage::Failed));
            return Err(Box::new(
                sink_failure(error, RunStage::Preparing, 0, 0, None).with_timings(
                    Duration::ZERO,
                    Duration::ZERO,
                    Duration::ZERO,
                    cursor_started.elapsed(),
                ),
            ));
        }
    };
    if opened.run_id != request.run_id {
        return Err(fail_before_commit(
            sink,
            &request.run_id,
            &mut observe,
            TransferFailure::new(
                RunStage::Preparing,
                FailureKind::Defect,
                "目标端开任务响应的 run_id 与请求不一致",
                0,
                0,
            )
            .with_timings(
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO,
                cursor_started.elapsed(),
            ),
        ));
    }
    let opened = if let Some(range_columns) = opened.range_check_columns.clone() {
        if range_columns.is_empty() {
            opened
        } else {
            let range_started = Instant::now();
            let (range_check_results, scanned_rows) = match source.range_check(&range_columns) {
                Ok(result) => result,
                Err(error) => {
                    observe(TransferEvent::StageChanged(RunStage::Failed));
                    return Err(Box::new(
                        TransferFailure::from_source_error(RunStage::Preparing, error, 0, 0)
                            .with_timings(
                                Duration::ZERO,
                                Duration::ZERO,
                                Duration::ZERO,
                                cursor_started.elapsed(),
                            ),
                    ));
                }
            };
            observe(TransferEvent::RangeCheckExecuted {
                columns: range_columns
                    .iter()
                    .map(|column| column.column.clone())
                    .collect(),
                scanned_rows,
                ms: elapsed_ms(range_started.elapsed()),
            });
            open_request.range_check_results = Some(range_check_results);
            match sink.open(&open_request) {
                Ok(opened) => opened,
                Err(error) => {
                    for issue in error.precheck_issues.iter() {
                        observe(TransferEvent::MappingPrecheckFailed {
                            column: issue.column.clone(),
                            source: issue.source.clone(),
                            target: issue.target.clone(),
                            rule: issue.rule.clone(),
                            suggestion: issue.suggestion.clone(),
                        });
                    }
                    observe(TransferEvent::StageChanged(RunStage::Failed));
                    return Err(Box::new(
                        sink_failure(error, RunStage::Preparing, 0, 0, None).with_timings(
                            Duration::ZERO,
                            Duration::ZERO,
                            Duration::ZERO,
                            cursor_started.elapsed(),
                        ),
                    ));
                }
            }
        }
    } else {
        opened
    };
    if opened.run_id != request.run_id {
        return Err(fail_before_commit(
            sink,
            &request.run_id,
            &mut observe,
            TransferFailure::new(
                RunStage::Preparing,
                FailureKind::Defect,
                "目标端第二次开任务响应的 run_id 与请求不一致",
                0,
                0,
            )
            .with_timings(
                Duration::ZERO,
                Duration::ZERO,
                Duration::ZERO,
                cursor_started.elapsed(),
            ),
        ));
    }
    observe(TransferEvent::RunOpened {
        staging_table: opened.staging_table,
        columns_checked: opened.columns_checked,
    });
    observe(TransferEvent::StageChanged(RunStage::Streaming));

    let expected_columns = source.columns().len();
    let mut rows = Vec::with_capacity(BATCH_ROW_LIMIT);
    let mut serialized_row_bytes = 0usize;
    let mut source_rows = 0u64;
    let mut total_batches = 0u64;
    let mut fetch_time = Duration::ZERO;
    let mut push_time = Duration::ZERO;

    loop {
        let fetch_started = Instant::now();
        let next = source.next_row();
        fetch_time += fetch_started.elapsed();
        let row = match next {
            Ok(Some(row)) => row,
            Ok(None) => break,
            Err(error) => {
                return Err(fail_before_commit(
                    sink,
                    &request.run_id,
                    &mut observe,
                    TransferFailure::from_source_error(
                        RunStage::Streaming,
                        error,
                        source_rows,
                        total_batches,
                    )
                    .with_timings(
                        fetch_time,
                        push_time,
                        Duration::ZERO,
                        cursor_started.elapsed(),
                    ),
                ));
            }
        };

        if row.len() != expected_columns {
            return Err(fail_before_commit(
                sink,
                &request.run_id,
                &mut observe,
                TransferFailure::new(
                    RunStage::Streaming,
                    FailureKind::Defect,
                    format!(
                        "源端行宽断言失败：describe 有 {expected_columns} 列，当前行有 {} 个值",
                        row.len()
                    ),
                    source_rows,
                    total_batches,
                )
                .with_timings(
                    fetch_time,
                    push_time,
                    Duration::ZERO,
                    cursor_started.elapsed(),
                ),
            ));
        }

        let row_bytes = serde_json::to_vec(&row)
            .expect("serializing Option<String> rows cannot fail")
            .len();
        let seq = total_batches + 1;
        let candidate_bytes =
            empty_payload_bytes(seq) + serialized_row_bytes + row_bytes + rows.len();
        if !rows.is_empty()
            && (rows.len() == BATCH_ROW_LIMIT || candidate_bytes > BATCH_BYTE_BUDGET)
        {
            push_rows(
                sink,
                &request.run_id,
                &mut rows,
                &mut total_batches,
                source_rows,
                &mut push_time,
                &mut observe,
            )
            .map_err(|error| {
                fail_before_commit(
                    sink,
                    &request.run_id,
                    &mut observe,
                    sink_failure(error, RunStage::Streaming, source_rows, total_batches, None)
                        .with_timings(
                            fetch_time,
                            push_time,
                            Duration::ZERO,
                            cursor_started.elapsed(),
                        ),
                )
            })?;
            serialized_row_bytes = 0;
        }

        serialized_row_bytes += row_bytes;
        rows.push(row);
        source_rows += 1;
    }

    if !rows.is_empty() {
        push_rows(
            sink,
            &request.run_id,
            &mut rows,
            &mut total_batches,
            source_rows,
            &mut push_time,
            &mut observe,
        )
        .map_err(|error| {
            fail_before_commit(
                sink,
                &request.run_id,
                &mut observe,
                sink_failure(error, RunStage::Streaming, source_rows, total_batches, None)
                    .with_timings(
                        fetch_time,
                        push_time,
                        Duration::ZERO,
                        cursor_started.elapsed(),
                    ),
            )
        })?;
    }

    observe(TransferEvent::StageChanged(RunStage::Committing));
    let commit_started = Instant::now();
    let committed = sink.commit(&request.run_id, total_batches, source_rows);
    let commit_time = commit_started.elapsed();
    let committed = match committed {
        Ok(response) => response,
        Err(error) => {
            let diagnostic = if error.kind == SinkErrorKind::Transport {
                Some(diagnose_commit(sink, &request.run_id, &mut observe))
            } else {
                None
            };
            observe(TransferEvent::StageChanged(RunStage::Failed));
            return Err(Box::new(
                sink_failure(
                    error,
                    RunStage::Committing,
                    source_rows,
                    total_batches,
                    diagnostic,
                )
                .with_timings(
                    fetch_time,
                    push_time,
                    commit_time,
                    cursor_started.elapsed(),
                ),
            ));
        }
    };

    if committed.source_rows != source_rows
        || committed.staged_rows != source_rows
        || committed.swapped_rows != committed.staged_rows
    {
        observe(TransferEvent::StageChanged(RunStage::Failed));
        return Err(Box::new(
            TransferFailure::new(
                RunStage::Committing,
                FailureKind::VerifyFailed,
                format!(
                    "目标端 commit 响应行数断言失败：source_rows={} staged_rows={} swapped_rows={}",
                    committed.source_rows, committed.staged_rows, committed.swapped_rows
                ),
                source_rows,
                total_batches,
            )
            .with_timings(fetch_time, push_time, commit_time, cursor_started.elapsed()),
        ));
    }

    observe(TransferEvent::StageChanged(RunStage::Succeeded));
    Ok(TransferSummary {
        source_rows,
        total_batches,
        staged_rows: committed.staged_rows,
        purged_rows: committed.purged_rows,
        swapped_rows: committed.swapped_rows,
        fetch_ms: elapsed_ms(fetch_time),
        push_ms: elapsed_ms(push_time),
        commit_ms: elapsed_ms(commit_time),
        count_ms: committed.count_ms,
        cursor_ms: elapsed_ms(cursor_started.elapsed()),
    })
}

fn push_rows(
    sink: &mut impl SinkClient,
    run_id: &str,
    rows: &mut Vec<Vec<Option<String>>>,
    total_batches: &mut u64,
    source_rows: u64,
    push_time: &mut Duration,
    observe: &mut impl FnMut(TransferEvent),
) -> Result<(), SinkError> {
    let seq = *total_batches + 1;
    let payload = BatchPayload {
        seq,
        rows: std::mem::take(rows),
    };
    let bytes = serde_json::to_vec(&payload)
        .expect("serializing Option<String> batch payload cannot fail")
        .len();
    let row_count = payload.rows.len();
    let started = Instant::now();
    let response = sink.push_batch(run_id, &payload);
    let elapsed = started.elapsed();
    *push_time += elapsed;
    let response = response?;

    if response.seq != seq
        || response.next_seq != seq + 1
        || response.rows_written != row_count as u64
    {
        return Err(SinkError::response(
            Some("INTERNAL_ASSERTION_FAILED".to_owned()),
            format!(
                "目标端第 {seq} 批响应断言失败：发送 {row_count} 行，响应 seq={}、rows_written={}、next_seq={}",
                response.seq, response.rows_written, response.next_seq
            ),
        ));
    }

    *total_batches = seq;
    observe(TransferEvent::BatchPushed {
        seq,
        rows: row_count,
        source_rows,
        bytes,
        written: response.rows_written,
        ms: elapsed_ms(elapsed),
    });
    Ok(())
}

fn empty_payload_bytes(seq: u64) -> usize {
    serde_json::to_vec(&BatchPayload {
        seq,
        rows: Vec::new(),
    })
    .expect("serializing an empty batch payload cannot fail")
    .len()
}

fn diagnose_commit(
    sink: &mut impl SinkClient,
    run_id: &str,
    observe: &mut impl FnMut(TransferEvent),
) -> String {
    let (terminal, message) = match sink.get(run_id) {
        Ok(RunResponse {
            run_id: response_run_id,
            terminal: Some(Terminal::Swapped),
            swapped_rows: Some(swapped_rows),
            ..
        }) if response_run_id == run_id => (
            Some(Terminal::Swapped),
            format!(
                "目标端报告该 run 已切换成功（swapped_rows={}），目标表已是新数据，重跑前请先确认",
                swapped_rows
            ),
        ),
        Ok(RunResponse {
            run_id: response_run_id,
            terminal: Some(Terminal::Discarded),
            ..
        }) if response_run_id == run_id => (
            Some(Terminal::Discarded),
            "目标端报告暂存表已丢弃，目标表未被触碰，可直接重跑".to_owned(),
        ),
        _ => (None, "无法确定目标表是否已被切换".to_owned()),
    };
    observe(TransferEvent::CommitDiagnosed {
        terminal,
        message: message.clone(),
    });
    message
}

fn fail_before_commit(
    sink: &mut impl SinkClient,
    run_id: &str,
    observe: &mut impl FnMut(TransferEvent),
    failure: TransferFailure,
) -> Box<TransferFailure> {
    abort_best_effort(sink, run_id, observe);
    observe(TransferEvent::StageChanged(RunStage::Failed));
    Box::new(failure)
}

fn abort_best_effort(
    sink: &mut impl SinkClient,
    run_id: &str,
    observe: &mut impl FnMut(TransferEvent),
) {
    if let Err(error) = sink.abort(run_id) {
        observe(TransferEvent::AbortFailed {
            message: error.message,
        });
    }
}

fn sink_failure(
    error: SinkError,
    stage: RunStage,
    source_rows: u64,
    total_batches: u64,
    commit_diagnostic: Option<String>,
) -> TransferFailure {
    let SinkError {
        kind,
        code,
        message,
        column,
        value,
        gate,
        ..
    } = error;
    // 传输层断了就是网络，谈不上目标端报了什么；只有拿到响应才有码可分类。
    let failure_kind = match kind {
        SinkErrorKind::Transport => FailureKind::Network,
        SinkErrorKind::Response => code
            .as_deref()
            .map_or(FailureKind::Defect, FailureKind::from_sink_code),
    };
    let mut failure = TransferFailure::new(
        stage,
        failure_kind,
        format!("目标端：{message}"),
        source_rows,
        total_batches,
    );
    failure.sink_code = code;
    failure.column = column;
    failure.value = value;
    failure.commit_diagnostic = commit_diagnostic;
    if let Some(gate) = gate {
        failure.staged_rows = Some(gate.staged_rows);
        failure.received_batches = Some(gate.received_batches);
        failure.sink_reported_rows = Some(gate.sink_reported_rows);
        failure.count_ms = Some(gate.count_ms);
    }
    failure
}

fn elapsed_ms(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}
