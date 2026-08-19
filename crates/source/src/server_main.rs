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

use chrono::Utc;
use db_qbs_shared::{write_log_line_with_fields, LogEvent, LogLevel};
use db_qbs_source::{
    embedded_web_asset, generate_target_ddl, load_source_config, validate_builder_dblink,
    ColumnPrecision, HistoryChange, HistoryStore, OracleRowSource, RunHistory, RunParams,
    SourceConfig, Task, TaskConfig, TaskInput, TaskSpec, TaskStore, UnknownReason,
};
use rand::RngCore;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use signal_hook::consts::SIGTERM;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const MAX_REQUEST_BODY_BYTES: u64 = 1024 * 1024;
const RUN_TASKS_DIRECTORY: &str = "run-tasks";

type RunRegistry = Arc<Mutex<RunState>>;

#[derive(Default)]
struct RunState {
    live_histories: HashMap<String, RunHistory>,
    active_runs: HashMap<String, ActiveRun>,
}

struct ActiveRun {
    task_id: String,
    /// 互斥键的后半段：本次运行参数集（ADR-0036 §7）。`RunParams` 是 `BTreeMap`，
    /// 相等比较天然按参数名排序、值原样比，正是那份规范形式。
    run_params: RunParams,
    child_pid: Option<u32>,
    stage: Option<String>,
}

enum StartRunError {
    AlreadyRunning,
    Internal(String),
}

struct ServerState<'a> {
    config: &'a SourceConfig,
    config_path: &'a Path,
    tasks: &'a TaskStore,
    history: &'a HistoryStore,
    runs: &'a RunRegistry,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartRunInput {
    task_id: String,
    #[serde(default)]
    run_params: RunParams,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuilderLinkInput {
    dblink: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuilderColumnsInput {
    dblink: Option<String>,
    owner: String,
    table: String,
}

/// 取列请求。`column_precision` **不在任务定义里**（ADR-0036 §6）：目标表 DDL 生成吃的是
/// describe 回来的源列，属「取列」链，这份精度提示随请求一起来、用完即弃。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ColumnFetchInput {
    spec: TaskSpec,
    #[serde(default)]
    column_precision: Option<ColumnPrecision>,
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

    let task_store = TaskStore::open(&config.data_dir)?;
    let history_store = HistoryStore::open(&config.data_dir)?;
    history_store.seal_incomplete(
        UnknownReason::ProcessDisappeared,
        Utc::now(),
        config.history_retention_days,
    )?;
    clean_run_tasks(&config.data_dir)?;
    let runs = Arc::new(Mutex::new(RunState::default()));
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
    let state = ServerState {
        config: &config,
        config_path: &config_path,
        tasks: &task_store,
        history: &history_store,
        runs: &runs,
    };

    while !terminated.load(Ordering::Relaxed) {
        if let Some(request) = server
            .recv_timeout(Duration::from_millis(100))
            .map_err(|error| format!("接收 HTTP 请求失败：{error}"))?
        {
            handle_request(request, &state);
        }
    }
    history_store.seal_incomplete(
        UnknownReason::ServiceRestarted,
        Utc::now(),
        config.history_retention_days,
    )?;
    clean_run_tasks(&config.data_dir)?;
    Ok(())
}

