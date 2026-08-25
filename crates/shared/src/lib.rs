use std::fmt;
use std::io::{self, Write};
use std::sync::LazyLock;

use chrono::{NaiveDate, SecondsFormat, Utc};
use regex::Regex;
use serde::{Serialize, Serializer};

mod protocol;
mod target_shape;

pub use protocol::{
    AbortResponse, AgentInfo, BatchPayload, BatchResponse, ColumnSupport, CommitRequest,
    CommitResponse, ErrorBody, ErrorEnvelope, OpenRunRequest, OpenRunResponse, PrecheckIssue,
    RangeCheckColumn, RangeCheckResult, RunResponse, SourceColumn, TargetConnection, Terminal,
};
pub use target_shape::{
    classify_column, column_support, derive_number_shape, is_business_date_column,
    is_supported_decimal_shape, ColumnShape, ShapeRejection, TargetShape,
};

static CANONICAL_NUMBER_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(0|-?[1-9][0-9]*(\.[0-9]*[1-9])?|-?0\.[0-9]*[1-9])$")
        .expect("canonical NUMBER grammar must compile")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonError {
    NonCanonicalNumber,
    InvalidDate,
    InvalidTimestamp,
}

impl fmt::Display for CanonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonCanonicalNumber => {
                formatter.write_str("driver emitted a non-canonical NUMBER")
            }
            Self::InvalidDate => formatter.write_str(
                "invalid DATE components: year must be within 0001..9999 \
                 (Oracle BC dates are not supported)",
            ),
            Self::InvalidTimestamp => formatter.write_str("invalid TIMESTAMP components"),
        }
    }
}

impl std::error::Error for CanonError {}

pub fn canon_number(value: &str) -> Result<&str, CanonError> {
    CANONICAL_NUMBER_REGEX
        .is_match(value)
        .then_some(value)
        .ok_or(CanonError::NonCanonicalNumber)
}

pub fn canon_date(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> Result<String, CanonError> {
    if !(1..=9999).contains(&year) {
        return Err(CanonError::InvalidDate);
    }

    let date = NaiveDate::from_ymd_opt(year, month, day).ok_or(CanonError::InvalidDate)?;
    let date_time = date
        .and_hms_opt(hour, minute, second)
        .ok_or(CanonError::InvalidDate)?;

    Ok(date_time.format("%Y-%m-%d %H:%M:%S").to_string())
}

pub fn canon_timestamp(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    nanosecond: u32,
) -> Result<String, CanonError> {
    if !(1..=9999).contains(&year) || nanosecond >= 1_000_000_000 || nanosecond % 1000 != 0 {
        return Err(CanonError::InvalidTimestamp);
    }

    let date = NaiveDate::from_ymd_opt(year, month, day).ok_or(CanonError::InvalidTimestamp)?;
    let date_time = date
        .and_hms_nano_opt(hour, minute, second, nanosecond)
        .ok_or(CanonError::InvalidTimestamp)?;

    Ok(format!(
        "{}.{:06}",
        date_time.format("%Y-%m-%d %H:%M:%S"),
        nanosecond / 1000
    ))
}

pub fn canon_text(value: &str) -> &str {
    value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogEvent {
    CliFailed,
    SourceStarted,
    /// 已退役、不再产生：业务日期不再是一等概念（ADR-0035 §3）。
    /// 闭集只增不删（ADR-0017 §5），所以值保留。
    BusinessDateInvalid,
    SourceConfigFailed,
    TaskConfigFailed,
    /// 已退役、不再产生：SQL 形状预检整段随 ADR-0036 §5 取消。同上，值保留。
    SqlShapePrecheckFailed,
    /// 已退役、不再产生，理由同 [`LogEvent::SqlShapePrecheckFailed`]。
    SqlShapePrecheckPassed,
    StageChanged,
    MappingPrecheckFailed,
    RangeCheckExecuted,
    /// 开跑前那一次源端 `COUNT(*)`（ADR-0043 §7）。**成败都发**：
    /// 失败时 `total_rows` 为 `null` 且带上原因，运行照常继续——
    /// 为了一个进度条把整次搬运判死，是拿主功能换装饰。
    PrecountFinished,
    RunOpened,
    BatchPushed,
    CommitDiagnosed,
    AbortFailed,
    RunFinished,
    SinkStarted,
    SinkUnavailable,
    HttpResponseFailed,
}

impl LogEvent {
    pub const ALL: [Self; 19] = [
        Self::CliFailed,
        Self::SourceStarted,
        Self::BusinessDateInvalid,
        Self::SourceConfigFailed,
        Self::TaskConfigFailed,
        Self::SqlShapePrecheckFailed,
        Self::SqlShapePrecheckPassed,
        Self::StageChanged,
        Self::MappingPrecheckFailed,
        Self::RangeCheckExecuted,
        Self::PrecountFinished,
        Self::RunOpened,
        Self::BatchPushed,
        Self::CommitDiagnosed,
        Self::AbortFailed,
        Self::RunFinished,
        Self::SinkStarted,
        Self::SinkUnavailable,
        Self::HttpResponseFailed,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CliFailed => "cli_failed",
            Self::SourceStarted => "source_started",
            Self::BusinessDateInvalid => "business_date_invalid",
            Self::SourceConfigFailed => "source_config_failed",
            Self::TaskConfigFailed => "task_config_failed",
            Self::SqlShapePrecheckFailed => "sql_shape_precheck_failed",
            Self::SqlShapePrecheckPassed => "sql_shape_precheck_passed",
            Self::StageChanged => "stage_changed",
            Self::MappingPrecheckFailed => "mapping_precheck_failed",
            Self::RangeCheckExecuted => "range_check_executed",
            Self::PrecountFinished => "precount_finished",
            Self::RunOpened => "run_opened",
            Self::BatchPushed => "batch_pushed",
            Self::CommitDiagnosed => "commit_diagnosed",
            Self::AbortFailed => "abort_failed",
            Self::RunFinished => "run_finished",
            Self::SinkStarted => "sink_started",
            Self::SinkUnavailable => "sink_unavailable",
            Self::HttpResponseFailed => "http_response_failed",
        }
    }
}

impl Serialize for LogEvent {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Serialize)]
struct LogLine<'a, T> {
    ts: String,
    level: LogLevel,
    event: LogEvent,
    run_id: Option<&'a str>,
    task: Option<&'a str>,
    #[serde(flatten)]
    fields: T,
}

#[derive(Serialize)]
struct NoFields {}

pub fn write_log_line_with_fields(
    writer: &mut impl Write,
    level: LogLevel,
    event: LogEvent,
    run_id: Option<&str>,
    task: Option<&str>,
    fields: impl Serialize,
) -> io::Result<()> {
    let line = LogLine {
        ts: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        level,
        event,
        run_id,
        task,
        fields,
    };

    serde_json::to_writer(&mut *writer, &line).map_err(io::Error::other)?;
    writer.write_all(b"\n")
}

pub fn write_log_line(
    writer: &mut impl Write,
    level: LogLevel,
    event: LogEvent,
    run_id: Option<&str>,
    task: Option<&str>,
) -> io::Result<()> {
    write_log_line_with_fields(writer, level, event, run_id, task, NoFields {})
}
