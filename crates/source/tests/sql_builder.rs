//! 构建器两件事：元数据查询，与「结构化规格 → 源端 SQL」的生成。
//!
//! 判据是**生成即合法**：投影恒是 `a.<源列> AS <目标字段>`，标识符恒过白名单——
//! 这些由生成器结构性保证，下面逐条钉住。
//!
//! **过滤是个例外，而且是刻意的**：WHERE 片段是用户写的一段自由文本，原样拼进去，
//! 不解析也不改写。所以这里钉的不是「它合法」，而是「它一个字符不差地到了生成的 SQL 里」。

use db_qbs_source::{
    builder_column_query, builder_dblink_query, builder_table_query, validate_builder_dblink,
    validate_source_sql, ColumnMapping, TaskSpec, WriteMode, WriteStatement,
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
        write_mode: WriteMode::Append,
        schedule_cron: None,
        schedule_enabled: false,
        columns: vec![identity("N_VA_PRICE"), identity("D_BIZ")],
        primary_key: vec!["D_BIZ".to_owned()],
        where_clause: None,
    }
}

#[test]
fn a_spec_without_a_where_clause_reads_the_whole_table() {
    let spec = spec();
    spec.validate().unwrap();

    // 过滤留空就是整表取数。量级风险归台架去证，不在这里挡。
    assert_eq!(
        spec.source_sql(),
        concat!(
            "SELECT a.N_VA_PRICE AS N_VA_PRICE,\n",
            "       a.D_BIZ AS D_BIZ\n",
            "  FROM HTBR45.T_R_FR_ASTSTAT@FA a"
        )
    );
}

/// 本票的核心断言：**文本框里那段字，一个字符不差地到了 `WHERE` 后面**。
#[test]
fn the_where_text_reaches_the_generated_sql_verbatim() {
    let mut spec = spec();
    spec.where_clause = Some("D_BIZ >= DATE '2026-08-01' AND STATUS IN ('OK','WARN')".to_owned());
    spec.validate().unwrap();

    assert_eq!(
        spec.source_sql(),
        concat!(
            "SELECT a.N_VA_PRICE AS N_VA_PRICE,\n",
            "       a.D_BIZ AS D_BIZ\n",
            "  FROM HTBR45.T_R_FR_ASTSTAT@FA a\n",
            " WHERE D_BIZ >= DATE '2026-08-01' AND STATUS IN ('OK','WARN')"
        )
    );
}

/// 那些四格表单永远表达不出来的形态——`>=`、`IN`、`BETWEEN`、`LIKE`、子查询、
/// 函数调用——现在**没有一条需要特殊照顾**：它们只是文本。
#[test]
fn the_shapes_the_four_slot_form_could_never_express_are_just_text_now() {
    for clause in [
        "N_VA_PRICE BETWEEN 1 AND 100",
        "C_CODE LIKE 'SH%'",
        "TRUNC(D_BIZ) = TRUNC(SYSDATE) - 1",
        "ID IN (SELECT ID FROM APP.WHITELIST)",
        "(A = 1 OR B = 2) AND C IS NOT NULL",
    ] {
        let mut spec = spec();
        spec.where_clause = Some(clause.to_owned());
        spec.validate().unwrap();
        assert!(
            spec.source_sql().ends_with(&format!(" WHERE {clause}")),
            "{clause} 没有原样落到 WHERE 后面：\n{}",
            spec.source_sql()
        );
    }
}

/// 多行片段：**一个字符不加不改**，续行的缩进也不重排。
///
/// 重排看着更齐，但要做对就得知道哪个换行落在字符串字面量里面——往 `'a\nb'` 中间
/// 插进去的空格会改掉那个字面量的值，也就改掉了搬的数据。认那件事需要一个词法器，
/// 而「不解析这段文本」正是这个字段的立身之本。首尾空白仍然只是 `trim` 掉。
#[test]
fn a_multiline_where_clause_is_spliced_character_for_character() {
    let mut spec = spec();
    spec.where_clause = Some("  D_BIZ >= DATE '2026-08-01'\nAND STATUS = 'OK'  ".to_owned());
    spec.validate().unwrap();

    assert_eq!(
        spec.source_sql(),
        concat!(
            "SELECT a.N_VA_PRICE AS N_VA_PRICE,\n",
            "       a.D_BIZ AS D_BIZ\n",
            "  FROM HTBR45.T_R_FR_ASTSTAT@FA a\n",
            " WHERE D_BIZ >= DATE '2026-08-01'\n",
            "AND STATUS = 'OK'"
        )
    );
}

