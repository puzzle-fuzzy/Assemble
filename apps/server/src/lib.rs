//! Assemble 服务端的库部分：配置、数据库、路由与处理器都在这里。
//! `main.rs` 只负责解析命令与初始化日志，这样测试可以直接驱动所有逻辑。
//!
//! 业务规则尽量写成不依赖 HTTP 的函数，再由处理器做薄薄的一层适配。

// 测试代码以可读性优先，不套用库代码的严格检查。
#![cfg_attr(test, allow(clippy::pedantic))]

pub mod accounts;
pub mod audit;
pub mod auth;
pub mod blobs;
pub mod config;
pub mod db;
pub mod desktop_login;
pub mod devices;
pub mod drive;
pub mod error;
pub mod events;
pub mod http;
pub mod maintenance;
pub mod planner;
pub mod presence;
pub mod secrets;
pub mod sends;
pub mod templates;
pub mod web;

use std::io::BufRead;
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

pub use assemble_store::secret::SecretString;
pub use config::Config;

#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("configuration could not be read: {0}")]
    Config(#[from] config::ConfigError),
    #[error("database is unavailable: {0}")]
    Database(#[from] db::DbError),
    #[error("{0}")]
    Serve(#[from] http::ServeError),
}

/// 读取配置并打开数据库。所有子命令共用的准备步骤。
pub async fn load() -> Result<(Config, SqlitePool), StartupError> {
    let config = Config::from_env()?;
    let pool = db::open(&config.data_dir).await?;
    Ok((config, pool))
}

/// `serve`：一直运行到收到退出信号。
pub async fn run_server() -> Result<(), StartupError> {
    let (config, pool) = load().await?;
    tracing::info!(
        address = %config.addr,
        origin = %config.public_origin,
        "starting Assemble server"
    );
    // 重启之后没有任何设备连着：先把它们都记成"离线到现在"。
    devices::mark_all_offline(&pool)
        .await
        .map_err(db::DbError::from)?;
    let state = std::sync::Arc::new(http::AppState::new(pool, config));
    let stop = CancellationToken::new();
    watch_for_shutdown(stop.clone());
    // 先把上次运行留下的尾巴处理掉，再开始接客；之后每隔一分钟清理一次。
    if let Err(error) = maintenance::run_once(&state).await {
        tracing::warn!(%error, "the first maintenance round failed; it will be retried");
    }
    maintenance::start(Arc::clone(&state), stop.clone());
    // 看一眼到点的日程与提醒。第一轮在一个间隔之后：停机期间到点的提醒那时补发。
    planner::tick::start(Arc::clone(&state), stop.clone());
    http::serve((*state.config).clone(), state, stop).await?;
    tracing::info!("server stopped");
    Ok(())
}

/// 收到 Ctrl+C（或 `SIGTERM`）时触发停止。
fn watch_for_shutdown(stop: CancellationToken) {
    tokio::spawn(async move {
        wait_for_signal().await;
        stop.cancel();
    });
}

/// 等到进程被要求退出。信号监听装不上时不返回：总不能因为收不到信号就误停。
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        let terminate = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut stream) => {
                    stream.recv().await;
                }
                Err(error) => {
                    tracing::warn!(%error, "SIGTERM cannot be watched");
                    std::future::pending::<()>().await;
                }
            }
        };
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(error) = result {
                    tracing::warn!(%error, "Ctrl+C cannot be watched");
                    std::future::pending::<()>().await;
                }
            }
            () = terminate => {}
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "Ctrl+C cannot be watched");
            std::future::pending::<()>().await;
        }
    }
}

/// `check-config`：读取配置、连接数据库并迁移，然后给出一份报告。
pub async fn check_config() -> Result<String, StartupError> {
    let (config, pool) = load().await?;
    let accounts: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts")
        .fetch_one(&pool)
        .await
        .map_err(db::DbError::from)?;
    pool.close().await;
    Ok(format!(
        "配置可用。\n  监听地址：{}\n  数据目录：{}\n  对外地址：{}\n  云盘上限：{} 字节\n  账户数：{}",
        config.addr,
        config.data_dir.display(),
        config.public_origin,
        config.drive_quota_bytes,
        accounts
    ))
}

/// `list-accounts`：列出全部账户的用户名，每行一个。不开放注册，账户有哪些只能从这里看。
pub async fn list_accounts() -> Result<Vec<String>, StartupError> {
    let (_config, pool) = load().await?;
    let usernames = accounts::list_usernames(&pool)
        .await
        .map_err(db::DbError::from)?;
    pool.close().await;
    Ok(usernames)
}

