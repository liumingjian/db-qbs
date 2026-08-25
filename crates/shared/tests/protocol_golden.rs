//! 报文形状的契约测试（#124）。
//!
//! 搬家的绝大部分由编译器兜底，但**编译器管不着序列化出来的 JSON 长什么样**：
//! serde 属性合并错一处、`rename` 漏一个，代码照样编译通过，错要到两端真跑起来才暴露。
//! 这里给每个报文钉一份固定样本，**正反双向**各走一遍；可选字段的 `Some` / `None`
//! 两种取值分别钉一份，否则 `skip_serializing_if` 合并错了测不出来。
//!
//! 附带作用：以后谁再把定义抄回两端，这组测试会立刻变成两份、自己暴露出来。

use db_qbs_shared::{
    AbortResponse, AgentInfo, BatchPayload, BatchResponse, CleanupRunRequest, CleanupRunResponse,
    ColumnSupport, CommitRequest, CommitResponse, ErrorBody, ErrorEnvelope, OpenOutcome,
    OpenRunRequest, OpenRunResponse, PrecheckIssue, RangeCheckColumn, RangeCheckResult, RunResponse,
    SourceColumn, TargetConnection, Terminal,
};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::json;

/// 正反双向：序列化必须逐字段等于样本，样本必须能反序列化回等价的值。
fn round_trip<T>(value: T, expected: serde_json::Value)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let encoded = serde_json::to_value(&value).expect("报文必须能序列化");
    assert_eq!(encoded, expected, "序列化结果与样本不一致");

    let decoded: T = serde_json::from_value(expected).expect("样本必须能反序列化");
    assert_eq!(decoded, value, "反序列化结果与原值不一致");
}

/// 目标端连接（ADR-0037 §1）。**口令在线上是明文**——这份样本把它钉死，
/// 免得日后有人「顺手」在报文里做一层编码就以为过线也加密了。
fn target() -> TargetConnection {
    TargetConnection {
        host: "10.0.0.9".to_owned(),
        port: 3306,
        username: "sink".to_owned(),
        password: "change-me".to_owned(),
        database: "qbs".to_owned(),
    }
}

fn target_json() -> serde_json::Value {
    json!({
        "host": "10.0.0.9",
        "port": 3306,
        "username": "sink",
        "password": "change-me",
        "database": "qbs"
    })
}

fn column_full() -> SourceColumn {
    SourceColumn {
        name: "AMT".to_owned(),
        data_type: "NUMBER".to_owned(),
        precision: Some(10),
        scale: Some(2),
        length: None,
        fsp: Some(6),
        support: Some(ColumnSupport::NeedsPrecision),
    }
}

fn column_minimal() -> SourceColumn {
    SourceColumn {
        name: "ID".to_owned(),
        data_type: "VARCHAR2".to_owned(),
        precision: None,
        scale: None,
        length: Some(32),
        fsp: None,
        support: None,
    }
}

#[test]
fn batch_payload_shape() {
    round_trip(
        BatchPayload {
            seq: 7,
            rows: vec![vec![Some("1".to_owned()), None]],
        },
        json!({ "seq": 7, "rows": [["1", null]] }),
    );
}

#[test]
fn column_support_shape() {
    // 三档标记走 snake_case，web 直接读这几个字面量。
    round_trip(ColumnSupport::Ok, json!("ok"));
    round_trip(ColumnSupport::NeedsPrecision, json!("needs_precision"));
    round_trip(ColumnSupport::Unsupported, json!("unsupported"));
}

#[test]
fn source_column_shape_with_optionals() {
    // `type` 是 rename 过来的；`fsp` / `support` 有值时必须出现。
    round_trip(
        column_full(),
        json!({
            "name": "AMT",
            "type": "NUMBER",
            "precision": 10,
            "scale": 2,
            "length": null,
            "fsp": 6,
            "support": "needs_precision"
        }),
    );
}

#[test]
fn source_column_shape_omits_absent_optionals() {
    // `fsp` / `support` 为空时**整个键都不出现**；`precision` 一类没有 skip，照发 null。
    round_trip(
        column_minimal(),
        json!({
            "name": "ID",
            "type": "VARCHAR2",
            "precision": null,
            "scale": null,
            "length": 32
        }),
    );
}

