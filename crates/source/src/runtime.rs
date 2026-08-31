use chrono::{DateTime, Utc};

/// Time used by resident source behavior whose outcome is persisted.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A fully rendered message handed to the configured mail adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingMail {
    pub envelope_from: String,
    pub envelope_to: String,
    pub message: Vec<u8>,
}

/// The only boundary at which source performs mail I/O.
pub trait MailTransport: Send + Sync {
    fn send(&self, mail: &OutgoingMail) -> Result<(), String>;
}

/// Runtime placeholder until email delivery is configured.
#[derive(Debug, Default)]
pub struct UnconfiguredMailTransport;

impl MailTransport for UnconfiguredMailTransport {
    fn send(&self, _mail: &OutgoingMail) -> Result<(), String> {
        Err("邮件传输尚未配置".to_owned())
    }
}
