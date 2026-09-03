---
status: proposed
---

# Deliver run alerts through a durable email outbox

A failed run must reach its terminal state independently of SMTP availability. When a run reaches
an alertable terminal outcome, the source service will create one Alert and persist its email
delivery in an outbox before attempting delivery. A background worker sends and retries pending
deliveries across service restarts; SMTP calls never keep a run in flight or change its outcome.

An Alert is an operational notice, not an email. Actual execution failures, scheduled occurrences
that could not start, and outcomes made unknown by a service interruption create Alerts. A run
deliberately stopped by a user does not. Each actual failed run is delivered separately. Repeated
scheduled occurrences skipped because the previous run is still active create records, but only the
first email in a one-hour window is delivered for the same task and reason.

Only submissions accepted by the resident source service and represented by a `run_record_id`
create Alerts. A standalone CLI invocation has no Run History record and continues to report failure
through its exit status and logs only.

Email is the first and only delivery channel in scope. Its SMTP connection, sender, global recipient
list, enable switch, and maximum retry window are system-wide settings editable only by the
Administrator. The retry window is the only retry parameter exposed: it is an integer number of
hours from 0 through 168, defaults to 24, and 0 means that only the immediate attempt is made. The
retry schedule is exponential and owned by the system rather than exposed as a collection of tuning
parameters. After the retry window expires, the delivery is failed until an Administrator retries it
manually. The one-hour suppression window for repeated scheduled skips is fixed rather than another
system setting.

SMTP supports implicit SSL/TLS and STARTTLS but never plaintext delivery. A Tencent Exmail preset
fills `smtp.exmail.qq.com`, port 465, and implicit SSL/TLS while leaving the fields editable. The
system also stores an instance name, defaulting to `db-qbs`, and an optional externally reachable
base URL so recipients can distinguish deployments and follow a run-detail link. Saving settings
validates their shape but does not require a live SMTP connection; sending a test email is a separate
Administrator action whose latest result is shown in the web interface.

Alert content is a snapshot of the failure. It includes task and run identifiers, trigger, failure
time and category, and a sanitised error explanation; it excludes SQL, credentials, and sampled
business values. Messages are Chinese multipart email with plain-text and HTML alternatives. Their
subject is `[db-qbs][<instance name>][告警] <task name>`, and their body carries the stable Alert ID
and optional run-detail link as well as the failure snapshot.

Every attempt uses the current SMTP connection, authentication, and sender settings, while the
recipient set is snapshotted when the Alert is created. Changing recipients affects only later
Alerts and cannot rewrite historical delivery meaning or send an old Alert to a newly added address.
The global list accepts any valid email domain, removes duplicates, is limited to 50 addresses, and
must contain at least one address before email delivery can be enabled.

Alerts created while email delivery is disabled or incomplete are recorded as not sent and are
never backfilled automatically. Disabling delivery also terminates pending attempts as not sent;
re-enabling does not revive them, and the web interface confirms this consequence before applying
the change. Not-sent and suppressed deliveries are final. Only deliveries that exhausted real SMTP
attempts in the failed state may be retried manually.

Operators may see delivery state on a run but cannot see recipients, SMTP errors, retry counts, or
retry controls. Administrators may inspect those details and manually retry a failed delivery.
Administrators manage these controls on a top-level System Settings screen with separate Email Alert
and Operator Account views; Operators cannot navigate to or call its configuration APIs.

The source persists the email lifecycle as structured JSON Lines in its local SQLite file. The log
uses the same `ts`, `level`, `event`, optional `run_id`/`task`, and event-field convention as source
runtime logs, and is read by administrators through a cursor-based `/api/email-logs` endpoint. It
covers settings changes, test-email results, queueing, attempts, manual retries, retry-window expiry,
suppressed or unsent deliveries, and worker errors. Transport failures include a stable safe error
code alongside the sanitized diagnostic. It retains these diagnostics for 30 days and never stores
the SMTP secret or a raw SMTP response.

Each recipient has an independent delivery record and retry lifecycle. A rejected recipient neither
blocks nor hides successful deliveries to the others, and the run-level status can therefore be
sent, partially failed, or failed. Alert and delivery records have no independent retention setting:
they are removed with their associated Run History record, whose retention defaults to 90 days.

## Considered Options

Sending synchronously from the run-finalisation path was rejected because a slow or unavailable mail
server would couple data-transfer completion to notification infrastructure. Logging one failed send
without persistence was rejected because temporary SMTP failures would silently lose the alert.

## Consequences

The outbox requires its own lifecycle and idempotency rule: one email delivery per Alert and
recipient. Delivery is at least once, so a crash after the SMTP server accepts a message but before
the acknowledgement is persisted may produce a duplicate email. Every message carries a stable
Alert identifier so recipients can recognise that case; avoiding it entirely would require
cooperation the SMTP protocol does not provide.
