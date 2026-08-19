use std::io::{self, Cursor, Read};
use std::sync::Arc;

use db_qbs_shared::{write_log_line_with_fields, LogEvent, LogLevel};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use crate::{
    ApiError, BatchPayload, CommitRequest, Destination, MysqlDestination, OpenRunRequest,
    SinkConfig, SinkService,
};

const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;

type HttpResponse = Response<Cursor<Vec<u8>>>;

pub fn serve(config: SinkConfig) -> Result<(), String> {
    if config.listen.is_empty() {
        return Err("sink 配置 listen 不能为空".to_owned());
    }
    {
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        let _ = write_log_line_with_fields(
            &mut writer,
            LogLevel::Warn,
            LogEvent::SinkStarted,
            None,
            None,
            json!({
                "listen": &config.listen,
                "message": format!(
                    "本服务无鉴权，能连上者可清空并重写任意暂存表与目标表；当前监听地址：{}",
                    config.listen
                ),
            }),
        );
    }
    let destination = Arc::new(MysqlDestination::new(&config)?);
    let service = Arc::new(SinkService::new(config.database.clone(), destination));
    let server = Server::http(&config.listen)
        .map_err(|error| format!("监听 {} 失败：{error}", config.listen))?;

    for request in server.incoming_requests() {
        handle_request(request, &service);
    }
    Ok(())
}

fn handle_request<D: Destination>(mut request: Request, service: &SinkService<D>) {
    let method = request.method().clone();
    let path = request.url().to_owned();
    let response = if method == Method::Post && path == "/v1/runs" {
        handle_open(&mut request, service)
    } else if method == Method::Post {
        match run_action(&path) {
            Some((run_id, "batches")) => handle_batch(&mut request, service, run_id),
            Some((run_id, "commit")) => handle_commit(&mut request, service, run_id),
            Some((run_id, "abort")) => handle_abort(&mut request, service, run_id),
            _ => error_response(not_found()),
        }
    } else if method == Method::Get {
        match run_resource(&path) {
            Some(run_id) => handle_get(service, run_id),
            None => error_response(not_found()),
        }
    } else {
        error_response(not_found())
    };

    if let Err(error) = request.respond(response) {
        let run_id = run_resource(&path).or_else(|| run_action(&path).map(|(run_id, _)| run_id));
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        let _ = write_log_line_with_fields(
            &mut writer,
            LogLevel::Error,
            LogEvent::HttpResponseFailed,
            run_id,
            None,
            json!({ "message": format!("HTTP 响应写入失败：{error}") }),
        );
    }
}

fn run_resource(path: &str) -> Option<&str> {
    let run_id = path.strip_prefix("/v1/runs/")?;
    if run_id.is_empty() || run_id.contains('/') {
        return None;
    }
    Some(run_id)
}

fn run_action(path: &str) -> Option<(&str, &str)> {
    let path = path.strip_prefix("/v1/runs/")?;
    let (run_id, action) = path.split_once('/')?;
    if run_id.is_empty() || action.is_empty() || action.contains('/') {
        return None;
    }
    Some((run_id, action))
}

fn handle_open(request: &mut Request, service: &SinkService<impl Destination>) -> HttpResponse {
    if !has_json_content_type(request) {
        return error_response(unsupported_media_type(None));
    }
    let request: OpenRunRequest = match read_json(request) {
        Ok(request) => request,
        Err(error) => return error_response(error),
    };
    match service.open(request) {
        Ok(response) => json_response(200, &response),
        Err(error) => error_response(error),
    }
}

fn handle_batch(
    request: &mut Request,
    service: &SinkService<impl Destination>,
    run_id: &str,
) -> HttpResponse {
    let payload: BatchPayload = match read_run_json(request, run_id) {
        Ok(payload) => payload,
        Err(error) => return error_response(error),
    };
    match service.write_batch(run_id, payload) {
        Ok(response) => json_response(200, &response),
        Err(error) => error_response(error),
    }
}

fn handle_abort(
    request: &mut Request,
    service: &SinkService<impl Destination>,
    run_id: &str,
) -> HttpResponse {
    if !has_json_content_type(request) {
        return error_response(unsupported_media_type(Some(run_id)));
    }
    if let Err(error) = read_json::<EmptyBody>(request) {
        return error_response(error);
    }
    match service.abort(run_id) {
        Ok(response) => json_response(200, &response),
        Err(error) => error_response(error),
    }
}

fn handle_commit(
    request: &mut Request,
    service: &SinkService<impl Destination>,
    run_id: &str,
) -> HttpResponse {
    let payload: CommitRequest = match read_run_json(request, run_id) {
        Ok(payload) => payload,
        Err(error) => return error_response(error),
    };
    match service.commit(run_id, payload.total_batches, payload.total_rows) {
        Ok(response) => json_response(200, &response),
        Err(error) => error_response(error),
    }
}

