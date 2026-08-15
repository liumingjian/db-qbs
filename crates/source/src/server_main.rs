use std::collections::HashMap;
use std::env;
use std::fs::{self, OpenOptions, Permissions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::ToSocketAddrs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use db_qbs_shared::{write_log_line_with_fields, LogEvent, LogLevel};
use db_qbs_source::{
    generate_target_ddl, load_source_config, parse_biz_date, sql_shape_report, OracleRowSource,
    SourceConfig, Task, TaskConfig, TaskInput, TaskStore,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use signal_hook::consts::SIGTERM;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const MAX_TASK_BODY_BYTES: u64 = 1024 * 1024;
const RUN_TASK_DIRECTORY: &str = "run-tasks";

type RunRegistry = Arc<Mutex<HashMap<String, RunRecord>>>;

#[derive(Clone)]
struct RunRecord {
    run_id: Option<String>,
    biz_date: Option<String>,
    staging_table: Option<String>,
    projection: RunProjection,
}

#[derive(Clone, Default, Serialize)]
struct RunProjection {
    stage: Option<String>,
    seq: u64,
    rows_pushed: u64,
    bytes: u64,
    ms: u64,
    last_ts: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartRunInput {
    task_id: String,
    biz_date: String,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            emit(
                LogLevel::Error,
                LogEvent::SourceConfigFailed,
                json!({ "message": message }),
            );
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let config_path = parse_config_path(env::args().skip(1))?;
    let config = load_source_config(Path::new(&config_path)).map_err(|error| error.to_string())?;
    serve(config, PathBuf::from(config_path))
}

fn parse_config_path(mut args: impl Iterator<Item = String>) -> Result<String, String> {
    match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--config"), Some(path), None) => Ok(path),
        _ => Err("用法：db-qbs-source --config <source.toml>".to_owned()),
    }
}

fn serve(config: SourceConfig, config_path: PathBuf) -> Result<(), String> {
    if config.listen.is_empty() {
        return Err("source 配置 listen 不能为空".to_owned());
    }

    let store = TaskStore::open(&config.data_dir)?;
    let runs = Arc::new(Mutex::new(HashMap::new()));
    let server = Server::http(&config.listen)
        .map_err(|error| format!("监听 {} 失败：{error}", config.listen))?;
    emit(
        LogLevel::Info,
        LogEvent::SourceStarted,
        json!({
            "listen": config.listen,
            "message": "source 长驻编排进程已启动",
        }),
    );
    if !is_loopback(&config.listen) {
        emit(
            LogLevel::Warn,
            LogEvent::SourceStarted,
            json!({
                "listen": config.listen,
                "message": format!(
                    "本服务无鉴权；能连上者可对源库跑任意 SQL 并清空重写目标表；运行历史含源库真实业务值；当前监听地址：{}",
                    config.listen
                ),
            }),
        );
    }

    let terminated = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGTERM, Arc::clone(&terminated))
        .map_err(|error| format!("注册 SIGTERM 处理失败：{error}"))?;

    while !terminated.load(Ordering::Relaxed) {
        if let Some(request) = server
            .recv_timeout(Duration::from_millis(100))
            .map_err(|error| format!("接收 HTTP 请求失败：{error}"))?
        {
            handle_request(request, &config, &config_path, &store, &runs);
        }
    }
    Ok(())
}

fn is_loopback(listen: &str) -> bool {
    let Ok(mut addresses) = listen.to_socket_addrs() else {
        return false;
    };
    let Some(first) = addresses.next() else {
        return false;
    };
    first.ip().is_loopback() && addresses.all(|address| address.ip().is_loopback())
}

fn handle_request(
    mut request: Request,
    config: &SourceConfig,
    config_path: &Path,
    store: &TaskStore,
    runs: &RunRegistry,
) {
    let response = route_request(&mut request, config, config_path, store, runs);

    if let Err(error) = request.respond(response) {
        emit(
            LogLevel::Error,
            LogEvent::HttpResponseFailed,
            json!({ "message": format!("HTTP 响应写入失败：{error}") }),
        );
    }
}

fn route_request(
    request: &mut Request,
    config: &SourceConfig,
    config_path: &Path,
    store: &TaskStore,
    runs: &RunRegistry,
) -> HttpResponse {
    let method = request.method().clone();
    let path = request.url().to_owned();

    if method == Method::Post && path == "/api/columns" {
        return handle_column_fetch(request, config);
    }

    if method == Method::Post && path == "/api/runs" {
        return handle_start_run(request, config, config_path, store, runs);
    }

    if method == Method::Get {
        if let Some(run_record_id) = run_record_id_from_path(&path) {
            return handle_get_run(runs, run_record_id);
        }
    }

    if path == "/api/tasks" {
        return match method {
            Method::Get => handle_list_tasks(store),
            Method::Post => handle_create_task(request, store),
            _ => not_found(),
        };
    }

    let Some(task_id) = task_id_from_path(&path) else {
        return not_found();
    };
    match method {
        Method::Get => handle_get_task(store, task_id),
        Method::Put => handle_update_task(request, store, task_id),
        Method::Delete => handle_delete_task(store, task_id),
        _ => not_found(),
    }
}

fn run_record_id_from_path(path: &str) -> Option<&str> {
    let run_record_id = path.strip_prefix("/api/runs/")?;
    if run_record_id.is_empty() || run_record_id.contains('/') {
        return None;
    }
    Some(run_record_id)
}

fn handle_start_run(
    request: &mut Request,
    config: &SourceConfig,
    config_path: &Path,
    store: &TaskStore,
    runs: &RunRegistry,
) -> HttpResponse {
    let input: StartRunInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    if let Err(error) = parse_biz_date(&input.biz_date) {
        return bad_request(error.to_owned());
    }
    let task = match store.get(&input.task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return not_found(),
        Err(error) => return internal_error(error),
    };

    match start_run(config, config_path, task, input.biz_date, runs) {
        Ok(run_record_id) => json_response(202, &json!({ "run_record_id": run_record_id })),
        Err(error) => internal_error(error),
    }
}

fn handle_get_run(runs: &RunRegistry, run_record_id: &str) -> HttpResponse {
    let record = match runs.lock() {
        Ok(runs) => runs.get(run_record_id).cloned(),
        Err(_) => return internal_error("run 投影锁已损坏".to_owned()),
    };
    let Some(record) = record else {
        return not_found();
    };
    json_response(
        200,
        &json!({
            "run_record_id": run_record_id,
            "run_id": record.run_id,
            "biz_date": record.biz_date,
            "staging_table": record.staging_table,
            "stage": record.projection.stage,
            "seq": record.projection.seq,
            "rows_pushed": record.projection.rows_pushed,
            "bytes": record.projection.bytes,
            "ms": record.projection.ms,
            "last_ts": record.projection.last_ts,
            "live": true,
        }),
    )
}

fn start_run(
    config: &SourceConfig,
    config_path: &Path,
    task: Task,
    biz_date: String,
    runs: &RunRegistry,
) -> Result<String, String> {
    let run_record_id = generate_record_id();
    let task_path = materialize_task(config, &task, &run_record_id)?;
    let mut child = match Command::new(&config.run_executable)
        .arg("--config")
        .arg(config_path)
        .arg("--task")
        .arg(&task_path)
        .arg("--biz-date")
        .arg(&biz_date)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = fs::remove_file(&task_path);
            return Err(format!("启动 run 子进程失败：{error}"));
        }
    };
    let stdout = child
        .stdout
        .take()
        .expect("stdout is available after configuring it as piped");

    runs.lock()
        .map_err(|_| "run 投影锁已损坏".to_owned())?
        .insert(
            run_record_id.clone(),
            RunRecord {
                run_id: None,
                biz_date: None,
                staging_table: None,
                projection: RunProjection::default(),
            },
        );

    let worker_runs = Arc::clone(runs);
    let worker_record_id = run_record_id.clone();
    thread::spawn(move || supervise_run(child, stdout, task_path, worker_record_id, worker_runs));
    Ok(run_record_id)
}

