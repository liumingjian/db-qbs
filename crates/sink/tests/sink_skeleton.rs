use std::fs;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use db_qbs_sink::{
    build_staging_ddl, check_connection_settings, precheck, AtomicSwapError, AtomicSwapRequest,
    AtomicSwapResult, CreateStagingError, Destination, DropStagingError, OpenRunRequest,
    RangeCheckColumn, RangeCheckResult, SinkConfig, SinkService, SourceColumn, TargetColumn,
    TargetConnection, TargetKey, WriteBatchError,
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
        fsp: None,
        support: None,
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
        default_value: None,
        extra: String::new(),
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
            // D_BIZ 是主键列：目标端必须 NOT NULL（ADR-0035 §2 第 3 条），
            // 「目标列必须可空」那条对主键列豁免——MySQL 的 PRIMARY KEY 列按定义就非空。
            target_column(
                "D_BIZ",
                "datetime",
                "datetime",
                None,
                None,
                None,
                Some(0),
                false,
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
fn startup_warns_about_the_unauthenticated_write_surface_before_connecting() {
    let config_path = std::env::temp_dir().join(format!(
        "db-qbs-sink-startup-banner-{}.toml",
        std::process::id()
    ));
    fs::write(
        &config_path,
        "mysql_dsn = \"mysql://sink:secret@127.0.0.1:1\"\n\
         database = \"qbs\"\n\
         listen = \"127.0.0.1:0\"\n",
    )
    .unwrap();

    // sink 启动**不再连 MySQL**（ADR-0037 §2），所以它不会再因为连不上而退出——
    // 这里必须读完想要的行就走人。`.output()` 会等进程结束，在新语义下是永久阻塞。
    let mut child = Command::new(env!("CARGO_BIN_EXE_db-qbs-sink"))
        .args(["--config"])
        .arg(&config_path)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut banner_line = String::new();
    reader.read_line(&mut banner_line).unwrap();
    let mut retired_line = String::new();
    reader.read_line(&mut retired_line).unwrap();
    // 横幅打在任何连接动作之前——而现在启动压根没有连接动作，所以进程还活着。
    // 这一条正是 ADR-0037 §2 把「启动即连库」拆掉之后新的可观测判据。
    assert!(
        child.try_wait().unwrap().is_none(),
        "sink 启动不该再因为连不上 MySQL 而退出"
    );
    child.kill().unwrap();
    child.wait().unwrap();
    fs::remove_file(config_path).unwrap();

    // 第二行：退役字段仍能解析，但一个字都不读，必须留声（ADR-0037 §2）。
    let retired: serde_json::Value = serde_json::from_str(retired_line.trim()).unwrap();
    assert_eq!(retired["level"], "warn");
    let retired_message = retired["message"].as_str().unwrap();
    assert!(retired_message.contains("mysql_dsn"), "{retired_message}");
    assert!(retired_message.contains("已退役"), "{retired_message}");

    let banner: serde_json::Value = serde_json::from_str(banner_line.trim()).unwrap();
    assert_eq!(banner["level"], "warn");
    assert_eq!(banner["event"], "sink_started");
    assert_eq!(banner["listen"], "127.0.0.1:0");
    assert!(banner["run_id"].is_null());
    assert!(banner["task"].is_null());
    let message = banner["message"].as_str().unwrap();
    assert!(message.contains("无鉴权"), "{message}");
    assert!(message.contains("任意暂存表与目标表"), "{message}");
    assert!(message.contains("127.0.0.1:0"), "{message}");
}

#[test]
fn precheck_reports_every_invalid_column_in_one_result() {
    let sources = vec![
        source_column("N_AMOUNT", "NUMBER", Some(18), Some(2), None),
        source_column("N_RAW", "NUMBER", None, None, None),
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

    for column in ["N_AMOUNT", "N_RAW", "C_NAME", "D_BIZ"] {
        assert!(
            issues.iter().any(|issue| issue.column == column),
            "missing {column} in {issues:?}"
        );
    }
    assert!(
        issues.iter().all(|issue| issue.suggestion.is_some()),
        "every precheck issue needs an action: {issues:?}"
    );
    assert!(issues.len() >= 5, "{issues:?}");
}

#[test]
fn precheck_uses_derived_number_lower_bounds_and_actionable_suggestions() {
    let sources = vec![
        source_column("N_INTEGER", "NUMBER", Some(12), Some(2), None),
        source_column("N_SCALE", "NUMBER", Some(4), Some(6), None),
        source_column("N_NEGATIVE", "NUMBER", Some(8), Some(-2), None),
        source_column("N_TOO_WIDE", "NUMBER", Some(38), Some(-30), None),
        SourceColumn {
            name: "T_TOO_PRECISE".to_owned(),
            data_type: "TIMESTAMP".to_owned(),
            precision: None,
            scale: None,
            length: None,
            fsp: Some(9),
            support: None,
        },
        SourceColumn {
            name: "T_NARROW".to_owned(),
            data_type: "TIMESTAMP".to_owned(),
            precision: None,
            scale: None,
            length: None,
            fsp: Some(3),
            support: None,
        },
    ];
    let targets = vec![
        target_column(
            "N_INTEGER",
            "decimal(8,2)",
            "decimal",
            Some(8),
            Some(2),
            None,
            None,
            true,
            None,
            1,
        ),
        target_column(
            "N_SCALE",
            "decimal(6,4)",
            "decimal",
            Some(6),
            Some(4),
            None,
            None,
            true,
            None,
            2,
        ),
        target_column(
            "N_NEGATIVE",
            "decimal(9,0)",
            "decimal",
            Some(9),
            Some(0),
            None,
            None,
            true,
            None,
            3,
        ),
        target_column(
            "N_TOO_WIDE",
            "decimal(38,0)",
            "decimal",
            Some(38),
            Some(0),
            None,
            None,
            true,
            None,
            4,
        ),
        target_column(
            "T_TOO_PRECISE",
            "datetime(6)",
            "datetime",
            None,
            None,
            None,
            Some(6),
            true,
            None,
            5,
        ),
        target_column(
            "T_NARROW",
            "datetime(3)",
            "datetime",
            None,
            None,
            None,
            Some(3),
            true,
            None,
            6,
        ),
    ];

    let issues = precheck("T_POSITION", &sources, &targets);
    let issue_for = |column: &str| {
        issues
            .iter()
            .find(|issue| issue.column == column)
            .unwrap_or_else(|| panic!("missing {column} in {issues:?}"))
    };

    let integer_issue = issue_for("N_INTEGER");
    assert!(integer_issue.rule.contains("整数位不足"));
    assert_eq!(
        integer_issue.suggestion.as_deref(),
        Some("改为 DECIMAL(12,2)")
    );

    let scale_issue = issue_for("N_SCALE");
    assert!(scale_issue.rule.contains("目标标度不足"));
    assert_eq!(scale_issue.suggestion.as_deref(), Some("改为 DECIMAL(6,6)"));

    assert_eq!(
        issue_for("N_NEGATIVE").suggestion.as_deref(),
        Some("改为 DECIMAL(10,0)")
    );
    assert_eq!(
        issue_for("N_TOO_WIDE").suggestion.as_deref(),
        Some("无合法目标形状，需改源 SQL 或 CAST 收窄")
    );
    assert_eq!(
        issue_for("T_TOO_PRECISE").suggestion.as_deref(),
        Some("改源 SQL 加 CAST 收窄到 TIMESTAMP(6)")
    );
    assert_eq!(
        issue_for("T_NARROW").suggestion.as_deref(),
        Some("改为 DATETIME(6)")
    );
}

#[test]
fn precheck_accepts_wider_number_targets_that_satisfy_lower_bounds() {
    let sources = vec![
        source_column("N_REGULAR", "NUMBER", Some(12), Some(2), None),
        source_column("N_FRACTION", "NUMBER", Some(4), Some(6), None),
        source_column("N_NEGATIVE", "NUMBER", Some(8), Some(-2), None),
    ];
    let targets = vec![
        target_column(
            "N_REGULAR",
            "decimal(14,3)",
            "decimal",
            Some(14),
            Some(3),
            None,
            None,
            true,
            None,
            1,
        ),
        target_column(
            "N_FRACTION",
            "decimal(20,6)",
            "decimal",
            Some(20),
            Some(6),
            None,
            None,
            true,
            None,
            2,
        ),
        target_column(
            "N_NEGATIVE",
            "decimal(12,1)",
            "decimal",
            Some(12),
            Some(1),
            None,
            None,
            true,
            None,
            3,
        ),
    ];

    assert_eq!(precheck("T_POSITION", &sources, &targets), []);
}

#[test]
fn precheck_rejects_overflowing_number_metadata_without_panicking() {
    let sources = vec![source_column(
        "N_OVERFLOW",
        "NUMBER",
        Some(i64::MAX),
        Some(i64::MIN),
        None,
    )];
    let targets = vec![target_column(
        "N_OVERFLOW",
        "decimal(65,30)",
        "decimal",
        Some(65),
        Some(30),
        None,
        None,
        true,
        None,
        1,
    )];

    let issues = precheck("T_POSITION", &sources, &targets);

    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].column, "N_OVERFLOW");
    assert!(issues[0].rule.contains("MySQL DECIMAL"), "{issues:?}");
    assert!(issues[0].suggestion.is_some(), "{issues:?}");
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

struct FakeDestination {
    columns: Vec<TargetColumn>,
    /// 目标表上的唯一约束。缺省给一条 `PRIMARY KEY (D_BIZ)`——多数用例的目标表是「配好的」，
    /// 缺约束是要单独立用例去证的例外，不是默认状态。
    keys: Vec<TargetKey>,
    created: Mutex<Vec<(String, String)>>,
    dropped: Mutex<Vec<String>>,
    create_error: Mutex<Option<CreateStagingError>>,
    drop_error: Mutex<Option<DropStagingError>>,
}

impl Default for FakeDestination {
    fn default() -> Self {
        Self {
            columns: Vec::new(),
            keys: vec![TargetKey {
                name: "PRIMARY".to_owned(),
                columns: vec!["D_BIZ".to_owned()],
            }],
            created: Mutex::new(Vec::new()),
            dropped: Mutex::new(Vec::new()),
            create_error: Mutex::new(None),
            drop_error: Mutex::new(None),
        }
    }
}

impl Destination for FakeDestination {
    fn target_columns(&self, _target_table: &str) -> Result<Vec<TargetColumn>, String> {
        Ok(self.columns.clone())
    }

    fn target_keys(&self, _target_table: &str) -> Result<Vec<TargetKey>, String> {
        Ok(self.keys.clone())
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
        target: TargetConnection {
            host: "127.0.0.1".to_owned(),
            port: 3306,
            username: "sink".to_owned(),
            password: "change-me".to_owned(),
            database: "qbs".to_owned(),
        },
        primary_key: vec!["D_BIZ".to_owned()],
        source_columns,
        range_check_results: None,
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
fn bare_number_range_check_delays_staging_and_rejects_invalid_rows() {
    let sources = vec![
        source_column("N_RAW", "NUMBER", None, None, None),
        source_column("D_BIZ", "DATE", None, None, None),
    ];
    let targets = vec![
        target_column(
            "N_RAW",
            "decimal(10,2)",
            "decimal",
            Some(10),
            Some(2),
            None,
            None,
            true,
            None,
            1,
        ),
        target_column(
            "D_BIZ",
            "datetime",
            "datetime",
            None,
            None,
            None,
            Some(0),
            false,
            None,
            2,
        ),
    ];
    let destination = Arc::new(FakeDestination {
        columns: targets,
        ..FakeDestination::default()
    });
    let service = SinkService::new("qbs", destination.clone());

    let first = service.open(open_request(sources.clone())).unwrap();

    assert_eq!(first.staging_table, "");
    assert_eq!(first.columns_checked, 2);
    assert_eq!(
        first.range_check_columns,
        Some(vec![RangeCheckColumn {
            column: "N_RAW".to_owned(),
            precision: 10,
            scale: 2,
        }])
    );
    assert!(destination.created.lock().unwrap().is_empty());

    let mut second_request = open_request(sources);
    second_request.range_check_results = Some(vec![RangeCheckResult {
        column: "N_RAW".to_owned(),
        invalid_rows: 3,
    }]);
    let error = service.open(second_request).unwrap_err();

    assert_eq!(error.code, "PRECHECK_FAILED");
    assert_eq!(error.details["issues"][0]["column"], "N_RAW");
    assert!(error.details["issues"][0]["rule"]
        .as_str()
        .unwrap()
        .contains("3"));
    assert_eq!(
        error.details["issues"][0]["suggestion"],
        "调整任务定义和目标 DECIMAL 的 (p,s)，或改源 SQL / CAST 收窄值域"
    );
    assert!(destination.created.lock().unwrap().is_empty());
}

#[test]
fn bare_number_range_check_with_no_invalid_rows_creates_staging() {
    let sources = vec![
        source_column("N_RAW", "NUMBER", None, None, None),
        source_column("D_BIZ", "DATE", None, None, None),
    ];
    let targets = vec![
        target_column(
            "N_RAW",
            "decimal(10,2)",
            "decimal",
            Some(10),
            Some(2),
            None,
            None,
            true,
            None,
            1,
        ),
        target_column(
            "D_BIZ",
            "datetime",
            "datetime",
            None,
            None,
            None,
            Some(0),
            false,
            None,
            2,
        ),
    ];
    let destination = Arc::new(FakeDestination {
        columns: targets,
        ..FakeDestination::default()
    });
    let service = SinkService::new("qbs", destination.clone());

    service.open(open_request(sources.clone())).unwrap();
    let mut second_request = open_request(sources);
    second_request.range_check_results = Some(vec![RangeCheckResult {
        column: "N_RAW".to_owned(),
        invalid_rows: 0,
    }]);

    let opened = service.open(second_request).unwrap();

    assert_eq!(opened.range_check_columns, None);
    assert_eq!(destination.created.lock().unwrap().len(), 1);
}

#[test]
fn open_rejects_a_primary_key_the_target_table_has_no_constraint_for() {
    // 撤掉 DELETE 之后这是**唯一**挡住静默重复的东西：目标表没有对应唯一约束时，
    // `ON DUPLICATE KEY UPDATE` 不报错、写得进去、重跑就多一份行。
    let (sources, targets) = valid_columns();
    let destination = Arc::new(FakeDestination {
        columns: targets,
        keys: Vec::new(),
        ..FakeDestination::default()
    });
    let service = SinkService::new("qbs", destination);

    let error = service.open(open_request(sources)).unwrap_err();

    assert_eq!(error.status, 422);
    assert!(
        error.details["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["target"] == "<无唯一约束>"),
        "{:?}",
        error.details
    );
}

#[test]
fn open_rejects_a_nullable_primary_key_column() {
    let (sources, mut targets) = valid_columns();
    targets[0].nullable = true;
    let destination = Arc::new(FakeDestination {
        columns: targets,
        ..FakeDestination::default()
    });
    let service = SinkService::new("qbs", destination);

    let error = service.open(open_request(sources)).unwrap_err();

    assert_eq!(error.status, 422);
    assert!(
        error.details["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["column"] == "D_BIZ"
                && issue["rule"].as_str().unwrap().contains("NOT NULL")),
        "{:?}",
        error.details
    );
}

#[test]
fn open_rejects_a_primary_key_column_that_is_not_among_the_selected_columns() {
    let (sources, targets) = valid_columns();
    let destination = Arc::new(FakeDestination {
        columns: targets,
        keys: vec![TargetKey {
            name: "PRIMARY".to_owned(),
            columns: vec!["C_MISSING".to_owned()],
        }],
        ..FakeDestination::default()
    });
    let service = SinkService::new("qbs", destination);
    let mut request = open_request(sources);
    request.primary_key = vec!["C_MISSING".to_owned()];

    let error = service.open(request).unwrap_err();

    assert_eq!(error.status, 422);
    assert!(
        error.details["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["column"] == "C_MISSING" && issue["source"] == "<missing>"),
        "{:?}",
        error.details
    );
}

#[test]
fn open_accepts_a_supported_timestamp_business_date_column() {
    let sources = vec![SourceColumn {
        name: "D_BIZ".to_owned(),
        data_type: "TIMESTAMP".to_owned(),
        precision: None,
        scale: None,
        length: None,
        fsp: Some(3),
        support: None,
    }];
    let targets = vec![target_column(
        "D_BIZ",
        "datetime(6)",
        "datetime",
        None,
        None,
        None,
        Some(6),
        false,
        None,
        1,
    )];
    let destination = Arc::new(FakeDestination {
        columns: targets,
        ..FakeDestination::default()
    });
    let service = SinkService::new("qbs", destination);

    let opened = service.open(open_request(sources)).unwrap();

    assert_eq!(opened.columns_checked, 1);
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
