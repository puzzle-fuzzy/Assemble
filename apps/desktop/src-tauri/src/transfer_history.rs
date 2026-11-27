//! 传输历史的本机持久化。调用方持锁覆盖状态修改和保存，防止旧快照覆盖新记录。

use std::path::PathBuf;

use assemble_store::json::{StoreError, read_json, write_private_json};

use crate::registry::{History, Registry};

#[derive(Debug, Clone, Copy)]
enum Blocked {
    Unreadable,
    Unsupported,
}

impl Blocked {
    fn error(self) -> StoreError {
        let message = match self {
            Self::Unreadable => "transfer history could not be read",
            Self::Unsupported => "unsupported transfer history version",
        };
        std::io::Error::new(std::io::ErrorKind::InvalidData, message).into()
    }
}

#[derive(Debug)]
pub struct HistoryStore {
    path: Option<PathBuf>,
    origin: String,
    account_id: Option<String>,
    saved: Option<History>,
    blocked: Option<Blocked>,
}

impl HistoryStore {
    pub fn new(origin: &str) -> Self {
        Self {
            path: None,
            origin: origin.to_owned(),
            account_id: None,
            saved: None,
            blocked: None,
        }
    }

    /// 真实应用在启动时传入自己的数据目录；测试传一次性的隔离路径。
    pub fn attach(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.account_id = None;
        self.saved = None;
        self.blocked = None;
    }

    /// 只有已验证会话告诉本机当前账户后才恢复，读不到的历史不被当成空文件覆盖。
    pub fn restore(&mut self, account_id: &str, registry: &mut Registry) -> Result<(), StoreError> {
        if self.account_id.as_deref() == Some(account_id) {
            return self.blocked.map_or(Ok(()), |reason| Err(reason.error()));
        }
        *registry = Registry::default();
        self.account_id = Some(account_id.to_owned());
        self.saved = None;
        self.blocked = None;
        let Some(path) = &self.path else {
            return Ok(());
        };
        let history = match read_json::<History>(path) {
            Ok(history) => history,
            Err(error) => {
                self.blocked = Some(Blocked::Unreadable);
                return Err(error);
            }
        };
        if history
            .as_ref()
            .is_some_and(|history| !history.is_supported())
        {
            self.blocked = Some(Blocked::Unsupported);
            return Err(Blocked::Unsupported.error());
        }
        if let Some(history) =
            history.filter(|history| history.belongs_to(&self.origin, account_id))
        {
            *registry = Registry::from_history(history, &self.origin, account_id);
        }
        self.persist(registry)
    }

    pub fn persist(&mut self, registry: &Registry) -> Result<(), StoreError> {
        if let Some(reason) = self.blocked {
            // 显式 flush 必须知道没有保存，不能把“保留未知文件”伪装成写入成功。
            return Err(reason.error());
        }
        let (Some(path), Some(account_id)) = (&self.path, &self.account_id) else {
            return Ok(());
        };
        let history = registry.history(&self.origin, account_id);
        if self.saved.as_ref() != Some(&history) {
            write_private_json(path, &history)?;
            self.saved = Some(history);
        }
        Ok(())
    }

    /// 失效只清除当前可见状态；历史仍按原账户隔离，重新登录后才能恢复。
    pub fn detach(&mut self) {
        self.account_id = None;
        self.saved = None;
    }

    /// 用户显式退出且服务端确认后才调用；移除名单不会删除已经收到的文件。
    pub fn clear(&mut self) -> Result<(), StoreError> {
        if let Some(path) = &self.path {
            // 用私有原子替换写空，不能先删旧文件再遇到写失败。
            write_private_json(path, &Registry::default().history(&self.origin, ""))?;
        }
        self.account_id = None;
        self.saved = None;
        self.blocked = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_and_server_changes_cannot_restore_another_owners_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transfers.json");
        let mut store = HistoryStore::new("https://one.example");
        store.attach(path.clone());
        let mut registry = Registry::default();
        store.restore("alice", &mut registry).unwrap();
        let history: History = read_json(&path).unwrap().unwrap();
        assert!(history.belongs_to("https://one.example", "alice"));
        store.clear().unwrap();
        let history: History = read_json(&path).unwrap().unwrap();
        assert!(!history.belongs_to("https://one.example", "alice"));
        let mut other = HistoryStore::new("https://two.example");
        other.attach(path);
        other.restore("bob", &mut registry).unwrap();
        assert_eq!(registry.views().len(), 0);
    }

    #[test]
    fn corrupt_history_is_preserved_instead_of_overwritten_by_an_empty_registry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transfers.json");
        std::fs::write(&path, b"broken history").unwrap();
        let mut store = HistoryStore::new("https://one.example");
        store.attach(path.clone());
        let mut registry = Registry::default();
        assert!(store.restore("alice", &mut registry).is_err());
        assert!(
            matches!(store.persist(&registry), Err(StoreError::Io(error)) if error.kind() == std::io::ErrorKind::InvalidData)
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"broken history");
    }

    #[test]
    fn a_failed_clear_keeps_the_previous_owner_and_saved_history() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transfers.json");
        let backup = dir.path().join("saved.json");
        let mut store = HistoryStore::new("https://one.example");
        store.attach(path.clone());
        store.restore("alice", &mut Registry::default()).unwrap();
        let before = std::fs::read(&path).unwrap();
        std::fs::rename(&path, &backup).unwrap();
        // 不依赖平台权限差异：目标是目录时，原子替换必然失败。
        std::fs::create_dir(&path).unwrap();
        assert!(store.clear().is_err());
        assert_eq!(store.account_id.as_deref(), Some("alice"));
        assert!(store.saved.is_some());
        assert_eq!(std::fs::read(&backup).unwrap(), before);
    }

    #[test]
    fn a_future_history_version_is_not_silently_replaced() {
        use crate::registry::Destination;
        use assemble_client::OutgoingFile;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transfers.json");
        let source = dir.path().join("保存过的文件.txt");
        std::fs::write(&source, "abc").unwrap();
        let file = OutgoingFile {
            path: source,
            size: 3,
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
        };
        let mut original = Registry::default();
        let ids = original.queue(Destination::Drive, vec![file.clone()]);
        original.begin_send(&ids[0]);
        assert!(original.drive_uploaded(&ids[0]));
        let mut value =
            serde_json::to_value(original.history("https://one.example", "alice")).unwrap();
        assert_eq!(value["entries"].as_array().unwrap().len(), 1);
        value["version"] = 99.into();
        let before = serde_json::to_vec(&value).unwrap();
        std::fs::write(&path, &before).unwrap();
        let mut store = HistoryStore::new("https://one.example");
        store.attach(path.clone());
        let mut registry = Registry::default();
        assert!(
            matches!(store.restore("alice", &mut registry), Err(StoreError::Io(error)) if error.kind() == std::io::ErrorKind::InvalidData && error.to_string() == "unsupported transfer history version")
        );
        assert_eq!(registry.views().len(), 0);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        // 跟真实 AppState.registry 一样，恢复失败后仍允许本次使用，只拒绝覆盖旧文件。
        registry.queue(Destination::Drive, vec![file]);
        assert_eq!(registry.views().len(), 1);
        for _ in 0..2 {
            assert!(
                matches!(store.persist(&registry), Err(StoreError::Io(error)) if error.kind() == std::io::ErrorKind::InvalidData && error.to_string() == "unsupported transfer history version")
            );
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        assert!(store.restore("alice", &mut registry).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
