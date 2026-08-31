use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::{multipart_mail, Clock, EmailAlertStore, MailTransport, RunHistory};

const DATABASE_FILE: &str = "db-qbs.sqlite3";
pub const RETRY_BASE_SECONDS: i64 = 60;
pub const RETRY_CAP_SECONDS: i64 = 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AlertDeliveryState {
    Pending,
    Sent,
    PartiallyFailed,
    Failed,
    NotSent,
    Suppressed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunAlertSummary {
    pub alert_id: String,
    pub delivery_state: AlertDeliveryState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EmailDeliveryState {
    Pending,
    Sent,
    Failed,
    NotSent,
    Suppressed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmailDeliveryHistory {
    pub delivery_id: String,
    pub alert_id: String,
    pub run_record_id: String,
    pub task_id: String,
    pub task_name: String,
    pub failed_at: String,
    pub recipient: String,
    pub state: EmailDeliveryState,
    pub attempt_count: u64,
    pub first_attempt_at: Option<String>,
    pub last_attempt_at: Option<String>,
    pub next_attempt_at: Option<String>,
    pub retry_window_started_at: String,
    pub retry_deadline_at: String,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualRetryOutcome {
    Retried(EmailDeliveryHistory),
    NotFound,
    Ineligible,
}

#[derive(Debug, Clone)]
pub struct AlertOutboxStore {
    database_path: PathBuf,
}

impl AlertOutboxStore {
    pub fn open(data_dir: &Path) -> Result<Self, String> {
        fs::create_dir_all(data_dir)
            .map_err(|error| format!("创建 source 数据目录失败：{error}"))?;
        let store = Self {
            database_path: data_dir.join(DATABASE_FILE),
        };
        let connection = store.connection()?;
        initialize_alert_tables(&connection)?;
        Ok(store)
    }

    pub fn summary_for_run(&self, run_record_id: &str) -> Result<Option<RunAlertSummary>, String> {
        let connection = self.connection()?;
        let alert_id: Option<String> = connection
            .query_row(
                "SELECT alert_id FROM alerts WHERE run_record_id = ?1",
                [run_record_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("查询运行告警状态失败：{error}"))?;
        let Some(alert_id) = alert_id else {
            return Ok(None);
        };
        let mut statement = connection
            .prepare("SELECT state FROM email_deliveries WHERE alert_id = ?1")
            .map_err(|error| format!("查询运行告警状态失败：{error}"))?;
        let states = statement
            .query_map([&alert_id], |row| row.get::<_, String>(0))
            .map_err(|error| format!("查询运行告警状态失败：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("查询运行告警状态失败：{error}"))?;
        Ok(Some(RunAlertSummary {
            alert_id,
            delivery_state: aggregate_state(&states),
        }))
    }

    /// Runs one deterministic worker pass. Due times are persisted, so callers never need to sleep.
    pub fn run_due_attempts(
        &self,
        settings_store: &EmailAlertStore,
        transport: &dyn MailTransport,
        clock: &dyn Clock,
    ) -> Result<usize, String> {
        let now = clock.now();
        self.expire_overdue(now)?;
        let due = self.due_deliveries(now)?;
        let mut attempted = 0;
        for delivery in due {
            // Reload for every recipient and every retry. Only the recipient is snapshotted.
            let delivery_settings = match settings_store.delivery_settings()? {
                Some(settings) => settings,
                None => break,
            };
            let settings = settings_store.get()?;
            let (subject, plain, html) = render_message(
                &delivery,
                &settings.instance_name,
                settings.external_base_url.as_deref(),
            );
            let mail = multipart_mail(
                &delivery_settings.sender_address,
                &delivery_settings.sender_name,
                &delivery.recipient,
                &subject,
                plain,
                html,
            )?;
            let attempt_time = clock.now();
            let result = transport.send(&delivery_settings, &mail);
            self.record_attempt(
                &delivery.delivery_id,
                attempt_time,
                settings.max_retry_hours,
                result,
            )?;
            attempted += 1;
        }
        Ok(attempted)
    }

    /// Compatibility name retained for callers introduced with the first outbox path.
    pub fn run_first_attempts(
        &self,
        settings_store: &EmailAlertStore,
        transport: &dyn MailTransport,
        clock: &dyn Clock,
    ) -> Result<usize, String> {
        self.run_due_attempts(settings_store, transport, clock)
    }

    pub fn delivery_history(
        &self,
        run_record_id: Option<&str>,
    ) -> Result<Vec<EmailDeliveryHistory>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT d.delivery_id, d.alert_id, a.run_record_id, a.task_id, a.task_name,
                        a.failed_at, d.recipient_snapshot, d.state, d.attempt_count,
                        d.first_attempt_at, d.last_attempt_at, d.next_attempt_at,
                        d.retry_window_started_at, d.retry_deadline_at, d.last_error
                   FROM email_deliveries d
                   JOIN alerts a ON a.alert_id = d.alert_id
                  WHERE ?1 IS NULL OR a.run_record_id = ?1
                  ORDER BY a.failed_at DESC, d.delivery_id",
            )
            .map_err(|error| format!("查询告警投递历史失败：{error}"))?;
        let deliveries = statement
            .query_map([run_record_id], delivery_history_from_row)
            .map_err(|error| format!("读取告警投递历史失败：{error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("读取告警投递历史失败：{error}"))?;
        Ok(deliveries)
    }

    pub fn manual_retry(
        &self,
        delivery_id: &str,
        now: DateTime<Utc>,
        max_retry_hours: u8,
    ) -> Result<ManualRetryOutcome, String> {
        let started_at = now.to_rfc3339();
        let deadline = retry_deadline(now, max_retry_hours).to_rfc3339();
        let changed = self
            .connection()?
            .execute(
                "UPDATE email_deliveries
                    SET state = 'PENDING', next_attempt_at = ?1,
                        retry_window_started_at = ?1, retry_deadline_at = ?2
                  WHERE delivery_id = ?3 AND state = 'FAILED'",
                params![started_at, deadline, delivery_id],
            )
            .map_err(|error| format!("重新安排告警投递失败：{error}"))?;
        if changed == 0 {
            let exists: bool = self
                .connection()?
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM email_deliveries WHERE delivery_id = ?1)",
                    [delivery_id],
                    |row| row.get(0),
                )
                .map_err(|error| format!("查询告警投递失败：{error}"))?;
            return Ok(if exists {
                ManualRetryOutcome::Ineligible
            } else {
                ManualRetryOutcome::NotFound
            });
        }
        let history = self
            .delivery_by_id(delivery_id)?
            .ok_or_else(|| "重新安排后的告警投递不存在".to_owned())?;
        Ok(ManualRetryOutcome::Retried(history))
    }

    fn delivery_by_id(&self, delivery_id: &str) -> Result<Option<EmailDeliveryHistory>, String> {
        self.connection()?
            .query_row(
                "SELECT d.delivery_id, d.alert_id, a.run_record_id, a.task_id, a.task_name,
                        a.failed_at, d.recipient_snapshot, d.state, d.attempt_count,
                        d.first_attempt_at, d.last_attempt_at, d.next_attempt_at,
                        d.retry_window_started_at, d.retry_deadline_at, d.last_error
                   FROM email_deliveries d
                   JOIN alerts a ON a.alert_id = d.alert_id
                  WHERE d.delivery_id = ?1",
                [delivery_id],
                delivery_history_from_row,
            )
            .optional()
            .map_err(|error| format!("查询告警投递失败：{error}"))
    }

    fn due_deliveries(&self, now: DateTime<Utc>) -> Result<Vec<PendingDelivery>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT d.delivery_id, d.recipient_snapshot, a.alert_id, a.run_record_id,
                        a.run_id, a.task_id, a.task_name, a.run_trigger, a.failed_at,
                        a.failure_category, a.safe_explanation
                   FROM email_deliveries d
                   JOIN alerts a ON a.alert_id = d.alert_id
                  WHERE d.state = 'PENDING'
                    AND (d.attempt_count = 0 OR d.next_attempt_at IS NULL OR d.next_attempt_at <= ?1)
                    AND (d.attempt_count = 0 OR d.retry_deadline_at >= ?1)
                  ORDER BY d.delivery_id",
            )
            .map_err(|error| format!("查询待发送告警失败：{error}"))?;
        let rows = statement
            .query_map([now.to_rfc3339()], |row| {
                Ok(PendingDelivery {
                    delivery_id: row.get(0)?,
                    recipient: row.get(1)?,
                    alert_id: row.get(2)?,
                    run_record_id: row.get(3)?,
                    run_id: row.get(4)?,
                    task_id: row.get(5)?,
                    task_name: row.get(6)?,
                    trigger: row.get(7)?,
                    failed_at: row.get(8)?,
                    failure_category: row.get(9)?,
                    safe_explanation: row.get(10)?,
                })
            })
            .map_err(|error| format!("读取待发送告警失败：{error}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("读取待发送告警失败：{error}"))
    }

    fn record_attempt(
        &self,
        delivery_id: &str,
        now: DateTime<Utc>,
        max_retry_hours: u8,
        result: Result<(), crate::MailTransportError>,
    ) -> Result<(), String> {
        let connection = self.connection()?;
        let (attempt_count, started_at, deadline): (u64, Option<String>, Option<String>) =
            connection
                .query_row(
                    "SELECT attempt_count, retry_window_started_at, retry_deadline_at
                   FROM email_deliveries WHERE delivery_id = ?1 AND state = 'PENDING'",
                    [delivery_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(|error| format!("读取告警重试状态失败：{error}"))?;
        let started_at = started_at.unwrap_or_else(|| now.to_rfc3339());
        let deadline = deadline
            .map(|value| parse_timestamp(&value))
            .transpose()?
            .unwrap_or_else(|| retry_deadline(now, max_retry_hours));
        let next_count = attempt_count + 1;
        let (state, next_attempt_at, error) = match result {
            Ok(()) => ("SENT", None, None),
            Err(error) => {
                let next = now + retry_delay(next_count);
                if next <= deadline {
                    (
                        "PENDING",
                        Some(next.to_rfc3339()),
                        Some(error.sanitized_message()),
                    )
                } else {
                    ("FAILED", None, Some(error.sanitized_message()))
                }
            }
        };
        connection
            .execute(
                "UPDATE email_deliveries
                    SET state = ?1, attempt_count = attempt_count + 1,
                        first_attempt_at = COALESCE(first_attempt_at, ?2),
                        last_attempt_at = ?2, next_attempt_at = ?3,
                        retry_window_started_at = ?4, retry_deadline_at = ?5,
                        last_error = ?6
                  WHERE delivery_id = ?7 AND state = 'PENDING' AND attempt_count = ?8",
                params![
                    state,
                    now.to_rfc3339(),
                    next_attempt_at,
                    started_at,
                    deadline.to_rfc3339(),
                    error,
                    delivery_id,
                    attempt_count,
                ],
            )
            .map_err(|error| format!("记录告警发送结果失败：{error}"))?;
        Ok(())
    }

    fn expire_overdue(&self, now: DateTime<Utc>) -> Result<(), String> {
        self.connection()?
            .execute(
                "UPDATE email_deliveries SET state = 'FAILED', next_attempt_at = NULL
                  WHERE state = 'PENDING' AND attempt_count > 0
                    AND retry_deadline_at < ?1",
                [now.to_rfc3339()],
            )
            .map_err(|error| format!("结束超期告警投递失败：{error}"))?;
        Ok(())
    }

    fn connection(&self) -> Result<Connection, String> {
        let connection = Connection::open(&self.database_path)
            .map_err(|error| format!("打开 SQLite 告警库失败：{error}"))?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| format!("配置 SQLite 告警忙等待失败：{error}"))?;
        Ok(connection)
    }
}

fn aggregate_state(states: &[String]) -> AlertDeliveryState {
    if states.is_empty() {
        return AlertDeliveryState::NotSent;
    }
    let count = |wanted: &str| {
        states
            .iter()
            .filter(|state| state.as_str() == wanted)
            .count()
    };
    let pending = count("PENDING");
    let sent = count("SENT");
    let failed = count("FAILED");
    if pending > 0 {
        AlertDeliveryState::Pending
    } else if sent == states.len() {
        AlertDeliveryState::Sent
    } else if sent > 0 {
        AlertDeliveryState::PartiallyFailed
    } else if failed > 0 {
        AlertDeliveryState::Failed
    } else if count("SUPPRESSED") > 0 {
        AlertDeliveryState::Suppressed
    } else {
        AlertDeliveryState::NotSent
    }
}

fn retry_delay(attempt_count: u64) -> TimeDelta {
    let exponent = attempt_count.saturating_sub(1).min(6) as u32;
    TimeDelta::seconds((RETRY_BASE_SECONDS * (1_i64 << exponent)).min(RETRY_CAP_SECONDS))
}

fn retry_deadline(started_at: DateTime<Utc>, max_retry_hours: u8) -> DateTime<Utc> {
    started_at + TimeDelta::hours(i64::from(max_retry_hours))
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| format!("读取告警重试时间失败：{error}"))
}

fn parse_delivery_state(value: &str) -> rusqlite::Result<EmailDeliveryState> {
    match value {
        "PENDING" => Ok(EmailDeliveryState::Pending),
        "SENT" => Ok(EmailDeliveryState::Sent),
        "FAILED" => Ok(EmailDeliveryState::Failed),
        "NOT_SENT" => Ok(EmailDeliveryState::NotSent),
        "SUPPRESSED" => Ok(EmailDeliveryState::Suppressed),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn delivery_history_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EmailDeliveryHistory> {
    let state: String = row.get(7)?;
    Ok(EmailDeliveryHistory {
        delivery_id: row.get(0)?,
        alert_id: row.get(1)?,
        run_record_id: row.get(2)?,
        task_id: row.get(3)?,
        task_name: row.get(4)?,
        failed_at: row.get(5)?,
        recipient: row.get(6)?,
        state: parse_delivery_state(&state)?,
        attempt_count: row.get(8)?,
        first_attempt_at: row.get(9)?,
        last_attempt_at: row.get(10)?,
        next_attempt_at: row.get(11)?,
        retry_window_started_at: row.get(12)?,
        retry_deadline_at: row.get(13)?,
        last_error: row.get(14)?,
    })
}

pub(crate) fn initialize_alert_tables(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS alerts (
                alert_id            TEXT PRIMARY KEY NOT NULL,
                run_record_id        TEXT NOT NULL UNIQUE,
                run_id               TEXT,
                task_id              TEXT NOT NULL,
                task_name            TEXT NOT NULL,
                run_trigger          TEXT NOT NULL,
                failed_at            TEXT NOT NULL,
                failure_category     TEXT NOT NULL,
                safe_explanation     TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS email_deliveries (
                delivery_id          TEXT PRIMARY KEY NOT NULL,
                alert_id             TEXT NOT NULL,
                recipient_snapshot   TEXT NOT NULL,
                state                TEXT NOT NULL CHECK (
                                         state IN ('PENDING', 'SENT', 'FAILED', 'NOT_SENT', 'SUPPRESSED')
                                     ),
                attempt_count        INTEGER NOT NULL DEFAULT 0,
                first_attempt_at     TEXT,
                last_attempt_at      TEXT,
                next_attempt_at      TEXT,
                retry_window_started_at TEXT,
                retry_deadline_at    TEXT,
                last_error           TEXT,
                UNIQUE(alert_id, recipient_snapshot)
             );
             CREATE INDEX IF NOT EXISTS email_deliveries_pending
                 ON email_deliveries(state, attempt_count);",
        )
        .map_err(|error| format!("初始化 SQLite 告警表失败：{error}"))
}

pub(crate) fn insert_alert_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    history: &RunHistory,
) -> Result<(), String> {
    if !is_alertable(history) {
        return Ok(());
    }
    let alert_id = format!("alert-{}", history.run_record_id);
    let failed_at = history
        .finished_at
        .as_deref()
        .unwrap_or(&history.started_at);
    transaction
        .execute(
            "INSERT OR IGNORE INTO alerts (
                alert_id, run_record_id, run_id, task_id, task_name, run_trigger,
                failed_at, failure_category, safe_explanation
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                alert_id,
                history.run_record_id,
                history.run_id,
                history.task_id,
                history.task_name,
                history.trigger,
                failed_at,
                history.failure_kind.as_deref().unwrap_or("UNKNOWN"),
                safe_explanation(history.failure_kind.as_deref()),
            ],
        )
        .map_err(|error| format!("创建 SQLite 运行告警失败：{error}"))?;

    let has_settings_table: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'email_alert_settings')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("检查 SQLite 告警设置失败：{error}"))?;
    let settings: Option<(String, u8)> = if has_settings_table {
        transaction
            .query_row(
                "SELECT recipients, max_retry_hours FROM email_alert_settings
                  WHERE singleton_id = 1 AND enabled = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| format!("读取 SQLite 告警收件人失败：{error}"))?
    } else {
        None
    };
    let recipients: Vec<String> = settings
        .as_ref()
        .map(|(recipients, _)| serde_json::from_str(recipients))
        .transpose()
        .map_err(|error| format!("读取 SQLite 告警收件人失败：{error}"))?
        .unwrap_or_default();
    let started_at = parse_timestamp(failed_at)?;
    let deadline = retry_deadline(started_at, settings.as_ref().map_or(0, |(_, hours)| *hours));
    for (index, recipient) in recipients.iter().enumerate() {
        transaction
            .execute(
                "INSERT OR IGNORE INTO email_deliveries (
                    delivery_id, alert_id, recipient_snapshot, state, next_attempt_at,
                    retry_window_started_at, retry_deadline_at
                 ) VALUES (?1, ?2, ?3, 'PENDING', ?4, ?4, ?5)",
                params![
                    format!("{alert_id}-{index}"),
                    alert_id,
                    recipient,
                    started_at.to_rfc3339(),
                    deadline.to_rfc3339(),
                ],
            )
            .map_err(|error| format!("创建 SQLite 告警投递失败：{error}"))?;
    }
    Ok(())
}

fn is_alertable(history: &RunHistory) -> bool {
    history.outcome.as_deref() == Some("FAILED")
        && history.unknown_reason.as_deref() != Some("STOPPED_BY_USER")
        && history.failure_kind.as_deref() != Some("SKIPPED")
}

fn safe_explanation(kind: Option<&str>) -> &'static str {
    match kind {
        Some("CONFIG") => "运行配置未通过检查，请在系统中查看运行详情。",
        Some("ORCHESTRATOR") => "运行未能正常启动，请在系统中查看运行详情。",
        Some("SOURCE_CONNECT") => "源端数据库连接失败，请在系统中查看运行详情。",
        Some("SOURCE_DBLINK") => "源端数据库链路不可用，请在系统中查看运行详情。",
        Some("SOURCE_QUERY") => "源端查询执行失败，请在系统中查看运行详情。",
        Some("SOURCE_VALUE") => "源端数据值无法转换，请在系统中查看运行详情。",
        Some("MAPPING_PRECHECK") => "字段映射检查未通过，请在系统中查看运行详情。",
        Some("NETWORK") => "运行期间网络通信失败，请在系统中查看运行详情。",
        Some("SINK_WRITE") => "目标端写入失败，请在系统中查看运行详情。",
        Some("DATA_REJECTED") => "目标端拒绝了部分数据，请在系统中查看运行详情。",
        Some("SINK_ENVIRONMENT") => "目标端环境不满足运行要求，请在系统中查看运行详情。",
        Some("TARGET_BUSY") => "目标端当前忙碌，请在系统中查看运行详情。",
        Some("VERIFY_FAILED") => "写入后的校验未通过，请在系统中查看运行详情。",
        Some("DEFECT") => "运行遇到内部一致性错误，请在系统中查看运行详情。",
        Some("UNKNOWN") => "运行结局无法确认，请在系统中查看运行详情。",
        _ => "运行失败，请在系统中查看运行详情。",
    }
}

struct PendingDelivery {
    delivery_id: String,
    recipient: String,
    alert_id: String,
    run_record_id: String,
    run_id: Option<String>,
    task_id: String,
    task_name: String,
    trigger: String,
    failed_at: String,
    failure_category: String,
    safe_explanation: String,
}

fn render_message(
    delivery: &PendingDelivery,
    instance_name: &str,
    base_url: Option<&str>,
) -> (String, String, String) {
    let subject = format!("[db-qbs][{instance_name}][告警] {}", delivery.task_name);
    let run_id = delivery.run_id.as_deref().unwrap_or("未分配");
    let link = base_url.map(|base| format!("{base}/#runs/{}", delivery.run_record_id));
    let link_plain = link
        .as_deref()
        .map(|value| format!("\n运行详情：{value}"))
        .unwrap_or_default();
    let link_html = link
        .as_deref()
        .map(|value| format!("<p><a href=\"{}\">打开运行详情</a></p>", html_escape(value)))
        .unwrap_or_default();
    let plain = format!(
        "db-qbs 运行失败告警\n告警 ID：{}\n任务：{}（{}）\n运行记录 ID：{}\n目标端运行 ID：{}\n触发方式：{}\n失败时间：{}\n失败分类：{}\n说明：{}{}",
        delivery.alert_id, delivery.task_name, delivery.task_id, delivery.run_record_id,
        run_id, delivery.trigger, delivery.failed_at, delivery.failure_category,
        delivery.safe_explanation, link_plain
    );
    let html = format!(
        "<!doctype html><html><body><h1>db-qbs 运行失败告警</h1><dl><dt>告警 ID</dt><dd>{}</dd><dt>任务</dt><dd>{}（{}）</dd><dt>运行记录 ID</dt><dd>{}</dd><dt>目标端运行 ID</dt><dd>{}</dd><dt>触发方式</dt><dd>{}</dd><dt>失败时间</dt><dd>{}</dd><dt>失败分类</dt><dd>{}</dd><dt>说明</dt><dd>{}</dd></dl>{}</body></html>",
        html_escape(&delivery.alert_id), html_escape(&delivery.task_name), html_escape(&delivery.task_id),
        html_escape(&delivery.run_record_id), html_escape(run_id), html_escape(&delivery.trigger),
        html_escape(&delivery.failed_at), html_escape(&delivery.failure_category),
        html_escape(&delivery.safe_explanation), link_html
    );
    (subject, plain, html)
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn spawn_outbox_worker(
    data_dir: PathBuf,
    transport: Arc<dyn MailTransport>,
    clock: Arc<dyn Clock>,
    terminated: Arc<std::sync::atomic::AtomicBool>,
) {
    std::thread::spawn(move || {
        let Ok(outbox) = AlertOutboxStore::open(&data_dir) else {
            return;
        };
        let Ok(settings) = EmailAlertStore::open(&data_dir) else {
            return;
        };
        while !terminated.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = outbox.run_due_attempts(&settings, transport.as_ref(), clock.as_ref());
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
}
