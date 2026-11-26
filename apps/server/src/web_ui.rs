//! 身份页使用共享 shadcn 的构建时输出；资产随二进制内嵌，独立启动 Rust 服务也不会缺样式。

use std::collections::HashMap;
use std::sync::LazyLock;

use axum::{
    Router,
    extract::Path,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use sha2::{Digest, Sha256};

use crate::http::Shared;

pub(crate) const STYLE: &str = include_str!("web_ui/generated/server.css");
static ATTRIBUTES: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("web_ui/generated/attributes.json"))
        .expect("bundled server UI attributes must be valid")
});
static MARKUP: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("web_ui/generated/markup.json"))
        .expect("bundled server UI markup must be valid")
});

/// 仅返回构建时由共享组件生成的固定结构，不接受来自请求的 HTML。
pub(crate) fn markup(component: &str) -> &'static str {
    MARKUP
        .get(component)
        .expect("server UI markup must be exported")
}

pub(crate) fn attrs(component: &str) -> &'static str {
    ATTRIBUTES
        .get(component)
        .expect("server UI component must be exported")
}

const FONTS: &[(&str, &[u8])] = &[
    ("OFL.txt", include_bytes!("web_ui/generated/OFL.txt")),
    (
        "geist-latin-wght-normal.woff2",
        include_bytes!("web_ui/generated/geist-latin-wght-normal.woff2"),
    ),
    (
        "geist-latin-ext-wght-normal.woff2",
        include_bytes!("web_ui/generated/geist-latin-ext-wght-normal.woff2"),
    ),
    (
        "geist-cyrillic-wght-normal.woff2",
        include_bytes!("web_ui/generated/geist-cyrillic-wght-normal.woff2"),
    ),
    (
        "geist-cyrillic-ext-wght-normal.woff2",
        include_bytes!("web_ui/generated/geist-cyrillic-ext-wght-normal.woff2"),
    ),
    (
        "geist-vietnamese-wght-normal.woff2",
        include_bytes!("web_ui/generated/geist-vietnamese-wght-normal.woff2"),
    ),
];

pub(crate) fn routes() -> Router<Shared> {
    Router::new().route("/web/ui/{name}", get(font))
}

async fn font(Path(name): Path<String>, headers: HeaderMap) -> Response {
    let Some((_, bytes)) = FONTS.iter().find(|(file, _)| *file == name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let etag = format!(
        "\"{}\"",
        assemble_protocol::primitives::to_hex(&Sha256::digest(bytes))
    );
    let cached = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        == Some(etag.as_str());
    let mut response = if cached {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        (
            [(
                header::CONTENT_TYPE,
                if name == "OFL.txt" {
                    "text/plain; charset=utf-8"
                } else {
                    "font/woff2"
                },
            )],
            *bytes,
        )
            .into_response()
    };
    let output = response.headers_mut();
    output.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, no-cache"),
    );
    output.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    output.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    if let Ok(etag) = HeaderValue::from_str(&etag) {
        output.insert(header::ETAG, etag);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    #[tokio::test]
    async fn fonts_are_embedded_scoped_and_revalidated_without_user_data() {
        let router = Router::new().route("/{name}", get(font));
        let response = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/geist-latin-wght-normal.woff2")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "font/woff2");
        assert_eq!(
            response.headers()["cross-origin-resource-policy"],
            "same-origin"
        );
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        let etag = response.headers()[header::ETAG].clone();
        let response = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/geist-latin-wght-normal.woff2")
                    .header(header::IF_NONE_MATCH, etag)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .uri("/Cargo.toml")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
