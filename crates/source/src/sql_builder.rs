//! 构建器的**元数据查询**面：列表 / 列清单。
//!
//! 源端 SQL 的生成已随 ADR-0036 §1 搬进 [`crate::task_spec`]——任务定义存的是结构化规格，
//! SQL 由规格现算，这里只剩「去数据字典问有哪些表、哪些列」。

use serde::Serialize;

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
    crate::task_spec::validate_identifier(dblink, "dblink")
        .map_err(|_| "dblink must be an unquoted Oracle identifier".to_owned())?;
    Ok(Some(dblink.to_ascii_uppercase()))
}