fn clean_run_tasks(data_dir: &Path) -> Result<(), String> {
    let directory = data_dir.join(RUN_TASKS_DIRECTORY);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("读取临时任务目录失败：{error}")),
    };
    for entry in entries {
        let path = entry
            .map_err(|error| format!("读取临时任务文件失败：{error}"))?
            .path();
        if path.is_file() {
            fs::remove_file(&path)
                .map_err(|error| format!("清扫临时任务文件 {} 失败：{error}", path.display()))?;
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

fn handle_request(mut request: Request, state: &ServerState<'_>) {
    let response = route_request(&mut request, state);

    if let Err(error) = request.respond(response) {
        emit(
            LogLevel::Error,
            LogEvent::HttpResponseFailed,
            json!({ "message": format!("HTTP 响应写入失败：{error}") }),
        );
    }
}

fn route_request(request: &mut Request, state: &ServerState<'_>) -> HttpResponse {
    let method = request.method().clone();
    let url = request.url().to_owned();
    let (path, query) = url
        .split_once('?')
        .map_or((url.as_str(), None), |(path, query)| (path, Some(query)));

    if path == "/api" || path.starts_with("/api/") {
        return route_api_request(request, state, method, path, query);
    }

    if method == Method::Get {
        if let Some(asset) = embedded_web_asset(path) {
            return embedded_response(asset.body.into_owned(), asset.content_type, path);
        }
    }
    not_found()
}

fn route_api_request(
    request: &mut Request,
    state: &ServerState<'_>,
    method: Method,
    path: &str,
    query: Option<&str>,
) -> HttpResponse {
    if method == Method::Post && path == "/api/columns" {
        return handle_column_fetch(request, state.config);
    }

    if method == Method::Post && path == "/api/builder/tables" {
        return handle_builder_tables(request, state.config);
    }

    if method == Method::Post && path == "/api/builder/columns" {
        return handle_builder_columns(request, state.config);
    }

    if method == Method::Post && path == "/api/builder/sql" {
        return handle_builder_sql(request);
    }

    if method == Method::Post && path == "/api/runs" {
        return handle_start_run(request, state);
    }

    if method == Method::Post {
        if let Some(run_record_id) = cancel_run_id_from_path(path) {
            return handle_cancel_run(state.runs, run_record_id);
        }
    }

    if method == Method::Get && path == "/api/runs" {
        return handle_list_history(state.history, query);
    }

    if method == Method::Get {
        if let Some(run_record_id) = resource_id_from_path(path, "/api/runs/") {
            return handle_get_run(state.runs, state.history, run_record_id);
        }
    }

    if path == "/api/tasks" {
        return match method {
            Method::Get => handle_list_tasks(state.tasks),
            Method::Post => handle_create_task(request, state.tasks),
            _ => not_found(),
        };
    }

    let Some(task_id) = resource_id_from_path(path, "/api/tasks/") else {
        return not_found();
    };
    match method {
        Method::Get => handle_get_task(state.tasks, task_id),
        Method::Put => handle_update_task(request, state.tasks, task_id),
        Method::Delete => handle_delete_task(state.tasks, task_id),
        _ => not_found(),
    }
}

fn resource_id_from_path<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let resource_id = path.strip_prefix(prefix)?;
    if resource_id.is_empty() || resource_id.contains('/') {
        return None;
    }
    Some(resource_id)
}

fn cancel_run_id_from_path(path: &str) -> Option<&str> {
    let run_record_id = path.strip_prefix("/api/runs/")?.strip_suffix("/cancel")?;
    if run_record_id.is_empty() || run_record_id.contains('/') {
        return None;
    }
    Some(run_record_id)
}

fn handle_start_run(request: &mut Request, state: &ServerState<'_>) -> HttpResponse {
    let input: StartRunInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    let task = match state.tasks.get(&input.task_id) {
        Ok(Some(task)) => task,
        Ok(None) => return not_found(),
        Err(error) => return internal_error(error),
    };
    // 发起时逐条取值：规格声明了哪些参数要运行时填，这里就必须恰好填哪些。
    if let Err(error) = check_run_params(&task.spec, &input.run_params) {
        return bad_request(error);
    }

    match start_run(
        state.config,
        state.config_path,
        &task,
        input.run_params,
        state.history,
        state.runs,
    ) {
        Ok(run_record_id) => json_response(202, &json!({ "run_record_id": run_record_id })),
        Err(StartRunError::AlreadyRunning) => json_response(
            409,
            &json!({ "error": { "message": "该任务以同一组运行参数已有 run 进行中" } }),
        ),
        Err(StartRunError::Internal(error)) => internal_error(error),
    }
}

fn check_run_params(spec: &TaskSpec, run_params: &RunParams) -> Result<(), String> {
    let declared: Vec<&str> = spec
        .runtime_parameters()
        .into_iter()
        .map(|condition| condition.parameter.as_str())
        .collect();
    for parameter in &declared {
        if !run_params.contains_key(*parameter) {
            return Err(format!("运行参数 {parameter} 未取值"));
        }
    }
    for parameter in run_params.keys() {
        if !declared.iter().any(|declared| declared == parameter) {
            return Err(format!("运行参数 {parameter} 不在任务定义里"));
        }
    }
    Ok(())
}

fn handle_cancel_run(runs: &RunRegistry, run_record_id: &str) -> HttpResponse {
    let runs = match runs.lock() {
        Ok(runs) => runs,
        Err(_) => return internal_error("run 控制锁已损坏".to_owned()),
    };
    let Some(run) = runs.active_runs.get(run_record_id) else {
        return not_found();
    };
    match run.stage.as_deref() {
        Some("COMMITTING") => json_response(
            409,
            &json!({ "error": { "message": "已过封口点，停不了" } }),
        ),
        Some("PREPARING" | "STREAMING") => {
            let Some(pid) = run.child_pid else {
                return internal_error("run 子进程尚未登记".to_owned());
            };
            match send_sigterm(pid) {
                Ok(()) => json_response(202, &json!({ "message": "已发送 SIGTERM" })),
                Err(error) => internal_error(error),
            }
        }
        _ => json_response(
            409,
            &json!({ "error": { "message": "run 尚未进入可取消阶段" } }),
        ),
    }
}

fn send_sigterm(pid: u32) -> Result<(), String> {
    let pid = i32::try_from(pid).map_err(|_| format!("run 子进程 PID 超出范围：{pid}"))?;
    // SAFETY: libc::kill has no memory-safety preconditions; pid fits the platform's pid_t.
    if unsafe { libc::kill(pid, SIGTERM) } == 0 {
        Ok(())
    } else {
        Err(format!(
            "向 run 子进程发送 SIGTERM 失败：{}",
            io::Error::last_os_error()
        ))
    }
}

fn handle_get_run(
    runs: &RunRegistry,
    history_store: &HistoryStore,
    run_record_id: &str,
) -> HttpResponse {
    let record = match runs.lock() {
        Ok(registry) => registry.live_histories.get(run_record_id).cloned(),
        Err(_) => return internal_error("run 投影锁已损坏".to_owned()),
    };
    if let Some(record) = record {
        return json_response(
            200,
            &json!({
                "run_record_id": run_record_id,
                "run_id": record.run_id,
                "run_params": record.run_params,
                "source_sql": record.source_sql,
                "staging_table": record.staging_table,
                "stage": record.stage,
                "seq": record.seq,
                "rows_pushed": record.rows_pushed,
                "bytes": record.bytes,
                "ms": record.ms,
                "last_ts": record.last_ts,
                "live": true,
            }),
        );
    }
    match history_store.get(run_record_id) {
        Ok(Some(history)) => history_response(&history),
        Ok(None) => not_found(),
        Err(error) => internal_error(error),
    }
}

fn handle_list_history(history_store: &HistoryStore, query: Option<&str>) -> HttpResponse {
    let mut task_id = None;
    for (key, value) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        if key.as_ref() == "task_id" {
            task_id = Some(value.into_owned());
        }
    }
    match history_store.list(task_id.as_deref()) {
        Ok(history) => json_response(200, &history),
        Err(error) => internal_error(error),
    }
}

