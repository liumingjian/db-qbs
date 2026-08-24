//! 构建器两件事：元数据查询，与「结构化规格 → 源端 SQL」的生成（ADR-0036 §1）。
//!
//! SQL 形状预检的六条规则已随 ADR-0036 §5 整段取消，原来那个「生成的 SQL 必须过六条」的
//! 断言随之失去对象。顶上来的判据是**生成即合法**：投影恒是 `a.<源列> AS <目标字段>`，值恒是绑定变量，
//! 标识符恒过白名单——这些由生成器结构性保证，下面逐条钉住。

use db_qbs_source::{
    builder_column_query, builder_dblink_query, builder_table_query, validate_builder_dblink,
    validate_source_sql, ColumnMapping, Comparison, Condition, Direction, OrderTerm, RunParams,
    TaskSpec, ValueSource, ValueType,
};

/// 恒等映射：目标字段预填成源列名（ADR-0038 §2）。改形状之前的规格就是这一份。
fn identity(column: &str) -> ColumnMapping {
    ColumnMapping {
        source: column.to_owned(),
        target: column.to_owned(),
    }
}

fn spec() -> TaskSpec {
    TaskSpec {
        source_sql: None,
        dblink: Some("FA".to_owned()),
        owner: "HTBR45".to_owned(),
        table: "T_R_FR_ASTSTAT".to_owned(),
        target_table: "T_POSITION".to_owned(),
        columns: vec![identity("N_VA_PRICE"), identity("D_BIZ")],
        primary_key: vec!["D_BIZ".to_owned()],
        conditions: Vec::new(),
        order_by: Vec::new(),
    }
}

fn condition(column: &str, parameter: &str, value_type: ValueType) -> Condition {
    Condition {
        column: column.to_owned(),
        operator: Comparison::Eq,
        value_type,
        parameter: parameter.to_owned(),
        value_source: ValueSource::Runtime,
        constant: String::new(),
    }
}

#[test]
fn a_spec_without_conditions_reads_the_whole_table() {
    let spec = spec();
    spec.validate().unwrap();

    // 一条条件都没有就是整表取数（ADR-0035 §3 明许）。量级风险归台架去证，不在这里挡。
    assert_eq!(
        spec.source_sql(),
        concat!(
            "SELECT a.N_VA_PRICE AS N_VA_PRICE,\n",
            "       a.D_BIZ AS D_BIZ\n",
            "  FROM HTBR45.T_R_FR_ASTSTAT@FA a"
        )
    );
    assert!(spec.runtime_parameters().is_empty());
    assert!(spec.bindings(&RunParams::new()).unwrap().is_empty());
}

/// 自定义 SQL 外面要再套一层投影，**不是**原样执行。
///
/// 理由在搬运链路那头：`transfer.rs` 把 `source.columns()`——执行语句的结果列——原样交给
/// sink，所以结果列名就是目标列名。少了这一层，勾选与目标字段改名两件事都落不了地。
/// 内层照旧不许被追加条件或排序（那两样只能由用户写进 SQL）。
#[test]
fn custom_select_is_wrapped_in_a_projection_and_gets_no_table_conditions() {
    let mut spec = spec();
    spec.source_sql = Some(
        "SELECT a.N_VA_PRICE, a.D_BIZ\n  FROM APP.T_CUSTOMER@FA a\n WHERE a.ACTIVE = 1;"
            .to_owned(),
    );
    spec.dblink = None;
    spec.conditions.clear();
    spec.order_by.clear();

    spec.validate().unwrap();
    assert_eq!(
        spec.source_sql(),
        concat!(
            "SELECT q.N_VA_PRICE AS N_VA_PRICE,\n",
            "       q.D_BIZ AS D_BIZ\n",
            "  FROM (\n",
            "         SELECT a.N_VA_PRICE, a.D_BIZ\n",
            "           FROM APP.T_CUSTOMER@FA a\n",
            "          WHERE a.ACTIVE = 1\n",
            "       ) q"
        )
    );
}

/// 没勾的列不进投影——这正是「自定义 SQL 也能筛列」在 SQL 上的全部痕迹。
/// 内层原文一个字节不动，用户写的 `SELECT *` 仍然是 `SELECT *`。
#[test]
fn unselected_columns_are_dropped_from_the_custom_sql_projection() {
    let mut spec = spec();
    spec.source_sql = Some("SELECT * FROM APP.T_CUSTOMER@FA".to_owned());
    spec.dblink = None;
    spec.conditions.clear();
    spec.order_by.clear();
    spec.columns = vec![identity("D_BIZ")];
    spec.primary_key = vec!["D_BIZ".to_owned()];

    spec.validate().unwrap();
    assert_eq!(
        spec.source_sql(),
        concat!(
            "SELECT q.D_BIZ AS D_BIZ\n",
            "  FROM (\n",
            "         SELECT * FROM APP.T_CUSTOMER@FA\n",
            "       ) q"
        )
    );
}

