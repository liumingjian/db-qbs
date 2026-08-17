use std::collections::{HashMap, HashSet};

use crate::{PrecheckIssue, RangeCheckColumn, SourceColumn, TargetColumn};

pub fn precheck(
    target_table: &str,
    source_columns: &[SourceColumn],
    target_columns: &[TargetColumn],
) -> Vec<PrecheckIssue> {
    precheck_inner(target_table, None, source_columns, target_columns)
}

pub(crate) fn precheck_with_date_column(
    target_table: &str,
    target_date_col: &str,
    source_columns: &[SourceColumn],
    target_columns: &[TargetColumn],
) -> Vec<PrecheckIssue> {
    precheck_inner(
        target_table,
        Some(target_date_col),
        source_columns,
        target_columns,
    )
}

pub(crate) fn range_check_columns(
    source_columns: &[SourceColumn],
    target_columns: &[TargetColumn],
) -> Vec<RangeCheckColumn> {
    let targets: HashMap<String, &TargetColumn> = target_columns
        .iter()
        .map(|column| (column.name.to_uppercase(), column))
        .collect();

    source_columns
        .iter()
        .filter(|source| {
            source.data_type.eq_ignore_ascii_case("NUMBER")
                && source.precision.is_none()
                && source.scale.is_none()
        })
        .filter_map(|source| {
            let target = targets.get(&source.name.to_uppercase()).copied()?;
            let (precision, scale) = target_decimal_shape(target)?;
            Some(RangeCheckColumn {
                column: source.name.clone(),
                precision,
                scale,
            })
        })
        .collect()
}

fn precheck_inner(
    target_table: &str,
    target_date_col: Option<&str>,
    source_columns: &[SourceColumn],
    target_columns: &[TargetColumn],
) -> Vec<PrecheckIssue> {
    let mut issues = Vec::new();
    if target_table.chars().count() > 37 {
        issues.push(PrecheckIssue {
            column: "<target_table>".to_owned(),
            source: "-".to_owned(),
            target: target_table.to_owned(),
            rule: "目标表名最多 37 个字符，否则暂存表名会超过 MySQL 64 字符上限；请缩短目标表名"
                .to_owned(),
            suggestion: None,
        });
    }

    let targets: HashMap<String, &TargetColumn> = target_columns
        .iter()
        .map(|column| (column.name.to_uppercase(), column))
        .collect();
    let mut source_names = HashSet::new();

    for source in source_columns {
        let normalized_name = source.name.to_uppercase();
        if !source_names.insert(normalized_name.clone()) {
            issues.push(issue(
                source,
                targets.get(&normalized_name).copied(),
                "源端列名重复，按名字无法唯一对齐",
            ));
        }

        let target = targets.get(&normalized_name).copied();
        let Some(target) = target else {
            issues.push(issue(source, None, "目标表缺少同名列"));
            validate_source_type(source, None, &mut issues);
            continue;
        };

        validate_source_type(source, Some(target), &mut issues);
        if !target.nullable {
            issues.push(issue(
                source,
                Some(target),
                "目标列必须可空，不能是 NOT NULL",
            ));
        }
    }

    for target in target_columns {
        if !source_names.contains(&target.name.to_uppercase()) {
            issues.push(PrecheckIssue {
                column: target.name.clone(),
                source: "<missing>".to_owned(),
                target: target_display(target),
                rule: "源端结果缺少同名列，源端与目标端列名集合必须完全相等".to_owned(),
                suggestion: None,
            });
        }
    }

    if let Some(date_column) = target_date_col {
        validate_date_column(date_column, source_columns, &targets, &mut issues);
    }

    issues
}

fn validate_date_column(
    date_column: &str,
    source_columns: &[SourceColumn],
    targets: &HashMap<String, &TargetColumn>,
    issues: &mut Vec<PrecheckIssue>,
) {
    let source = source_columns
        .iter()
        .find(|column| column.name.eq_ignore_ascii_case(date_column));
    let is_oracle_date = source
        .map(|column| column.data_type.eq_ignore_ascii_case("DATE"))
        .unwrap_or(false);
    if is_oracle_date {
        return;
    }

    issues.push(PrecheckIssue {
        column: date_column.to_owned(),
        source: source
            .map(source_display)
            .unwrap_or_else(|| "<missing>".to_owned()),
        target: targets
            .get(&date_column.to_uppercase())
            .map(|column| target_display(column))
            .unwrap_or_else(|| "<missing>".to_owned()),
        rule: "target_date_col 必须对应同名的 Oracle DATE 源列".to_owned(),
        suggestion: None,
    });
}

fn validate_source_type(
    source: &SourceColumn,
    target: Option<&TargetColumn>,
    issues: &mut Vec<PrecheckIssue>,
) {
    match source.data_type.to_uppercase().as_str() {
        "NUMBER" => validate_number(source, target, issues),
        "VARCHAR2" => validate_varchar(source, target, issues),
        "DATE" => validate_date(source, target, issues),
        _ => issues.push(issue(
            source,
            target,
            "M1 只支持 NUMBER(p,s)、VARCHAR2(n) 和 DATE",
        )),
    }
}