/// 换行落在字符串字面量里的那一条：值里的空白**一个都不许多出来**。
/// 这是上一条不重排缩进的全部理由，所以单独钉住。
#[test]
fn a_newline_inside_a_string_literal_is_left_exactly_as_written() {
    let mut spec = spec();
    spec.where_clause = Some("C_NOTE = 'first\nsecond'".to_owned());
    spec.validate().unwrap();

    assert!(
        spec.source_sql()
            .ends_with(" WHERE C_NOTE = 'first\nsecond'"),
        "字面量里的换行被动过：\n{}",
        spec.source_sql()
    );
}

/// 只有空白的片段等同于没写：不生成一个空的 `WHERE`。
#[test]
fn a_blank_where_clause_is_the_same_as_none() {
    let mut spec = spec();
    spec.where_clause = Some("   \n  ".to_owned());
    spec.validate().unwrap();
    assert!(!spec.source_sql().contains("WHERE"));
}

/// 自定义 SQL 外面要再套一层投影，**不是**原样执行。
///
/// 理由在搬运链路那头：`transfer.rs` 把 `source.columns()`——执行语句的结果列——原样交给
/// sink，所以结果列名就是目标列名。少了这一层，勾选与目标字段改名两件事都落不了地。
/// 内层照旧不许被追加过滤（过滤只能由用户写进那段 SQL 自己）。
#[test]
fn custom_select_is_wrapped_in_a_projection_and_gets_no_table_conditions() {
    let mut spec = spec();
    spec.source_sql = Some(
        "SELECT a.N_VA_PRICE, a.D_BIZ\n  FROM APP.T_CUSTOMER@FA a\n WHERE a.ACTIVE = 1;".to_owned(),
    );
    spec.dblink = None;

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
    // 映射就是投影的别名，搬运语义一个字节不变。所以「改了目标字段」在 SQL 上的
    // 全部痕迹就是 `AS` 右边那个词——WHERE 片段是用户自己写的，生成器不碰它一个字符，
    // 于是里面写的仍然是**源列名**（片段筛的是源表的列）。
    let mut renamed = spec();
    renamed.columns = vec![
        ColumnMapping {
            source: "C_NAME".to_owned(),
            target: "CUST_NAME".to_owned(),
        },
        identity("D_BIZ"),
    ];
    renamed.primary_key = vec!["CUST_NAME".to_owned()];
    renamed.where_clause = Some("D_BIZ = DATE '2026-08-14'".to_owned());
    renamed.validate().unwrap();

    assert_eq!(
        renamed.source_sql(),
        concat!(
            "SELECT a.C_NAME AS CUST_NAME,\n",
            "       a.D_BIZ AS D_BIZ\n",
            "  FROM HTBR45.T_R_FR_ASTSTAT@FA a\n",
            " WHERE D_BIZ = DATE '2026-08-14'"
        )
    );
}

#[test]
fn validation_refuses_the_ways_a_spec_can_be_unusable() {
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

    // 自定义 SQL 已经自带过滤，再挂一段 WHERE 就有两个说了算的地方。
    let mut both_filters = spec();
    both_filters.source_sql = Some("SELECT * FROM APP.T_CUSTOMER".to_owned());
    both_filters.dblink = None;
    both_filters.where_clause = Some("STATUS = 'OK'".to_owned());
    assert_eq!(
        both_filters.validate().unwrap_err(),
        "自定义 SQL 模式不能再单独配置过滤条件，请直接写进 SQL"
    );
}

/// WHERE 片段上**唯一**的一条禁令：不许有分号。
///
/// 它挡的不是注入——这段文本本来就是用户写的 SQL——而是**语句拼接**：分号之后那一段
/// 会被缝进一条本该只有一个 `SELECT` 的语句，于是预览的和执行的不是同一条。
#[test]
fn a_where_clause_may_not_smuggle_in_a_second_statement() {
    let mut injected = spec();
    injected.where_clause = Some("1=1; DROP TABLE T_R_FR_ASTSTAT".to_owned());
    assert_eq!(
        injected.validate().unwrap_err(),
        "过滤条件里不能出现分号：它只是拼进 WHERE 的一段条件，不是一条语句"
    );

    // 除此之外一律放行：注释、引号、括号、函数——合不合法由 Oracle 当场说了算。
    let mut quirky = spec();
    quirky.where_clause = Some("C_CODE = 'a''b' -- 尾注释\nAND (1=1)".to_owned());
    quirky.validate().unwrap();
}

