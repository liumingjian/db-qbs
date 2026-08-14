mod http;
mod mysql_destination;

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};

use chrono::NaiveDate;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub use http::serve;
pub use mysql_destination::MysqlDestination;

static RUN_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]{14}_[0-9a-f]{6}$").expect("run id regex must compile"));

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SinkConfig {
    pub mysql_dsn: String,
    pub database: String,
    pub listen: String,
}

impl SinkConfig {
    pub fn parse(input: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(input)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let input = fs::read_to_string(path)
            .map_err(|error| format!("读取 sink 配置 {} 失败：{error}", path.display()))?;
        Self::parse(&input)
            .map_err(|error| format!("解析 sink 配置 {} 失败：{error}", path.display()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub data_type: String,
    pub precision: Option<i64>,
    pub scale: Option<i64>,
    pub length: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetColumn {
    pub name: String,
    pub column_type: String,
    pub data_type: String,
    pub precision: Option<u64>,
    pub scale: Option<u64>,
    pub length: Option<u64>,
    pub datetime_precision: Option<u64>,
    pub nullable: bool,
    pub character_set: Option<String>,
    pub ordinal: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrecheckIssue {
    pub column: String,
    pub source: String,
    pub target: String,
    pub rule: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRunRequest {
    pub run_id: String,
    pub target_table: String,
    pub target_date_col: String,
    pub biz_date: String,
    pub source_columns: Vec<SourceColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenRunResponse {
    pub run_id: String,
    pub staging_table: String,
    pub columns_checked: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AbortResponse {
    pub run_id: String,
    pub staging_dropped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateStagingError {
    TableExists,
    PermissionDenied,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropStagingError {
    PermissionDenied,
    Other(String),
}

pub trait Destination: Send + Sync {
    fn target_columns(&self, target_table: &str) -> Result<Vec<TargetColumn>, String>;
    fn create_staging(&self, staging_table: &str, ddl: &str) -> Result<(), CreateStagingError>;
    fn drop_staging(&self, staging_table: &str) -> Result<(), DropStagingError>;
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApiError {
    #[serde(skip)]
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    pub run_id: Option<String>,
    pub details: Value,
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

pub struct SinkService<D: Destination> {
    database: String,
    destination: Arc<D>,
    active_runs: Mutex<HashMap<String, String>>,
}

impl<D: Destination> SinkService<D> {
    pub fn new(database: impl Into<String>, destination: Arc<D>) -> Self {
        Self {
            database: database.into(),
            destination,
            active_runs: Mutex::new(HashMap::new()),
        }
    }

    pub fn open(&self, request: OpenRunRequest) -> Result<OpenRunResponse, ApiError> {
        validate_open_request(&request)?;

        let target_columns = self
            .destination
            .target_columns(&request.target_table)
            .map_err(|message| ApiError {
                status: 500,
                code: "STAGING_CREATE_FAILED",
                message: format!(
                    "读取目标表元数据失败：{message}。这是目标端环境故障，目标表未被改动"
                ),
                run_id: Some(request.run_id.clone()),
                details: json!({ "kind": "OTHER" }),
            })?;
        let issues = precheck_with_date_column(
            &request.target_table,
            &request.target_date_col,
            &request.source_columns,
            &target_columns,
        );
        if !issues.is_empty() {
            let total = issues.len();
            return Err(ApiError {
                status: 422,
                code: "PRECHECK_FAILED",
                message: format!("映射预检未通过：一次发现 {total} 项问题，未创建暂存表"),
                run_id: Some(request.run_id),
                details: json!({ "issues": issues, "total": total }),
            });
        }

        let staging_table = format!("{}__stg_{}", request.target_table, request.run_id);
        let ddl = build_staging_ddl(&self.database, &staging_table, &target_columns);
        self.destination
            .create_staging(&staging_table, &ddl)
            .map_err(|error| create_staging_api_error(&request.run_id, &staging_table, error))?;

        self.active_runs
            .lock()
            .expect("active run mutex poisoned")
            .insert(request.run_id.clone(), staging_table.clone());

        Ok(OpenRunResponse {
            run_id: request.run_id,
            staging_table,
            columns_checked: target_columns.len(),
        })
    }

    pub fn abort(&self, run_id: &str) -> Result<AbortResponse, ApiError> {
        let staging_table = self
            .active_runs
            .lock()
            .expect("active run mutex poisoned")
            .get(run_id)
            .cloned();

        let Some(staging_table) = staging_table else {
            return Ok(AbortResponse {
                run_id: run_id.to_owned(),
                staging_dropped: false,
            });
        };

        self.destination
            .drop_staging(&staging_table)
            .map_err(|error| drop_staging_api_error(run_id, &staging_table, error))?;
        self.active_runs
            .lock()
            .expect("active run mutex poisoned")
            .remove(run_id);

        Ok(AbortResponse {
            run_id: run_id.to_owned(),
            staging_dropped: true,
        })
    }
}

fn validate_open_request(request: &OpenRunRequest) -> Result<(), ApiError> {
    let mut problems = Vec::new();
    if !RUN_ID_RE.is_match(&request.run_id) {
        problems.push("run_id 必须是 14 位 UTC 时间、下划线和 6 位小写 hex");
    }
    if request.target_table.is_empty() {
        problems.push("target_table 不能为空");
    }
    if request.target_date_col.is_empty() {
        problems.push("target_date_col 不能为空");
    }
    if NaiveDate::parse_from_str(&request.biz_date, "%Y-%m-%d").is_err() {
        problems.push("biz_date 必须是有效的 YYYY-MM-DD 日历日");
    }
    if request.source_columns.is_empty() {
        problems.push("source_columns 不能为空");
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(ApiError {
            status: 400,
            code: "BAD_REQUEST",
            message: problems.join("；"),
            run_id: Some(request.run_id.clone()),
            details: json!({ "problems": problems }),
        })
    }
}

pub fn precheck(
    target_table: &str,
    source_columns: &[SourceColumn],
    target_columns: &[TargetColumn],
) -> Vec<PrecheckIssue> {
    precheck_inner(target_table, None, source_columns, target_columns)
}

fn precheck_with_date_column(
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
            });
        }
    }

    if let Some(date_column) = target_date_col {
        let source = source_columns
            .iter()
            .find(|column| column.name.eq_ignore_ascii_case(date_column));
        if source.map(|column| !column.data_type.eq_ignore_ascii_case("DATE")) != Some(false) {
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
            });
        }
    }

    issues
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

pub fn build_staging_ddl(
    database: &str,
    staging_table: &str,
    target_columns: &[TargetColumn],
) -> String {
    let mut columns = target_columns.to_vec();
    columns.sort_by_key(|column| column.ordinal);
    let definitions = columns
        .iter()
        .map(|column| {
            let character_set = column
                .character_set
                .as_deref()
                .map(|value| format!(" CHARACTER SET {value}"))
                .unwrap_or_default();
            format!(
                "  {} {}{} NULL",
                quote_identifier(&column.name),
                column.column_type,
                character_set
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    format!(
        "CREATE TABLE {}.{} (\n{}\n)",
        quote_identifier(database),
        quote_identifier(staging_table),
        definitions
    )
}

pub(crate) fn quote_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}

fn create_staging_api_error(
    run_id: &str,
    staging_table: &str,
    error: CreateStagingError,
) -> ApiError {
    match error {
        CreateStagingError::TableExists => ApiError {
            status: 409,
            code: "STAGING_CREATE_FAILED",
            message: format!(
                "暂存表 {staging_table} 已存在，绝不会自动 DROP 或重建；它可能是遗留暂存表，表名时间为 {}",
                run_id_time(run_id)
            ),
            run_id: Some(run_id.to_owned()),
            details: json!({ "kind": "TABLE_EXISTS", "staging_table": staging_table }),
        },
        CreateStagingError::PermissionDenied => ApiError {
            status: 500,
            code: "STAGING_CREATE_FAILED",
            message: format!(
                "创建暂存表 {staging_table} 失败：sink 的 MySQL 账号缺少 CREATE 权限；目标表未被改动"
            ),
            run_id: Some(run_id.to_owned()),
            details: json!({ "kind": "PERMISSION_DENIED", "operation": "CREATE" }),
        },
        CreateStagingError::Other(message) => ApiError {
            status: 500,
            code: "STAGING_CREATE_FAILED",
            message: format!(
                "创建暂存表 {staging_table} 失败：{message}。这是目标端故障，目标表未被改动"
            ),
            run_id: Some(run_id.to_owned()),
            details: json!({ "kind": "OTHER" }),
        },
    }
}

fn drop_staging_api_error(run_id: &str, staging_table: &str, error: DropStagingError) -> ApiError {
    let (message, details) = match error {
        DropStagingError::PermissionDenied => (
            format!(
                "丢弃暂存表 {staging_table} 失败：sink 的 MySQL 账号缺少 DROP 权限；目标表未被改动"
            ),
            json!({ "kind": "PERMISSION_DENIED", "operation": "DROP" }),
        ),
        DropStagingError::Other(message) => (
            format!("丢弃暂存表 {staging_table} 失败：{message}。这是目标端故障，目标表未被改动"),
            json!({ "kind": "OTHER" }),
        ),
    };
    ApiError {
        status: 500,
        code: "STAGING_CREATE_FAILED",
        message,
        run_id: Some(run_id.to_owned()),
        details,
    }
}

fn run_id_time(run_id: &str) -> String {
    let timestamp = &run_id[..14];
    format!(
        "{}-{}-{} {}:{}:{} UTC",
        &timestamp[0..4],
        &timestamp[4..6],
        &timestamp[6..8],
        &timestamp[8..10],
        &timestamp[10..12],
        &timestamp[12..14]
    )
}

pub fn check_connection_settings(
    character_set_client: &str,
    character_set_connection: &str,
    character_set_results: &str,
    sql_mode: &str,
    max_allowed_packet: u64,
) -> Result<(), String> {
    const EXPECTED_CHARSET: &str = "utf8mb4";
    const EXPECTED_SQL_MODE: &str = "STRICT_ALL_TABLES";
    const MIN_PACKET: u64 = 64 * 1024 * 1024;

    let mut problems = Vec::new();
    for (name, actual) in [
        ("character_set_client", character_set_client),
        ("character_set_connection", character_set_connection),
        ("character_set_results", character_set_results),
    ] {
        if actual != EXPECTED_CHARSET {
            problems.push(format!(
                "环境配置错误：{name} 期望 {EXPECTED_CHARSET}，实际 {actual}"
            ));
        }
    }
    if sql_mode != EXPECTED_SQL_MODE {
        problems.push(format!(
            "环境配置错误：sql_mode 期望完整值 {EXPECTED_SQL_MODE}，实际 {sql_mode}"
        ));
    }
    if max_allowed_packet < MIN_PACKET {
        problems.push(format!(
            "环境配置错误：max_allowed_packet 期望至少 {MIN_PACKET} 字节，实际 {max_allowed_packet} 字节；请调整 MySQL 配置，不要排查业务数据"
        ));
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("；"))
    }
}