fn handle_get(service: &SinkService<impl Destination>, run_id: &str) -> HttpResponse {
    match service.get(run_id) {
        Ok(response) => json_response(200, &response),
        Err(error) => error_response(error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyBody {}

fn read_run_json<T: DeserializeOwned>(request: &mut Request, run_id: &str) -> Result<T, ApiError> {
    if !has_json_content_type(request) {
        return Err(unsupported_media_type(Some(run_id)));
    }
    read_json(request).map_err(|mut error| {
        error.run_id = Some(run_id.to_owned());
        error
    })
}

fn read_json<T: DeserializeOwned>(request: &mut Request) -> Result<T, ApiError> {
    let mut body = Vec::new();
    request
        .as_reader()
        .take(MAX_BODY_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|error| bad_request(format!("读取请求体失败：{error}")))?;
    if body.len() as u64 > MAX_BODY_BYTES {
        return Err(ApiError {
            status: 413,
            code: "PAYLOAD_TOO_LARGE",
            message:
                "请求体超过 64 MiB 断路器；这是批次预算逻辑缺陷，不是数据或环境问题，请报 issue"
                    .to_owned(),
            run_id: None,
            details: json!({ "max_bytes": MAX_BODY_BYTES }),
        });
    }
    serde_json::from_slice(&body).map_err(|error| bad_request(format!("JSON 请求体无效：{error}")))
}

fn has_json_content_type(request: &Request) -> bool {
    request.headers().iter().any(|header| {
        header.field.equiv("Content-Type")
            && header
                .value
                .as_str()
                .eq_ignore_ascii_case("application/json")
    })
}

fn unsupported_media_type(run_id: Option<&str>) -> ApiError {
    ApiError {
        status: 415,
        code: "BAD_REQUEST",
        message: "Content-Type 必须是 application/json".to_owned(),
        run_id: run_id.map(str::to_owned),
        details: json!({}),
    }
}

fn bad_request(message: String) -> ApiError {
    ApiError {
        status: 400,
        code: "BAD_REQUEST",
        message,
        run_id: None,
        details: json!({}),
    }
}

fn not_found() -> ApiError {
    ApiError {
        status: 404,
        code: "RUN_UNKNOWN",
        message: "请求的 sink v1 资源不存在".to_owned(),
        run_id: None,
        details: json!({}),
    }
}

fn error_response(error: ApiError) -> HttpResponse {
    let status = error.status;
    json_response(status, &error.into_envelope())
}

fn json_response(status: u16, value: &impl Serialize) -> HttpResponse {
    let body = serde_json::to_vec(value).expect("serializing an HTTP response must succeed");
    let content_type = Header::from_bytes("Content-Type", "application/json")
        .expect("static response header must be valid");
    Response::from_data(body)
        .with_status_code(StatusCode(status))
        .with_header(content_type)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::{Arc, Mutex};
    use std::thread;

    use serde_json::Value;

    use super::*;
    use crate::{
        AtomicSwapError, AtomicSwapRequest, AtomicSwapResult, CreateStagingError, DropStagingError,
        TargetColumn, TargetKey, WriteBatchError,
    };

    #[derive(Default)]
    struct FakeDestination {
        dropped: Mutex<Vec<String>>,
    }

    impl Destination for FakeDestination {
        fn target_columns(&self, _target_table: &str) -> Result<Vec<TargetColumn>, String> {
            Ok(vec![TargetColumn {
                name: "D_BIZ".to_owned(),
                column_type: "datetime".to_owned(),
                data_type: "datetime".to_owned(),
                precision: None,
                scale: None,
                length: None,
                datetime_precision: Some(0),
                // 主键列必须 NOT NULL（ADR-0035 §2 第 3 条）。
                nullable: false,
                character_set: None,
                ordinal: 1,
            }])
        }

        fn target_keys(&self, _target_table: &str) -> Result<Vec<TargetKey>, String> {
            Ok(vec![TargetKey {
                name: "PRIMARY".to_owned(),
                columns: vec!["D_BIZ".to_owned()],
            }])
        }

        fn create_staging(
            &self,
            _staging_table: &str,
            _ddl: &str,
        ) -> Result<(), CreateStagingError> {
            Ok(())
        }

        fn write_batch(
            &self,
            _staging_table: &str,
            _columns: &[String],
            rows: &[Vec<Option<String>>],
            _max_rows_per_insert: usize,
        ) -> Result<u64, WriteBatchError> {
            Ok(rows.len() as u64)
        }

        fn atomic_swap(
            &self,
            request: &AtomicSwapRequest,
        ) -> Result<AtomicSwapResult, AtomicSwapError> {
            Ok(AtomicSwapResult {
                staged_rows: request.source_rows,
                purged_rows: 0,
                swapped_rows: request.source_rows,
                count_ms: 0,
            })
        }

        fn drop_staging(&self, staging_table: &str) -> Result<(), DropStagingError> {
            self.dropped.lock().unwrap().push(staging_table.to_owned());
            Ok(())
        }
    }

    #[test]
    fn run_action_requires_exactly_one_run_and_action_segment() {
        assert_eq!(run_action("/v1/runs/run/batches"), Some(("run", "batches")));
        assert_eq!(run_action("/v1/runs/run/abort"), Some(("run", "abort")));
        assert_eq!(run_action("/v1/runs/run/commit"), Some(("run", "commit")));
        assert_eq!(run_action("/v1/runs//batches"), None);
        assert_eq!(run_action("/v1/runs/run/"), None);
        assert_eq!(run_action("/v1/runs/run/batches/extra"), None);
        assert_eq!(run_action("/runs/run/batches"), None);
    }

    #[test]
    fn run_resource_requires_exactly_one_run_segment() {
        assert_eq!(run_resource("/v1/runs/run"), Some("run"));
        assert_eq!(run_resource("/v1/runs/"), None);
        assert_eq!(run_resource("/v1/runs/run/extra"), None);
        assert_eq!(run_resource("/runs/run"), None);
    }

    #[test]
    fn http_open_batch_and_abort_lifecycle_uses_contract_statuses_and_bodies() {
        let service = Arc::new(SinkService::new(
            "qbs",
            Arc::new(FakeDestination::default()),
        ));
        let run_id = "20260814091530_a3f19c";
        let open_body = format!(
            r#"{{"run_id":"{run_id}","target_table":"T_POSITION","primary_key":["D_BIZ"],"source_columns":[{{"name":"D_BIZ","type":"DATE","precision":null,"scale":null,"length":null}}]}}"#
        );

        let (status, opened) = exchange(service.clone(), "/v1/runs", &open_body);
        assert_eq!(status, 200);
        assert_eq!(opened["run_id"], run_id);

        let batch_path = format!("/v1/runs/{run_id}/batches");
        let (status, batch) =
            exchange(service.clone(), &batch_path, r#"{"seq":1,"rows":[[null]]}"#);
        assert_eq!(status, 200);
        assert_eq!(batch["seq"], 1);
        assert_eq!(batch["rows_written"], 1);
        assert_eq!(batch["next_seq"], 2);

        let abort_path = format!("/v1/runs/{run_id}/abort");
        let (status, aborted) = exchange(service.clone(), &abort_path, "{}");
        assert_eq!(status, 200);
        assert_eq!(aborted["staging_dropped"], true);

        let (status, repeated) = exchange(service.clone(), &abort_path, "{}");
        assert_eq!(status, 200);
        assert_eq!(repeated["staging_dropped"], false);

        let (status, unknown) = exchange(service, "/v1/runs/20260814091531_b4e20d/abort", "{}");
        assert_eq!(status, 200);
        assert_eq!(unknown["staging_dropped"], false);
    }

    #[test]
    fn http_commit_and_get_expose_the_terminal_resource() {
        let service = Arc::new(SinkService::new(
            "qbs",
            Arc::new(FakeDestination::default()),
        ));
        let run_id = "20260814091530_a3f19c";
        let open_body = format!(
            r#"{{"run_id":"{run_id}","target_table":"T_POSITION","primary_key":["D_BIZ"],"source_columns":[{{"name":"D_BIZ","type":"DATE","precision":null,"scale":null,"length":null}}]}}"#
        );
        exchange(service.clone(), "/v1/runs", &open_body);
        exchange(
            service.clone(),
            &format!("/v1/runs/{run_id}/batches"),
            r#"{"seq":1,"rows":[["2026-08-14 12:00:00"]]}"#,
        );

        let (status, committed) = exchange(
            service.clone(),
            &format!("/v1/runs/{run_id}/commit"),
            r#"{"total_batches":1,"total_rows":1}"#,
        );
        assert_eq!(status, 200);
        assert_eq!(committed["source_rows"], 1);
        assert_eq!(committed["swapped_rows"], 1);
        assert_eq!(committed["count_ms"], 0);

        let (status, terminal) =
            exchange_method(service.clone(), "GET", &format!("/v1/runs/{run_id}"), "");
        assert_eq!(status, 200);
        assert_eq!(terminal["terminal"], "SWAPPED");

        let (status, unknown) =
            exchange_method(service, "GET", "/v1/runs/20260814091531_b4e20d", "");
        assert_eq!(status, 404);
        assert_eq!(unknown["error"]["code"], "RUN_UNKNOWN");
    }

    fn exchange(
        service: Arc<SinkService<FakeDestination>>,
        path: &str,
        body: &str,
    ) -> (u16, Value) {
        exchange_method(service, "POST", path, body)
    }

    fn exchange_method(
        service: Arc<SinkService<FakeDestination>>,
        method: &str,
        path: &str,
        body: &str,
    ) -> (u16, Value) {
        let server = Server::http("127.0.0.1:0").unwrap();
        let address = server.server_addr().to_ip().unwrap();
        let worker = thread::spawn(move || {
            let request = server.recv().unwrap();
            handle_request(request, &service);
        });

        let mut stream = TcpStream::connect(address).unwrap();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut raw_response = String::new();
        stream.read_to_string(&mut raw_response).unwrap();
        worker.join().unwrap();

        let (head, body) = raw_response.split_once("\r\n\r\n").unwrap();
        let status = head
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        (status, serde_json::from_str(body).unwrap())
    }
}
