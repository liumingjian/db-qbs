use db_qbs_source::{
    builder_column_query, builder_table_query, generate_builder_task, sql_shape_report,
    BuilderTaskInput,
};

#[test]
fn generated_builder_task_has_four_fields_and_passes_all_shape_checks() {
    let task = generate_builder_task(BuilderTaskInput {
        dblink: Some("FA".to_owned()),
        owner: "HTBR45".to_owned(),
        table: "T_R_FR_ASTSTAT".to_owned(),
        columns: vec!["N_VA_PRICE".to_owned(), "D_BIZ".to_owned()],
        source_date_col: "D_BIZ".to_owned(),
        target_table: "T_POSITION".to_owned(),
        target_date_col: "D_BIZ".to_owned(),
    })
    .unwrap();

    let json = serde_json::to_value(&task).unwrap();
    assert_eq!(json.as_object().unwrap().len(), 4);
    assert_eq!(task.source_date_col, "D_BIZ");
    assert_eq!(task.target_table, "T_POSITION");
    assert_eq!(task.target_date_col, "D_BIZ");
    assert!(task.source_sql.contains("FROM HTBR45.T_R_FR_ASTSTAT@FA a"));
    assert!(task.source_sql.contains("a.N_VA_PRICE AS N_VA_PRICE"));
    let checks = sql_shape_report(&task);
    assert!(
        checks.iter().all(|check| check.passed),
        "generated SQL failed shape checks: {checks:#?}\n{}",
        task.source_sql
    );
}

#[test]
fn builder_allows_a_true_column_subset_and_requires_the_date_column_in_it() {
    let input = BuilderTaskInput {
        dblink: None,
        owner: "APP".to_owned(),
        table: "ORDERS".to_owned(),
        columns: vec!["D_BIZ".to_owned()],
        source_date_col: "D_BIZ".to_owned(),
        target_table: String::new(),
        target_date_col: "D_BIZ".to_owned(),
    };

    let task = generate_builder_task(input.clone()).unwrap();
    let checks = sql_shape_report(&task);
    assert!(
        checks.iter().all(|check| check.passed),
        "generated SQL failed shape checks: {checks:#?}\n{}",
        task.source_sql
    );

    let error = generate_builder_task(BuilderTaskInput {
        columns: vec!["ID".to_owned()],
        ..input
    })
    .unwrap_err();
    assert_eq!(error, "source_date_col must be one of the selected columns");
}

#[test]
fn metadata_queries_use_only_a_validated_dblink_suffix() {
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
    assert!(builder_table_query(Some("FA WHERE 1=1")).is_err());
}
