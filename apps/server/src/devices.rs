//! 设备：登录过账户的每一台电脑，以及它们的接口。
//!
//! 一台设备对应一个桌面端安装，它持有的设备令牌就是它的身份。
//! 删除设备行，令牌就失效了——这就是"撤销一台设备"的全部实现。

use assemble_protocol::device::{Device, DeviceListing, NewDevice, RenameDeviceInput};
use assemble_protocol::login::MeResponse;
use assemble_protocol::primitives::{Platform, Timestamp};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use sqlx::SqlitePool;

use crate::audit::{self, actions};
use crate::auth::Identity;
use crate::error::ApiError;
use crate::http::Shared;
use crate::presence::Presence;
use crate::secrets;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DeviceRow {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub platform: String,
    pub created_at: i64,
    pub last_seen_at: Option<i64>,
}

/// 库里的平台名由 [`Platform::as_str`] 写入。这一版不认识的值不报错，落进 [`Platform::Other`]：
/// 报错的话，一个新平台会让那个账户的设备名单、自检和管理端一起失败，事件流也被掐断。
pub(crate) fn platform_from(stored: &str) -> Platform {
    match stored {
        "windows" => Platform::Windows,
        "macos" => Platform::Macos,
        "linux" => Platform::Linux,
        "ios" => Platform::Ios,
        other => {
            tracing::warn!(
                platform = other,
                "a stored device has a platform this version does not know"
            );
            Platform::Other
        }
    }
}

