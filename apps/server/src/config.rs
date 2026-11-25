//! 服务端配置：全部来自环境变量，前缀 `ASSEMBLE_`。
//!
//! 没有配置文件。数据目录与对外地址必须显式给出——猜错任何一个的后果
//! （数据写到别处、Cookie 发错域）都比启动失败更糟。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use assemble_protocol::origin::ServerOrigin;
use subtle::ConstantTimeEq;

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
/// 邀请码：填了才开放网页注册，要填对它才能建账户。不填就是不开放注册。
pub const INVITE_CODE: &str = "ASSEMBLE_INVITE_CODE";

/// 邀请码的长度范围（字符）。太短了可以被一个个试出来。
pub const INVITE_CODE_MIN_CHARS: usize = 8;
pub const INVITE_CODE_MAX_CHARS: usize = 128;

pub const DEFAULT_ADDR: &str = "127.0.0.1:8787";
pub const DEFAULT_DRIVE_QUOTA_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const DEFAULT_SEND_TTL: Duration = Duration::from_hours(24);
pub const DEFAULT_SEND_HISTORY: Duration = Duration::from_hours(24 * 30);
pub const DEFAULT_WEB_SESSION_TTL: Duration = Duration::from_hours(12);

/// 小时数的上限：一年。填错单位（比如把秒数当小时填）时不至于把东西留一辈子。
const MAX_HOURS: u64 = 365 * 24;
/// 天数的上限：十年。
const MAX_DAYS: u64 = 3650;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub addr: SocketAddr,
    pub data_dir: PathBuf,
    pub public_origin: ServerOrigin,
    pub drive_quota_bytes: u64,
    pub send_ttl: Duration,
    pub send_history: Duration,
    pub web_session_ttl: Duration,
    /// 邀请码的摘要，`None` 是不开放注册。
    ///
    /// 配置里给的是明文，读完只留摘要：进程里、日志里、这份配置的调试输出里都不会出现邀请码本身。
    /// 比对的时候把用户填的也算一遍摘要再比。
    pub invite_code: Option<String>,
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
        let invite_code = optional(INVITE_CODE, get(INVITE_CODE), |value| {
            invite_digest(value).ok_or_else(|| {
                format!(
                    "需要是 {INVITE_CODE_MIN_CHARS} 到 {INVITE_CODE_MAX_CHARS} 个字符，不能有空白"
                )
            })
        })?;

        Ok(Self {
            addr,
            data_dir,
            public_origin,
            drive_quota_bytes,
            send_ttl,
            send_history,
            web_session_ttl,
            invite_code,
        })
    }

    /// 网页注册开着没有：配了邀请码才开。
    pub fn registration_open(&self) -> bool {
        self.invite_code.is_some()
    }

    /// 用户填的邀请码对不对。没有配邀请码，填什么都是错的。
    pub fn invite_matches(&self, typed: &str) -> bool {
        self.invite_code.as_ref().is_some_and(|expected| {
            // 长度先比，内容再按恒定时间比：从快慢上看不出对到了第几个字符。
            same_secret(expected, &crate::secrets::token_digest(typed))
        })
    }

    /// 在这份配置里设一个邀请码，把注册打开。测试与开发工具用。
    #[must_use]
    pub fn with_invite_code(mut self, code: &str) -> Self {
        self.invite_code = invite_digest(code);
        self
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
            invite_code: None,
        }
    }

    /// 把对外地址换成指定的来源。测试里端口要到监听之后才知道。
    #[must_use]
    pub fn with_public_origin(mut self, origin: ServerOrigin) -> Self {
        self.public_origin = origin;
        self
    }
}

/// 邀请码合不合规则；合的话交出它的摘要。
fn invite_digest(raw: &str) -> Option<String> {
    let length = raw.chars().count();
    let too_short = length < INVITE_CODE_MIN_CHARS;
    let too_long = length > INVITE_CODE_MAX_CHARS;
    let untypable = raw
        .chars()
        .any(|character| character.is_whitespace() || character.is_control());
    (!too_short && !too_long && !untypable).then(|| crate::secrets::token_digest(raw))
}

/// 两个值必须一样长、内容相同。长度不同的直接就是不一样，不值得做恒定时间比较。
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
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
        // 没配邀请码：不开放注册。
        assert_eq!(config.invite_code, None);
        assert!(!config.registration_open());
        assert!(!config.invite_matches(""));
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
            (INVITE_CODE, "welcome-to-assemble"),
        ]))
        .unwrap();
        assert_eq!(config.addr.to_string(), "0.0.0.0:9000");
        // 配置里只留摘要，不留邀请码本身。
        assert_eq!(
            config.invite_code.as_deref(),
            Some(crate::secrets::token_digest("welcome-to-assemble").as_str())
        );
        assert!(!format!("{config:?}").contains("welcome-to-assemble"));
        assert!(config.registration_open());
        assert!(config.invite_matches("welcome-to-assemble"));
        assert!(!config.invite_matches("welcome-to-assemblf"));
        assert!(!config.invite_matches(""));
        assert_eq!(config.drive_quota_bytes, 1000);
        assert_eq!(config.send_ttl, Duration::from_secs(6 * 60 * 60));
        assert_eq!(config.send_history, Duration::from_secs(7 * 24 * 60 * 60));
        assert_eq!(config.web_session_ttl, Duration::from_secs(60 * 60));

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
            (INVITE_CODE, "short"),
            (INVITE_CODE, &"x".repeat(129)),
            (INVITE_CODE, "has a space"),
            (INVITE_CODE, "line\nbreak"),
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
        // 邀请码太短时的说明里也不回显填进去的是什么。
        let short = Config::from_lookup(lookup(&[
            (DATA_DIR, "/data"),
            (PUBLIC_ORIGIN, "https://assemble.example"),
            (INVITE_CODE, "hunter2"),
        ]))
        .unwrap_err()
        .to_string();
        assert!(
            short.starts_with("ASSEMBLE_INVITE_CODE is invalid"),
            "{short}"
        );
        assert!(!short.contains("hunter2"), "{short}");
    }
}
