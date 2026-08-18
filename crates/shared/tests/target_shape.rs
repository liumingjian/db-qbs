//! ADR-0030 §1 九行形态的推导：每种形态与每条边界各钉一份（#125 Q9）。
//!
//! 这里只钉**推导**——「怎么比」是判定式，归 sink，不在本套件的射程内。
//! 跨端「生成的表喂回预检必过」那条回路在 `crates/sink/tests/target_ddl_drift.rs`。

use db_qbs_shared::{
    classify_column, column_support, derive_number_shape, is_business_date_column,
    is_supported_decimal_shape, ColumnShape, ColumnSupport, ShapeRejection, SourceColumn,
    TargetShape,
};

fn column(
    data_type: &str,
    precision: Option<i64>,
    scale: Option<i64>,
    length: Option<u64>,
    fsp: Option<u32>,
) -> SourceColumn {
    SourceColumn {
        name: "C_UNDER_TEST".to_owned(),
        data_type: data_type.to_owned(),
        precision,
        scale,
        length,
        fsp,
        support: None,
    }
}

fn number(precision: Option<i64>, scale: Option<i64>) -> SourceColumn {
    column("NUMBER", precision, scale, None, None)
}

fn decimal(precision: i64, scale: i64) -> ColumnShape {
    ColumnShape::Resolved(TargetShape::Decimal { precision, scale })
}

#[test]
fn every_whitelisted_shape_derives_its_target_shape() {
    let cases: Vec<(&str, SourceColumn, ColumnShape)> = vec![
        // 形态 1——常规 `0 <= s <= p`。
        ("NUMBER(18,4)", number(Some(18), Some(4)), decimal(18, 4)),
        ("NUMBER(38,0)", number(Some(38), Some(0)), decimal(38, 0)),
        // 形态 2——纯小数 `s > p`，推导取 `(s,s)`。
        ("NUMBER(4,6)", number(Some(4), Some(6)), decimal(6, 6)),
        // 形态 3——负标度，推导取 `(p+|s|,0)`。
        ("NUMBER(10,-5)", number(Some(10), Some(-5)), decimal(15, 0)),
        // 形态 4——裸 NUMBER / 数值表达式列：形状等任务定义配。
        ("裸 NUMBER", number(None, None), ColumnShape::NeedsPrecision),
        // 形态 5/6/7——字符族一律推 `VARCHAR(n)`（`CHAR` 绝不照抄，ADR-0030 §5）。
        (
            "VARCHAR2(50)",
            column("VARCHAR2", None, None, Some(50), None),
            ColumnShape::Resolved(TargetShape::Varchar { length: 50 }),
        ),
        (
            "NVARCHAR2(20)",
            column("NVARCHAR2", None, None, Some(20), None),
            ColumnShape::Resolved(TargetShape::Varchar { length: 20 }),
        ),
        (
            "CHAR(30)",
            column("CHAR", None, None, Some(30), None),
            ColumnShape::Resolved(TargetShape::Varchar { length: 30 }),
        ),
        (
            "NCHAR(10)",
            column("NCHAR", None, None, Some(10), None),
            ColumnShape::Resolved(TargetShape::Varchar { length: 10 }),
        ),
        // 形态 8——DATE 推 `DATETIME(0)`。
        (
            "DATE",
            column("DATE", None, None, None, None),
            ColumnShape::Resolved(TargetShape::Datetime { fsp: 0 }),
        ),
        // 形态 9——TIMESTAMP(n) 恒推 `DATETIME(6)`，不随 n 走（ADR-0030 §3）。
        (
            "TIMESTAMP(0)",
            column("TIMESTAMP", None, None, None, Some(0)),
            ColumnShape::Resolved(TargetShape::Datetime { fsp: 6 }),
        ),
        (
            "TIMESTAMP(3)",
            column("TIMESTAMP", None, None, None, Some(3)),
            ColumnShape::Resolved(TargetShape::Datetime { fsp: 6 }),
        ),
        (
            "TIMESTAMP(6)",
            column("TIMESTAMP", None, None, None, Some(6)),
            ColumnShape::Resolved(TargetShape::Datetime { fsp: 6 }),
        ),
    ];

    for (label, source, expected) in cases {
        assert_eq!(classify_column(&source), expected, "{label}");
    }
}

#[test]
fn boundaries_and_rejections_are_classified_by_reason() {
    let cases: Vec<(&str, SourceColumn, ShapeRejection)> = vec![
        // ADR-0030 §6 / ADR-0027 A5：判的是**推导形状**，源侧一条判据都不命中也照样拒。
        (
            "NUMBER(38,-30) 推出 DECIMAL(68,0)",
            number(Some(38), Some(-30)),
            ShapeRejection::DecimalShapeUnrepresentable {
                precision: 68,
                scale: 0,
            },
        ),
        (
            "NUMBER(4,35) 推出 DECIMAL(35,35)",
            number(Some(4), Some(35)),
            ShapeRejection::DecimalShapeUnrepresentable {
                precision: 35,
                scale: 35,
            },
        ),
        (
            "NUMBER(66,0) 整数位越界",
            number(Some(66), Some(0)),
            ShapeRejection::DecimalShapeUnrepresentable {
                precision: 66,
                scale: 0,
            },
        ),
        (
            "NUMBER 精度只有一半",
            number(Some(38), None),
            ShapeRejection::NumberPrecisionIncomplete,
        ),
        (
            "字符表达式列没有 length",
            column("VARCHAR2", None, None, None, None),
            ShapeRejection::CharacterLengthMissing,
        ),
        (
            "TIMESTAMP(7) 超出规范形式的 6 位",
            column("TIMESTAMP", None, None, None, Some(7)),
            ShapeRejection::TimestampFspTooPrecise { fsp: 7 },
        ),
        (
            "TIMESTAMP 没带 fsp",
            column("TIMESTAMP", None, None, None, None),
            ShapeRejection::TimestampFspMissing,
        ),
        (
            "CLOB 在白名单外",
            column("CLOB", None, None, None, None),
            ShapeRejection::TypeNotWhitelisted,
        ),
        (
            "BINARY_DOUBLE 在白名单外",
            column("BINARY_DOUBLE", None, None, None, None),
            ShapeRejection::TypeNotWhitelisted,
        ),
    ];

    for (label, source, expected) in cases {
        assert_eq!(
            classify_column(&source),
            ColumnShape::Rejected(expected),
            "{label}"
        );
    }
}

