//! 账户：创建、查找、改密码，以及用户名与密码的规则。

use assemble_protocol::primitives::Timestamp;
use assemble_store::secret::SecretString;
use sqlx::SqlitePool;

use crate::secrets;

/// 一行账户记录。`password_hash` 只在本模块与登录核对里出现，不对外传。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AccountRow {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub password_hash: String,
    pub created_at: i64,
}

/// 用户名的长度范围。
pub const USERNAME_MIN_CHARS: usize = 3;
pub const USERNAME_MAX_CHARS: usize = 32;
/// 显示名的长度上限（字符）。
pub const DISPLAY_NAME_MAX_CHARS: usize = 60;
/// 密码的最短长度。
pub const PASSWORD_MIN_CHARS: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum CreateAccountError {
    #[error("{0}")]
    Username(&'static str),
    #[error("the display name is empty or too long")]
    DisplayName,
    #[error("the password must be at least {PASSWORD_MIN_CHARS} characters")]
    PasswordTooShort,
    #[error("the password is too long")]
    PasswordTooLong,
    #[error("the username is already taken")]
    Taken,
    #[error("password hashing failed: {0}")]
    Hash(#[from] secrets::SecretError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// 用户名的小写形式，或者一条说明它为什么不合法。
///
/// 只收小写字母、数字、`_`、`.`、`-`，3 到 32 个字符，且以字母或数字开头。
/// 收到的大写字母一律转成小写：用户名不分大小写，库里也只存小写。
pub fn normalize_username(raw: &str) -> Result<String, &'static str> {
    let trimmed = raw.trim();
    if trimmed.chars().any(char::is_whitespace) {
        return Err("用户名里不能有空白");
    }
    let username = trimmed.to_lowercase();
    let count = username.chars().count();
    if !(USERNAME_MIN_CHARS..=USERNAME_MAX_CHARS).contains(&count) {
        return Err("用户名需要 3 到 32 个字符");
    }
    if !username
        .starts_with(|character: char| character.is_ascii_lowercase() || character.is_ascii_digit())
    {
        return Err("用户名要以字母或数字开头");
    }
    if !username.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'.' | b'-')
    }) {
        return Err("用户名只能用小写字母、数字、下划线、点和横线");
    }
    Ok(username)
}

/// 显示名：空的话就用用户名。
pub fn display_name(raw: &str, username: &str) -> Result<String, CreateAccountError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(username.to_owned());
    }
    if trimmed.chars().count() > DISPLAY_NAME_MAX_CHARS || trimmed.chars().any(char::is_control) {
        return Err(CreateAccountError::DisplayName);
    }
    Ok(trimmed.to_owned())
}

/// 一份密码合不合长度规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PasswordRule {
    #[error("the password must be at least {PASSWORD_MIN_CHARS} characters")]
    TooShort,
    #[error("the password is too long")]
    TooLong,
}

/// 上限见 [`crate::PASSWORD_MAX_CHARS`]：正常密码远不到这个数，超出多半是哪里接错了。
pub fn check_password(password: &SecretString) -> Result<(), PasswordRule> {
    let length = password.expose().chars().count();
    if length < PASSWORD_MIN_CHARS {
        return Err(PasswordRule::TooShort);
    }
    if length > crate::PASSWORD_MAX_CHARS {
        return Err(PasswordRule::TooLong);
    }
    Ok(())
}

