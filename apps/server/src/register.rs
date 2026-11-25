//! 公开注册；是否已配 SMTP 不改变注册规则，没有邀请码和注册开关。

use crate::http::Shared;
use axum::routing::get;

pub const REGISTER_PATH: &str = "/web/register";

pub fn routes() -> axum::Router<Shared> {
    axum::Router::new().route(
        REGISTER_PATH,
        get(crate::identity_web::register_form).post(crate::identity_web::register_submit),
    )
}
