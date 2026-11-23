//! 服务来源与部署参数。
//!
//! 来源由构建决定，不从界面输入或用户配置读取。正式服务只接受 HTTPS；
//! HTTP 只允许用在开发时的回环地址上。

use std::fmt;
use std::str::FromStr;

use url::Url;

/// 一个服务来源：协议、主机与端口，不带路径、查询或片段。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ApiOrigin(Url);

impl ApiOrigin {
    /// 正式服务的来源。
    pub const PRODUCTION: &'static str = "https://jarvis.yxswy.com";

    /// 本地开发时 API 的默认来源。
    pub const DEVELOPMENT_API: &'static str = "http://127.0.0.1:3300";

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

    pub fn as_str(&self) -> &str {
        self.0.as_str().trim_end_matches('/')
    }

    /// 是否通过 HTTPS 访问；决定会话 Cookie 是否带 `Secure`。
    pub fn is_secure(&self) -> bool {
        self.0.scheme() == "https"
    }

    /// 把一条以 `/` 开头的路径接到这个来源后面。
    pub fn join(&self, path: &str) -> Url {
        let mut url = self.0.clone();
        url.set_path(path);
        url
    }
}

impl fmt::Display for ApiOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ApiOrigin {
    type Err = OriginError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// 日常网页的基址。可以带路径前缀：正式环境是 `/app/`，本地开发是根路径。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClientUrl(Url);

impl ClientUrl {
    /// 正式环境的日常网页。
    pub const PRODUCTION: &'static str = "https://jarvis.yxswy.com/app/";

    /// 本地开发的日常网页。
    pub const DEVELOPMENT: &'static str = "http://127.0.0.1:5474/";

    /// 解析网页基址。协议规则与 [`ApiOrigin::parse`] 相同，路径必须以 `/` 结尾。
    pub fn parse(value: &str) -> Result<Self, OriginError> {
        let mut url = Url::parse(value).map_err(|_| OriginError::NotAnOrigin)?;
        let clean = url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none();
        if !clean {
            return Err(OriginError::NotAnOrigin);
        }
        check_scheme(&url)?;
        // 没有路径时补上根路径，之后的拼接才不会丢掉前缀。
        if url.path().is_empty() {
            url.set_path("/");
        }
        if !url.path().ends_with('/') {
            return Err(OriginError::NotABase);
        }
        Ok(Self(url))
    }

    /// 网页所在的来源，写请求用它作为 `Origin`。
    pub fn origin(&self) -> ApiOrigin {
        let mut origin = self.0.clone();
        origin.set_path("/");
        ApiOrigin(origin)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// 在基址之后接上一段相对路径。
    pub fn join(&self, relative: &str) -> Url {
        self.0
            .join(relative)
            .expect("a base URL can always join a relative reference")
    }
}

impl fmt::Display for ClientUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OriginError {
    #[error("an origin has no path, query, fragment or credentials")]
    NotAnOrigin,
    #[error("a base URL must end with a slash")]
    NotABase,
    #[error("use an HTTPS origin, or HTTP on loopback for development")]
    NotSecure,
}

fn check_scheme(url: &Url) -> Result<(), OriginError> {
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    match url.scheme() {
        "https" => Ok(()),
        "http" if loopback => Ok(()),
        _ => Err(OriginError::NotSecure),
    }
}

/// 构建时确定的部署参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deployment {
    /// API 所在的来源。
    pub api: ApiOrigin,
    /// 日常网页的基址。登录页从这里打开。
    pub client: ClientUrl,
}

impl Deployment {
    /// 连接正式服务。
    pub fn production() -> Self {
        Self::new(ApiOrigin::PRODUCTION, ClientUrl::PRODUCTION)
            .expect("the production deployment is valid")
    }

    /// 连接本地开发服务。
    pub fn development() -> Self {
        Self::new(ApiOrigin::DEVELOPMENT_API, ClientUrl::DEVELOPMENT)
            .expect("the development deployment is valid")
    }