#[test]
fn range_check_shapes() {
    round_trip(
        RangeCheckColumn {
            column: "AMT".to_owned(),
            precision: 10,
            scale: 2,
        },
        json!({ "column": "AMT", "precision": 10, "scale": 2 }),
    );
    round_trip(
        RangeCheckResult {
            column: "AMT".to_owned(),
            invalid_rows: 3,
        },
        json!({ "column": "AMT", "invalid_rows": 3 }),
    );
}

#[test]
fn precheck_issue_shape_with_suggestion() {
    round_trip(
        PrecheckIssue {
            column: "AMT".to_owned(),
            source: "NUMBER(10,2)".to_owned(),
            target: "DECIMAL(8,2)".to_owned(),
            rule: "precision_shrink".to_owned(),
            suggestion: Some("把目标列改成 DECIMAL(10,2)".to_owned()),
        },
        json!({
            "column": "AMT",
            "source": "NUMBER(10,2)",
            "target": "DECIMAL(8,2)",
            "rule": "precision_shrink",
            "suggestion": "把目标列改成 DECIMAL(10,2)"
        }),
    );
}

#[test]
fn precheck_issue_shape_omits_absent_suggestion() {
    round_trip(
        PrecheckIssue {
            column: "AMT".to_owned(),
            source: "NUMBER".to_owned(),
            target: "DECIMAL(65,30)".to_owned(),
            rule: "bare_number".to_owned(),
            suggestion: None,
        },
        json!({
            "column": "AMT",
            "source": "NUMBER",
            "target": "DECIMAL(65,30)",
            "rule": "bare_number"
        }),
    );
}

#[test]
fn open_run_request_shape_with_range_check_results() {
    round_trip(
        OpenRunRequest {
            run_id: "20260818120000_a1b2c3".to_owned(),
            target_table: "ORDERS".to_owned(),
            target: target(),
            primary_key: vec!["ID".to_owned()],
            source_columns: vec![column_minimal()],
            range_check_results: Some(vec![RangeCheckResult {
                column: "AMT".to_owned(),
                invalid_rows: 0,
            }]),
        },
        json!({
            "run_id": "20260818120000_a1b2c3",
            "target_table": "ORDERS",
            "target": target_json(),
            "primary_key": ["ID"],
            "source_columns": [{
                "name": "ID",
                "type": "VARCHAR2",
                "precision": null,
                "scale": null,
                "length": 32
            }],
            "range_check_results": [{ "column": "AMT", "invalid_rows": 0 }]
        }),
    );
}

#[test]
fn open_run_request_shape_omits_absent_range_check_results() {
    round_trip(
        OpenRunRequest {
            run_id: "20260818120000_a1b2c3".to_owned(),
            target_table: "ORDERS".to_owned(),
            target: target(),
            primary_key: vec!["ID".to_owned()],
            source_columns: vec![],
            range_check_results: None,
        },
        json!({
            "run_id": "20260818120000_a1b2c3",
            "target_table": "ORDERS",
            "target": target_json(),
            "primary_key": ["ID"],
            "source_columns": []
        }),
    );
}

#[test]
fn open_run_response_shape_with_range_check_columns() {
    round_trip(
        OpenRunResponse {
            run_id: "run-1".to_owned(),
            staging_table: "ORDERS__stg_run-1".to_owned(),
            columns_checked: 2,
            range_check_columns: Some(vec![RangeCheckColumn {
                column: "AMT".to_owned(),
                precision: 10,
                scale: 2,
            }]),
        },
        json!({
            "run_id": "run-1",
            "staging_table": "ORDERS__stg_run-1",
            "columns_checked": 2,
            "range_check_columns": [{ "column": "AMT", "precision": 10, "scale": 2 }]
        }),
    );
}

