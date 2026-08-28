//! sink 的 HTTP 面，**进程内直调**。
//!
//! 这里一条测试都不开 socket、不 spawn 二进制：`Api::handle(&Request) -> Response`
//! 就是全部入口。业务那半边本来就在 `SinkService` 接缝后面（`test_support::InMemoryDestination`），
//! 路由这一层从前够不着，只能手搓 HTTP——#200 把它一起搬进了库里。
//! `sink_skeleton.rs` 里那几条仍然走真进程，证的是「二进制起得来、对外真的在服务」。

use std::sync::Arc;

use db_qbs_shared::AgentInfo;
use db_qbs_sink::http::{routes, Api, Method, Request, Response};
use db_qbs_sink::test_support::{datetime_target_column, InMemoryDestination};
use db_qbs_sink::{FixedDestination, SinkService};
use serde_json::{json, Value};

/// 夹具用的工厂：固定一份内存目的地，请求里的连接信息被忽略。
type Fixture = FixedDestination<InMemoryDestination>;

/// 报文里的目标端连接（ADR-0037 §1）。夹具走 `FixedDestination`，这份值被忽略，
/// 但**必须带**——`OpenRunRequest` 的 `deny_unknown_fields` 与必填字段一起把它钉住了。
const TARGET_JSON: &str = r#""target":{"host":"127.0.0.1","port":3306,"username":"sink","password":"change-me","database":"qbs"},"#;

/// 真去连、且必然连不上的一份连接：127.0.0.1:1 上没有 MySQL。
/// 目标端元数据那三条**不走工厂**，它们自己建连接，所以夹具挡不住它们。
const UNREACHABLE_TARGET: &str =
    r#"{"host":"127.0.0.1","port":1,"username":"sink","password":"x","database":"qbs"}"#;

/// 通用 404 的那句话。判断「路由压根没匹配上」就靠它。
const UNROUTED: &str = "请求的 sink v1 资源不存在";

/// 一套进程内的 sink：一个服务实例加一份**不落盘**的身份。
///
/// 身份怎么来的由 `agent.rs` 自己的用例守（跨重启稳定那条）；这一层测的是路由与报文。
struct Rig {
    service: SinkService<Fixture>,
    agent: AgentInfo,
    /// 同一份目的地，测试这一侧也留一把手：#257 要在它身上装「这台 MySQL 报什么版本」。
    destination: Arc<InMemoryDestination>,
}

impl Rig {
    fn new() -> Self {
        Self::with_destination(d_biz_destination())
    }

    fn with_destination(destination: InMemoryDestination) -> Self {
        let destination = Arc::new(destination);
        Self {
            service: SinkService::new("qbs", Arc::clone(&destination)),
            agent: AgentInfo {
                agent_id: "fixture-agent".to_owned(),
                name: "fixture".to_owned(),
                version: "0.0.0-test".to_owned(),
                mysql: None,
            },
            destination,
        }
    }

    fn api(&self) -> Api<'_, Fixture> {
        Api {
            service: &self.service,
            agent: &self.agent,
        }
    }

    fn send(&self, method: Method, path: &str, body: &str) -> Response {
        self.api().handle(
            &Request::new(method, path, body.as_bytes().to_vec())
                .with_header("Content-Type", "application/json"),
        )
    }

    fn post(&self, path: &str, body: &str) -> Response {
        self.send(Method::Post, path, body)
    }

    fn get(&self, path: &str) -> Response {
        self.send(Method::Get, path, "")
    }

    fn json(&self, response: &Response) -> Value {
        serde_json::from_slice(&response.body)
            .unwrap_or_else(|_| panic!("响应不是 JSON：{}", response.body_text()))
    }

    /// 开一次 run，返回它的 id。
    fn open_run(&self, run_id: &str) -> String {
        let response = self.post("/v1/runs", &open_body(run_id));
        assert_eq!(response.status, 200, "{}", response.body_text());
        run_id.to_owned()
    }
}

