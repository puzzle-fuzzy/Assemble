//! 原生宿主验收夹具：仅建立独立临时用途的钥匙串条目，收到 stdin 后显式清理。
use assemble_download::{DownloadManager, MediaTools, browser::BrowserServer};
use assemble_store::secret::system_secret_store;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let app_id = args.next().expect("test app id");
    let folder = PathBuf::from(args.next().expect("test directory"));
    assert!(app_id.starts_with("com.puzzlefuzzy.assemble.smoke."));
    std::fs::create_dir_all(&folder).expect("fixture directory");
    let manager = DownloadManager::open(
        folder.join("history.json"),
        folder,
        MediaTools::discover(),
        |_| {},
    )
    .expect("fixture history");
    let server = BrowserServer::start(
        Arc::clone(&manager),
        Arc::new(system_secret_store().expect("system credential store")),
        &app_id,
        || {},
    )
    .await
    .expect("test bridge");
    println!("READY");
    std::io::stdout().flush().expect("fixture ready");
    tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
    })
    .await
    .expect("stop reader");
    server.cancel();
    manager.shutdown().await;
    server.shutdown().await;
    println!("CLEANED");
}
