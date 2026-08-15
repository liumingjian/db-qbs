use std::fmt;

use crate::SourceColumn;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetDdlError {
    pub column: String,
    pub message: String,
}

impl fmt::Display for TargetDdlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.column, self.message)
    }
}

impl std::error::Error for TargetDdlError {}

pub fn generate_target_ddl(
    columns: &[SourceColumn],
    target_table: &str,
    target_date_col: &str,
) -> Result<String, TargetDdlError> {
    let mut definitions = columns
        .iter()
        .map(|column| {
            Ok(format!(
                "  {} {} NULL",
                quote_identifier(&column.name),
                target_column_type(column)?
            ))
        })
        .collect::<Result<Vec<_>, TargetDdlError>>()?;

    let target_date_column = columns
        .iter()
        .find(|column| column.name.eq_ignore_ascii_case(target_date_col));
    if !target_date_column.is_some_and(|column| column.data_type.eq_ignore_ascii_case("DATE")) {
        return Err(TargetDdlError {
            column: target_date_col.to_owned(),
            message: "target_date_col must name an Oracle DATE describe column".to_owned(),
        });
    }

    let table_name = if target_table.is_empty() {
        "<目标表名>".to_owned()
    } else {
        quote_identifier(target_table)
    };
    let index_name = format!("idx_{}", target_date_col.to_ascii_lowercase());
    definitions.push(format!(
        "  KEY {} ({})",
        quote_identifier(&index_name),
        quote_identifier(target_date_col)
    ));

    Ok(format!(
        "-- db-qbs 生成的目标表建表 SQL，请自行执行；产品不会替你建表。\n\
         -- 下面这条索引不是可选项：切换事务的 DELETE 会锁住目标表当日范围，\n\
         -- 业务日期列无索引时锁全表。\n\
         CREATE TABLE {table_name} (\n{}\n\
         ) DEFAULT CHARSET=utf8mb4;",
        definitions.join(",\n")
    ))
}

fn target_column_type(column: &SourceColumn) -> Result<String, TargetDdlError> {
    match column.data_type.to_uppercase().as_str() {
        "NUMBER" => match (column.precision, column.scale) {
            (Some(precision), Some(scale))
                if precision <= 65 && (0..=30).contains(&scale) && scale <= precision =>
            {
                Ok(format!("DECIMAL({precision},{scale})"))
            }
            _ => Err(column_error(
                column,
                "NUMBER describe metadata cannot be represented by M1 DECIMAL",
            )),
        },
        "VARCHAR2" => column
            .length
            .map(|length| format!("VARCHAR({length})"))
            .ok_or_else(|| column_error(column, "VARCHAR2 describe metadata has no length")),
        "DATE" => Ok("DATETIME(0)".to_owned()),
        _ => Err(column_error(
            column,
            "describe type has no M1 target DDL derivation",
        )),
    }
}

fn column_error(column: &SourceColumn, message: &str) -> TargetDdlError {
    TargetDdlError {
        column: column.name.clone(),
        message: message.to_owned(),
    }
}

fn quote_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}
