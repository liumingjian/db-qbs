use db_qbs_sink::{precheck_with_primary_key, TargetColumn, TargetKey};
// 列结构与三档标记的定义在 `db-qbs-shared`（#124），两端各是同一个类型——
// 这里不再有「source 的列结构抄成 sink 的列结构」那层恒等变换（#125 Q10）。
use db_qbs_source::{
    generate_target_ddl, ColumnPrecision, ColumnSupport, SourceColumn as DdlSourceColumn,
};
use serde::Deserialize;
use sqlparser::ast::{
    CharacterLength, ColumnOption, DataType, ExactNumberInfo, Statement, TableConstraint,
};
use sqlparser::dialect::MySqlDialect;
use sqlparser::parser::Parser;

const CANON_FIXTURE: &str = include_str!("../../../docs/spikes/fixtures/canon-golden.json");

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    #[serde(rename = "type")]
    data_type: String,
    tier: String,
}

#[test]
fn generated_target_ddl_stays_compatible_with_sink_precheck_for_m1_types() {
    let fixture: Fixture = serde_json::from_str(CANON_FIXTURE).unwrap();
    let cases: Vec<_> = fixture
        .cases
        .iter()
        .filter(|case| case.tier == "m1")
        .collect();
    assert_eq!(cases.len(), 36);

    for case in cases {
        for source_columns in source_columns_for(case) {
            let ddl = generate_target_ddl(&source_columns, "T_GENERATED", &key(), None).unwrap();
            let target_columns = parse_target_columns(&ddl);
            let target_keys = parse_target_keys(&ddl);

            // `support` 随报文经过 sink，但不得成为判定输入。
            // 主键这一维也要一起过：生成的 DDL 带 `PRIMARY KEY`，ADR-0035 §2 那三条
            // （约束确有、列在选中列里、列 NOT NULL）必须全过，否则两端就漂了。
            assert_eq!(
                precheck_with_primary_key(
                    "T_GENERATED",
                    &key(),
                    &source_columns,
                    &target_columns,
                    &target_keys,
                ),
                [],
                "{}: {ddl}",
                case.id
            );
        }
    }
}

#[test]
fn generated_target_ddl_stays_compatible_with_sink_precheck_for_m3_shapes() {
    let source_columns = vec![
        source_column_with_support(
            "N_REGULAR",
            "NUMBER",
            Some(18),
            Some(4),
            None,
            ColumnSupport::Ok,
        ),
        source_column_with_support(
            "N_FRACTION",
            "NUMBER",
            Some(4),
            Some(6),
            None,
            ColumnSupport::Ok,
        ),
        source_column_with_support(
            "N_NEGATIVE",
            "NUMBER",
            Some(8),
            Some(-2),
            None,
            ColumnSupport::Ok,
        ),
        // A configured bare NUMBER reaches the sink with its declared comparison shape.
        source_column_with_support(
            "N_RAW",
            "NUMBER",
            None,
            None,
            None,
            ColumnSupport::NeedsPrecision,
        ),
        source_column_with_support(
            "C_VARCHAR",
            "VARCHAR2",
            None,
            None,
            Some(50),
            ColumnSupport::Ok,
        ),
        source_column_with_support(
            "C_NVARCHAR",
            "NVARCHAR2",
            None,
            None,
            Some(40),
            ColumnSupport::Ok,
        ),
        source_column_with_support("C_CHAR", "CHAR", None, None, Some(30), ColumnSupport::Ok),
        source_column_with_support("C_NCHAR", "NCHAR", None, None, Some(20), ColumnSupport::Ok),
        source_column_with_support("D_BIZ", "DATE", None, None, None, ColumnSupport::Ok),
        timestamp_column_with_support("D_EVENT", 3, ColumnSupport::Ok),
        // 每形态各一条边界（#125 Q3）：标度顶格、负标度顶到精度 65、字符最短、小数秒两端。
        source_column_with_support(
            "N_SCALE_MAX",
            "NUMBER",
            Some(4),
            Some(30),
            None,
            ColumnSupport::Ok,
        ),
        source_column_with_support(
            "N_NEGATIVE_MAX",
            "NUMBER",
            Some(38),
            Some(-27),
            None,
            ColumnSupport::Ok,
        ),
        source_column_with_support("C_MIN", "CHAR", None, None, Some(1), ColumnSupport::Ok),
        timestamp_column_with_support("D_FSP_MIN", 0, ColumnSupport::Ok),
        timestamp_column_with_support("D_FSP_MAX", 6, ColumnSupport::Ok),
    ];

    let mut column_precision = ColumnPrecision::new();
    column_precision.insert("N_RAW".to_owned(), [12, 2]);
    let ddl = generate_target_ddl(
        &source_columns,
        "T_GENERATED",
        &key(),
        Some(&column_precision),
    )
    .unwrap();
    let target_columns = parse_target_columns(&ddl);
    let target_keys = parse_target_keys(&ddl);
    // 裸 NUMBER 带着任务定义里配好的比较形状到达 sink，其余列原样。
    let sink_source_columns = source_columns
        .iter()
        .cloned()
        .map(|mut column| {
            if column.name == "N_RAW" {
                column.precision = Some(12);
                column.scale = Some(2);
            }
            column
        })
        .collect::<Vec<_>>();

    assert_eq!(
        precheck_with_primary_key(
            "T_GENERATED",
            &key(),
            &sink_source_columns,
            &target_columns,
            &target_keys,
        ),
        [],
        "{ddl}"
    );
}

