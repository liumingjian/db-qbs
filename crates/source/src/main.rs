use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use db_qbs_shared::{write_log_line_with_fields, LogEvent, LogLevel};
use db_qbs_source::{
    fetch_agent_info, generate_run_id, load_source_config, load_task_config, run_transfer,
    FailureKind, HttpSinkClient, OracleRowSource, RunStage, Terminal, TransferEvent,
    TransferFailure, TransferRequest, TransferSummary, WriteMode,
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
        [(
            "message",
            json!("source one-shot process started; the task spec is the only source of truth"),
        )],
    );

    // `source.toml` 仍然要读得动、解得开——**但这个进程已经不从里面取任何值了**：
    // 目标端地址随任务文件里的 agent 端点过来（ADR-0044 §4），Oracle 客户端库目录在
    // `task.oracle` 里。留着这一步是为了「配置坏了要在开跑前就报 CONFIG」这条行为不变。
    let _source_config = match load_source_config(&arguments.config) {
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

    // SQL 形状预检整段取消（ADR-0036 §5）：六条规则里五条由生成器**结构性保证**
    // 或随「业务日期」一等概念退役，第六条（精度确定性）按所有者的降级裁定一并取消。
    // 判定并未全部消失——ADR-0009 的映射预检仍在 sink 侧硬拒。

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
    let mut source = match OracleRowSource::connect(&task) {
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

    // 开跑前的一次源端 `COUNT(*)`：迁移进度那一列的分母（ADR-0043 §7）。
    //
    // **成败都发事件，失败不中断运行**（§7 边界 3）：`total_rows` 为 null 时界面把进度退回
    // `—` 并自陈「未取到总行数」，这次搬运照样跑完。计数自己的耗时单独记 `precount_ms`，
    // 不混进 `fetch_ms`——揉进去的话，下一个人看到的「取数慢」会是两件事的和。
    //
    // 摆在游标已开、sink 还没连之前：连不上 Oracle 那一类失败在上一步就已经如实报成失败了，
    // 到这里再失败的只可能是计数本身（超时、权限、语句），那正是可以降级的那一类。
    let precount_started = Instant::now();
    match OracleRowSource::precount(&task) {
        Ok(total_rows) => emit_with_run(
            LogLevel::Info,
            LogEvent::PrecountFinished,
            Some(&run_id),
            Some(&task_path),
            [
                ("total_rows", json!(total_rows)),
                (
                    "precount_ms",
                    json!(precount_started.elapsed().as_millis() as u64),
                ),
                ("message", Value::Null),
            ],
        ),
        Err(error) => emit_with_run(
            LogLevel::Warn,
            LogEvent::PrecountFinished,
            Some(&run_id),
            Some(&task_path),
            [
                ("total_rows", Value::Null),
                (
                    "precount_ms",
                    json!(precount_started.elapsed().as_millis() as u64),
                ),
                ("message", json!(error.user_message())),
            ],
        ),
    }

    // 开跑前核一次目标端 agent 的身份（ADR-0044 §4）。**这是「停了 agent 就搬不动」
    // 那条保证的最后一道**：编排进程发起时核过一次，但从那一刻到真的开始写之间，
    // agent 可能已经停掉或被顶替；这里不核，写入就又变成了「谁在那个地址上都行」。
    if let Err(message) = verify_agent(&task) {
        emit_with_run(
            LogLevel::Error,
            LogEvent::StageChanged,
            Some(&run_id),
            Some(&task_path),
            [
                ("stage", json!(RunStage::Failed.as_str())),
                ("message", json!("target agent verification failed")),
            ],
        );
        let failure =
            TransferFailure::new(RunStage::Preparing, FailureKind::Network, message, 0, 0);
        emit_failed_run(&failure, &run_id, &task_path);
        return false;
    }

    let mut sink = match HttpSinkClient::new(&task.agent.base_url) {
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
        target_table: task.spec.target_table.clone(),
        // 目标端连接是编排进程解好写进临时任务文件的（ADR-0037 §1/§8）——
        // 子进程不碰数据源库、也不碰密钥文件。
        target: task.target.clone(),
        primary_key: task.spec.primary_key.clone(),
        write_mode: task.spec.write_mode,
        pre_sql: task.spec.pre_sql.clone(),
    };
    let result = run_transfer(&mut source, &mut sink, request, |event| {
        emit_transfer_event(event, &run_id, &task_path)
    });

    match result {
        Ok(summary) => {
            emit_successful_run(
                &summary,
                task.spec.write_mode,
                task.spec
                    .pre_sql
                    .as_deref()
                    .is_some_and(|sql| !sql.trim().is_empty()),
                &run_id,
                &task_path,
            );
            true
        }
        Err(failure) => {
            emit_failed_run(&failure, &run_id, &task_path);
            false
        }
    }
}

/// 目标端 agent 的身份核对。**地址通还不够**：注册时钉下的 `instance_id` 必须仍是
/// 现在应答的那一个，否则「同一个地址后面换了一台 agent」会被当成一切正常。
///
/// 迁移进来、还没探过的那条记录 `instance_id` 是空的（ADR-0044 §5）——那时候只核连通性，
/// 因为根本没有可比的身份；第一次探测把它补上之后这条分支就不再走了。
fn verify_agent(task: &db_qbs_source::TaskConfig) -> Result<(), String> {
    let info = fetch_agent_info(&task.agent.base_url).map_err(|error| {
        format!(
            "目标端 agent「{}」（{}）不可用：{error}。目标库只能经它访问",
            task.agent.name, task.agent.base_url
        )
    })?;
    if !task.agent.instance_id.is_empty() && info.agent_id != task.agent.instance_id {
        return Err(format!(
            "目标端 agent「{}」（{}）身份不符：注册时钉的是 {}，现在应答的是 {}",
            task.agent.name, task.agent.base_url, task.agent.instance_id, info.agent_id
        ));
    }
    Ok(())
}

/// 跑成功的那条终态日志。
///
/// **`target_table_effect` 在这里就说清楚，不留给父进程去猜**（#264）：目标表到底是
/// 「按主键合并」还是「整表被替换」，只有跑数的这一端知道本次运行的写入模式。
/// 父进程从前拿 `SUCCEEDED` 一律折成 `SWAPPED`，清空后导入接进来之后那就是假话了。
fn emit_successful_run(
    summary: &TransferSummary,
    write_mode: WriteMode,
    has_pre_sql: bool,
    run_id: &str,
    task: &Path,
) {
    let target_table_effect = if write_mode.clears_target() {
        Terminal::Replaced
    } else if has_pre_sql {
        Terminal::CleanedAndSwapped
    } else {
        Terminal::Swapped
    };
    emit_with_run(
        LogLevel::Info,
        LogEvent::RunFinished,
        Some(run_id),
        Some(task),
        [
            ("terminal", json!(RunStage::Succeeded.as_str())),
            ("stage", json!(RunStage::Succeeded.as_str())),
            ("target_table_effect", json!(target_table_effect.as_str())),
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
            suggestion,
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
                ("suggestion", json!(suggestion)),
            ],
        ),
        TransferEvent::RangeCheckExecuted {
            columns,
            scanned_rows,
            ms,
        } => emit_with_run(
            LogLevel::Info,
            LogEvent::RangeCheckExecuted,
            Some(run_id),
            Some(task),
            [
                ("columns", json!(columns)),
                ("scanned_rows", json!(scanned_rows)),
                ("ms", json!(ms)),
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
}

impl Arguments {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut config = None;
        let mut task = None;
        let mut index = 0;

        while index < arguments.len() {
            let flag = arguments[index].as_str();
            let slot = match flag {
                "--config" => &mut config,
                "--task" => &mut task,
                _ => {
                    return Err(format!(
                        "unknown argument {flag}; only --config and --task are accepted"
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
    fn only_the_two_documented_options_are_accepted() {
        // `--biz-date` 早已取消：业务日期不是一等概念。过滤条件是任务定义里那段
        // WHERE 文本，跟着任务文件走，一次运行不再从命令行接任何取值。
        let arguments = vec![
            "--config".to_owned(),
            "source.toml".to_owned(),
            "--task".to_owned(),
            "task.toml".to_owned(),
        ];
        assert!(Arguments::parse(&arguments).is_ok());

        let mut arguments_with_extra = arguments.clone();
        arguments_with_extra.extend(["--granularity".to_owned(), "DAY".to_owned()]);
        assert!(Arguments::parse(&arguments_with_extra)
            .unwrap_err()
            .contains("--granularity"));

        let mut arguments_with_biz_date = arguments;
        arguments_with_biz_date.extend(["--biz-date".to_owned(), "2026-08-14".to_owned()]);
        assert!(Arguments::parse(&arguments_with_biz_date)
            .unwrap_err()
            .contains("--biz-date"));
    }
}
