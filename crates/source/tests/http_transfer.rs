use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use db_qbs_source::{
    run_transfer, ColumnSupport, HttpSinkClient, OpenRunRequest, RowSource, SinkClient,
    SinkErrorKind, SourceColumn, SourceReadError, TargetConnection, TransferRequest,
};
use serde_json::{json, Value};

const RUN_ID: &str = "20260814153000_a3f19c";

#[test]
fn rows_cross_the_http_protocol_then_commit() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        [
            json!({
                "run_id": RUN_ID,
                "staging_table": format!("ORDERS__stg_{RUN_ID}"),
                "columns_checked": 1,
            }),
            json!({ "seq": 1, "rows_written": 2, "next_seq": 2 }),
            json!({
                "source_rows": 2,
                "staged_rows": 2,
                "purged_rows": 3,
                "swapped_rows": 2,
                "count_ms": 4,
            }),
        ]
        .into_iter()
        .map(|response| serve_one(&listener, response))
        .collect::<Vec<_>>()
    });

    let mut source = FakeSource {
        rows: vec![vec![Some("1".to_owned())], vec![None]].into_iter(),
    };
    let mut sink = HttpSinkClient::new(&base_url).unwrap();
    let summary = run_transfer(
        &mut source,
        &mut sink,
        TransferRequest {
            run_id: RUN_ID.to_owned(),
            target_table: "ORDERS".to_owned(),
            target: TargetConnection {
                host: "127.0.0.1".to_owned(),
                port: 3306,
                username: "sink".to_owned(),
                password: "change-me".to_owned(),
                database: "qbs".to_owned(),
            },
            primary_key: vec!["ID".to_owned()],
        },
        |_| {},
    )
    .unwrap();

    let requests = server.join().unwrap();
    assert_eq!(
        requests
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "/v1/runs",
            &format!("/v1/runs/{RUN_ID}/batches"),
            &format!("/v1/runs/{RUN_ID}/commit"),
        ]
    );
    assert!(requests
        .iter()
        .all(|request| request.content_type == "application/json"));
    assert_eq!(requests[0].body["run_id"], RUN_ID);
    assert_eq!(requests[0].body["source_columns"][0]["name"], "ID");
    assert_eq!(
        requests[1].body,
        json!({ "seq": 1, "rows": [["1"], [null]] })
    );
    assert_eq!(
        requests[2].body,
        json!({ "total_batches": 1, "total_rows": 2 })
    );
    assert_eq!(summary.source_rows, 2);
    assert_eq!(summary.purged_rows, 3);
    assert_eq!(summary.count_ms, 4);
}

#[test]
fn error_response_preserves_sink_diagnostics() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        serve_one_with_status(
            &listener,
            "422 Unprocessable Entity",
            json!({
                "error": {
                    "code": "PRECHECK_FAILED",
                    "message": "mapping rejected",
                    "details": { "column": "AMOUNT", "value": "invalid" },
                }
            }),
        )
    });

    let mut sink = HttpSinkClient::new(&base_url).unwrap();
    let error = sink
        .open(&OpenRunRequest {
            run_id: RUN_ID.to_owned(),
            target_table: "ORDERS".to_owned(),
            target: TargetConnection {
                host: "127.0.0.1".to_owned(),
                port: 3306,
                username: "sink".to_owned(),
                password: "change-me".to_owned(),
                database: "qbs".to_owned(),
            },
            primary_key: vec!["ID".to_owned()],
            source_columns: Vec::new(),
            range_check_results: None,
        })
        .unwrap_err();

    let request = server.join().unwrap();
    assert_eq!(request.path, "/v1/runs");
    assert_eq!(error.kind, SinkErrorKind::Response);
    assert_eq!(error.code.as_deref(), Some("PRECHECK_FAILED"));
    assert_eq!(error.message, "mapping rejected");
    assert_eq!(error.column.as_deref(), Some("AMOUNT"));
    assert_eq!(error.value.as_deref(), Some("invalid"));
}

struct FakeSource {
    rows: std::vec::IntoIter<Vec<Option<String>>>,
}

impl RowSource for FakeSource {
    fn columns(&self) -> &[SourceColumn] {
        static COLUMNS: std::sync::LazyLock<Vec<SourceColumn>> = std::sync::LazyLock::new(|| {
            vec![SourceColumn {
                name: "ID".to_owned(),
                data_type: "NUMBER".to_owned(),
                precision: Some(8),
                scale: Some(0),
                length: None,
                fsp: None,
                support: Some(ColumnSupport::Ok),
            }]
        });
        &COLUMNS
    }

    fn next_row(&mut self) -> Result<Option<Vec<Option<String>>>, SourceReadError> {
        Ok(self.rows.next())
    }
}

struct Request {
    path: String,
    content_type: String,
    body: Value,
}

fn serve_one(listener: &TcpListener, response: Value) -> Request {
    serve_one_with_status(listener, "200 OK", response)
}

fn serve_one_with_status(listener: &TcpListener, status: &str, response: Value) -> Request {
    let (stream, _) = listener.accept().unwrap();
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).unwrap();
    let path = request_line.split_whitespace().nth(1).unwrap().to_owned();
    let mut content_length = 0usize;
    let mut content_type = String::new();

    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" {
            break;
        }
        let Some((name, value)) = line.trim_end().split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse().unwrap();
        } else if name.eq_ignore_ascii_case("content-type") {
            content_type = value.trim().to_owned();
        }
    }

    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).unwrap();
    write_response(reader.into_inner(), status, response);
    Request {
        path,
        content_type,
        body: serde_json::from_slice(&body).unwrap(),
    }
}

fn write_response(mut stream: TcpStream, status: &str, response: Value) {
    let body = serde_json::to_vec(&response).unwrap();
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(&body).unwrap();
}
