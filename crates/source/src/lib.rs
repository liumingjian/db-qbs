use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sqlparser::ast::{
    visit_expressions, BinaryOperator, Expr, FunctionArg, FunctionArgExpr, FunctionArguments,
    Query, Select, SelectItem, SetExpr, Statement, Value, Visit, Visitor,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer};

mod failure_kind;
mod oracle_source;
mod protocol;
mod run_history;
mod sql_builder;
mod target_ddl;
mod task_store;
mod transfer;
mod web_assets;

pub use db_qbs_shared::BatchPayload;
pub use failure_kind::{oracle_kind, FailureKind};
pub use oracle_source::OracleRowSource;
pub use protocol::{
    BatchResponse, ColumnSupport, CommitResponse, HttpSinkClient, OpenRunRequest, OpenRunResponse,
    RangeCheckColumn, RangeCheckResult, RunResponse, SinkClient, SinkError, SinkErrorKind,
    SinkGateDetails, SinkPrecheckIssue, SourceColumn, Terminal,
};
pub use run_history::{
    expired_history_indices, fold_history_lines, HistoryChange, HistoryStore, RunHistory,
    UnknownReason,
};
pub use sql_builder::{
    builder_column_query, builder_table_query, generate_builder_task, validate_builder_dblink,
    BuilderColumn, BuilderTable, BuilderTaskInput,
};
pub use target_ddl::{generate_target_ddl, TargetDdlColumnError, TargetDdlError};
pub use task_store::{Task, TaskInput, TaskStore};
pub use transfer::{
    generate_run_id, run_transfer, RowSource, RunStage, SourceReadError, TransferEvent,
    TransferFailure, TransferRequest, TransferSummary, BATCH_BYTE_BUDGET, BATCH_ROW_LIMIT,
    FETCH_ARRAY_SIZE,
};
pub use web_assets::{embedded_web_asset, EmbeddedWebAsset};

const RELATIVE_TIME_FUNCTION_NAMES: &[&str] = &[
    "SYSDATE",
    "CURRENT_DATE",
    "SYSTIMESTAMP",
    "CURRENT_TIMESTAMP",
    "LOCALTIMESTAMP",
];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    pub oracle_connect_string: String,
    pub oracle_username: String,
    pub oracle_password: String,
    pub oracle_client_lib_dir: String,
    pub sink_base_url: String,
    pub listen: String,
    pub data_dir: PathBuf,
    #[serde(default = "default_history_retention_days")]
    pub history_retention_days: u64,
    #[serde(default = "default_run_executable")]
    pub run_executable: PathBuf,
}

fn default_history_retention_days() -> u64 {
    90
}

fn default_run_executable() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_owned))
        .unwrap_or_default()
        .join("db-qbs-source-run")
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskConfig {
    pub source_sql: String,
    pub source_date_col: String,
    pub target_table: String,
    pub target_date_col: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column_precision: Option<ColumnPrecision>,
}

/// This is not the #100 column-mapping table: a wrong mapping can go unnoticed,
/// while these precision hints are checked by the full value-domain validation.
pub type ColumnPrecision = BTreeMap<String, [i64; 2]>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub kind: &'static str,
    pub path: PathBuf,
    pub detail: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "could not load {} file {}: {}",
            self.kind,
            self.path.display(),
            self.detail
        )
    }
}

impl std::error::Error for ConfigError {}

pub fn load_source_config(path: &Path) -> Result<SourceConfig, ConfigError> {
    load_toml(path, "source config")
}

pub fn load_task_config(path: &Path) -> Result<TaskConfig, ConfigError> {
    load_toml(path, "task")
}

fn load_toml<T: DeserializeOwned>(path: &Path, kind: &'static str) -> Result<T, ConfigError> {
    let text = fs::read_to_string(path).map_err(|error| ConfigError {
        kind,
        path: path.to_owned(),
        detail: error.to_string(),
    })?;

    toml::from_str(&text).map_err(|error| ConfigError {
        kind,
        path: path.to_owned(),
        detail: error.to_string(),
    })
}

