//! 密钥存储：凭据只进系统钥匙串。
//!
//! 界面层、日志和 RPC 结果都拿不到凭据。明文只在 [`SecretString`] 里
//! 短暂存在，释放时清零；调试输出永远只显示占位符。

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use zeroize::Zeroizing;

/// 密钥存储不可用。错误里刻意不带系统诊断，避免泄露钥匙串条目或账户名。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    /// 系统钥匙串无法初始化，或本平台没有可用实现。此时必须拒绝工作，不能退回明文。
    #[error("the system credential store is unavailable")]
    Unavailable,
    /// 用户拒绝了钥匙串访问，或应用签名变化后系统不再放行。
    #[error("access to the system credential store was denied")]
    Denied,
    /// 其它失败：条目不存在却要删除以外的所有情况，例如值无法解码。
    #[error("the system credential store rejected the request")]
    Rejected,
}

pub type Result<T> = std::result::Result<T, SecretError>;

/// 钥匙串里的一条凭据：`service` 标识用途，`account` 在用途内区分来源与账户。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SecretKey {
    pub service: String,
    pub account: String,
}

impl SecretKey {
    pub fn new(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            account: account.into(),
        }
    }
}

/// 一段明文凭据。只能经 [`SecretString::expose`] 读取，释放时清零。
///
/// 克隆会多出一份同样会被清零的副本，只在确实需要两个持有方时使用。
#[derive(Clone)]
pub struct SecretString(Zeroizing<String>);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// 借出明文。只用于立刻发往系统钥匙串或网络，不要复制或记录。
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString([redacted])")
    }
}

/// 一个可插拔的密钥存储。正式运行用系统钥匙串，测试用内存实现。
pub trait SecretStore: Send + Sync {
    fn put(&self, key: &SecretKey, value: &SecretString) -> Result<()>;
    /// 条目不存在时返回 `None`。
    fn get(&self, key: &SecretKey) -> Result<Option<SecretString>>;
    /// 删除不存在的条目不算错误。
    fn delete(&self, key: &SecretKey) -> Result<()>;
}

/// 系统钥匙串。只在 macOS 与 Windows 上存在。
#[cfg(all(feature = "keyring", any(target_os = "macos", target_os = "windows")))]
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemSecretStore;

#[cfg(all(feature = "keyring", any(target_os = "macos", target_os = "windows")))]
impl SecretStore for SystemSecretStore {
    fn put(&self, key: &SecretKey, value: &SecretString) -> Result<()> {
        keyring::Entry::new(&key.service, &key.account)
            .and_then(|entry| entry.set_password(value.expose()))
            .map_err(|error| map_error(&error))
    }

    fn get(&self, key: &SecretKey) -> Result<Option<SecretString>> {
        match keyring::Entry::new(&key.service, &key.account).and_then(|entry| entry.get_password())
        {
            Ok(value) => Ok(Some(SecretString::new(value))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(map_error(&error)),
        }
    }

    fn delete(&self, key: &SecretKey) -> Result<()> {
        match keyring::Entry::new(&key.service, &key.account)
            .and_then(|entry| entry.delete_credential())
        {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(map_error(&error)),
        }
    }
}

/// 当前平台可用的系统密钥存储；没有可用实现时返回 `None`。
#[cfg(all(feature = "keyring", any(target_os = "macos", target_os = "windows")))]
pub fn system_secret_store() -> Option<SystemSecretStore> {
    match keyring::Entry::store_status() {
        Ok(()) => Some(SystemSecretStore),
        Err(_) => None,
    }
}

#[cfg(not(all(feature = "keyring", any(target_os = "macos", target_os = "windows"))))]
pub fn system_secret_store() -> Option<SystemSecretStore> {
    None
}

#[cfg(not(all(feature = "keyring", any(target_os = "macos", target_os = "windows"))))]
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemSecretStore;

// 把钥匙串的具体错误收敛成三类，不把系统诊断带出去。
#[cfg(all(feature = "keyring", any(target_os = "macos", target_os = "windows")))]
fn map_error(error: &keyring::Error) -> SecretError {
    match error {
        // 钥匙串被锁定、访问被拒，或用户在系统弹窗里取消。
        keyring::Error::NoStorageAccess(_) | keyring::Error::PlatformFailure(_) => {
            SecretError::Denied
        }
        // 没有可用的凭据后端。
        keyring::Error::NoDefaultStore | keyring::Error::NotSupportedByStore(_) => {
            SecretError::Unavailable
        }
        _ => SecretError::Rejected,
    }
}

/// 只存在于内存里的密钥存储。用于测试，以及明确指定不落盘的场景。
#[derive(Debug, Default)]
pub struct MemorySecretStore {
    entries: Mutex<HashMap<SecretKey, SecretString>>,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn with_entries<T>(
        &self,
        action: impl FnOnce(&mut HashMap<SecretKey, SecretString>) -> T,
    ) -> Result<T> {
        let mut entries = self.entries.lock().map_err(|_| SecretError::Unavailable)?;
        Ok(action(&mut entries))
    }
}

impl SecretStore for MemorySecretStore {
    fn put(&self, key: &SecretKey, value: &SecretString) -> Result<()> {
        self.with_entries(|entries| {
            entries.insert(key.clone(), SecretString::new(value.expose()));
        })
    }

