use crate::auth::Identity;
use assemble_protocol::{
    activity::ActivitySnapshot,
    primitives::Timestamp,
    remote::{RemotePhase, RemoteSession},
};
use sqlx::SqlitePool;

/// 不序列化 RemoteSession：它含安全核对材料，日志只逐项取白名单字段。
pub async fn record_remote(
    pool: &SqlitePool,
    owner: &str,
    actor: Option<&Identity>,
    action: &str,
    session: &RemoteSession,
    at: Timestamp,
) -> bool {
    if !matches!(
        (action, session.phase),
        ("remote.requested", RemotePhase::Pending)
            | ("remote.accepted", RemotePhase::Active)
            | ("remote.ended", RemotePhase::Ended)
    ) {
        return false;
    }
    let snapshot = ActivitySnapshot::Remote {
        controller_id: session.controller_id.clone(),
        controller_name: session.controller_name.clone(),
        host_id: session.host_id.clone(),
        host_name: session.host_name.clone(),
        phase: crate::planner::spelling(&session.phase),
        end_reason: session
            .end_reason
            .map(|reason| crate::planner::spelling(&reason)),
    };
    let result=async {
        let mut tx=pool.begin_with("BEGIN IMMEDIATE").await?;
        let key=format!("{}:{action}",session.id);
        let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM audit_log WHERE event_key=?)").bind(&key).fetch_one(&mut *tx).await?;
        if exists {return tx.rollback().await;}
        let data=serde_json::to_string(&snapshot).map_err(|e|sqlx::Error::Encode(Box::new(e)))?;
        let now=at.as_millis();
        sqlx::query("INSERT INTO activity_subjects(kind,id,owner_id,label,data,created_at,updated_at) VALUES('remote',?,?,?,?,?,?) ON CONFLICT(kind,id) DO UPDATE SET label=excluded.label,data=excluded.data,updated_at=excluded.updated_at WHERE json_extract(activity_subjects.data,'$.phase')!='ended' AND NOT(json_extract(activity_subjects.data,'$.phase')='active' AND json_extract(excluded.data,'$.phase')='pending')")
            .bind(&session.id).bind(owner).bind(format!("{} → {}",session.controller_name,session.host_name)).bind(&data).bind(session.created_at.as_millis()).bind(now).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO audit_log(account_id,device_id,action,target,created_at,event_key,snapshot) VALUES(?,?,?,?,?,?,?)")
            .bind(actor.map(|a|a.account_id.as_str())).bind(actor.map(|a|a.device_id.as_str())).bind(action).bind(&session.id).bind(now).bind(key).bind(data).execute(&mut *tx).await?;
        tx.commit().await
    }.await;
    if result.is_err() {
        tracing::error!(
            code = "ACTIVITY_WRITE_FAILED",
            "remote activity could not be recorded"
        );
    }
    result.is_ok()
}

/// 新进程还没有接受连接时运行；旧进程的内存会话已不存在，只结束日志中的未完成状态。
pub async fn recover_remote(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let now = Timestamp::now().as_millis();
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let ids:Vec<String>=sqlx::query_scalar("UPDATE activity_subjects SET data=json_set(data,'$.phase','ended','$.endReason',CASE WHEN json_extract(data,'$.phase')='pending' THEN 'expired' ELSE 'disconnected' END),updated_at=? WHERE kind='remote' AND json_extract(data,'$.phase') IN ('pending','active') RETURNING id")
        .bind(now).fetch_all(&mut *tx).await?;
    for id in ids {
        sqlx::query("INSERT OR IGNORE INTO audit_log(action,target,created_at,event_key) VALUES('remote.ended',?,?,?)").bind(&id).bind(now).bind(format!("{id}:remote.ended")).execute(&mut *tx).await?;
    }
    tx.commit().await
}
