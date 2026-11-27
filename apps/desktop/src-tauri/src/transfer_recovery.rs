//! 独立于历史的私有工作日志。未知版本和坏文件保留，不能以空队列覆盖。
use crate::registry::{Registry, TransferRecord};
use assemble_store::json::{StoreError, read_json, write_private_json};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    origin: String,
    account: String,
    device: String,
    tasks: Vec<TransferRecord>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    owners: Vec<Owner>,
}

#[derive(Debug)]
pub struct RecoveryStore {
    path: Option<PathBuf>,
    origin: String,
    owner: Option<(String, String)>,
    data: Journal,
    blocked: bool,
}
fn invalid() -> StoreError {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "transfer recovery journal is unavailable",
    )
    .into()
}
impl RecoveryStore {
    pub fn new(origin: &str) -> Self {
        Self {
            path: None,
            origin: origin.to_owned(),
            owner: None,
            data: Journal {
                version: 1,
                owners: vec![],
            },
            blocked: false,
        }
    }
    pub fn attach(&mut self, path: PathBuf) -> Result<(), StoreError> {
        self.path = Some(path.clone());
        self.owner = None;
        let result = (|| {
            if let Ok(meta) = std::fs::symlink_metadata(&path)
                && (!meta.is_file()
                    || meta.file_type().is_symlink()
                    || meta.len() > 4 * 1024 * 1024)
            {
                return Err(invalid());
            }
            let data: Journal = read_json(&path)?.unwrap_or(Journal {
                version: 1,
                owners: vec![],
            });
            if data.version != 1
                || data.owners.len() > 20
                || data.owners.iter().map(|o| o.tasks.len()).sum::<usize>() > 200
            {
                return Err(invalid());
            }
            for owner in &data.owners {
                if !Registry::default().restore_recovery(owner.tasks.clone()) {
                    return Err(invalid());
                }
            }
            self.data = data;
            Ok(())
        })();
        self.blocked = result.is_err();
        result
    }
    pub fn detach(&mut self) {
        self.owner = None;
    }
    pub fn ready(&self) -> bool {
        !self.blocked
    }
    pub fn active(&self) -> bool {
        self.owner.is_some()
    }
    pub fn protected_staging(&self) -> Option<Vec<(PathBuf, String)>> {
        self.ready().then(|| {
            self.data
                .owners
                .iter()
                .flat_map(|o| o.tasks.iter())
                .filter_map(|r| r.download.as_ref())
                .map(|c| (c.directory.clone(), c.staging.clone()))
                .collect()
        })
    }
    pub fn restore(
        &mut self,
        account: &str,
        device: &str,
        registry: &mut Registry,
    ) -> Result<(), StoreError> {
        if self.blocked {
            return Err(invalid());
        }
        if self
            .owner
            .as_ref()
            .is_some_and(|(a, d)| a == account && d == device)
        {
            return Ok(());
        }
        // 调用者即使复用同一份 Registry，也不能把上一设备的工作意图交给新身份。
        registry.detach_recovery();
        self.owner = Some((account.to_owned(), device.to_owned()));
        if let Some(owner) = self
            .data
            .owners
            .iter()
            .find(|o| o.origin == self.origin && o.account == account && o.device == device)
            && !registry.restore_recovery(owner.tasks.clone())
        {
            self.blocked = true;
            return Err(invalid());
        }
        Ok(())
    }
    /// 调用方在同一把锁下改状态和写日志，下一次网络/文件副作用之前必须成功。
    pub fn persist(&mut self, registry: &Registry) -> Result<(), StoreError> {
        if self.blocked {
            return Err(invalid());
        }
        let (Some(path), Some((account, device))) = (&self.path, &self.owner) else {
            return Ok(());
        };
        let mut next = self.data.clone();
        next.owners
            .retain(|o| !(o.origin == self.origin && &o.account == account && &o.device == device));
        let tasks = registry.recovery();
        if !tasks.is_empty() {
            next.owners.push(Owner {
                origin: self.origin.clone(),
                account: account.clone(),
                device: device.clone(),
                tasks,
            });
        }
        if next.owners.len() > 20
            || next.owners.iter().map(|o| o.tasks.len()).sum::<usize>() > 200
            || next.owners.iter().any(|o| o.tasks.len() > 100)
            || serde_json::to_vec(&next)?.len() > 4 * 1024 * 1024
        {
            self.blocked = true;
            return Err(invalid());
        }
        if next != self.data {
            if let Err(error) = write_private_json(path, &next) {
                self.blocked = true;
                return Err(error);
            }
            self.data = next;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Destination, Next, TransferPhase};
    #[test]
    fn restarts_paused_and_never_crosses_origin_account_or_device() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery.json");
        let mut store = RecoveryStore::new("https://one.example");
        store.attach(path.clone()).unwrap();
        let mut registry = Registry::default();
        store.restore("alice", "device-a", &mut registry).unwrap();
        registry.queue(
            Destination::Drive,
            vec![assemble_client::OutgoingFile {
                path: dir.path().join("a"),
                size: 3,
                sha256: "a".repeat(64),
            }],
        );
        store.persist(&registry).unwrap();
        for (origin, account, device, expected) in [
            ("https://one.example", "alice", "device-a", 1),
            ("https://two.example", "alice", "device-a", 0),
            ("https://one.example", "bob", "device-a", 0),
            ("https://one.example", "alice", "device-b", 0),
        ] {
            let mut store = RecoveryStore::new(origin);
            store.attach(path.clone()).unwrap();
            let mut registry = Registry::default();
            store.restore(account, device, &mut registry).unwrap();
            assert_eq!(registry.views().len(), expected);
            if expected == 1 {
                assert_eq!(registry.views()[0].phase, TransferPhase::Paused);
                assert_eq!(registry.next_job(std::time::Instant::now()), Next::Nothing);
            }
        }
        let value = std::fs::read_to_string(path).unwrap();
        assert!(!value.contains("token"));
        assert!(!value.contains("password"));
    }
    #[test]
    fn switching_device_in_the_same_registry_removes_executable_old_intents() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = RecoveryStore::new("origin");
        store.attach(dir.path().join("recovery.json")).unwrap();
        let mut registry = Registry::default();
        store.restore("a", "old", &mut registry).unwrap();
        registry.queue(
            Destination::Drive,
            vec![assemble_client::OutgoingFile {
                path: dir.path().join("source"),
                size: 1,
                sha256: "a".repeat(64),
            }],
        );
        store.persist(&registry).unwrap();
        store.restore("a", "new", &mut registry).unwrap();
        assert_eq!(registry.next_job(std::time::Instant::now()), Next::Nothing);
        assert!(!registry.views()[0].resumable);
        assert!(!registry.resume_upload(&registry.views()[0].id));
    }
    #[test]
    fn corrupt_and_future_journals_survive_failed_restore_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery.json");
        for bytes in [b"broken".as_slice(), br#"{"version":99,"owners":[]}"#] {
            std::fs::write(&path, bytes).unwrap();
            let mut store = RecoveryStore::new("x");
            assert!(store.attach(path.clone()).is_err());
            assert!(store.protected_staging().is_none());
            assert!(store.restore("a", "d", &mut Registry::default()).is_err());
            assert!(store.persist(&Registry::default()).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
}
