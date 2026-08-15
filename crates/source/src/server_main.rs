use std::env;
use std::io::{self, Read};
use std::net::ToSocketAddrs;
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use db_qbs_shared::{write_log_line_with_fields, LogEvent, LogLevel};
use db_qbs_source::{load_source_config, SourceConfig, TaskInput, TaskStore};
use serde::Serialize;
use serde_json::json;
use signal_hook::consts::SIGTERM;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const MAX_TASK_BODY_BYTES: u64 = 1024 * 1024;

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
    serve(config)
}

fn parse_config_path(mut args: impl Iterator<Item = String>) -> Result<String, String> {
    match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--config"), Some(path), None) => Ok(path),
        _ => Err("用法：db-qbs-source --config <source.toml>".to_owned()),
    }
}

fn serve(config: SourceConfig) -> Result<(), String> {
    if config.listen.is_empty() {
        return Err("source 配置 listen 不能为空".to_owned());
    }

    let store = TaskStore::open(&config.data_dir)?;
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
            handle_request(request, &store);
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

fn handle_request(mut request: Request, store: &TaskStore) {
    let response = route_request(&mut request, store);

    if let Err(error) = request.respond(response) {
        emit(
            LogLevel::Error,
            LogEvent::HttpResponseFailed,
            json!({ "message": format!("HTTP 响应写入失败：{error}") }),
        );
    }
}

fn route_request(request: &mut Request, store: &TaskStore) -> HttpResponse {
    let method = request.method().clone();
    let path = request.url().to_owned();

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
    let mut body = Vec::new();
    request
        .as_reader()
        .take(MAX_TASK_BODY_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|error| format!("读取请求体失败：{error}"))?;
    if body.len() as u64 > MAX_TASK_BODY_BYTES {
        return Err("任务定义请求体超过 1 MiB".to_owned());
    }
    serde_json::from_slice(&body).map_err(|error| format!("JSON 任务定义无效：{error}"))
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

fn emit(level: LogLevel, event: LogEvent, fields: serde_json::Value) {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    let _ = write_log_line_with_fields(&mut writer, level, event, None, None, fields);
}