/// 内层把结果列别名成了**带引号的小写**（`AS "id"`）时，外层的引用必须也带引号。
/// 不带引号的引用被 Oracle 折成大写，`q.id` → `Q.ID`，打不中那一列——
/// ORA-00904，而且只在真跑的时候才炸（ADR-0045 §3）。
#[test]
fn a_lowercase_result_column_is_referenced_with_quotes() {
    let mut spec = spec();
    spec.source_sql = Some("SELECT ID AS \"id\" FROM APP.T_CUSTOMER@FA".to_owned());
    spec.dblink = None;
    spec.conditions.clear();
    spec.order_by.clear();
    spec.columns = vec![ColumnMapping {
        source: "id".to_owned(),
        target: "BIZ_ID".to_owned(),
    }];
    spec.primary_key = vec!["BIZ_ID".to_owned()];

    spec.validate().unwrap();
    assert!(
        spec.source_sql().contains("q.\"id\" AS BIZ_ID"),
        "小写结果列必须带引号引用，实际生成：\n{}",
        spec.source_sql()
    );
}

/// 反过来：全大写的列名**不加引号**——绝大多数任务走这一支，
/// 加引号会把每一条既有任务的生成文本都改掉，收益为零（ADR-0045 §3 否掉「一律加引号」）。
#[test]
fn an_uppercase_result_column_is_referenced_without_quotes() {
    let mut spec = spec();
    spec.source_sql = Some("SELECT * FROM APP.T_CUSTOMER@FA".to_owned());
    spec.dblink = None;
    spec.conditions.clear();
    spec.order_by.clear();

    spec.validate().unwrap();
    let sql = spec.source_sql();
    assert!(sql.contains("q.N_VA_PRICE AS N_VA_PRICE"), "{sql}");
    assert!(!sql.contains('"'), "全大写列名不该出现引号：\n{sql}");
}

/// 按表选择那一路同样遵守这条规则，两条路径共用一份投影，形状不会漂。
#[test]
fn the_table_path_keeps_unquoted_uppercase_references() {
    let spec = spec();
    spec.validate().unwrap();
    assert!(spec.source_sql().contains("a.N_VA_PRICE AS N_VA_PRICE"));
    assert!(!spec.source_sql().contains('"'));
}

/// 目标字段改名在自定义 SQL 模式下同样要生效——改之前它被静默忽略。
#[test]
fn a_renamed_target_field_reaches_the_custom_sql_projection() {
    let mut spec = spec();
    spec.source_sql = Some("SELECT * FROM APP.T_CUSTOMER@FA".to_owned());
    spec.dblink = None;
    spec.conditions.clear();
    spec.order_by.clear();
    spec.columns = vec![ColumnMapping {
        source: "D_BIZ".to_owned(),
        target: "BIZ_DATE".to_owned(),
    }];
    spec.primary_key = vec!["BIZ_DATE".to_owned()];

    spec.validate().unwrap();
    assert!(spec.source_sql().contains("q.D_BIZ AS BIZ_DATE"));
}

#[test]
fn custom_source_sql_only_accepts_one_select_statement() {
    assert_eq!(validate_source_sql("SELECT 1"), Ok(()));
    assert_eq!(validate_source_sql("SELECT 1;"), Ok(()));
    assert!(validate_source_sql("UPDATE T SET C = 1").is_err());
    assert!(validate_source_sql("SELECT 1; SELECT 2").is_err());
}

#[test]
fn a_renamed_column_shows_up_as_the_alias_and_nothing_else_moves() {
    // ADR-0038 §1：映射就是投影的别名，搬运语义一个字节不变。所以「改了目标字段」
    // 在 SQL 上的全部痕迹就是 `AS` 右边那个词——WHERE / ORDER BY 仍然按**源列名**走
    // （条件挑的是源表的列，改目标字段名不该动它们）。
    let mut renamed = spec();
    renamed.columns = vec![
        ColumnMapping {
            source: "C_NAME".to_owned(),
            target: "CUST_NAME".to_owned(),
        },
        identity("D_BIZ"),
    ];
    renamed.primary_key = vec!["CUST_NAME".to_owned()];
    renamed.conditions = vec![condition("D_BIZ", "d_biz", ValueType::Date)];
    renamed.validate().unwrap();

    assert_eq!(
        renamed.source_sql(),
        concat!(
            "SELECT a.C_NAME AS CUST_NAME,\n",
            "       a.D_BIZ AS D_BIZ\n",
            "  FROM HTBR45.T_R_FR_ASTSTAT@FA a\n",
            " WHERE a.D_BIZ = TO_DATE(:d_biz,'YYYY-MM-DD')"
        )
    );
}

