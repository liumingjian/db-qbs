use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use db_qbs_shared::{write_log_line_with_fields, LogLevel};
use db_qbs_source::{
    load_source_config, load_task_config, parse_biz_date, precheck_sql, probe_sink,
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
                "cli_failed",
                task_hint.as_deref(),
                [("message", json!(message))],
            );
            return false;
        }
    };

    let task_path = absolute_path(&arguments.task);
    emit(
        LogLevel::Info,
        "source_started",
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
            "business_date_invalid",
            Some(&task_path),
            [
                ("message", json!(message)),
                ("value", json!(arguments.biz_date)),
            ],
        );
        return false;
    }

    let source_config = match load_source_config(&arguments.config) {
        Ok(config) => config,
        Err(error) => {
            emit(
                LogLevel::Error,
                "source_config_failed",
                Some(&task_path),
                [("message", json!(error.to_string()))],
            );
            return false;
        }
    };
    let task = match load_task_config(&arguments.task) {
        Ok(task) => task,
        Err(error) => {
            emit(
                LogLevel::Error,
                "task_config_failed",
                Some(&task_path),
                [("message", json!(error.to_string()))],
            );
            return false;
        }
    };

    if let Err(problems) = precheck_sql(&task) {
        emit(
            LogLevel::Error,
            "sql_shape_precheck_failed",
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
            ],
        );
        return false;
    }

    emit(
        LogLevel::Info,
        "sql_shape_precheck_passed",
        Some(&task_path),
        [("message", json!("source-local SQL shape precheck passed"))],
    );

    let message = match probe_sink(&source_config.sink_base_url) {
        Ok(()) => "next source stage is not implemented".to_owned(),
        Err(message) => message,
    };
    emit(
        LogLevel::Error,
        "next_stage_failed",
        Some(&task_path),
        [("message", json!(message))],
    );
    false
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
    event: &str,
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
    let _ = write_log_line_with_fields(&mut writer, level, event, None, task.as_deref(), details);
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
