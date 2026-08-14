use std::sync::{Arc, Mutex};

use db_qbs_sink::{
    build_staging_ddl, check_connection_settings, precheck, CreateStagingError, Destination,
    DropStagingError, OpenRunRequest, SinkConfig, SinkService, SourceColumn, TargetColumn,
    WriteBatchError,
};

const RUN_ID: &str = "20260814091530_a3f19c";

fn source_column(
    name: &str,
    data_type: &str,
    precision: Option<i64>,
    scale: Option<i64>,
    length: Option<u64>,
) -> SourceColumn {
    SourceColumn {
        name: name.to_owned(),
        data_type: data_type.to_owned(),
        precision,
        scale,
        length,
    }
}

#[allow(clippy::too_many_arguments)]
fn target_column(
    name: &str,
    column_type: &str,
    data_type: &str,
    precision: Option<u64>,
    scale: Option<u64>,
    length: Option<u64>,
    datetime_precision: Option<u64>,
    nullable: bool,
    character_set: Option<&str>,
    ordinal: u64,
) -> TargetColumn {
    TargetColumn {
        name: name.to_owned(),
        column_type: column_type.to_owned(),
        data_type: data_type.to_owned(),
        precision,
        scale,
        length,
        datetime_precision,
        nullable,
        character_set: character_set.map(str::to_owned),
        ordinal,
    }
}

fn valid_columns() -> (Vec<SourceColumn>, Vec<TargetColumn>) {
    (
        vec![
            source_column("N_AMOUNT", "NUMBER", Some(18), Some(2), None),
            source_column("C_NAME", "VARCHAR2", None, None, Some(50)),
            source_column("D_BIZ", "DATE", None, None, None),
        ],
        vec![
            target_column(
                "D_BIZ",
                "datetime",
                "datetime",
                None,
                None,
                None,
                Some(0),
                true,
                None,
                3,
            ),
            target_column(
                "N_AMOUNT",
                "decimal(18,2)",
                "decimal",
                Some(18),
                Some(2),
                None,
                None,
                true,
                None,
                1,
            ),
            target_column(
                "C_NAME",
                "varchar(80)",
                "varchar",
                None,
                None,
                Some(80),
                None,
                true,
                Some("utf8mb4"),
                2,
            ),
        ],
    )
}

#[test]
fn sink_config_rejects_unknown_fields() {
    let config = r#"
mysql_dsn = "mysql://sink:secret@127.0.0.1:3306"
database = "qbs"
listen = "127.0.0.1:8080"
databsae = "misspelled"
"#;

    let error = SinkConfig::parse(config).unwrap_err().to_string();

    assert!(error.contains("databsae"), "{error}");
}

#[test]
fn precheck_reports_every_invalid_column_in_one_result() {
    let sources = vec![
        source_column("N_AMOUNT", "NUMBER", Some(18), Some(2), None),
        source_column("C_NAME", "VARCHAR2", None, None, Some(50)),
        source_column("D_BIZ", "DATE", None, None, None),
    ];
    let targets = vec![
        target_column(
            "N_AMOUNT",
            "decimal(18,3)",
            "decimal",
            Some(18),
            Some(3),
            None,
            None,
            false,
            None,
            1,
        ),
        target_column(
            "C_NAME",
            "varchar(20)",
            "varchar",
            None,
            None,
            Some(20),
            None,
            true,
            Some("latin1"),
            2,
        ),
        target_column(
            "D_BIZ",
            "datetime(6)",
            "datetime",
            None,
            None,
            None,
            Some(6),
            true,
            None,
            3,
        ),
    ];

    let issues = precheck("T_POSITION", &sources, &targets);

    for column in ["N_AMOUNT", "C_NAME", "D_BIZ"] {
        assert!(
            issues.iter().any(|issue| issue.column == column),
            "missing {column} in {issues:?}"
        );
    }
    assert!(issues.len() >= 5, "{issues:?}");
}

#[test]
fn precheck_rejects_target_names_over_37_characters_with_actionable_text() {
    let (sources, targets) = valid_columns();
    let issues = precheck(
        "THIS_TARGET_TABLE_NAME_IS_LONGER_THAN_37",
        &sources,
        &targets,
    );

    assert!(issues.iter().any(|issue| {
        issue.column == "<target_table>"
            && issue.rule.contains("37")
            && issue.rule.contains("暂存表")
    }));
}

#[test]
fn staging_ddl_uses_target_order_and_types_without_constraints() {
    let (_, targets) = valid_columns();

    let ddl = build_staging_ddl("qbs", "T_POSITION__stg_20260814091530_a3f19c", &targets);

    assert_eq!(
        ddl,
        "CREATE TABLE `qbs`.`T_POSITION__stg_20260814091530_a3f19c` (\n  `N_AMOUNT` decimal(18,2) NULL,\n  `C_NAME` varchar(80) CHARACTER SET utf8mb4 NULL,\n  `D_BIZ` datetime NULL\n)"
    );
    assert!(!ddl.contains("PRIMARY KEY"));
    assert!(!ddl.contains("INDEX"));
    assert!(!ddl.contains("DEFAULT"));
}

