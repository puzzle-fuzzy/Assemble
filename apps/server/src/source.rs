//! 请求真正来自哪个地址：只在部署者说明了"哪个请求头可信"之后才认。
//!
//! 服务端不读转发头。在它看来，所有请求都来自反向代理，对端地址永远是同一个。
//! 可是要按来源限速、不让旁人锁住别人的账户，就得知道请求从哪里来。所以由部署者
//! 显式配置反向代理写进来源地址的那个请求头（nginx 的 `X-Real-IP`）；没配置时，
//! 所有请求一律当作"来源未知"，各处按来源设的限额不生效，只留下不按来源的那一层。
//!
//! 这样做的代价要说清楚：配置了它，就等于相信只有反向代理能连到服务端。服务端只
//! 监听本机、端口不对外时成立；要是有人能让服务端直接收到请求，他就能自己填这个头。
//!
//! 来源只拿来限速，不写日志，不参与授权，也不回给任何调用方。

use axum::http::{HeaderMap, HeaderName};

/// 一个来源：反向代理记下的地址；没配置或者头里没有，就是未知。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Source(Option<String>);

/// 来源地址最多这么长。正常的 IPv4、IPv6 远不到这个数；更长的不像地址，按未知算。
const MAX_CHARS: usize = 64;

impl Source {
    /// 来源未知。只按账户、不按来源设的限额照样生效。
    pub fn unknown() -> Self {
        Self(None)
    }

    /// 从请求头里取来源。`trusted` 是配置里说明的那个头；没配置就什么都不看。
    pub fn read(headers: &HeaderMap, trusted: Option<&HeaderName>) -> Self {
        let Some(trusted) = trusted else {
            return Self(None);
        };
        let value = headers
            .get(trusted)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= MAX_CHARS
                    && value.bytes().all(|byte| byte.is_ascii_graphic())
            });
        Self(value.map(ToOwned::to_owned))
    }

    pub fn is_unknown(&self) -> bool {
        self.0.is_none()
    }

    /// 已知时给出那个地址，未知时是 `None`。
    pub fn known(&self) -> Option<&str> {
        self.0.as_deref()
    }

    /// 当限流的键用。未知的来源共用一个键，于是它们互相之间不区分。
    pub fn key(&self) -> &str {
        self.0.as_deref().unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn the_source_is_read_only_from_the_header_the_operator_trusted() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static(" 203.0.113.7 "));
        headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.9"));
        let trusted = HeaderName::from_static("x-real-ip");

        // 配置了才认，只认那一个头；两端的空白不算地址的一部分。
        assert_eq!(Source::read(&headers, Some(&trusted)).key(), "203.0.113.7");
        assert!(Source::read(&headers, None).is_unknown());
        let other = HeaderName::from_static("x-client-ip");
        assert!(Source::read(&headers, Some(&other)).is_unknown());

        // 不像地址的值按未知算：空的、过长的、中间夹着空白的。
        for odd in ["", &"a".repeat(65), "203.0 113.7", "   "] {
            let mut headers = HeaderMap::new();
            headers.insert("x-real-ip", HeaderValue::from_str(odd).unwrap());
            assert!(
                Source::read(&headers, Some(&trusted)).is_unknown(),
                "{odd:?}"
            );
        }
        assert!(Source::unknown().is_unknown());
        assert_eq!(Source::unknown().key(), "");
    }
}
