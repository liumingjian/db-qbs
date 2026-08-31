use std::fs;
use std::path::Path;

use chrono::{DateTime, TimeDelta, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use super::message::{is_alertable, safe_explanation, PendingDelivery};
use super::{
    AlertDeliveryState, AlertOutboxStore, EmailDeliveryHistory, EmailDeliveryState,
    ManualRetryOutcome, RunAlertSummary, BUSY_SKIP_SUPPRESSION_HOURS, DATABASE_FILE,
    RETRY_BASE_SECONDS, RETRY_CAP_SECONDS,
};
use crate::{EmailAlertStore, RunHistory};

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
        let alert: Option<(String, Option<String>)> = connection
            .query_row(
                "SELECT alert_id, delivery_state FROM alerts WHERE run_record_id = ?1",
                [run_record_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| format!("查询运行告警状态失败：{error}"))?;
        let Some((alert_id, alert_state)) = alert else {
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
            delivery_state: alert_state
                .as_deref()
                .map(parse_alert_state)
                .transpose()?
                .unwrap_or_else(|| aggregate_state(&states)),
        }))
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
        settings_store: &EmailAlertStore,
    ) -> Result<ManualRetryOutcome, String> {
        let Some(delivery) = self.delivery_by_id(delivery_id)? else {
            return Ok(ManualRetryOutcome::NotFound);
        };
        if delivery.state != EmailDeliveryState::Failed {
            return Ok(ManualRetryOutcome::Ineligible);
        }
        let Some(_) = settings_store.delivery_settings()? else {
            return Ok(ManualRetryOutcome::Ineligible);
        };
        let max_retry_hours = settings_store.get()?.max_retry_hours;
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
            return Ok(ManualRetryOutcome::Ineligible);
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

    pub(super) fn due_deliveries(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Vec<PendingDelivery>, String> {
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

    pub(super) fn record_attempt(
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

    pub(super) fn expire_overdue(&self, now: DateTime<Utc>) -> Result<(), String> {
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

pub(super) fn retry_deadline(started_at: DateTime<Utc>, max_retry_hours: u8) -> DateTime<Utc> {
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

fn parse_alert_state(value: &str) -> Result<AlertDeliveryState, String> {
    match value {
        "NOT_SENT" => Ok(AlertDeliveryState::NotSent),
        "SUPPRESSED" => Ok(AlertDeliveryState::Suppressed),
        _ => Err(format!("读取运行告警状态失败：未知状态 {value}")),
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
                safe_explanation     TEXT NOT NULL,
                delivery_state       TEXT CHECK (delivery_state IN ('NOT_SENT', 'SUPPRESSED')),
                delivery_candidate  INTEGER NOT NULL DEFAULT 0
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
        .map_err(|error| format!("初始化 SQLite 告警表失败：{error}"))?;
    let has_delivery_state: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('alerts') WHERE name = 'delivery_state')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("检查 SQLite 告警状态列失败：{error}"))?;
    if !has_delivery_state {
        connection
            .execute("ALTER TABLE alerts ADD COLUMN delivery_state TEXT", [])
            .map_err(|error| format!("迁移 SQLite 告警状态列失败：{error}"))?;
    }
    let has_delivery_candidate: bool = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM pragma_table_info('alerts') WHERE name = 'delivery_candidate'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("检查 SQLite 告警表结构失败：{error}"))?;
    if !has_delivery_candidate {
        connection
            .execute(
                "ALTER TABLE alerts ADD COLUMN delivery_candidate INTEGER NOT NULL DEFAULT 0",
                [],
            )
            .map_err(|error| format!("迁移 SQLite 告警表失败：{error}"))?;
    }
    Ok(())
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
    let failure_category = history
        .scheduled_refusal_reason
        .as_deref()
        .or(history.failure_kind.as_deref())
        .unwrap_or("UNKNOWN");
    let has_settings_table: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'email_alert_settings')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("检查 SQLite 告警设置失败：{error}"))?;
    let settings: Option<(bool, bool, String, u8)> = if has_settings_table {
        transaction
            .query_row(
                "SELECT enabled,
                        enabled = 1 AND smtp_host <> '' AND smtp_port > 0
                          AND smtp_username <> '' AND smtp_secret <> ''
                          AND sender_address <> '' AND sender_name <> '' AND recipients <> '[]',
                        recipients, max_retry_hours
                   FROM email_alert_settings WHERE singleton_id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|error| format!("读取 SQLite 告警收件人失败：{error}"))?
    } else {
        None
    };
    let recipients: Vec<String> = settings
        .as_ref()
        .map(|(_, _, recipients, _)| serde_json::from_str(recipients))
        .transpose()
        .map_err(|error| format!("读取 SQLite 告警收件人失败：{error}"))?
        .unwrap_or_default();
    let delivery_available = settings
        .as_ref()
        .is_some_and(|(_, complete, _, _)| *complete);
    let started_at = parse_timestamp(failed_at)?;
    let suppressed = failure_category == "PREVIOUS_RUN_ACTIVE"
        && delivery_available
        && has_busy_skip_candidate(transaction, &history.task_id, failure_category, started_at)?;
    let alert_state = if !delivery_available {
        Some("NOT_SENT")
    } else if suppressed {
        Some("SUPPRESSED")
    } else {
        None
    };
    transaction
        .execute(
            "INSERT OR IGNORE INTO alerts (
                alert_id, run_record_id, run_id, task_id, task_name, run_trigger,
                failed_at, failure_category, safe_explanation, delivery_state
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                alert_id,
                history.run_record_id,
                history.run_id,
                history.task_id,
                history.task_name,
                history.trigger,
                failed_at,
                failure_category,
                safe_explanation(Some(failure_category), history.unknown_reason.as_deref()),
                alert_state,
            ],
        )
        .map_err(|error| format!("创建 SQLite 运行告警失败：{error}"))?;

    let deadline = retry_deadline(
        started_at,
        settings.as_ref().map_or(0, |(_, _, _, hours)| *hours),
    );
    let delivery_state = if !delivery_available {
        "NOT_SENT"
    } else if suppressed {
        "SUPPRESSED"
    } else {
        "PENDING"
    };
    let unavailable_reason = settings.as_ref().map_or(
        "创建告警时邮件告警配置不完整",
        |(enabled, _, _, _)| {
            if *enabled {
                "创建告警时邮件告警配置不完整"
            } else {
                "创建告警时邮件告警未启用"
            }
        },
    );
    if delivery_available && !suppressed {
        transaction
            .execute(
                "UPDATE alerts SET delivery_candidate = 1 WHERE alert_id = ?1",
                [&alert_id],
            )
            .map_err(|error| format!("记录 SQLite 告警投递候选状态失败：{error}"))?;
    }
    for (index, recipient) in recipients.iter().enumerate() {
        transaction
            .execute(
                "INSERT OR IGNORE INTO email_deliveries (
                    delivery_id, alert_id, recipient_snapshot, state, next_attempt_at,
                    retry_window_started_at, retry_deadline_at, last_error
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    format!("{alert_id}-{index}"),
                    alert_id,
                    recipient,
                    delivery_state,
                    (delivery_state == "PENDING").then(|| started_at.to_rfc3339()),
                    started_at.to_rfc3339(),
                    deadline.to_rfc3339(),
                    (!delivery_available).then_some(unavailable_reason),
                ],
            )
            .map_err(|error| format!("创建 SQLite 告警投递失败：{error}"))?;
    }
    Ok(())
}

fn has_busy_skip_candidate(
    transaction: &rusqlite::Transaction<'_>,
    task_id: &str,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    let window_start = now - TimeDelta::hours(BUSY_SKIP_SUPPRESSION_HOURS);
    transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                  FROM alerts a
                 WHERE a.task_id = ?1 AND a.failure_category = ?2
                   AND julianday(a.failed_at) > julianday(?3)
                   AND julianday(a.failed_at) <= julianday(?4)
                   AND a.delivery_candidate = 1
             )",
            params![task_id, reason, window_start.to_rfc3339(), now.to_rfc3339()],
            |row| row.get(0),
        )
        .map_err(|error| format!("检查调度跳过告警抑制窗口失败：{error}"))
}