#[test]
fn each_value_type_renders_its_own_binding_form() {
    // DATE 列拿字符串裸比会走 Oracle 隐式转换、吃 NLS_DATE_FORMAT，换个会话换个语义，
    // 所以每条条件自带 value_type，三种类型各有各的写法。
    let mut spec = spec();
    spec.conditions = vec![
        Condition {
            operator: Comparison::Gt,
            ..condition("D_BIZ", "from_date", ValueType::Date)
        },
        Condition {
            operator: Comparison::Lt,
            ..condition("N_VA_PRICE", "cap", ValueType::Number)
        },
        condition("C_CODE", "code", ValueType::Text),
    ];
    spec.validate().unwrap();

    assert_eq!(
        spec.source_sql(),
        concat!(
            "SELECT a.N_VA_PRICE AS N_VA_PRICE,\n",
            "       a.D_BIZ AS D_BIZ\n",
            "  FROM HTBR45.T_R_FR_ASTSTAT@FA a\n",
            " WHERE a.D_BIZ > TO_DATE(:from_date,'YYYY-MM-DD')\n",
            "   AND a.N_VA_PRICE < TO_NUMBER(:cap)\n",
            "   AND a.C_CODE = :code"
        )
    );
}

#[test]
fn order_terms_land_after_the_predicates() {
    let mut spec = spec();
    spec.conditions = vec![condition("D_BIZ", "d_biz", ValueType::Date)];
    spec.order_by = vec![
        OrderTerm {
            column: "D_BIZ".to_owned(),
            direction: Direction::Desc,
        },
        OrderTerm {
            column: "N_VA_PRICE".to_owned(),
            direction: Direction::Asc,
        },
    ];
    spec.validate().unwrap();

    assert!(spec
        .source_sql()
        .ends_with(" ORDER BY a.D_BIZ DESC, a.N_VA_PRICE ASC"));
}

#[test]
fn constants_bind_too_and_stay_out_of_the_run_parameter_set() {
    // 常量也走绑定变量：理由不是防注入，是转义正确性（ADR-0011 §2「不发明第二套转义」）。
    // 但常量每次都一样，进「运行参数集」不增加任何区分度，所以互斥键里没有它。
    let mut spec = spec();
    spec.conditions = vec![
        Condition {
            value_source: ValueSource::Constant,
            constant: "CNY".to_owned(),
            ..condition("C_CURRENCY", "currency", ValueType::Text)
        },
        condition("D_BIZ", "d_biz", ValueType::Date),
    ];
    spec.validate().unwrap();

    assert!(spec.source_sql().contains("a.C_CURRENCY = :currency"));
    assert_eq!(
        spec.runtime_parameters()
            .iter()
            .map(|condition| condition.parameter.as_str())
            .collect::<Vec<_>>(),
        vec!["d_biz"]
    );

    let mut run_params = RunParams::new();
    run_params.insert("d_biz".to_owned(), "2026-08-18".to_owned());
    assert_eq!(
        spec.bindings(&run_params).unwrap(),
        vec![
            ("currency".to_owned(), "CNY".to_owned()),
            ("d_biz".to_owned(), "2026-08-18".to_owned()),
        ]
    );

    // 少填一个运行参数就不许开跑——报的是参数名，不是「参数不全」。
    assert_eq!(
        spec.bindings(&RunParams::new()).unwrap_err(),
        "运行参数 d_biz 未取值"
    );
}

#[test]
fn describe_bindings_cover_every_parameter_with_a_typed_dummy() {
    let mut spec = spec();
    spec.conditions = vec![
        condition("D_BIZ", "d_biz", ValueType::Date),
        condition("N_VA_PRICE", "cap", ValueType::Number),
        condition("C_CODE", "code", ValueType::Text),
    ];

    assert_eq!(
        spec.describe_bindings(),
        vec![
            ("d_biz".to_owned(), "1970-01-01".to_owned()),
            ("cap".to_owned(), "0".to_owned()),
            ("code".to_owned(), String::new()),
        ]
    );
}

