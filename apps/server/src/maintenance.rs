//! 定时清理：过期的发送、过期的登录凭据、没人要的内容、太久的发送记录。
//!
//! 每一轮都很便宜，所以隔一分钟跑一次。服务端只有一个实例，这里不需要跨进程协调；
//! 状态改动都带条件（`WHERE state = 'offered'`），万一跑重了也不会重复释放内容。

use std::sync::Arc;
use std::time::Duration;

use assemble_protocol::events::Event;
use assemble_protocol::primitives::Timestamp;
use tokio_util::sync::CancellationToken;

use crate::blobs::Sweep;
use crate::events::Message;
use crate::http::Shared;
use crate::sends::SendRow;

/// 多久跑一轮。
pub const INTERVAL: Duration = Duration::from_secs(60);

/// 通知保留多久：三十天。过了这么久的场次早已不在宽限之内，
/// 删掉它的记录不会让它再被提醒一遍。
pub const NOTIFICATION_HISTORY_MILLIS: i64 = 30 * 24 * 60 * 60 * 1000;

/// 一轮清理的结果，只进日志。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    /// 超过时限还没送达、被标成过期的发送。
    pub expired_sends: u32,
    /// 超过保留期、被删掉的发送记录。
    pub pruned_sends: u64,
    pub expired_codes: u64,
    pub expired_sessions: u64,
    /// 超过保留期、被删掉的通知。
    pub pruned_notifications: u64,
    pub blobs: Sweep,
}

/// 在后台按固定间隔运行，直到服务端退出。
pub fn start(state: Shared, stop: CancellationToken) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = stop.cancelled() => return,
                _ = ticker.tick() => {}
            }
            match run_once(&state).await {
                Ok(report) => {
                    if report != Report::default() {
                        tracing::info!(?report, "maintenance finished");
                    }
                }
                Err(error) => tracing::error!(%error, "maintenance round failed"),
            }
        }
    });
}

/// 跑一轮。服务端启动时也调一次，把上次运行留下的尾巴处理掉。
pub async fn run_once(state: &Shared) -> Result<Report, sqlx::Error> {
    let mut report = Report::default();
    let pool = &state.pool;
    let now = Timestamp::now().as_millis();

    // 1. 过期的发送：标记、释放内容、通知两边。
    let cutoff = (Timestamp::now() - state.config.send_ttl).as_millis();
    let overdue: Vec<SendRow> =
        sqlx::query_as("SELECT * FROM sends WHERE state = 'offered' AND created_at < ?")
            .bind(cutoff)
            .fetch_all(pool)
            .await?;
    for row in &overdue {
        if let Some(expired) = crate::sends::expire(state, row).await? {
            notify_settled(state, &expired);
            report.expired_sends += 1;
        }
    }

    // 2. 太久的发送记录。
    let history_cutoff = (Timestamp::now() - state.config.send_history).as_millis();
    report.pruned_sends =
        sqlx::query("DELETE FROM sends WHERE created_at < ? AND state != 'offered'")
            .bind(history_cutoff)
            .execute(pool)
            .await?
            .rows_affected();

    // 3. 过期的授权码与网页会话。
    report.expired_codes = sqlx::query("DELETE FROM desktop_auth_codes WHERE expires_at < ?")
        .bind(now)
        .execute(pool)
        .await?
        .rows_affected();
    report.expired_sessions = sqlx::query("DELETE FROM web_sessions WHERE expires_at < ?")
        .bind(now)
        .execute(pool)
        .await?
        .rows_affected();

    sqlx::query("DELETE FROM email_actions WHERE expires_at <= ?")
        .bind(now)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM email_rate_limits WHERE started_at < ?")
        .bind(now - 3_600_000)
        .execute(pool)
        .await?;

    // 4. 太久的通知，读没读过都一样。
    report.pruned_notifications = sqlx::query("DELETE FROM notifications WHERE raised_at < ?")
        .bind(now - NOTIFICATION_HISTORY_MILLIS)
        .execute(pool)
        .await?
        .rows_affected();

    // 5. 没人引用的内容与中断的暂存文件。
    report.blobs = state.blobs.sweep().await?;
    Ok(report)
}

/// 把一次已经到终态的发送告诉收发两边。
fn notify_settled(state: &Shared, row: &SendRow) {
    for device in [&row.from_device_id, &row.to_device_id] {
        let Ok(send) = row.view_for(device) else {
            continue;
        };
        state.hub.publish(
            &row.account_id,
            Message::Event {
                audience: Some(device.clone()),
                event: Arc::new(Event::SendUpdated { send }),
            },
        );
    }
}
