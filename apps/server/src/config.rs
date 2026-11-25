//! 服务端配置：环境变量和受保护密钥文件引用。
//!
//! 没有配置文件。数据目录与对外地址必须显式给出——猜错任何一个的后果
//! （数据写到别处、Cookie 发错域）都比启动失败更糟。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use assemble_protocol::origin::ServerOrigin;
use assemble_store::secret::SecretString;

/// 数据目录必须显式指定。
pub const DATA_DIR: &str = "ASSEMBLE_DATA_DIR";
/// 浏览器访问服务端用的地址。决定 Cookie 的属性、Origin 校验和授权跳转。
pub const PUBLIC_ORIGIN: &str = "ASSEMBLE_PUBLIC_ORIGIN";
/// 监听地址。默认只监听本机，由反向代理转发进来。
pub const ADDR: &str = "ASSEMBLE_ADDR";
/// 每个账户的云盘上限（字节）。
pub const DRIVE_QUOTA_BYTES: &str = "ASSEMBLE_DRIVE_QUOTA_BYTES";
/// 未送达的发送保留多少小时。
pub const SEND_TTL_HOURS: &str = "ASSEMBLE_SEND_TTL_HOURS";
/// 发送记录保留多少天。
pub const SEND_HISTORY_DAYS: &str = "ASSEMBLE_SEND_HISTORY_DAYS";
/// 网页登录状态保留多少小时。
pub const WEB_SESSION_HOURS: &str = "ASSEMBLE_WEB_SESSION_HOURS";
/// 只为已经验证的指定邮箱授予管理权限；不影响公开注册。
pub const SUPER_ADMIN_EMAIL: &str = "ASSEMBLE_SUPER_ADMIN_EMAIL";
/// 与服务端同源发布的管理端构建目录，不包含任何账户数据。
pub const ADMIN_ASSETS_DIR: &str = "ASSEMBLE_ADMIN_ASSETS_DIR";
pub const DEFAULT_ADDR: &str = "127.0.0.1:8787";
pub const DEFAULT_DRIVE_QUOTA_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const DEFAULT_SEND_TTL: Duration = Duration::from_hours(24);
pub const DEFAULT_SEND_HISTORY: Duration = Duration::from_hours(24 * 30);
pub const DEFAULT_WEB_SESSION_TTL: Duration = Duration::from_hours(12);

/// 小时数的上限：一年。填错单位（比如把秒数当小时填）时不至于把东西留一辈子。
const MAX_HOURS: u64 = 365 * 24;
/// 天数的上限：十年。
const MAX_DAYS: u64 = 3650;

