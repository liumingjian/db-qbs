use std::fmt;
use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::NaiveDate;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use sqlparser::ast::{
    visit_expressions, BinaryOperator, Expr, FunctionArg, FunctionArgExpr, FunctionArguments,
    Query, Select, SelectItem, SetExpr, Statement, Value, Visit, Visitor,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    pub oracle_connect_string: String,
    pub oracle_username: String,
    pub oracle_password: String,
    pub oracle_client_lib_dir: String,
    pub sink_base_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskConfig {
    pub source_sql: String,
    pub source_date_col: String,
    pub target_table: String,
    pub target_date_col: String,
}

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

    let query = match statements.as_slice() {
        [Statement::Query(query)] => Some(query.as_ref()),
        _ => None,
    };

    let Some(query) = query else {
        problems.push(ShapeProblem::new(
            "unsupported_statement",
            "source SQL must contain exactly one SELECT statement",
        ));
        return Err(problems);
    };
    let SetExpr::Select(select) = query.body.as_ref() else {
        problems.push(ShapeProblem::new(
            "unsupported_statement",
            "source SQL must contain exactly one SELECT statement",
        ));
        return Err(problems);
    };

    check_projection(select, &mut problems);
    let mut selections = SelectionCollector::default();
    let _: ControlFlow<()> = query.visit(&mut selections);
    check_predicates(
        &selections.values,
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

fn has_relative_time_function(sql: &str) -> bool {
    let Ok(tokens) = Tokenizer::new(&GenericDialect, sql).tokenize() else {
        return false;
    };

    tokens.into_iter().any(|token| match token {
        Token::Word(word) => matches!(
            word.value.to_ascii_uppercase().as_str(),
            "SYSDATE" | "CURRENT_DATE" | "SYSTIMESTAMP" | "CURRENT_TIMESTAMP" | "LOCALTIMESTAMP"
        ),
        _ => false,
    })
}

fn check_projection(select: &Select, problems: &mut Vec<ShapeProblem>) {
    let mut unnamed = false;
    let mut indeterminate = false;

    for item in &select.projection {
        match item {
            SelectItem::UnnamedExpr(expr) if is_column(expr) => {}
            SelectItem::ExprWithAlias { expr, .. } if is_column(expr) => {}
            SelectItem::UnnamedExpr(_) => {
                unnamed = true;
                indeterminate = true;
            }
            SelectItem::ExprWithAlias { .. } => indeterminate = true,
            SelectItem::QualifiedWildcard(_, _) | SelectItem::Wildcard(_) => {
                unnamed = true;
                indeterminate = true;
            }
        }
    }

    if unnamed {
        problems.push(ShapeProblem::new(
            "unnamed_projection",
            "every selected column must be explicitly named; wildcards and unaliased expressions are not allowed",
        ));
    }
    if indeterminate {
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
struct SelectionCollector {
    values: Vec<Expr>,
}

impl Visitor for SelectionCollector {
    type Break = ();

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<Self::Break> {
        if let SetExpr::Select(select) = query.body.as_ref() {
            if let Some(selection) = &select.selection {
                self.values.push(selection.clone());
            }
        }
        ControlFlow::Continue(())
    }
}

fn check_predicates(
    selections: &[Expr],
    placeholder_count: usize,
    source_date_col: &str,
    problems: &mut Vec<ShapeProblem>,
) {
    let [selection] = selections else {
        problems.push(invalid_date_predicate());
        if selections.len() > 1 {
            problems.push(ShapeProblem::new(
                "additional_where_predicate",
                "WHERE must not contain predicates other than the business-date range",
            ));
        }
        return;
    };
    let mut predicates = Vec::new();
    flatten_and(selection, &mut predicates);
    let valid_range = predicates.len() == 2
        && placeholder_count == 2
        && predicates
            .iter()
            .any(|expr| is_lower_bound(expr, source_date_col))
        && predicates
            .iter()
            .any(|expr| is_upper_bound(expr, source_date_col));

    if !valid_range {
        problems.push(invalid_date_predicate());
    }

    let date_related = predicates
        .iter()
        .filter(|predicate| predicate_mentions_column(predicate, source_date_col))
        .count();
    if predicates.len().saturating_sub(date_related) > 0 {
        problems.push(ShapeProblem::new(
            "additional_where_predicate",
            "WHERE must not contain predicates other than the business-date range",
        ));
    }
}

fn invalid_date_predicate() -> ShapeProblem {
    ShapeProblem::new(
        "invalid_date_predicate",
        "WHERE must contain exactly source_date_col >= TO_DATE(:biz_date,'YYYY-MM-DD') AND source_date_col < TO_DATE(:biz_date,'YYYY-MM-DD') + 1",
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
    *op == expected_operator && column_name(left, source_date_col) && matches_right(right)
}

fn column_name(expr: &Expr, expected: &str) -> bool {
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
    if function.name.0.len() != 1 || !function.name.0[0].value.eq_ignore_ascii_case("TO_DATE") {
        return false;
    }
    let FunctionArguments::List(arguments) = &function.args else {
        return false;
    };
    if arguments.args.len() != 2 {
        return false;
    }

    matches_function_expr(
        &arguments.args[0],
        |expr| matches!(strip_nested(expr), Expr::Value(Value::Placeholder(value)) if value.eq_ignore_ascii_case(":biz_date")),
    ) && matches_function_expr(
        &arguments.args[1],
        |expr| matches!(strip_nested(expr), Expr::Value(Value::SingleQuotedString(value)) if value == "YYYY-MM-DD"),
    )
}

fn matches_function_expr(argument: &FunctionArg, predicate: impl FnOnce(&Expr) -> bool) -> bool {
    match argument {
        FunctionArg::Unnamed(FunctionArgExpr::Expr(expr)) => predicate(expr),
        _ => false,
    }
}

fn is_number_one(expr: &Expr) -> bool {
    matches!(strip_nested(expr), Expr::Value(Value::Number(value, _)) if value == "1")
}

fn count_biz_date_placeholders(node: &impl Visit) -> usize {
    let mut count = 0;
    let _: ControlFlow<()> = visit_expressions(node, |candidate| {
        if matches!(candidate, Expr::Value(Value::Placeholder(value)) if value.eq_ignore_ascii_case(":biz_date"))
        {
            count += 1;
        }
        ControlFlow::Continue(())
    });
    count
}

fn predicate_mentions_column(expr: &Expr, source_date_col: &str) -> bool {
    let mut found = false;
    let _: ControlFlow<()> = visit_expressions(expr, |candidate| {
        if column_name(candidate, source_date_col) {
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

pub fn probe_sink(base_url: &str) -> Result<(), String> {
    let url = Url::parse(base_url).map_err(|error| format!("invalid sink_base_url: {error}"))?;
    if url.scheme() != "http" {
        return Err("sink_base_url must use http for the M1 local endpoint".to_owned());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "sink_base_url must include a host".to_owned())?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "sink_base_url must include a port".to_owned())?;
    let address = (host, port)
        .to_socket_addrs()
        .map_err(|error| format!("could not resolve sink endpoint: {error}"))?
        .next()
        .ok_or_else(|| "sink endpoint did not resolve to an address".to_owned())?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
        .map_err(|error| format!("could not connect to sink endpoint: {error}"))?;

    use std::io::Write;
    let base_path = url.path().trim_end_matches('/');
    let request_path = format!("{base_path}/v1/runs");
    write!(
        stream,
        "POST {request_path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .map_err(|error| format!("could not send request to sink endpoint: {error}"))?;

    Err("SQL shape precheck passed; Oracle describe and POST /v1/runs payload are not implemented yet".to_owned())
}
