use std::collections::HashSet;
use std::fs::{self, OpenOptions, Permissions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::secret::SecretBox;

const DATABASE_FILE: &str = "db-qbs.sqlite3";
const MAX_RECIPIENTS: usize = 50;
pub const ADMIN_DISABLED_REASON: &str = "管理员已停用邮件告警";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EmailProviderPreset {
    TencentExmail,
    Generic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SmtpSecurity {
    ImplicitTls,
    Starttls,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmailAlertSettings {
    pub enabled: bool,
    pub provider_preset: EmailProviderPreset,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: SmtpSecurity,
    pub smtp_username: String,
    pub has_smtp_secret: bool,
    pub sender_address: String,
    pub sender_name: String,
    pub recipients: Vec<String>,
    pub max_retry_hours: u8,
    pub instance_name: String,
    pub external_base_url: Option<String>,
    pub latest_test_result: Option<EmailTestResult>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EmailTestStatus {
    Success,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmailTestResult {
    pub status: EmailTestStatus,
    pub tested_at: String,
    pub error: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmailAlertSettingsInput {
    pub enabled: bool,
    pub provider_preset: EmailProviderPreset,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_security: SmtpSecurity,
    pub smtp_username: String,
    #[serde(default)]
    pub smtp_secret: String,
    pub sender_address: String,
    pub sender_name: String,
    pub recipients: Vec<String>,
    pub max_retry_hours: u8,
    pub instance_name: String,
    pub external_base_url: Option<String>,
}

/// Current delivery credentials for the SMTP adapter. Unlike the API view, this type is never
/// serializable or debuggable because it contains the plaintext secret in process memory.
pub struct EmailDeliverySettings {
    pub host: String,
    pub port: u16,
    pub security: SmtpSecurity,
    pub username: String,
    pub secret: String,
    pub sender_address: String,
    pub sender_name: String,
}

struct StoredSettings {
    view: EmailAlertSettings,
    sealed_secret: String,
}

pub struct EmailAlertStore {
    connection: Mutex<Connection>,
    secrets: SecretBox,
}

impl EmailAlertStore {
    pub fn open(data_dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(data_dir)
            .map_err(|error| format!("创建 source 数据目录失败：{error}"))?;
        let database_path = data_dir.join(DATABASE_FILE);
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&database_path)
            .map_err(|error| format!("创建 SQLite 库文件失败：{error}"))?;
        fs::set_permissions(&database_path, Permissions::from_mode(0o600))
            .map_err(|error| format!("设置 SQLite 库文件权限失败：{error}"))?;

        let connection = Connection::open(database_path)
            .map_err(|error| format!("打开 SQLite 库文件失败：{error}"))?;
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS email_alert_settings (
                    singleton_id       INTEGER PRIMARY KEY CHECK (singleton_id = 1),
                    enabled            INTEGER NOT NULL,
                    provider_preset    TEXT NOT NULL,
                    smtp_host          TEXT NOT NULL,
                    smtp_port          INTEGER NOT NULL,
                    smtp_security      TEXT NOT NULL,
                    smtp_username      TEXT NOT NULL,
                    smtp_secret        TEXT NOT NULL,
                    sender_address     TEXT NOT NULL,
                    sender_name        TEXT NOT NULL,
                    recipients         TEXT NOT NULL,
                    max_retry_hours    INTEGER NOT NULL,
                    instance_name      TEXT NOT NULL,
                    external_base_url  TEXT
                );
                INSERT OR IGNORE INTO email_alert_settings (
                    singleton_id, enabled, provider_preset, smtp_host, smtp_port, smtp_security,
                    smtp_username, smtp_secret, sender_address, sender_name, recipients,
                    max_retry_hours, instance_name, external_base_url
                ) VALUES
                    (1, 0, 'TENCENT_EXMAIL', 'smtp.exmail.qq.com', 465, 'IMPLICIT_TLS',
                     '', '', '', '', '[]', 24, 'db-qbs', NULL);",
            )
            .map_err(|error| format!("初始化 SQLite 邮件告警设置失败：{error}"))?;
        add_column_if_missing(&connection, "latest_test_status", "TEXT")?;
        add_column_if_missing(&connection, "latest_test_at", "TEXT")?;
        add_column_if_missing(&connection, "latest_test_error", "TEXT")?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|error| format!("配置 SQLite 忙等待失败：{error}"))?;

        Ok(Self {
            connection: Mutex::new(connection),
            secrets: SecretBox::open(data_dir)?,
        })
    }

    pub fn get(&self) -> Result<EmailAlertSettings, String> {
        Ok(self.stored()?.view)
    }

    pub fn update(&self, mut input: EmailAlertSettingsInput) -> Result<EmailAlertSettings, String> {
        normalize_and_validate(&mut input)?;
        let replacement_secret = if input.smtp_secret.is_empty() {
            None
        } else {
            Some(self.secrets.seal(&input.smtp_secret)?)
        };
        let recipients = serde_json::to_string(&input.recipients)
            .map_err(|error| format!("序列化告警收件人失败：{error}"))?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| format!("开始 SQLite 邮件告警设置事务失败：{error}"))?;
        let (was_enabled, existing_secret): (bool, String) = transaction
            .query_row(
                "SELECT enabled, smtp_secret FROM email_alert_settings WHERE singleton_id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| format!("读取 SQLite 邮件告警设置失败：{error}"))?;
        let sealed_secret = replacement_secret.unwrap_or(existing_secret);
        if input.enabled {
            validate_complete(&input, !sealed_secret.is_empty())?;
        }
        transaction
            .execute(
                "UPDATE email_alert_settings SET
                    enabled = ?1, provider_preset = ?2, smtp_host = ?3, smtp_port = ?4,
                    smtp_security = ?5, smtp_username = ?6, smtp_secret = ?7,
                    sender_address = ?8, sender_name = ?9, recipients = ?10,
                    max_retry_hours = ?11, instance_name = ?12, external_base_url = ?13
                 WHERE singleton_id = 1",
                params![
                    input.enabled,
                    preset_name(input.provider_preset),
                    input.smtp_host,
                    input.smtp_port,
                    security_name(input.smtp_security),
                    input.smtp_username,
                    sealed_secret,
                    input.sender_address,
                    input.sender_name,
                    recipients,
                    input.max_retry_hours,
                    input.instance_name,
                    input.external_base_url,
                ],
            )
            .map_err(|error| format!("更新 SQLite 邮件告警设置失败：{error}"))?;
        if was_enabled && !input.enabled {
            let has_deliveries: bool = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master
                                      WHERE type = 'table' AND name = 'email_deliveries')",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| format!("检查 SQLite 告警投递表失败：{error}"))?;
            if has_deliveries {
                transaction
                    .execute(
                        "UPDATE email_deliveries
                            SET state = 'NOT_SENT', next_attempt_at = NULL, last_error = ?1
                          WHERE state = 'PENDING'",
                        [ADMIN_DISABLED_REASON],
                    )
                    .map_err(|error| format!("终止 SQLite 待发送告警失败：{error}"))?;
            }
        }
        transaction
            .commit()
            .map_err(|error| format!("提交 SQLite 邮件告警设置失败：{error}"))?;
        drop(connection);
        self.get()
    }

    pub fn delivery_settings(&self) -> Result<Option<EmailDeliverySettings>, String> {
        let stored = self.stored()?;
        if !stored.view.enabled || stored.sealed_secret.is_empty() {
            return Ok(None);
        }
        Ok(Some(EmailDeliverySettings {
            host: stored.view.smtp_host,
            port: stored.view.smtp_port,
            security: stored.view.smtp_security,
            username: stored.view.smtp_username,
            secret: self.secrets.open_secret(&stored.sealed_secret)?,
            sender_address: stored.view.sender_address,
            sender_name: stored.view.sender_name,
        }))
    }

    pub fn test_delivery_settings(&self) -> Result<EmailDeliverySettings, String> {
        let stored = self.stored()?;
        validate_complete_view(&stored.view, !stored.sealed_secret.is_empty())?;
        Ok(EmailDeliverySettings {
            host: stored.view.smtp_host,
            port: stored.view.smtp_port,
            security: stored.view.smtp_security,
            username: stored.view.smtp_username,
            secret: self.secrets.open_secret(&stored.sealed_secret)?,
            sender_address: stored.view.sender_address,
            sender_name: stored.view.sender_name,
        })
    }

    pub fn record_test_result(&self, result: &EmailTestResult) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE email_alert_settings SET latest_test_status = ?1,
                    latest_test_at = ?2, latest_test_error = ?3 WHERE singleton_id = 1",
                params![
                    test_status_name(result.status),
                    result.tested_at,
                    result.error,
                ],
            )
            .map_err(|error| format!("保存 SQLite 测试邮件结果失败：{error}"))?;
        Ok(())
    }

    fn stored(&self) -> Result<StoredSettings, String> {
        self.connection()?
            .query_row(
                "SELECT enabled, provider_preset, smtp_host, smtp_port, smtp_security,
                        smtp_username, smtp_secret, sender_address, sender_name, recipients,
                        max_retry_hours, instance_name, external_base_url,
                        latest_test_status, latest_test_at, latest_test_error
                 FROM email_alert_settings WHERE singleton_id = 1",
                [],
                |row| {
                    let recipients_json: String = row.get(9)?;
                    let recipients = serde_json::from_str(&recipients_json).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            9,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                    let preset: String = row.get(1)?;
                    let security: String = row.get(4)?;
                    let sealed_secret: String = row.get(6)?;
                    let latest_status: Option<String> = row.get(13)?;
                    let latest_at: Option<String> = row.get(14)?;
                    let latest_error: Option<String> = row.get(15)?;
                    let latest_test_result = match (latest_status, latest_at) {
                        (Some(status), Some(tested_at)) => Some(EmailTestResult {
                            status: parse_test_status(&status)?,
                            tested_at,
                            error: latest_error,
                        }),
                        _ => None,
                    };
                    Ok(StoredSettings {
                        view: EmailAlertSettings {
                            enabled: row.get(0)?,
                            provider_preset: parse_preset(&preset)?,
                            smtp_host: row.get(2)?,
                            smtp_port: row.get(3)?,
                            smtp_security: parse_security(&security)?,
                            smtp_username: row.get(5)?,
                            has_smtp_secret: !sealed_secret.is_empty(),
                            sender_address: row.get(7)?,
                            sender_name: row.get(8)?,
                            recipients,
                            max_retry_hours: row.get(10)?,
                            instance_name: row.get(11)?,
                            external_base_url: row.get(12)?,
                            latest_test_result,
                        },
                        sealed_secret,
                    })
                },
            )
            .optional()
            .map_err(|error| format!("读取 SQLite 邮件告警设置失败：{error}"))?
            .ok_or_else(|| "SQLite 邮件告警设置缺少单例行".to_owned())
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.connection
            .lock()
            .map_err(|_| "SQLite 邮件告警设置库的锁已损坏".to_owned())
    }
}