/// 「还没开成」那一答的暗号：空 `staging_table` 配上有值的 `range_check_columns`。
/// 两端都不再手搓这一对，但**线上的字节一个都没动**——这里钉的就是那件事。
#[test]
fn range_check_needed_outcome_keeps_the_bytes_it_has_always_had() {
    let response = OpenOutcome::RangeCheckNeeded {
        run_id: "run-1".to_owned(),
        columns_checked: 2,
        columns: vec![RangeCheckColumn {
            column: "AMT".to_owned(),
            precision: 10,
            scale: 2,
        }],
    }
    .into_response();

    round_trip(
        response.clone(),
        json!({
            "run_id": "run-1",
            "staging_table": "",
            "columns_checked": 2,
            "range_check_columns": [{ "column": "AMT", "precision": 10, "scale": 2 }]
        }),
    );
    assert!(matches!(
        OpenOutcome::from_response(response),
        OpenOutcome::RangeCheckNeeded { .. }
    ));
}

#[test]
fn opened_outcome_keeps_the_bytes_it_has_always_had() {
    let response = OpenOutcome::Opened {
        run_id: "run-1".to_owned(),
        staging_table: "ORDERS__stg_run-1".to_owned(),
        columns_checked: 2,
    }
    .into_response();

    round_trip(
        response.clone(),
        json!({
            "run_id": "run-1",
            "staging_table": "ORDERS__stg_run-1",
            "columns_checked": 2
        }),
    );
    assert!(matches!(
        OpenOutcome::from_response(response),
        OpenOutcome::Opened { .. }
    ));
}

/// 「要核这零列」不是一个请求。旧 source 一向把它当作开成了，换成 outcome 之后仍然如此。
#[test]
fn an_empty_range_check_list_reads_as_opened() {
    assert!(matches!(
        OpenOutcome::from_response(OpenRunResponse {
            run_id: "run-1".to_owned(),
            staging_table: "ORDERS__stg_run-1".to_owned(),
            columns_checked: 2,
            range_check_columns: Some(Vec::new()),
        }),
        OpenOutcome::Opened { .. }
    ));
}

#[test]
fn open_run_response_shape_omits_absent_range_check_columns() {
    round_trip(
        OpenRunResponse {
            run_id: "run-1".to_owned(),
            staging_table: "ORDERS__stg_run-1".to_owned(),
            columns_checked: 0,
            range_check_columns: None,
        },
        json!({
            "run_id": "run-1",
            "staging_table": "ORDERS__stg_run-1",
            "columns_checked": 0
        }),
    );
}

#[test]
fn batch_response_shape() {
    round_trip(
        BatchResponse {
            seq: 1,
            rows_written: 500,
            next_seq: 2,
        },
        json!({ "seq": 1, "rows_written": 500, "next_seq": 2 }),
    );
}

#[test]
fn commit_request_and_response_shapes() {
    round_trip(
        CommitRequest {
            total_batches: 4,
            total_rows: 2000,
        },
        json!({ "total_batches": 4, "total_rows": 2000 }),
    );
    round_trip(
        CommitResponse {
            source_rows: 2000,
            staged_rows: 2000,
            purged_rows: 10,
            swapped_rows: 2000,
            count_ms: 42,
        },
        json!({
            "source_rows": 2000,
            "staged_rows": 2000,
            "purged_rows": 10,
            "swapped_rows": 2000,
            "count_ms": 42
        }),
    );
}

#[test]
fn cleanup_request_and_response_shapes() {
    round_trip(
        CleanupRunRequest {
            run_id: "20260814091530_a3f19c".to_owned(),
            target_table: "T_POSITION".to_owned(),
            target: target(),
            primary_key: vec!["ID".to_owned(), "TENANT".to_owned()],
        },
        json!({
            "run_id": "20260814091530_a3f19c",
            "target_table": "T_POSITION",
            "target": {
                "host": "10.0.0.9",
                "port": 3306,
                "username": "sink",
                "password": "change-me",
                "database": "qbs"
            },
            "primary_key": ["ID", "TENANT"]
        }),
    );
    round_trip(
        CleanupRunResponse {
            run_id: "20260814091530_a3f19c".to_owned(),
            deleted_rows: 7,
        },
        json!({ "run_id": "20260814091530_a3f19c", "deleted_rows": 7 }),
    );
}

#[test]
fn abort_response_shape() {
    round_trip(
        AbortResponse {
            run_id: "run-1".to_owned(),
            staging_dropped: true,
        },
        json!({ "run_id": "run-1", "staging_dropped": true }),
    );
}

