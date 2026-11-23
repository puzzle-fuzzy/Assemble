//! 服务来源。
//!
//! Assemble 的服务端同时提供接口与登录页，所以只有这一个来源。
//! 桌面端连接哪个来源由构建决定：不从界面输入，也不从用户配置读取。
//! 正式服务只接受 HTTPS；HTTP 只允许用在开发时的回环地址上。
//! 服务端自己也用同一个规则校验对外公布的地址。

use std::fmt;
use std::str::FromStr;

use url::Url;

use crate::login::AUTHORIZATION_PATH;

/// 一个服务来源：协议、主机与端口，不带路径、查询或片段。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServerOrigin(Url);

impl ServerOrigin {
    /// 本地开发服务的默认来源。
    pub const DEVELOPMENT: &'static str = "http://127.0.0.1:8787";

    /// 解析一个来源。只接受 HTTPS，或回环地址上的 HTTP。
    pub fn parse(value: &str) -> Result<Self, OriginError> {
        let url = Url::parse(value).map_err(|_| OriginError::NotAnOrigin)?;
        let clean = url.path() == "/"
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none();
        if !clean {
            return Err(OriginError::NotAnOrigin);
        }
        check_scheme(&url)?;
        Ok(Self(url))
    }

    /// 本地开发服务。
    pub fn development() -> Self {
        Self::parse(Self::DEVELOPMENT).expect("the development origin is valid")
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str().trim_end_matches('/')
    }

    /// 是否通过 HTTPS 访问。
    pub fn is_secure(&self) -> bool {
        self.0.scheme() == "https"
    }

    /// 把一条以 `/` 开头的路径接到这个来源后面。
    pub fn join(&self, path: &str) -> Url {
        let mut url = self.0.clone();
        url.set_path(path);
        url
    }

    /// 授权页的地址：桌面登录请求编码在 `request` 查询参数里。
    pub fn authorization_url(&self, encoded_request: &str) -> Url {
        let mut url = self.join(AUTHORIZATION_PATH);
        url.query_pairs_mut()
            .append_pair("request", encoded_request);
        url
    }
}

impl fmt::Display for ServerOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ServerOrigin {
    type Err = OriginError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginError {
    NotAnOrigin,
    NotSecure,
}

impl fmt::Display for OriginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::NotAnOrigin => "an origin has no path, query, fragment or credentials",
            Self::NotSecure => "use an HTTPS origin, or HTTP on loopback for development",
        };
        f.write_str(message)
    }
}

impl std::error::Error for OriginError {}

fn check_scheme(url: &Url) -> Result<(), OriginError> {
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    match url.scheme() {
        "https" => Ok(()),
        "http" if loopback => Ok(()),
        _ => Err(OriginError::NotSecure),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_origin_has_no_path_query_or_credentials() {
        let origin = ServerOrigin::parse("https://assemble.example:8443/").unwrap();
        assert_eq!(origin.as_str(), "https://assemble.example:8443");
        assert!(origin.is_secure());
        assert_eq!(origin.join("/v1/devices").path(), "/v1/devices");

        for rejected in [
            "https://assemble.example/app",
            "https://assemble.example?x=1",
            "https://assemble.example#top",
            "https://user:pass@assemble.example",
            "not a url",
        ] {
            assert_eq!(
                ServerOrigin::parse(rejected).unwrap_err(),
                OriginError::NotAnOrigin,
                "{rejected}"
            );
        }
    }

    #[test]
    fn plain_http_is_only_allowed_on_loopback() {
        for allowed in [
            "http://127.0.0.1:8787",
            "http://localhost:8787",
            "http://[::1]:8787",
        ] {
            let origin = ServerOrigin::parse(allowed).unwrap();
            assert!(!origin.is_secure(), "{allowed}");
        }
        for rejected in [
            "http://assemble.example",
            "http://192.168.1.2:8787",
            "ftp://127.0.0.1",
            "file:///",
        ] {
            assert_eq!(
                ServerOrigin::parse(rejected).unwrap_err(),
                OriginError::NotSecure,
                "{rejected}"
            );
        }
    }

    #[test]
    fn the_authorization_url_carries_the_request_in_the_query() {
        let production = ServerOrigin::parse("https://assemble.example").unwrap();
        assert_eq!(
            production.authorization_url("eyJabc-_123").as_str(),
            "https://assemble.example/desktop/authorize?request=eyJabc-_123"
        );
        // 请求里出现需要转义的字符时，查询参数仍然是一个完整的值。
        let escaped = production.authorization_url("a+b=c&d");
        let query = escaped.query().unwrap();
        assert_eq!(query, "request=a%2Bb%3Dc%26d");

        let development = ServerOrigin::development();
        assert_eq!(development.as_str(), "http://127.0.0.1:8787");
        assert_eq!(
            development.authorization_url("eyJabc-_123").as_str(),
            "http://127.0.0.1:8787/desktop/authorize?request=eyJabc-_123"
        );
        assert_eq!(development.to_string(), "http://127.0.0.1:8787");
        assert_eq!(
            "https://assemble.example".parse::<ServerOrigin>().unwrap(),
            production
        );
    }
}
