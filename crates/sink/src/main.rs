use std::env;
use std::io;
use std::process::ExitCode;

use db_qbs_shared::{write_log_line_with_fields, LogEvent, LogLevel};
use db_qbs_sink::{serve, SinkConfig};
use serde_json::json;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            let stdout = io::stdout();
            let mut writer = stdout.lock();
            let _ = write_log_line_with_fields(
                &mut writer,
                LogLevel::Error,
                LogEvent::SinkUnavailable,
                None,
                None,
                json!({ "message": message }),
            );
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let config_path = parse_config_path(env::args().skip(1))?;
    let config = SinkConfig::load(&config_path)?;
    serve(config)
}

fn parse_config_path(mut args: impl Iterator<Item = String>) -> Result<String, String> {
    match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--config"), Some(path), None) => Ok(path),
        _ => Err("用法：db-qbs-sink --config <sink.toml>".to_owned()),
    }
}
