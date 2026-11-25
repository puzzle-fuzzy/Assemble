//! 只读影视站同源发布；静态构建里没有账号数据和来源凭据。
use crate::http::Shared;
use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use std::path::Path;
use tower::ServiceExt as _;
use tower_http::services::{ServeDir, ServeFile};

pub fn routes(directory: &Path) -> Router<Shared> {
    let index = directory.join("index.html");
    let fallback = tower::service_fn(move |request: Request| {
        let index = index.clone();
        async move {
            if !spa_route(request.uri().path()) {
                return Ok::<_, std::convert::Infallible>(StatusCode::NOT_FOUND.into_response());
            }
            let response = ServeFile::new(index)
                .oneshot(request)
                .await
                .expect("static file services are infallible");
            Ok(response.into_response())
        }
    });
    Router::new()
        .route("/movies", get(|| async { Redirect::permanent("/movies/") }))
        .nest_service("/movies/", ServeDir::new(directory).fallback(fallback))
        .layer(middleware::from_fn(headers))
}
fn spa_route(path: &str) -> bool {
    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    let id = |value: &&str| {
        assemble_protocol::primitives::is_valid_id(value) && !matches!(*value, "." | "..")
    };
    let provider = |value: &&str| matches!(*value, "tmdb" | "douban");
    match parts.as_slice() {
        ["private" | "collections"] => true,
        ["collections", value] | ["entries", "shared" | "private", value] => id(value),
        ["actors", source, value] | ["catalog", source, "movie" | "tv", value] => {
            provider(source) && id(value)
        }
        _ => false,
    }
}
async fn headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "cross-origin-resource-policy",
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(header::CONTENT_SECURITY_POLICY,HeaderValue::from_static("default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self'; img-src 'self' data: https://avatars.githubusercontent.com; connect-src 'self'; base-uri 'self'; object-src 'none'; frame-ancestors 'none'; form-action 'self'"));
    response
}
