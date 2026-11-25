//! 事务邮件的边界：SMTP 凭据只在 Rust 内存中，底层错误和邮件正文都不进日志。

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use assemble_store::secret::SecretString;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::config::ConfigError;

#[derive(Debug, Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub secure: bool,
    pub username: String,
    pub password: SecretString,
    pub from: String,
}

impl SmtpConfig {
    pub fn from_lookup(
        get: &impl Fn(&'static str) -> Option<String>,
    ) -> Result<Option<Self>, ConfigError> {
        let keys = [
            "SMTP_HOST",
            "SMTP_PORT",
            "SMTP_SECURE",
            "SMTP_USER",
            "SMTP_PASS",
            "SMTP_PASS_FILE",
            "SMTP_FROM",
        ];
        if keys.iter().all(|key| get(key).is_none()) {
            return Ok(None);
        }
        let required = |key| {
            get(key)
                .filter(|v| !v.trim().is_empty())
                .ok_or(ConfigError::Missing { variable: key })
        };
        let invalid = |variable| ConfigError::Invalid {
            variable,
            reason: "邮件配置不合法，请核对连接参数".to_owned(),
        };
        let host = required("SMTP_HOST")?;
        if host.len() > 253
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
        {
            return Err(invalid("SMTP_HOST"));
        }
        let port = required("SMTP_PORT")?
            .parse::<u16>()
            .ok()
            .filter(|p| *p > 0)
            .ok_or_else(|| invalid("SMTP_PORT"))?;
        let secure = match required("SMTP_SECURE")?.as_str() {
            "true" => true,
            "false" => false,
            _ => return Err(invalid("SMTP_SECURE")),
        };
        let from = required("SMTP_FROM")?;
        from.parse::<Mailbox>().map_err(|_| invalid("SMTP_FROM"))?;
        Ok(Some(Self {
            host,
            port,
            secure,
            from,
            username: required("SMTP_USER")?,
            password: crate::config::secret(get, "SMTP_PASS", "SMTP_PASS_FILE")?.ok_or(
                ConfigError::Missing {
                    variable: "SMTP_PASS",
                },
            )?,
        }))
    }
}

#[derive(Debug)]
pub struct Mail {
    pub to: String,
    pub subject: &'static str,
    pub body: SecretString,
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("transactional mail is unavailable")]
pub struct MailError;

pub type Delivery<'a> = Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>>;

pub trait Mailer: std::fmt::Debug + Send + Sync {
    fn available(&self) -> bool {
        true
    }
    fn send(&self, mail: Mail) -> Delivery<'_>;
}

#[derive(Debug)]
pub struct SmtpMailer(pub Option<SmtpConfig>);

impl Mailer for SmtpMailer {
    fn available(&self) -> bool {
        self.0.is_some()
    }
    fn send(&self, mail: Mail) -> Delivery<'_> {
        Box::pin(async move {
            let config = self.0.as_ref().ok_or(MailError)?;
            let builder = if config.secure {
                AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host)
            } else {
                // false 指 STARTTLS，不允许明文发送授权码。
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.host)
            }
            .map_err(|_| MailError)?;
            let transport = builder
                .port(config.port)
                .credentials(Credentials::new(
                    config.username.clone(),
                    config.password.expose().to_owned(),
                ))
                .timeout(Some(Duration::from_secs(15)))
                .build();
            let message = Message::builder()
                .from(config.from.parse().map_err(|_| MailError)?)
                .to(mail.to.parse().map_err(|_| MailError)?)
                .subject(mail.subject)
                .body(mail.body.expose().to_owned())
                .map_err(|_| MailError)?;
            tokio::time::timeout(Duration::from_secs(20), transport.send(message))
                .await
                .map_err(|_| MailError)?
                .map_err(|_| MailError)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smtp_config_requires_all_fields_and_redacts_the_authorization_code() {
        let values = [
            ("SMTP_HOST", "smtp.example.test"),
            ("SMTP_PORT", "465"),
            ("SMTP_SECURE", "true"),
            ("SMTP_USER", "sender@example.test"),
            ("SMTP_PASS", "test-authorization-code"),
            ("SMTP_FROM", "Impact <sender@example.test>"),
        ];
        let get = |key| {
            values
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_owned())
        };
        let config = SmtpConfig::from_lookup(&get).unwrap().unwrap();
        assert!(config.secure);
        assert_eq!(config.port, 465);
        assert!(!format!("{config:?}").contains("test-authorization-code"));
        for missing in ["SMTP_USER", "SMTP_PASS", "SMTP_FROM"] {
            let get = |key| {
                (key != missing)
                    .then(|| {
                        values
                            .iter()
                            .find(|(k, _)| *k == key)
                            .map(|(_, v)| (*v).to_owned())
                    })
                    .flatten()
            };
            assert!(SmtpConfig::from_lookup(&get).is_err());
        }
        assert!(SmtpConfig::from_lookup(&|_| None).unwrap().is_none());
    }
}