fn materialize_task(
    config: &SourceConfig,
    task: &Task,
    run_record_id: &str,
) -> Result<PathBuf, String> {
    let directory = config.data_dir.join(RUN_TASK_DIRECTORY);
    fs::create_dir_all(&directory).map_err(|error| format!("创建临时任务目录失败：{error}"))?;
    let path = directory.join(format!("task-{run_record_id}.toml"));
    let task_config = TaskConfig {
        source_sql: task.source_sql.clone(),
        source_date_col: task.source_date_col.clone(),
        target_table: task.target_table.clone(),
        target_date_col: task.target_date_col.clone(),
    };
    let contents = toml::to_string(&task_config)
        .map_err(|error| format!("序列化临时任务定义失败：{error}"))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|error| format!("创建临时任务定义失败：{error}"))?;
    file.write_all(contents.as_bytes())
        .map_err(|error| format!("写入临时任务定义失败：{error}"))?;
    fs::set_permissions(&path, Permissions::from_mode(0o600))
        .map_err(|error| format!("设置临时任务定义权限失败：{error}"))?;
    Ok(path)
}

fn supervise_run(
    mut child: Child,
    stdout: impl Read,
    task_path: PathBuf,
    run_record_id: String,
    runs: RunRegistry,
) {
    let mut pending_biz_date = None;
    let mut active_run_id = None;
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else {
            break;
        };
        {
            let stdout = io::stdout();
            let mut writer = stdout.lock();
            let _ = writeln!(writer, "{line}");
        }
        let Ok(log) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let terminal = apply_log_line(
            &runs,
            &run_record_id,
            &log,
            &mut pending_biz_date,
            &mut active_run_id,
        );
        if terminal {
            remove_projection(&runs, &run_record_id);
        }
    }
    let _ = child.wait();
    let _ = fs::remove_file(task_path);
    remove_projection(&runs, &run_record_id);
}

