//! Windows 凭据项容量小，来源按 UTF-8 边界分段；普通历史从不保存来源或恢复密钥。
use crate::{DownloadError, Result};
use assemble_store::secret::{SecretKey, SecretStore, SecretString};

const PARTS: usize = 16;
fn part(id: &str, index: usize) -> SecretKey {
    super::key(&format!("{id}.{index}"))
}

pub(super) fn put(store: &dyn SecretStore, id: &str, value: &SecretString) -> Result<()> {
    let mut remaining = value.expose();
    let result = (|| {
        let mut count = 0;
        while !remaining.is_empty() {
            if count >= PARTS {
                return Err(DownloadError::Limit);
            }
            let mut end = remaining.len().min(1024);
            while !remaining.is_char_boundary(end) {
                end -= 1;
            }
            store
                .put(&part(id, count), &SecretString::new(&remaining[..end]))
                .map_err(|_| DownloadError::CredentialsUnavailable)?;
            remaining = &remaining[end..];
            count += 1;
        }
        store
            .put(
                &super::key(id),
                &SecretString::new(format!("parts:{count}")),
            )
            .map_err(|_| DownloadError::CredentialsUnavailable)
    })();
    if result.is_err() {
        let _ = delete(store, id);
    }
    result
}
pub(super) fn get(store: &dyn SecretStore, id: &str) -> Result<SecretString> {
    let header = store
        .get(&super::key(id))
        .map_err(|_| DownloadError::CredentialsUnavailable)?
        .ok_or(DownloadError::CredentialsUnavailable)?;
    let count = header
        .expose()
        .strip_prefix("parts:")
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| (1..=PARTS).contains(n))
        .ok_or(DownloadError::CredentialsUnavailable)?;
    let mut source = SecretString::new(String::new());
    for index in 0..count {
        let value = store
            .get(&part(id, index))
            .map_err(|_| DownloadError::CredentialsUnavailable)?
            .ok_or(DownloadError::CredentialsUnavailable)?;
        if value.expose().len() > 1024 {
            return Err(DownloadError::CredentialsUnavailable);
        }
        // 拼接后的旧值由 SecretString 清零，不留普通 String 的中间副本。
        source = SecretString::new(format!("{}{}", source.expose(), value.expose()));
    }
    Ok(source)
}
pub(super) fn delete(store: &dyn SecretStore, id: &str) -> Result<()> {
    for index in 0..PARTS {
        store
            .delete(&part(id, index))
            .map_err(|_| DownloadError::CredentialsUnavailable)?;
    }
    store
        .delete(&super::key(id))
        .map_err(|_| DownloadError::CredentialsUnavailable)
}
