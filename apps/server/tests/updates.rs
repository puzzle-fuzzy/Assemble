//! 桌面端更新的服务端一半，对着真实运行的服务端走一遍：发布之前什么都读不到；发布命令
//! 先核对再发布；只有更新的版本才能替换已发布的；两个读接口按来源限速；子命令真的验签。
#![cfg_attr(test, allow(clippy::pedantic))]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assemble_server::{Config, db, http::TestServer, updates};
use base64::Engine as _;
use reqwest::StatusCode;

/// 测试用的另一把钥匙：服务端不该认它以外的任何钥匙，所以每一项测试都带上自己的公钥。
struct Keys {
    pair: minisign::KeyPair,
    /// 公钥文件的内容，Tauri 的 `signer generate` 写出来的那一种。
    public: String,
}

impl Keys {
    fn new() -> Self {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let public =
            base64::engine::general_purpose::STANDARD.encode(pair.pk.to_box().unwrap().to_string());
        Self { pair, public }
    }

    /// 签名文件的内容：和 `tauri signer sign --app-version` 写出来的一样。
    fn signature(&self, bytes: &[u8], version: &str) -> String {
        let comment = format!("timestamp:1700000000\tfile:app\tversion:{version}");
        let signature = minisign::sign(
            None,
            &self.pair.sk,
            bytes,
            Some(comment.as_str()),
            Some("signature from tauri secret key"),
        )
        .unwrap();
        base64::engine::general_purpose::STANDARD.encode(signature.to_string())
    }
}

const WINDOWS: &str = "windows-x86_64";
const MAC: &str = "darwin-aarch64";

/// 一个待发布的目录：清单，加上两个平台的包和它们的签名。`mac` 为 `None` 时只发 Windows。
fn release(directory: &Path, keys: &Keys, version: &str, mac: bool) -> PathBuf {
    std::fs::create_dir_all(directory).unwrap();
    let windows_file = format!("Assemble_{version}_x64-setup.exe");
    let windows = format!("windows installer {version}").into_bytes();
    std::fs::write(directory.join(&windows_file), &windows).unwrap();
    std::fs::write(
        directory.join(format!("{windows_file}.sig")),
        keys.signature(&windows, version),
    )
    .unwrap();
    let mut platforms = vec![format!(
        "{{\"platform\":\"{WINDOWS}\",\"file\":\"{windows_file}\"}}"
    )];
    if mac {
        let mac_file = format!("Assemble_{version}_aarch64.app.tar.gz");
        let mac_bytes = format!("mac bundle {version}").into_bytes();
        std::fs::write(directory.join(&mac_file), &mac_bytes).unwrap();
        std::fs::write(
            directory.join(format!("{mac_file}.sig")),
            keys.signature(&mac_bytes, version),
        )
        .unwrap();
        platforms.push(format!(
            "{{\"platform\":\"{MAC}\",\"file\":\"{mac_file}\"}}"
        ));
    }
    std::fs::write(
        directory.join("manifest.json"),
        format!(
            r#"{{"version":"{version}","notes":"","pubDate":"2026-11-27T01:30:00Z","platforms":[{}]}}"#,
            platforms.join(",")
        ),
    )
    .unwrap();
    directory.to_path_buf()
}