fn apply_log_line(
    runs: &RunRegistry,
    run_record_id: &str,
    log: &Value,
    pending_biz_date: &mut Option<String>,
    active_run_id: &mut Option<String>,
) -> bool {
    let event = log.get("event").and_then(Value::as_str);
    if event == Some("source_started") {
        *pending_biz_date = log
            .get("biz_date")
            .and_then(Value::as_str)
            .map(str::to_owned);
    }

    let line_run_id = log.get("run_id").and_then(Value::as_str);
    if let Some(line_run_id) = line_run_id {
        if let Some(active_run_id) = active_run_id.as_deref() {
            if active_run_id != line_run_id {
                return false;
            }
        } else {
            *active_run_id = Some(line_run_id.to_owned());
        }
    }

    let Ok(mut registry) = runs.lock() else {
        return false;
    };
    let Some(record) = registry.get_mut(run_record_id) else {
        return event == Some("run_finished");
    };
    if record.run_id.is_none() {
        if let Some(run_id) = active_run_id.as_ref() {
            record.run_id = Some(run_id.clone());
            record.biz_date = pending_biz_date.clone();
        }
    }
    if let Some(ts) = log.get("ts").and_then(Value::as_str) {
        record.projection.last_ts = Some(ts.to_owned());
    }

    match event {
        Some("stage_changed") => {
            if let Some(stage) = log.get("stage").and_then(Value::as_str) {
                record.projection.stage = Some(stage.to_owned());
            }
        }
        Some("run_opened") => {
            record.staging_table = log
                .get("staging_table")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        Some("batch_pushed") => {
            if let Some(seq) = log.get("seq").and_then(Value::as_u64) {
                record.projection.seq = seq;
            }
            record.projection.rows_pushed += log.get("rows").and_then(Value::as_u64).unwrap_or(0);
            record.projection.bytes += log.get("bytes").and_then(Value::as_u64).unwrap_or(0);
            record.projection.ms += log.get("ms").and_then(Value::as_u64).unwrap_or(0);
        }
        _ => {}
    }

    event == Some("run_finished")
}

fn remove_projection(runs: &RunRegistry, run_record_id: &str) {
    if let Ok(mut runs) = runs.lock() {
        runs.remove(run_record_id);
    }
}

fn generate_record_id() -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut id = String::with_capacity(32);
    for byte in bytes {
        id.push(HEX[(byte >> 4) as usize] as char);
        id.push(HEX[(byte & 0x0f) as usize] as char);
    }
    id
}

fn task_id_from_path(path: &str) -> Option<&str> {
    let task_id = path.strip_prefix("/api/tasks/")?;
    if task_id.is_empty() || task_id.contains('/') {
        return None;
    }
    Some(task_id)
}

fn handle_list_tasks(store: &TaskStore) -> HttpResponse {
    match store.list() {
        Ok(tasks) => json_response(200, &tasks),
        Err(error) => internal_error(error),
    }
}

