use serde::{Deserialize, Serialize};

use crate::TaskConfig;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuilderTaskInput {
    pub dblink: Option<String>,
    pub owner: String,
    pub table: String,
    pub columns: Vec<String>,
    pub source_date_col: String,
    pub target_table: String,
    pub target_date_col: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuilderTable {
    pub owner: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuilderColumn {
    pub name: String,
    pub data_type: String,
    pub precision: Option<i64>,
    pub scale: Option<i64>,
    pub length: Option<u64>,
    pub nullable: bool,
}

pub fn generate_builder_task(input: BuilderTaskInput) -> Result<TaskConfig, String> {
    validate_builder_task_input(&input)?;

    let BuilderTaskInput {
        dblink,
        owner,
        table,
        columns,
        source_date_col,
        target_table,
        target_date_col,
    } = input;
    let dblink_suffix = dictionary_suffix(dblink.as_deref())?;
    let projection = columns
        .iter()
        .map(|column| format!("a.{column} AS {column}"))
        .collect::<Vec<_>>()
        .join(",\n       ");
    let target_date_col = if target_date_col.trim().is_empty() {
        source_date_col.clone()
    } else {
        target_date_col
    };

    Ok(TaskConfig {
        source_sql: format!(
            "SELECT {projection}\n  FROM {owner}.{table}{dblink_suffix} a\n WHERE a.{source_date_col} >= TO_DATE(:biz_date,'YYYY-MM-DD')\n   AND a.{source_date_col} <  TO_DATE(:biz_date,'YYYY-MM-DD') + 1"
        ),
        source_date_col,
        target_table,
        target_date_col,
        column_precision: None,
    })
}

fn validate_builder_task_input(input: &BuilderTaskInput) -> Result<(), String> {
    if input.owner.trim().is_empty() || input.table.trim().is_empty() {
        return Err("owner and table are required".to_owned());
    }
    for identifier in [&input.owner, &input.table] {
        validate_oracle_identifier(identifier)?;
    }
    if input.columns.is_empty() {
        return Err("at least one column must be selected".to_owned());
    }
    if input.columns.iter().any(|column| column.trim().is_empty()) {
        return Err("selected column names must not be empty".to_owned());
    }
    for column in &input.columns {
        validate_oracle_identifier(column)?;
    }
    for (index, column) in input.columns.iter().enumerate() {
        if input.columns[..index]
            .iter()
            .any(|previous| previous.eq_ignore_ascii_case(column))
        {
            return Err("selected columns must not contain duplicates".to_owned());
        }
    }
    if !input
        .columns
        .iter()
        .any(|column| column.eq_ignore_ascii_case(&input.source_date_col))
    {
        return Err("source_date_col must be one of the selected columns".to_owned());
    }
    Ok(())
}

pub fn builder_table_query(dblink: Option<&str>) -> Result<String, String> {
    Ok(format!(
        "SELECT OWNER, TABLE_NAME FROM ALL_TABLES{} ORDER BY OWNER, TABLE_NAME",
        dictionary_suffix(dblink)?
    ))
}

pub fn builder_column_query(dblink: Option<&str>) -> Result<String, String> {
    Ok(format!(
        "SELECT COLUMN_NAME, DATA_TYPE, DATA_PRECISION, DATA_SCALE, CHAR_LENGTH, NULLABLE \
FROM ALL_TAB_COLUMNS{} WHERE OWNER = :owner AND TABLE_NAME = :table_name ORDER BY COLUMN_ID",
        dictionary_suffix(dblink)?
    ))
}

pub fn validate_builder_dblink(dblink: Option<&str>) -> Result<(), String> {
    normalize_dblink(dblink).map(|_| ())
}

fn dictionary_suffix(dblink: Option<&str>) -> Result<String, String> {
    Ok(normalize_dblink(dblink)?
        .map(|link| format!("@{link}"))
        .unwrap_or_default())
}

fn normalize_dblink(dblink: Option<&str>) -> Result<Option<String>, String> {
    let Some(dblink) = dblink.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    validate_oracle_identifier(dblink)
        .map_err(|_| "dblink must be an unquoted Oracle identifier".to_owned())?;
    Ok(Some(dblink.to_ascii_uppercase()))
}

fn validate_oracle_identifier(identifier: &str) -> Result<(), String> {
    let mut bytes = identifier.bytes();
    let first_is_valid = bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic());
    if first_is_valid
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$' | b'#'))
    {
        Ok(())
    } else {
        Err(format!(
            "Oracle identifier {identifier:?} must use an unquoted name"
        ))
    }
}