async fn server(data_dir: &Path) -> TestServer {
    let pool = db::open(data_dir).await.unwrap();
    let mut config = Config::development(data_dir);
    // 限速要按来源分，得先告诉服务端哪个请求头可信。
    config.client_ip_header = Some(axum::http::HeaderName::from_static("x-real-ip"));
    TestServer::start(config, pool).await.unwrap()
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

#[tokio::test]
async fn nothing_is_served_before_a_release() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let mut server = server(&data).await;
    let origin = server.origin.as_str();

    let response = client()
        .get(format!("{origin}/updates/latest.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    // 什么都还没发布：这个回答不能被留在缓存里，发布之后要立刻读得到。
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body = response.text().await.unwrap();
    assert!(
        body.starts_with(r#"{"error":{"code":"NOT_FOUND""#),
        "{body}"
    );

    let response = client()
        .get(format!(
            "{origin}/updates/{WINDOWS}/0.1.1/Assemble_0.1.1_x64-setup.exe"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers()["cache-control"], "no-store");
    server.stop().await;
}

#[tokio::test]
async fn a_release_is_checked_before_it_is_published_and_then_served() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let keys = Keys::new();
    let staging = release(&dir.path().join("staging"), &keys, "0.1.1000", true);
    let mut server = server(&data).await;
    let origin = server.origin.clone();
    let origin_text = origin.as_str();

    // 缺省只校验：什么都没发布。
    let report = updates::publish(&data, &origin, &staging, &keys.public, false)
        .await
        .unwrap();
    assert!(!report.applied);
    assert_eq!(report.version, "0.1.1000");
    assert_eq!(report.platforms.len(), 2);
    assert!(!data.join("updates").join("latest.json").exists());

    // 包被改过一个字节：签名对不上，发不出去。
    let file = staging.join("Assemble_0.1.1000_x64-setup.exe");
    let original = std::fs::read(&file).unwrap();
    std::fs::write(&file, b"windows installer 0.1.1001").unwrap();
    let refused = updates::publish(&data, &origin, &staging, &keys.public, true)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), "UPDATE_SIGNATURE_INVALID");
    std::fs::write(&file, &original).unwrap();

    // 不是这一把钥匙签的：公钥换成另一把。
    let other = Keys::new();
    let foreign = updates::publish(&data, &origin, &staging, &other.public, true)
        .await
        .unwrap_err();
    assert_eq!(foreign.code(), "UPDATE_SIGNATURE_INVALID");

    // 目录里多了个文件。
    std::fs::write(staging.join("notes.txt"), "stray").unwrap();
    let stray = updates::publish(&data, &origin, &staging, &keys.public, true)
        .await
        .unwrap_err();
    assert_eq!(stray.code(), "UPDATE_UNEXPECTED_FILE");
    std::fs::remove_file(staging.join("notes.txt")).unwrap();
    assert!(!data.join("updates").exists());

    // 发布。
    let report = updates::publish(&data, &origin, &staging, &keys.public, true)
        .await
        .unwrap();
    assert!(report.applied);
    assert!(report.removed.is_empty());
    let windows = &report.platforms[WINDOWS];
    assert_eq!(windows.bytes, b"windows installer 0.1.1000".len() as u64);
    assert_eq!(windows.sha256.len(), 64);

    let client = client();
    let response = client
        .get(format!("{origin_text}/updates/latest.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/json");
    assert_eq!(response.headers()["cache-control"], "public, no-cache");
    let etag = response.headers()["etag"].to_str().unwrap().to_owned();
    let manifest: updates::Manifest = response.json().await.unwrap();
    assert_eq!(manifest.version, "0.1.1000");
    assert_eq!(manifest.pub_date, "2026-11-27T01:30:00Z");
    assert_eq!(manifest.platforms.len(), 2);
    let entry = &manifest.platforms[WINDOWS];
    assert_eq!(
        entry.url,
        format!("{origin_text}/updates/{WINDOWS}/0.1.1000/Assemble_0.1.1000_x64-setup.exe")
    );
    // 签名每次签都不一样（里面有随机的数），所以和发布目录里那一份比，不是重新签一份来比。
    assert_eq!(
        entry.signature,
        std::fs::read_to_string(staging.join("Assemble_0.1.1000_x64-setup.exe.sig")).unwrap()
    );

    // 没变的话回 304，不重传正文。
    let unchanged = client
        .get(format!("{origin_text}/updates/latest.json"))
        .header("if-none-match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(unchanged.status(), StatusCode::NOT_MODIFIED);

    // 更新包：原样交出去，可以只取一段，可以长留。
    let response = client.get(&entry.url).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "application/octet-stream"
    );
    assert_eq!(
        response.headers()["cache-control"],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert!(
        response.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains("Assemble_0.1.1000_x64-setup.exe")
    );
    assert_eq!(
        &response.bytes().await.unwrap()[..],
        &b"windows installer 0.1.1000"[..]
    );

    let response = client
        .get(&entry.url)
        .header("range", "bytes=0-6")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(&response.bytes().await.unwrap()[..], &b"windows"[..]);

    // 路径上做手脚都到不了磁盘。（带 `..` 的在这里试不出来：客户端发出去之前就把它规整掉了，
    // 那一步由上面的单元测试守着。）
    for path in [
        "/updates/latest.json/extra",
        "/updates/linux-x86_64/0.1.1000/Assemble_0.1.1000_x64-setup.exe",
        "/updates/windows-x86_64/latest/Assemble_0.1.1000_x64-setup.exe",
        "/updates/windows-x86_64/0.1.1001/Assemble_0.1.1000_x64-setup.exe",
    ] {
        let response = client
            .get(format!("{origin_text}{path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    server.stop().await;
}

#[tokio::test]
async fn only_a_newer_release_replaces_the_published_one() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let keys = Keys::new();
    let mut server = server(&data).await;
    let origin = server.origin.clone();

    let first = release(&dir.path().join("a"), &keys, "0.1.1000", true);
    updates::publish(&data, &origin, &first, &keys.public, true)
        .await
        .unwrap();

    // 同一个版本再发一次，或者发一个更老的：都不收。
    let same = release(&dir.path().join("b"), &keys, "0.1.1000", true);
    assert_eq!(
        updates::publish(&data, &origin, &same, &keys.public, true)
            .await
            .unwrap_err()
            .code(),
        "UPDATE_VERSION_NOT_NEWER"
    );
    let older = release(&dir.path().join("c"), &keys, "0.1.999", true);
    assert_eq!(
        updates::publish(&data, &origin, &older, &keys.public, true)
            .await
            .unwrap_err()
            .code(),
        "UPDATE_VERSION_NOT_NEWER"
    );

    // 已经发了两个平台，这一版少了 Mac：Mac 上的电脑会被反复要求"更新"到同一个版本。
    let windows_only = release(&dir.path().join("d"), &keys, "0.1.1001", false);
    assert_eq!(
        updates::publish(&data, &origin, &windows_only, &keys.public, true)
            .await
            .unwrap_err()
            .code(),
        "UPDATE_PLATFORM_MISSING"
    );

    // 签名是为别的版本签的。
    let mislabelled = release(&dir.path().join("e"), &keys, "0.1.1001", true);
    let manifest = std::fs::read_to_string(mislabelled.join("manifest.json")).unwrap();
    std::fs::write(
        mislabelled.join("manifest.json"),
        manifest.replace("\"version\":\"0.1.1001\"", "\"version\":\"0.1.1002\""),
    )
    .unwrap();
    assert_eq!(
        updates::publish(&data, &origin, &mislabelled, &keys.public, true)
            .await
            .unwrap_err()
            .code(),
        "UPDATE_SIGNED_VERSION_MISMATCH"
    );

    // 合法的新版本：清单换掉，旧版本的目录删掉。
    let second = release(&dir.path().join("f"), &keys, "0.1.1001", true);
    let report = updates::publish(&data, &origin, &second, &keys.public, true)
        .await
        .unwrap();
    let mut removed = report.removed.clone();
    removed.sort();
    assert_eq!(
        removed,
        vec![format!("{MAC}/0.1.1000"), format!("{WINDOWS}/0.1.1000")]
    );
    let updates = data.join("updates");
    assert!(!updates.join(WINDOWS).join("0.1.1000").exists());
    assert!(!updates.join(MAC).join("0.1.1000").exists());
    assert!(
        updates
            .join(MAC)
            .join("0.1.1001")
            .join("Assemble_0.1.1001_aarch64.app.tar.gz")
            .is_file()
    );
    let manifest: updates::Manifest =
        serde_json::from_slice(&std::fs::read(updates.join("latest.json")).unwrap()).unwrap();
    assert_eq!(manifest.version, "0.1.1001");
    assert_eq!(manifest.platforms.len(), 2);
    // 磁盘上写着的清单和接口交出去的是同一份。
    let served = client()
        .get(format!("{}/updates/latest.json", origin.as_str()))
        .send()
        .await
        .unwrap()
        .json::<updates::Manifest>()
        .await
        .unwrap();
    assert_eq!(served, manifest);
    server.stop().await;
}

#[tokio::test]
async fn the_public_endpoints_are_rate_limited_per_source() {
    let dir = tempfile::tempdir().unwrap();
    let mut server = server(&dir.path().join("data")).await;
    let origin = server.origin.as_str();
    let client = client();
    let url = format!("{origin}/updates/latest.json");

    // 来源未知的不限速：没有配置可信的请求头时，不能让一个乱试的人挡住所有人。
    for _ in 0..130 {
        let response = client.get(&url).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    let mut limited = None;
    for _ in 0..121 {
        limited = Some(
            client
                .get(&url)
                .header("x-real-ip", "203.0.113.7")
                .send()
                .await
                .unwrap(),
        );
    }
    let limited = limited.unwrap();
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.headers()["retry-after"], "3600");
    let body = limited.text().await.unwrap();
    assert!(body.contains(r#""TOO_MANY_ATTEMPTS""#), "{body}");

    // 别的来源不受影响；安装包那个接口有自己的额度。
    let other = client
        .get(&url)
        .header("x-real-ip", "203.0.113.8")
        .send()
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::NOT_FOUND);
    let artifact = client
        .get(format!(
            "{origin}/updates/{WINDOWS}/0.1.1000/Assemble_0.1.1000_x64-setup.exe"
        ))
        .header("x-real-ip", "203.0.113.7")
        .send()
        .await
        .unwrap();
    assert_eq!(artifact.status(), StatusCode::NOT_FOUND);
    server.stop().await;
}

/// 发布目录拿给真正的可执行文件：它用的是编进服务端的公钥，测试钥匙签的包过不了它这一关。
fn cli(data_dir: &Path, arguments: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_assemble-server"));
    command.env_clear();
    for key in ["SystemRoot", "WINDIR", "TMP", "TEMP"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env("ASSEMBLE_DATA_DIR", data_dir)
        .env("ASSEMBLE_PUBLIC_ORIGIN", "http://127.0.0.1:8787")
        .args(arguments)
        .output()
        .unwrap()
}

#[test]
fn the_command_line_publishes_only_what_the_embedded_key_accepts() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let missing = cli(&data, &["publish-update", "/no/such/directory"]);
    assert!(!missing.status.success());
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(stderr.contains("UPDATE_MANIFEST_UNREADABLE"), "{stderr}");

    let keys = Keys::new();
    let staging = release(&dir.path().join("staging"), &keys, "0.1.1000", true);
    // 测试钥匙不是编进服务端的那一把：子命令认得出签名对不上，什么都没发出去。
    let refused = cli(
        &data,
        &["publish-update", staging.to_str().unwrap(), "--apply"],
    );
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("UPDATE_SIGNATURE_INVALID"), "{stderr}");
    assert!(!data.join("updates").join("latest.json").exists());
    assert!(String::from_utf8_lossy(&refused.stdout).is_empty());
}
