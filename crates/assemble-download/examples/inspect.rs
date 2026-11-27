//! 手工验收公开来源：只输出已脱敏的预览或稳定错误码，不打印网络诊断。
use assemble_download::{DownloadManager, MediaTools};
use assemble_protocol::downloads::{DownloadInput, DownloadKind};

#[tokio::main]
async fn main() {
    let Some(value) = std::env::args().nth(1) else {
        eprintln!("usage: inspect <public-url>");
        std::process::exit(2);
    };
    let directory = tempfile::tempdir().expect("local temporary directory");
    let manager = DownloadManager::open(
        directory.path().join("history.json"),
        directory.path().to_owned(),
        MediaTools::discover(),
        |_| {},
    )
    .expect("local history");
    let kind = if value.contains("douyin.com") {
        DownloadKind::Douyin
    } else {
        DownloadKind::Hls
    };
    match manager
        .prepare(DownloadInput {
            magnet_network: None,
            kind,
            url: value,
            page_url: None,
            title: None,
        })
        .await
    {
        Ok(value) => {
            println!("{}", serde_json::to_string(&value).expect("preview"));
            if std::env::args().nth(2).as_deref() == Some("--download") {
                let id = manager
                    .start(&value.id, &value.variants[0].id)
                    .await
                    .expect("start");
                let result = tokio::time::timeout(std::time::Duration::from_mins(3), async {
                    loop {
                        let view = manager.view();
                        let task = view.tasks.iter().find(|t| t.id == id).expect("task");
                        if !task.phase.active() {
                            return task.clone();
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                })
                .await;
                manager.shutdown().await;
                let task = result.expect("download timeout");
                println!("{}", serde_json::to_string(&task).expect("task"));
                if task.phase != assemble_protocol::downloads::DownloadPhase::Completed {
                    std::process::exit(1);
                }
                let path = manager.completed_path(&id).await.expect("completed file");
                let probe = std::process::Command::new("ffprobe")
                    .args([
                        "-v",
                        "error",
                        "-show_entries",
                        "stream=codec_type",
                        "-of",
                        "json",
                    ])
                    .arg(path)
                    .output()
                    .expect("ffprobe");
                assert!(probe.status.success());
                let streams: serde_json::Value =
                    serde_json::from_slice(&probe.stdout).expect("stream metadata");
                println!("{streams}");
            }
        }
        Err(error) => {
            eprintln!("{}", error.code());
            manager.shutdown().await;
            std::process::exit(1);
        }
    }
    manager.shutdown().await;
}
