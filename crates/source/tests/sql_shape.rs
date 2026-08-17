use db_qbs_source::{precheck_sql, sql_shape_report, TaskConfig};

#[test]
fn precheck_reports_all_shape_violations_together() {
    let task = TaskConfig {
        source_sql: "SELECT *, amount * 2 FROM orders WHERE biz_day = SYSDATE AND status = 'OPEN'"
            .to_owned(),
        source_date_col: "BIZ_DAY".to_owned(),
        target_table: "ORDERS".to_owned(),
        target_date_col: "OTHER_DAY".to_owned(),
        column_precision: None,
    };

    let problems = precheck_sql(&task).unwrap_err();
    let codes: Vec<_> = problems.iter().map(|problem| problem.code).collect();

    assert!(codes.contains(&"relative_time_function"));
    assert!(codes.contains(&"invalid_date_predicate"));
    assert!(codes.contains(&"additional_where_predicate"));
    assert!(codes.contains(&"unnamed_projection"));
    assert!(codes.contains(&"date_column_mismatch"));
}

#[test]
fn valid_half_open_range_and_named_columns_pass() {
    let task = task(
        "SELECT a.id AS ID, a.biz_day AS BIZ_DAY FROM orders a \
         WHERE a.biz_day >= TO_DATE(:biz_date,'YYYY-MM-DD') \
         AND a.biz_day < TO_DATE(:biz_date,'YYYY-MM-DD') + 1",
    );

    assert_eq!(precheck_sql(&task), Ok(()));
}

#[test]
fn valid_oracle_dblink_subquery_shape_passes() {
    let task = task(
        "SELECT t.id AS ID, t.biz_day AS BIZ_DAY \
         FROM (SELECT * FROM orders@FA a \
               WHERE a.biz_day >= TO_DATE(:biz_date,'YYYY-MM-DD') \
               AND a.biz_day < TO_DATE(:biz_date,'YYYY-MM-DD') + 1) t",
    );

    assert_eq!(precheck_sql(&task), Ok(()));
}

#[test]
fn shape_report_marks_uninspectable_rules_failed_in_stable_order() {
    let checks = sql_shape_report(&task(""));
    let results = checks
        .iter()
        .map(|check| (check.rule, check.passed))
        .collect::<Vec<_>>();

    assert_eq!(
        results,
        [
            ("business_date_range", false),
            ("no_additional_predicates", false),
            ("named_projection", false),
            ("determinate_projection", false),
            ("no_relative_time_functions", true),
            ("matching_date_columns", true),
        ]
    );
}

#[test]
fn equality_date_predicate_is_rejected() {
    assert_has_problem(
        "SELECT a.id AS ID, a.biz_day AS BIZ_DAY FROM orders a \
         WHERE a.biz_day = TO_DATE(:biz_date,'YYYY-MM-DD')",
        "invalid_date_predicate",
    );
}

#[test]
fn additional_business_predicate_is_rejected() {
    assert_has_problem(
        "SELECT a.id AS ID, a.biz_day AS BIZ_DAY FROM orders a \
         WHERE a.biz_day >= TO_DATE(:biz_date,'YYYY-MM-DD') \
         AND a.biz_day < TO_DATE(:biz_date,'YYYY-MM-DD') + 1 \
         AND a.status = 'OPEN'",
        "additional_where_predicate",
    );
}

#[test]
fn biz_date_placeholder_must_appear_exactly_twice_in_the_whole_query() {
    assert_has_problem(
        "SELECT a.id AS ID, a.biz_day AS BIZ_DAY FROM orders a \
         JOIN regions r ON r.created_at >= TO_DATE(:biz_date,'YYYY-MM-DD') \
         WHERE a.biz_day >= TO_DATE(:biz_date,'YYYY-MM-DD') \
         AND a.biz_day < TO_DATE(:biz_date,'YYYY-MM-DD') + 1",
        "invalid_date_predicate",
    );
}

#[test]
fn relative_time_function_is_rejected_but_not_when_quoted() {
    assert_has_problem(
        "SELECT a.id AS ID, a.biz_day AS BIZ_DAY FROM orders a \
         WHERE a.biz_day >= SYSDATE AND a.biz_day < SYSDATE + 1",
        "relative_time_function",
    );

    let quoted = task(
        "SELECT a.id AS ID, a.biz_day AS BIZ_DAY, a.note AS NOTE FROM orders a \
         WHERE a.biz_day >= TO_DATE(:biz_date,'YYYY-MM-DD') \
         AND a.biz_day < TO_DATE(:biz_date,'YYYY-MM-DD') + 1 \
         AND 'SYSDATE' = 'SYSDATE'",
    );
    let problems = precheck_sql(&quoted).unwrap_err();
    assert!(!problems
        .iter()
        .any(|problem| problem.code == "relative_time_function"));
}

#[test]
fn wildcard_and_unaliased_expression_are_rejected() {
    assert_has_problem(
        "SELECT a.*, a.amount * 2 FROM orders a \
         WHERE a.biz_day >= TO_DATE(:biz_date,'YYYY-MM-DD') \
         AND a.biz_day < TO_DATE(:biz_date,'YYYY-MM-DD') + 1",
        "unnamed_projection",
    );
}

fn assert_has_problem(sql: &str, expected_code: &str) {
    let problems = precheck_sql(&task(sql)).unwrap_err();
    assert!(
        problems.iter().any(|problem| problem.code == expected_code),
        "missing {expected_code} in {problems:?}"
    );
}

fn task(sql: &str) -> TaskConfig {
    TaskConfig {
        source_sql: sql.to_owned(),
        source_date_col: "BIZ_DAY".to_owned(),
        target_table: "ORDERS".to_owned(),
        target_date_col: "biz_day".to_owned(),
        column_precision: None,
    }
}
