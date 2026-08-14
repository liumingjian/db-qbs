use std::env;
use std::process::ExitCode;

use chrono::{SecondsFormat, Utc};
use db_qbs_sink::{serve, SinkConfig};
use serde_json::json;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            println!(
                "{}",
                json!({
                    "ts": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                    "level": "error",
                    "event": "sink_unavailable",
                    "run_id": null,
                    "task": null,
                    "message": message,
                })
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