pub fn to_device(row: DeviceRow, online: bool) -> Device {
    Device {
        id: row.id,
        name: row.name,
        platform: platform_from(&row.platform),
        online,
        last_seen_at: row.last_seen_at.and_then(Timestamp::from_millis),
        created_at: Timestamp::from_millis(row.created_at).unwrap_or_else(Timestamp::now),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegisterError {
    #[error("a device id could not be generated: {0}")]
    Id(#[from] secrets::SecretError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

/// 注册一台设备。`token_hash` 是它令牌的摘要。
pub async fn register<'e, E>(
    executor: E,
    account_id: &str,
    device: &NewDevice,
    token_hash: &str,
) -> Result<DeviceRow, RegisterError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let id = secrets::random_id()?;
    let created_at = Timestamp::now().as_millis();
    let platform = device.platform.as_str();
    sqlx::query(
        "INSERT INTO devices (id, account_id, name, platform, token_hash, created_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(account_id)
    .bind(&device.name)
    .bind(platform)
    .bind(token_hash)
    .bind(created_at)
    .execute(executor)
    .await?;
    Ok(DeviceRow {
        id,
        account_id: account_id.to_owned(),
        name: device.name.clone(),
        platform: platform.to_owned(),
        created_at,
        last_seen_at: None,
    })
}

pub async fn find(
    pool: &SqlitePool,
    account_id: &str,
    device_id: &str,
) -> Result<Option<DeviceRow>, sqlx::Error> {
    sqlx::query_as::<_, DeviceRow>("SELECT * FROM devices WHERE account_id = ? AND id = ?")
        .bind(account_id)
        .bind(device_id)
        .fetch_optional(pool)
        .await
}

pub async fn exists(pool: &SqlitePool, device_id: &str) -> Result<bool, sqlx::Error> {
    let found: Option<i64> = sqlx::query_scalar("SELECT 1 FROM devices d JOIN accounts a ON a.id = d.account_id WHERE d.id = ? AND a.suspended_at IS NULL")
        .bind(device_id)
        .fetch_optional(pool)
        .await?;
    Ok(found.is_some())
}

/// 一个账户的全部设备，按注册先后排列。在线与否问 [`Presence`]。
///
/// 同一毫秒内注册的设备靠 `rowid` 分先后：`id` 是随机的，拿它当次序会让
/// 列表顺序时对时错（测试里偶尔复现）。
pub async fn list(
    pool: &SqlitePool,
    presence: &Presence,
    account_id: &str,
) -> Result<Vec<Device>, ApiError> {
    let rows = sqlx::query_as::<_, DeviceRow>(
        "SELECT * FROM devices WHERE account_id = ? ORDER BY created_at, rowid",
    )
    .bind(account_id)
    .fetch_all(pool)
    .await?;
    let devices = rows
        .into_iter()
        .map(|row| {
            let online = presence.is_online(account_id, &row.id);
            to_device(row, online)
        })
        .collect();
    Ok(devices)
}

/// 改名。名字两端的空白会去掉。返回这台设备是否属于这个账户。
pub async fn rename(
    pool: &SqlitePool,
    account_id: &str,
    device_id: &str,
    name: &str,
) -> Result<bool, sqlx::Error> {
    let written = sqlx::query("UPDATE devices SET name = ? WHERE account_id = ? AND id = ?")
        .bind(name.trim())
        .bind(account_id)
        .bind(device_id)
        .execute(pool)
        .await?;
    Ok(written.rows_affected() > 0)
}

/// 删除设备，也就是撤销它。返回这台设备是否属于这个账户。
pub async fn remove(
    pool: &SqlitePool,
    account_id: &str,
    device_id: &str,
) -> Result<bool, sqlx::Error> {
    let deleted = sqlx::query("DELETE FROM devices WHERE account_id = ? AND id = ?")
        .bind(account_id)
        .bind(device_id)
        .execute(pool)
        .await?;
    Ok(deleted.rows_affected() > 0)
}

/// 记下"最近一次在线到此为止"。
pub async fn touch_last_seen(pool: &SqlitePool, device_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE devices SET last_seen_at = ? WHERE id = ?")
        .bind(Timestamp::now().as_millis())
        .bind(device_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 服务端刚启动：所有设备都断着，把它们的离线时间记成现在。
pub async fn mark_all_offline(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE devices SET last_seen_at = ?")
        .bind(Timestamp::now().as_millis())
        .execute(pool)
        .await?;
    Ok(())
}

/// 设备情况变了：查一次完整列表，发给账户里的所有设备。
pub async fn broadcast_changed(state: &Shared, account_id: &str) {
    match list(&state.pool, &state.presence, account_id).await {
        Ok(devices) => state.hub.publish(
            account_id,
            crate::events::Message::Event {
                audience: None,
                event: std::sync::Arc::new(assemble_protocol::events::Event::DevicesChanged {
                    devices,
                }),
            },
        ),
        Err(error) => tracing::error!(%error, "devices could not be listed for an event"),
    }
}

// ---- 接口 ----

/// `GET /v1/me`
pub async fn me(
    State(state): State<Shared>,
    identity: Identity,
) -> Result<Json<MeResponse>, ApiError> {
    let account = crate::accounts::find_by_id(&state.pool, &identity.account_id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let device = find(&state.pool, &identity.account_id, &identity.device_id)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    // 能发出这个请求，说明这台设备至少这一次是连着的。
    let online = state
        .presence
        .is_online(&identity.account_id, &identity.device_id);
    Ok(Json(MeResponse {
        account: account.public(),
        device: to_device(device, online),
    }))
}

/// `GET /v1/devices`
pub async fn list_devices(
    State(state): State<Shared>,
    identity: Identity,
) -> Result<Json<DeviceListing>, ApiError> {
    let items = list(&state.pool, &state.presence, &identity.account_id).await?;
    Ok(Json(DeviceListing { items }))
}

/// `PATCH /v1/devices/{id}`
pub async fn rename_device(
    State(state): State<Shared>,
    identity: Identity,
    Path(device_id): Path<String>,
    body: Bytes,
) -> Result<Json<Device>, ApiError> {
    let input: RenameDeviceInput = crate::http::read_json(&body)?;
    let name = input.name.trim().to_owned();
    if !assemble_protocol::device::is_valid_device_name(&name) {
        return Err(ApiError::invalid("the device name is not acceptable"));
    }
    if !rename(&state.pool, &identity.account_id, &device_id, &name).await? {
        return Err(ApiError::not_found());
    }
    audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        actions::DEVICE_RENAMED,
        Some(&device_id),
    )
    .await;
    broadcast_changed(&state, &identity.account_id).await;
    let row = find(&state.pool, &identity.account_id, &device_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(to_device(
        row,
        state.presence.is_online(&identity.account_id, &device_id),
    )))
}

/// `DELETE /v1/devices/{id}`
pub async fn remove_device(
    State(state): State<Shared>,
    identity: Identity,
    Path(device_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let removed = remove(&state.pool, &identity.account_id, &device_id).await?;
    if !removed {
        return Err(ApiError::not_found());
    }
    // 删掉自己就是退出登录，审计里用另一个名字。
    let action = if device_id == identity.device_id {
        actions::DEVICE_LOGOUT
    } else {
        actions::DEVICE_REMOVED
    };
    audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        action,
        Some(&device_id),
    )
    .await;
    // 那台设备如果还连着事件流，下一次探活就会发现令牌已失效、自己断开。
    broadcast_changed(&state, &identity.account_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/auth/logout`
pub async fn logout(
    State(state): State<Shared>,
    identity: Identity,
) -> Result<StatusCode, ApiError> {
    remove(&state.pool, &identity.account_id, &identity.device_id).await?;
    audit::record(
        &state.pool,
        Some(&identity.account_id),
        Some(&identity.device_id),
        actions::DEVICE_LOGOUT,
        Some(&identity.device_id),
    )
    .await;
    broadcast_changed(&state, &identity.account_id).await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use assemble_store::secret::SecretString;

    use super::*;

    fn new_device(name: &str, platform: Platform) -> NewDevice {
        NewDevice {
            name: name.to_owned(),
            platform,
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        pool: SqlitePool,
        presence: Presence,
        account: crate::accounts::AccountRow,
    }

    async fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(dir.path()).await.unwrap();
        let account = crate::accounts::create(
            &pool,
            "alex",
            "",
            SecretString::new("a-long-enough-password".to_owned()),
        )
        .await
        .unwrap();
        Fixture {
            _dir: dir,
            pool,
            presence: Presence::default(),
            account,
        }
    }

    #[tokio::test]
    async fn devices_are_listed_in_registration_order_with_their_presence() {
        let fixture = fixture().await;
        let mac = register(
            &fixture.pool,
            &fixture.account.id,
            &new_device("Mac", Platform::Macos),
            "h1",
        )
        .await
        .unwrap();
        let windows = register(
            &fixture.pool,
            &fixture.account.id,
            &new_device("工作电脑", Platform::Windows),
            "h2",
        )
        .await
        .unwrap();
        assert_ne!(mac.id, windows.id);
        assert!(mac.last_seen_at.is_none());

        let listed = list(&fixture.pool, &fixture.presence, &fixture.account.id)
            .await
            .unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].name, "Mac");
        assert_eq!(listed[0].platform, Platform::Macos);
        // 同一毫秒内注册（时间戳完全相同）时，先后仍按注册顺序，而不是看随机的主键。
        sqlx::query("UPDATE devices SET created_at = ?")
            .bind(mac.created_at)
            .execute(&fixture.pool)
            .await
            .unwrap();
        let tied = list(&fixture.pool, &fixture.presence, &fixture.account.id)
            .await
            .unwrap();
        assert_eq!(
            tied.iter()
                .map(|device| device.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Mac", "工作电脑"]
        );
        assert_eq!(listed[1].platform, Platform::Windows);
        assert!(!listed[0].online);

        fixture.presence.connect(&fixture.account.id, &windows.id);
        let listed = list(&fixture.pool, &fixture.presence, &fixture.account.id)
            .await
            .unwrap();
        assert!(!listed[0].online);
        assert!(listed[1].online);
        // 另一个账户看不到这些设备。
        assert!(
            list(&fixture.pool, &fixture.presence, "other-account")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn devices_can_be_renamed_and_removed_only_within_their_account() {
        let fixture = fixture().await;
        let device = register(
            &fixture.pool,
            &fixture.account.id,
            &new_device("Mac", Platform::Macos),
            "h1",
        )
        .await
        .unwrap();

        assert!(
            !rename(&fixture.pool, "other", &device.id, "x")
                .await
                .unwrap()
        );
        assert!(
            rename(
                &fixture.pool,
                &fixture.account.id,
                &device.id,
                "  家里的 Mac  "
            )
            .await
            .unwrap()
        );
        assert_eq!(
            find(&fixture.pool, &fixture.account.id, &device.id)
                .await
                .unwrap()
                .unwrap()
                .name,
            "家里的 Mac"
        );

        assert!(exists(&fixture.pool, &device.id).await.unwrap());
        assert!(!remove(&fixture.pool, "other", &device.id).await.unwrap());
        assert!(exists(&fixture.pool, &device.id).await.unwrap());
        assert!(
            remove(&fixture.pool, &fixture.account.id, &device.id)
                .await
                .unwrap()
        );
        assert!(!exists(&fixture.pool, &device.id).await.unwrap());
        assert!(
            find(&fixture.pool, &fixture.account.id, &device.id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn going_offline_and_restarting_stamp_the_last_seen_time() {
        let fixture = fixture().await;
        let device = register(
            &fixture.pool,
            &fixture.account.id,
            &new_device("Mac", Platform::Macos),
            "h1",
        )
        .await
        .unwrap();
        touch_last_seen(&fixture.pool, &device.id).await.unwrap();
        let seen = find(&fixture.pool, &fixture.account.id, &device.id)
            .await
            .unwrap()
            .unwrap()
            .last_seen_at;
        assert!(seen.unwrap() >= device.created_at);

        mark_all_offline(&fixture.pool).await.unwrap();
        let second = register(
            &fixture.pool,
            &fixture.account.id,
            &new_device("PC", Platform::Windows),
            "h2",
        )
        .await
        .unwrap();
        assert!(second.last_seen_at.is_none());
        mark_all_offline(&fixture.pool).await.unwrap();
        assert!(
            find(&fixture.pool, &fixture.account.id, &second.id)
                .await
                .unwrap()
                .unwrap()
                .last_seen_at
                .is_some()
        );
    }

    #[test]
    fn stored_platforms_map_back_to_their_enum_values() {
        assert_eq!(platform_from("windows"), Platform::Windows);
        assert_eq!(platform_from("macos"), Platform::Macos);
        assert_eq!(platform_from("linux"), Platform::Linux);
        assert_eq!(platform_from("ios"), Platform::Ios);
        // 这里的值可以是更新版本的服务端写进去的：读成兜底的那一个，不报错。
        assert_eq!(platform_from("darwin"), Platform::Other);
        assert_eq!(platform_from(""), Platform::Other);
    }
}