fn handle_create_task(request: &mut Request, store: &TaskStore) -> HttpResponse {
    let input = match read_task_input(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    match store.create(input) {
        Ok(task) => json_response(201, &task),
        Err(error) => internal_error(error),
    }
}

fn handle_get_task(store: &TaskStore, task_id: &str) -> HttpResponse {
    match store.get(task_id) {
        Ok(Some(task)) => json_response(200, &task),
        Ok(None) => not_found(),
        Err(error) => internal_error(error),
    }
}

fn handle_update_task(request: &mut Request, store: &TaskStore, task_id: &str) -> HttpResponse {
    let input = match read_task_input(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    match store.update(task_id, input) {
        Ok(Some(task)) => json_response(200, &task),
        Ok(None) => not_found(),
        Err(error) => internal_error(error),
    }
}

fn handle_delete_task(store: &TaskStore, task_id: &str) -> HttpResponse {
    match store.delete(task_id) {
        Ok(Some(task)) => json_response(200, &task),
        Ok(None) => not_found(),
        Err(error) => internal_error(error),
    }
}

fn read_task_input(request: &mut Request) -> Result<TaskInput, String> {
    read_json_body(request)
}

fn read_json_body<T: for<'de> Deserialize<'de>>(request: &mut Request) -> Result<T, String> {
    let mut body = Vec::new();
    request
        .as_reader()
        .take(MAX_TASK_BODY_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|error| format!("读取请求体失败：{error}"))?;
    if body.len() as u64 > MAX_TASK_BODY_BYTES {
        return Err("请求体超过 1 MiB".to_owned());
    }
    serde_json::from_slice(&body).map_err(|error| format!("JSON 请求体无效：{error}"))
}

type HttpResponse = Response<std::io::Cursor<Vec<u8>>>;

fn not_found() -> HttpResponse {
    json_response(
        404,
        &json!({ "error": { "message": "请求的 source API 资源不存在" } }),
    )
}

fn bad_request(message: String) -> HttpResponse {
    json_response(400, &json!({ "error": { "message": message } }))
}

fn internal_error(message: String) -> HttpResponse {
    json_response(500, &json!({ "error": { "message": message } }))
}

fn json_response(status: u16, value: &impl Serialize) -> HttpResponse {
    let body = serde_json::to_vec(value).expect("serializing an HTTP response must succeed");
    let content_type = Header::from_bytes("Content-Type", "application/json; charset=utf-8")
        .expect("static response header must be valid");
    Response::from_data(body)
        .with_status_code(StatusCode(status))
        .with_header(content_type)
}

fn handle_column_fetch(request: &mut Request, config: &SourceConfig) -> HttpResponse {
    let mut body = String::new();
    if let Err(error) = request.as_reader().read_to_string(&mut body) {
        return json_response(
            400,
            &json!({ "kind": "request", "message": format!("could not read request: {error}") }),
        );
    }
    let task: TaskConfig = match serde_json::from_str(&body) {
        Ok(task) => task,
        Err(error) => {
            return json_response(
                400,
                &json!({ "kind": "request", "message": format!("invalid JSON request: {error}") }),
            )
        }
    };

    let checks = sql_shape_report(&task);
    if checks.iter().any(|check| !check.passed) {
        return json_response(
            422,
            &json!({
                "kind": "sql_shape",
                "message": "source-local SQL shape precheck failed",
                "checks": checks,
            }),
        );
    }

    let columns = match OracleRowSource::describe(config, &task) {
        Ok(columns) => columns,
        Err(error) => {
            return json_response(
                502,
                &json!({
                    "kind": "oracle",
                    "message": error.user_message(),
                    "oracle_code": error.oracle_code,
                }),
            )
        }
    };
    match generate_target_ddl(&columns, &task.target_table, &task.target_date_col) {
        Ok(target_ddl) => json_response(
            200,
            &json!({ "columns": columns, "target_ddl": target_ddl }),
        ),
        Err(error) => json_response(
            422,
            &json!({
                "kind": "target_ddl",
                "message": error.message,
                "column": error.column,
            }),
        ),
    }
}
fn emit(level: LogLevel, event: LogEvent, fields: serde_json::Value) {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    let _ = write_log_line_with_fields(&mut writer, level, event, None, None, fields);
}
