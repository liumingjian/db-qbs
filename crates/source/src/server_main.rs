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
    ColumnPrecision, DatasourceInput, DatasourceStore, HistoryChange, HistoryStore, OracleAccess,
    OracleRowSource, RunHistory, RunParams, SourceConfig, TargetConnection, Task, TaskConfig,
    TaskInput, TaskSpec, TaskStore, UnknownReason,
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
    datasources: &'a DatasourceStore,
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
    /// 取哪个 Oracle 数据源的表清单（ADR-0037 §1）。构建器不再吃进程级凭据。
    datasource_id: String,
    dblink: Option<String>,
}

/// 草稿测连的请求体（ADR-0039 §3）：吃的是**表单里当前填的那组值**，不是库里存的那条。
///
/// `datasource_id` 只有编辑态才有，用途单一——口令留空时去库里取那一份
/// （与保存的「留空 = 不改」是同一条解释规则，两处不许分岔）。新建态没有它。
///
/// **不加 `deny_unknown_fields`**：`DatasourceInput` 内部用了 `flatten`，两者不兼容
/// （与 `DatasourceInput` 自己的注释同一条理由）。
#[derive(Deserialize)]
struct DatasourceTestInput {
    #[serde(default)]
    datasource_id: Option<String>,
    #[serde(flatten)]
    draft: DatasourceInput,
}

/// 目标端元数据面的两个代理入口（ADR-0038 §3）：界面只报**数据源 id**，
/// 凭据由 source 在这里解一次再过线——与「测试连接」同一条路径（ADR-0037 §1/§8）。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetMetadataInput {
    datasource_id: String,
    /// 只有取列面要它；取表清单不带。
    #[serde(default)]
    target_table: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuilderColumnsInput {
    datasource_id: String,
    dblink: Option<String>,
    owner: String,
    table: String,
}

/// 取列请求。`column_precision` **不在任务定义里**（ADR-0036 §6）：目标表 DDL 生成吃的是
/// describe 回来的源列，属「取列」链，这份精度提示随请求一起来、用完即弃。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ColumnFetchInput {
    datasource_id: String,
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
    let datasource_store = DatasourceStore::open(&config.data_dir)?;
    migrate_legacy_oracle_datasource(&config, &datasource_store)?;
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
                    "本服务无鉴权；能连上者等价于持有**全部已配置数据源**的凭据与写权限：可对任一源库跑任意 SQL 并清空重写任一目标表；运行历史含源库真实业务值；当前监听地址：{}",
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
        datasources: &datasource_store,
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

