//! 令牌与密码：怎么生成、怎么存、怎么核对。
//!
//! 服务端不保存任何明文：令牌存 SHA-256 摘要，密码存 argon2id 散列。
//! argon2 故意很慢，所以散列与核对都放到阻塞线程池里做，不占住异步线程。

use std::sync::LazyLock;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use assemble_store::secret::SecretString;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("a random value could not be generated")]
    Random,
    #[error("the hashing task did not complete")]
    Task,
    #[error("the password could not be hashed: {0}")]
    Hash(argon2::password_hash::Error),
}

/// 一段 32 字节随机数的 base64url：43 个字符，与 PKCE 的形式一致。
pub fn random_token() -> Result<SecretString, SecretError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| SecretError::Random)?;
    Ok(SecretString::new(URL_SAFE_NO_PAD.encode(bytes)))
}

/// 一个 16 字节随机数的十六进制：账户、设备与发送的标识。
pub fn random_id() -> Result<String, SecretError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| SecretError::Random)?;
    Ok(assemble_protocol::primitives::to_hex(&bytes))
}

/// 账户不存在时拿来"陪跑"的散列：让核对所花的时间与真有这个账户时一样，
/// 这样从回答的快慢上看不出一个用户名是否存在。
pub static DECOY_HASH: LazyLock<String> = LazyLock::new(|| {
    Argon2::default()
        .hash_password(b"decoy password for timing only")
        .map(|hash| hash.to_string())
        .expect("hashing a short constant password cannot fail")
});

/// 令牌在库里的样子：SHA-256 小写十六进制。库泄露时也还原不出令牌。
pub fn token_digest(token: &str) -> String {
    assemble_protocol::primitives::to_hex(&Sha256::digest(token.as_bytes()))
}

/// 算出密码的 argon2id 散列（PHC 格式，自带盐与参数）。
pub async fn hash_password(password: SecretString) -> Result<String, SecretError> {
    tokio::task::spawn_blocking(move || {
        // 默认参数：argon2id，m=19 MiB，t=2，p=1。
        Argon2::default()
            .hash_password(password.expose().as_bytes())
            .map(|hash| hash.to_string())
            .map_err(SecretError::Hash)
    })
    .await
    .map_err(|_| SecretError::Task)?
}

/// 核对密码。`stored` 读不懂时当作不匹配，不向上抛：登录失败只有一个结果。
pub async fn verify_password(stored: &str, password: SecretString) -> Result<bool, SecretError> {
    let Ok(parsed) = PasswordHash::new(stored) else {
        tracing::warn!("a stored password hash could not be parsed");
        return Ok(false);
    };
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .verify_password(password.expose().as_bytes(), &parsed)
            .is_ok()
    })
    .await
    .map_err(|_| SecretError::Task)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_and_only_stored_as_digests() {
        let first = random_token().unwrap();
        let second = random_token().unwrap();
        assert!(assemble_protocol::login::is_proof(first.expose()));
        assert_ne!(first.expose(), second.expose());

        let digest = token_digest(first.expose());
        assert!(assemble_protocol::primitives::is_sha256_hex(&digest));
        assert_ne!(digest, first.expose());
        assert_eq!(digest, token_digest(first.expose()));
        assert_eq!(
            digest,
            // SHA-256 的已知性质：同样的输入永远得到同样的摘要。
            assemble_protocol::primitives::to_hex(&Sha256::digest(first.expose().as_bytes()))
        );
    }

    #[test]
    fn ids_are_32_lowercase_hex_characters() {
        let id = random_id().unwrap();
        assert_eq!(id.len(), 32);
        assert!(
            id.bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );
        assert_ne!(id, random_id().unwrap());
        assert!(assemble_protocol::primitives::is_valid_id(&id));
    }

    #[tokio::test]
    async fn password_hashes_verify_the_right_password_and_reject_others() {
        let stored = hash_password(SecretString::new("正确马蹄钉 battery".to_owned()))
            .await
            .unwrap();
        // PHC 格式：算法、参数、盐、摘要。明文不在其中。
        assert!(stored.starts_with("$argon2id$v=19$"), "{stored}");
        assert!(!stored.contains("battery"));
        assert_eq!(stored.matches('$').count(), 5);

        assert!(
            verify_password(&stored, SecretString::new("正确马蹄钉 battery".to_owned()))
                .await
                .unwrap()
        );
        assert!(
            !verify_password(&stored, SecretString::new("正确马蹄钉 batterY".to_owned()))
                .await
                .unwrap()
        );
        assert!(
            !verify_password(&stored, SecretString::new(String::new()))
                .await
                .unwrap()
        );
        // 同一个密码每次散列都不同：盐是随机的。
        let again = hash_password(SecretString::new("正确马蹄钉 battery".to_owned()))
            .await
            .unwrap();
        assert_ne!(stored, again);
    }

    #[tokio::test]
    async fn an_unreadable_stored_hash_never_matches() {
        assert!(
            !verify_password("not-a-phc-hash", SecretString::new("x".to_owned()))
                .await
                .unwrap()
        );
    }
}
