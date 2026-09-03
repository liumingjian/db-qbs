use std::path::PathBuf;

use serde::Serialize;

mod message;
mod persistence;
mod worker;

pub(crate) use persistence::{initialize_alert_tables, insert_alert_in_transaction};
pub use worker::spawn_outbox_worker;

const DATABASE_FILE: &str = "db-qbs.sqlite3";
const BUSY_SKIP_SUPPRESSION_HOURS: i64 = 1;
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
    email_logs: crate::EmailLogStore,
}

impl AlertOutboxStore {
    pub fn email_logs(&self) -> &crate::EmailLogStore {
        &self.email_logs
    }
}