fn validate_number(
    source: &SourceColumn,
    target: Option<&TargetColumn>,
    issues: &mut Vec<PrecheckIssue>,
) {
    let (precision, scale) = match (source.precision, source.scale) {
        (Some(precision), Some(scale)) => (precision, scale),
        (None, None) => {
            match target {
                Some(target) if !target.data_type.eq_ignore_ascii_case("decimal") => {
                    issues.push(issue(source, Some(target), "NUMBER 的目标类型必须是 DECIMAL"));
                }
                Some(target) if target_decimal_shape(target).is_none() => issues.push(issue(
                    source,
                    Some(target),
                    "裸 NUMBER / 数值表达式列的目标 DECIMAL 必须具有有效的 precision 和 scale",
                )),
                Some(_) => {}
                None => issues.push(issue(
                    source,
                    target,
                    "NUMBER 必须同时具有可判定的 precision 和 scale，裸 NUMBER 与表达式列需要目标 DECIMAL 形状",
                )),
            }
            return;
        }
        _ => {
            issues.push(issue(
                source,
                target,
                "NUMBER 必须同时具有可判定的 precision 和 scale，裸 NUMBER 与表达式列不支持",
            ));
            return;
        }
    };

    if scale > 30 || precision > 65 {
        issues.push(issue(
            source,
            target,
            "MySQL DECIMAL 无法表达该源类型（precision <= 65 且 scale <= 30）",
        ));
    }
    if scale < 0 {
        issues.push(issue(source, target, "M1 不支持负标度 NUMBER"));
    }
    if scale > precision {
        issues.push(issue(
            source,
            target,
            "M1 不支持 scale 大于 precision 的纯小数 NUMBER",
        ));
    }

    if let Some(target) = target {
        if !target.data_type.eq_ignore_ascii_case("decimal") {
            issues.push(issue(
                source,
                Some(target),
                "NUMBER 的目标类型必须是 DECIMAL",
            ));
        } else if target.precision != u64::try_from(precision).ok()
            || target.scale != u64::try_from(scale).ok()
        {
            issues.push(issue(
                source,
                Some(target),
                "NUMBER 与 DECIMAL 的 precision、scale 必须逐位相等",
            ));
        }
    }
}

pub(crate) fn range_check_issue(
    source: &SourceColumn,
    target: &TargetColumn,
    range_column: &RangeCheckColumn,
    invalid_rows: u64,
) -> PrecheckIssue {
    issue(
        source,
        Some(target),
        &format!(
            "值域校核失败：{invalid_rows} 行无法无损写入 DECIMAL({}, {})",
            range_column.precision, range_column.scale
        ),
    )
}

fn validate_varchar(
    source: &SourceColumn,
    target: Option<&TargetColumn>,
    issues: &mut Vec<PrecheckIssue>,
) {
    let Some(length) = source.length else {
        issues.push(issue(source, target, "VARCHAR2 必须具有可判定的 length"));
        return;
    };

    if let Some(target) = target {
        if !target.data_type.eq_ignore_ascii_case("varchar") {
            issues.push(issue(
                source,
                Some(target),
                "VARCHAR2 的目标类型必须是 VARCHAR",
            ));
        } else if target
            .length
            .map_or(true, |target_length| target_length < length)
        {
            issues.push(issue(
                source,
                Some(target),
                "目标 VARCHAR 长度必须大于或等于源 VARCHAR2 长度",
            ));
        }
        if target.character_set.as_deref() != Some("utf8mb4") {
            issues.push(issue(
                source,
                Some(target),
                "VARCHAR 目标列的字符集必须是 utf8mb4",
            ));
        }
    }
}

fn validate_date(
    source: &SourceColumn,
    target: Option<&TargetColumn>,
    issues: &mut Vec<PrecheckIssue>,
) {
    if let Some(target) = target {
        if !target.data_type.eq_ignore_ascii_case("datetime") {
            issues.push(issue(
                source,
                Some(target),
                "DATE 的目标类型必须是 DATETIME",
            ));
        } else if target.datetime_precision != Some(0) {
            issues.push(issue(
                source,
                Some(target),
                "DATE 的目标 DATETIME 小数秒精度必须严格等于 0",
            ));
        }
    }
}

fn issue(source: &SourceColumn, target: Option<&TargetColumn>, rule: &str) -> PrecheckIssue {
    PrecheckIssue {
        column: source.name.clone(),
        source: source_display(source),
        target: target
            .map(target_display)
            .unwrap_or_else(|| "<missing>".to_owned()),
        rule: rule.to_owned(),
        // `suggestion` 归子票 ⑥（sink 预检扩九行 + 下界式）；本票只加字段，恒 `None`（#107）。
        suggestion: None,
    }
}

fn source_display(column: &SourceColumn) -> String {
    match column.data_type.to_uppercase().as_str() {
        "NUMBER" => match (column.precision, column.scale) {
            (Some(precision), Some(scale)) => format!("NUMBER({precision},{scale})"),
            _ => "NUMBER(?,?)".to_owned(),
        },
        "VARCHAR2" => column
            .length
            .map(|length| format!("VARCHAR2({length})"))
            .unwrap_or_else(|| "VARCHAR2(?)".to_owned()),
        "DATE" => "DATE".to_owned(),
        _ => column.data_type.clone(),
    }
}

fn target_display(column: &TargetColumn) -> String {
    column.column_type.to_uppercase()
}

fn target_decimal_shape(column: &TargetColumn) -> Option<(u32, u32)> {
    if !column.data_type.eq_ignore_ascii_case("decimal") {
        return None;
    }
    let precision = u32::try_from(column.precision?).ok()?;
    let scale = u32::try_from(column.scale?).ok()?;
    (1..=65)
        .contains(&precision)
        .then_some(())
        .filter(|_| scale <= 30 && scale <= precision)
        .map(|_| (precision, scale))
}