#[test]
fn connection_ritual_reports_variable_expected_and_actual_values() {
    let error = check_connection_settings(
        "utf8mb4",
        "latin1",
        "utf8mb4",
        "STRICT_TRANS_TABLES",
        16 * 1024 * 1024,
    )
    .unwrap_err();

    assert!(error.contains("character_set_connection"), "{error}");
    assert!(error.contains("utf8mb4"), "{error}");
    assert!(error.contains("latin1"), "{error}");
    assert!(error.contains("sql_mode"), "{error}");
    assert!(error.contains("STRICT_ALL_TABLES"), "{error}");
    assert!(error.contains("STRICT_TRANS_TABLES"), "{error}");
    assert!(error.contains("max_allowed_packet"), "{error}");
    assert!(error.contains("67108864"), "{error}");
    assert!(error.contains("16777216"), "{error}");
}

#[derive(Default)]
struct FakeDestination {
    columns: Vec<TargetColumn>,
    created: Mutex<Vec<(String, String)>>,
    dropped: Mutex<Vec<String>>,
    create_error: Mutex<Option<CreateStagingError>>,
    drop_error: Mutex<Option<DropStagingError>>,
}

impl Destination for FakeDestination {
    fn target_columns(&self, _target_table: &str) -> Result<Vec<TargetColumn>, String> {
        Ok(self.columns.clone())
    }

    fn create_staging(&self, staging_table: &str, ddl: &str) -> Result<(), CreateStagingError> {
        if let Some(error) = self.create_error.lock().unwrap().take() {
            return Err(error);
        }
        self.created
            .lock()
            .unwrap()
            .push((staging_table.to_owned(), ddl.to_owned()));
        Ok(())
    }

    fn write_batch(
        &self,
        _staging_table: &str,
        _columns: &[String],
        rows: &[Vec<Option<String>>],
        _chunk_rows: usize,
    ) -> Result<u64, WriteBatchError> {
        Ok(rows.len() as u64)
    }

    fn drop_staging(&self, staging_table: &str) -> Result<(), DropStagingError> {
        if let Some(error) = self.drop_error.lock().unwrap().take() {
            return Err(error);
        }
        self.dropped.lock().unwrap().push(staging_table.to_owned());
        Ok(())
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

#[test]
fn open_creates_staging_then_abort_is_idempotent() {
    let (sources, targets) = valid_columns();
    let destination = Arc::new(FakeDestination {
        columns: targets,
        ..FakeDestination::default()
    });
    let service = SinkService::new("qbs", destination.clone());

    let opened = service.open(open_request(sources)).unwrap();

    assert_eq!(opened.run_id, RUN_ID);
    assert_eq!(
        opened.staging_table,
        "T_POSITION__stg_20260814091530_a3f19c"
    );
    assert_eq!(opened.columns_checked, 3);
    assert_eq!(destination.created.lock().unwrap().len(), 1);

    assert!(service.abort(RUN_ID).unwrap().staging_dropped);
    assert!(!service.abort(RUN_ID).unwrap().staging_dropped);
    assert!(
        !service
            .abort("20260814091531_b4e20d")
            .unwrap()
            .staging_dropped
    );
    assert_eq!(destination.dropped.lock().unwrap().len(), 1);
}

#[test]
fn open_rejects_a_target_date_column_mapped_from_a_non_date_source() {
    let (sources, targets) = valid_columns();
    let destination = Arc::new(FakeDestination {
        columns: targets,
        ..FakeDestination::default()
    });
    let service = SinkService::new("qbs", destination);
    let mut request = open_request(sources);
    request.target_date_col = "C_NAME".to_owned();

    let error = service.open(request).unwrap_err();

    assert_eq!(error.status, 422);
    assert!(error.details["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| {
            issue["column"] == "C_NAME"
                && issue["rule"] == "target_date_col 必须对应同名的 Oracle DATE 源列"
        }));
}

#[test]
fn existing_staging_table_is_never_dropped_and_message_names_its_time() {
    let (sources, targets) = valid_columns();
    let destination = Arc::new(FakeDestination {
        columns: targets,
        create_error: Mutex::new(Some(CreateStagingError::TableExists)),
        ..FakeDestination::default()
    });
    let service = SinkService::new("qbs", destination.clone());

    let error = service.open(open_request(sources)).unwrap_err();

    assert_eq!(error.status, 409);
    assert_eq!(error.code, "STAGING_CREATE_FAILED");
    assert!(error
        .message
        .contains("T_POSITION__stg_20260814091530_a3f19c"));
    assert!(error.message.contains("2026-08-14 09:15:30 UTC"));
    assert!(destination.dropped.lock().unwrap().is_empty());
}

#[test]
fn staging_permission_errors_name_create_or_drop() {
    let (sources, targets) = valid_columns();
    let create_denied = Arc::new(FakeDestination {
        columns: targets.clone(),
        create_error: Mutex::new(Some(CreateStagingError::PermissionDenied)),
        ..FakeDestination::default()
    });
    let service = SinkService::new("qbs", create_denied);

    let error = service.open(open_request(sources.clone())).unwrap_err();
    assert!(error.message.contains("CREATE"), "{}", error.message);
    assert_eq!(error.details["operation"], "CREATE");

    let drop_denied = Arc::new(FakeDestination {
        columns: targets,
        ..FakeDestination::default()
    });
    let service = SinkService::new("qbs", drop_denied.clone());
    service.open(open_request(sources)).unwrap();
    *drop_denied.drop_error.lock().unwrap() = Some(DropStagingError::PermissionDenied);

    let error = service.abort(RUN_ID).unwrap_err();
    assert!(error.message.contains("DROP"), "{}", error.message);
    assert_eq!(error.details["operation"], "DROP");
}