fn history_response(history: &RunHistory) -> HttpResponse {
    let mut value =
        serde_json::to_value(history).expect("serializing a run history response must succeed");
    value
        .as_object_mut()
        .expect("run history serializes as an object")
        .insert("live".to_owned(), Value::Bool(false));
    json_response(200, &value)
}

fn start_run(
    config: &SourceConfig,
    config_path: &Path,
    task: &Task,
    run_params: RunParams,
    history_store: &HistoryStore,
    runs: &RunRegistry,
) -> Result<String, StartRunError> {
    let run_record_id = generate_run_record_id();
    // 历史里钉的是**当次实际执行**的语句文本（ADR-0036 §2）：规格以后改了它也不跟着变。
    let mut history = RunHistory::accepted(
        &run_record_id,
        &task.task_id,
        run_params.clone(),
        &task.spec.source_sql(),
        Utc::now(),
    );
    register_active_run(runs, &run_record_id, &task.task_id, &run_params)?;
    if let Err(error) = history_store.insert(&history, Utc::now(), config.history_retention_days) {
        remove_active_run(runs, &run_record_id);
        return Err(StartRunError::Internal(error));
    }
    runs.lock()
        .map_err(|_| StartRunError::Internal("run 投影锁已损坏".to_owned()))?
        .live_histories
        .insert(run_record_id.clone(), history.clone());

    let task_path = match materialize_task(config, task, &run_params, &run_record_id) {
        Ok(path) => path,
        Err(error) => {
            history.mark_parent_failure(error.clone(), Utc::now());
            let _ = history_store.save(&history, Utc::now(), config.history_retention_days);
            remove_live_history(runs, &run_record_id);
            remove_active_run(runs, &run_record_id);
            return Err(StartRunError::Internal(error));
        }
    };
    let mut child = match Command::new(&config.run_executable)
        .arg("--config")
        .arg(config_path)
        .arg("--task")
        .arg(&task_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = fs::remove_file(&task_path);
            let message = format!("启动 run 子进程失败：{error}");
            history.mark_parent_failure(message.clone(), Utc::now());
            let _ = history_store.save(&history, Utc::now(), config.history_retention_days);
            remove_live_history(runs, &run_record_id);
            remove_active_run(runs, &run_record_id);
            return Err(StartRunError::Internal(message));
        }
    };
    runs.lock()
        .map_err(|_| StartRunError::Internal("run 控制锁已损坏".to_owned()))?
        .active_runs
        .get_mut(&run_record_id)
        .expect("an active run remains registered until child reap")
        .child_pid = Some(child.id());
    let stdout = child
        .stdout
        .take()
        .expect("stdout is available after configuring it as piped");

    let worker_runs = Arc::clone(runs);
    let worker_record_id = run_record_id.clone();
    let worker_history_store = history_store.clone();
    let retention_days = config.history_retention_days;
    thread::spawn(move || {
        supervise_run(
            child,
            stdout,
            task_path,
            worker_record_id,
            worker_history_store,
            retention_days,
            worker_runs,
        )
    });
    Ok(run_record_id)
}

