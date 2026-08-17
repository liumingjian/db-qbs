use db_qbs_sink::{precheck, SourceColumn as SinkSourceColumn, TargetColumn};
use db_qbs_source::{generate_target_ddl, ColumnSupport, SourceColumn as DdlSourceColumn};
use serde::Deserialize;
use sqlparser::ast::{CharacterLength, ColumnOption, DataType, ExactNumberInfo, Statement};
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
            let ddl = generate_target_ddl(&source_columns, "T_GENERATED", "D_BIZ", None).unwrap();
            let target_columns = parse_target_columns(&ddl);
            let sink_source_columns = source_columns
                .iter()
                .map(|column| SinkSourceColumn {
                    name: column.name.clone(),
                    data_type: column.data_type.clone(),
                    precision: column.precision,
                    scale: column.scale,
                    length: column.length,
                    fsp: column.fsp,
                    // sink 不得读 `support` 做判定（ADR-0010 增补二 §2），故不搬运。
                    support: None,
                })
                .collect::<Vec<_>>();

            assert_eq!(
                precheck("T_GENERATED", &sink_source_columns, &target_columns),
                [],
                "{}: {ddl}",
                case.id
            );
        }
    }
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
    DdlSourceColumn {
        name: name.to_owned(),
        data_type: data_type.to_owned(),
        precision,
        scale,
        length,
        fsp: None,
        support: Some(ColumnSupport::Ok),
    }
}
