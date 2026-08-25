use std::io::{self, Cursor, Read};
use std::sync::Arc;

use db_qbs_shared::{write_log_line_with_fields, AgentInfo, LogEvent, LogLevel};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use crate::{
    load_agent_identity, ApiError, BatchPayload, CommitRequest, Destination, DestinationFactory,
    MysqlDestination, MysqlFactory, OpenRunRequest, SinkConfig, SinkService, TargetConnection,
    relaxed_precheck_enabled,
};

const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;

type HttpResponse = Response<Cursor<Vec<u8>>>;

pub fn serve(config: SinkConfig) -> Result<(), String> {
    if config.listen.is_empty() {
        return Err("sink 配置 listen 不能为空".to_owned());
    }
    let relaxed_precheck = relaxed_precheck_enabled();
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
                "relaxed_precheck": relaxed_precheck,
                "message": format!(
                    "{}；当前监听地址：{}",
                    if relaxed_precheck {
                        "POC 宽松预检查已开启，映射类型、可空性、唯一键和值域检查不会拦截运行"
                    } else {
                        "本服务无鉴权，能连上者可用调用方给的凭据清空并重写任意暂存表与目标表"
                    },
                    config.listen,
                ),
            }),
        );
    }
    // 退役字段仍能解析，但一个字都不读（ADR-0037 §2）。留一条 warn，
    // 否则部署者会以为 `sink.toml` 里那份凭据仍然是生效的那一份。
    if config.mysql_dsn.is_some() || config.database.is_some() {
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        let _ = write_log_line_with_fields(
            &mut writer,
            LogLevel::Warn,
            LogEvent::SinkStarted,
            None,
            None,
            json!({
                "message": "sink.toml 的 mysql_dsn / database 已退役且不再被读取（ADR-0037 §2）：目标端凭据随每个 run 的请求过线，请从配置文件里删掉这两个字段",
            }),
        );
    }
    // 身份先于监听：起不来就别开门（ADR-0044 §2）。id 文件写不下去时 source 那侧的
    // 「注册」会在下一次重启后认到另一个身份，那正是本票要挡的静默——所以这里硬失败。
    let agent = Arc::new(load_agent_identity(
        &config.agent_id_path(),
        config.agent_name.as_deref(),
    )?);
    {
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        let _ = write_log_line_with_fields(
            &mut writer,
            LogLevel::Info,
            LogEvent::SinkStarted,
            None,
            None,
            json!({
                "agent_id": &agent.agent_id,
                "agent_name": &agent.name,
                "version": &agent.version,
                "message": "本进程即目标端 agent；请在 source 的「目标端 Agent」屏用这个地址注册它（ADR-0044 §3）",
            }),
        );
    }
    // sink 启动**不再连 MySQL**：连接按 run 建，连不上的失败点在 POST /v1/runs。
    let service = Arc::new(SinkService::with_factory(MysqlFactory));
    let server = Server::http(&config.listen)
        .map_err(|error| format!("监听 {} 失败：{error}", config.listen))?;

    for request in server.incoming_requests() {
        handle_request(request, &service, &agent);
    }
    Ok(())
}

