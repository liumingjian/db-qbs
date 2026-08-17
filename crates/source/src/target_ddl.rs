use std::fmt;

use serde::Serialize;

use crate::{ColumnPrecision, SourceColumn};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetDdlError {
    pub columns: Vec<TargetDdlColumnError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetDdlColumnError {
    pub column: String,
    pub source: String,
    pub message: String,
}

impl fmt::Display for TargetDdlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let details = self
            .columns
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        write!(formatter, "target DDL cannot be generated: {details}")
    }
}

impl fmt::Display for TargetDdlColumnError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}): {}",
            self.column, self.source, self.message
        )
    }
}

impl std::error::Error for TargetDdlError {}

pub fn generate_target_ddl(
    columns: &[SourceColumn],
    target_table: &str,
    target_date_col: &str,
    column_precision: Option<&ColumnPrecision>,
) -> Result<String, TargetDdlError> {
    let mut definitions = Vec::new();
    let mut errors = Vec::new();
    let mut precision_placeholders = Vec::new();

    for column in columns {
        if needs_precision_placeholder(column, column_precision) {
            precision_placeholders.push(column.name.escape_debug().to_string());
        }

        match target_column_type(column, column_precision) {
            Ok(column_type) => definitions.push(format!(
                "  {} {} NULL",
                quote_identifier(&column.name),
                column_type
            )),
            Err(error) => errors.push(error),
        }
    }

    let target_date_column = columns
        .iter()
        .find(|column| column.name.eq_ignore_ascii_case(target_date_col));
    if !target_date_column.is_some_and(is_supported_target_date_column)
        && !errors
            .iter()
            .any(|error| error.column.eq_ignore_ascii_case(target_date_col))
    {
        let source = target_date_column
            .map(source_type)
            .unwrap_or_else(|| "<missing>".to_owned());
        errors.push(named_error(
            target_date_col,
            source,
            "target_date_col must name an Oracle DATE or TIMESTAMP(0..6) describe column",
        ));
    }

    if !errors.is_empty() {
        return Err(TargetDdlError { columns: errors });
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

    let precision_note = if precision_placeholders.is_empty() {
        String::new()
    } else {
        format!(
            "-- {} 列的精度 describe 给不出，请在任务定义的 [column_precision] 字段为它们配 (p,s)。\n",
            precision_placeholders.join("、")
        )
    };

    Ok(format!(
        "-- db-qbs 生成的目标表建表 SQL，请自行执行；产品不会替你建表。\n\
         -- 下面这条索引不是可选项：切换事务的 DELETE 会锁住目标表当日范围，\n\
         -- 业务日期列无索引时锁全表。\n\
         {precision_note}CREATE TABLE {table_name} (\n{}\n\
         ) DEFAULT CHARSET=utf8mb4;",
        definitions.join(",\n")
    ))
}

pub(crate) fn derive_number_shape(precision: i64, scale: i64) -> (i64, i64) {
    if scale < 0 {
        (precision + scale.abs(), 0)
    } else if scale > precision {
        (scale, scale)
    } else {
        (precision, scale)
    }
}

pub(crate) fn is_supported_decimal_shape(precision: i64, scale: i64) -> bool {
    (1..=65).contains(&precision) && (0..=30).contains(&scale) && scale <= precision
}

fn target_column_type(
    column: &SourceColumn,
    column_precision: Option<&ColumnPrecision>,
) -> Result<String, TargetDdlColumnError> {
    match column.data_type.to_uppercase().as_str() {
        "NUMBER" => match (column.precision, column.scale) {
            (Some(precision), Some(scale)) => {
                let (target_precision, target_scale) = derive_number_shape(precision, scale);
                if is_supported_decimal_shape(target_precision, target_scale) {
                    Ok(format!("DECIMAL({target_precision},{target_scale})"))
                } else {
                    Err(column_error(
                        column,
                        format!(
                            "derived target shape DECIMAL({target_precision},{target_scale}) exceeds MySQL DECIMAL(65,30); change the source SQL or add a CAST"
                        ),
                    ))
                }
            }
            (None, None) => match precision_hint(column, column_precision) {
                Some([precision, scale]) if is_supported_decimal_shape(precision, scale) => {
                    Ok(format!("DECIMAL({precision},{scale})"))
                }
                Some([precision, scale]) => Err(column_error(
                    column,
                    format!(
                        "configured DECIMAL({precision},{scale}) is outside MySQL DECIMAL(65,30)"
                    ),
                )),
                None => Ok("DECIMAL(<p>,<s>)".to_owned()),
            },
            _ => Err(column_error(
                column,
                "NUMBER describe metadata must include both precision and scale",
            )),
        },
        "VARCHAR2" | "NVARCHAR2" | "CHAR" | "NCHAR" => column
            .length
            .map(|length| format!("VARCHAR({length})"))
            .ok_or_else(|| column_error(column, "character describe metadata has no length")),
        "DATE" => Ok("DATETIME(0)".to_owned()),
        "TIMESTAMP" if column.fsp.is_some_and(|fsp| fsp <= 6) => Ok("DATETIME(6)".to_owned()),
        "TIMESTAMP" => Err(column_error(
            column,
            "TIMESTAMP describe metadata must have fsp in 0..=6; change the source SQL or add a CAST to TIMESTAMP(6)",
        )),
        _ => Err(column_error(
            column,
            "source type is outside the target DDL whitelist; change the source SQL or add a CAST",
        )),
    }
}

fn is_supported_target_date_column(column: &SourceColumn) -> bool {
    match column.data_type.to_uppercase().as_str() {
        "DATE" => true,
        "TIMESTAMP" => column.fsp.is_some_and(|fsp| fsp <= 6),
        _ => false,
    }
}

fn needs_precision_placeholder(
    column: &SourceColumn,
    column_precision: Option<&ColumnPrecision>,
) -> bool {
    column.data_type.eq_ignore_ascii_case("NUMBER")
        && column.precision.is_none()
        && column.scale.is_none()
        && precision_hint(column, column_precision).is_none()
}

fn precision_hint(
    column: &SourceColumn,
    column_precision: Option<&ColumnPrecision>,
) -> Option<[i64; 2]> {
    column_precision.and_then(|hints| {
        hints
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(&column.name))
            .map(|(_, shape)| *shape)
    })
}

fn column_error(column: &SourceColumn, message: impl Into<String>) -> TargetDdlColumnError {
    named_error(&column.name, source_type(column), message)
}

fn named_error(column: &str, source: String, message: impl Into<String>) -> TargetDdlColumnError {
    TargetDdlColumnError {
        column: column.to_owned(),
        source,
        message: message.into(),
    }
}

fn source_type(column: &SourceColumn) -> String {
    match column.data_type.to_uppercase().as_str() {
        "NUMBER" => match (column.precision, column.scale) {
            (Some(precision), Some(scale)) => format!("NUMBER({precision},{scale})"),
            _ => "NUMBER".to_owned(),
        },
        "VARCHAR2" | "NVARCHAR2" | "CHAR" | "NCHAR" => column
            .length
            .map(|length| format!("{}({length})", column.data_type))
            .unwrap_or_else(|| column.data_type.clone()),
        "TIMESTAMP" => column
            .fsp
            .map(|fsp| format!("TIMESTAMP({fsp})"))
            .unwrap_or_else(|| column.data_type.clone()),
        _ => column.data_type.clone(),
    }
}

fn quote_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}