fn add_column_if_missing(
    connection: &Connection,
    name: &str,
    sql_type: &str,
) -> Result<(), String> {
    let columns = connection
        .prepare("PRAGMA table_info(email_alert_settings)")
        .and_then(|mut statement| {
            let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|error| format!("读取 SQLite 邮件告警设置结构失败：{error}"))?;
    if !columns.iter().any(|column| column == name) {
        connection
            .execute_batch(&format!(
                "ALTER TABLE email_alert_settings ADD COLUMN {name} {sql_type}"
            ))
            .map_err(|error| format!("迁移 SQLite 邮件告警设置失败：{error}"))?;
    }
    Ok(())
}

fn normalize_and_validate(input: &mut EmailAlertSettingsInput) -> Result<(), String> {
    input.smtp_host = input.smtp_host.trim().to_owned();
    input.smtp_username = input.smtp_username.trim().to_owned();
    input.sender_address = input.sender_address.trim().to_owned();
    input.sender_name = input.sender_name.trim().to_owned();
    input.instance_name = input.instance_name.trim().to_owned();
    if input.smtp_port == 0 {
        return Err("SMTP 端口必须在 1 到 65535 之间".to_owned());
    }
    if !input.smtp_host.is_empty() {
        validate_smtp_host(&input.smtp_host)?;
    }
    if input.max_retry_hours > 168 {
        return Err("最大重试时长必须是 0 到 168 的整数小时".to_owned());
    }
    if input.instance_name.is_empty() {
        return Err("实例名称不能为空".to_owned());
    }
    if !input.sender_address.is_empty() && !valid_email_address(&input.sender_address) {
        return Err("发件人地址不是有效的电子邮箱地址".to_owned());
    }

    let mut seen = HashSet::new();
    let mut recipients = Vec::with_capacity(input.recipients.len());
    for address in &input.recipients {
        let address = address.trim();
        if address.is_empty() {
            continue;
        }
        if !valid_email_address(address) {
            return Err(format!("收件人地址不是有效的电子邮箱地址：{address}"));
        }
        if seen.insert(address.to_ascii_lowercase()) {
            recipients.push(address.to_owned());
        }
    }
    if recipients.len() > MAX_RECIPIENTS {
        return Err(format!("收件人不能超过 {MAX_RECIPIENTS} 个"));
    }
    input.recipients = recipients;

    input.external_base_url = match input.external_base_url.take() {
        Some(value) if value.trim().is_empty() => None,
        Some(value) => Some(validate_origin(value.trim())?),
        None => None,
    };
    Ok(())
}

fn validate_complete(input: &EmailAlertSettingsInput, has_secret: bool) -> Result<(), String> {
    if input.smtp_host.is_empty()
        || input.smtp_username.is_empty()
        || !has_secret
        || input.sender_address.is_empty()
        || input.sender_name.is_empty()
        || input.recipients.is_empty()
    {
        return Err("启用邮件告警前必须填写完整 SMTP、发件人设置和至少一个收件人".to_owned());
    }
    Ok(())
}

fn validate_complete_view(input: &EmailAlertSettings, has_secret: bool) -> Result<(), String> {
    if input.smtp_host.is_empty()
        || input.smtp_username.is_empty()
        || !has_secret
        || input.sender_address.is_empty()
        || input.sender_name.is_empty()
        || input.recipients.is_empty()
    {
        return Err("发送测试邮件前必须保存完整 SMTP、发件人设置和至少一个收件人".to_owned());
    }
    Ok(())
}

fn validate_origin(value: &str) -> Result<String, String> {
    let url = Url::parse(value).map_err(|_| "外部访问地址不是有效 URL".to_owned())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err("外部访问地址必须是没有路径、凭据、查询或片段的 HTTP(S) origin".to_owned());
    }
    Ok(url.origin().ascii_serialization())
}

