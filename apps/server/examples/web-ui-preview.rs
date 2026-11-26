//! 独立回环预览：只用临时库和合成账户，浏览器永远拿不到预览账户的密码。
//! 额外路由仅属于这个示例可执行文件，不进入正式服务；退出后删除整个临时库。

use std::sync::Arc;

use assemble_protocol::device::NewDevice;
use assemble_protocol::login::AuthorizationRequest;
use assemble_protocol::primitives::Platform;
use assemble_server::{
    Config, SecretString, accounts, db, desktop_login, http, identity_templates, templates, web,
};
use axum::{
    Router,
    extract::{Path, Query},
    http::{StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

async fn sign_in(origin: &str, password: &SecretString) -> Result<Response, StatusCode> {
    let client = reqwest::Client::builder()
        .cookie_store(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let html = client
        .get(format!("{origin}/web/login"))
        .send()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .text()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let csrf = html
        .split("name=\"csrf\" value=\"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let response = client
        .post(format!("{origin}/web/login"))
        .header(header::ORIGIN, origin)
        .form(&[
            ("email", "preview@example.test"),
            ("password", password.expose()),
            ("next", "/web/account"),
            ("csrf", csrf),
        ])
        .send()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if response.status() != StatusCode::SEE_OTHER {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let mut redirected = Redirect::to("/web/account").into_response();
    for cookie in response.headers().get_all(header::SET_COOKIE) {
        redirected
            .headers_mut()
            .append(header::SET_COOKIE, cookie.clone());
    }
    redirected
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    Ok(redirected)
}

fn preview_page(Path(kind): Path<String>) -> Response {
    let body = match kind.as_str() {
        "verify-confirm" => identity_templates::verify_confirm(
            &assemble_server::email_identity::Action::new(
                "register",
                "preview@example.test".into(),
                String::new(),
            ),
            "preview-only",
            "preview-only",
            None,
        ),
        "github" => identity_templates::github_profile(
            "preview-only",
            "preview@example.test",
            "预览账户",
            None,
        ),
        "login-github" => templates::login_page(None, "", "preview-only", true),
        "register-github" => {
            identity_templates::request_page("register", None, "", "preview-only", true)
        }
        "verify-email" => identity_templates::verify_confirm(
            &assemble_server::email_identity::Action::new(
                "change_new",
                "preview@example.test".into(),
                String::new(),
            ),
            "preview-only",
            "preview-only",
            None,
        ),
        "success" => templates::message_page("密码已更新", "其他网页登录已退出。"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    web::page(StatusCode::OK, body)
}

async fn callbacks(listener: TcpListener, stop: CancellationToken) {
    let router = Router::new()
        .route(
            "/callback",
            get(
                |Query(values): Query<std::collections::HashMap<String, String>>| async move {
                    let result = if values.get("state").is_some_and(|v| v == &"a".repeat(43))
                        && values
                            .get("code")
                            .is_some_and(|v| assemble_protocol::login::is_proof(v))
                    {
                        "allowed"
                    } else if values
                        .get("state")
                        .is_some_and(|v| v == &"a".repeat(43) || v == &"b".repeat(43))
                        && values.get("error").is_some_and(|v| v == "access_denied")
                    {
                        "denied"
                    } else {
                        return StatusCode::BAD_REQUEST.into_response();
                    };
                    // 状态页不保留授权码，自动化只读取这里的公开结果。
                    Redirect::to(&format!("/done/{result}")).into_response()
                },
            ),
        )
        .route(
            "/done/{result}",
            get(|Path(result): Path<String>| async move {
                match result.as_str() {
                    "allowed" => "allowed".into_response(),
                    "denied" => "denied".into_response(),
                    _ => StatusCode::NOT_FOUND.into_response(),
                }
            }),
        );
    let _ = axum::serve(listener, router)
        .with_graceful_shutdown(stop.cancelled_owned())
        .await;
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let folder = tempfile::tempdir()?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let callback = TcpListener::bind("127.0.0.1:0").await?;
    let redirect_uri = format!("http://{}/callback", callback.local_addr()?);
    let pool = db::open(folder.path()).await?;
    let password = Arc::new(assemble_server::secrets::random_token()?);
    let account = accounts::create(
        &pool,
        "identity-preview",
        "预览账户",
        SecretString::new(password.expose().to_owned()),
    )
    .await?;
    // 只为本进程拥有的临时夹具准备已验证身份，不接触环境变量或任何既有账户。
    sqlx::query("UPDATE accounts SET email = 'preview@example.test', email_verified_at = created_at WHERE id = ?").bind(&account.id).execute(&pool).await?;
    let config = Config::development(folder.path())
        .with_public_origin(assemble_protocol::origin::ServerOrigin::parse(&origin)?);
    let state = Arc::new(http::AppState::new(pool.clone(), config));
    let owned_origin = origin.clone();
    let router = http::router(Arc::clone(&state))
        .route(
            "/__preview/session",
            get(move || {
                let origin = owned_origin.clone();
                let password = Arc::clone(&password);
                async move { sign_in(&origin, &password).await }
            }),
        )
        .route(
            "/__preview/page/{kind}",
            get(|path| async move { preview_page(path) }),
        )
        .route(
            "/__preview/authorize/{choice}",
            get(move |Path(choice): Path<String>| {
                let request = AuthorizationRequest {
                    redirect_uri: redirect_uri.clone(),
                    code_challenge: "c".repeat(43),
                    state: if choice == "allow" { "a" } else { "b" }.repeat(43),
                    device: NewDevice {
                        name: "预览工作电脑".into(),
                        platform: Platform::Windows,
                    },
                };
                async move { Redirect::to(&desktop_login::authorize_path(&request)) }
            }),
        );
    let stop = CancellationToken::new();
    let callback_task = tokio::spawn(callbacks(callback, stop.clone()));
    let stopped = stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(stopped.cancelled_owned())
            .await
    });
    println!("READY {origin}");
    let _ = tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)
    })
    .await?;
    stop.cancel();
    state.hub.close();
    server.await??;
    callback_task.await?;
    pool.close().await;
    println!("CLEANED");
    Ok(())
}