    fn get(&self, key: &SecretKey) -> Result<Option<SecretString>> {
        self.with_entries(|entries| {
            entries
                .get(key)
                .map(|value| SecretString::new(value.expose()))
        })
    }

    fn delete(&self, key: &SecretKey) -> Result<()> {
        self.with_entries(|entries| {
            entries.remove(key);
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn exercise(store: &dyn SecretStore) {
        let key = SecretKey::new("assemble.session:https://jarvis.example", "account-1");

        assert!(store.get(&key).unwrap().is_none());
        store.delete(&key).unwrap();

        store.put(&key, &SecretString::new("cookie:abc")).unwrap();
        let loaded = store.get(&key).unwrap().expect("the entry should exist");
        assert_eq!(loaded.expose(), "cookie:abc");

        // 同一用途下换个账户就是另一条凭据。
        let other = SecretKey::new("assemble.session:https://jarvis.example", "account-2");
        assert!(store.get(&other).unwrap().is_none());

        store
            .put(&key, &SecretString::new("cookie:rotated"))
            .unwrap();
        assert_eq!(store.get(&key).unwrap().unwrap().expose(), "cookie:rotated");

        store.delete(&key).unwrap();
        assert!(store.get(&key).unwrap().is_none());
    }

    #[test]
    fn memory_store_round_trips_and_isolates_keys() {
        exercise(&MemorySecretStore::new());
    }

    #[test]
    fn secrets_are_redacted_in_debug_output_and_report_emptiness() {
        let secret = SecretString::new("bearer:very-secret");
        assert_eq!(format!("{secret:?}"), "SecretString([redacted])");
        assert!(!format!("{secret:?}").contains("very-secret"));
        assert!(!secret.is_empty());
        assert!(SecretString::new("").is_empty());
        let store = MemorySecretStore::new();
        store.put(&SecretKey::new("s", "a"), &secret).unwrap();
        assert!(!format!("{store:?}").contains("very-secret"));
    }

    #[test]
    fn a_store_is_shared_safely_across_threads() {
        let store = Arc::new(MemorySecretStore::new());
        let workers: Vec<_> = (0..8)
            .map(|worker| {
                let store = Arc::clone(&store);
                std::thread::spawn(move || {
                    let key = SecretKey::new("assemble.test", worker.to_string());
                    store
                        .put(&key, &SecretString::new(format!("value-{worker}")))
                        .unwrap();
                    assert_eq!(
                        store.get(&key).unwrap().unwrap().expose(),
                        format!("value-{worker}")
                    );
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        for worker in 0..8 {
            let key = SecretKey::new("assemble.test", worker.to_string());
            assert!(store.get(&key).unwrap().is_some());
        }
    }

    #[test]
    fn error_messages_carry_no_system_detail() {
        for error in [
            SecretError::Unavailable,
            SecretError::Denied,
            SecretError::Rejected,
        ] {
            let text = error.to_string();
            assert!(!text.is_empty());
            assert_eq!(text, text.to_ascii_lowercase());
        }
        assert_ne!(SecretError::Unavailable, SecretError::Denied);
    }

    // 系统钥匙串会弹出授权提示并写入用户真实的钥匙串，自动测试不触碰它；
    // `SystemSecretStore` 的行为只在真机验收中确认。
}