    pub fn new(api: &str, client: &str) -> Result<Self, OriginError> {
        Ok(Self {
            api: ApiOrigin::parse(api)?,
            client: ClientUrl::parse(client)?,
        })
    }

    /// 登录页的地址：把授权请求编码在路径里。
    pub fn authorization_url(&self, encoded_request: &str) -> Url {
        self.client
            .join(&format!("desktop-login/{encoded_request}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_origin_has_no_path_query_or_credentials() {
        let origin = ApiOrigin::parse("https://jarvis.example:8443/").unwrap();
        assert_eq!(origin.as_str(), "https://jarvis.example:8443");
        assert!(origin.is_secure());
        assert_eq!(
            origin.join("/api/jarvis/capabilities").path(),
            "/api/jarvis/capabilities"
        );

        for rejected in [
            "https://jarvis.example/app",
            "https://jarvis.example?x=1",
            "https://jarvis.example#top",
            "https://user:pass@jarvis.example",
            "not a url",
        ] {
            assert!(ApiOrigin::parse(rejected).is_err(), "{rejected}");
        }
    }

    #[test]
    fn plain_http_is_only_allowed_on_loopback() {
        for allowed in [
            "http://127.0.0.1:3300",
            "http://localhost:3300",
            "http://[::1]:3300",
        ] {
            let origin = ApiOrigin::parse(allowed).unwrap();
            assert!(!origin.is_secure(), "{allowed}");
        }
        for rejected in [
            "http://jarvis.example",
            "http://192.168.1.2:3300",
            "ftp://127.0.0.1",
            "file:///",
        ] {
            assert_eq!(
                ApiOrigin::parse(rejected).unwrap_err(),
                OriginError::NotSecure,
                "{rejected}"
            );
        }
    }

    #[test]
    fn a_client_url_keeps_its_path_prefix() {
        let production = ClientUrl::parse("https://jarvis.example/app/").unwrap();
        assert_eq!(production.origin().as_str(), "https://jarvis.example");
        assert_eq!(
            production.join("desktop-login/abc").as_str(),
            "https://jarvis.example/app/desktop-login/abc"
        );

        let development = ClientUrl::parse("http://127.0.0.1:5474").unwrap();
        assert_eq!(development.as_str(), "http://127.0.0.1:5474/");
        assert_eq!(
            development.join("desktop-login/abc").as_str(),
            "http://127.0.0.1:5474/desktop-login/abc"
        );

        // 路径必须以斜杠结尾，否则拼接时会丢掉最后一段。
        assert_eq!(
            ClientUrl::parse("https://jarvis.example/app").unwrap_err(),
            OriginError::NotABase
        );
        assert_eq!(
            ClientUrl::parse("http://jarvis.example/app/").unwrap_err(),
            OriginError::NotSecure
        );
    }

    #[test]
    fn deployments_point_at_their_own_services() {
        let production = Deployment::production();
        assert_eq!(production.api.as_str(), "https://jarvis.yxswy.com");
        assert_eq!(production.client.as_str(), "https://jarvis.yxswy.com/app/");
        let development = Deployment::development();
        assert_eq!(development.api.as_str(), "http://127.0.0.1:3300");
        assert_eq!(development.client.as_str(), "http://127.0.0.1:5474/");
        assert!(Deployment::new("http://evil.example", ClientUrl::PRODUCTION).is_err());
    }

    #[test]
    fn the_authorization_url_carries_the_request_in_its_path() {
        let production = Deployment::production().authorization_url("eyJabc-_123");
        assert_eq!(
            production.as_str(),
            "https://jarvis.yxswy.com/app/desktop-login/eyJabc-_123"
        );
        let development = Deployment::development().authorization_url("eyJabc-_123");
        assert_eq!(
            development.as_str(),
            "http://127.0.0.1:5474/desktop-login/eyJabc-_123"
        );
    }
}
