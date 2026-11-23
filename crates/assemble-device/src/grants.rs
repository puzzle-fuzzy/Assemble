//! 目录授权账本：云端记录必须与本机私有授权取交集。
//!
//! 云端的共享目录记录只是一份声明。只有当用户在本机通过系统目录选择器
//! 亲手确认过同一个路径，这里才会留下一条私有授权。云端改写路径、换服务
//! 来源或换账户，都不会让旧授权继续有效。

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use assemble_protocol::resources::{JarvisRecord, ResourceData};

/// 当前身份的范围：哪个服务、哪个账户、哪台设备。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub origin: String,
    pub account_id: String,
    pub device_id: String,
}

/// 本机对某一个云端共享目录的私有授权。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootGrant {
    pub path: PathBuf,
    pub origin: String,
    pub account_id: String,
    pub device_id: String,
}

impl RootGrant {
    /// 这条授权是否仍然对当前身份有效。
    pub fn covers(&self, scope: &Scope) -> bool {
        self.origin == scope.origin
            && self.account_id == scope.account_id
            && self.device_id == scope.device_id
    }
}

/// 全部私有授权，按云端目录的编号索引。落盘格式与 Electron 版的 `rootGrants` 相同。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GrantLedger(BTreeMap<String, RootGrant>);

/// 云端目录里通过了授权交集的一支，等待文件系统层面的最后确认。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedRoot {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub path: PathBuf,
}

impl GrantLedger {
    pub fn get(&self, root_id: &str) -> Option<&RootGrant> {
        self.0.get(root_id)
    }

    pub fn insert(&mut self, root_id: impl Into<String>, grant: RootGrant) -> Option<RootGrant> {
        self.0.insert(root_id.into(), grant)
    }

