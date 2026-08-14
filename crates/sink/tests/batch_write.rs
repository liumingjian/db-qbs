use std::sync::{Arc, Mutex};

use db_qbs_shared::BatchPayload;
use db_qbs_sink::{
    CreateStagingError, Destination, DropStagingError, OpenRunRequest, SinkService, SourceColumn,
    TargetColumn, WriteBatchError,
};

const RUN_ID: &str = "20260814091530_a3f19c";
const GOLDEN_PAYLOAD: &str =
    include_str!("../../../docs/spikes/fixtures/batch-payload-golden.json");

#[derive(Clone, Debug)]
struct BatchCall {
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
    chunk_rows: usize,
}

#[derive(Default)]
struct FakeDestination {
    columns: Vec<TargetColumn>,
    calls: Mutex<Vec<BatchCall>>,
    committed_rows: Mutex<Vec<Vec<Option<String>>>>,
    fail_chunk: Mutex<Option<usize>>,
    affected_rows: Mutex<Option<u64>>,
}

impl Destination for FakeDestination {
    fn target_columns(&self, _target_table: &str) -> Result<Vec<TargetColumn>, String> {
        Ok(self.columns.clone())
    }

    fn create_staging(&self, _staging_table: &str, _ddl: &str) -> Result<(), CreateStagingError> {
        Ok(())
    }

    fn write_batch(
        &self,
        _staging_table: &str,
        columns: &[String],
        rows: &[Vec<Option<String>>],
        chunk_rows: usize,
    ) -> Result<u64, WriteBatchError> {
        self.calls.lock().unwrap().push(BatchCall {
            columns: columns.to_vec(),
            rows: rows.to_vec(),
            chunk_rows,
        });

        let pending = rows.to_vec();
        for (chunk_index, _) in rows.chunks(chunk_rows).enumerate() {
            if *self.fail_chunk.lock().unwrap() == Some(chunk_index) {
                return Err(WriteBatchError::Other(format!(
                    "sub-statement {chunk_index} failed"
                )));
            }
        }
        self.committed_rows.lock().unwrap().extend(pending);
        Ok(self
            .affected_rows
            .lock()
            .unwrap()
            .unwrap_or(rows.len() as u64))
    }

    fn drop_staging(&self, _staging_table: &str) -> Result<(), DropStagingError> {
        Ok(())
    }
}

#[test]
fn golden_payload_keeps_source_order_null_and_empty_string() {
    let payload: BatchPayload = serde_json::from_str(GOLDEN_PAYLOAD).unwrap();
    let (sources, targets) = golden_columns();
    let destination = Arc::new(FakeDestination {
        columns: targets,
        ..FakeDestination::default()
    });
    let service = SinkService::new("qbs", destination.clone());
    service.open(open_request(sources)).unwrap();

    let response = service.write_batch(RUN_ID, payload.clone()).unwrap();

    assert_eq!(response.seq, 1);
    assert_eq!(response.rows_written, 6);
    assert_eq!(response.next_seq, 2);
    let calls = destination.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].columns,
        ["N_AMT", "C_NAME", "C_ESC", "C_EMPTY", "D_BIZ", "N_OPT"]
    );
    assert_eq!(calls[0].rows, payload.rows);
    assert_eq!(calls[0].rows[0][3].as_deref(), Some(""));
    assert!(calls[0].rows[0][5].is_none());
    assert_eq!(calls[0].chunk_rows, 65_535 / 6);
}

