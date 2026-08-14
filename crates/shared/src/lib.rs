use std::fmt;
use std::io::{self, Write};
use std::sync::LazyLock;

use chrono::{NaiveDate, SecondsFormat, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize, Serializer};

static CANONICAL_NUMBER_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(0|-?[1-9][0-9]*(\.[0-9]*[1-9])?|-?0\.[0-9]*[1-9])$")
        .expect("canonical NUMBER grammar must compile")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonError {
    NonCanonicalNumber,
    InvalidDate,
}

impl fmt::Display for CanonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonCanonicalNumber => {
                formatter.write_str("driver emitted a non-canonical NUMBER")
            }
            Self::InvalidDate => formatter.write_str("invalid DATE components"),
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

pub fn canon_text(value: &str) -> &str {
    value
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchPayload {
    pub seq: u64,
    pub rows: Vec<Vec<Option<String>>>,
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
    BusinessDateInvalid,
    SourceConfigFailed,
    TaskConfigFailed,
    SqlShapePrecheckFailed,
    SqlShapePrecheckPassed,
    StageChanged,
    MappingPrecheckFailed,
    RunOpened,
    BatchPushed,
    CommitDiagnosed,
    AbortFailed,
    RunFinished,
    SinkUnavailable,
    HttpResponseFailed,
}

impl LogEvent {
    pub const ALL: [Self; 16] = [
        Self::CliFailed,
        Self::SourceStarted,
        Self::BusinessDateInvalid,
        Self::SourceConfigFailed,
        Self::TaskConfigFailed,
        Self::SqlShapePrecheckFailed,
        Self::SqlShapePrecheckPassed,
        Self::StageChanged,
        Self::MappingPrecheckFailed,
        Self::RunOpened,
        Self::BatchPushed,
        Self::CommitDiagnosed,
        Self::AbortFailed,
        Self::RunFinished,
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
            Self::RunOpened => "run_opened",
            Self::BatchPushed => "batch_pushed",
            Self::CommitDiagnosed => "commit_diagnosed",
            Self::AbortFailed => "abort_failed",
            Self::RunFinished => "run_finished",
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
