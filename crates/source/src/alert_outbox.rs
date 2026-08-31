use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::{multipart_mail, Clock, EmailAlertStore, MailTransport, RunHistory};

const DATABASE_FILE: &str = "db-qbs.sqlite3";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AlertDeliveryState {
    Pending,
    Sent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunAlertSummary {
    pub alert_id: String,
    pub delivery_state: AlertDeliveryState,
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
        self.connection()?
            .query_row(
                "SELECT a.alert_id,
                        CASE WHEN COUNT(d.delivery_id) > 0
                                   AND SUM(CASE WHEN d.state = 'SENT' THEN 1 ELSE 0 END)
                                       = COUNT(d.delivery_id)
                             THEN 'SENT' ELSE 'PENDING' END
                   FROM alerts a
                   LEFT JOIN email_deliveries d ON d.alert_id = a.alert_id
                  WHERE a.run_record_id = ?1
                  GROUP BY a.alert_id",
                [run_record_id],
                |row| {
                    let state: String = row.get(1)?;
                    Ok(RunAlertSummary {
                        alert_id: row.get(0)?,
                        delivery_state: if state == "SENT" {
                            AlertDeliveryState::Sent
                        } else {
                            AlertDeliveryState::Pending
                        },
                    })
                },
            )
            .optional()
            .map_err(|error| format!("查询运行告警状态失败：{error}"))
    }

    /// Attempts each newly-created delivery once. Failures remain durable and pending; #287 owns
    /// retry scheduling and terminal exhaustion policy.
    pub fn run_first_attempts(
        &self,
        settings_store: &EmailAlertStore,
        transport: &dyn MailTransport,
        clock: &dyn Clock,
    ) -> Result<usize, String> {
        let due = self.new_deliveries()?;
        let mut attempted = 0;
        for delivery in due {
            // Settings are deliberately not cached across recipients: every attempt uses the
            // connection, authentication, and sender configuration current at that instant.
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
            let result = transport.send(&delivery_settings, &mail);
            self.record_attempt(&delivery.delivery_id, clock.now(), result)?;
            attempted += 1;
        }
        Ok(attempted)
    }

    fn new_deliveries(&self) -> Result<Vec<PendingDelivery>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT d.delivery_id, d.recipient_snapshot, a.alert_id, a.run_record_id,
                        a.run_id, a.task_id, a.task_name, a.run_trigger, a.failed_at,
                        a.failure_category, a.safe_explanation
                   FROM email_deliveries d
                   JOIN alerts a ON a.alert_id = d.alert_id
                  WHERE d.state = 'PENDING' AND d.attempt_count = 0
                  ORDER BY d.delivery_id",
            )
            .map_err(|error| format!("查询待发送告警失败：{error}"))?;
        let rows = statement
            .query_map([], |row| {
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
        result: Result<(), crate::MailTransportError>,
    ) -> Result<(), String> {
        let (state, error) = match result {
            Ok(()) => ("SENT", None),
            Err(error) => ("PENDING", Some(error.sanitized_message())),
        };
        self.connection()?
            .execute(
                "UPDATE email_deliveries
                    SET state = ?1, attempt_count = attempt_count + 1,
                        first_attempt_at = COALESCE(first_attempt_at, ?2),
                        last_attempt_at = ?2, last_error = ?3
                  WHERE delivery_id = ?4 AND state = 'PENDING' AND attempt_count = 0",
                params![state, now.to_rfc3339(), error, delivery_id],
            )
            .map_err(|error| format!("记录告警发送结果失败：{error}"))?;
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
    let recipients: Option<String> = if has_settings_table {
        transaction
            .query_row(
                "SELECT recipients FROM email_alert_settings WHERE singleton_id = 1 AND enabled = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("读取 SQLite 告警收件人失败：{error}"))?
    } else {
        None
    };
    let recipients: Vec<String> = recipients
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|error| format!("读取 SQLite 告警收件人失败：{error}"))?
        .unwrap_or_default();
    for (index, recipient) in recipients.iter().enumerate() {
        transaction
            .execute(
                "INSERT OR IGNORE INTO email_deliveries (
                    delivery_id, alert_id, recipient_snapshot, state
                 ) VALUES (?1, ?2, ?3, 'PENDING')",
                params![format!("{alert_id}-{index}"), alert_id, recipient],
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
            let _ = outbox.run_first_attempts(&settings, transport.as_ref(), clock.as_ref());
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
}