#[derive(Debug, Clone)]
pub struct Config {
    pub addr: SocketAddr,
    pub data_dir: PathBuf,
    pub public_origin: ServerOrigin,
    pub drive_quota_bytes: u64,
    pub send_ttl: Duration,
    pub send_history: Duration,
    pub web_session_ttl: Duration,
    pub smtp: Option<crate::mail::SmtpConfig>,
    pub github: Option<crate::github::GithubConfig>,
    pub super_admin_email: Option<String>,
    pub admin_assets_dir: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// 必填的环境变量没有给。
    Missing { variable: &'static str },
    /// 给了，但看不懂或不合理。
    Invalid {
        variable: &'static str,
        reason: String,
    },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { variable } => write!(f, "{variable} is required"),
            Self::Invalid { variable, reason } => {
                write!(f, "{variable} is invalid: {reason}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// 密钥可以引用受保护文件；错误不回显值或文件路径。
pub(crate) fn secret(
    get: &impl Fn(&'static str) -> Option<String>,
    variable: &'static str,
    file_variable: &'static str,
) -> Result<Option<SecretString>, ConfigError> {
    match (get(variable), get(file_variable)) {
        (None, None) => Ok(None),
        (Some(_), Some(_)) => Err(ConfigError::Invalid {
            variable,
            reason: "密钥值与文件引用不能同时配置".to_owned(),
        }),
        (value, path) => {
            let value = if let Some(path) = path {
                let invalid = || ConfigError::Invalid {
                    variable: file_variable,
                    reason: "密钥文件不可读、为空或过大".to_owned(),
                };
                let file = std::fs::File::open(path).map_err(|_| invalid())?;
                if !file.metadata().map_err(|_| invalid())?.is_file() {
                    return Err(invalid());
                }
                let mut value = String::new();
                std::io::Read::read_to_string(&mut std::io::Read::take(file, 4097), &mut value)
                    .map_err(|_| invalid())?;
                if value.trim().is_empty() || value.len() > 4096 {
                    return Err(invalid());
                }
                value.trim_end_matches(['\r', '\n']).to_owned()
            } else {
                value.expect("one source exists")
            };
            if value.trim().is_empty() {
                return Err(ConfigError::Missing { variable });
            }
            Ok(Some(SecretString::new(value)))
        }
    }
}

impl Config {
    /// 从进程环境变量读取。
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
    }

    /// 从任意来源读取配置。环境变量为空串等同于没设。
    pub fn from_lookup(get: impl Fn(&'static str) -> Option<String>) -> Result<Self, ConfigError> {
        let addr = optional(ADDR, get(ADDR), |value| {
            value
                .parse::<SocketAddr>()
                .map_err(|_| "要写成 地址:端口，例如 127.0.0.1:8787".to_owned())
        })?
        .unwrap_or_else(|| DEFAULT_ADDR.parse().expect("the default address is valid"));
        let data_dir = PathBuf::from(required(&get, DATA_DIR)?);
        let public_origin = optional(PUBLIC_ORIGIN, get(PUBLIC_ORIGIN), |value| {
            ServerOrigin::parse(value).map_err(|_| {
                "必须是 HTTPS 地址（本机开发可以用 http://127.0.0.1:端口），不带路径、查询和用户信息"
                    .to_owned()
            })
        })?
        .ok_or(ConfigError::Missing {
            variable: PUBLIC_ORIGIN,
        })?;
        let drive_quota_bytes = optional(
            DRIVE_QUOTA_BYTES,
            get(DRIVE_QUOTA_BYTES),
            whole(1, u64::MAX),
        )?
        .unwrap_or(DEFAULT_DRIVE_QUOTA_BYTES);
        let send_ttl =
            optional(SEND_TTL_HOURS, get(SEND_TTL_HOURS), hours)?.unwrap_or(DEFAULT_SEND_TTL);
        let send_history = optional(SEND_HISTORY_DAYS, get(SEND_HISTORY_DAYS), |value| {
            whole(1, MAX_DAYS)(value).map(|days| Duration::from_secs(days * 24 * 60 * 60))
        })?
        .unwrap_or(DEFAULT_SEND_HISTORY);
        let web_session_ttl = optional(WEB_SESSION_HOURS, get(WEB_SESSION_HOURS), hours)?
            .unwrap_or(DEFAULT_WEB_SESSION_TTL);

        Ok(Self {
            addr,
            data_dir,
            public_origin,
            drive_quota_bytes,
            send_ttl,
            send_history,
            web_session_ttl,
            smtp: crate::mail::SmtpConfig::from_lookup(&get)?,
            github: crate::github::GithubConfig::from_lookup(&get)?,
            super_admin_email: optional(SUPER_ADMIN_EMAIL, get(SUPER_ADMIN_EMAIL), |value| {
                crate::email_identity::normalize_email(value)
                    .map_err(|_| "必须是合法邮箱".to_owned())
            })?,
            admin_assets_dir: get(ADMIN_ASSETS_DIR)
                .filter(|value| !value.trim().is_empty())
                .map_or_else(|| PathBuf::from("apps/admin/dist"), PathBuf::from),
        })
    }

    /// 本地开发与测试用的配置：只监听回环地址的随机端口，其余取默认值。
    pub fn development(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            addr: "127.0.0.1:0".parse().expect("a loopback address parses"),
            data_dir: data_dir.into(),
            public_origin: ServerOrigin::development(),
            drive_quota_bytes: DEFAULT_DRIVE_QUOTA_BYTES,
            send_ttl: DEFAULT_SEND_TTL,
            send_history: DEFAULT_SEND_HISTORY,
            web_session_ttl: DEFAULT_WEB_SESSION_TTL,
            smtp: None,
            github: None,
            super_admin_email: None,
            admin_assets_dir: PathBuf::from("apps/admin/dist"),
        }
    }

    /// 把对外地址换成指定的来源。测试里端口要到监听之后才知道。
    #[must_use]
    pub fn with_public_origin(mut self, origin: ServerOrigin) -> Self {
        self.public_origin = origin;
        self
    }
}

fn required(
    get: &impl Fn(&'static str) -> Option<String>,
    variable: &'static str,
) -> Result<String, ConfigError> {
    get(variable)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or(ConfigError::Missing { variable })
}

/// 解析一个可选的环境变量：没设就是 `None`，设了就必须合法。
fn optional<T>(
    variable: &'static str,
    raw: Option<String>,
    interpret: impl Fn(&str) -> Result<T, String>,
) -> Result<Option<T>, ConfigError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    interpret(raw)
        .map(Some)
        .map_err(|reason| ConfigError::Invalid { variable, reason })
}

/// 一个落在区间内的整数。说明会原样出现在命令行上，所以用中文；不回显填进去的值。
fn whole(min: u64, max: u64) -> impl Fn(&str) -> Result<u64, String> {
    move |value| {
        value
            .parse::<u64>()
            .ok()
            .filter(|number| (min..=max).contains(number))
            .ok_or_else(|| format!("需要是 {min} 到 {max} 之间的整数"))
    }
}

fn hours(value: &str) -> Result<Duration, String> {
    whole(1, MAX_HOURS)(value).map(|count| Duration::from_secs(count * 60 * 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_files_are_read_only_by_rust_and_diagnostics_never_echo_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("password");
        std::fs::write(&path, "test-private-value\n").unwrap();
        let get = |key| (key == "SMTP_PASS_FILE").then(|| path.to_string_lossy().into_owned());
        let value = secret(&get, "SMTP_PASS", "SMTP_PASS_FILE")
            .unwrap()
            .unwrap();
        assert_eq!(value.expose(), "test-private-value");
        assert!(!format!("{value:?}").contains("test-private-value"));
        let get = |key| match key {
            "SMTP_PASS_FILE" => Some(path.to_string_lossy().into_owned()),
            "SMTP_PASS" => Some("test-private-value".to_owned()),
            _ => None,
        };
        let error = secret(&get, "SMTP_PASS", "SMTP_PASS_FILE")
            .unwrap_err()
            .to_string();
        assert!(!error.contains("test-private-value"));
        assert!(!error.contains(path.to_string_lossy().as_ref()));
        std::fs::write(&path, "x".repeat(4097)).unwrap();
        let get = |key| (key == "SMTP_PASS_FILE").then(|| path.to_string_lossy().into_owned());
        assert!(secret(&get, "SMTP_PASS", "SMTP_PASS_FILE").is_err());
    }

    fn lookup<'a>(
        values: &'a [(&'static str, &'a str)],
    ) -> impl Fn(&'static str) -> Option<String> + 'a {
        move |name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    const MINIMAL: [(&str, &str); 2] = [
        (DATA_DIR, "/var/lib/assemble"),
        (PUBLIC_ORIGIN, "https://assemble.example"),
    ];

    #[test]
    fn the_two_required_variables_must_be_given() {
        let config = Config::from_lookup(lookup(&MINIMAL)).unwrap();
        assert_eq!(config.data_dir, PathBuf::from("/var/lib/assemble"));
        assert_eq!(config.public_origin.as_str(), "https://assemble.example");
        assert_eq!(config.addr.to_string(), DEFAULT_ADDR);
        assert_eq!(config.drive_quota_bytes, DEFAULT_DRIVE_QUOTA_BYTES);
        assert_eq!(config.send_ttl, DEFAULT_SEND_TTL);
        assert_eq!(config.send_history, DEFAULT_SEND_HISTORY);
        assert_eq!(config.web_session_ttl, DEFAULT_WEB_SESSION_TTL);
        assert!(config.smtp.is_none());
        assert!(config.github.is_none());
        assert!(config.super_admin_email.is_none());
        assert_eq!(DEFAULT_DRIVE_QUOTA_BYTES, 2 * 1024 * 1024 * 1024);

        assert_eq!(
            Config::from_lookup(|_| None).unwrap_err(),
            ConfigError::Missing { variable: DATA_DIR }
        );
        // 空串等同于没设。
        assert_eq!(
            Config::from_lookup(lookup(&[(DATA_DIR, "  "), (PUBLIC_ORIGIN, "")])).unwrap_err(),
            ConfigError::Missing { variable: DATA_DIR }
        );
        assert_eq!(
            Config::from_lookup(lookup(&[(DATA_DIR, "/data")])).unwrap_err(),
            ConfigError::Missing {
                variable: PUBLIC_ORIGIN
            }
        );
    }

    #[test]
    fn optional_variables_take_their_defaults_and_must_be_valid_when_given() {
        let config = Config::from_lookup(lookup(&[
            (DATA_DIR, "/data"),
            (PUBLIC_ORIGIN, "http://127.0.0.1:8787"),
            (ADDR, "0.0.0.0:9000"),
            (DRIVE_QUOTA_BYTES, "1000"),
            (SEND_TTL_HOURS, "6"),
            (SEND_HISTORY_DAYS, "7"),
            (WEB_SESSION_HOURS, "1"),
            (SUPER_ADMIN_EMAIL, " Admin@Example.Test "),
        ]))
        .unwrap();
        assert_eq!(config.addr.to_string(), "0.0.0.0:9000");
        assert_eq!(config.drive_quota_bytes, 1000);
        assert_eq!(config.send_ttl, Duration::from_secs(6 * 60 * 60));
        assert_eq!(config.send_history, Duration::from_secs(7 * 24 * 60 * 60));
        assert_eq!(config.web_session_ttl, Duration::from_secs(60 * 60));
        assert_eq!(
            config.super_admin_email.as_deref(),
            Some("admin@example.test")
        );

        let too_many_hours = MAX_HOURS.saturating_add(1).to_string();
        for (variable, value) in [
            (ADDR, "localhost:8787"),
            (PUBLIC_ORIGIN, "http://assemble.example"),
            (PUBLIC_ORIGIN, "https://user:pass@assemble.example"),
            (DRIVE_QUOTA_BYTES, "2 GB"),
            (DRIVE_QUOTA_BYTES, "0"),
            (SEND_TTL_HOURS, "0"),
            (SEND_TTL_HOURS, "-1"),
            (SEND_TTL_HOURS, too_many_hours.as_str()),
            (SEND_HISTORY_DAYS, "forever"),
            (WEB_SESSION_HOURS, "1.5"),
            (SUPER_ADMIN_EMAIL, "invalid-email"),
        ] {
            // 同名变量以排在前面的为准，所以把不合法的值放在最前。
            let get = |name: &'static str| {
                if name == variable {
                    return Some(value.to_owned());
                }
                lookup(&MINIMAL)(name)
            };
            let error = Config::from_lookup(get).unwrap_err();
            assert!(
                matches!(
                    &error,
                    ConfigError::Invalid {
                        variable: found,
                        ..
                    } if *found == variable
                ),
                "{variable}={value} -> {error:?}"
            );
        }
    }

    #[test]
    fn the_error_names_the_variable_without_echoing_its_value() {
        let error = Config::from_lookup(lookup(&[
            (DATA_DIR, "/data"),
            (PUBLIC_ORIGIN, "http://secret-host.example"),
        ]))
        .unwrap_err();
        let text = error.to_string();
        assert!(
            text.starts_with("ASSEMBLE_PUBLIC_ORIGIN is invalid"),
            "{text}"
        );
        assert!(!text.contains("secret-host"));
        assert_eq!(
            ConfigError::Missing { variable: DATA_DIR }.to_string(),
            "ASSEMBLE_DATA_DIR is required"
        );
    }
}
