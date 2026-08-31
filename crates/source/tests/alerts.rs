use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use base64::Engine;
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use db_qbs_source::{
    AlertDeliveryState, AlertOutboxStore, Clock, EmailAlertSettingsInput, EmailAlertStore,
    EmailProviderPreset, HistoryStore, MailTransport, MailTransportError, OutgoingMail, RunHistory,
    RunTrigger, SmtpSecurity, UnknownReason,
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct FixedClock(DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

#[derive(Default)]
struct RecordingTransport {
    sent: Mutex<Vec<OutgoingMail>>,
    failure: Mutex<Option<MailTransportError>>,
}

impl MailTransport for RecordingTransport {
    fn send(
        &self,
        _settings: &db_qbs_source::EmailDeliverySettings,
        mail: &OutgoingMail,
    ) -> Result<(), MailTransportError> {
        self.sent.lock().unwrap().push(mail.clone());
        match *self.failure.lock().unwrap() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

fn temp_directory() -> PathBuf {
    let suffix = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("db-qbs-alert-test-{}-{suffix}", std::process::id()));
    std::fs::create_dir(&path).unwrap();
    path
}

fn settings(recipients: Vec<&str>) -> EmailAlertSettingsInput {
    EmailAlertSettingsInput {
        enabled: true,
        provider_preset: EmailProviderPreset::Generic,
        smtp_host: "mail.example.com".to_owned(),
        smtp_port: 587,
        smtp_security: SmtpSecurity::Starttls,
        smtp_username: "mailer-user".to_owned(),
        smtp_secret: "SMTP-secret-marker".to_owned(),
        sender_address: "alerts@example.com".to_owned(),
        sender_name: "db-qbs alerts".to_owned(),
        recipients: recipients.into_iter().map(str::to_owned).collect(),
        max_retry_hours: 24,
        instance_name: "华东生产".to_owned(),
        external_base_url: Some("https://qbs.example.com".to_owned()),
    }
}

fn failure(id: &str, kind: &str, trigger: RunTrigger, now: DateTime<Utc>) -> RunHistory {
    let mut history = RunHistory::accepted(
        id,
        "task-sensitive",
        "SELECT SQL-secret-marker FROM payroll",
        now,
    );
    history.task_name = "薪资同步 <紧急>".to_owned();
    history.trigger = trigger.as_str().to_owned();
    history.run_id = Some(format!("run-{id}"));
    history.outcome = Some("FAILED".to_owned());
    history.finished_at = Some(now.to_rfc3339());
    history.failure_kind = Some(kind.to_owned());
    history.message = Some("raw-failure-marker credential-marker".to_owned());
    history.column = Some("salary".to_owned());
    history.value = Some("sample-value-marker".to_owned());
    history
}

#[test]
fn failed_accepted_run_is_atomic_idempotent_snapshotted_and_sent_as_safe_multipart() {
    let directory = temp_directory();
    let now = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 0).unwrap();
    let email = EmailAlertStore::open(&directory).unwrap();
    email
        .update(settings(vec!["first@example.com", "second@example.com"]))
        .unwrap();
    let history_store = HistoryStore::open(&directory).unwrap();
    let outbox = AlertOutboxStore::open(&directory).unwrap();
    let failed = failure("record-1", "SINK_WRITE", RunTrigger::Manual, now);

    history_store.finalize(&failed, now, 90).unwrap();
    history_store.finalize(&failed, now, 90).unwrap();
    let pending = outbox.summary_for_run("record-1").unwrap().unwrap();
    assert_eq!(pending.alert_id, "alert-record-1");
    assert_eq!(pending.delivery_state, AlertDeliveryState::Pending);

    // Recipient membership is historical. Connection, authentication, and sender settings are not.
    email.update(settings(vec!["later@example.com"])).unwrap();
    let transport = RecordingTransport::default();
    assert_eq!(
        outbox
            .run_first_attempts(&email, &transport, &FixedClock(now))
            .unwrap(),
        2
    );
    assert_eq!(
        outbox
            .run_first_attempts(&email, &transport, &FixedClock(now))
            .unwrap(),
        0,
        "a replay must not resend a successful first attempt"
    );
    assert_eq!(
        outbox
            .summary_for_run("record-1")
            .unwrap()
            .unwrap()
            .delivery_state,
        AlertDeliveryState::Sent
    );

    let sent = transport.sent.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].envelope_to, "first@example.com");
    assert_eq!(sent[1].envelope_to, "second@example.com");
    for mail in sent.iter() {
        let raw = String::from_utf8_lossy(&mail.message);
        assert!(raw.contains("multipart/alternative"));
        let decoded = decoded_mime_bodies(&raw);
        assert_eq!(
            decoded_subject(&raw),
            "[db-qbs][华东生产][告警] 薪资同步 <紧急>"
        );
        assert!(decoded.contains("alert-record-1"));
        assert!(decoded.contains("https://qbs.example.com/#runs/record-1"));
        for forbidden in [
            "SQL-secret-marker",
            "SMTP-secret-marker",
            "raw-failure-marker",
            "credential-marker",
            "sample-value-marker",
        ] {
            assert!(!decoded.contains(forbidden), "message leaked {forbidden}");
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

fn decoded_mime_bodies(raw: &str) -> String {
    let mut decoded = String::new();
    let mut lines = raw.lines().peekable();
    while let Some(line) = lines.next() {
        if line != "Content-Transfer-Encoding: base64" {
            continue;
        }
        if lines.peek() == Some(&"") {
            lines.next();
        }
        let encoded = lines
            .by_ref()
            .take_while(|line| !line.starts_with("--"))
            .collect::<String>();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        decoded.push_str(&String::from_utf8(bytes).unwrap());
    }
    decoded
}

fn decoded_subject(raw: &str) -> String {
    let subject_lines = raw
        .lines()
        .skip_while(|line| !line.starts_with("Subject: "))
        .take_while(|line| line.starts_with("Subject: ") || line.starts_with(' '))
        .collect::<Vec<_>>()
        .join(" ");
    subject_lines
        .split_whitespace()
        .filter_map(|part| {
            part.strip_prefix("=?utf-8?b?")
                .and_then(|part| part.strip_suffix("?="))
        })
        .map(|encoded| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap();
            String::from_utf8(bytes).unwrap()
        })
        .collect()
}

#[test]
fn every_actual_failure_category_alerts_for_manual_and_scheduled_runs() {
    let directory = temp_directory();
    let now = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 0).unwrap();
    let email = EmailAlertStore::open(&directory).unwrap();
    email.update(settings(vec!["ops@example.com"])).unwrap();
    let history_store = HistoryStore::open(&directory).unwrap();
    let outbox = AlertOutboxStore::open(&directory).unwrap();
    let kinds = [
        "CONFIG",
        "ORCHESTRATOR",
        "SOURCE_CONNECT",
        "SOURCE_DBLINK",
        "SOURCE_QUERY",
        "SOURCE_VALUE",
        "MAPPING_PRECHECK",
        "NETWORK",
        "SINK_WRITE",
        "DATA_REJECTED",
        "SINK_ENVIRONMENT",
        "TARGET_BUSY",
        "VERIFY_FAILED",
        "DEFECT",
        "UNKNOWN",
    ];
    for (index, kind) in kinds.iter().enumerate() {
        let id = format!("category-{index}");
        let trigger = if index % 2 == 0 {
            RunTrigger::Manual
        } else {
            RunTrigger::Scheduled
        };
        history_store
            .finalize(&failure(&id, kind, trigger, now), now, 90)
            .unwrap();
        assert!(outbox.summary_for_run(&id).unwrap().is_some(), "{kind}");
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_failed_first_attempt_stays_durable_without_reopening_or_resending_the_run() {
    let directory = temp_directory();
    let now = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 0).unwrap();
    let email = EmailAlertStore::open(&directory).unwrap();
    email.update(settings(vec!["ops@example.com"])).unwrap();
    let history_store = HistoryStore::open(&directory).unwrap();
    let outbox = AlertOutboxStore::open(&directory).unwrap();
    history_store
        .finalize(
            &failure("smtp-down", "NETWORK", RunTrigger::Scheduled, now),
            now,
            90,
        )
        .unwrap();
    let transport = RecordingTransport::default();
    *transport.failure.lock().unwrap() = Some(MailTransportError::Timeout);

    assert_eq!(
        outbox
            .run_first_attempts(&email, &transport, &FixedClock(now))
            .unwrap(),
        1
    );
    assert_eq!(
        history_store
            .get("smtp-down")
            .unwrap()
            .unwrap()
            .outcome
            .as_deref(),
        Some("FAILED")
    );
    assert_eq!(
        outbox
            .summary_for_run("smtp-down")
            .unwrap()
            .unwrap()
            .delivery_state,
        AlertDeliveryState::Pending
    );
    assert_eq!(
        outbox
            .run_first_attempts(&email, &transport, &FixedClock(now))
            .unwrap(),
        0,
        "#287 must schedule later retries; the first-attempt worker must not spin"
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn success_user_stop_and_scheduled_skip_do_not_create_an_alert() {
    let directory = temp_directory();
    let now = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 0).unwrap();
    let email = EmailAlertStore::open(&directory).unwrap();
    email.update(settings(vec!["ops@example.com"])).unwrap();
    let history_store = HistoryStore::open(&directory).unwrap();
    let outbox = AlertOutboxStore::open(&directory).unwrap();

    let mut success = RunHistory::accepted("success", "task", "SELECT 1", now);
    success.outcome = Some("SUCCEEDED".to_owned());
    success.finished_at = Some(now.to_rfc3339());
    history_store.finalize(&success, now, 90).unwrap();

    let mut stopped = RunHistory::accepted("stopped", "task", "SELECT 1", now);
    stopped.mark_unknown(UnknownReason::StoppedByUser, now);
    history_store.finalize(&stopped, now, 90).unwrap();

    let mut skipped = RunHistory::accepted("skipped", "task", "SELECT 1", now);
    skipped.mark_skipped("previous run active".to_owned(), now);
    history_store.finalize(&skipped, now, 90).unwrap();

    assert!(outbox.summary_for_run("success").unwrap().is_none());
    assert!(outbox.summary_for_run("stopped").unwrap().is_none());
    assert!(outbox.summary_for_run("skipped").unwrap().is_none());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn retention_removes_alerts_and_deliveries_only_with_expired_history() {
    let directory = temp_directory();
    let now = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 0).unwrap();
    let old = now - TimeDelta::days(91);
    let email = EmailAlertStore::open(&directory).unwrap();
    email.update(settings(vec!["ops@example.com"])).unwrap();
    let history_store = HistoryStore::open(&directory).unwrap();
    let outbox = AlertOutboxStore::open(&directory).unwrap();

    history_store
        .finalize(
            &failure("expired", "NETWORK", RunTrigger::Manual, old),
            old,
            90,
        )
        .unwrap();
    history_store
        .finalize(
            &failure("retained", "NETWORK", RunTrigger::Manual, now),
            now,
            90,
        )
        .unwrap();
    // Any normal history write runs the same retention transaction.
    let accepted = RunHistory::accepted("cleanup-trigger", "task", "SELECT 1", now);
    history_store.insert(&accepted, now, 90).unwrap();

    assert!(history_store.get("expired").unwrap().is_none());
    assert!(outbox.summary_for_run("expired").unwrap().is_none());
    assert!(history_store.get("retained").unwrap().is_some());
    assert!(outbox.summary_for_run("retained").unwrap().is_some());
    std::fs::remove_dir_all(directory).unwrap();
}