#[test]
fn terminal_shape() {
    // 大写字面量与 `Terminal::as_str()` 必须一致——两处漂了，运行历史就对不上。
    round_trip(Terminal::Swapped, json!("SWAPPED"));
    round_trip(Terminal::Discarded, json!("DISCARDED"));
    assert_eq!(Terminal::Swapped.as_str(), "SWAPPED");
    assert_eq!(Terminal::Discarded.as_str(), "DISCARDED");
}

#[test]
fn run_response_shape_terminal() {
    round_trip(
        RunResponse {
            run_id: "run-1".to_owned(),
            staging_table: "ORDERS__stg_run-1".to_owned(),
            batches_received: 4,
            rows_written: 2000,
            sealed: true,
            terminal: Some(Terminal::Swapped),
            purged_rows: Some(10),
            swapped_rows: Some(2000),
        },
        json!({
            "run_id": "run-1",
            "staging_table": "ORDERS__stg_run-1",
            "batches_received": 4,
            "rows_written": 2000,
            "sealed": true,
            "terminal": "SWAPPED",
            "purged_rows": 10,
            "swapped_rows": 2000
        }),
    );
}

#[test]
fn run_response_shape_in_flight_omits_terminal_fields() {
    round_trip(
        RunResponse {
            run_id: "run-1".to_owned(),
            staging_table: "ORDERS__stg_run-1".to_owned(),
            batches_received: 1,
            rows_written: 500,
            sealed: false,
            terminal: None,
            purged_rows: None,
            swapped_rows: None,
        },
        json!({
            "run_id": "run-1",
            "staging_table": "ORDERS__stg_run-1",
            "batches_received": 1,
            "rows_written": 500,
            "sealed": false
        }),
    );
}

#[test]
fn error_envelope_shape() {
    // `run_id` 没有 skip：为空时照发 null（sink 今天就是这么发的，别顺手加 skip）。
    round_trip(
        ErrorEnvelope {
            error: ErrorBody {
                code: "PRECHECK_FAILED".to_owned(),
                message: "预检不通过".to_owned(),
                run_id: Some("run-1".to_owned()),
                details: json!({ "issues": [] }),
            },
        },
        json!({
            "error": {
                "code": "PRECHECK_FAILED",
                "message": "预检不通过",
                "run_id": "run-1",
                "details": { "issues": [] }
            }
        }),
    );
    round_trip(
        ErrorEnvelope {
            error: ErrorBody {
                code: "BAD_REQUEST".to_owned(),
                message: "请求体不是有效 JSON".to_owned(),
                run_id: None,
                details: json!({}),
            },
        },
        json!({
            "error": {
                "code": "BAD_REQUEST",
                "message": "请求体不是有效 JSON",
                "run_id": null,
                "details": {}
            }
        }),
    );
}

#[test]
fn error_envelope_tolerates_unknown_details_keys() {
    // 信封刻意不开严格字段校验：`details` 是无类型口袋，sink 往里塞什么都收得下。
    // 这条钉的是「今天的宽松」，不是主张它对——见 #124「本票明确不做」第 2 条。
    let decoded: ErrorEnvelope = serde_json::from_value(json!({
        "error": {
            "code": "VERIFY_FAILED",
            "message": "行数校验不过",
            "run_id": "run-1",
            "details": { "source_rows": 10, "staged_rows": 9, "从没见过的键": true }
        }
    }))
    .expect("信封必须容得下没见过的明细键");
    assert_eq!(decoded.error.code, "VERIFY_FAILED");
}

/// agent 身份自述（ADR-0044 §2）。三个字段全必填、**一个凭据字段都没有**——
/// 这个端点是未鉴权面（ADR-0024），钉死形状就是钉死「这里读不到别的东西」。
#[test]
fn agent_info_shape() {
    round_trip(
        AgentInfo {
            agent_id: "6f1a9c2d4e8b47f0a1b2c3d4e5f60718".to_owned(),
            name: "target-a".to_owned(),
            version: "0.1.0".to_owned(),
        },
        json!({
            "agent_id": "6f1a9c2d4e8b47f0a1b2c3d4e5f60718",
            "name": "target-a",
            "version": "0.1.0",
        }),
    );
}