/// 创建一个账户。用户名已被占用时返回 [`CreateAccountError::Taken`]。
pub async fn create(
    pool: &SqlitePool,
    username: &str,
    display: &str,
    password: SecretString,
) -> Result<AccountRow, CreateAccountError> {
    let username = normalize_username(username).map_err(CreateAccountError::Username)?;
    let display_name = display_name(display, &username)?;
    check_password(&password).map_err(|rule| match rule {
        PasswordRule::TooShort => CreateAccountError::PasswordTooShort,
        PasswordRule::TooLong => CreateAccountError::PasswordTooLong,
    })?;
    let password_hash = secrets::hash_password(password).await?;
    let id = secrets::random_id()?;
    let now = Timestamp::now().as_millis();
    let written = sqlx::query(
        "INSERT INTO accounts (id, username, display_name, password_hash, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&username)
    .bind(&display_name)
    .bind(&password_hash)
    .bind(now)
    .execute(pool)
    .await;
    match written {
        Ok(_) => {}
        // 用户名列上有唯一约束；撞上约束就是重名。
        Err(sqlx::Error::Database(ref error)) if error.is_unique_violation() => {
            return Err(CreateAccountError::Taken);
        }
        Err(error) => return Err(error.into()),
    }
    Ok(AccountRow {
        id,
        username,
        display_name,
        password_hash,
        created_at: now,
    })
}

pub async fn find_by_username(
    pool: &SqlitePool,
    username: &str,
) -> Result<Option<AccountRow>, sqlx::Error> {
    sqlx::query_as::<_, AccountRow>("SELECT * FROM accounts WHERE username = ?")
        .bind(username.to_lowercase())
        .fetch_optional(pool)
        .await
}

pub async fn find_by_id(pool: &SqlitePool, id: &str) -> Result<Option<AccountRow>, sqlx::Error> {
    sqlx::query_as::<_, AccountRow>("SELECT * FROM accounts WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
}

#[derive(Debug, thiserror::Error)]
pub enum ChangePasswordError {
    #[error("the account does not exist")]
    Missing,
    #[error("the current password is wrong")]
    WrongCurrent,
    #[error("the new password is the same as the current one")]
    Unchanged,
    #[error(transparent)]
    Rule(#[from] PasswordRule),
    #[error("password hashing failed: {0}")]
    Hash(#[from] secrets::SecretError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// 用户自己改密码：先核对现在的密码，再换成新的。新的不能和现在的一样。
///
/// 核对与替换分两步，中间没有锁：同一个账户的两个人同时改密码，后到的那个会赢，
/// 但两人都得先拿对当时的密码才行——不会有人在不了解旧密码的情况下把它改掉。
pub async fn change_password(
    pool: &SqlitePool,
    account_id: &str,
    current: SecretString,
    new: SecretString,
) -> Result<(), ChangePasswordError> {
    let account = find_by_id(pool, account_id)
        .await?
        .ok_or(ChangePasswordError::Missing)?;
    // 两个都是这次请求里刚填的明文，直接比就行。
    if current.expose() == new.expose() {
        return Err(ChangePasswordError::Unchanged);
    }
    if !secrets::verify_password(&account.password_hash, current).await? {
        return Err(ChangePasswordError::WrongCurrent);
    }
    check_password(&new)?;
    replace_password(pool, &account, new).await
}

/// 命令行上给一个账户换密码（忘了密码的时候用）。不核对旧的，所以不该留给网页用。
pub async fn set_password(
    pool: &SqlitePool,
    account_id: &str,
    new: SecretString,
) -> Result<(), ChangePasswordError> {
    let account = find_by_id(pool, account_id)
        .await?
        .ok_or(ChangePasswordError::Missing)?;
    check_password(&new)?;
    replace_password(pool, &account, new).await
}

async fn replace_password(
    pool: &SqlitePool,
    account: &AccountRow,
    new: SecretString,
) -> Result<(), ChangePasswordError> {
    // "新的不能和现在的一样"只在用户自己改的时候拦（那里知道现在的明文）；
    // 命令行找回不知道旧的，不去拦。
    let password_hash = secrets::hash_password(new).await?;
    sqlx::query("UPDATE accounts SET password_hash = ? WHERE id = ?")
        .bind(password_hash)
        .bind(&account.id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 撤掉这个账户的网页登录状态，交出撤掉了几条。`keep` 是要留着的那一条（正在用的这次登录）。
pub async fn revoke_web_sessions(
    pool: &SqlitePool,
    account_id: &str,
    keep: Option<&str>,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM web_sessions \
         WHERE account_id = ? AND (? IS NULL OR id != ?)",
    )
    .bind(account_id)
    .bind(keep)
    .bind(keep)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// 全部账户的用户名，按建立的先后。只有用户名：密码散列没有理由离开这个模块。
pub async fn list_usernames(pool: &SqlitePool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT username FROM accounts ORDER BY rowid")
        .fetch_all(pool)
        .await
}

impl AccountRow {
    /// 交给接口的公开信息。
    pub fn public(&self) -> assemble_protocol::account::Account {
        assemble_protocol::account::Account {
            id: self.id.clone(),
            username: self.username.clone(),
            display_name: self.display_name.clone(),
            created_at: Timestamp::from_millis(self.created_at).unwrap_or_else(Timestamp::now),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn password() -> SecretString {
        SecretString::new("a-long-enough-password".to_owned())
    }

    #[test]
    fn usernames_are_lowercased_and_restricted() {
        assert_eq!(normalize_username("Alex.01_x-y").unwrap(), "alex.01_x-y");
        assert_eq!(normalize_username("  alex  ").unwrap(), "alex");
        assert_eq!(normalize_username(&"a".repeat(32)).unwrap(), "a".repeat(32));
        for (raw, reason) in [
            ("ab", "用户名需要 3 到 32 个字符"),
            ("abc def", "用户名里不能有空白"),
            ("_alex", "用户名要以字母或数字开头"),
            ("-alex", "用户名要以字母或数字开头"),
            (".alex", "用户名要以字母或数字开头"),
            ("中文用户名", "用户名要以字母或数字开头"),
            ("ale/x", "用户名只能用小写字母、数字、下划线、点和横线"),
            ("alex!", "用户名只能用小写字母、数字、下划线、点和横线"),
            ("", "用户名需要 3 到 32 个字符"),
        ] {
            assert_eq!(normalize_username(raw).unwrap_err(), reason, "{raw:?}");
        }
        assert!(normalize_username(&"a".repeat(33)).is_err());
    }

    #[test]
    fn the_display_name_falls_back_to_the_username_and_is_bounded() {
        assert_eq!(display_name("", "alex").unwrap(), "alex");
        assert_eq!(display_name("  ", "alex").unwrap(), "alex");
        assert_eq!(display_name(" Alex ", "alex").unwrap(), "Alex");
        assert_eq!(display_name(&"名".repeat(60), "alex").unwrap().len(), 180);
        assert!(matches!(
            display_name(&"名".repeat(61), "alex"),
            Err(CreateAccountError::DisplayName)
        ));
        assert!(matches!(
            display_name("A\nB", "alex"),
            Err(CreateAccountError::DisplayName)
        ));
    }

    #[tokio::test]
    async fn accounts_can_be_created_once_and_found_either_way() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();

        let account = create(&pool, "Alex", "Alex Wang", password())
            .await
            .unwrap();
        assert_eq!(account.username, "alex");
        assert_eq!(account.display_name, "Alex Wang");
        assert!(account.password_hash.starts_with("$argon2id$"));
        let public = account.public();
        assert_eq!(public.username, "alex");
        assert!(public.created_at.as_millis() > 1_700_000_000_000);

        // 大小写不同的同名账户就是同一个账户。
        assert!(matches!(
            create(&pool, "ALEX", "", password()).await,
            Err(CreateAccountError::Taken)
        ));
        // 没填显示名就用用户名。
        assert_eq!(
            create(&pool, "sam", "", password())
                .await
                .unwrap()
                .display_name,
            "sam"
        );

        let found = find_by_username(&pool, "AlEx").await.unwrap().unwrap();
        assert_eq!(found.id, account.id);
        assert!(find_by_username(&pool, "nobody").await.unwrap().is_none());
        assert_eq!(
            find_by_id(&pool, &account.id).await.unwrap().unwrap().id,
            account.id
        );
        assert!(find_by_id(&pool, "missing").await.unwrap().is_none());

        // 清单按建立的先后，一个账户一行。
        assert_eq!(list_usernames(&pool).await.unwrap(), ["alex", "sam"]);
    }

    #[tokio::test]
    async fn passwords_have_a_floor_and_a_ceiling() {
        let long = |count: usize| SecretString::new("x".repeat(count));
        assert_eq!(
            check_password(&SecretString::new("short".to_owned())),
            Err(PasswordRule::TooShort)
        );
        assert_eq!(check_password(&long(PASSWORD_MIN_CHARS)), Ok(()));
        assert_eq!(check_password(&long(crate::PASSWORD_MAX_CHARS)), Ok(()));
        assert_eq!(
            check_password(&long(crate::PASSWORD_MAX_CHARS + 1)),
            Err(PasswordRule::TooLong)
        );
        // 按字符数，不按字节。
        assert_eq!(
            check_password(&SecretString::new("密".repeat(PASSWORD_MIN_CHARS))),
            Ok(())
        );
    }

    #[tokio::test]
    async fn the_password_changes_only_for_someone_who_knows_the_current_one() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        let account = create(&pool, "alex", "", password()).await.unwrap();
        let wrong = SecretString::new("not-the-current-one".to_owned());
        let new = SecretString::new("a-brand-new-password".to_owned());

        assert!(matches!(
            change_password(&pool, &account.id, wrong, new.clone()).await,
            Err(ChangePasswordError::WrongCurrent)
        ));
        // 现在的密码不对：什么都没动，原来的密码还能用。
        let stored = find_by_id(&pool, &account.id).await.unwrap().unwrap();
        assert_eq!(stored.password_hash, account.password_hash);
        assert!(
            secrets::verify_password(&stored.password_hash, password())
                .await
                .unwrap()
        );

        assert!(matches!(
            change_password(&pool, &account.id, password(), password()).await,
            Err(ChangePasswordError::Unchanged)
        ));
        assert!(matches!(
            change_password(
                &pool,
                &account.id,
                password(),
                SecretString::new("short".to_owned())
            )
            .await,
            Err(ChangePasswordError::Rule(PasswordRule::TooShort))
        ));

        change_password(&pool, &account.id, password(), new.clone())
            .await
            .unwrap();
        let stored = find_by_id(&pool, &account.id).await.unwrap().unwrap();
        assert!(
            !secrets::verify_password(&stored.password_hash, password())
                .await
                .unwrap()
        );
        assert!(
            secrets::verify_password(&stored.password_hash, new)
                .await
                .unwrap()
        );

        // 命令行找回：不核对旧的。
        let again = SecretString::new("yet-another-password".to_owned());
        set_password(&pool, &account.id, again.clone())
            .await
            .unwrap();
        let stored = find_by_id(&pool, &account.id).await.unwrap().unwrap();
        assert!(
            secrets::verify_password(&stored.password_hash, again)
                .await
                .unwrap()
        );
        assert!(matches!(
            set_password(&pool, "missing", password()).await,
            Err(ChangePasswordError::Missing)
        ));

        // 网页登录状态：留着一条，撤掉其余的。
        let session = |id: &str| {
            sqlx::query(
                "INSERT INTO web_sessions (id, account_id, token_hash, csrf_token, created_at, expires_at) \
                 VALUES (?, ?, ?, 'c', 1, 9223372036854775807)",
            )
            .bind(id)
            .bind(&account.id)
            .bind(id)
        };
        session("s1").execute(&pool).await.unwrap();
        session("s2").execute(&pool).await.unwrap();
        assert_eq!(
            revoke_web_sessions(&pool, &account.id, Some("s1"))
                .await
                .unwrap(),
            1
        );
        let left: Vec<String> = sqlx::query_scalar("SELECT id FROM web_sessions")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(left, ["s1"]);
        assert_eq!(
            revoke_web_sessions(&pool, &account.id, None).await.unwrap(),
            1
        );
        assert_eq!(
            revoke_web_sessions(&pool, &account.id, None).await.unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn unusable_input_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();

        assert!(matches!(
            create(&pool, "x", "", password()).await,
            Err(CreateAccountError::Username(_))
        ));
        assert!(matches!(
            create(&pool, "alex", "\u{7}", password()).await,
            Err(CreateAccountError::DisplayName)
        ));
        assert!(matches!(
            create(&pool, "alex", "", SecretString::new("short".to_owned())).await,
            Err(CreateAccountError::PasswordTooShort)
        ));
        let accounts: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(accounts, 0);
    }
}
