//! 只发布管理端的静态构建；账户数据仍由逐次核验权限的管理 API 提供。

use std::path::Path;

use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderValue, header};
use axum::middleware::{self, Next};
use axum::response::{Redirect, Response};
use axum::routing::get;
use tower_http::services::ServeDir;

use crate::http::Shared;

pub fn routes(directory: &Path) -> Router<Shared> {
    Router::new()
        .route("/admin", get(|| async { Redirect::permanent("/admin/") }))
        .nest_service("/admin/", ServeDir::new(directory))
        .layer(middleware::from_fn(headers))
}

async fn headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    // HTML 与权限变更一起即时生效；不让浏览器留下管理页面的离线副本。
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
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(
        "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self'; img-src 'self' data: https://avatars.githubusercontent.com; connect-src 'self'; base-uri 'self'; object-src 'none'; frame-ancestors 'none'; form-action 'self'"
    ));
    response
}
