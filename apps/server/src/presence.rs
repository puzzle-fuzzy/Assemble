//! 在线状态。
//!
//! "在线"的定义只有一条：这台设备此刻连着事件流。没有另外的心跳，
//! 连接断了就是离线。连接可能同时开着好几条，所以按设备记条数。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use crate::events::Hub;

#[derive(Debug, Default)]
pub struct Presence {
    /// 账户 → 设备 → 此刻的事件流连接数。
    connections: Mutex<HashMap<String, HashMap<String, usize>>>,
}

impl Presence {
    /// 记一条连接。返回这台设备是否因此刚刚上线。
    pub fn connect(&self, account_id: &str, device_id: &str) -> bool {
        let mut connections = self
            .connections
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let count = connections
            .entry(account_id.to_owned())
            .or_default()
            .entry(device_id.to_owned())
            .or_insert(0);
        *count += 1;
        *count == 1
    }

    /// 撤一条连接。返回这台设备是否因此刚刚离线。
    pub fn disconnect(&self, account_id: &str, device_id: &str) -> bool {
        let mut connections = self
            .connections
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(devices) = connections.get_mut(account_id) else {
            return false;
        };
        let went_offline = match devices.get_mut(device_id) {
            Some(count) if *count > 1 => {
                *count -= 1;
                false
            }
            Some(_) => {
                devices.remove(device_id);
                true
            }
            None => false,
        };
        if devices.is_empty() {
            connections.remove(account_id);
        }
        went_offline
    }

    pub fn is_online(&self, account_id: &str, device_id: &str) -> bool {
        self.connections
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(account_id)
            .is_some_and(|devices| devices.contains_key(device_id))
    }
}

/// 一条事件流连接的登记。
///
/// 建立时登记；注销时（[`Connection::close`] 或被丢弃）这台设备少一条连接。
/// 上线与下线都会通知同账户的设备。
pub struct Connection {
    presence: Arc<Presence>,
    hub: Arc<Hub>,
    account_id: String,
    device_id: String,
    /// 离线时要把时间写进库，需要连接池。
    pool: sqlx::SqlitePool,
    /// 还没有注销。注销只做一次。
    live: bool,
}

impl Connection {
    pub fn open(
        presence: Arc<Presence>,
        hub: Arc<Hub>,
        pool: sqlx::SqlitePool,
        account_id: &str,
        device_id: &str,
    ) -> Self {
        if presence.connect(account_id, device_id) {
            hub.devices_changed(account_id);
        }
        Self {
            presence,
            hub,
            account_id: account_id.to_owned(),
            device_id: device_id.to_owned(),
            pool,
            live: true,
        }
    }

    /// 注销这条连接。正常的断开都走这里：先把"离线到现在"写进库，
    /// 再通知别的设备——这样它们拿到的列表里已经带着离线时间。
    pub async fn close(&mut self) {
        if !self.live {
            return;
        }
        self.live = false;
        if self.went_offline() {
            if let Err(error) = crate::devices::touch_last_seen(&self.pool, &self.device_id).await {
                tracing::debug!(%error, "last-seen time could not be written");
            }
            self.hub.devices_changed(&self.account_id);
        }
    }

