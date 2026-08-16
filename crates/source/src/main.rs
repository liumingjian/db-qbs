use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use db_qbs_shared::{write_log_line_with_fields, LogEvent, LogLevel};
use db_qbs_source::{
    generate_run_id, load_source_config, load_task_config, parse_biz_date, precheck_sql,
    run_transfer, FailureKind, HttpSinkClient, OracleRowSource, RunStage, TransferEvent,
    TransferFailure, TransferRequest, TransferSummary,
};
use serde_json::{json, Map, Value};

fn main() -> ExitCode {
    if run() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn run() -> bool {
    let raw_arguments: Vec<String> = env::args().skip(1).collect();
    let task_hint = argument_value(&raw_arguments, "--task").map(absolute_path);
    let arguments = match Arguments::parse(&raw_arguments) {
        Ok(arguments) => arguments,
        Err(message) => {
            emit(
                LogLevel::Error,
                LogEvent::CliFailed,
                task_hint.as_deref(),
                [
                    ("message", json!(message)),
                    ("failure_kind", json!(FailureKind::Config.as_str())),
                ],
            );
            return false;
        }
    };

    let task_path = absolute_path(&arguments.task);
    emit(
        LogLevel::Info,
        LogEvent::SourceStarted,
        Some(&task_path),
        [
            ("biz_date", json!(arguments.biz_date)),
            (
                "message",
                json!("source one-shot process started; SQL shape messages are authored locally"),
            ),
        ],
    );

    if let Err(message) = parse_biz_date(&arguments.biz_date) {
        emit(
            LogLevel::Error,
            LogEvent::BusinessDateInvalid,
            Some(&task_path),
            [
                ("message", json!(message)),
                ("value", json!(arguments.biz_date)),
                ("failure_kind", json!(FailureKind::Config.as_str())),
            ],
        );
        return false;
    }

    let source_config = match load_source_config(&arguments.config) {
        Ok(config) => config,
        Err(error) => {
            emit(
                LogLevel::Error,
                LogEvent::SourceConfigFailed,
                Some(&task_path),
                [
                    ("message", json!(error.to_string())),
                    ("failure_kind", json!(FailureKind::Config.as_str())),
                ],
            );
            return false;
        }
    };
    let task = match load_task_config(&arguments.task) {
        Ok(task) => task,
        Err(error) => {
            emit(
                LogLevel::Error,
                LogEvent::TaskConfigFailed,
                Some(&task_path),
                [
                    ("message", json!(error.to_string())),
                    ("failure_kind", json!(FailureKind::Config.as_str())),
                ],
            );
            return false;
        }
    };

    if let Err(problems) = precheck_sql(&task) {
        emit(
            LogLevel::Error,
            LogEvent::SqlShapePrecheckFailed,
            Some(&task_path),
            [
                (
                    "message",
                    json!(format!(
                        "source-local SQL shape precheck found {} problem(s)",
                        problems.len()
                    )),
                ),
                ("problems", json!(problems)),
                ("failure_kind", json!(FailureKind::ShapePrecheck.as_str())),
            ],
        );
        return false;
    }

    emit(
        LogLevel::Info,
        LogEvent::SqlShapePrecheckPassed,
        Some(&task_path),
        [("message", json!("source-local SQL shape precheck passed"))],
    );

    let run_id = generate_run_id();
    emit_with_run(
        LogLevel::Info,
        LogEvent::StageChanged,
        Some(&run_id),
        Some(&task_path),
        [
            ("stage", json!(RunStage::Preparing.as_str())),
            (
                "message",
                json!("preparing Oracle cursor and describe metadata"),
            ),
        ],
    );
    let mut source = match OracleRowSource::connect(&source_config, &task, &arguments.biz_date) {
        Ok(source) => source,
        Err(error) => {
            emit_with_run(
                LogLevel::Error,
                LogEvent::StageChanged,
                Some(&run_id),
                Some(&task_path),
                [
                    ("stage", json!(RunStage::Failed.as_str())),
                    ("message", json!("Oracle cursor preparation failed")),
                ],
            );
            let failure = TransferFailure::from_source_error(RunStage::Preparing, error, 0, 0);
            emit_failed_run(&failure, &run_id, &task_path);
            return false;
        }
    };

    let mut sink = match HttpSinkClient::new(&source_config.sink_base_url) {
        Ok(sink) => sink,
        Err(message) => {
            emit_with_run(
                LogLevel::Error,
                LogEvent::StageChanged,
                Some(&run_id),
                Some(&task_path),
                [
                    ("stage", json!(RunStage::Failed.as_str())),
                    ("message", json!("sink client preparation failed")),
                ],
            );
            let failure =
                TransferFailure::new(RunStage::Preparing, FailureKind::Config, message, 0, 0);
            emit_failed_run(&failure, &run_id, &task_path);
            return false;
        }
    };

    let request = TransferRequest {
        run_id: run_id.clone(),
        target_table: task.target_table,
        target_date_col: task.target_date_col,
        biz_date: arguments.biz_date,
    };
    let result = run_transfer(&mut source, &mut sink, request, |event| {
        emit_transfer_event(event, &run_id, &task_path)
    });

    match result {
        Ok(summary) => {
            emit_successful_run(&summary, &run_id, &task_path);
            true
        }
        Err(failure) => {
            emit_failed_run(&failure, &run_id, &task_path);
            false
        }
    }
}

fn emit_successful_run(summary: &TransferSummary, run_id: &str, task: &Path) {
    emit_with_run(
        LogLevel::Info,
        LogEvent::RunFinished,
        Some(run_id),
        Some(task),
        [
            ("terminal", json!(RunStage::Succeeded.as_str())),
            ("stage", json!(RunStage::Succeeded.as_str())),
            ("message", json!("run completed successfully")),
            ("failure_kind", Value::Null),
            ("source_code", Value::Null),
            ("sink_code", Value::Null),
            ("column", Value::Null),
            ("value", Value::Null),
            ("source_rows", json!(summary.source_rows)),
            ("source_batches", json!(summary.total_batches)),
            ("staged_rows", json!(summary.staged_rows)),
            ("received_batches", json!(summary.total_batches)),
            ("sink_reported_rows", json!(summary.source_rows)),
            ("purged_rows", json!(summary.purged_rows)),
            ("fetch_ms", json!(summary.fetch_ms)),
            ("push_ms", json!(summary.push_ms)),
            ("commit_ms", json!(summary.commit_ms)),
            ("count_ms", json!(summary.count_ms)),
            ("cursor_ms", json!(summary.cursor_ms)),
        ],
    );
}

fn emit_transfer_event(event: TransferEvent, run_id: &str, task: &Path) {
    match event {
        TransferEvent::StageChanged(RunStage::Preparing) => {}
        TransferEvent::StageChanged(stage) => emit_with_run(
            if stage == RunStage::Failed {
                LogLevel::Error
            } else {
                LogLevel::Info
            },
            LogEvent::StageChanged,
            Some(run_id),
            Some(task),
            [
                ("stage", json!(stage.as_str())),
                ("message", json!(format!("run entered {}", stage.as_str()))),
            ],
        ),
        TransferEvent::RunOpened {
            staging_table,
            columns_checked,
        } => emit_with_run(
            LogLevel::Info,
            LogEvent::RunOpened,
            Some(run_id),
            Some(task),
            [
                ("staging_table", json!(staging_table)),
                ("columns_checked", json!(columns_checked)),
                ("message", json!("sink accepted run and created staging")),
            ],
        ),
        TransferEvent::BatchPushed {
            seq,
            rows,
            source_rows,
            bytes,
            written,
            ms,
        } => emit_with_run(
            LogLevel::Info,
            LogEvent::BatchPushed,
            Some(run_id),
            Some(task),
            [
                ("seq", json!(seq)),
                ("rows", json!(rows)),
                ("source_rows", json!(source_rows)),
                ("bytes", json!(bytes)),
                ("written", json!(written)),
                ("ms", json!(ms)),
            ],
        ),
        TransferEvent::MappingPrecheckFailed {
            column,
            source,
            target,
            rule,
        } => emit_with_run(
            LogLevel::Error,
            LogEvent::MappingPrecheckFailed,
            Some(run_id),
            Some(task),
            [
                ("column", json!(column)),
                ("source", json!(source)),
                ("target", json!(target)),
                ("message", json!(format!("目标端：{rule}"))),
                ("rule", json!(rule)),
            ],
        ),
        TransferEvent::CommitDiagnosed { terminal, message } => emit_with_run(
            LogLevel::Warn,
            LogEvent::CommitDiagnosed,
            Some(run_id),
            Some(task),
            [
                ("terminal", json!(terminal.map(|value| value.as_str()))),
                ("message", json!(message)),
            ],
        ),
        TransferEvent::AbortFailed { message } => emit_with_run(
            LogLevel::Warn,
            LogEvent::AbortFailed,
            Some(run_id),
            Some(task),
            [("message", json!(message))],
        ),
    }
}

fn emit_failed_run(failure: &TransferFailure, run_id: &str, task: &Path) {
    emit_with_run(
        LogLevel::Error,
        LogEvent::RunFinished,
        Some(run_id),
        Some(task),
        [
            ("terminal", json!(RunStage::Failed.as_str())),
            ("stage", json!(failure.stage.as_str())),
            ("message", json!(failure.message)),
            ("failure_kind", json!(failure.kind.as_str())),
            ("source_code", json!(failure.source_code)),
            ("sink_code", json!(failure.sink_code)),
            ("column", json!(failure.column)),
            ("value", json!(failure.value)),
            ("source_rows", json!(failure.source_rows)),
            ("source_batches", json!(failure.total_batches)),
            ("staged_rows", json!(failure.staged_rows)),
            ("received_batches", json!(failure.received_batches)),
            ("sink_reported_rows", json!(failure.sink_reported_rows)),
            ("purged_rows", json!(failure.purged_rows)),
            ("fetch_ms", json!(failure.fetch_ms)),
            ("push_ms", json!(failure.push_ms)),
            ("commit_ms", json!(failure.commit_ms)),
            ("count_ms", json!(failure.count_ms)),
            ("cursor_ms", json!(failure.cursor_ms)),
        ],
    );
}

#[derive(Debug, PartialEq, Eq)]
struct Arguments {
    config: PathBuf,
    task: PathBuf,
    biz_date: String,
}

impl Arguments {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut config = None;
        let mut task = None;
        let mut biz_date = None;
        let mut index = 0;

        while index < arguments.len() {
            let flag = arguments[index].as_str();
            let slot = match flag {
                "--config" => &mut config,
                "--task" => &mut task,
                "--biz-date" => &mut biz_date,
                _ => {
                    return Err(format!(
                        "unknown argument {flag}; only --config, --task, and --biz-date are accepted"
                    ));
                }
            };
            let value = arguments
                .get(index + 1)
                .ok_or_else(|| format!("{flag} requires a value"))?;
            if value.starts_with("--") {
                return Err(format!("{flag} requires a value"));
            }

            if slot.replace(value.clone()).is_some() {
                return Err(format!("{flag} may be provided only once"));
            }
            index += 2;
        }

        Ok(Self {
            config: config
                .map(PathBuf::from)
                .ok_or_else(|| "missing required --config".to_owned())?,
            task: task
                .map(PathBuf::from)
                .ok_or_else(|| "missing required --task".to_owned())?,
            biz_date: biz_date.ok_or_else(|| "missing required --biz-date".to_owned())?,
        })
    }
}

