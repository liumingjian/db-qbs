use chrono::{DateTime, Utc};

use crate::EmailDeliverySettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailTransportError {
    Timeout,
    Tls,
    Transient,
    Permanent,
    Network,
}

impl MailTransportError {
    pub fn sanitized_message(self) -> &'static str {
        match self {
            Self::Timeout => "SMTP 连接或响应超时",
            Self::Tls => "SMTP TLS 握手或证书验证失败",
            Self::Transient => "SMTP 服务器暂时拒绝请求",
            Self::Permanent => "SMTP 服务器拒绝请求，请检查认证或邮件设置",
            Self::Network => "无法连接 SMTP 服务器或完成邮件传输",
        }
    }
}

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
    fn send(
        &self,
        settings: &EmailDeliverySettings,
        mail: &OutgoingMail,
    ) -> Result<(), MailTransportError>;
}

/// Runtime placeholder until email delivery is configured.
#[derive(Debug, Default)]
pub struct UnconfiguredMailTransport;

impl MailTransport for UnconfiguredMailTransport {
    fn send(
        &self,
        _settings: &EmailDeliverySettings,
        _mail: &OutgoingMail,
    ) -> Result<(), MailTransportError> {
        Err(MailTransportError::Network)
    }
}