    pub fn remove(&mut self, root_id: &str) -> Option<RootGrant> {
        self.0.remove(root_id)
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 只保留仍属于当前服务来源与账户的授权；返回是否有授权被作废。
    ///
    /// 同一账户重新登录时授权原样保留；换服务或换号则全部清掉。
    pub fn retain_scope(&mut self, origin: &str, account_id: &str) -> bool {
        let before = self.0.len();
        self.0
            .retain(|_, grant| grant.origin == origin && grant.account_id == account_id);
        self.0.len() != before
    }

    /// 在云端目录里找出本机授权覆盖到的那一部分。
    ///
    /// 只做身份与路径字面值的比对；返回的路径还要再过一次文件系统校验。
    pub fn approve<'a>(
        &self,
        scope: &Scope,
        cloud_roots: impl IntoIterator<Item = &'a JarvisRecord>,
    ) -> Vec<ApprovedRoot> {
        cloud_roots
            .into_iter()
            .filter_map(|record| {
                let ResourceData::Roots(data) = &record.data else {
                    return None;
                };
                // 别的设备的目录与本机无关。
                if data.device_id != scope.device_id {
                    return None;
                }
                let grant = self.get(&record.id)?;
                let granted =
                    grant.covers(scope) && grant.path.as_os_str() == OsStr::new(&data.path);
                granted.then(|| ApprovedRoot {
                    id: record.id.clone(),
                    name: data.name.clone(),
                    enabled: data.enabled,
                    path: grant.path.clone(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const ORIGIN: &str = "https://jarvis.example";

    fn scope(account: &str, device: &str) -> Scope {
        Scope {
            origin: ORIGIN.into(),
            account_id: account.into(),
            device_id: device.into(),
        }
    }

    fn grant(path: &str, account: &str, device: &str) -> RootGrant {
        RootGrant {
            path: PathBuf::from(path),
            origin: ORIGIN.into(),
            account_id: account.into(),
            device_id: device.into(),
        }
    }

    fn cloud_root(id: &str, device: &str, path: &str, enabled: bool) -> JarvisRecord {
        serde_json::from_value(json!({
            "id": id,
            "resource": "roots",
            "version": 1,
            "createdAt": "2026-11-23T00:00:00.000Z",
            "updatedAt": "2026-11-23T00:00:00.000Z",
            "data": { "deviceId": device, "name": "文档", "path": path, "enabled": enabled }
        }))
        .unwrap()
    }

    #[test]
    fn a_cloud_root_without_a_local_grant_is_never_approved() {
        let ledger = GrantLedger::default();
        assert!(
            ledger
                .approve(
                    &scope("u1", "d1"),
                    &[cloud_root("r1", "d1", "/Users/u/docs", true)]
                )
                .is_empty()
        );
    }

    #[test]
    fn a_grant_covers_only_its_own_service_account_and_device() {
        let mut ledger = GrantLedger::default();
        ledger.insert("r1", grant("/Users/u/docs", "u1", "d1"));
        let root = cloud_root("r1", "d1", "/Users/u/docs", true);

        let approved = ledger.approve(&scope("u1", "d1"), [&root]);
        assert_eq!(
            approved,
            vec![ApprovedRoot {
                id: "r1".into(),
                name: "文档".into(),
                enabled: true,
                path: PathBuf::from("/Users/u/docs"),
            }]
        );
        // 换账户、换设备、换服务来源，同一条云端记录都不再被认可。
        assert!(ledger.approve(&scope("u2", "d1"), [&root]).is_empty());
        assert!(ledger.approve(&scope("u1", "d2"), [&root]).is_empty());
        let other_origin = Scope {
            origin: "http://127.0.0.1:3300".into(),
            ..scope("u1", "d1")
        };
        assert!(ledger.approve(&other_origin, [&root]).is_empty());
    }

    #[test]
    fn a_remote_path_edit_cannot_widen_local_access() {
        let mut ledger = GrantLedger::default();
        ledger.insert("r1", grant("/Users/u/docs", "u1", "d1"));
        let scope = scope("u1", "d1");

        // 云端把路径改成整个用户目录：没有效果。
        assert!(
            ledger
                .approve(&scope, &[cloud_root("r1", "d1", "/Users/u", true)])
                .is_empty()
        );
        // 改回原路径才重新生效；停用状态如实带出。
        let approved = ledger.approve(&scope, &[cloud_root("r1", "d1", "/Users/u/docs", false)]);
        assert!(!approved[0].enabled);
        assert_eq!(approved[0].path, PathBuf::from("/Users/u/docs"));
    }

    #[test]
    fn another_devices_directory_is_never_approved_here() {
        let mut ledger = GrantLedger::default();
        // 即使账本里误存了一条属于别的设备的授权，也不会被认可。
        ledger.insert("r2", grant("/Users/u/docs", "u1", "d2"));
        assert!(
            ledger
                .approve(
                    &scope("u1", "d1"),
                    &[cloud_root("r2", "d2", "/Users/u/docs", true)]
                )
                .is_empty()
        );
    }

    #[test]
    fn switching_account_or_service_retires_grants_but_relogin_keeps_them() {
        // 同一账户重新登录：授权原样保留。
        let mut ledger = GrantLedger::default();
        ledger.insert("r1", grant("/Users/u/docs", "u1", "d1"));
        assert!(!ledger.retain_scope(ORIGIN, "u1"));
        assert_eq!(ledger.len(), 1);
        assert!(ledger.get("r1").is_some());

        // 换账户：原账户的授权被作废，新账户的保留。
        ledger.insert("r2", grant("/Users/u/music", "u2", "d1"));
        assert!(ledger.retain_scope(ORIGIN, "u2"));
        assert!(ledger.get("r1").is_none());
        assert_eq!(ledger.get("r2"), Some(&grant("/Users/u/music", "u2", "d1")));

        // 换服务来源：全部作废，开发环境与正式环境互不继承。
        assert!(ledger.retain_scope("http://127.0.0.1:3300", "u2"));
        assert!(ledger.is_empty());

        let mut ledger = GrantLedger::default();
        ledger.insert("r1", grant("/Users/u/docs", "u1", "d1"));
        ledger.clear();
        assert!(ledger.is_empty());
    }

    #[test]
    fn the_ledger_round_trips_through_the_original_state_format() {
        let json_text = r#"{"r1":{"path":"/Users/u/docs","origin":"https://jarvis.example","accountId":"u1","deviceId":"d1"}}"#;
        let ledger: GrantLedger = serde_json::from_str(json_text).unwrap();
        assert_eq!(ledger.get("r1"), Some(&grant("/Users/u/docs", "u1", "d1")));
        assert_eq!(serde_json::to_string(&ledger).unwrap(), json_text);

        // 落盘后再读回来，授权依然有效。
        let scope = scope("u1", "d1");
        assert_eq!(
            ledger
                .approve(&scope, &[cloud_root("r1", "d1", "/Users/u/docs", true)])
                .len(),
            1
        );
        let mut ledger = ledger;
        assert_eq!(
            ledger.remove("r1"),
            Some(grant("/Users/u/docs", "u1", "d1"))
        );
        assert!(ledger.remove("r1").is_none());
    }
}
