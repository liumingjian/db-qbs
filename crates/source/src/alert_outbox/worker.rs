use std::path::PathBuf;
use std::sync::Arc;

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