#[test]
fn sink_rejects_source_shapes_marked_unsupported() {
    let source_columns = vec![
        source_column_with_support(
            "N_TOO_WIDE",
            "NUMBER",
            Some(38),
            Some(-30),
            None,
            ColumnSupport::Unsupported,
        ),
        source_column_with_support(
            "N_TOO_SCALE",
            "NUMBER",
            Some(4),
            Some(35),
            None,
            ColumnSupport::Unsupported,
        ),
        timestamp_column_with_support("TS_TOO_PRECISE", 9, ColumnSupport::Unsupported),
        source_column_with_support(
            "PAYLOAD",
            "CLOB",
            None,
            None,
            None,
            ColumnSupport::Unsupported,
        ),
    ];
    let target_columns = vec![
        target_column("N_TOO_WIDE", "decimal(68,0)", "decimal", Some(68), Some(0)),
        target_column(
            "N_TOO_SCALE",
            "decimal(35,35)",
            "decimal",
            Some(35),
            Some(35),
        ),
        target_column("TS_TOO_PRECISE", "datetime(6)", "datetime", None, None),
        target_column("PAYLOAD", "text", "text", None, None),
    ];
    let issues =
        precheck_with_primary_key("T_GENERATED", &[], &source_columns, &target_columns, &[]);

    for column in &source_columns {
        assert_eq!(column.support, Some(ColumnSupport::Unsupported));
        assert!(
            issues.iter().any(|issue| issue.column == column.name),
            "missing {} in {issues:?}",
            column.name
        );
    }
}

/// 这份夹具一律以 `D_BIZ` 作主键：每个用例的列集合里它都在。
fn key() -> Vec<String> {
    vec!["D_BIZ".to_owned()]
}

/// 从生成的建表语句里读回唯一约束——目标端真有什么，就喂什么给预检。
fn parse_target_keys(ddl: &str) -> Vec<TargetKey> {
    let statement = Parser::parse_sql(&MySqlDialect {}, ddl)
        .unwrap()
        .pop()
        .unwrap();
    let Statement::CreateTable { constraints, .. } = statement else {
        panic!("generated SQL was not CREATE TABLE: {ddl}");
    };

    constraints
        .into_iter()
        .filter_map(|constraint| match constraint {
            TableConstraint::PrimaryKey { columns, .. } => Some(TargetKey {
                name: "PRIMARY".to_owned(),
                columns: columns.into_iter().map(|column| column.value).collect(),
            }),
            TableConstraint::Unique { name, columns, .. } => Some(TargetKey {
                name: name.map(|name| name.value).unwrap_or_default(),
                columns: columns.into_iter().map(|column| column.value).collect(),
            }),
            _ => None,
        })
        .collect()
}