    /// 撤掉登记，返回这台设备是否因此刚刚离线。
    fn went_offline(&self) -> bool {
        self.presence.disconnect(&self.account_id, &self.device_id)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // 兜底：连接被直接丢掉（任务被中止等）而没人调用过 `close`。
        if !self.live {
            return;
        }
        self.live = false;
        if self.went_offline() {
            // 析构里做不了异步操作：先通知，离线时间交给运行时去写。
            self.hub.devices_changed(&self.account_id);
            let pool = self.pool.clone();
            let device_id = self.device_id.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    if let Err(error) = crate::devices::touch_last_seen(&pool, &device_id).await {
                        tracing::debug!(%error, "last-seen time could not be written");
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_is_online_exactly_while_it_holds_at_least_one_connection() {
        let presence = Presence::default();
        assert!(!presence.is_online("a", "d1"));

        assert!(
            presence.connect("a", "d1"),
            "the first connection brings it online"
        );
        assert!(presence.is_online("a", "d1"));
        // 第二条连接不再算"刚刚上线"。
        assert!(!presence.connect("a", "d1"));
        // 别的账户里同名的设备互不影响。
        assert!(presence.connect("b", "d1"));

        // 关掉一条还有一条：仍然在线。
        assert!(!presence.disconnect("a", "d1"));
        assert!(presence.is_online("a", "d1"));
        // 最后一条关掉才算离线。
        assert!(
            presence.disconnect("a", "d1"),
            "the last connection takes it offline"
        );
        assert!(!presence.is_online("a", "d1"));
        assert!(presence.is_online("b", "d1"));
    }

    #[test]
    fn disconnecting_more_often_than_connected_is_harmless() {
        let presence = Presence::default();
        assert!(!presence.disconnect("nobody", "d1"));
        assert!(presence.connect("a", "d1"));
        // 另一台设备第一次连上：算它"刚刚上线"。
        assert!(presence.connect("a", "d2"));
        assert!(presence.disconnect("a", "d1"));
        assert!(!presence.disconnect("a", "d1"));
        assert!(!presence.disconnect("a", "missing"));
        assert!(presence.disconnect("a", "d2"));
        // 账户里一台设备都不剩时，账户这一项也清掉。
        assert!(presence.connections.lock().unwrap().is_empty());
    }

    /// 一个真实账户里的一台设备，加上在线状态、事件中心和一个订阅者。
    struct Rig {
        _dir: tempfile::TempDir,
        pool: sqlx::SqlitePool,
        presence: Arc<Presence>,
        hub: Arc<Hub>,
        listener: tokio::sync::broadcast::Receiver<Arc<crate::events::Message>>,
        account_id: String,
        device: crate::devices::DeviceRow,
    }

    async fn rig() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        let account = crate::accounts::create(
            &pool,
            "alex",
            "",
            assemble_store::secret::SecretString::new("a-long-enough-password".to_owned()),
        )
        .await
        .unwrap();
        let presence = Arc::new(Presence::default());
        let hub = Arc::new(Hub::default());
        let listener = hub.subscribe(&account.id);
        let device = crate::devices::register(
            &pool,
            &account.id,
            &assemble_protocol::device::NewDevice {
                name: "Mac".into(),
                platform: assemble_protocol::primitives::Platform::Macos,
            },
            "digest",
        )
        .await
        .unwrap();
        Rig {
            _dir: dir,
            pool,
            presence,
            hub,
            listener,
            account_id: account.id,
            device,
        }
    }

    #[tokio::test]
    async fn a_connection_announces_coming_online_and_going_offline_with_the_time_first() {
        let mut rig = rig().await;
        let account_id = rig.account_id.clone();
        let mut connection = Connection::open(
            Arc::clone(&rig.presence),
            Arc::clone(&rig.hub),
            rig.pool.clone(),
            &account_id,
            &rig.device.id,
        );
        assert!(matches!(
            rig.listener.try_recv().unwrap().as_ref(),
            crate::events::Message::DevicesChanged
        ));
        assert!(rig.presence.is_online(&account_id, &rig.device.id));

        connection.close().await;
        assert!(!rig.presence.is_online(&account_id, &rig.device.id));
        assert!(matches!(
            rig.listener.try_recv().unwrap().as_ref(),
            crate::events::Message::DevicesChanged
        ));
        // 通知发出时，离线时间已经写好了：别人去查列表就能看到。
        let seen = crate::devices::find(&rig.pool, &account_id, &rig.device.id)
            .await
            .unwrap()
            .unwrap()
            .last_seen_at;
        assert!(seen.is_some());
        // 再关一次、再丢弃，都不会重复注销或重复通知。
        connection.close().await;
        drop(connection);
        assert!(rig.listener.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_connection_that_is_dropped_without_closing_still_signs_off() {
        let rig = rig().await;
        let account_id = rig.account_id.clone();
        let connection = Connection::open(
            Arc::clone(&rig.presence),
            Arc::clone(&rig.hub),
            rig.pool.clone(),
            &account_id,
            &rig.device.id,
        );
        assert!(rig.presence.is_online(&account_id, &rig.device.id));
        drop(connection);
        assert!(!rig.presence.is_online(&account_id, &rig.device.id));
        // 离线时间由另起的任务去写：稍等一下再查。
        let seen = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Some(seen) = crate::devices::find(&rig.pool, &account_id, &rig.device.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .last_seen_at
                {
                    break seen;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(
            seen.is_ok(),
            "the offline time should be written eventually"
        );
    }
}