#[derive(Debug, thiserror::Error)]
pub enum CreateAccountError {
    #[error(transparent)]
    Startup(#[from] StartupError),
    #[error(transparent)]
    Password(#[from] PasswordError),
    #[error(transparent)]
    Account(#[from] accounts::CreateAccountError),
}

/// `create-account`：建一个账户。密码从终端或标准输入读，绝不作为参数传入。
pub async fn create_account(
    username: &str,
    display_name: Option<&str>,
    password: SecretString,
) -> Result<accounts::AccountRow, CreateAccountError> {
    // 用户名先校验再连库、再读密码：拼错了不该让人白输一遍密码。
    let username =
        accounts::normalize_username(username).map_err(accounts::CreateAccountError::Username)?;
    let (_config, pool) = load().await?;
    let account =
        accounts::create(&pool, &username, display_name.unwrap_or_default(), password).await?;
    audit::record(
        &pool,
        Some(&account.id),
        None,
        audit::actions::ACCOUNT_CREATED,
        None,
    )
    .await;
    pool.close().await;
    Ok(account)
}

#[derive(Debug, thiserror::Error)]
pub enum PasswordError {
    #[error("the password could not be read: {0}")]
    Io(std::io::Error),
    #[error("the two passwords did not match")]
    Mismatch,
    #[error("no password was provided on standard input")]
    Missing,
    #[error("the password is too long")]
    TooLong,
}

/// 密码的长度上限（字符）。正常密码远不到这个数，超出多半是管道接错了。
pub const PASSWORD_MAX_CHARS: usize = 1024;

/// 从终端读两遍密码（不回显），两遍必须一致。
pub fn prompt_password(
    first: impl FnOnce() -> std::io::Result<String>,
    second: impl FnOnce() -> std::io::Result<String>,
) -> Result<SecretString, PasswordError> {
    let first = first().map_err(PasswordError::Io)?;
    let second = second().map_err(PasswordError::Io)?;
    if first != second {
        return Err(PasswordError::Mismatch);
    }
    checked(first)
}

/// 从一行输入读密码（脚本或管道）。只取第一行，行尾的换行符不算密码的一部分。
pub fn piped_password(input: &mut impl BufRead) -> Result<SecretString, PasswordError> {
    let mut line = String::new();
    input.read_line(&mut line).map_err(PasswordError::Io)?;
    // 只去掉行尾的换行：密码前后的空格是有意输入的。
    let line = line.strip_suffix('\n').unwrap_or(&line);
    let line = line.strip_suffix('\r').unwrap_or(line);
    checked(line.to_owned())
}

fn checked(password: String) -> Result<SecretString, PasswordError> {
    if password.is_empty() {
        return Err(PasswordError::Missing);
    }
    if password.chars().count() > PASSWORD_MAX_CHARS {
        return Err(PasswordError::TooLong);
    }
    Ok(SecretString::new(password))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_piped_password_is_one_line_without_its_line_ending() {
        let mut input = &b"correct horse\nignored second line\n"[..];
        assert_eq!(
            piped_password(&mut input).unwrap().expose(),
            "correct horse"
        );
        let mut crlf = &b"with-crlf\r\n"[..];
        assert_eq!(piped_password(&mut crlf).unwrap().expose(), "with-crlf");
        // 行尾的空格是密码的一部分。
        let mut padded = &b"padded \n"[..];
        assert_eq!(piped_password(&mut padded).unwrap().expose(), "padded ");
        let mut blank = &b"\n"[..];
        assert!(matches!(
            piped_password(&mut blank),
            Err(PasswordError::Missing)
        ));
        let mut empty = &b""[..];
        assert!(matches!(
            piped_password(&mut empty),
            Err(PasswordError::Missing)
        ));
        let huge = format!("{}\n", "x".repeat(PASSWORD_MAX_CHARS + 1)).into_bytes();
        let mut reader = huge.as_slice();
        assert!(matches!(
            piped_password(&mut reader),
            Err(PasswordError::TooLong)
        ));
    }

    #[test]
    fn the_interactive_prompt_requires_two_matching_entries() {
        let password = prompt_password(|| Ok("same".into()), || Ok("same".into())).unwrap();
        assert_eq!(password.expose(), "same");
        assert!(matches!(
            prompt_password(|| Ok("one".into()), || Ok("two".into())),
            Err(PasswordError::Mismatch)
        ));
        assert!(matches!(
            prompt_password(|| Ok("x".into()), || Err(std::io::Error::other("closed"))),
            Err(PasswordError::Io(_))
        ));
        assert!(matches!(
            prompt_password(|| Ok(String::new()), || Ok(String::new())),
            Err(PasswordError::Missing)
        ));
    }
}