#[test]
fn decimal_representability_boundary_is_65_by_30() {
    assert_eq!(
        classify_column(&number(Some(65), Some(30))),
        decimal(65, 30)
    );
    assert!(is_supported_decimal_shape(65, 30));
    assert!(!is_supported_decimal_shape(66, 0));
    assert!(!is_supported_decimal_shape(65, 31));
    // 标度不得大于精度——推导出的形状永远满足它，直接判也要守住。
    assert!(!is_supported_decimal_shape(4, 6));
    // 精度 0 不是合法形状：`NUMBER(0,_)` 在源端就归到裸 NUMBER 那一路。
    assert!(!is_supported_decimal_shape(0, 0));
}

#[test]
fn number_shape_derivation_covers_the_three_branches_and_the_upper_bound() {
    assert_eq!(derive_number_shape(18, 4), (18, 4));
    assert_eq!(derive_number_shape(4, 6), (6, 6));
    assert_eq!(derive_number_shape(10, -5), (15, 0));
    // 注释里那条上界：Oracle 精度上限 38、标度下限 −84，推导结果最大 122。
    assert_eq!(derive_number_shape(38, -84), (122, 0));
    // sink 收的是网线上的报文字段，`(p,s)` 是对端给的任意 i64：饱和，不 panic，
    // 饱和出来的形状照样判「装不进 DECIMAL(65,30)」。
    assert_eq!(derive_number_shape(i64::MAX, i64::MIN), (i64::MAX, 0));
    assert!(!is_supported_decimal_shape(i64::MAX, 0));
    assert_eq!(
        classify_column(&number(Some(i64::MAX), Some(i64::MIN))),
        ColumnShape::Rejected(ShapeRejection::DecimalShapeUnrepresentable {
            precision: i64::MAX,
            scale: 0,
        })
    );
}

#[test]
fn shapes_render_as_mysql_type_text() {
    // 两端的文字都从这里渲染：source 的建表 SQL 与 sink 的建议。
    assert_eq!(
        TargetShape::Decimal {
            precision: 18,
            scale: 4
        }
        .to_string(),
        "DECIMAL(18,4)"
    );
    assert_eq!(
        TargetShape::Varchar { length: 50 }.to_string(),
        "VARCHAR(50)"
    );
    assert_eq!(TargetShape::Datetime { fsp: 0 }.to_string(), "DATETIME(0)");
    assert_eq!(TargetShape::Datetime { fsp: 6 }.to_string(), "DATETIME(6)");
}

#[test]
fn business_date_column_takes_date_and_timestamp_up_to_six() {
    assert!(is_business_date_column(&column(
        "DATE", None, None, None, None
    )));
    assert!(is_business_date_column(&column(
        "TIMESTAMP",
        None,
        None,
        None,
        Some(6)
    )));
    assert!(!is_business_date_column(&column(
        "TIMESTAMP",
        None,
        None,
        None,
        Some(7)
    )));
    assert!(!is_business_date_column(&column(
        "TIMESTAMP",
        None,
        None,
        None,
        None
    )));
    // 字符族与 NUMBER 族一律拒做业务日期列（ADR-0027 A6）。
    assert!(!is_business_date_column(&column(
        "VARCHAR2",
        None,
        None,
        Some(8),
        None
    )));
    assert!(!is_business_date_column(&number(Some(8), Some(0))));
}

#[test]
fn support_tier_is_the_three_outcomes_of_the_same_derivation() {
    assert_eq!(
        column_support(classify_column(&number(Some(18), Some(4)))),
        ColumnSupport::Ok
    );
    assert_eq!(
        column_support(classify_column(&number(None, None))),
        ColumnSupport::NeedsPrecision
    );
    assert_eq!(
        column_support(classify_column(&number(Some(38), Some(-30)))),
        ColumnSupport::Unsupported
    );
    assert_eq!(
        column_support(classify_column(&column("CLOB", None, None, None, None))),
        ColumnSupport::Unsupported
    );
}

#[test]
fn type_names_are_matched_case_insensitively() {
    assert_eq!(classify_column(&number(Some(9), Some(2))), decimal(9, 2));
    let mut lowercase = number(Some(9), Some(2));
    lowercase.data_type = "number".to_owned();
    assert_eq!(classify_column(&lowercase), decimal(9, 2));

    let mut mixed = column("Timestamp", None, None, None, Some(2));
    assert_eq!(
        classify_column(&mixed),
        ColumnShape::Resolved(TargetShape::Datetime { fsp: 6 })
    );
    mixed.data_type = "nChar".to_owned();
    mixed.fsp = None;
    mixed.length = Some(4);
    assert_eq!(
        classify_column(&mixed),
        ColumnShape::Resolved(TargetShape::Varchar { length: 4 })
    );
}
