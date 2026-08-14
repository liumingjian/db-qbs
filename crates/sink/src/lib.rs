mod http;
mod mysql_destination;
mod precheck;
mod service;

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use http::serve;
pub use mysql_destination::{check_connection_settings, MysqlDestination};
pub use precheck::precheck;
pub use service::build_staging_ddl;

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

pub struct SinkService<D: Destination> {
    database: String,
    destination: Arc<D>,
    active_runs: Mutex<HashMap<String, String>>,
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