/// `source.toml` 里那三个已退役字段的一次性迁移（ADR-0037 §10）。
fn migrate_legacy_oracle_datasource(
    config: &SourceConfig,
    datasources: &DatasourceStore,
) -> Result<(), String> {
    let migrated = datasources.migrate_legacy_oracle(
        config.oracle_connect_string.as_deref(),
        config.oracle_username.as_deref(),
        config.oracle_password.as_deref(),
    )?;
    if let Some(datasource_id) = migrated {
        emit(
            LogLevel::Warn,
            LogEvent::SourceStarted,
            json!({
                "datasource_id": datasource_id,
                "message": "source.toml 的 oracle_connect_string / oracle_username / oracle_password 已退役（ADR-0037 §10），本次已迁成一条名为「默认」的 Oracle 数据源；请从配置文件里删掉这三个字段。oracle_client_lib_dir 不退役",
            }),
        );
    }
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
        return handle_column_fetch(request, state);
    }

    if method == Method::Post && path == "/api/builder/tables" {
        return handle_builder_tables(request, state);
    }

    if method == Method::Post && path == "/api/builder/columns" {
        return handle_builder_columns(request, state);
    }

    if path == "/api/datasources" {
        return match method {
            Method::Get => handle_list_datasources(state.datasources),
            Method::Post => handle_create_datasource(request, state.datasources),
            _ => not_found(),
        };
    }

    // 草稿测连必须排在按 id 的那条前面：`/api/datasources/test-connection` 里那截
    // `test-connection` 会被 `resource_id_from_path` 当成一个数据源 id 吃掉。
    if method == Method::Post && path == "/api/datasources/test-connection" {
        return handle_test_datasource_draft(request, state);
    }

    if method == Method::Post {
        if let Some(datasource_id) = test_connection_id_from_path(path) {
            return handle_test_datasource(state, datasource_id);
        }
    }

    if path.starts_with("/api/datasources/") {
        let Some(datasource_id) = resource_id_from_path(path, "/api/datasources/") else {
            return not_found();
        };
        return match method {
            Method::Get => handle_get_datasource(state.datasources, datasource_id),
            Method::Put => handle_update_datasource(request, state.datasources, datasource_id),
            Method::Delete => handle_delete_datasource(state, datasource_id),
            _ => not_found(),
        };
    }

    if method == Method::Post && path == "/api/target/tables" {
        return handle_target_tables(request, state);
    }

    if method == Method::Post && path == "/api/target/columns" {
        return handle_target_columns(request, state);
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

fn test_connection_id_from_path(path: &str) -> Option<&str> {
    let datasource_id = path
        .strip_prefix("/api/datasources/")?
        .strip_suffix("/test-connection")?;
    if datasource_id.is_empty() || datasource_id.contains('/') {
        return None;
    }
    Some(datasource_id)
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

    // 两端连接在发起时解一次（ADR-0037 §8）：解不开就当场拒，不要等到子进程起来了才炸。
    let access = match oracle_access(state, &task.source_datasource_id) {
        Ok(access) => access,
        Err(error) => return bad_request(error),
    };
    let target = match state
        .datasources
        .target_connection(&task.target_datasource_id)
    {
        Ok(target) => target,
        Err(error) => return bad_request(error),
    };

    match start_run(
        state.config,
        state.config_path,
        &task,
        access,
        target,
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
    access: OracleAccess,
    target: TargetConnection,
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

    let task_path =
        match materialize_task(config, task, access, target, &run_params, &run_record_id) {
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
    access: OracleAccess,
    target: TargetConnection,
    run_params: &RunParams,
    run_record_id: &str,
) -> Result<PathBuf, String> {
    let directory = config.data_dir.join(RUN_TASKS_DIRECTORY);
    fs::create_dir_all(&directory).map_err(|error| format!("创建临时任务目录失败：{error}"))?;
    let path = directory.join(format!("task-{run_record_id}.toml"));
    let task_config = task_config_from_task(task, access, target, run_params);
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

/// 临时任务文件里带着**解出来的明文凭据**（ADR-0037 §1/§8）。
/// 落盘形态由 [`materialize_task`] 保证：0600、跑完即删、启动与退出各扫一次。
fn task_config_from_task(
    task: &Task,
    access: OracleAccess,
    target: TargetConnection,
    run_params: &RunParams,
) -> TaskConfig {
    TaskConfig {
        spec: task.spec.clone(),
        oracle: access,
        target,
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

fn handle_list_datasources(store: &DatasourceStore) -> HttpResponse {
    match store.list() {
        // 只出 view：口令连密文都不回（ADR-0037 §5）。
        Ok(datasources) => json_response(
            200,
            &datasources
                .iter()
                .map(|datasource| datasource.view())
                .collect::<Vec<_>>(),
        ),
        Err(error) => internal_error(error),
    }
}

fn handle_create_datasource(request: &mut Request, store: &DatasourceStore) -> HttpResponse {
    let input: DatasourceInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    match store.create(input) {
        Ok(datasource) => json_response(201, &datasource.view()),
        Err(error) => bad_request(error),
    }
}

fn handle_get_datasource(store: &DatasourceStore, datasource_id: &str) -> HttpResponse {
    match store.get(datasource_id) {
        Ok(Some(datasource)) => json_response(200, &datasource.view()),
        Ok(None) => not_found(),
        Err(error) => internal_error(error),
    }
}

fn handle_update_datasource(
    request: &mut Request,
    store: &DatasourceStore,
    datasource_id: &str,
) -> HttpResponse {
    let input: DatasourceInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    match store.update(datasource_id, input) {
        Ok(Some(datasource)) => json_response(200, &datasource.view()),
        Ok(None) => not_found(),
        Err(error) => bad_request(error),
    }
}

/// 删数据源：还有任务引着就拒（ADR-0037 §7），措辞点名是哪几个任务——
/// 只说「有任务在用」会让用户挨个点开去找。
fn handle_delete_datasource(state: &ServerState<'_>, datasource_id: &str) -> HttpResponse {
    match state.tasks.names_referencing(datasource_id) {
        Ok(names) if !names.is_empty() => {
            return json_response(
                409,
                &json!({
                    "error": {
                        "message": format!("数据源仍被 {} 个任务引用：{}；请先改这些任务的数据源", names.len(), names.join("、")),
                        "tasks": names,
                    }
                }),
            )
        }
        Ok(_) => {}
        Err(error) => return internal_error(error),
    }
    match state.datasources.delete(datasource_id) {
        Ok(Some(datasource)) => json_response(200, &datasource.view()),
        Ok(None) => not_found(),
        Err(error) => internal_error(error),
    }
}

/// 草稿「测试连接」（ADR-0039 §3）：用**表单里当前填的值**测，不是库里存的那份。
///
/// 存在的理由是「测通才让存」这条门槛（所有者 2026-08-19 裁定 2）——新建的数据源
/// 库里根本还没有，按 id 测在新建态上无从谈起；改了口令的编辑态按 id 测的也是旧口令。
///
/// **不写任何存储**：解出来的连接用完即弃，与两个目标端元数据入口同一处置（ADR-0038 §3）。
/// 回报里带 `elapsed_ms` 与 `label`，`.inline-result` 那一行纯文字要用（ADR-0039 §3）。
fn handle_test_datasource_draft(request: &mut Request, state: &ServerState<'_>) -> HttpResponse {
    let input: DatasourceTestInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    let datasource_id = input.datasource_id.as_deref().filter(|id| !id.is_empty());
    let started = std::time::Instant::now();
    match &input.draft.settings {
        db_qbs_source::DatasourceSettings::Oracle { connect_string, .. } => {
            let label = connect_string.clone();
            let access = match state.datasources.draft_oracle_access(
                datasource_id,
                &input.draft.settings,
                &state.config.oracle_client_lib_dir,
            ) {
                Ok(access) => access,
                Err(error) => return bad_request(error),
            };
            match OracleRowSource::test_connection(&access) {
                Ok(()) => test_connection_ok(started, label),
                Err(error) => oracle_failure(error),
            }
        }
        db_qbs_source::DatasourceSettings::Mysql { database, .. } => {
            let label = database.clone();
            let target = match state
                .datasources
                .draft_target_connection(datasource_id, &input.draft.settings)
            {
                Ok(target) => target,
                Err(error) => return bad_request(error),
            };
            match test_target_connection(&state.config.sink_base_url, &target) {
                Ok(()) => test_connection_ok(started, label),
                Err(error) => json_response(502, &json!({ "kind": "sink", "message": error })),
            }
        }
    }
}

/// 成功那一格：耗时与库名/连接串一起回去，界面拼成「连接成功 · 186 ms · dw_stage」。
fn test_connection_ok(started: std::time::Instant, label: String) -> HttpResponse {
    json_response(
        200,
        &json!({
            "ok": true,
            "elapsed_ms": started.elapsed().as_millis() as u64,
            "label": label,
        }),
    )
}

/// 「测试连接」（ADR-0037 §9）：Oracle 在本机直连，MySQL 走 sink 的新端点——
/// source 仍不建 MySQL 连接，`CONTEXT.md` 那条不对称在这一点上保留。
fn handle_test_datasource(state: &ServerState<'_>, datasource_id: &str) -> HttpResponse {
    let datasource = match state.datasources.get(datasource_id) {
        Ok(Some(datasource)) => datasource,
        Ok(None) => return not_found(),
        Err(error) => return internal_error(error),
    };
    match datasource.settings {
        db_qbs_source::DatasourceSettings::Oracle { .. } => {
            let access = match oracle_access(state, datasource_id) {
                Ok(access) => access,
                Err(error) => return bad_request(error),
            };
            match OracleRowSource::test_connection(&access) {
                Ok(()) => json_response(200, &json!({ "ok": true })),
                Err(error) => oracle_failure(error),
            }
        }
        db_qbs_source::DatasourceSettings::Mysql { .. } => {
            let target = match state.datasources.target_connection(datasource_id) {
                Ok(target) => target,
                Err(error) => return bad_request(error),
            };
            match test_target_connection(&state.config.sink_base_url, &target) {
                Ok(()) => json_response(200, &json!({ "ok": true })),
                Err(error) => json_response(502, &json!({ "kind": "sink", "message": error })),
            }
        }
    }
}

/// 目标库的表清单（ADR-0038 §3）。source 仍**不建 MySQL 连接**——查询在 sink 那侧跑，
/// `CONTEXT.md` 那条不对称原样保留。结果纯瞬态：不进任务定义、不进 SQLite（ADR-0038 §8）。
fn handle_target_tables(request: &mut Request, state: &ServerState<'_>) -> HttpResponse {
    let input: TargetMetadataInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    let target = match state.datasources.target_connection(&input.datasource_id) {
        Ok(target) => target,
        Err(error) => return bad_request(error),
    };
    match post_to_sink(
        &state.config.sink_base_url,
        "/v1/target/tables",
        &serde_json::to_value(&target).expect("target connection must serialize"),
    ) {
        Ok(body) => json_response(200, &body),
        Err(error) => json_response(502, &json!({ "kind": "sink", "message": error })),
    }
}

/// 一张目标表的列清单与唯一性约束（ADR-0038 §3）。**表不存在回空清单，不是错误**（§9）。
fn handle_target_columns(request: &mut Request, state: &ServerState<'_>) -> HttpResponse {
    let input: TargetMetadataInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    if input.target_table.trim().is_empty() {
        return bad_request("target_table 不能为空".to_owned());
    }
    let target = match state.datasources.target_connection(&input.datasource_id) {
        Ok(target) => target,
        Err(error) => return bad_request(error),
    };
    match post_to_sink(
        &state.config.sink_base_url,
        "/v1/target/columns",
        &json!({ "target": target, "target_table": input.target_table }),
    ) {
        Ok(body) => json_response(200, &body),
        Err(error) => json_response(502, &json!({ "kind": "sink", "message": error })),
    }
}

/// 往 sink 发一个「不属于任何 run」的元数据请求，把 JSON 回话原样带回来。
///
/// 与 [`test_target_connection`] 同一条通道、同一条部署前提（ADR-0037 §4：通道必须可信），
/// 只是多一个「把响应体读回来」——`test-connection` 只关心成没成。
fn post_to_sink(sink_base_url: &str, path: &str, body: &Value) -> Result<Value, String> {
    let url = format!("{}{path}", sink_base_url.trim_end_matches('/'));
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .redirects(0)
        .build();
    match agent.post(&url).send_json(body.clone()) {
        Ok(response) => response
            .into_json::<Value>()
            .map_err(|error| format!("目标端回话不是 JSON：{error}")),
        Err(ureq::Error::Status(_, response)) => Err(response
            .into_json::<Value>()
            .ok()
            .and_then(|body| {
                body.get("error")?
                    .get("message")?
                    .as_str()
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "目标端拒绝了元数据请求".to_owned())),
        Err(ureq::Error::Transport(error)) => Err(format!("连不上 sink：{error}")),
    }
}

/// 把连接信息交给 sink 试一把。**口令在这里过线**——这是 ADR-0037 §1 认下的那条路径，
/// 与发起运行走同一个通道、同一条部署前提（§4：通道必须可信）。
fn test_target_connection(sink_base_url: &str, target: &TargetConnection) -> Result<(), String> {
    let url = format!(
        "{}/v1/target/test-connection",
        sink_base_url.trim_end_matches('/')
    );
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .redirects(0)
        .build();
    match agent
        .post(&url)
        .send_json(serde_json::to_value(target).expect("target connection must serialize"))
    {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(_, response)) => Err(response
            .into_json::<Value>()
            .ok()
            .and_then(|body| {
                body.get("error")?
                    .get("message")?
                    .as_str()
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "目标端拒绝了测试连接请求".to_owned())),
        Err(ureq::Error::Transport(error)) => Err(format!("连不上 sink：{error}")),
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

/// 解出某个 Oracle 数据源的连接信息：数据源那三样 + 进程级的 client 库目录（ADR-0037 §6）。
fn oracle_access(state: &ServerState<'_>, datasource_id: &str) -> Result<OracleAccess, String> {
    state
        .datasources
        .oracle_access(datasource_id, &state.config.oracle_client_lib_dir)
}

fn handle_builder_tables(request: &mut Request, state: &ServerState<'_>) -> HttpResponse {
    let input: BuilderLinkInput = match read_json_body(request) {
        Ok(input) => input,
        Err(error) => return bad_request(error),
    };
    if let Err(error) = validate_builder_dblink(input.dblink.as_deref()) {
        return bad_request(error);
    }
    let access = match oracle_access(state, &input.datasource_id) {
        Ok(access) => access,
        Err(error) => return bad_request(error),
    };
    match OracleRowSource::list_builder_tables(&access, input.dblink.as_deref()) {
        Ok(tables) => json_response(200, &tables),
        Err(error) => oracle_failure(error),
    }
}

fn handle_builder_columns(request: &mut Request, state: &ServerState<'_>) -> HttpResponse {
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
    let access = match oracle_access(state, &input.datasource_id) {
        Ok(access) => access,
        Err(error) => return bad_request(error),
    };
    match OracleRowSource::list_builder_columns(
        &access,
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

fn handle_column_fetch(request: &mut Request, state: &ServerState<'_>) -> HttpResponse {
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

    let access = match oracle_access(state, &input.datasource_id) {
        Ok(access) => access,
        Err(error) => return json_response(400, &json!({ "kind": "request", "message": error })),
    };
    let columns = match OracleRowSource::describe(&access, &input.spec) {
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