#[test]
fn narrow_batch_uses_one_statement_and_wide_batch_is_split_at_placeholder_limit() {
    let (narrow_sources, narrow_targets) = columns(3);
    let narrow_destination = Arc::new(FakeDestination {
        columns: narrow_targets,
        ..FakeDestination::default()
    });
    let narrow_service = SinkService::new("qbs", narrow_destination.clone());
    narrow_service.open(open_request(narrow_sources)).unwrap();
    narrow_service
        .write_batch(RUN_ID, payload(1, 5_000, 3))
        .unwrap();
    let narrow_calls = narrow_destination.calls.lock().unwrap();
    let narrow_call = &narrow_calls[0];
    assert_eq!(narrow_call.rows.chunks(narrow_call.chunk_rows).count(), 1);

    let (wide_sources, wide_targets) = columns(70);
    let wide_destination = Arc::new(FakeDestination {
        columns: wide_targets,
        ..FakeDestination::default()
    });
    let wide_service = SinkService::new("qbs", wide_destination.clone());
    wide_service.open(open_request(wide_sources)).unwrap();
    wide_service
        .write_batch(RUN_ID, payload(1, 1_000, 70))
        .unwrap();
    let wide_calls = wide_destination.calls.lock().unwrap();
    let wide_call = &wide_calls[0];
    assert_eq!(wide_call.chunk_rows, 65_535 / 70);
    assert_eq!(wide_call.rows.chunks(wide_call.chunk_rows).count(), 2);
}

#[test]
fn sub_statement_failure_commits_nothing_and_does_not_advance_sequence() {
    let (sources, targets) = columns(70);
    let destination = Arc::new(FakeDestination {
        columns: targets,
        fail_chunk: Mutex::new(Some(1)),
        ..FakeDestination::default()
    });
    let service = SinkService::new("qbs", destination.clone());
    service.open(open_request(sources)).unwrap();

    let error = service
        .write_batch(RUN_ID, payload(1, 1_000, 70))
        .unwrap_err();

    assert!(error.message.contains("第 1 批"), "{}", error.message);
    assert!(destination.committed_rows.lock().unwrap().is_empty());
    *destination.fail_chunk.lock().unwrap() = None;
    assert!(service.write_batch(RUN_ID, payload(1, 1_000, 70)).is_ok());
}

#[test]
fn unknown_run_wrong_sequence_and_wrong_row_width_are_rejected_before_writing() {
    let (sources, targets) = columns(3);
    let destination = Arc::new(FakeDestination {
        columns: targets,
        ..FakeDestination::default()
    });
    let service = SinkService::new("qbs", destination.clone());

    let unknown = service.write_batch(RUN_ID, payload(1, 1, 3)).unwrap_err();
    assert_eq!((unknown.status, unknown.code), (404, "RUN_UNKNOWN"));

    service.open(open_request(sources)).unwrap();
    let sequence = service.write_batch(RUN_ID, payload(2, 1, 3)).unwrap_err();
    assert_eq!((sequence.status, sequence.code), (409, "SEQ_MISMATCH"));
    assert_eq!(sequence.details["expected"], 1);
    assert_eq!(sequence.details["got"], 2);

    let width = service.write_batch(RUN_ID, payload(1, 1, 2)).unwrap_err();
    assert_eq!((width.status, width.code), (400, "BAD_REQUEST"));
    assert!(width.message.contains("第 1 批"), "{}", width.message);
    assert!(destination.calls.lock().unwrap().is_empty());
}

#[test]
fn affected_rows_mismatch_names_batch_and_counts() {
    let (sources, targets) = columns(3);
    let destination = Arc::new(FakeDestination {
        columns: targets,
        affected_rows: Mutex::new(Some(4_999)),
        ..FakeDestination::default()
    });
    let service = SinkService::new("qbs", destination.clone());
    service.open(open_request(sources)).unwrap();

    let error = service
        .write_batch(RUN_ID, payload(1, 5_000, 3))
        .unwrap_err();

    assert!(error.message.contains("第 1 批"), "{}", error.message);
    assert_eq!(error.details["expected"], 5_000);
    assert_eq!(error.details["written"], 4_999);
}

fn payload(seq: u64, row_count: usize, column_count: usize) -> BatchPayload {
    BatchPayload {
        seq,
        rows: vec![vec![Some("value".to_owned()); column_count]; row_count],
    }
}

