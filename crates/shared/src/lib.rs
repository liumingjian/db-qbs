use std::fmt;
use std::io::{self, Write};
use std::sync::LazyLock;

use chrono::{NaiveDate, SecondsFormat, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};

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

#[derive(Serialize)]
struct LogLine<'a, T> {
    ts: String,
    level: LogLevel,
    event: &'a str,
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
    event: &str,
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
    event: &str,
    run_id: Option<&str>,
    task: Option<&str>,
) -> io::Result<()> {
    write_log_line_with_fields(writer, level, event, run_id, task, NoFields {})
}
