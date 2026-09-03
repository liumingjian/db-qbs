use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use db_qbs_shared::{LogEvent, LogLevel};
use serde_json::json;

use super::message::render_message;
use super::AlertOutboxStore;
use crate::{multipart_mail, Clock, EmailAlertStore, MailTransport};

impl AlertOutboxStore {
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
            let attempt = delivery.attempt_count + 1;
            let log_context = || {
                json!({
                    "alert_id": &delivery.alert_id,
                    "delivery_id": &delivery.delivery_id,
                    "run_record_id": &delivery.run_record_id,
                    "task_id": &delivery.task_id,
                    "recipient": &delivery.recipient,
                    "attempt": attempt,
                })
            };
            let mut attempt_fields = log_context();
            if let serde_json::Value::Object(fields) = &mut attempt_fields {
                fields.insert("smtp_host".to_owned(), json!(&delivery_settings.host));
                fields.insert("smtp_port".to_owned(), json!(delivery_settings.port));
                fields.insert(
                    "smtp_security".to_owned(),
                    json!(delivery_settings.security),
                );
                fields.insert(
                    "sender_address".to_owned(),
                    json!(&delivery_settings.sender_address),
                );
            }
            let _ = self.email_logs.append(
                LogLevel::Info,
                LogEvent::EmailDeliveryAttempted,
                delivery.run_id.as_deref(),
                Some(&delivery.task_name),
                attempt_fields,
            );
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
            let recorded = self.record_attempt(
                &delivery.delivery_id,
                attempt_time,
                settings.max_retry_hours,
                result,
            )?;
            let level = match recorded.state {
                "SENT" => LogLevel::Info,
                "PENDING" => LogLevel::Warn,
                _ => LogLevel::Error,
            };
            let _ = self.email_logs.append(
                level,
                LogEvent::EmailDeliveryCompleted,
                delivery.run_id.as_deref(),
                Some(&delivery.task_name),
                json!({
                    "alert_id": &delivery.alert_id,
                    "delivery_id": &delivery.delivery_id,
                    "run_record_id": &delivery.run_record_id,
                    "task_id": &delivery.task_id,
                    "recipient": &delivery.recipient,
                    "attempt": recorded.attempt_count,
                    "state": recorded.state,
                    "error_code": recorded.error_code,
                    "error": recorded.error,
                    "next_attempt_at": recorded.next_attempt_at,
                    "retry_deadline_at": recorded.retry_deadline_at,
                }),
            );
            attempted += 1;
        }
        Ok(attempted)
    }
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
        let mut previous_error = None;
        while !terminated.load(Ordering::Relaxed) {
            match outbox.run_due_attempts(&settings, transport.as_ref(), clock.as_ref()) {
                Ok(_) => previous_error = None,
                Err(error) if previous_error.as_deref() != Some(error.as_str()) => {
                    let _ = outbox.email_logs.append(
                        LogLevel::Error,
                        LogEvent::EmailWorkerError,
                        None,
                        None,
                        json!({ "error": error }),
                    );
                    previous_error = Some(error);
                }
                Err(_) => {}
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
}
