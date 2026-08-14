use std::io::Read;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use crate::{ApiError, Destination, MysqlDestination, OpenRunRequest, SinkConfig, SinkService};

const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;

pub fn serve(config: SinkConfig) -> Result<(), String> {
    if config.listen.is_empty() {
        return Err("sink 配置 listen 不能为空".to_owned());
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
        match path
            .strip_prefix("/v1/runs/")
            .and_then(|path| path.strip_suffix("/abort"))
            .filter(|run_id| !run_id.is_empty() && !run_id.contains('/'))
        {
            Some(run_id) => handle_abort(&mut request, service, run_id),
            None => error_response(not_found()),
        }
    } else {
        error_response(not_found())
    };

    if let Err(error) = request.respond(response) {
        eprintln!("HTTP 响应写入失败：{error}");
    }
}

fn handle_open(
    request: &mut Request,
    service: &SinkService<impl Destination>,
) -> Response<std::io::Cursor<Vec<u8>>> {
    if !has_json_content_type(request) {
        return error_response(ApiError {
            status: 415,
            code: "BAD_REQUEST",
            message: "Content-Type 必须是 application/json".to_owned(),
            run_id: None,
            details: json!({}),
        });
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

fn handle_abort(
    request: &mut Request,
    service: &SinkService<impl Destination>,
    run_id: &str,
) -> Response<std::io::Cursor<Vec<u8>>> {
    if !has_json_content_type(request) {
        return error_response(ApiError {
            status: 415,
            code: "BAD_REQUEST",
            message: "Content-Type 必须是 application/json".to_owned(),
            run_id: Some(run_id.to_owned()),
            details: json!({}),
        });
    }
    if let Err(error) = read_json::<EmptyBody>(request) {
        return error_response(error);
    }
    match service.abort(run_id) {
        Ok(response) => json_response(200, &response),
        Err(error) => error_response(error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyBody {}

fn read_json<T: for<'de> Deserialize<'de>>(request: &mut Request) -> Result<T, ApiError> {
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

fn error_response(error: ApiError) -> Response<std::io::Cursor<Vec<u8>>> {
    let status = error.status;
    json_response(status, &json!({ "error": error }))
}

fn json_response(status: u16, value: &impl serde::Serialize) -> Response<std::io::Cursor<Vec<u8>>> {
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
    use crate::{CreateStagingError, DropStagingError, TargetColumn};

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
                nullable: true,
                character_set: None,
                ordinal: 1,
            }])
        }

        fn create_staging(
            &self,
            _staging_table: &str,
            _ddl: &str,
        ) -> Result<(), CreateStagingError> {
            Ok(())
        }

        fn drop_staging(&self, staging_table: &str) -> Result<(), DropStagingError> {
            self.dropped.lock().unwrap().push(staging_table.to_owned());
            Ok(())
        }
    }

    #[test]
    fn http_open_and_abort_lifecycle_uses_contract_statuses_and_bodies() {
        let service = Arc::new(SinkService::new(
            "qbs",
            Arc::new(FakeDestination::default()),
        ));
        let run_id = "20260814091530_a3f19c";
        let open_body = format!(
            r#"{{"run_id":"{run_id}","target_table":"T_POSITION","target_date_col":"D_BIZ","biz_date":"2026-08-14","source_columns":[{{"name":"D_BIZ","type":"DATE","precision":null,"scale":null,"length":null}}]}}"#
        );

        let (status, opened) = exchange(service.clone(), "/v1/runs", &open_body);
        assert_eq!(status, 200);
        assert_eq!(opened["run_id"], run_id);

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

    fn exchange(
        service: Arc<SinkService<FakeDestination>>,
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
            "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