fn validate_smtp_host(value: &str) -> Result<(), String> {
    let url = Url::parse(&format!("smtp://{value}"))
        .map_err(|_| "SMTP 主机不是有效的主机名或 IP 地址".to_owned())?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("SMTP 主机只能填写主机名或 IP 地址，端口请单独填写".to_owned());
    }
    Ok(())
}

fn valid_email_address(value: &str) -> bool {
    if value.is_empty()
        || value.len() > 254
        || !value.is_ascii()
        || value.chars().any(char::is_whitespace)
    {
        return false;
    }
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    if local.is_empty() || local.len() > 64 || domain.is_empty() || domain.contains('@') {
        return false;
    }
    let atom_char = |ch: char| ch.is_ascii_alphanumeric() || "!#$%&'*+-/=?^_`{|}~".contains(ch);
    if local.starts_with('.')
        || local.ends_with('.')
        || local
            .split('.')
            .any(|atom| atom.is_empty() || !atom.chars().all(atom_char))
    {
        return false;
    }
    domain.len() <= 253
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        })
}

fn preset_name(value: EmailProviderPreset) -> &'static str {
    match value {
        EmailProviderPreset::TencentExmail => "TENCENT_EXMAIL",
        EmailProviderPreset::Generic => "GENERIC",
    }
}