fn open_request(source_columns: Vec<SourceColumn>) -> OpenRunRequest {
    OpenRunRequest {
        run_id: RUN_ID.to_owned(),
        target_table: "T_POSITION".to_owned(),
        target_date_col: "D_BIZ".to_owned(),
        biz_date: "2026-08-14".to_owned(),
        source_columns,
    }
}

fn columns(count: usize) -> (Vec<SourceColumn>, Vec<TargetColumn>) {
    let mut sources = Vec::with_capacity(count);
    let mut targets = Vec::with_capacity(count);
    for index in 0..count {
        let name = if index == 0 {
            "D_BIZ".to_owned()
        } else {
            format!("C_{index}")
        };
        sources.push(source_column(
            &name,
            if index == 0 { "DATE" } else { "VARCHAR2" },
            if index == 0 { None } else { Some(10) },
        ));
        targets.push(target_column(
            &name,
            if index == 0 {
                "datetime"
            } else {
                "varchar(10)"
            },
            if index == 0 { "datetime" } else { "varchar" },
            if index == 0 { None } else { Some(10) },
            (count - index) as u64,
        ));
    }
    (sources, targets)
}

fn golden_columns() -> (Vec<SourceColumn>, Vec<TargetColumn>) {
    let specs = [
        ("N_AMT", "NUMBER", None, "decimal(38,10)", "decimal"),
        ("C_NAME", "VARCHAR2", Some(50), "varchar(50)", "varchar"),
        ("C_ESC", "VARCHAR2", Some(200), "varchar(200)", "varchar"),
        ("C_EMPTY", "VARCHAR2", Some(10), "varchar(10)", "varchar"),
        ("D_BIZ", "DATE", None, "datetime", "datetime"),
        ("N_OPT", "NUMBER", None, "decimal(10,0)", "decimal"),
    ];
    let sources = specs
        .iter()
        .map(|(name, data_type, length, _, _)| SourceColumn {
            name: (*name).to_owned(),
            data_type: (*data_type).to_owned(),
            precision: match *name {
                "N_AMT" => Some(38),
                "N_OPT" => Some(10),
                _ => None,
            },
            scale: match *name {
                "N_AMT" => Some(10),
                "N_OPT" => Some(0),
                _ => None,
            },
            length: *length,
        })
        .collect();
    let targets = specs
        .iter()
        .enumerate()
        .map(
            |(index, (name, _, length, column_type, data_type))| TargetColumn {
                name: (*name).to_owned(),
                column_type: (*column_type).to_owned(),
                data_type: (*data_type).to_owned(),
                precision: match *name {
                    "N_AMT" => Some(38),
                    "N_OPT" => Some(10),
                    _ => None,
                },
                scale: match *name {
                    "N_AMT" => Some(10),
                    "N_OPT" => Some(0),
                    _ => None,
                },
                length: *length,
                datetime_precision: (*name == "D_BIZ").then_some(0),
                nullable: true,
                character_set: length.map(|_| "utf8mb4".to_owned()),
                ordinal: (specs.len() - index) as u64,
            },
        )
        .collect();
    (sources, targets)
}

fn source_column(name: &str, data_type: &str, length: Option<u64>) -> SourceColumn {
    SourceColumn {
        name: name.to_owned(),
        data_type: data_type.to_owned(),
        precision: None,
        scale: None,
        length,
    }
}

fn target_column(
    name: &str,
    column_type: &str,
    data_type: &str,
    length: Option<u64>,
    ordinal: u64,
) -> TargetColumn {
    TargetColumn {
        name: name.to_owned(),
        column_type: column_type.to_owned(),
        data_type: data_type.to_owned(),
        precision: None,
        scale: None,
        length,
        datetime_precision: (data_type == "datetime").then_some(0),
        nullable: true,
        character_set: length.map(|_| "utf8mb4".to_owned()),
        ordinal,
    }
}
