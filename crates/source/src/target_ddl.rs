use std::fmt;

use serde::Serialize;

use crate::{
    classify_column, is_supported_decimal_shape, ColumnPrecision, ColumnShape, ShapeRejection,
    SourceColumn, TargetShape,
};

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

/// 生成目标表的建表 SQL。
///
/// `target_collation` 是**目标端 agent 上报的 utf8mb4 默认字符序**（#257）。source 一条
/// MySQL 连接都不建，除了 agent 上报之外没有第二个信息源，而 8.0 与 5.7 的默认值不同
/// （`utf8mb4_0900_ai_ci` / `utf8mb4_general_ci`）——5.7 上根本没有前者，照 8.0 写死会
/// 直接建表失败。
///
/// **`None` 时不写 `COLLATE`**，只留 `DEFAULT CHARSET=utf8mb4`：那是本票之前的形态，
/// 字符序交给目标库自己的默认值。旧版本 agent 报不出这一项，此时挑一个填进去就是猜——
/// 猜错的代价是一张字符序不对的表，而它要到比较、排序出怪结果时才暴露。
pub fn generate_target_ddl(
    columns: &[SourceColumn],
    target_table: &str,
    primary_key: &[String],
    column_precision: Option<&ColumnPrecision>,
    target_collation: Option<&str>,
) -> Result<String, TargetDdlError> {
    let mut definitions = Vec::new();
    let mut errors = Vec::new();
    let mut precision_placeholders = Vec::new();

    for column in columns {
        if needs_precision_placeholder(column, column_precision) {
            precision_placeholders.push(column.name.escape_debug().to_string());
        }

        // 主键列必须 NOT NULL，其余列必须可空——两条各有出处：前者是 ADR-0035 §2 第 3 条
        // （可空会让 upsert 静默退化），后者是 ADR-0009 的映射预检。
        // 一列主键都没勾时（#261 的纯追加写）全部列都落到 `NULL` 这一支，这是对的：
        // 没有 upsert 要去重，也就没有哪一列非 NOT NULL 不可。
        let nullability = if is_key_column(&column.name, primary_key) {
            "NOT NULL"
        } else {
            "NULL"
        };
        match target_column_type(column, column_precision) {
            Ok(column_type) => definitions.push(format!(
                "  {} {} {nullability}",
                quote_identifier(&column.name),
                column_type
            )),
            Err(error) => errors.push(error),
        }
    }

    for key_column in primary_key {
        let described = columns
            .iter()
            .any(|column| column.name.eq_ignore_ascii_case(key_column));
        if !described
            && !errors
                .iter()
                .any(|error| error.column.eq_ignore_ascii_case(key_column))
        {
            errors.push(named_error(
                key_column,
                "<missing>".to_owned(),
                "primary key column is not among the described source columns",
            ));
        }
    }

    if !errors.is_empty() {
        return Err(TargetDdlError { columns: errors });
    }

    let table_name = if target_table.is_empty() {
        "<目标表名>".to_owned()
    } else {
        quote_identifier(target_table)
    };
    // 主键是**可选**的（#261）。勾了主键就是要 upsert，那时它一个字都不能少：撤掉
    // DELETE 之后幂等全靠它，目标表没有对应约束时 `ON DUPLICATE KEY UPDATE` 会静默
    // 退化成纯 INSERT（ADR-0035 §2），sink 侧预检会直接拒跑。
    //
    // 一列都没勾时**不写这一段**。写一条空的 `PRIMARY KEY ()` 是语法错，随便挑一列
    // 凑上去则更糟：那会建出一张有主键的表，而任务定义记的是「无主键、纯追加写」，
    // 两边一对不上，第一次运行就被预检拦下来——用户手里拿的正是本工具给他的语句。
    if !primary_key.is_empty() {
        definitions.push(format!(
            "  PRIMARY KEY ({})",
            primary_key
                .iter()
                .map(|column| quote_identifier(column))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let precision_note = if precision_placeholders.is_empty() {
        String::new()
    } else {
        format!(
            "-- {} 列的精度 describe 给不出，请在取列面为它们配 (p,s)。\n",
            precision_placeholders.join("、")
        )
    };

    // 字符序来自 agent 上报（#257）。没报就整段不写——见函数头上那段。
    let collation_clause = target_collation
        .map(str::trim)
        .filter(|collation| !collation.is_empty())
        .map(|collation| format!(" COLLATE={collation}"))
        .unwrap_or_default();

    // 表头那句话跟着写法走（#261）：有主键时说的是「别去掉它」，无主键时说的是
    // 「这就是纯追加，重跑会翻倍」。两种情况说同一句话等于其中一句必然是假的。
    let write_note = if primary_key.is_empty() {
        "-- 没有主键：本任务写纯 INSERT，每跑一次都会把这批数据再追加一份。\n"
    } else {
        "-- 下面那条主键不是可选项：写入走 upsert，目标表没有它时重跑会静默出重复行。\n"
    };
    Ok(format!(
        "-- db-qbs 生成的目标表建表 SQL，请自行执行；产品不会替你建表。\n\
         {write_note}{precision_note}CREATE TABLE {table_name} (\n{}\n\
         ) DEFAULT CHARSET=utf8mb4{collation_clause};",
        definitions.join(",\n")
    ))
}

fn is_key_column(name: &str, primary_key: &[String]) -> bool {
    primary_key.iter().any(|key| key.eq_ignore_ascii_case(name))
}

fn target_column_type(
    column: &SourceColumn,
    column_precision: Option<&ColumnPrecision>,
) -> Result<String, TargetDdlColumnError> {
    match classify_column(column) {
        ColumnShape::Resolved(shape) => Ok(shape.to_string()),
        // 裸 `NUMBER` / 数值表达式列的形状由任务定义给（ADR-0030 §4）——
        // 那是 source 侧的输入，共用的分类函数只看 describe。
        ColumnShape::NeedsPrecision => match precision_hint(column, column_precision) {
            Some([precision, scale]) if is_supported_decimal_shape(precision, scale) => {
                Ok(TargetShape::Decimal { precision, scale }.to_string())
            }
            Some([precision, scale]) => Err(column_error(
                column,
                format!("configured DECIMAL({precision},{scale}) is outside MySQL DECIMAL(65,30)"),
            )),
            None => Ok("DECIMAL(<p>,<s>)".to_owned()),
        },
        ColumnShape::Rejected(rejection) => Err(column_error(column, rejection_message(rejection))),
    }
}

fn rejection_message(rejection: ShapeRejection) -> String {
    match rejection {
        ShapeRejection::DecimalShapeUnrepresentable { precision, scale } => format!(
            "derived target shape DECIMAL({precision},{scale}) exceeds MySQL DECIMAL(65,30); change the source SQL or add a CAST"
        ),
        ShapeRejection::NumberPrecisionIncomplete => {
            "NUMBER describe metadata must include both precision and scale".to_owned()
        }
        ShapeRejection::CharacterLengthMissing => {
            "character describe metadata has no length".to_owned()
        }
        ShapeRejection::TimestampFspMissing | ShapeRejection::TimestampFspTooPrecise { .. } => {
            "TIMESTAMP describe metadata must have fsp in 0..=6; change the source SQL or add a CAST to TIMESTAMP(6)".to_owned()
        }
        ShapeRejection::TypeNotWhitelisted => {
            "source type is outside the target DDL whitelist; change the source SQL or add a CAST"
                .to_owned()
        }
    }
}

fn needs_precision_placeholder(
    column: &SourceColumn,
    column_precision: Option<&ColumnPrecision>,
) -> bool {
    matches!(classify_column(column), ColumnShape::NeedsPrecision)
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
