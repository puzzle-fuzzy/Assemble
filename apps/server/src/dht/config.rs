use crate::config::ConfigError;
use std::net::SocketAddr;

#[derive(Debug, Clone)]
pub struct Config {
    pub enabled: bool,
    pub listen: SocketAddr,
    pub max_records: i64,
    pub peer_exchange: bool,
    /// 固定引导节点；测试可注入空列表，使网络验收保持离线。
    pub bootstrap_hosts: Vec<String>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: "0.0.0.0:6882".parse().expect("a fixed address parses"),
            max_records: 100_000,
            peer_exchange: false,
            bootstrap_hosts: vec![
                "router.bittorrent.com:6881".into(),
                "dht.transmissionbt.com:6881".into(),
                "router.utorrent.com:6881".into(),
            ],
        }
    }
}
impl Config {
    pub fn from_lookup(get: &impl Fn(&'static str) -> Option<String>) -> Result<Self, ConfigError> {
        fn boolean(
            get: &impl Fn(&'static str) -> Option<String>,
            key: &'static str,
        ) -> Result<bool, ConfigError> {
            match get(key).as_deref() {
                None | Some("false") => Ok(false),
                Some("true") => Ok(true),
                _ => Err(ConfigError::Invalid {
                    variable: key,
                    reason: "需要 true 或 false".into(),
                }),
            }
        }
        let defaults = Self::default();
        let listen = get("ASSEMBLE_DHT_LISTEN")
            .map(|value| {
                value
                    .parse::<SocketAddr>()
                    .ok()
                    .filter(|address| address.is_ipv4() && address.port() != 0)
                    .ok_or_else(|| ConfigError::Invalid {
                        variable: "ASSEMBLE_DHT_LISTEN",
                        reason: "需要 IPv4 地址与非零端口".into(),
                    })
            })
            .transpose()?
            .unwrap_or(defaults.listen);
        let max_records = get("ASSEMBLE_DHT_MAX_RECORDS")
            .map(|value| {
                value
                    .parse::<i64>()
                    .ok()
                    .filter(|value| (1000..=1_000_000).contains(value))
                    .ok_or_else(|| ConfigError::Invalid {
                        variable: "ASSEMBLE_DHT_MAX_RECORDS",
                        reason: "需要 1000 到 1000000 之间的整数".into(),
                    })
            })
            .transpose()?
            .unwrap_or(defaults.max_records);
        Ok(Self {
            enabled: boolean(get, "ASSEMBLE_DHT_ENABLED")?,
            listen,
            max_records,
            peer_exchange: boolean(get, "ASSEMBLE_DHT_PEER_EXCHANGE")?,
            bootstrap_hosts: defaults.bootstrap_hosts,
        })
    }
}
