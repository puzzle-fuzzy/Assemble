//! 邮箱身份与单次凭据。HTTP 和 SMTP 不参与规则；消费凭据和账户变更在同一事务里。

use assemble_protocol::primitives::Timestamp;
use assemble_store::secret::SecretString;
use sqlx::SqlitePool;

use crate::accounts::{self, AccountRow};
use crate::secrets::{self, token_digest};
use crate::source::Source;

pub const LIFETIME_MILLIS: i64 = 15 * 60 * 1000;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid email address")]
    Email,
    #[error("the action is expired, used, or no longer matches the account")]
    Invalid,
    #[error("the initiating browser is required")]
    Browser,
    #[error("the email or provider identity is already registered")]
    Conflict,
    #[error("mail rate limit reached")]
    RateLimited,
    #[error("invalid display name or password")]
    Profile,
    #[error("secret operation failed")]
    Secret(#[from] secrets::SecretError),
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
}

/// 一致地不区分邮箱大小写；不删除点、加号或别名，不推测提供商的投递规则。
pub fn normalize_email(raw: &str) -> Result<String, Error> {
    let email = raw.trim();
    let valid = email.len() <= 254
        && email.is_ascii()
        && !email.contains(['\r', '\n', ' ', '<', '>', '"', '\\'])
        && email_address::EmailAddress::is_valid(email)
        && email
            .split_once('@')
            .is_some_and(|(local, domain)| local.len() <= 64 && domain.contains('.'));
    valid
        .then(|| email.to_ascii_lowercase())
        .ok_or(Error::Email)
}

pub async fn by_email(pool: &SqlitePool, email: &str) -> Result<Option<AccountRow>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM accounts WHERE email = ? AND email_verified_at IS NOT NULL")
        .bind(email)
        .fetch_optional(pool)
        .await
}