pub fn parse_biz_date(value: &str) -> Result<NaiveDate, &'static str> {
    if value.len() != 10
        || value.as_bytes()[4] != b'-'
        || value.as_bytes()[7] != b'-'
        || !value
            .bytes()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
    {
        return Err("business date must use YYYY-MM-DD");
    }
    if value.starts_with("0000-") {
        return Err("business date year must be between 0001 and 9999");
    }

    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| "business date must be a valid calendar day in YYYY-MM-DD form")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ShapeProblem {
    pub code: &'static str,
    pub message: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ShapeCheck {
    pub rule: &'static str,
    pub passed: bool,
    pub message: &'static str,
}

struct ShapeRule {
    name: &'static str,
    problem_code: &'static str,
    message: &'static str,
    requires_inspectable_statement: bool,
}

const SHAPE_RULES: [ShapeRule; 6] = [
    ShapeRule {
        name: "business_date_range",
        problem_code: "invalid_date_predicate",
        message: "WHERE must contain the exact half-open :biz_date range on source_date_col",
        requires_inspectable_statement: true,
    },
    ShapeRule {
        name: "no_additional_predicates",
        problem_code: "additional_where_predicate",
        message: "WHERE must not contain predicates other than the business-date range",
        requires_inspectable_statement: true,
    },
    ShapeRule {
        name: "named_projection",
        problem_code: "unnamed_projection",
        message: "every selected column must be explicitly named",
        requires_inspectable_statement: true,
    },
    ShapeRule {
        name: "determinate_projection",
        problem_code: "indeterminate_projection",
        message: "selected expressions must have statically determinate precision",
        requires_inspectable_statement: true,
    },
    ShapeRule {
        name: "no_relative_time_functions",
        problem_code: "relative_time_function",
        message: "source SQL must not use relative time functions such as SYSDATE",
        requires_inspectable_statement: false,
    },
    ShapeRule {
        name: "matching_date_columns",
        problem_code: "date_column_mismatch",
        message: "target_date_col must equal source_date_col ignoring case",
        requires_inspectable_statement: false,
    },
];

impl ShapeProblem {
    const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

pub fn precheck_sql(task: &TaskConfig) -> Result<(), Vec<ShapeProblem>> {
    let mut problems = Vec::new();

    if has_relative_time_function(&task.source_sql) {
        problems.push(ShapeProblem::new(
            "relative_time_function",
            "source SQL must not use relative time functions such as SYSDATE",
        ));
    }

    if !task
        .source_date_col
        .eq_ignore_ascii_case(&task.target_date_col)
    {
        problems.push(ShapeProblem::new(
            "date_column_mismatch",
            "target_date_col must equal source_date_col ignoring case",
        ));
    }

    let statements = match Parser::parse_sql(&GenericDialect, &task.source_sql) {
        Ok(statements) => statements,
        Err(_) => {
            problems.push(ShapeProblem::new(
                "invalid_sql",
                "source SQL could not be parsed",
            ));
            return Err(problems);
        }
    };

    let [Statement::Query(query)] = statements.as_slice() else {
        problems.push(unsupported_statement_problem());
        return Err(problems);
    };
    let query = query.as_ref();
    let SetExpr::Select(select) = query.body.as_ref() else {
        problems.push(unsupported_statement_problem());
        return Err(problems);
    };

    check_projection(select, &mut problems);
    let mut where_clauses = WhereClauseCollector::default();
    let _: ControlFlow<()> = query.visit(&mut where_clauses);
    check_where_predicates(
        &where_clauses.expressions,
        count_biz_date_placeholders(query),
        &task.source_date_col,
        &mut problems,
    );

    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

pub fn sql_shape_report(task: &TaskConfig) -> Vec<ShapeCheck> {
    let problems = precheck_sql(task).err().unwrap_or_default();
    let statement_is_inspectable = !problems
        .iter()
        .any(|problem| matches!(problem.code, "invalid_sql" | "unsupported_statement"));

    SHAPE_RULES
        .iter()
        .map(|rule| ShapeCheck {
            rule: rule.name,
            passed: (!rule.requires_inspectable_statement || statement_is_inspectable)
                && !problems
                    .iter()
                    .any(|problem| problem.code == rule.problem_code),
            message: rule.message,
        })
        .collect()
}

fn unsupported_statement_problem() -> ShapeProblem {
    ShapeProblem::new(
        "unsupported_statement",
        "source SQL must contain exactly one SELECT statement",
    )
}

fn has_relative_time_function(sql: &str) -> bool {
    let Ok(tokens) = Tokenizer::new(&GenericDialect, sql).tokenize() else {
        return false;
    };

    tokens.into_iter().any(|token| match token {
        Token::Word(word) => RELATIVE_TIME_FUNCTION_NAMES
            .iter()
            .any(|name| word.value.eq_ignore_ascii_case(name)),
        _ => false,
    })
}

fn check_projection(select: &Select, problems: &mut Vec<ShapeProblem>) {
    let mut has_unnamed_projection = false;
    let mut has_indeterminate_projection = false;

    for item in &select.projection {
        match item {
            SelectItem::UnnamedExpr(expr) if is_column(expr) => {}
            SelectItem::ExprWithAlias { expr, .. } if is_column(expr) => {}
            SelectItem::UnnamedExpr(_) => {
                has_unnamed_projection = true;
                has_indeterminate_projection = true;
            }
            // M3 lets the sink decide whether an aliased expression has a supported
            // source shape. The source parser cannot distinguish numeric arithmetic
            // from a character expression without describing the Oracle cursor.
            SelectItem::ExprWithAlias { .. } => {}
            SelectItem::QualifiedWildcard(_, _) | SelectItem::Wildcard(_) => {
                has_unnamed_projection = true;
                has_indeterminate_projection = true;
            }
        }
    }

    if has_unnamed_projection {
        problems.push(ShapeProblem::new(
            "unnamed_projection",
            "every selected column must be explicitly named; wildcards and unaliased expressions are not allowed",
        ));
    }
    if has_indeterminate_projection {
        problems.push(ShapeProblem::new(
            "indeterminate_projection",
            "selected expressions must have statically determinate precision",
        ));
    }
}

fn is_column(expr: &Expr) -> bool {
    matches!(
        strip_nested(expr),
        Expr::Identifier(_) | Expr::CompoundIdentifier(_)
    )
}

#[derive(Default)]
struct WhereClauseCollector {
    expressions: Vec<Expr>,
}

impl Visitor for WhereClauseCollector {
    type Break = ();

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<Self::Break> {
        if let SetExpr::Select(select) = query.body.as_ref() {
            if let Some(where_clause) = &select.selection {
                self.expressions.push(where_clause.clone());
            }
        }
        ControlFlow::Continue(())
    }
}

fn check_where_predicates(
    where_clauses: &[Expr],
    placeholder_count: usize,
    source_date_col: &str,
    problems: &mut Vec<ShapeProblem>,
) {
    let [where_clause] = where_clauses else {
        problems.push(invalid_date_predicate_problem());
        if where_clauses.len() > 1 {
            problems.push(additional_where_predicate_problem());
        }
        return;
    };
    let mut predicates = Vec::new();
    flatten_and(where_clause, &mut predicates);
    let valid_range = predicates.len() == 2
        && placeholder_count == 2
        && predicates
            .iter()
            .any(|expr| is_lower_bound(expr, source_date_col))
        && predicates
            .iter()
            .any(|expr| is_upper_bound(expr, source_date_col));

    if !valid_range {
        problems.push(invalid_date_predicate_problem());
    }

    let has_unrelated_predicate = predicates
        .iter()
        .any(|predicate| !predicate_mentions_column(predicate, source_date_col));
    if has_unrelated_predicate {
        problems.push(additional_where_predicate_problem());
    }
}

fn invalid_date_predicate_problem() -> ShapeProblem {
    ShapeProblem::new(
        "invalid_date_predicate",
        "WHERE must contain exactly source_date_col >= TO_DATE(:biz_date,'YYYY-MM-DD') AND source_date_col < TO_DATE(:biz_date,'YYYY-MM-DD') + 1",
    )
}

fn additional_where_predicate_problem() -> ShapeProblem {
    ShapeProblem::new(
        "additional_where_predicate",
        "WHERE must not contain predicates other than the business-date range",
    )
}

fn flatten_and<'a>(expr: &'a Expr, output: &mut Vec<&'a Expr>) {
    match strip_nested(expr) {
        Expr::BinaryOp {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            flatten_and(left, output);
            flatten_and(right, output);
        }
        expr => output.push(expr),
    }
}

fn is_lower_bound(expr: &Expr, source_date_col: &str) -> bool {
    matches_binary(expr, BinaryOperator::GtEq, source_date_col, is_to_date)
}

fn is_upper_bound(expr: &Expr, source_date_col: &str) -> bool {
    matches_binary(expr, BinaryOperator::Lt, source_date_col, |right| {
        let Expr::BinaryOp {
            left,
            op: BinaryOperator::Plus,
            right,
        } = strip_nested(right)
        else {
            return false;
        };
        is_to_date(left) && is_number_one(right)
    })
}

fn matches_binary(
    expr: &Expr,
    expected_operator: BinaryOperator,
    source_date_col: &str,
    matches_right: impl FnOnce(&Expr) -> bool,
) -> bool {
    let Expr::BinaryOp { left, op, right } = strip_nested(expr) else {
        return false;
    };
    *op == expected_operator && matches_column_name(left, source_date_col) && matches_right(right)
}

fn matches_column_name(expr: &Expr, expected: &str) -> bool {
    match strip_nested(expr) {
        Expr::Identifier(identifier) => identifier.value.eq_ignore_ascii_case(expected),
        Expr::CompoundIdentifier(identifiers) => identifiers
            .last()
            .is_some_and(|identifier| identifier.value.eq_ignore_ascii_case(expected)),
        _ => false,
    }
}

fn is_to_date(expr: &Expr) -> bool {
    let Expr::Function(function) = strip_nested(expr) else {
        return false;
    };
    let [function_name] = function.name.0.as_slice() else {
        return false;
    };
    if !function_name.value.eq_ignore_ascii_case("TO_DATE") {
        return false;
    }
    let FunctionArguments::List(arguments) = &function.args else {
        return false;
    };
    let [biz_date_argument, format_argument] = arguments.args.as_slice() else {
        return false;
    };
    let FunctionArg::Unnamed(FunctionArgExpr::Expr(biz_date)) = biz_date_argument else {
        return false;
    };
    let FunctionArg::Unnamed(FunctionArgExpr::Expr(format)) = format_argument else {
        return false;
    };

    is_biz_date_placeholder(biz_date) && is_date_format(format)
}

fn is_biz_date_placeholder(expr: &Expr) -> bool {
    matches!(strip_nested(expr), Expr::Value(Value::Placeholder(value)) if value.eq_ignore_ascii_case(":biz_date"))
}

fn is_date_format(expr: &Expr) -> bool {
    matches!(strip_nested(expr), Expr::Value(Value::SingleQuotedString(value)) if value == "YYYY-MM-DD")
}

fn is_number_one(expr: &Expr) -> bool {
    matches!(strip_nested(expr), Expr::Value(Value::Number(value, _)) if value == "1")
}

fn count_biz_date_placeholders(query: &Query) -> usize {
    let mut count = 0;
    let _: ControlFlow<()> = visit_expressions(query, |candidate| {
        if is_biz_date_placeholder(candidate) {
            count += 1;
        }
        ControlFlow::Continue(())
    });
    count
}

fn predicate_mentions_column(expr: &Expr, source_date_col: &str) -> bool {
    let mut found = false;
    let _: ControlFlow<()> = visit_expressions(expr, |candidate| {
        if matches_column_name(candidate, source_date_col) {
            found = true;
        }
        ControlFlow::Continue(())
    });
    found
}

fn strip_nested(mut expr: &Expr) -> &Expr {
    while let Expr::Nested(inner) = expr {
        expr = inner;
    }
    expr
}
