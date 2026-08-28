use db_qbs_source::{generate_target_ddl, ColumnPrecision, ColumnSupport, SourceColumn};

#[test]
fn target_ddl_is_derived_from_describe_columns() {
    let columns = vec![
        source_column("N_VA_PRICE", "NUMBER", Some(18), Some(4), None),
        source_column("C_NAME", "VARCHAR2", None, None, Some(50)),
        source_column("D_BIZ", "DATE", None, None, None),
    ];

    let ddl = generate_target_ddl(&columns, "T_POSITION", &key("D_BIZ"), None, None).unwrap();

    assert_eq!(
        ddl,
        concat!(
            "-- db-qbs 生成的目标表建表 SQL，请自行执行；产品不会替你建表。\n",
            "-- 下面那条主键不是可选项：写入走 upsert，目标表没有它时重跑会静默出重复行。\n",
            "CREATE TABLE `T_POSITION` (\n",
            "  `N_VA_PRICE` DECIMAL(18,4) NULL,\n",
            "  `C_NAME` VARCHAR(50) NULL,\n",
            "  `D_BIZ` DATETIME(0) NOT NULL,\n",
            "  PRIMARY KEY (`D_BIZ`)\n",
            ") DEFAULT CHARSET=utf8mb4;"
        )
    );
    assert!(!ddl.contains("ALTER TABLE"));
}

#[test]
fn empty_target_table_uses_the_visible_placeholder() {
    let columns = vec![source_column("D_BIZ", "DATE", None, None, None)];

    let ddl = generate_target_ddl(&columns, "", &key("D_BIZ"), None, None).unwrap();

    assert!(ddl.contains("CREATE TABLE <目标表名> ("));
}

#[test]
fn target_ddl_uses_all_m3_source_shapes() {
    let columns = vec![
        source_column("N_REGULAR", "NUMBER", Some(18), Some(4), None),
        source_column("N_FRACTION", "NUMBER", Some(4), Some(6), None),
        source_column("N_NEGATIVE", "NUMBER", Some(8), Some(-2), None),
        source_column("N_RAW", "NUMBER", None, None, None),
        source_column("C_VARCHAR", "VARCHAR2", None, None, Some(50)),
        source_column("C_NVARCHAR", "NVARCHAR2", None, None, Some(40)),
        source_column("C_CHAR", "CHAR", None, None, Some(30)),
        source_column("C_NCHAR", "NCHAR", None, None, Some(20)),
        source_column("D_BIZ", "DATE", None, None, None),
        timestamp_column("D_EVENT", 3),
    ];
    let mut precision = ColumnPrecision::new();
    precision.insert("N_RAW".to_owned(), [12, 2]);

    let ddl = generate_target_ddl(
        &columns,
        "T_POSITION",
        &key("D_BIZ"),
        Some(&precision),
        None,
    )
    .unwrap();

    for expected in [
        "`N_REGULAR` DECIMAL(18,4) NULL",
        "`N_FRACTION` DECIMAL(6,6) NULL",
        "`N_NEGATIVE` DECIMAL(10,0) NULL",
        "`N_RAW` DECIMAL(12,2) NULL",
        "`C_VARCHAR` VARCHAR(50) NULL",
        "`C_NVARCHAR` VARCHAR(40) NULL",
        "`C_CHAR` VARCHAR(30) NULL",
        "`C_NCHAR` VARCHAR(20) NULL",
        "`D_BIZ` DATETIME(0) NOT NULL",
        "`D_EVENT` DATETIME(6) NULL",
    ] {
        assert!(ddl.contains(expected), "missing {expected} in {ddl}");
    }
    assert!(!ddl.contains("`C_CHAR` CHAR(30) NULL"));
    assert!(!ddl.contains("`C_NCHAR` NCHAR(20) NULL"));
}

#[test]
fn target_ddl_leaves_unconfigured_number_shapes_as_placeholders() {
    let columns = vec![
        source_column("N_RAW", "NUMBER", None, None, None),
        source_column("N_EXPR", "NUMBER", None, None, None),
        source_column("D_BIZ", "DATE", None, None, None),
    ];

    let ddl = generate_target_ddl(&columns, "T_POSITION", &key("D_BIZ"), None, None).unwrap();

    assert!(ddl.contains("-- N_RAW、N_EXPR 列的精度 describe 给不出，请在取列面为它们配 (p,s)。"));
    assert_eq!(ddl.matches("DECIMAL(<p>,<s>)").count(), 2);
}

#[test]
fn target_ddl_escapes_placeholder_names_inside_the_sql_comment() {
    let columns = vec![
        source_column("N_RAW\nDROP TABLE `T_AUDIT`;", "NUMBER", None, None, None),
        source_column("D_BIZ", "DATE", None, None, None),
    ];

    let ddl = generate_target_ddl(&columns, "T_POSITION", &key("D_BIZ"), None, None).unwrap();

    assert!(ddl.contains("-- N_RAW\\nDROP TABLE `T_AUDIT`; 列的精度"));
    assert!(!ddl.contains("\nDROP TABLE `T_AUDIT`;"));
}