#[test]
fn validation_refuses_the_six_ways_a_spec_can_be_unusable() {
    let mut no_key = spec();
    no_key.primary_key.clear();
    assert_eq!(
        no_key.validate().unwrap_err(),
        "主键必选：至少要勾一列作为 upsert 的去重键"
    );

    let mut key_outside = spec();
    key_outside.primary_key = vec!["ID".to_owned()];
    assert_eq!(
        key_outside.validate().unwrap_err(),
        "主键列 ID 不在选中的列里"
    );

    // 主键存的是**目标字段**：改过名的列上，源列名才是那个「不在选中的列里」的。
    let mut key_by_source_name = spec();
    key_by_source_name.columns = vec![ColumnMapping {
        source: "C_NAME".to_owned(),
        target: "CUST_NAME".to_owned(),
    }];
    key_by_source_name.primary_key = vec!["C_NAME".to_owned()];
    assert_eq!(
        key_by_source_name.validate().unwrap_err(),
        "主键列 C_NAME 不在选中的列里"
    );
    key_by_source_name.primary_key = vec!["CUST_NAME".to_owned()];
    key_by_source_name.validate().unwrap();

    // 两列映到同一个目标字段会生成两个同名别名，按名字对齐时后一列静默盖掉前一列。
    let mut duplicate_target = spec();
    duplicate_target.columns = vec![
        identity("N_VA_PRICE"),
        ColumnMapping {
            source: "D_BIZ".to_owned(),
            target: "N_VA_PRICE".to_owned(),
        },
    ];
    assert_eq!(
        duplicate_target.validate().unwrap_err(),
        "目标字段 N_VA_PRICE 重复"
    );

    let mut duplicate_parameter = spec();
    duplicate_parameter.conditions = vec![
        condition("D_BIZ", "d_biz", ValueType::Date),
        condition("N_VA_PRICE", "D_BIZ", ValueType::Number),
    ];
    assert_eq!(
        duplicate_parameter.validate().unwrap_err(),
        "参数名 D_BIZ 重复"
    );

    let mut runtime_with_constant = spec();
    runtime_with_constant.conditions = vec![Condition {
        constant: "2026-08-18".to_owned(),
        ..condition("D_BIZ", "d_biz", ValueType::Date)
    }];
    assert_eq!(
        runtime_with_constant.validate().unwrap_err(),
        "条件 d_biz 标了运行时填，不能同时写死常量值"
    );
}

#[test]
fn identifiers_are_the_only_thing_not_bound_so_they_are_whitelisted() {
    // 值走绑定变量，标识符不能——标识符这一侧只能靠白名单式校验挡住拼串。
    let mut injected = spec();
    injected.table = "T_R_FR_ASTSTAT WHERE 1=1 --".to_owned();
    assert!(injected.validate().is_err());

    let mut injected_condition = spec();
    injected_condition.conditions = vec![condition("D_BIZ) OR (1=1", "d_biz", ValueType::Date)];
    assert!(injected_condition.validate().is_err());

    let mut injected_order = spec();
    injected_order.order_by = vec![OrderTerm {
        column: "D_BIZ; DROP TABLE T".to_owned(),
        direction: Direction::Asc,
    }];
    assert!(injected_order.validate().is_err());
}

#[test]
fn metadata_queries_use_only_a_validated_dblink_suffix() {
    assert_eq!(
        builder_dblink_query(),
        "SELECT DB_LINK FROM USER_DB_LINKS ORDER BY DB_LINK"
    );
    assert_eq!(validate_builder_dblink(Some("fa")), Ok(()));
    assert_eq!(
        builder_table_query(Some("fa")),
        Ok("SELECT OWNER, TABLE_NAME FROM ALL_TABLES@FA ORDER BY OWNER, TABLE_NAME".to_owned())
    );
    assert_eq!(
        builder_column_query(None),
        Ok(concat!(
            "SELECT COLUMN_NAME, DATA_TYPE, DATA_PRECISION, DATA_SCALE, CHAR_LENGTH, NULLABLE ",
            "FROM ALL_TAB_COLUMNS WHERE OWNER = :owner AND TABLE_NAME = :table_name ",
            "ORDER BY COLUMN_ID"
        )
        .to_owned())
    );
    assert!(validate_builder_dblink(Some("FA WHERE 1=1")).is_err());
    assert!(builder_table_query(Some("FA WHERE 1=1")).is_err());
}
