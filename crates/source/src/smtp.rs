use std::error::Error as StdError;
use std::io::ErrorKind;
use std::time::Duration;

use lettre::address::Envelope;
use lettre::message::{header::ContentType, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
#[cfg(test)]
use lettre::transport::smtp::client::{Certificate, CertificateStore};
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{Address, Message, SmtpTransport, Transport};

use crate::{EmailDeliverySettings, MailTransport, MailTransportError, OutgoingMail, SmtpSecurity};

const SMTP_TIMEOUT: Duration = Duration::from_secs(10);

/// Production SMTP adapter. TLS certificate and hostname verification are always enabled.
#[derive(Debug)]
pub struct SmtpMailTransport {
    timeout: Duration,
    #[cfg(test)]
    test_root_certificate: Option<Vec<u8>>,
}

impl Default for SmtpMailTransport {
    fn default() -> Self {
        Self {
            timeout: SMTP_TIMEOUT,
            #[cfg(test)]
            test_root_certificate: None,
        }
    }
}

impl MailTransport for SmtpMailTransport {
    fn send(
        &self,
        settings: &EmailDeliverySettings,
        mail: &OutgoingMail,
    ) -> Result<(), MailTransportError> {
        let tls_parameters = self
            .tls_parameters(&settings.host)
            .map_err(|_| MailTransportError::Tls)?;
        let tls = match settings.security {
            SmtpSecurity::ImplicitTls => Tls::Wrapper(tls_parameters),
            SmtpSecurity::Starttls => Tls::Required(tls_parameters),
        };
        let transport = SmtpTransport::builder_dangerous(&settings.host)
            .port(settings.port)
            .tls(tls)
            .credentials(Credentials::new(
                settings.username.clone(),
                settings.secret.clone(),
            ))
            .timeout(Some(self.timeout))
            .build();
        let envelope = Envelope::new(
            Some(parse_address(&mail.envelope_from).map_err(|_| MailTransportError::Permanent)?),
            vec![parse_address(&mail.envelope_to).map_err(|_| MailTransportError::Permanent)?],
        )
        .map_err(|_| MailTransportError::Permanent)?;
        transport
            .send_raw(&envelope, &mail.message)
            .map(|_| ())
            .map_err(sanitize_smtp_error)
    }
}

impl SmtpMailTransport {
    fn tls_parameters(&self, host: &str) -> Result<TlsParameters, lettre::transport::smtp::Error> {
        let builder = TlsParameters::builder(host.to_owned());
        #[cfg(test)]
        let builder = if let Some(certificate) = &self.test_root_certificate {
            builder
                .certificate_store(CertificateStore::None)
                .add_root_certificate(Certificate::from_der(certificate.clone())?)
        } else {
            builder
        };
        builder.build()
    }

    #[cfg(test)]
    fn with_test_root(certificate: Vec<u8>, timeout: Duration) -> Self {
        Self {
            timeout,
            test_root_certificate: Some(certificate),
        }
    }
}

/// Builds a MIME multipart/alternative message and an SMTP envelope for one recipient.
pub fn multipart_mail(
    sender_address: &str,
    sender_name: &str,
    recipient: &str,
    subject: &str,
    plain: String,
    html: String,
) -> Result<OutgoingMail, String> {
    let sender = mailbox(sender_name, sender_address)?;
    let recipient_mailbox = mailbox("", recipient)?;
    let message = Message::builder()
        .from(sender)
        .to(recipient_mailbox)
        .subject(subject)
        .multipart(
            MultiPart::alternative()
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_PLAIN)
                        .body(plain),
                )
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_HTML)
                        .body(html),
                ),
        )
        .map_err(|_| "生成邮件内容失败".to_owned())?;
    Ok(OutgoingMail {
        envelope_from: sender_address.to_owned(),
        envelope_to: recipient.to_owned(),
        message: message.formatted(),
    })
}

fn mailbox(name: &str, address: &str) -> Result<Mailbox, String> {
    let address = parse_address(address)?;
    Ok(Mailbox::new(
        (!name.is_empty()).then(|| name.to_owned()),
        address,
    ))
}

