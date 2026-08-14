use std::fmt;
use std::time::{Duration, Instant};

use chrono::Utc;
use rand::RngCore;

use crate::{
    BatchPayload, OpenRunRequest, SinkClient, SinkError, SinkErrorKind, SourceColumn, Terminal,
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
}

impl SourceReadError {
    pub fn new(message: impl Into<String>, oracle_code: Option<i32>) -> Self {
        Self {
            message: message.into(),
            oracle_code,
            column: None,
            value: None,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferRequest {
    pub run_id: String,
    pub target_table: String,
    pub target_date_col: String,
    pub biz_date: String,
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
        bytes: usize,
        written: u64,
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
    pub message: String,
    pub source_code: Option<i32>,
    pub sink_code: Option<String>,
    pub column: Option<String>,
    pub value: Option<String>,
    pub commit_diagnostic: Option<String>,
    pub source_rows: u64,
    pub total_batches: u64,
}

impl fmt::Display for TransferFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TransferFailure {}

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

    let open_request = OpenRunRequest {
        run_id: request.run_id.clone(),
        target_table: request.target_table,
        target_date_col: request.target_date_col,
        biz_date: request.biz_date,
        source_columns: source.columns().to_vec(),
    };
    let opened = match sink.open(&open_request) {
        Ok(opened) => opened,
        Err(error) => {
            observe(TransferEvent::StageChanged(RunStage::Failed));
            return Err(Box::new(sink_failure(
                error,
                RunStage::Preparing,
                0,
                0,
                None,
            )));
        }
    };
    if opened.run_id != request.run_id {
        abort_best_effort(sink, &request.run_id, &mut observe);
        observe(TransferEvent::StageChanged(RunStage::Failed));
        return Err(Box::new(failure(
            RunStage::Preparing,
            "目标端开任务响应的 run_id 与请求不一致".to_owned(),
            0,
            0,
        )));
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
                abort_best_effort(sink, &request.run_id, &mut observe);
                observe(TransferEvent::StageChanged(RunStage::Failed));
                return Err(Box::new(TransferFailure {
                    stage: RunStage::Streaming,
                    message: error.user_message(),
                    source_code: error.oracle_code,
                    sink_code: None,
                    column: error.column,
                    value: error.value,
                    commit_diagnostic: None,
                    source_rows,
                    total_batches,
                }));
            }
        };

        if row.len() != expected_columns {
            abort_best_effort(sink, &request.run_id, &mut observe);
            observe(TransferEvent::StageChanged(RunStage::Failed));
            return Err(Box::new(failure(
                RunStage::Streaming,
                format!(
                    "源端行宽断言失败：describe 有 {expected_columns} 列，当前行有 {} 个值",
                    row.len()
                ),
                source_rows,
                total_batches,
            )));
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
                &mut push_time,
                &mut observe,
            )
            .map_err(|error| {
                abort_best_effort(sink, &request.run_id, &mut observe);
                observe(TransferEvent::StageChanged(RunStage::Failed));
                Box::new(sink_failure(
                    error,
                    RunStage::Streaming,
                    source_rows,
                    total_batches,
                    None,
                ))
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
            &mut push_time,
            &mut observe,
        )
        .map_err(|error| {
            abort_best_effort(sink, &request.run_id, &mut observe);
            observe(TransferEvent::StageChanged(RunStage::Failed));
            Box::new(sink_failure(
                error,
                RunStage::Streaming,
                source_rows,
                total_batches,
                None,
            ))
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
            return Err(Box::new(sink_failure(
                error,
                RunStage::Committing,
                source_rows,
                total_batches,
                diagnostic,
            )));
        }
    };

    if committed.source_rows != source_rows
        || committed.staged_rows != source_rows
        || committed.swapped_rows != committed.staged_rows
    {
        observe(TransferEvent::StageChanged(RunStage::Failed));
        return Err(Box::new(failure(
            RunStage::Committing,
            format!(
                "目标端 commit 响应行数断言失败：source_rows={} staged_rows={} swapped_rows={}",
                committed.source_rows, committed.staged_rows, committed.swapped_rows
            ),
            source_rows,
            total_batches,
        )));
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
        cursor_ms: elapsed_ms(cursor_started.elapsed()),
    })
}

fn push_rows(
    sink: &mut impl SinkClient,
    run_id: &str,
    rows: &mut Vec<Vec<Option<String>>>,
    total_batches: &mut u64,
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
    let response = sink.push_batch(run_id, &payload)?;
    let elapsed = started.elapsed();
    *push_time += elapsed;

    if response.seq != seq
        || response.next_seq != seq + 1
        || response.rows_written != row_count as u64
    {
        return Err(SinkError::response(
            Some("INTERNAL_PRECHECK_ESCAPE".to_owned()),
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
        Ok(response)
            if response.run_id == run_id
                && response.terminal == Some(Terminal::Swapped)
                && response.swapped_rows.is_some() =>
        {
            (
                Some(Terminal::Swapped),
                format!(
                    "目标端报告该 run 已切换成功（swapped_rows={}），目标表已是新数据，重跑前请先确认",
                    response.swapped_rows.expect("guarded above")
                ),
            )
        }
        Ok(response)
            if response.run_id == run_id && response.terminal == Some(Terminal::Discarded) =>
        {
            (
                Some(Terminal::Discarded),
                "目标端报告暂存表已丢弃，目标表未被触碰，可直接重跑".to_owned(),
            )
        }
        _ => (None, "无法确定目标表是否已被切换".to_owned()),
    };
    observe(TransferEvent::CommitDiagnosed {
        terminal,
        message: message.clone(),
    });
    message
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
    TransferFailure {
        stage,
        message: format!("目标端：{}", error.message),
        source_code: None,
        sink_code: error.code,
        column: error.column,
        value: error.value,
        commit_diagnostic,
        source_rows,
        total_batches,
    }
}

fn failure(
    stage: RunStage,
    message: String,
    source_rows: u64,
    total_batches: u64,
) -> TransferFailure {
    TransferFailure {
        stage,
        message,
        source_code: None,
        sink_code: None,
        column: None,
        value: None,
        commit_diagnostic: None,
        source_rows,
        total_batches,
    }
}

fn elapsed_ms(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}