/// 已发凭据随邮箱或密码变更一起失效，旧密码不能抢先把旧账户绑定到另一邮箱。
pub fn account_proof(account: &AccountRow) -> String {
    token_digest(&format!(
        "{}|{}|{:?}",
        account.password_hash,
        account.email.as_deref().unwrap_or_default(),
        account.email_verified_at
    ))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Action {
    pub token_hash: String,
    pub kind: String,
    pub account_id: Option<String>,
    pub email: String,
    pub previous_email: Option<String>,
    pub proof_hash: Option<String>,
    pub browser_hash: Option<String>,
    pub github_id: Option<String>,
    pub github_login: Option<String>,
    pub display_name: Option<String>,
    pub next: String,
    pub created_at: i64,
    pub expires_at: i64,
}

impl Action {
    pub fn new(kind: &str, email: String, next: String) -> Self {
        let now = Timestamp::now().as_millis();
        Self {
            token_hash: String::new(),
            kind: kind.to_owned(),
            account_id: None,
            email,
            previous_email: None,
            proof_hash: None,
            browser_hash: None,
            github_id: None,
            github_login: None,
            display_name: None,
            next,
            created_at: now,
            expires_at: now + LIFETIME_MILLIS,
        }
    }

    #[must_use]
    pub fn for_account(mut self, account: &AccountRow) -> Self {
        self.account_id = Some(account.id.clone());
        self.previous_email.clone_from(&account.email);
        self.proof_hash = Some(account_proof(account));
        self
    }
}

/// 来源已知时，同一个来源每小时最多申请这么多封：和一个邮箱自己的上限一样。
const MAILS_PER_SOURCE_PER_HOUR: i64 = 5;

/// 每邮箱每分钟一次、每小时五次，服务每小时最多 120 封；来源已知的请求，每个来源
/// 每小时也最多五封。失败请求也占额，不能用它探测账号。
///
/// 全站那一项是所有人共用的。只按邮箱计的话，谁换着邮箱提交一百多次，所有人就都收不到
/// 验证邮件了；所以再按来源分一份，一个来源用不掉别人的份额。来源未知时不设这一项
/// （见 [`crate::source`]）。限额的键只存摘要：库里不留邮箱和地址本身。
pub async fn reserve_mail(pool: &SqlitePool, email: &str, source: &Source) -> Result<(), Error> {
    let now = Timestamp::now().as_millis();
    let key = token_digest(email);
    let mut buckets = vec![
        (format!("minute:{key}"), 60_000, 1),
        (format!("hour:{key}"), 3_600_000, 5),
    ];
    if !source.is_unknown() {
        buckets.push((
            format!("source:{}", token_digest(source.key())),
            3_600_000,
            MAILS_PER_SOURCE_PER_HOUR,
        ));
    }
    buckets.push(("global".to_owned(), 3_600_000, 120));
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM email_rate_limits WHERE started_at < ?")
        .bind(now - 3_600_000)
        .execute(&mut *tx)
        .await?;
    for (bucket, window, limit) in buckets {
        let count: Option<i64> = sqlx::query_scalar(
            "INSERT INTO email_rate_limits (bucket, started_at, requests) VALUES (?, ?, 1) \
             ON CONFLICT(bucket) DO UPDATE SET \
             requests = CASE WHEN started_at <= ? THEN 1 ELSE requests + 1 END, \
             started_at = CASE WHEN started_at <= ? THEN excluded.started_at ELSE started_at END \
             WHERE started_at <= ? OR requests < ? RETURNING requests",
        )
        .bind(bucket)
        .bind(now)
        .bind(now - window)
        .bind(now - window)
        .bind(now - window)
        .bind(limit)
        .fetch_optional(&mut *tx)
        .await?;
        if count.is_none() {
            return Err(Error::RateLimited);
        }
    }
    tx.commit().await?;
    Ok(())
}

pub async fn issue(pool: &SqlitePool, action: &Action) -> Result<SecretString, Error> {
    let token = secrets::random_token()?;
    sqlx::query("INSERT INTO email_actions (token_hash, kind, account_id, email, previous_email, proof_hash, browser_hash, github_id, github_login, display_name, next, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(token_digest(token.expose())).bind(&action.kind).bind(&action.account_id).bind(&action.email)
        .bind(&action.previous_email).bind(&action.proof_hash).bind(&action.browser_hash).bind(&action.github_id).bind(&action.github_login)
        .bind(&action.display_name).bind(&action.next).bind(action.created_at).bind(action.expires_at)
        .execute(pool).await?;
    Ok(token)
}

pub async fn inspect(pool: &SqlitePool, token: &str) -> Result<Action, Error> {
    if token.len() > 128 || token.len() < 32 {
        return Err(Error::Invalid);
    }
    sqlx::query_as("SELECT * FROM email_actions WHERE token_hash = ? AND expires_at > ?")
        .bind(token_digest(token))
        .bind(Timestamp::now().as_millis())
        .fetch_optional(pool)
        .await?
        .ok_or(Error::Invalid)
}

#[derive(Debug)]
pub struct Completed {
    pub account_id: String,
    pub kind: String,
    pub next: String,
    pub followup: Option<Action>,
}

/// 注册只在验信后的这个入口设置密码，从未验证的浏览器提交的密码绝不预存。
pub async fn complete(
    pool: &SqlitePool,
    token: &str,
    password: Option<SecretString>,
    display: &str,
    browser_hash: Option<&str>,
) -> Result<Completed, Error> {
    let preview = inspect(pool, token).await?;
    check_browser(&preview, browser_hash)?;
    let password_hash = prepare_password(&preview.kind, password).await?;
    let mut tx = pool.begin().await?;
    let action: Action = sqlx::query_as(
        "DELETE FROM email_actions WHERE token_hash = ? AND expires_at > ? RETURNING *",
    )
    .bind(token_digest(token))
    .bind(Timestamp::now().as_millis())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(Error::Invalid)?;
    let account = checked_account(&mut tx, &action).await?;
    let now = Timestamp::now().as_millis();
    let id;
    let mut followup = None;
    match action.kind.as_str() {
        "register" | "github" => {
            id = secrets::random_id()?;
            let username = format!("u_{}", &id[..24]);
            let fallback: String = action
                .email
                .split('@')
                .next()
                .ok_or(Error::Email)?
                .chars()
                .take(accounts::DISPLAY_NAME_MAX_CHARS)
                .collect();
            let name = accounts::display_name(
                action.display_name.as_deref().unwrap_or(display),
                &fallback,
            )
            .map_err(|_| Error::Profile)?;
            let inserted = sqlx::query("INSERT INTO accounts (id, username, display_name, password_hash, created_at, email, email_verified_at, password_enabled, avatar_url) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(&id).bind(username).bind(name).bind(password_hash.ok_or(Error::Profile)?).bind(now).bind(&action.email).bind(now)
                .bind(action.kind == "register").bind(action.github_id.as_deref().map(crate::github::avatar_url)).execute(&mut *tx).await;
            unique(inserted)?;
            if let (Some(github_id), Some(login)) = (&action.github_id, &action.github_login) {
                unique(sqlx::query("INSERT INTO github_identities (github_id, account_id, login) VALUES (?, ?, ?)")
                    .bind(github_id).bind(&id).bind(login).execute(&mut *tx).await)?;
            }
        }
        "reset" => {
            id = account.ok_or(Error::Invalid)?.id;
            sqlx::query("UPDATE accounts SET password_hash = ?, password_enabled = 1 WHERE id = ?")
                .bind(password_hash.ok_or(Error::Profile)?)
                .bind(&id)
                .execute(&mut *tx)
                .await?;
        }
        "bind" | "change_new" => {
            let account = account.ok_or(Error::Invalid)?;
            if action.kind == "bind" && account.email.is_some() {
                return Err(Error::Invalid);
            }
            id = account.id;
            unique(
                sqlx::query("UPDATE accounts SET email = ?, email_verified_at = ? WHERE id = ?")
                    .bind(&action.email)
                    .bind(now)
                    .bind(&id)
                    .execute(&mut *tx)
                    .await,
            )?;
        }
        "change_old" => {
            let account = account.ok_or(Error::Invalid)?;
            id = account.id.clone();
            followup = Some(
                Action::new("change_new", action.email.clone(), action.next.clone())
                    .for_account(&account),
            );
        }
        _ => return Err(Error::Invalid),
    }
    if action.kind != "change_old" {
        sqlx::query("DELETE FROM email_actions WHERE account_id = ? OR (email = ? AND kind IN ('register', 'github'))")
            .bind(&id).bind(&action.email).execute(&mut *tx).await?;
        if matches!(action.kind.as_str(), "reset" | "change_new" | "bind") {
            sqlx::query("DELETE FROM web_sessions WHERE account_id = ?")
                .bind(&id)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(Completed {
        account_id: id,
        kind: action.kind,
        next: action.next,
        followup,
    })
}

pub fn check_browser(action: &Action, browser_hash: Option<&str>) -> Result<(), Error> {
    if action.browser_hash.is_some() && action.browser_hash.as_deref() != browser_hash {
        return Err(Error::Browser);
    }
    Ok(())
}

async fn checked_account(
    connection: &mut sqlx::SqliteConnection,
    action: &Action,
) -> Result<Option<AccountRow>, Error> {
    let account = if let Some(id) = &action.account_id {
        let account: AccountRow = sqlx::query_as("SELECT * FROM accounts WHERE id = ?")
            .bind(id)
            .fetch_optional(connection)
            .await?
            .ok_or(Error::Invalid)?;
        if action.proof_hash.as_deref() != Some(account_proof(&account).as_str()) {
            return Err(Error::Invalid);
        }
        Some(account)
    } else {
        None
    };
    Ok(account)
}

async fn prepare_password(
    kind: &str,
    password: Option<SecretString>,
) -> Result<Option<String>, Error> {
    let hash = if matches!(kind, "register" | "reset") {
        let password = password.ok_or(Error::Profile)?;
        accounts::check_password(&password).map_err(|_| Error::Profile)?;
        Some(secrets::hash_password(password).await?)
    } else if kind == "github" {
        // OAuth 账户没有可登录的密码；仍给旧的非空列写入随机、无人知道的散列。
        Some(secrets::hash_password(secrets::random_token()?).await?)
    } else {
        None
    };
    Ok(hash)
}

fn unique(result: Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error>) -> Result<(), Error> {
    match result {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(error)) if error.is_unique_violation() => Err(Error::Conflict),
        Err(error) => Err(error.into()),
    }
}
