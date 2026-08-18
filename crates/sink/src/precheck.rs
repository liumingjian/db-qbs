use std::collections::{HashMap, HashSet};

use crate::{
    classify_column, is_business_date_column, ColumnShape, PrecheckIssue, RangeCheckColumn,
    ShapeRejection, SourceColumn, TargetColumn, TargetShape,
};

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
        .filter(|source| matches!(classify_column(source), ColumnShape::NeedsPrecision))
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
            suggestion: Some("缩短目标表名".to_owned()),
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
            issues.push(issue_with_suggestion(
                source,
                targets.get(&normalized_name).copied(),
                "源端列名重复，按名字无法唯一对齐",
                Some("改源 SQL 使列名唯一".to_owned()),
            ));
        }

        let target = targets.get(&normalized_name).copied();
        let Some(target) = target else {
            issues.push(issue_with_suggestion(
                source,
                None,
                "目标表缺少同名列",
                Some(missing_target_suggestion(source)),
            ));
            validate_source_type(source, None, &mut issues);
            continue;
        };

        validate_source_type(source, Some(target), &mut issues);
        if !target.nullable {
            issues.push(issue_with_suggestion(
                source,
                Some(target),
                "目标列必须可空，不能是 NOT NULL",
                Some("改为 NULL".to_owned()),
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
                suggestion: Some("改源 SQL 补出该列，或从目标表删列".to_owned()),
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
    if source.is_some_and(|column| is_business_date_column(column)) {
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
        rule: "target_date_col 必须对应同名的 Oracle DATE 或 TIMESTAMP(0..6) 源列".to_owned(),
        suggestion: Some("改任务定义的 target_date_col，或改源 SQL 该列类型".to_owned()),
    });
}

fn validate_source_type(
    source: &SourceColumn,
    target: Option<&TargetColumn>,
    issues: &mut Vec<PrecheckIssue>,
) {
    // 推导（源形态 → 目标形状、白名单成员）只有一份定义，在 `db-qbs-shared`（#125）。
    // 下面全是**判定式**——「怎么比」按 ADR-0010 §3.1 归 sink，一行不外移。
    match classify_column(source) {
        ColumnShape::Rejected(rejection) => {
            let (rule, suggestion) = rejection_issue(rejection);
            issues.push(issue_with_suggestion(
                source,
                target,
                &rule,
                Some(suggestion),
            ));
        }
        ColumnShape::NeedsPrecision => validate_unshaped_number(source, target, issues),
        ColumnShape::Resolved(TargetShape::Decimal { precision, scale }) => {
            validate_number(source, target, precision, scale, issues);
        }
        ColumnShape::Resolved(TargetShape::Varchar { length }) => {
            validate_varchar(source, target, length, issues);
        }
        ColumnShape::Resolved(TargetShape::Datetime { fsp }) => {
            validate_datetime(source, target, fsp, issues);
        }
    }
}

/// 推不出形状时的**措辞**。类型化的原因是共用的闭集，中文文字留在 sink（#125 Q6）。
fn rejection_issue(rejection: ShapeRejection) -> (String, String) {
    match rejection {
        ShapeRejection::NumberPrecisionIncomplete => (
            "任务定义未配置 (p,s)，NUMBER 无法判定".to_owned(),
            "在任务定义为该列配 (p,s)".to_owned(),
        ),
        ShapeRejection::DecimalShapeUnrepresentable { precision, scale } => (
            format!("MySQL DECIMAL 无法表达推导形状 DECIMAL({precision},{scale})"),
            "无合法目标形状，需改源 SQL 或 CAST 收窄".to_owned(),
        ),
        ShapeRejection::CharacterLengthMissing => (
            "字符列必须具有可判定的 length".to_owned(),
            "改源 SQL 或 CAST 明确字符长度".to_owned(),
        ),
        ShapeRejection::TimestampFspMissing => (
            "TIMESTAMP 必须具有可判定的 fsp".to_owned(),
            "改源 SQL 或加 CAST 明确为 TIMESTAMP(0..6)".to_owned(),
        ),
        ShapeRejection::TimestampFspTooPrecise { .. } => (
            "TIMESTAMP(n>6) 不在白名单".to_owned(),
            "改源 SQL 加 CAST 收窄到 TIMESTAMP(6)".to_owned(),
        ),
        ShapeRejection::TypeNotWhitelisted => (
            "源类型不在 M3 九行白名单内".to_owned(),
            "改源 SQL 或加 CAST 到支持的类型".to_owned(),
        ),
    }
}

/// 裸 `NUMBER` / 数值表达式列：形状由任务定义配，判定基准取**目标** `DECIMAL` 的 `(p,s)`，
/// 值本身由 ADR-0030 §4.3 第 3.5 步的全量值域校核兜住。
fn validate_unshaped_number(
    source: &SourceColumn,
    target: Option<&TargetColumn>,
    issues: &mut Vec<PrecheckIssue>,
) {
    match target {
        Some(target) if !target.data_type.eq_ignore_ascii_case("decimal") => {
            issues.push(issue_with_suggestion(
                source,
                Some(target),
                "NUMBER 的目标类型必须是 DECIMAL",
                Some("在任务定义为该列配 (p,s)，并将目标列改为对应 DECIMAL".to_owned()),
            ));
        }
        Some(target) if target_decimal_shape(target).is_none() => issues.push(issue_with_suggestion(
            source,
            Some(target),
            "裸 NUMBER / 数值表达式列的目标 DECIMAL 必须具有有效的 precision 和 scale",
            Some("将目标列改为具有有效 (p,s) 的 DECIMAL".to_owned()),
        )),
        Some(_) => {}
        None => issues.push(issue_with_suggestion(
            source,
            target,
            "NUMBER 必须同时具有可判定的 precision 和 scale，裸 NUMBER 与表达式列需要目标 DECIMAL 形状",
            Some("在任务定义为该列配 (p,s)，再在目标表补出对应 DECIMAL 列".to_owned()),
        )),
    }
}

/// `NUMBER` 族的判定式：下界式（#97 / ADR-0031）。
fn validate_number(
    source: &SourceColumn,
    target: Option<&TargetColumn>,
    target_precision: i64,
    target_scale: i64,
    issues: &mut Vec<PrecheckIssue>,
) {
    let Some(target) = target else {
        return;
    };

    if !target.data_type.eq_ignore_ascii_case("decimal") {
        issues.push(issue_with_suggestion(
            source,
            Some(target),
            "NUMBER 的目标类型必须是 DECIMAL",
            Some(format!("改为 DECIMAL({target_precision},{target_scale})")),
        ));
    } else if !target_satisfies_number_lower_bound(
        target,
        target_precision as u64,
        target_scale as u64,
    ) {
        issues.push(issue_with_suggestion(
            source,
            Some(target),
            &number_lower_bound_rule(target, target_precision as u64, target_scale as u64),
            Some(format!("改为 DECIMAL({target_precision},{target_scale})")),
        ));
    }
}

pub(crate) fn range_check_issue(
    source: &SourceColumn,
    target: &TargetColumn,
    range_column: &RangeCheckColumn,
    invalid_rows: u64,
) -> PrecheckIssue {
    issue_with_suggestion(
        source,
        Some(target),
        &format!(
            "值域校核失败：{invalid_rows} 行无法无损写入 DECIMAL({}, {})",
            range_column.precision, range_column.scale
        ),
        Some("调整任务定义和目标 DECIMAL 的 (p,s)，或改源 SQL / CAST 收窄值域".to_owned()),
    )
}

/// 字符族的判定式：下界式 `n' >= n` + 目标字符集必须 `utf8mb4`（ADR-0009 §6）。
fn validate_varchar(
    source: &SourceColumn,
    target: Option<&TargetColumn>,
    length: u64,
    issues: &mut Vec<PrecheckIssue>,
) {
    if let Some(target) = target {
        if !target.data_type.eq_ignore_ascii_case("varchar") {
            issues.push(issue_with_suggestion(
                source,
                Some(target),
                "字符族目标类型必须是 VARCHAR",
                Some(format!("改为 VARCHAR({length})")),
            ));
        } else if target
            .length
            .map_or(true, |target_length| target_length < length)
        {
            issues.push(issue_with_suggestion(
                source,
                Some(target),
                "目标 VARCHAR 长度不足",
                Some(format!("改为 VARCHAR({length})")),
            ));
        }
        if target.character_set.as_deref() != Some("utf8mb4") {
            issues.push(issue_with_suggestion(
                source,
                Some(target),
                "VARCHAR 目标列的字符集必须是 utf8mb4",
                Some("改为 utf8mb4".to_owned()),
            ));
        }
    }
}

/// 日期族的判定式：严格相等（ADR-0030 §3 两边都严格）。
///
/// 措辞里的源族名按**推导出的小数秒位数**取：ADR-0030 §3 定死「一列一型」——
/// `DATETIME(0)` 只来自 `DATE`、`DATETIME(6)` 只来自 `TIMESTAMP(0..6)`，
/// 放宽任一边才会捅破它，而两边都严格正是本条判定式在守的东西。
fn validate_datetime(
    source: &SourceColumn,
    target: Option<&TargetColumn>,
    fsp: u32,
    issues: &mut Vec<PrecheckIssue>,
) {
    let Some(target) = target else {
        return;
    };
    let family = if fsp == 0 { "DATE" } else { "TIMESTAMP" };

    if !target.data_type.eq_ignore_ascii_case("datetime") {
        issues.push(issue_with_suggestion(
            source,
            Some(target),
            &format!("{family} 的目标类型必须是 DATETIME"),
            Some(format!("改为 DATETIME({fsp})")),
        ));
    } else if target.datetime_precision != Some(u64::from(fsp)) {
        issues.push(issue_with_suggestion(
            source,
            Some(target),
            &format!("{family} 的目标 DATETIME 小数秒精度必须严格等于 {fsp}"),
            Some(format!("改为 DATETIME({fsp})")),
        ));
    }
}

fn issue_with_suggestion(
    source: &SourceColumn,
    target: Option<&TargetColumn>,
    rule: &str,
    suggestion: Option<String>,
) -> PrecheckIssue {
    PrecheckIssue {
        column: source.name.clone(),
        source: source_display(source),
        target: target
            .map(target_display)
            .unwrap_or_else(|| "<missing>".to_owned()),
        rule: rule.to_owned(),
        suggestion,
    }
}

fn missing_target_suggestion(source: &SourceColumn) -> String {
    match classify_column(source) {
        ColumnShape::Resolved(shape) => format!("在目标表加列 {shape} NULL"),
        ColumnShape::NeedsPrecision
        | ColumnShape::Rejected(ShapeRejection::NumberPrecisionIncomplete) => {
            "在任务定义为该列配 (p,s)，再在目标表补出对应 DECIMAL 列".to_owned()
        }
        ColumnShape::Rejected(ShapeRejection::DecimalShapeUnrepresentable { .. }) => {
            "无合法目标形状，需改源 SQL 或 CAST 收窄".to_owned()
        }
        ColumnShape::Rejected(ShapeRejection::CharacterLengthMissing) => {
            "改源 SQL 或 CAST 明确字符长度".to_owned()
        }
        ColumnShape::Rejected(_) => "改源 SQL 移除该列，或在目标表补出兼容列".to_owned(),
    }
}

fn target_satisfies_number_lower_bound(
    target: &TargetColumn,
    expected_precision: u64,
    expected_scale: u64,
) -> bool {
    let Some((precision, scale)) = target.precision.zip(target.scale) else {
        return false;
    };
    scale >= expected_scale
        && precision
            .checked_sub(scale)
            .is_some_and(|integer_digits| integer_digits >= expected_precision - expected_scale)
}

fn number_lower_bound_rule(
    target: &TargetColumn,
    expected_precision: u64,
    expected_scale: u64,
) -> String {
    let Some((precision, scale)) = target.precision.zip(target.scale) else {
        return "目标 DECIMAL 必须提供 precision 和 scale，并满足下界式".to_owned();
    };
    let expected_integer_digits = expected_precision - expected_scale;
    let actual_integer_digits = precision.checked_sub(scale);
    let scale_short = scale < expected_scale;
    let integer_short = actual_integer_digits.map_or(true, |integer_digits| {
        integer_digits < expected_integer_digits
    });

    match (scale_short, integer_short) {
        (true, true) => format!(
            "目标标度与整数位均不足：s' = {scale} < s = {expected_scale}；p'-s' = {} < p-s = {expected_integer_digits}",
            actual_integer_digits.unwrap_or(0)
        ),
        (true, false) => format!(
            "目标标度不足：s' = {scale} < s = {expected_scale}，写入按声明标度静默舍入"
        ),
        (false, true) => format!(
            "整数位不足：p'-s' = {} < p-s = {expected_integer_digits}",
            actual_integer_digits.unwrap_or(0)
        ),
        (false, false) => "目标 DECIMAL 不满足下界式".to_owned(),
    }
}

fn source_display(column: &SourceColumn) -> String {
    match column.data_type.to_uppercase().as_str() {
        "NUMBER" => match (column.precision, column.scale) {
            (Some(precision), Some(scale)) => format!("NUMBER({precision},{scale})"),
            _ => "NUMBER(?,?)".to_owned(),
        },
        "VARCHAR2" | "NVARCHAR2" | "CHAR" | "NCHAR" => {
            let data_type = column.data_type.to_uppercase();
            column
                .length
                .map(|length| format!("{data_type}({length})"))
                .unwrap_or_else(|| format!("{data_type}(?)"))
        }
        "DATE" => "DATE".to_owned(),
        "TIMESTAMP" => column
            .fsp
            .map(|fsp| format!("TIMESTAMP({fsp})"))
            .unwrap_or_else(|| "TIMESTAMP(?)".to_owned()),
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