/// 这些用例开的都是同一张表：单列 `D_BIZ datetime`，主键就是它。
fn d_biz_destination() -> InMemoryDestination {
    InMemoryDestination {
        columns: vec![datetime_target_column("D_BIZ")],
        ..InMemoryDestination::default()
    }
}

fn open_body(run_id: &str) -> String {
    format!(
        r#"{{"run_id":"{run_id}",{TARGET_JSON}"target_table":"T_POSITION","primary_key":["D_BIZ"],"source_columns":[{{"name":"D_BIZ","type":"DATE","precision":null,"scale":null,"length":null}}]}}"#
    )
}

/// 每条路由都到得了它的 handler——**一条都不许漏**。
///
/// 最后一段跟 `routes()` 对账：**新加一条路由却不在这张表里，这里就红**。
#[test]
fn every_route_reaches_its_handler() {
    let rig = Rig::new();
    let run_id = rig.open_run("20260814091530_a3f19c");

    // (方法, 路由样式, 实打实的 URL, 请求体, 期望状态码)
    let checks: Vec<(Method, &str, String, String, u16)> = vec![
        (
            Method::Get,
            "/v1/agent/info",
            "/v1/agent/info".into(),
            String::new(),
            200,
        ),
        // 已经开过一次的 run 再开一次是幂等的 200，所以这一行换个 id 走首开那一支。
        (
            Method::Post,
            "/v1/runs",
            "/v1/runs".into(),
            open_body("20260814091531_b4e20d"),
            200,
        ),
        // `primary_key` 空 → 400，不必真连目标端。
        (
            Method::Post,
            "/v1/runs/cleanup",
            "/v1/runs/cleanup".into(),
            format!(
                r#"{{"run_id":"{run_id}","target_table":"T_POSITION","target":{UNREACHABLE_TARGET},"primary_key":[]}}"#
            ),
            400,
        ),
        (
            Method::Post,
            "/v1/target/test-connection",
            "/v1/target/test-connection".into(),
            UNREACHABLE_TARGET.into(),
            500,
        ),
        (
            Method::Post,
            "/v1/target/tables",
            "/v1/target/tables".into(),
            UNREACHABLE_TARGET.into(),
            500,
        ),
        (
            Method::Post,
            "/v1/target/columns",
            "/v1/target/columns".into(),
            format!(r#"{{"target":{UNREACHABLE_TARGET},"target_table":"T_POSITION"}}"#),
            500,
        ),
        (
            Method::Post,
            "/v1/target/check",
            "/v1/target/check".into(),
            format!(
                r#"{{{TARGET_JSON}"target_table":"T_POSITION","primary_key":["D_BIZ"],"source_columns":[{{"name":"D_BIZ","type":"DATE","precision":null,"scale":null,"length":null}}]}}"#
            ),
            200,
        ),
        (
            Method::Post,
            "/v1/runs/{}/batches",
            format!("/v1/runs/{run_id}/batches"),
            r#"{"seq":1,"rows":[["2026-08-14 12:00:00"]]}"#.into(),
            200,
        ),
        (
            Method::Post,
            "/v1/runs/{}/commit",
            format!("/v1/runs/{run_id}/commit"),
            r#"{"total_batches":1,"total_rows":1}"#.into(),
            200,
        ),
        // 认不出的 run 上的 abort 是 200（幂等），所以它不需要一个活着的 run。
        (
            Method::Post,
            "/v1/runs/{}/abort",
            "/v1/runs/20260814091532_c5f31e/abort".into(),
            "{}".into(),
            200,
        ),
        (
            Method::Get,
            "/v1/runs/{}",
            format!("/v1/runs/{run_id}"),
            String::new(),
            200,
        ),
    ];

    for (method, pattern, url, body, expected) in &checks {
        let response = rig.send(*method, url, body);
        assert!(
            !response.body_text().contains(UNROUTED),
            "{method:?} {pattern} 没有匹配上任何路由"
        );
        assert_eq!(
            response.status,
            *expected,
            "{method:?} {url} 回的是 {}：{}",
            response.status,
            response.body_text()
        );
    }

    let covered: Vec<(Method, &str)> = checks
        .iter()
        .map(|(method, pattern, ..)| (*method, *pattern))
        .collect();
    for route in routes::<Fixture>() {
        assert!(
            covered.contains(&(route.method, route.pattern)),
            "路由 {:?} {} 没有测试走过它——每条路由都得在上面那张表里有一行",
            route.method,
            route.pattern
        );
    }
    assert_eq!(
        covered.len(),
        routes::<Fixture>().len(),
        "表里有路由表上没有的行"
    );
}

/// 表里的先后**不承重**：字面量样式永远压过带占位的样式。
#[test]
fn literal_patterns_win_over_placeholder_patterns() {
    let rig = Rig::new();

    // `cleanup` 从前靠写在按 run id 分发的那一支之前才不会被当成一个 run id。
    let cleanup = rig.post("/v1/runs/cleanup", "{}");
    assert_eq!(cleanup.status, 400, "{}", cleanup.body_text());
    assert!(!cleanup.body_text().contains(UNROUTED));

    // 同一段路径换成 GET 就只剩按 run id 那条路由——它是个认不出的 run。
    let as_a_run = rig.get("/v1/runs/cleanup");
    assert_eq!(as_a_run.status, 404);
    assert_eq!(rig.json(&as_a_run)["error"]["code"], "RUN_UNKNOWN");
}

/// 路由表里不许有两行同方法同样式——重复的那条永远是死的。
#[test]
fn route_table_declares_each_method_and_pattern_once() {
    let mut seen: Vec<(Method, &str)> = Vec::new();
    for route in routes::<Fixture>() {
        let key = (route.method, route.pattern);
        assert!(!seen.contains(&key), "路由重复：{:?} {}", key.0, key.1);
        seen.push(key);
    }
}

/// 一段 run id 里不许有 `/`，也不许为空——这条规矩从前抄在 `run_resource` 与
/// `run_action` 两处，现在只有 `match_pattern` 一处。
#[test]
fn run_ids_are_a_single_path_segment() {
    let rig = Rig::new();
    for path in [
        "/v1/runs/",
        "/v1/runs/a/b",
        "/v1/runs//batches",
        "/v1/runs/a/",
    ] {
        let response = rig.post(path, "{}");
        assert_eq!(response.status, 404, "{path} 不该匹配上任何路由");
        assert!(response.body_text().contains(UNROUTED), "{path}");
    }
    // 按 run id 取资源那条（GET）走的是同一条规矩，它自己也得被问一遍。
    for path in ["/v1/runs/", "/v1/runs/a/b", "/v1/runs/a/extra"] {
        let response = rig.get(path);
        assert_eq!(response.status, 404, "{path} 不该匹配上任何路由");
        assert!(response.body_text().contains(UNROUTED), "{path}");
    }
}

/// 请求体的 64 MiB 断路器。**判定只有一处**（`read_json`）：翻译层只多读一个字节，
/// 那一个字节就是判据。超了是 413，且报文里带上限，方便调用方对账自己的批次预算。
#[test]
fn an_oversized_request_body_trips_the_circuit_breaker() {
    let rig = Rig::new();
    let max_bytes = 64 * 1024 * 1024_usize;

    let response = rig.send(Method::Post, "/v1/runs", &"a".repeat(max_bytes + 1));

    assert_eq!(response.status, 413);
    let body = rig.json(&response);
    assert_eq!(body["error"]["code"], "PAYLOAD_TOO_LARGE");
    assert_eq!(body["error"]["details"]["max_bytes"], max_bytes as u64);

    // 正好压线的那一份过得了这道闸，倒在下一道（它不是 JSON）。
    let at_the_limit = rig.send(Method::Post, "/v1/runs", &"a".repeat(max_bytes));
    assert_eq!(at_the_limit.status, 400, "{}", at_the_limit.body_text());
    assert_eq!(rig.json(&at_the_limit)["error"]["code"], "BAD_REQUEST");
}

/// Content-Type 的判定是「同名头里**有没有任意一份**是 application/json」。
/// 收窄成只看第一份，一份带了两个 Content-Type 的合法请求就会变成 415。
#[test]
fn a_json_content_type_counts_wherever_among_the_headers_it_sits() {
    let rig = Rig::new();

    let response = rig.api().handle(
        &Request::new(
            Method::Post,
            "/v1/runs",
            open_body("20260814091530_a3f19c").into_bytes(),
        )
        .with_header("Content-Type", "text/plain")
        .with_header("content-type", "APPLICATION/JSON"),
    );

    assert_eq!(response.status, 200, "{}", response.body_text());
}

/// 认不出的方法、认不出的路径，回的是同一句 404。
#[test]
fn unknown_method_and_unknown_path_share_one_404() {
    let rig = Rig::new();

    let unknown_path = rig.post("/v1/nope", "{}");
    assert_eq!(unknown_path.status, 404);
    assert!(unknown_path.body_text().contains(UNROUTED));

    // 路由表里只有 GET 与 POST：别的方法一律落进 `Method::Other`。
    let wrong_method = rig.send(Method::Other, "/v1/runs", "{}");
    assert_eq!(wrong_method.status, 404);
    assert!(wrong_method.body_text().contains(UNROUTED));

    // 方法对不上样式也是同一句，不另开一种「405」。
    let get_on_post_only = rig.get("/v1/target/tables");
    assert_eq!(get_on_post_only.status, 404);
}

/// 带请求体的端点一律要 `application/json`，缺了就是 415，而且**不在 run 上留痕**。
#[test]
fn a_body_without_a_json_content_type_is_refused() {
    let rig = Rig::new();
    let run_id = rig.open_run("20260814091530_a3f19c");

    let response = rig.api().handle(&Request::new(
        Method::Post,
        "/v1/runs",
        open_body("20260814091531_b4e20d").into_bytes(),
    ));
    assert_eq!(response.status, 415, "{}", response.body_text());
    assert_eq!(rig.json(&response)["error"]["code"], "BAD_REQUEST");

    // run 上的端点把 run_id 一并报回去——调用方靠它把失败挂到自己那次运行上。
    let on_a_run = rig.api().handle(&Request::new(
        Method::Post,
        &format!("/v1/runs/{run_id}/batches"),
        br#"{"seq":1,"rows":[[null]]}"#.to_vec(),
    ));
    assert_eq!(on_a_run.status, 415, "{}", on_a_run.body_text());
    assert_eq!(rig.json(&on_a_run)["error"]["run_id"], run_id);
}

#[test]
fn open_batch_and_abort_lifecycle_uses_contract_statuses_and_bodies() {
    let rig = Rig::new();
    let run_id = "20260814091530_a3f19c";

    let opened = rig.post("/v1/runs", &open_body(run_id));
    assert_eq!(opened.status, 200, "{}", opened.body_text());
    assert_eq!(rig.json(&opened)["run_id"], run_id);

    let batch = rig.post(
        &format!("/v1/runs/{run_id}/batches"),
        r#"{"seq":1,"rows":[[null]]}"#,
    );
    assert_eq!(batch.status, 200, "{}", batch.body_text());
    let batch = rig.json(&batch);
    assert_eq!(batch["seq"], 1);
    assert_eq!(batch["rows_written"], 1);
    assert_eq!(batch["next_seq"], 2);

    let abort_path = format!("/v1/runs/{run_id}/abort");
    let aborted = rig.post(&abort_path, "{}");
    assert_eq!(aborted.status, 200, "{}", aborted.body_text());
    assert_eq!(rig.json(&aborted)["staging_dropped"], true);

    // 重复 abort 与从没见过的 run 上的 abort 是同一个回答：200 + 没丢过暂存表。
    let repeated = rig.post(&abort_path, "{}");
    assert_eq!(repeated.status, 200);
    assert_eq!(rig.json(&repeated)["staging_dropped"], false);

    let unknown = rig.post("/v1/runs/20260814091531_b4e20d/abort", "{}");
    assert_eq!(unknown.status, 200);
    assert_eq!(rig.json(&unknown)["staging_dropped"], false);
}

#[test]
fn commit_and_get_expose_the_terminal_resource() {
    let rig = Rig::new();
    let run_id = rig.open_run("20260814091530_a3f19c");
    rig.post(
        &format!("/v1/runs/{run_id}/batches"),
        r#"{"seq":1,"rows":[["2026-08-14 12:00:00"]]}"#,
    );

    let committed = rig.post(
        &format!("/v1/runs/{run_id}/commit"),
        r#"{"total_batches":1,"total_rows":1}"#,
    );
    assert_eq!(committed.status, 200, "{}", committed.body_text());
    let committed = rig.json(&committed);
    assert_eq!(committed["source_rows"], 1);
    assert_eq!(committed["swapped_rows"], 1);
    assert_eq!(committed["count_ms"], 0);

    let terminal = rig.get(&format!("/v1/runs/{run_id}"));
    assert_eq!(terminal.status, 200, "{}", terminal.body_text());
    assert_eq!(rig.json(&terminal)["terminal"], "SWAPPED");

    let unknown = rig.get("/v1/runs/20260814091531_b4e20d");
    assert_eq!(unknown.status, 404);
    assert_eq!(rig.json(&unknown)["error"]["code"], "RUN_UNKNOWN");
}

#[test]
fn the_target_metadata_face_fails_as_an_environment_fault_and_leaves_no_run_behind() {
    // 连不上目标端 → SINK_ENVIRONMENT + details.kind = "OTHER"，码闭集不增
    // （ADR-0038 §9，与 test-connection 同一个码）。
    let rig = Rig::new();

    let tables = rig.post("/v1/target/tables", UNREACHABLE_TARGET);
    assert_eq!(tables.status, 500, "{}", tables.body_text());
    let body = rig.json(&tables);
    assert_eq!(body["error"]["code"], "SINK_ENVIRONMENT");
    assert_eq!(body["error"]["details"]["kind"], "OTHER");
    // 不属于任何 run：报文里没有 run_id，注册表里也没多出东西（ADR-0038 §3）。
    assert!(body["error"]["run_id"].is_null(), "{body}");

    let columns = rig.post(
        "/v1/target/columns",
        &format!(r#"{{"target":{UNREACHABLE_TARGET},"target_table":"T_POSITION"}}"#),
    );
    assert_eq!(columns.status, 500, "{}", columns.body_text());
    assert_eq!(rig.json(&columns)["error"]["code"], "SINK_ENVIRONMENT");

    let unknown = rig.get("/v1/runs/20260814091530_a3f19c");
    assert_eq!(unknown.status, 404);
    assert_eq!(rig.json(&unknown)["error"]["code"], "RUN_UNKNOWN");
}

#[test]
fn the_columns_endpoint_nests_the_connection_and_refuses_a_stray_field() {
    // 连接嵌在 `target` 里（与 OpenRunRequest 同形），顶层只多一个 `target_table`。
    // flatten 进顶层就得放弃 `deny_unknown_fields`，拼错字段名会静默通过。
    let rig = Rig::new();

    let flattened = rig.post(
        "/v1/target/columns",
        &format!(r#"{{{TARGET_JSON}"target_table":"T","host":"127.0.0.1"}}"#),
    );
    assert_eq!(flattened.status, 400, "{}", flattened.body_text());
    assert_eq!(rig.json(&flattened)["error"]["code"], "BAD_REQUEST");

    let missing_table = rig.post(
        "/v1/target/columns",
        &format!(r#"{{"target":{UNREACHABLE_TARGET}}}"#),
    );
    assert_eq!(missing_table.status, 400, "{}", missing_table.body_text());
}

#[test]
fn target_check_endpoint_returns_the_typed_precheck_result() {
    let rig = Rig::new();
    let result = rig.post(
        "/v1/target/check",
        &format!(
            r#"{{{TARGET_JSON}"target_table":"T_POSITION","primary_key":["D_BIZ"],"source_columns":[{{"name":"D_BIZ","type":"DATE","precision":null,"scale":null,"length":null}}]}}"#
        ),
    );
    assert_eq!(result.status, 200, "{}", result.body_text());
    let result = rig.json(&result);
    assert_eq!(result["ok"], true);
    assert_eq!(result["findings"], json!([]));
    assert!(result["suggested_ddl"].is_null());
}

/// `GET /v1/agent/info`（ADR-0044 §2）：未鉴权、无请求体、回三个字段。
/// source 的注册与每次开跑前的身份核对都打这里，路由掉了整条链就哑了。
#[test]
fn agent_info_is_served() {
    let rig = Rig::new();

    let response = rig.get("/v1/agent/info");

    assert_eq!(response.status, 200, "{}", response.body_text());
    assert_eq!(response.header("Content-Type"), Some("application/json"));
    let body = rig.json(&response);
    assert_eq!(body["agent_id"], "fixture-agent");
    assert_eq!(body["name"], "fixture");
    assert!(body.get("version").is_some(), "{body}");
}

/// #257 的核心：这台 agent 连的是哪个 MySQL，只有**手上有凭据的那些请求**才知道
/// （sink 自己不持有目标端凭据，ADR-0037 §2）。所以信息接口在第一次目标端检查之前
/// **必须报「未知」**——不带 `mysql` 字段，而不是端出一个 8.0 来。
#[test]
fn agent_info_reports_no_mysql_until_a_credentialed_request_has_observed_one() {
    let rig = Rig::new();
    rig.destination
        .report_mysql("5.7.44-log", "utf8mb4_general_ci");

    let before = rig.json(&rig.get("/v1/agent/info"));
    assert_eq!(
        before.get("mysql"),
        None,
        "还没连过目标端就报版本，等于凭空猜一个出来：{before}"
    );

    let checked = rig.post(
        "/v1/target/check",
        &format!(
            r#"{{{TARGET_JSON}"target_table":"T_POSITION","primary_key":["D_BIZ"],"source_columns":[{{"name":"D_BIZ","type":"DATE","precision":null,"scale":null,"length":null}}]}}"#
        ),
    );
    assert_eq!(checked.status, 200, "{}", checked.body_text());

    let after = rig.json(&rig.get("/v1/agent/info"));
    assert_eq!(after["mysql"]["version"], "5.7.44-log");
    assert_eq!(after["mysql"]["utf8mb4_collation"], "utf8mb4_general_ci");
}

/// 开跑那条路径也算一次观察——两条路径都带着凭据，谁先来谁把版本记上。
#[test]
fn opening_a_run_also_records_the_observed_mysql() {
    let rig = Rig::new();
    rig.destination.report_mysql("8.0.36", "utf8mb4_0900_ai_ci");

    rig.open_run("20260814091530_a3f19c");

    let info = rig.json(&rig.get("/v1/agent/info"));
    assert_eq!(info["mysql"]["version"], "8.0.36");
    assert_eq!(info["mysql"]["utf8mb4_collation"], "utf8mb4_0900_ai_ci");
}

/// 目的地报不出版本（连得上、但字符集表查不了）时，信息接口照旧报「未知」——
/// 一台连得上的 MySQL 不会因为这一项读不到就让整条链停摆，但也不许因此被猜成 8.0。
#[test]
fn a_destination_that_reports_nothing_leaves_the_info_endpoint_at_unknown() {
    let rig = Rig::new();

    rig.open_run("20260814091530_a3f19c");

    let info = rig.json(&rig.get("/v1/agent/info"));
    assert_eq!(info.get("mysql"), None, "{info}");
}