fn security_name(value: SmtpSecurity) -> &'static str {
    match value {
        SmtpSecurity::ImplicitTls => "IMPLICIT_TLS",
        SmtpSecurity::Starttls => "STARTTLS",
    }
}

fn parse_preset(value: &str) -> rusqlite::Result<EmailProviderPreset> {
    match value {
        "TENCENT_EXMAIL" => Ok(EmailProviderPreset::TencentExmail),
        "GENERIC" => Ok(EmailProviderPreset::Generic),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_security(value: &str) -> rusqlite::Result<SmtpSecurity> {
    match value {
        "IMPLICIT_TLS" => Ok(SmtpSecurity::ImplicitTls),
        "STARTTLS" => Ok(SmtpSecurity::Starttls),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn test_status_name(value: EmailTestStatus) -> &'static str {
    match value {
        EmailTestStatus::Success => "SUCCESS",
        EmailTestStatus::Failed => "FAILED",
    }
}

fn parse_test_status(value: &str) -> rusqlite::Result<EmailTestStatus> {
    match value {
        "SUCCESS" => Ok(EmailTestStatus::Success),
        "FAILED" => Ok(EmailTestStatus::Failed),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_mailboxes_and_origins() {
        assert!(valid_email_address("ops+alerts@example.com"));
        assert!(valid_email_address("ops@example"));
        assert!(!valid_email_address("ops@-example.com"));
        assert!(!valid_email_address("ops..alerts@example.com"));
        assert_eq!(
            validate_origin("https://example.com/").unwrap(),
            "https://example.com"
        );
        assert!(validate_origin("https://example.com/db-qbs").is_err());
        assert!(validate_origin("smtp://example.com").is_err());
        assert!(validate_smtp_host("smtp.example.com").is_ok());
        assert!(validate_smtp_host("smtp.example.com:587").is_err());
    }
}