fn parse_address(address: &str) -> Result<Address, String> {
    address.parse().map_err(|_| "邮件信封地址无效".to_owned())
}

fn sanitize_smtp_error(error: lettre::transport::smtp::Error) -> MailTransportError {
    if error.is_timeout() || has_timeout_source(&error) {
        MailTransportError::Timeout
    } else if error.is_tls() {
        MailTransportError::Tls
    } else if error.is_transient() {
        MailTransportError::Transient
    } else if error.is_permanent() {
        MailTransportError::Permanent
    } else {
        MailTransportError::Network
    }
}

fn has_timeout_source(error: &lettre::transport::smtp::Error) -> bool {
    let mut source = error.source();
    while let Some(cause) = source {
        if let Some(io_error) = cause.downcast_ref::<std::io::Error>() {
            if matches!(io_error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) {
                return true;
            }
        }
        source = cause.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex, Once};
    use std::thread;

    use base64::Engine;
    use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
    use rustls::{ServerConfig, ServerConnection, StreamOwned};

    use super::*;

    static INSTALL_PROVIDER: Once = Once::new();
    const TEST_CERTIFICATE: &str = "MIICwzCCAaugAwIBAgIJALCDkqdiP/sUMA0GCSqGSIb3DQEBCwUAMBQxEjAQBgNVBAMMCWxvY2FsaG9zdDAeFw0yNjA4MzExMzA3MThaFw0zNjA4MjgxMzA3MThaMBQxEjAQBgNVBAMMCWxvY2FsaG9zdDCCASIwDQYJKoZIhvcNAQEBBQADggEPADCCAQoCggEBALJVC/Ey1dY8fQtVwBPzwVKYXQmOviR5mszxxnYjwtKOxGfAYPHqYe7ofIdAs74+OPGdu6z8RbjMPN5ynf7UMjqwi2KDd6c1+Iw/dzzrlV93oL2CKrrt33aWuDeA5RLUTOX+pOJuVk1eRClVA3OdUOLTI5Jf8FzXDItSIHgZB5DBoEz/3kbGFqG/qosaJiCXe3I43P1/XKTONabRfcTs1wsabI6gRzR7J8KJghiFRfACKpmzCLrYJoHVYJhgM3kCgAPCjNIWQU8pqp8Kmgy3sE4c3X0wU00r54gKPEoa1Wn8SDKzemQ0LipiZEjoAZAgfw2T4/rtboWYbPExJYvVwBcCAwEAAaMYMBYwFAYDVR0RBA0wC4IJbG9jYWxob3N0MA0GCSqGSIb3DQEBCwUAA4IBAQCaxc00e5i2XPFTPh01rqdt4sRO8E+ret4R+f2l0Spi6C+nt7FrppxQHC7AaGbYpzX1zPB1hhefFiHdTnp3ermwrbSvK5PhE52YZ317FzSmMNc88BiV4teRS202WVFfSiyECCJrCrpRS6nKbm9Ab4Rlpb3P/FnZ7rQlhWyEzXUb30zc5+BXzwTAJYrCoXTS788au3M1vWFphk3Xhmg2llUtfWHOP1CgVsJKlkMRweFt4TNmOlMB8yHCSRPPFlXaxw9B1XUcZPWRWmWPa/+6eume4rDc5Wu9qGMIjOWS1m49cbN3iPTia6rO6WbyKCWIylNqcdNzemxxGb6iRsKMXyAx";
    const TEST_PRIVATE_KEY: &str = "MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQCyVQvxMtXWPH0LVcAT88FSmF0Jjr4keZrM8cZ2I8LSjsRnwGDx6mHu6HyHQLO+Pjjxnbus/EW4zDzecp3+1DI6sItig3enNfiMP3c865Vfd6C9giq67d92lrg3gOUS1Ezl/qTiblZNXkQpVQNznVDi0yOSX/Bc1wyLUiB4GQeQwaBM/95Gxhahv6qLGiYgl3tyONz9f1ykzjWm0X3E7NcLGmyOoEc0eyfCiYIYhUXwAiqZswi62CaB1WCYYDN5AoADwozSFkFPKaqfCpoMt7BOHN19MFNNK+eICjxKGtVp/Egys3pkNC4qYmRI6AGQIH8Nk+P67W6FmGzxMSWL1cAXAgMBAAECggEBAJZzr/aXN9deAvUcLEfo/3Hqf5u/pOVq/tHnLNOhCg3QSx1pLaELaAJCfEUzrjFTl4Eo3RxtXXkyPixCMM+8QIBJT98WIU2d+AqCxNtNuiDn8WHQvrIkW8JWGCcjhJ/lItdrhbpO8lqlrAXe5mGVGJe1IC6u6D+7Yqbr697G5x4VBNVc3GLVAGpJpi5sVIab+8ns7uo8ZpDr6Puc9CAObcx4WIZq2pX12YkYzZaU2KtLKoUjfNP+h8rCBkzcNz28CMKu3jgY3pE4tnf4rUUWyTqClQLDHguSi434iS21Z/Ks4leNkXdvZOAoeDQ+DS0UEKe4N0AEnuuRSM6vQDfLvxkCgYEA2FokwGlmKPvJZYlkgRqJJ6Zfee23mEGSHvm4OHZjNeKn3MwyC/U19GnfmufkwNh8bo2vPZUHdJ/DIzQCrhzAHHjNVXuejqI2kGUxGpqOoQ82mSMJGotWfgq73xe6u59YXI/v/JrgaimwIIn2SmP/nO22DY7pVwjdn+XOjF4LAPUCgYEA0wNAmWaGMvtfSBEaIqtoEdinDMCLfB2f1rpn4FNMo3k2oVdEMMnqQ53YI+SKGGyLdBqUttaNP/uJkK2KdICrM5rGnXxtzhqDh2r5RwG5WoF5z3JMXpYKN5krWKdQVkc0CjXqj0Kd2X9atp+TDo8uvgU2QyviwqQGIn3+EjoFJVsCgYBy/B3SQXIxT/huxYGr9/1zHEJcHBJakmblnZTiNVFvHyJWABSNNGrTlr1np910/NnNK/I6CY2n1w0wFYFjJhaYSz/eMdBIQEA9p/pcCE7LnLlI1E0PVYTHgk7tN8Bf3UVqFHnYyDuDUNqxwIEsck81CUWbmRu8zRJ02/9VrNmuTQKBgCG6FFowe+SsLveK3D2MXg70LQcpw2GsLn8Yvj+psMc0OZoiI6EUtN/n28Mo5TWwK737/acXte3zG3LHeijS5ApUg8hqOfbGYB2F6KAD04d2yGxy3WgE3U8zqSz7WSjhKp0zLvGE+UvpQiuMZ+nc0uDGXnzwB8eKhfx/XNu28FmfAoGBAJjSpfFEAnfFGUjZDEQthL1MZ5L07/MgIUwPtW+pqcCG5DwJXAyb6NwIUJwILMoszPAlD5GBoCR3WbRnfGYN2WqOHaVrYVsP8sAob7Ys+XZ2+O8N/i0kk9aHOhteuRpwXNZC5PUmtUA/2qqF6Sq54Yz3DJj//hBc9qN8MidiQ7PL";

    #[derive(Debug, Default)]
    struct Session {
        commands: Vec<String>,
        message: String,
    }

    fn tls_fixture() -> (Vec<u8>, Arc<ServerConfig>) {
        INSTALL_PROVIDER.call_once(|| {
            rustls::crypto::ring::default_provider()
                .install_default()
                .unwrap();
        });
        let certificate = base64::engine::general_purpose::STANDARD
            .decode(TEST_CERTIFICATE)
            .unwrap();
        let key = base64::engine::general_purpose::STANDARD
            .decode(TEST_PRIVATE_KEY)
            .unwrap();
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(certificate.clone())],
                PrivatePkcs8KeyDer::from(key).into(),
            )
            .unwrap();
        (certificate, Arc::new(config))
    }

    fn settings(port: u16, security: SmtpSecurity) -> EmailDeliverySettings {
        EmailDeliverySettings {
            host: "localhost".to_owned(),
            port,
            security,
            username: "saved-user".to_owned(),
            secret: "saved-password".to_owned(),
            sender_address: "alerts@example.com".to_owned(),
            sender_name: "db-qbs alerts".to_owned(),
        }
    }

    fn line(stream: &mut impl Read) -> String {
        let mut bytes = Vec::new();
        loop {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            bytes.push(byte[0]);
            if bytes.ends_with(b"\r\n") {
                return String::from_utf8(bytes).unwrap();
            }
        }
    }

    fn reply(stream: &mut impl Write, response: &str) {
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    }

    fn dialog(stream: &mut (impl Read + Write), reject_auth: bool) -> Session {
        let mut session = Session::default();
        reply(stream, "220 scripted SMTP ready\r\n");
        loop {
            let command = line(stream);
            session.commands.push(command.trim_end().to_owned());
            if command.starts_with("EHLO ") {
                reply(stream, "250-localhost\r\n250 AUTH PLAIN LOGIN\r\n");
            } else if command.starts_with("AUTH ") {
                if reject_auth {
                    reply(stream, "535 secret server diagnostic must not escape\r\n");
                    break;
                }
                reply(stream, "235 authenticated\r\n");
            } else if command.starts_with("MAIL FROM:") || command.starts_with("RCPT TO:") {
                reply(stream, "250 accepted\r\n");
            } else if command == "DATA\r\n" {
                reply(stream, "354 end with dot\r\n");
                loop {
                    let message_line = line(stream);
                    if message_line == ".\r\n" {
                        break;
                    }
                    session.message.push_str(&message_line);
                }
                reply(stream, "250 queued\r\n");
            } else if command == "QUIT\r\n" {
                reply(stream, "221 bye\r\n");
                break;
            } else {
                reply(stream, "500 unexpected\r\n");
                break;
            }
        }
        session
    }

    #[test]
    fn implicit_tls_contract_covers_auth_envelope_multipart_and_per_recipient_send() {
        let (certificate, config) = tls_fixture();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let sessions = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&sessions);
        let server = thread::spawn(move || {
            for _ in 0..2 {
                let (socket, _) = listener.accept().unwrap();
                let connection = ServerConnection::new(Arc::clone(&config)).unwrap();
                let mut tls = StreamOwned::new(connection, socket);
                recorded.lock().unwrap().push(dialog(&mut tls, false));
            }
        });
        let adapter = SmtpMailTransport::with_test_root(certificate, Duration::from_secs(2));
        let settings = settings(port, SmtpSecurity::ImplicitTls);

        for recipient in ["ops@example.com", "audit@example.org"] {
            let mail = multipart_mail(
                &settings.sender_address,
                &settings.sender_name,
                recipient,
                "[db-qbs][production][测试] 邮件配置验证",
                "plain content marker / 中文纯文本测试".to_owned(),
                "<p>html content marker / 中文 HTML 测试</p>".to_owned(),
            )
            .unwrap();
            adapter.send(&settings, &mail).unwrap();
        }
        server.join().unwrap();

        let sessions = sessions.lock().unwrap();
        assert_eq!(sessions.len(), 2);
        for (session, recipient) in sessions
            .iter()
            .zip(["ops@example.com", "audit@example.org"])
        {
            assert!(session
                .commands
                .iter()
                .any(|command| { command == "AUTH PLAIN AHNhdmVkLXVzZXIAc2F2ZWQtcGFzc3dvcmQ=" }));
            assert!(session
                .commands
                .iter()
                .any(|command| command == "MAIL FROM:<alerts@example.com>"));
            assert!(session
                .commands
                .iter()
                .any(|command| command == &format!("RCPT TO:<{recipient}>")));
            assert!(session.message.contains("multipart/alternative"));
            assert!(session.message.contains("text/plain"));
            assert!(session.message.contains("text/html"));
            assert!(session.message.contains("cGxhaW4gY29udGVudCBtYXJrZXI"));
            assert!(session.message.contains("html content marker"));
        }
    }

    #[test]
    fn required_starttls_upgrades_before_authentication() {
        let (certificate, config) = tls_fixture();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            reply(&mut socket, "220 scripted SMTP ready\r\n");
            assert!(line(&mut socket).starts_with("EHLO "));
            reply(&mut socket, "250-localhost\r\n250 STARTTLS\r\n");
            assert_eq!(line(&mut socket), "STARTTLS\r\n");
            reply(&mut socket, "220 begin TLS\r\n");
            let connection = ServerConnection::new(config).unwrap();
            let mut tls = StreamOwned::new(connection, socket);
            dialog_after_upgrade(&mut tls)
        });
        let adapter = SmtpMailTransport::with_test_root(certificate, Duration::from_secs(2));
        let settings = settings(port, SmtpSecurity::Starttls);
        let mail = multipart_mail(
            &settings.sender_address,
            &settings.sender_name,
            "ops@example.com",
            "test",
            "plain".to_owned(),
            "<p>html</p>".to_owned(),
        )
        .unwrap();

        adapter.send(&settings, &mail).unwrap();
        let session = server.join().unwrap();
        assert!(session
            .commands
            .iter()
            .any(|command| { command == "AUTH PLAIN AHNhdmVkLXVzZXIAc2F2ZWQtcGFzc3dvcmQ=" }));
        assert!(session
            .commands
            .iter()
            .any(|command| command == "RCPT TO:<ops@example.com>"));
    }

    fn dialog_after_upgrade(stream: &mut (impl Read + Write)) -> Session {
        let mut session = Session::default();
        loop {
            let command = line(stream);
            session.commands.push(command.trim_end().to_owned());
            if command.starts_with("EHLO ") {
                reply(stream, "250-localhost\r\n250 AUTH PLAIN LOGIN\r\n");
            } else if command.starts_with("AUTH ") {
                reply(stream, "235 authenticated\r\n");
            } else if command.starts_with("MAIL FROM:") || command.starts_with("RCPT TO:") {
                reply(stream, "250 accepted\r\n");
            } else if command == "DATA\r\n" {
                reply(stream, "354 end with dot\r\n");
                loop {
                    let message_line = line(stream);
                    if message_line == ".\r\n" {
                        break;
                    }
                    session.message.push_str(&message_line);
                }
                reply(stream, "250 queued\r\n");
            } else if command == "QUIT\r\n" {
                reply(stream, "221 bye\r\n");
                break;
            }
        }
        session
    }

    #[test]
    fn adapter_classifies_timeout_and_hides_server_diagnostics() {
        let (certificate, config) = tls_fixture();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            let connection = ServerConnection::new(config).unwrap();
            let mut tls = StreamOwned::new(connection, socket);
            dialog(&mut tls, true)
        });
        let adapter = SmtpMailTransport::with_test_root(certificate, Duration::from_secs(2));
        let delivery_settings = settings(port, SmtpSecurity::ImplicitTls);
        let mail = multipart_mail(
            &delivery_settings.sender_address,
            &delivery_settings.sender_name,
            "ops@example.com",
            "test",
            "plain".to_owned(),
            "<p>html</p>".to_owned(),
        )
        .unwrap();
        let error = adapter.send(&delivery_settings, &mail).unwrap_err();
        server.join().unwrap();
        assert_eq!(error, MailTransportError::Permanent);
        assert!(!error.sanitized_message().contains("diagnostic"));

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let black_hole = thread::spawn(move || {
            let (_socket, _) = listener.accept().unwrap();
            thread::sleep(Duration::from_secs(1));
        });
        let (timeout_certificate, _) = tls_fixture();
        let adapter =
            SmtpMailTransport::with_test_root(timeout_certificate, Duration::from_millis(100));
        let timeout_settings = settings(port, SmtpSecurity::ImplicitTls);
        assert_eq!(
            adapter.send(&timeout_settings, &mail).unwrap_err(),
            MailTransportError::Timeout
        );
        black_hole.join().unwrap();
    }
}