fn materialize_task(
    config: &SourceConfig,
    task: &Task,
    run_params: &RunParams,
    run_record_id: &str,
) -> Result<PathBuf, String> {
    let directory = config.data_dir.join(RUN_TASKS_DIRECTORY);
    fs::create_dir_all(&directory).map_err(|error| format!("创建临时任务目录失败：{error}"))?;
    let path = directory.join(format!("task-{run_record_id}.toml"));
    let task_config = task_config_from_task(task, run_params);
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

fn task_config_from_task(task: &Task, run_params: &RunParams) -> TaskConfig {
    TaskConfig {
        spec: task.spec.clone(),
        run_params: run_params.clone(),
    }
}

fn supervise_run(
    mut child: Child,
    stdout: impl Read,
    task_path: PathBuf,
    run_record_id: String,
    history_store: HistoryStore,
    retention_days: u64,
    runs: RunRegistry,
) {
    let mut terminal_observed = false;
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
        let Some((change, history)) = apply_log_line(&runs, &run_record_id, &log) else {
            continue;
        };
        let is_terminal = change == HistoryChange::Terminal;
        let requires_persistence = change != HistoryChange::MemoryOnly;
        terminal_observed |= is_terminal;
        if requires_persistence
            && history_store
                .save(&history, Utc::now(), retention_days)
                .is_err()
        {
            continue;
        }
        if is_terminal {
            remove_live_history(&runs, &run_record_id);
        }
    }
    let _ = child.wait();
    let _ = fs::remove_file(task_path);
    if !terminal_observed {
        let history = runs.lock().ok().and_then(|mut registry| {
            let history = registry.live_histories.get_mut(&run_record_id)?;
            history.mark_unknown(UnknownReason::ProcessDisappeared, Utc::now());
            Some(history.clone())
        });
        if let Some(history) = history {
            let _ = history_store.save(&history, Utc::now(), retention_days);
        }
    }
    remove_live_history(&runs, &run_record_id);
    remove_active_run(&runs, &run_record_id);
}