#[test]
fn target_ddl_rejects_derived_shapes_at_both_decimal_boundaries() {
    for (name, precision, scale, expected_shape) in [
        ("N_NEGATIVE", 38, -30, "DECIMAL(68,0)"),
        ("N_FRACTION", 4, 35, "DECIMAL(35,35)"),
    ] {
        let columns = vec![
            source_column(name, "NUMBER", Some(precision), Some(scale), None),
            source_column("D_BIZ", "DATE", None, None, None),
        ];

        let error =
            generate_target_ddl(&columns, "T_POSITION", &key("D_BIZ"), None, None).unwrap_err();

        assert_eq!(error.columns.len(), 1);
        assert_eq!(error.columns[0].column, name);
        assert!(error.columns[0].message.contains(expected_shape));
    }
}

#[test]
fn target_ddl_error_reports_all_unsupported_columns() {
    let columns = vec![
        source_column("PAYLOAD", "CLOB", None, None, None),
        source_column("SCORE_F", "BINARY_DOUBLE", None, None, None),
        timestamp_column("AUDIT_TS", 9),
        source_column("D_BIZ", "DATE", None, None, None),
    ];

    let error = generate_target_ddl(&columns, "T_POSITION", &key("D_BIZ"), None, None).unwrap_err();

    assert_eq!(
        error
            .columns
            .iter()
            .map(|column| column.column.as_str())
            .collect::<Vec<_>>(),
        vec!["PAYLOAD", "SCORE_F", "AUDIT_TS"]
    );
    assert_eq!(error.columns[0].source, "CLOB");
    assert_eq!(error.columns[1].source, "BINARY_DOUBLE");
    assert_eq!(error.columns[2].source, "TIMESTAMP(9)");
    assert!(error
        .columns
        .iter()
        .all(|column| column.message.contains("source SQL") || column.message.contains("CAST")));
}

#[test]
fn a_composite_primary_key_makes_every_key_column_not_null() {
    let columns = vec![
        source_column("C_FUND", "VARCHAR2", None, None, Some(20)),
        timestamp_column("D_BIZ", 6),
        source_column("N_AMT", "NUMBER", Some(18), Some(2), None),
    ];

    let ddl = generate_target_ddl(
        &columns,
        "T_POSITION",
        &["C_FUND".to_owned(), "D_BIZ".to_owned()],
        None,
        None,
    )
    .unwrap();

    assert!(ddl.contains("`C_FUND` VARCHAR(20) NOT NULL"));
    assert!(ddl.contains("`D_BIZ` DATETIME(6) NOT NULL"));
    // 非主键列仍要可空——那是 ADR-0009 映射预检的要求，两条各管各的列。
    assert!(ddl.contains("`N_AMT` DECIMAL(18,2) NULL"));
    assert!(ddl.contains("PRIMARY KEY (`C_FUND`, `D_BIZ`)"));
}

/// #261：一列主键都没勾时，语句里不出现主键约束，而且每一列都可空。
///
/// 表头那句交底也跟着换：说的不再是「别去掉这条主键」，而是「重跑会再追加一份」。
#[test]
fn no_primary_key_means_no_constraint_and_a_note_that_says_why() {
    let columns = vec![
        source_column("C_FUND", "VARCHAR2", None, None, Some(20)),
        source_column("N_AMT", "NUMBER", Some(18), Some(2), None),
    ];

    let ddl = generate_target_ddl(&columns, "T_FLOW", &[], None, None).unwrap();

    assert!(!ddl.contains("PRIMARY KEY"), "{ddl}");
    assert!(ddl.contains("`C_FUND` VARCHAR(20) NULL"), "{ddl}");
    assert!(ddl.contains("`N_AMT` DECIMAL(18,2) NULL"), "{ddl}");
    assert!(ddl.contains("每跑一次都会把这批数据再追加一份"), "{ddl}");
}

#[test]
fn a_primary_key_column_missing_from_describe_is_named() {
    let columns = vec![source_column("D_BIZ", "DATE", None, None, None)];

    let error =
        generate_target_ddl(&columns, "T_POSITION", &key("C_FUND"), None, None).unwrap_err();

    assert_eq!(error.columns.len(), 1);
    assert_eq!(error.columns[0].column, "C_FUND");
    assert_eq!(error.columns[0].source, "<missing>");
}

fn key(column: &str) -> Vec<String> {
    vec![column.to_owned()]
}

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
        support: Some(ColumnSupport::Ok),
    }
}

fn timestamp_column(name: &str, fsp: u32) -> SourceColumn {
    SourceColumn {
        name: name.to_owned(),
        data_type: "TIMESTAMP".to_owned(),
        precision: None,
        scale: None,
        length: None,
        fsp: Some(fsp),
        support: Some(ColumnSupport::Ok),
    }
}