fn source_columns_for(case: &Case) -> Vec<Vec<DdlSourceColumn>> {
    let concrete_types: &[&str] = match case.data_type.as_str() {
        "NUMBER" => &["NUMBER"],
        "VARCHAR2" => &["VARCHAR2"],
        "DATE" => &["DATE"],
        "*" => &["NUMBER", "VARCHAR2", "DATE"],
        other => panic!("unexpected M1 fixture type {other}"),
    };

    concrete_types
        .iter()
        .map(|data_type| {
            let mut columns = vec![match *data_type {
                "NUMBER" => source_column("VALUE", "NUMBER", Some(38), Some(10), None),
                "VARCHAR2" => source_column("VALUE", "VARCHAR2", None, None, Some(50)),
                "DATE" => source_column("D_BIZ", "DATE", None, None, None),
                _ => unreachable!(),
            }];
            if *data_type != "DATE" {
                columns.push(source_column("D_BIZ", "DATE", None, None, None));
            }
            columns
        })
        .collect()
}

fn parse_target_columns(ddl: &str) -> Vec<TargetColumn> {
    let statement = Parser::parse_sql(&MySqlDialect {}, ddl)
        .unwrap()
        .pop()
        .unwrap();
    let Statement::CreateTable {
        columns,
        default_charset,
        ..
    } = statement
    else {
        panic!("generated SQL was not CREATE TABLE: {ddl}");
    };

    columns
        .into_iter()
        .enumerate()
        .map(|(index, column)| {
            let nullable = column
                .options
                .iter()
                .any(|option| option.option == ColumnOption::Null)
                && !column
                    .options
                    .iter()
                    .any(|option| option.option == ColumnOption::NotNull);
            let column_type = column.data_type.to_string();
            let (data_type, precision, scale, length, datetime_precision, character_set) =
                match column.data_type {
                    DataType::Decimal(ExactNumberInfo::PrecisionAndScale(precision, scale)) => {
                        ("decimal", Some(precision), Some(scale), None, None, None)
                    }
                    DataType::Varchar(Some(CharacterLength::IntegerLength { length, .. })) => (
                        "varchar",
                        None,
                        None,
                        Some(length),
                        None,
                        default_charset.clone(),
                    ),
                    DataType::Datetime(precision) => {
                        ("datetime", None, None, None, precision, None)
                    }
                    other => panic!("unexpected generated target type {other}"),
                };
            TargetColumn {
                name: column.name.value,
                column_type,
                data_type: data_type.to_owned(),
                precision,
                scale,
                length,
                datetime_precision,
                nullable,
                character_set,
                ordinal: u64::try_from(index + 1).unwrap(),
            }
        })
        .collect()
}

fn source_column(
    name: &str,
    data_type: &str,
    precision: Option<i64>,
    scale: Option<i64>,
    length: Option<u64>,
) -> DdlSourceColumn {
    source_column_with_support(name, data_type, precision, scale, length, ColumnSupport::Ok)
}

fn source_column_with_support(
    name: &str,
    data_type: &str,
    precision: Option<i64>,
    scale: Option<i64>,
    length: Option<u64>,
    support: ColumnSupport,
) -> DdlSourceColumn {
    DdlSourceColumn {
        name: name.to_owned(),
        data_type: data_type.to_owned(),
        precision,
        scale,
        length,
        fsp: None,
        support: Some(support),
    }
}

fn timestamp_column_with_support(name: &str, fsp: u32, support: ColumnSupport) -> DdlSourceColumn {
    DdlSourceColumn {
        name: name.to_owned(),
        data_type: "TIMESTAMP".to_owned(),
        precision: None,
        scale: None,
        length: None,
        fsp: Some(fsp),
        support: Some(support),
    }
}

fn target_column(
    name: &str,
    column_type: &str,
    data_type: &str,
    precision: Option<u64>,
    scale: Option<u64>,
) -> TargetColumn {
    TargetColumn {
        name: name.to_owned(),
        column_type: column_type.to_owned(),
        data_type: data_type.to_owned(),
        precision,
        scale,
        length: None,
        datetime_precision: (data_type == "datetime").then_some(6),
        nullable: true,
        character_set: None,
        ordinal: 1,
    }
}