#[test]
fn table_and_column_names_are_whitelisted_because_they_are_spliced() {
    // 表名、列名由界面上的选择产生，不该出现任何用户手写的字符——白名单式校验。
    // 过滤片段是另一回事：它按设计就是手写 SQL，只挡分号（见上一条）。
    let mut injected = spec();
    injected.table = "T_R_FR_ASTSTAT WHERE 1=1 --".to_owned();
    assert!(injected.validate().is_err());

    let mut injected_column = spec();
    injected_column.columns = vec![ColumnMapping {
        source: "D_BIZ) OR (1=1".to_owned(),
        target: "D_BIZ".to_owned(),
    }];
    assert!(injected_column.validate().is_err());

    let mut injected_target = spec();
    injected_target.columns = vec![ColumnMapping {
        source: "D_BIZ".to_owned(),
        target: "D_BIZ; DROP TABLE T".to_owned(),
    }];
    assert!(injected_target.validate().is_err());
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

/// #261：空主键不是「还没填」，它是一个有含义的值——目标表没有可合并的唯一约束，
/// 本任务写纯 `INSERT ... SELECT`。源端因此没有可判的东西：目标表到底有没有主键，
/// 不是源端能回答的问题，它归 sink 侧的映射预检。
#[test]
fn an_empty_primary_key_is_a_value_and_the_source_end_has_nothing_to_say_about_it() {
    let mut no_key = spec();
    no_key.primary_key.clear();

    no_key.validate().unwrap();
    assert_eq!(no_key.write_statement(), WriteStatement::Insert);
    // 其余每一条校验都还在——放开的只有「必须勾一列」这一条。
    assert_eq!(spec().write_statement(), WriteStatement::Upsert);
}

/// 临时任务文件走 TOML 落盘，而**标量排在 array-of-tables 之后会直接序列化失败**。
/// `write_mode` 是新加的标量，这条测试就是它必须排在 `columns` 之前的那个理由。
#[test]
fn the_spec_serialises_to_toml_which_is_what_pins_the_field_order() {
    let encoded = toml::to_string(&spec()).expect("任务定义必须能落成 TOML");

    let write_mode_at = encoded.find("write_mode").expect("write_mode 必须落盘");
    let columns_at = encoded.find("[[columns]]").expect("columns 是 array-of-tables");
    assert!(
        write_mode_at < columns_at,
        "write_mode 必须排在 columns 之前，否则整个任务定义序列化不出来：\n{encoded}"
    );
    assert!(encoded.contains("write_mode = \"APPEND\""), "{encoded}");

    // 无主键那一份同样要落得下来。
    let keyless = TaskSpec {
        primary_key: Vec::new(),
        ..spec()
    };
    let encoded = toml::to_string(&keyless).expect("无主键的任务定义同样要能落盘");
    assert!(encoded.contains("primary_key = []"), "{encoded}");

    // #265 的两个调度标量落在同一条规矩下。`schedule_cron` 是 `Option`，缺席时整个键不
    // 序列化，所以真正能证明顺序的是**配了表达式**的那一份。
    let scheduled = TaskSpec {
        schedule_cron: Some("0 2 * * *".to_owned()),
        schedule_enabled: true,
        ..spec()
    };
    let encoded = toml::to_string(&scheduled).expect("带调度的任务定义必须能落成 TOML");
    let columns_at = encoded.find("[[columns]]").expect("columns 是 array-of-tables");
    for scalar in ["schedule_cron", "schedule_enabled"] {
        let at = encoded
            .find(scalar)
            .unwrap_or_else(|| panic!("{scalar} 必须落盘：\n{encoded}"));
        assert!(
            at < columns_at,
            "{scalar} 必须排在 columns 之前，否则整个任务定义序列化不出来：\n{encoded}"
        );
    }
}

/// 无效的 cron 表达式在**保存这一刻**被拒，理由原样来自解析器（#265）。
///
/// 校验放在 `TaskSpec::validate` 上，而它是建任务、改任务、生成 SQL 三条路共用的那一道门——
/// 于是「存下一个永远不会响的闹钟」在源端没有任何一条绕行的路。
#[test]
fn an_invalid_cron_expression_is_refused_when_the_task_is_saved() {
    let mut scheduled = spec();
    scheduled.schedule_cron = Some("0 2 * * *".to_owned());
    scheduled.schedule_enabled = true;
    scheduled.validate().unwrap();

    // 关掉开关也照样校验：存下一行读不懂的文本，等到有人打开开关那天才报错，
    // 那一天离写下它的那一刻已经隔了很久。
    let mut broken = spec();
    broken.schedule_cron = Some("0 25 * * *".to_owned());
    broken.schedule_enabled = false;
    assert_eq!(
        broken.validate(),
        Err("小时字段的 25 超出取值范围 0-23".to_owned())
    );

    // 开着开关却没有表达式是自相矛盾的：这个任务永远不会被自动发起，而界面上它写着「已启用」。
    let mut enabled_without_expression = spec();
    enabled_without_expression.schedule_enabled = true;
    assert_eq!(
        enabled_without_expression.validate(),
        Err("启用了周期调度就必须写一条 cron 表达式".to_owned())
    );

    // 空白 = 没配，不是「配了个空的」。这与 `where_clause` 是同一套口径。
    let mut blank = spec();
    blank.schedule_cron = Some("   ".to_owned());
    blank.validate().unwrap();

    // 没配调度是默认状态，不该被判成错。
    spec().validate().unwrap();
}
