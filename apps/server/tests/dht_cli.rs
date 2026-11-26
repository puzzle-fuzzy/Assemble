//! 运维入口只接收具名模式，不需要网页登录凭据，也不访问公网。
#![cfg_attr(test, allow(clippy::pedantic))]
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn cli(directory: &Path, enabled: bool, arguments: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_assemble-server"));
    command.env_clear();
    for key in ["SystemRoot", "WINDIR", "TMP", "TEMP"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env("ASSEMBLE_DATA_DIR", directory)
        .env("ASSEMBLE_PUBLIC_ORIGIN", "http://127.0.0.1:8787")
        .env(
            "ASSEMBLE_DHT_ENABLED",
            if enabled { "true" } else { "false" },
        )
        .args(arguments)
        .output()
        .unwrap()
}

#[test]
fn disabled_collection_and_unknown_modes_are_rejected_without_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let rejected = cli(directory.path(), false, &["dht-mode", "collecting"]);
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8(rejected.stderr)
            .unwrap()
            .contains("DHT_UNAVAILABLE")
    );
    assert!(
        !cli(directory.path(), true, &["dht-mode", "unknown"])
            .status
            .success()
    );
    let report = cli(directory.path(), false, &["dht-status"]);
    assert!(report.status.success());
    let report: Value = serde_json::from_slice(&report.stdout).unwrap();
    assert_eq!(report["mode"], "stopped");
    assert_eq!(report["enabled"], false);
}

#[tokio::test]
async fn operator_mode_is_persisted_and_audited_once_and_can_be_stopped_when_disabled() {
    let directory = tempfile::tempdir().unwrap();
    for _ in 0..2 {
        let result = cli(directory.path(), true, &["dht-mode", "collecting"]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(report["mode"], "collecting");
        assert_eq!(report.as_object().unwrap().len(), 5);
    }
    let pool = assemble_server::db::open(directory.path()).await.unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'dht.mode_changed' AND account_id IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let stopped = cli(directory.path(), false, &["dht-mode", "stopped"]);
    assert!(stopped.status.success());
    assert_eq!(
        assemble_server::dht::store::mode(&pool).await.unwrap(),
        assemble_protocol::dht::DhtMode::Stopped
    );
    pool.close().await;
}
