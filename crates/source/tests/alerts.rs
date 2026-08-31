use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use base64::Engine;
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use db_qbs_source::{
    AlertDeliveryState, AlertOutboxStore, Clock, EmailAlertSettingsInput, EmailAlertStore,
    EmailProviderPreset, HistoryStore, MailTransport, MailTransportError, OutgoingMail, RunHistory,
    RunTrigger, ScheduledRefusalReason, SmtpSecurity, UnknownReason,
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct FixedClock(DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

struct MutableClock(Mutex<DateTime<Utc>>);

impl MutableClock {
    fn new(now: DateTime<Utc>) -> Self {
        Self(Mutex::new(now))
    }

    fn set(&self, now: DateTime<Utc>) {
        *self.0.lock().unwrap() = now;
    }
}

impl Clock for MutableClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CapturedAttempt {
    host: String,
    sender_address: String,
    sender_name: String,
    recipient: String,
    message: Vec<u8>,
}

#[derive(Default)]
struct SelectiveTransport {
    failing: Mutex<HashSet<String>>,
    attempts: Mutex<Vec<CapturedAttempt>>,
}

impl MailTransport for SelectiveTransport {
    fn send(
        &self,
        settings: &db_qbs_source::EmailDeliverySettings,
        mail: &OutgoingMail,
    ) -> Result<(), MailTransportError> {
        self.attempts.lock().unwrap().push(CapturedAttempt {
            host: settings.host.clone(),
            sender_address: settings.sender_address.clone(),
            sender_name: settings.sender_name.clone(),
            recipient: mail.envelope_to.clone(),
            message: mail.message.clone(),
        });
        if self.failing.lock().unwrap().contains(&mail.envelope_to) {
            Err(MailTransportError::Permanent)
        } else {
            Ok(())
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
    let database = std::fs::read(directory.join("db-qbs.sqlite3")).unwrap();
    assert!(
        !String::from_utf8_lossy(&database).contains("SMTP-secret-marker"),
        "the SMTP secret must remain encrypted after Alert creation"
    );
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
fn restart_unknown_alerts_explain_only_the_sanitized_operational_cause() {
    let directory = temp_directory();
    let now = Utc.with_ymd_and_hms(2026, 9, 1, 10, 0, 0).unwrap();
    let email = EmailAlertStore::open(&directory).unwrap();
    email.update(settings(vec!["ops@example.com"])).unwrap();
    let histories = [
        (
            "service-restarted",
            UnknownReason::ServiceRestarted,
            "服务重启期间运行未留下终态",
        ),
        (
            "process-disappeared",
            UnknownReason::ProcessDisappeared,
            "运行进程消失且未留下终态",
        ),
    ];
    let history_store = HistoryStore::open(&directory).unwrap();
    for (id, reason, _) in histories {
        let mut history = RunHistory::accepted(
            id,
            "restart-task",
            "SELECT SQL-secret-marker FROM payroll",
            now,
        );
        history.task_name = "Restart recovery".to_owned();
        history.value = Some("sample-value-marker".to_owned());
        history.mark_unknown(reason, now);
        history_store.finalize(&history, now, 90).unwrap();
    }

    let transport = RecordingTransport::default();
    assert_eq!(
        AlertOutboxStore::open(&directory)
            .unwrap()
            .run_due_attempts(&email, &transport, &FixedClock(now))
            .unwrap(),
        2
    );
    let decoded = transport
        .sent
        .lock()
        .unwrap()
        .iter()
        .map(|mail| decoded_mime_bodies(&String::from_utf8_lossy(&mail.message)))
        .collect::<Vec<_>>()
        .join("\n");
    for (_, _, explanation) in histories {
        assert!(decoded.contains(explanation), "{decoded}");
    }
    for forbidden in ["SQL-secret-marker", "sample-value-marker"] {
        assert!(!decoded.contains(forbidden), "message leaked {forbidden}");
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
fn retries_use_persisted_exponential_due_times_and_survive_reopen() {
    let directory = temp_directory();
    let started = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 0).unwrap();
    let clock = MutableClock::new(started);
    let email = EmailAlertStore::open(&directory).unwrap();
    email.update(settings(vec!["ops@example.com"])).unwrap();
    HistoryStore::open(&directory)
        .unwrap()
        .finalize(
            &failure("retry-schedule", "NETWORK", RunTrigger::Manual, started),
            started,
            90,
        )
        .unwrap();
    let transport = RecordingTransport::default();
    *transport.failure.lock().unwrap() = Some(MailTransportError::Timeout);
    let outbox = AlertOutboxStore::open(&directory).unwrap();

    assert_eq!(
        outbox.run_due_attempts(&email, &transport, &clock).unwrap(),
        1
    );
    let first = outbox.delivery_history(None).unwrap().remove(0);
    assert_eq!(first.attempt_count, 1);
    assert_eq!(
        parse(&first.next_attempt_at.unwrap()),
        started + TimeDelta::minutes(1)
    );
    assert_eq!(parse(&first.retry_window_started_at), started);
    assert_eq!(
        parse(&first.retry_deadline_at),
        started + TimeDelta::hours(24)
    );
    assert_eq!(first.last_error.as_deref(), Some("SMTP 连接或响应超时"));
    assert_eq!(
        outbox.run_due_attempts(&email, &transport, &clock).unwrap(),
        0
    );

    let reopened = AlertOutboxStore::open(&directory).unwrap();
    clock.set(started + TimeDelta::minutes(1));
    assert_eq!(
        reopened
            .run_due_attempts(&email, &transport, &clock)
            .unwrap(),
        1
    );
    let second = reopened.delivery_history(None).unwrap().remove(0);
    assert_eq!(second.attempt_count, 2);
    assert_eq!(
        parse(&second.next_attempt_at.unwrap()),
        started + TimeDelta::minutes(3)
    );

    *transport.failure.lock().unwrap() = None;
    clock.set(started + TimeDelta::minutes(3));
    assert_eq!(
        reopened
            .run_due_attempts(&email, &transport, &clock)
            .unwrap(),
        1
    );
    let sent = reopened.delivery_history(None).unwrap().remove(0);
    assert_eq!(sent.state, db_qbs_source::EmailDeliveryState::Sent);
    assert_eq!(sent.attempt_count, 3);
    assert!(sent.next_attempt_at.is_none());
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn crash_after_transport_acceptance_retries_the_same_alert_id() {
    #[derive(Default)]
    struct CrashAfterAcceptance {
        accepted: Mutex<Vec<OutgoingMail>>,
    }

    impl MailTransport for CrashAfterAcceptance {
        fn send(
            &self,
            _settings: &db_qbs_source::EmailDeliverySettings,
            mail: &OutgoingMail,
        ) -> Result<(), MailTransportError> {
            self.accepted.lock().unwrap().push(mail.clone());
            panic!("simulated process crash after SMTP acceptance");
        }
    }

    let directory = temp_directory();
    let now = Utc.with_ymd_and_hms(2026, 9, 1, 10, 0, 0).unwrap();
    let clock = FixedClock(now);
    let email = EmailAlertStore::open(&directory).unwrap();
    email.update(settings(vec!["ops@example.com"])).unwrap();
    HistoryStore::open(&directory)
        .unwrap()
        .finalize(
            &failure("crash-window", "NETWORK", RunTrigger::Manual, now),
            now,
            90,
        )
        .unwrap();
    let outbox = AlertOutboxStore::open(&directory).unwrap();
    let crashing = CrashAfterAcceptance::default();

    assert!(catch_unwind(AssertUnwindSafe(|| {
        outbox.run_due_attempts(&email, &crashing, &clock)
    }))
    .is_err());
    let pending = outbox.delivery_history(None).unwrap().remove(0);
    assert_eq!(pending.alert_id, "alert-crash-window");
    assert_eq!(pending.recipient, "ops@example.com");
    assert_eq!(pending.attempt_count, 0);
    assert_eq!(pending.state, db_qbs_source::EmailDeliveryState::Pending);

    let retry = RecordingTransport::default();
    assert_eq!(outbox.run_due_attempts(&email, &retry, &clock).unwrap(), 1);
    let first = crashing.accepted.lock().unwrap()[0].clone();
    let second = retry.sent.lock().unwrap()[0].clone();
    assert_eq!(first.envelope_to, second.envelope_to);
    for message in [&first.message, &second.message] {
        let raw = String::from_utf8_lossy(message);
        let decoded = decoded_mime_bodies(&raw);
        assert!(decoded.contains("alert-crash-window"), "{decoded}");
    }
    let sent = outbox.delivery_history(None).unwrap().remove(0);
    assert_eq!(sent.alert_id, "alert-crash-window");
    assert_eq!(sent.attempt_count, 1);
    assert_eq!(sent.state, db_qbs_source::EmailDeliveryState::Sent);

    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn exponential_retry_intervals_cap_at_one_hour() {
    let directory = temp_directory();
    let started = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 0).unwrap();
    let clock = MutableClock::new(started);
    let email = EmailAlertStore::open(&directory).unwrap();
    email.update(settings(vec!["ops@example.com"])).unwrap();
    HistoryStore::open(&directory)
        .unwrap()
        .finalize(
            &failure("retry-cap", "NETWORK", RunTrigger::Manual, started),
            started,
            90,
        )
        .unwrap();
    let transport = RecordingTransport::default();
    *transport.failure.lock().unwrap() = Some(MailTransportError::Network);
    let outbox = AlertOutboxStore::open(&directory).unwrap();
    let due_minutes = [0, 1, 3, 7, 15, 31, 63, 123];
    let next_minutes = [1, 3, 7, 15, 31, 63, 123, 183];

    for (due, next) in due_minutes.into_iter().zip(next_minutes) {
        clock.set(started + TimeDelta::minutes(due));
        assert_eq!(
            outbox.run_due_attempts(&email, &transport, &clock).unwrap(),
            1
        );
        let delivery = outbox.delivery_history(None).unwrap().remove(0);
        assert_eq!(
            parse(&delivery.next_attempt_at.unwrap()),
            started + TimeDelta::minutes(next)
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn zero_default_and_maximum_retry_windows_have_explicit_deadlines() {
    for (suffix, hours) in [("zero", 0), ("default", 24), ("maximum", 168)] {
        let directory = temp_directory();
        let started = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 0).unwrap();
        let mut configured = settings(vec!["ops@example.com"]);
        configured.max_retry_hours = hours;
        let email = EmailAlertStore::open(&directory).unwrap();
        email.update(configured).unwrap();
        HistoryStore::open(&directory)
            .unwrap()
            .finalize(
                &failure(suffix, "NETWORK", RunTrigger::Manual, started),
                started,
                90,
            )
            .unwrap();
        let outbox = AlertOutboxStore::open(&directory).unwrap();
        let created = outbox.delivery_history(None).unwrap().remove(0);
        assert_eq!(
            parse(&created.retry_deadline_at),
            started + TimeDelta::hours(i64::from(hours))
        );

        if hours == 0 {
            let transport = RecordingTransport::default();
            *transport.failure.lock().unwrap() = Some(MailTransportError::Transient);
            assert_eq!(
                outbox
                    .run_due_attempts(&email, &transport, &FixedClock(started))
                    .unwrap(),
                1
            );
            let exhausted = outbox.delivery_history(None).unwrap().remove(0);
            assert_eq!(exhausted.state, db_qbs_source::EmailDeliveryState::Failed);
            assert_eq!(exhausted.attempt_count, 1);
            assert!(exhausted.next_attempt_at.is_none());
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn mixed_recipient_results_have_aggregate_states_without_coupling_attempts() {
    let directory = temp_directory();
    let now = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 0).unwrap();
    let mut configured = settings(vec!["sent@example.com", "failed@example.com"]);
    configured.max_retry_hours = 0;
    let email = EmailAlertStore::open(&directory).unwrap();
    email.update(configured).unwrap();
    HistoryStore::open(&directory)
        .unwrap()
        .finalize(
            &failure("mixed", "SINK_WRITE", RunTrigger::Manual, now),
            now,
            90,
        )
        .unwrap();
    let transport = SelectiveTransport::default();
    transport
        .failing
        .lock()
        .unwrap()
        .insert("failed@example.com".to_owned());
    let outbox = AlertOutboxStore::open(&directory).unwrap();

    assert_eq!(
        outbox
            .run_due_attempts(&email, &transport, &FixedClock(now))
            .unwrap(),
        2
    );
    assert_eq!(
        outbox
            .summary_for_run("mixed")
            .unwrap()
            .unwrap()
            .delivery_state,
        AlertDeliveryState::PartiallyFailed
    );
    let deliveries = outbox.delivery_history(Some("mixed")).unwrap();
    assert_eq!(
        deliveries
            .iter()
            .map(|row| row.attempt_count)
            .collect::<Vec<_>>(),
        vec![1, 1]
    );
    assert!(deliveries
        .iter()
        .any(|row| row.state == db_qbs_source::EmailDeliveryState::Sent));
    assert!(deliveries
        .iter()
        .any(|row| row.state == db_qbs_source::EmailDeliveryState::Failed));

    HistoryStore::open(&directory)
        .unwrap()
        .finalize(
            &failure("all-failed", "NETWORK", RunTrigger::Manual, now),
            now,
            90,
        )
        .unwrap();
    transport.failing.lock().unwrap().extend([
        "sent@example.com".to_owned(),
        "failed@example.com".to_owned(),
    ]);
    assert_eq!(
        outbox
            .run_due_attempts(&email, &transport, &FixedClock(now))
            .unwrap(),
        2
    );
    assert_eq!(
        outbox
            .summary_for_run("all-failed")
            .unwrap()
            .unwrap()
            .delivery_state,
        AlertDeliveryState::Failed
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn manual_retry_uses_a_new_window_current_settings_and_the_original_recipient() {
    let directory = temp_directory();
    let started = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 0).unwrap();
    let mut initial = settings(vec!["original@example.com"]);
    initial.max_retry_hours = 0;
    let email = EmailAlertStore::open(&directory).unwrap();
    email.update(initial).unwrap();
    HistoryStore::open(&directory)
        .unwrap()
        .finalize(
            &failure("manual-retry", "NETWORK", RunTrigger::Manual, started),
            started,
            90,
        )
        .unwrap();
    let outbox = AlertOutboxStore::open(&directory).unwrap();
    let transport = SelectiveTransport::default();
    transport
        .failing
        .lock()
        .unwrap()
        .insert("original@example.com".to_owned());
    outbox
        .run_due_attempts(&email, &transport, &FixedClock(started))
        .unwrap();
    let failed = outbox.delivery_history(None).unwrap().remove(0);

    let mut current = settings(vec!["new-list@example.com"]);
    current.smtp_host = "current.example.com".to_owned();
    current.sender_address = "current-sender@example.com".to_owned();
    current.sender_name = "Current Sender".to_owned();
    current.max_retry_hours = 2;
    email.update(current).unwrap();
    let retried_at = started + TimeDelta::hours(1);
    let result = outbox
        .manual_retry(&failed.delivery_id, retried_at, 2)
        .unwrap();
    let db_qbs_source::ManualRetryOutcome::Retried(retried) = result else {
        panic!("failed delivery was not retried");
    };
    assert_eq!(retried.attempt_count, 1, "attempt count is lifetime total");
    assert_eq!(parse(&retried.retry_window_started_at), retried_at);
    assert_eq!(
        parse(&retried.retry_deadline_at),
        retried_at + TimeDelta::hours(2)
    );

    transport.failing.lock().unwrap().clear();
    outbox
        .run_due_attempts(&email, &transport, &FixedClock(retried_at))
        .unwrap();
    let attempt = transport.attempts.lock().unwrap().last().unwrap().clone();
    assert_eq!(attempt.recipient, "original@example.com");
    assert_eq!(attempt.host, "current.example.com");
    assert_eq!(attempt.sender_address, "current-sender@example.com");
    assert_eq!(attempt.sender_name, "Current Sender");
    assert!(
        decoded_mime_bodies(&String::from_utf8_lossy(&attempt.message))
            .contains("alert-manual-retry")
    );
    assert_eq!(outbox.delivery_history(None).unwrap()[0].attempt_count, 2);
    assert!(matches!(
        outbox
            .manual_retry(&failed.delivery_id, retried_at, 2)
            .unwrap(),
        db_qbs_source::ManualRetryOutcome::Ineligible
    ));
    std::fs::remove_dir_all(directory).unwrap();
}

fn parse(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
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
fn busy_schedule_skips_keep_alert_evidence_but_suppress_delivery_for_one_hour() {
    let directory = temp_directory();
    let first_at = Utc.with_ymd_and_hms(2026, 8, 31, 10, 0, 0).unwrap();
    let email = EmailAlertStore::open(&directory).unwrap();
    email.update(settings(vec!["ops@example.com"])).unwrap();
    let history_store = HistoryStore::open(&directory).unwrap();
    let outbox = AlertOutboxStore::open(&directory).unwrap();

    let record = |id: &str, task_id: &str, reason: ScheduledRefusalReason, at| {
        let mut history = RunHistory::accepted(id, task_id, "SELECT 1", at);
        history.task_name = "hourly import".to_owned();
        history.trigger = RunTrigger::Scheduled.as_str().to_owned();
        history.mark_scheduled_refusal(reason, "display wording is not an identity".to_owned(), at);
        history_store.finalize(&history, at, 90).unwrap();
    };

    record(
        "busy-first",
        "task-a",
        ScheduledRefusalReason::PreviousRunActive,
        first_at,
    );
    record(
        "busy-inside",
        "task-a",
        ScheduledRefusalReason::PreviousRunActive,
        first_at + TimeDelta::minutes(30),
    );
    record(
        "busy-boundary",
        "task-a",
        ScheduledRefusalReason::PreviousRunActive,
        first_at + TimeDelta::hours(1),
    );
    record(
        "busy-outside",
        "task-a",
        ScheduledRefusalReason::PreviousRunActive,
        first_at + TimeDelta::hours(1) + TimeDelta::seconds(1),
    );

    for id in ["busy-inside", "busy-boundary"] {
        let summary = outbox.summary_for_run(id).unwrap().unwrap();
        assert_eq!(summary.delivery_state, AlertDeliveryState::Suppressed);
        let delivery = outbox.delivery_history(Some(id)).unwrap().remove(0);
        assert_eq!(
            delivery.state,
            db_qbs_source::EmailDeliveryState::Suppressed
        );
        assert!(matches!(
            outbox
                .manual_retry(&delivery.delivery_id, first_at, 24)
                .unwrap(),
            db_qbs_source::ManualRetryOutcome::Ineligible
        ));
    }
    assert_eq!(
        outbox
            .summary_for_run("busy-first")
            .unwrap()
            .unwrap()
            .delivery_state,
        AlertDeliveryState::Pending
    );
    assert_eq!(
        outbox
            .summary_for_run("busy-outside")
            .unwrap()
            .unwrap()
            .delivery_state,
        AlertDeliveryState::Pending
    );
    let alert_ids = ["busy-first", "busy-inside", "busy-boundary", "busy-outside"]
        .map(|id| outbox.summary_for_run(id).unwrap().unwrap().alert_id);
    assert_eq!(
        alert_ids
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        4
    );

    // Task and stable reason are both part of the key; display wording is not.
    record(
        "other-task",
        "task-b",
        ScheduledRefusalReason::PreviousRunActive,
        first_at + TimeDelta::minutes(30),
    );
    record(
        "other-reason",
        "task-a",
        ScheduledRefusalReason::TargetAgentUnavailable,
        first_at + TimeDelta::minutes(30),
    );
    for id in ["other-task", "other-reason"] {
        assert_eq!(
            outbox.summary_for_run(id).unwrap().unwrap().delivery_state,
            AlertDeliveryState::Pending
        );
    }
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
    assert_eq!(outbox.delivery_history(Some("expired")).unwrap().len(), 1);
    history_store
        .finalize(
            &failure("retained", "NETWORK", RunTrigger::Manual, now),
            old,
            90,
        )
        .unwrap();
    assert_eq!(outbox.delivery_history(Some("retained")).unwrap().len(), 1);
    // Any normal history write runs the same retention transaction.
    let accepted = RunHistory::accepted("cleanup-trigger", "task", "SELECT 1", now);
    history_store.insert(&accepted, now, 90).unwrap();

    assert!(history_store.get("expired").unwrap().is_none());
    assert!(outbox.summary_for_run("expired").unwrap().is_none());
    let database = rusqlite::Connection::open(directory.join("db-qbs.sqlite3")).unwrap();
    let expired_deliveries: u64 = database
        .query_row(
            "SELECT COUNT(*) FROM email_deliveries WHERE alert_id = 'alert-expired'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(expired_deliveries, 0, "recipient rows must not be orphaned");
    assert!(history_store.get("retained").unwrap().is_some());
    assert!(outbox.summary_for_run("retained").unwrap().is_some());
    assert_eq!(outbox.delivery_history(Some("retained")).unwrap().len(), 1);
    std::fs::remove_dir_all(directory).unwrap();
}