fn argument_value<'a>(arguments: &'a [String], name: &str) -> Option<&'a str> {
    arguments
        .windows(2)
        .find(|pair| pair[0] == name && !pair[1].starts_with("--"))
        .map(|pair| pair[1].as_str())
}

fn absolute_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_owned()
        } else {
            env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    })
}

fn emit<const N: usize>(
    level: LogLevel,
    event: LogEvent,
    task: Option<&Path>,
    fields: [(&str, Value); N],
) {
    emit_with_run(level, event, None, task, fields);
}

fn emit_with_run<const N: usize>(
    level: LogLevel,
    event: LogEvent,
    run_id: Option<&str>,
    task: Option<&Path>,
    fields: [(&str, Value); N],
) {
    let mut details = Map::new();
    for (name, value) in fields {
        details.insert(name.to_owned(), value);
    }

    let stdout = io::stdout();
    let mut writer = stdout.lock();
    let task = task.map(|path| path.to_string_lossy());
    let _ = write_log_line_with_fields(&mut writer, level, event, run_id, task.as_deref(), details);
}

#[cfg(test)]
mod tests {
    use super::Arguments;

    #[test]
    fn only_the_three_documented_options_are_accepted() {
        let arguments = vec![
            "--config".to_owned(),
            "source.toml".to_owned(),
            "--task".to_owned(),
            "task.toml".to_owned(),
            "--biz-date".to_owned(),
            "2026-08-14".to_owned(),
        ];
        assert!(Arguments::parse(&arguments).is_ok());

        let mut arguments_with_extra = arguments;
        arguments_with_extra.extend(["--granularity".to_owned(), "DAY".to_owned()]);
        assert!(Arguments::parse(&arguments_with_extra)
            .unwrap_err()
            .contains("--granularity"));
    }
}
