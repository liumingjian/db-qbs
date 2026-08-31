use std::fmt;

use sqlparser::ast::{Delete, FromTable, ObjectName, Statement, TableFactor, TableWithJoins};
use sqlparser::dialect::MySqlDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer};

use crate::WriteMode;

/// Why a configured preSQL statement is unsafe for the current target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreSqlValidationError {
    WriteMode,
    InvalidSql,
    StatementCount,
    NotDelete,
    UnsupportedDeleteShape,
    MissingWhere,
    WrongTarget,
}

impl fmt::Display for PreSqlValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::WriteMode => "preSQL 只能用于 APPEND 写入模式",
            Self::InvalidSql => "preSQL 不是有效的 MySQL SQL",
            Self::StatementCount => "preSQL 必须恰好包含一条语句",
            Self::NotDelete => "preSQL 必须是一条 DELETE 语句",
            Self::UnsupportedDeleteShape => {
                "preSQL 必须是当前目标表上的单表 DELETE，不能使用多目标、JOIN、USING 或 CTE DELETE"
            }
            Self::MissingWhere => "preSQL DELETE 必须包含 WHERE 条件",
            Self::WrongTarget => "preSQL DELETE 的目标必须是当前任务的目标表",
        })
    }
}

impl std::error::Error for PreSqlValidationError {}

/// Normalize and validate the destructive SQL which runs before an import.
///
/// Missing and blank input are the same absence. A configured statement is
/// returned as the original slice so callers cannot accidentally persist a
/// parser-rendered approximation of what the task author wrote.
pub fn validate_pre_sql<'sql>(
    pre_sql: Option<&'sql str>,
    target_database: &str,
    target_table: &str,
    write_mode: WriteMode,
) -> Result<Option<&'sql str>, PreSqlValidationError> {
    let Some(pre_sql) = pre_sql.filter(|sql| !sql.trim().is_empty()) else {
        return Ok(None);
    };

    if write_mode != WriteMode::Append {
        return Err(PreSqlValidationError::WriteMode);
    }

    let dialect = MySqlDialect {};
    validate_statement_delimiter(&dialect, pre_sql)?;
    let mut statements =
        Parser::parse_sql(&dialect, pre_sql).map_err(|_| PreSqlValidationError::InvalidSql)?;
    if statements.len() != 1 {
        return Err(PreSqlValidationError::StatementCount);
    }

    let Statement::Delete(delete) = statements.remove(0) else {
        return Err(PreSqlValidationError::NotDelete);
    };
    validate_delete(&delete, target_database, target_table)?;

    Ok(Some(pre_sql))
}

fn validate_statement_delimiter(
    dialect: &MySqlDialect,
    pre_sql: &str,
) -> Result<(), PreSqlValidationError> {
    let tokens = Tokenizer::new(dialect, pre_sql)
        .tokenize()
        .map_err(|_| PreSqlValidationError::InvalidSql)?;
    let significant: Vec<_> = tokens
        .iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)))
        .collect();
    let delimiters = significant
        .iter()
        .filter(|token| matches!(token, Token::SemiColon))
        .count();
    if delimiters > 1 || (delimiters == 1 && !matches!(significant.last(), Some(Token::SemiColon)))
    {
        return Err(PreSqlValidationError::StatementCount);
    }
    Ok(())
}

fn validate_delete(
    delete: &Delete,
    target_database: &str,
    target_table: &str,
) -> Result<(), PreSqlValidationError> {
    if !delete.tables.is_empty()
        || delete.using.is_some()
        || delete.returning.is_some()
        || delete.selection.is_none()
    {
        return if delete.selection.is_none() {
            Err(PreSqlValidationError::MissingWhere)
        } else {
            Err(PreSqlValidationError::UnsupportedDeleteShape)
        };
    }

    let from = match &delete.from {
        FromTable::WithFromKeyword(from) => from,
        FromTable::WithoutKeyword(_) => {
            return Err(PreSqlValidationError::UnsupportedDeleteShape);
        }
    };
    let [target] = from.as_slice() else {
        return Err(PreSqlValidationError::UnsupportedDeleteShape);
    };
    let name = plain_table_name(target)?;
    if !is_current_target(name, target_database, target_table) {
        return Err(PreSqlValidationError::WrongTarget);
    }

    Ok(())
}

fn plain_table_name(target: &TableWithJoins) -> Result<&ObjectName, PreSqlValidationError> {
    if !target.joins.is_empty() {
        return Err(PreSqlValidationError::UnsupportedDeleteShape);
    }
    match &target.relation {
        TableFactor::Table {
            name,
            args: None,
            with_hints,
            version: None,
            ..
        } if with_hints.is_empty() => Ok(name),
        _ => Err(PreSqlValidationError::UnsupportedDeleteShape),
    }
}

fn is_current_target(name: &ObjectName, database: &str, table: &str) -> bool {
    match name.0.as_slice() {
        [actual_table] => actual_table.value.eq_ignore_ascii_case(table),
        [actual_database, actual_table] => {
            actual_database.value.eq_ignore_ascii_case(database)
                && actual_table.value.eq_ignore_ascii_case(table)
        }
        _ => false,
    }
}