fn handle_request<F: DestinationFactory>(
    mut request: Request,
    service: &SinkService<F>,
    agent: &AgentInfo,
) {
    let method = request.method().clone();
    let path = request.url().to_owned();
    let response = if method == Method::Get && path == "/v1/agent/info" {
        json_response(200, agent)
    } else if method == Method::Post && path == "/v1/runs" {
        handle_open(&mut request, service)
    } else if method == Method::Post && path == "/v1/target/test-connection" {
        handle_test_connection(&mut request)
    } else if method == Method::Post && path == "/v1/target/tables" {
        handle_target_tables(&mut request)
    } else if method == Method::Post && path == "/v1/target/columns" {
        handle_target_columns(&mut request)
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

fn handle_open(
    request: &mut Request,
    service: &SinkService<impl DestinationFactory>,
) -> HttpResponse {
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
    service: &SinkService<impl DestinationFactory>,
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
    service: &SinkService<impl DestinationFactory>,
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
    service: &SinkService<impl DestinationFactory>,
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

fn handle_get(service: &SinkService<impl DestinationFactory>, run_id: &str) -> HttpResponse {
    match service.get(run_id) {
        Ok(response) => json_response(200, &response),
        Err(error) => error_response(error),
    }
}

/// 「测试连接」（ADR-0037 §9）——**不属于任何 run**，所以它不进 run 注册表、
/// 不留 tombstone，也不需要服务实例。source 侧的数据源管理面靠它验 MySQL 那一侧。
fn handle_test_connection(request: &mut Request) -> HttpResponse {
    if !has_json_content_type(request) {
        return error_response(unsupported_media_type(None));
    }
    let target: TargetConnection = match read_json(request) {
        Ok(target) => target,
        Err(error) => return error_response(error),
    };
    match MysqlDestination::test_connection(&target) {
        Ok(()) => json_response(200, &json!({ "ok": true })),
        // 码闭集不增：连不上是目标端环境故障（ADR-0037 §9）。
        Err(message) => error_response(ApiError {
            status: 500,
            code: "SINK_ENVIRONMENT",
            message: format!("连接目标端失败：{message}"),
            run_id: None,
            details: json!({ "kind": "OTHER" }),
        }),
    }
}

/// `POST /v1/target/columns` 的请求体。
///
/// 连接**嵌在 `target` 里**，不 flatten 进顶层：`OpenRunRequest` 已经是这个形状，
/// 而 serde 的 `flatten` 与 `deny_unknown_fields` 不能共存——拼字段名的错就会静默通过。
/// `/v1/target/tables` 没有第二个字段，所以它原样收一个 `TargetConnection`，
/// 与 `/v1/target/test-connection` 一致。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetColumnsRequest {
    target: TargetConnection,
    target_table: String,
}

/// 目标端元数据面（ADR-0038 §3）：ADR-0027 §3 那道封条到这里完整解除。
///
/// 与 `test-connection` 同属「不属于任何 run 的端点」——**不产生 `run_id`、不进 run 注册表、
/// 不留 tombstone、不写任何存储**，连接按请求建、用完即弃（`MysqlDestination` 出作用域即断）。
/// 它喂的是**选择面**，不是判定面：拦截层仍然只有映射预检一处（ADR-0009 增补 §3 一字不改）。
fn handle_target_tables(request: &mut Request) -> HttpResponse {
    if !has_json_content_type(request) {
        return error_response(unsupported_media_type(None));
    }
    let target: TargetConnection = match read_json(request) {
        Ok(target) => target,
        Err(error) => return error_response(error),
    };
    let destination = match MysqlDestination::connect(&target) {
        Ok(destination) => destination,
        Err(message) => return error_response(target_environment(message)),
    };
    match destination.target_tables() {
        Ok(tables) => json_response(200, &json!({ "tables": tables })),
        Err(message) => error_response(target_environment(message)),
    }
}

/// 一张目标表的列清单与唯一性约束（ADR-0038 §3）。
///
/// **表不存在不是错误**：`information_schema` 查不到就是空清单（ADR-0038 §9）。
/// 构建器只亮不判——「这张表能不能用」的结论归映射预检出。
fn handle_target_columns(request: &mut Request) -> HttpResponse {
    if !has_json_content_type(request) {
        return error_response(unsupported_media_type(None));
    }
    let payload: TargetColumnsRequest = match read_json(request) {
        Ok(payload) => payload,
        Err(error) => return error_response(error),
    };
    let destination = match MysqlDestination::connect(&payload.target) {
        Ok(destination) => destination,
        Err(message) => return error_response(target_environment(message)),
    };
    let columns = match destination.target_columns(&payload.target_table) {
        Ok(columns) => columns,
        Err(message) => return error_response(target_environment(message)),
    };
    let keys = match destination.target_keys(&payload.target_table) {
        Ok(keys) => keys,
        Err(message) => return error_response(target_environment(message)),
    };
    json_response(200, &json!({ "columns": columns, "keys": keys }))
}

/// 错误码闭集不增（ADR-0010 十五码，ADR-0038 §9）：目标端连不上或查不动，
/// 都是目标端环境故障，与 `test-connection` 同一个码、同一个 `details.kind`。
fn target_environment(message: String) -> ApiError {
    ApiError {
        status: 500,
        code: "SINK_ENVIRONMENT",
        message: format!("读取目标端元数据失败：{message}"),
        run_id: None,
        details: json!({ "kind": "OTHER" }),
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
        AtomicSwapError, AtomicSwapRequest, AtomicSwapResult, CreateStagingError, Destination,
        DropStagingError, FixedDestination, TargetColumn, TargetKey, WriteBatchError,
    };

    /// 报文里的目标端连接（ADR-0037 §1）。夹具走 `FixedDestination`，这份值被忽略，
    /// 但**必须带**——`OpenRunRequest` 的 `deny_unknown_fields` 与必填字段一起把它钉住了。
    const TARGET_JSON: &str = r#""target":{"host":"127.0.0.1","port":3306,"username":"sink","password":"change-me","database":"qbs"},"#;

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
                default_value: None,
                extra: String::new(),
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
            r#"{{"run_id":"{run_id}",{TARGET_JSON}"target_table":"T_POSITION","primary_key":["D_BIZ"],"source_columns":[{{"name":"D_BIZ","type":"DATE","precision":null,"scale":null,"length":null}}]}}"#
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
            r#"{{"run_id":"{run_id}",{TARGET_JSON}"target_table":"T_POSITION","primary_key":["D_BIZ"],"source_columns":[{{"name":"D_BIZ","type":"DATE","precision":null,"scale":null,"length":null}}]}}"#
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

    #[test]
    fn the_target_metadata_face_fails_as_an_environment_fault_and_leaves_no_run_behind() {
        // 连不上目标端 → SINK_ENVIRONMENT + details.kind = "OTHER"，码闭集不增
        // （ADR-0038 §9，与 test-connection 同一个码）。127.0.0.1:1 上没有 MySQL。
        let service = Arc::new(SinkService::new(
            "qbs",
            Arc::new(FakeDestination::default()),
        ));
        let target = r#"{"host":"127.0.0.1","port":1,"username":"sink","password":"x","database":"qbs"}"#;

        let (status, body) = exchange(service.clone(), "/v1/target/tables", target);
        assert_eq!(status, 500, "{body}");
        assert_eq!(body["error"]["code"], "SINK_ENVIRONMENT");
        assert_eq!(body["error"]["details"]["kind"], "OTHER");
        // 不属于任何 run：报文里没有 run_id，注册表里也没多出东西（ADR-0038 §3）。
        assert!(body["error"]["run_id"].is_null(), "{body}");

        let (status, body) = exchange(
            service.clone(),
            "/v1/target/columns",
            &format!(r#"{{"target":{target},"target_table":"T_POSITION"}}"#),
        );
        assert_eq!(status, 500, "{body}");
        assert_eq!(body["error"]["code"], "SINK_ENVIRONMENT");

        let (status, unknown) =
            exchange_method(service, "GET", "/v1/runs/20260814091530_a3f19c", "");
        assert_eq!(status, 404);
        assert_eq!(unknown["error"]["code"], "RUN_UNKNOWN");
    }

    #[test]
    fn the_columns_endpoint_nests_the_connection_and_refuses_a_stray_field() {
        // 连接嵌在 `target` 里（与 OpenRunRequest 同形），顶层只多一个 `target_table`。
        // flatten 进顶层就得放弃 `deny_unknown_fields`，拼错字段名会静默通过。
        let service = Arc::new(SinkService::new(
            "qbs",
            Arc::new(FakeDestination::default()),
        ));
        let target = r#"{"host":"127.0.0.1","port":1,"username":"sink","password":"x","database":"qbs"}"#;

        let (status, flattened) = exchange(
            service.clone(),
            "/v1/target/columns",
            &format!(r#"{{{TARGET_JSON}"target_table":"T","host":"127.0.0.1"}}"#),
        );
        assert_eq!(status, 400, "{flattened}");
        assert_eq!(flattened["error"]["code"], "BAD_REQUEST");

        let (status, missing_table) =
            exchange(service, "/v1/target/columns", &format!(r#"{{"target":{target}}}"#));
        assert_eq!(status, 400, "{missing_table}");
    }

    /// 夹具的身份。**不落盘**：这一层测的是路由与报文，身份怎么来的由
    /// `agent.rs` 自己的用例守（跨重启稳定那条）。
    fn fixture_agent() -> AgentInfo {
        AgentInfo {
            agent_id: "fixture-agent".to_owned(),
            name: "fixture".to_owned(),
            version: "0.0.0-test".to_owned(),
        }
    }

    /// `GET /v1/agent/info`（ADR-0044 §2）：未鉴权、无请求体、回三个字段。
    /// source 的注册与每次开跑前的身份核对都打这里，路由掉了整条链就哑了。
    #[test]
    fn agent_info_is_served() {
        let service = Arc::new(SinkService::new("qbs", Arc::new(FakeDestination::default())));

        let (status, body) = exchange_method(service, "GET", "/v1/agent/info", "");

        assert_eq!(status, 200, "{body}");
        assert_eq!(body["agent_id"], "fixture-agent");
        assert_eq!(body["name"], "fixture");
        assert!(body.get("version").is_some(), "{body}");
    }

    fn exchange(
        service: Arc<SinkService<FixedDestination<FakeDestination>>>,
        path: &str,
        body: &str,
    ) -> (u16, Value) {
        exchange_method(service, "POST", path, body)
    }

    fn exchange_method(
        service: Arc<SinkService<FixedDestination<FakeDestination>>>,
        method: &str,
        path: &str,
        body: &str,
    ) -> (u16, Value) {
        let server = Server::http("127.0.0.1:0").unwrap();
        let address = server.server_addr().to_ip().unwrap();
        let worker = thread::spawn(move || {
            let request = server.recv().unwrap();
            handle_request(request, &service, &fixture_agent());
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
