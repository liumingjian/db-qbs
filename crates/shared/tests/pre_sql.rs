use db_qbs_shared::{validate_pre_sql, PreSqlValidationError, WriteMode};

const DATABASE: &str = "warehouse";
const TABLE: &str = "daily_orders";

fn validate(sql: Option<&str>) -> Result<Option<&str>, PreSqlValidationError> {
    validate_pre_sql(sql, DATABASE, TABLE, WriteMode::Append)
}

#[test]
fn absent_and_blank_sql_normalize_to_none() {
    assert_eq!(validate(None), Ok(None));
    assert_eq!(validate(Some("")), Ok(None));
    assert_eq!(validate(Some(" \n\t ")), Ok(None));
    assert_eq!(
        validate_pre_sql(Some(" \n\t "), DATABASE, TABLE, WriteMode::ClearThenImport),
        Ok(None)
    );
}

#[test]
fn valid_sql_is_returned_byte_for_byte() {
    let sql = "/* copied from mysql */\nDeLeTe FROM `warehouse`.`daily_orders`\nWHERE DATE(created_at) < CURRENT_DATE; -- keep this comment\n";
    let validated = validate(Some(sql)).expect("valid preSQL");
    assert_eq!(validated, Some(sql));
    assert!(std::ptr::eq(validated.unwrap().as_ptr(), sql.as_ptr()));
}

#[test]
fn accepts_unqualified_and_qualified_current_targets() {
    for sql in [
        "DELETE FROM daily_orders WHERE id = 7",
        "DELETE FROM WAREHOUSE.DAILY_ORDERS WHERE id = 7;",
        "DELETE FROM daily_orders AS doomed WHERE doomed.id = 7",
    ] {
        assert_eq!(validate(Some(sql)), Ok(Some(sql)), "{sql}");
    }
}

#[test]
fn accepts_functions_and_condition_subqueries() {
    let sql = "DELETE FROM daily_orders WHERE DATE(created_at) < CURRENT_DATE AND account_id IN (SELECT id FROM stale_accounts WHERE disabled = 1)";
    assert_eq!(validate(Some(sql)), Ok(Some(sql)));
}

#[test]
fn rejects_a_target_outside_the_current_task() {
    for sql in [
        "DELETE FROM other_table WHERE id = 7",
        "DELETE FROM other_database.daily_orders WHERE id = 7",
        "DELETE FROM catalog.warehouse.daily_orders WHERE id = 7",
    ] {
        assert_eq!(
            validate(Some(sql)),
            Err(PreSqlValidationError::WrongTarget),
            "{sql}"
        );
    }
}

#[test]
fn rejects_zero_or_multiple_statements() {
    assert_eq!(
        validate(Some("-- only a comment")),
        Err(PreSqlValidationError::StatementCount)
    );
    assert_eq!(
        validate(Some("DELETE FROM daily_orders WHERE id = 7; SELECT 1")),
        Err(PreSqlValidationError::StatementCount)
    );
    for sql in [
        "; DELETE FROM daily_orders WHERE id = 7",
        "DELETE FROM daily_orders WHERE id = 7;;",
    ] {
        assert_eq!(
            validate(Some(sql)),
            Err(PreSqlValidationError::StatementCount),
            "{sql}"
        );
    }
}

#[test]
fn rejects_missing_where_and_non_delete_statements() {
    assert_eq!(
        validate(Some("DELETE FROM daily_orders")),
        Err(PreSqlValidationError::MissingWhere)
    );
    for sql in [
        "UPDATE daily_orders SET status = 'gone' WHERE id = 7",
        "INSERT INTO daily_orders (id) VALUES (7)",
        "DROP TABLE daily_orders",
    ] {
        assert_eq!(
            validate(Some(sql)),
            Err(PreSqlValidationError::NotDelete),
            "{sql}"
        );
    }
}

#[test]
fn rejects_multi_table_join_using_and_cte_deletes() {
    for sql in [
        "DELETE daily_orders, archived_orders FROM daily_orders, archived_orders WHERE daily_orders.id = archived_orders.id",
        "DELETE FROM daily_orders JOIN archived_orders ON archived_orders.id = daily_orders.id WHERE daily_orders.id = 7",
        "DELETE FROM daily_orders USING daily_orders JOIN archived_orders ON archived_orders.id = daily_orders.id WHERE daily_orders.id = 7",
        "WITH doomed AS (SELECT 7 AS id) DELETE FROM daily_orders WHERE id IN (SELECT id FROM doomed)",
    ] {
        assert!(validate(Some(sql)).is_err(), "accepted unsupported DELETE: {sql}");
    }
}

#[test]
fn clear_then_import_rejects_configured_sql() {
    assert_eq!(
        validate_pre_sql(
            Some("DELETE FROM daily_orders WHERE id = 7"),
            DATABASE,
            TABLE,
            WriteMode::ClearThenImport,
        ),
        Err(PreSqlValidationError::WriteMode)
    );
}
