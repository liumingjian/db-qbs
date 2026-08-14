use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use chrono::NaiveDate;
use regex::Regex;
use serde_json::json;

use crate::precheck::precheck_with_date_column;
use crate::{
    AbortResponse, ApiError, CreateStagingError, Destination, DropStagingError, OpenRunRequest,
    OpenRunResponse, SinkService, TargetColumn,
};

static RUN_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]{14}_[0-9a-f]{6}$").expect("run id regex must compile"));

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

pub fn build_staging_ddl(
    database: &str,
    staging_table: &str,
    target_columns: &[TargetColumn],
) -> String {
    let mut columns: Vec<_> = target_columns.iter().collect();
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