fn apply_log_line(
    runs: &RunRegistry,
    run_record_id: &str,
    log: &Value,
) -> Option<(HistoryChange, RunHistory)> {
    let Ok(mut registry) = runs.lock() else {
        return None;
    };
    let (change, history) = {
        let record = registry.live_histories.get_mut(run_record_id)?;
        let change = record.apply_log(log);
        (change, record.clone())
    };
    if change == HistoryChange::StageChanged {
        registry.active_runs.get_mut(run_record_id)?.stage = history.stage.clone();
    }
    Some((change, history))
}

fn register_active_run(
    runs: &RunRegistry,
    run_record_id: &str,
    task_id: &str,
    run_params: &RunParams,
) -> Result<(), StartRunError> {
    let mut runs = runs
        .lock()
        .map_err(|_| StartRunError::Internal("run 控制锁已损坏".to_owned()))?;
    if runs
        .active_runs
        .values()
        .any(|run| run.task_id == task_id && &run.run_params == run_params)
    {
        return Err(StartRunError::AlreadyRunning);
    }
    runs.active_runs.insert(
        run_record_id.to_owned(),
        ActiveRun {
            task_id: task_id.to_owned(),
            run_params: run_params.clone(),
            child_pid: None,
            stage: None,
        },
    );
    Ok(())
}

fn remove_active_run(runs: &RunRegistry, run_record_id: &str) {
    if let Ok(mut runs) = runs.lock() {
        runs.active_runs.remove(run_record_id);
    }
}

fn remove_live_history(runs: &RunRegistry, run_record_id: &str) {
    if let Ok(mut registry) = runs.lock() {
        registry.live_histories.remove(run_record_id);
    }
}

fn generate_run_record_id() -> String {
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

fn handle_list_tasks(store: &TaskStore) -> HttpResponse {
    match store.list() {
        Ok(tasks) => json_response(200, &tasks),
        Err(error) => internal_error(error),
    }
}

fn handle_create_task(request: &mut Request, store: &TaskStore) -> HttpResponse {
    let input: TaskInput = match read_json_body(request) {
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
    let input: TaskInput = match read_json_body(request) {
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

fn read_json_body<T: DeserializeOwned>(request: &mut Request) -> Result<T, String> {
    let mut body = Vec::new();
    request
        .as_reader()
        .take(MAX_REQUEST_BODY_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|error| format!("读取请求体失败：{error}"))?;
    if body.len() as u64 > MAX_REQUEST_BODY_BYTES {
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

fn embedded_response(body: Vec<u8>, content_type: &str, path: &str) -> HttpResponse {
    let content_type_header = Header::from_bytes("Content-Type", content_type)
        .expect("embedded content type must be valid");
    let request_path = path.split('?').next().unwrap_or(path);
    let cache_control_value = if matches!(request_path, "/" | "/index.html") {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    let cache_control_header = Header::from_bytes("Cache-Control", cache_control_value)
        .expect("static cache control must be valid");
    Response::from_data(body)
        .with_status_code(StatusCode(200))
        .with_header(content_type_header)
        .with_header(cache_control_header)
}

fn handle_builder_tables(request: &mut Request, config: &SourceConfig) -> HttpResponse {
    let input: BuilderLinkInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    if let Err(error) = validate_builder_dblink(input.dblink.as_deref()) {
        return bad_request(error);
    }
    match OracleRowSource::list_builder_tables(config, input.dblink.as_deref()) {
        Ok(tables) => json_response(200, &tables),
        Err(error) => oracle_failure(error),
    }
}

fn handle_builder_columns(request: &mut Request, config: &SourceConfig) -> HttpResponse {
    let input: BuilderColumnsInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    if input.owner.trim().is_empty() || input.table.trim().is_empty() {
        return bad_request("owner and table are required".to_owned());
    }
    if let Err(error) = validate_builder_dblink(input.dblink.as_deref()) {
        return bad_request(error);
    }
    match OracleRowSource::list_builder_columns(
        config,
        input.dblink.as_deref(),
        &input.owner,
        &input.table,
    ) {
        Ok(columns) => json_response(200, &columns),
        Err(error) => oracle_failure(error),
    }
}

/// 规格的派生面之一（ADR-0036 §6）：源端 SQL 与运行参数清单。
///
/// **只读**——v1 界面上没有编辑入口，所以这里只出不进：web 拿规格来换一份现算的 SQL 展示，
/// 不存在「web 改了 SQL 再传回来」这条路。
fn handle_builder_sql(request: &mut Request) -> HttpResponse {
    let spec: TaskSpec = match read_json_body(request) {
        Ok(spec) => spec,
        Err(error) => return bad_request(error),
    };
    if let Err(error) = spec.validate() {
        return bad_request(error);
    }
    let parameters = spec
        .runtime_parameters()
        .into_iter()
        .map(|condition| {
            json!({
                "parameter": condition.parameter,
                "column": condition.column,
                "value_type": condition.value_type,
            })
        })
        .collect::<Vec<_>>();
    json_response(
        200,
        &json!({ "source_sql": spec.source_sql(), "run_parameters": parameters }),
    )
}

fn oracle_failure(error: db_qbs_source::SourceReadError) -> HttpResponse {
    json_response(
        502,
        &json!({
            "kind": "oracle",
            "message": error.user_message(),
            "oracle_code": error.oracle_code,
            // 取列失败不是一次 run，进不了运行历史；分类仍照实给出，
            // 否则「连不上 Oracle」与「dblink 不可用」在这个面上又只能靠人话反推。
            "failure_kind": error.kind.as_str(),
        }),
    )
}

fn handle_column_fetch(request: &mut Request, config: &SourceConfig) -> HttpResponse {
    let mut body = String::new();
    if let Err(error) = request.as_reader().read_to_string(&mut body) {
        return json_response(
            400,
            &json!({ "kind": "request", "message": format!("could not read request: {error}") }),
        );
    }
    let input: ColumnFetchInput = match serde_json::from_str(&body) {
        Ok(input) => input,
        Err(error) => {
            return json_response(
                400,
                &json!({ "kind": "request", "message": format!("invalid JSON request: {error}") }),
            )
        }
    };
    if let Err(error) = input.spec.validate() {
        return json_response(400, &json!({ "kind": "request", "message": error }));
    }

    let columns = match OracleRowSource::describe(config, &input.spec) {
        Ok(columns) => columns,
        Err(error) => return oracle_failure(error),
    };
    match generate_target_ddl(
        &columns,
        &input.spec.target_table,
        &input.spec.primary_key,
        input.column_precision.as_ref(),
    ) {
        Ok(target_ddl) => json_response(
            200,
            &json!({ "columns": columns, "target_ddl": target_ddl }),
        ),
        Err(error) => {
            let message = error.to_string();
            let issues = error.columns;
            json_response(
                422,
                &json!({
                    "kind": "target_ddl",
                    "message": message,
                    "columns": issues,
                    "described_columns": columns,
                }),
            )
        }
    }
}
fn emit(level: LogLevel, event: LogEvent, fields: serde_json::Value) {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    let _ = write_log_line_with_fields(&mut writer, level, event, None, None, fields);
}
