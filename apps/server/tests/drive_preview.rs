//! 真服务预览：实际内容识别、账户隔离、管理员来源保护与分段读取。
mod common;

use assemble_client::{Client, OutgoingFile, TransferOptions};
use assemble_protocol::preview::{FilePreview, PreviewKind, TEXT_PREVIEW_BYTES};
use assemble_protocol::primitives::Platform;
use common::*;
use reqwest::StatusCode;

async fn upload(
    f: &Fixture,
    device: &Registered,
    bytes: &[u8],
    name: &str,
) -> assemble_protocol::drive::DriveFile {
    let path = f.dir.path().join("preview-source");
    tokio::fs::write(&path, bytes).await.unwrap();
    let file = OutgoingFile::open(&path, &device.cancel).await.unwrap();
    device
        .drive_upload(name, &file, TransferOptions::new(&device.cancel), None)
        .await
        .unwrap()
}
fn png() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(3, 2)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

#[tokio::test]
async fn real_images_ignore_names_and_declared_mime_and_support_ranges() {
    let mut f = fixture().await;
    let device = register(&f, 'a', "preview", Platform::Windows).await;
    let bytes = png();
    let file = upload(&f, &device, &bytes, "伪装<script>.html").await;
    sqlx::query("UPDATE drive_files SET mime_type = 'text/html' WHERE id = ?")
        .bind(&file.id)
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    let preview = device
        .drive_preview(&file.id, &device.options())
        .await
        .unwrap();
    assert!(preview.valid());
    assert_eq!(preview.info.kind, PreviewKind::Image);
    assert_eq!(preview.info.mime_type.as_deref(), Some("image/png"));
    let chunk = device
        .drive_preview_chunk(&file.id, 3, 20, file.size, &device.options())
        .await
        .unwrap();
    assert_eq!(chunk.bytes, bytes[3..=20]);
    let response = reqwest::Client::new()
        .get(format!(
            "{}/v1/drive/files/{}/preview/content",
            f.browser.origin, file.id
        ))
        .bearer_auth(device.token.expose())
        .header("range", "bytes=0-4")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()["content-disposition"], "inline");
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    let json = serde_json::to_string(&preview).unwrap();
    for secret in ["token", "path", "authorization", "sha256", "src"] {
        assert!(!json.contains(secret));
    }
    f.server.stop().await;
}

#[tokio::test]
async fn text_is_plain_data_and_truncates_at_a_complete_utf8_character() {
    let mut f = fixture().await;
    let device = register(&f, 'a', "preview", Platform::Windows).await;
    let text = "<script>alert('中文')</script>🌙".repeat(60_000);
    let file = upload(&f, &device, text.as_bytes(), "正文.txt").await;
    let preview = device
        .drive_preview(&file.id, &device.options())
        .await
        .unwrap();
    assert_eq!(preview.info.kind, PreviewKind::Text);
    assert!(preview.truncated);
    let excerpt = preview.text.unwrap();
    assert!(excerpt.len() <= TEXT_PREVIEW_BYTES);
    assert!(text.starts_with(&excerpt));
    let response = reqwest::Client::new()
        .get(format!(
            "{}/v1/drive/files/{}/preview/content",
            f.browser.origin, file.id
        ))
        .bearer_auth(device.token.expose())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    f.server.stop().await;
}

#[tokio::test]
async fn corrupt_images_and_binary_files_are_not_rendered_as_media() {
    let mut f = fixture().await;
    let device = register(&f, 'a', "preview", Platform::Windows).await;
    let mut oversized = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(8193, 1)
        .write_to(&mut oversized, image::ImageFormat::Png)
        .unwrap();
    let file = upload(&f, &device, oversized.get_ref(), "wide.png").await;
    assert_eq!(
        device
            .drive_preview(&file.id, &device.options())
            .await
            .unwrap()
            .info
            .kind,
        PreviewKind::Unsupported
    );
    for bytes in [
        b"\x89PNG\r\n\x1a\ninvalid".as_slice(),
        b"PK\x03\x04\0binary".as_slice(),
        b"RIFFbad\0".as_slice(),
    ] {
        let file = upload(&f, &device, bytes, "picture.png").await;
        let preview = device
            .drive_preview(&file.id, &device.options())
            .await
            .unwrap();
        assert_eq!(preview.info.kind, PreviewKind::Unsupported);
        assert!(preview.valid());
    }
    f.server.stop().await;
}

#[tokio::test]
async fn preview_rechecks_owner_and_deletion_even_after_format_is_cached() {
    let mut f = fixture().await;
    let device = register(&f, 'a', "preview", Platform::Windows).await;
    let file = upload(&f, &device, &png(), "image.png").await;
    device
        .drive_preview(&file.id, &device.options())
        .await
        .unwrap();
    let other = assemble_server::accounts::create(
        &f.server.state.pool,
        "other",
        "Other",
        assemble_server::SecretString::new(PASSWORD),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE drive_files SET account_id = ? WHERE id = ?")
        .bind(&other.id)
        .bind(&file.id)
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(
        device
            .drive_preview(&file.id, &device.options())
            .await
            .unwrap_err()
            .status(),
        Some(404)
    );
    let anonymous = Client::new(f.server.origin.clone()).unwrap();
    assert!(
        anonymous
            .drive_preview(&file.id, &device.options())
            .await
            .is_err()
    );
    let path = format!("/v1/admin/drive/files/{}/preview", file.id);
    assert_eq!(
        f.browser
            .get(&path)
            .header("origin", &f.browser.origin)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assemble_server::admin::grant_super_admin(&f.server.state.pool, EMAIL)
        .await
        .unwrap();
    let preview: FilePreview = f
        .browser
        .get(&path)
        .header("sec-fetch-site", "same-origin")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(preview.info.kind, PreviewKind::Image);
    for suffix in ["", "/content"] {
        for (origin, site) in [
            (None, None),
            (None, Some("cross-site")),
            (None, Some("same-site")),
            (Some("https://foreign.example"), Some("same-origin")),
        ] {
            let mut request = f.browser.get(&format!("{path}{suffix}"));
            if let Some(value) = origin {
                request = request.header("origin", value);
            }
            if let Some(value) = site {
                request = request.header("sec-fetch-site", value);
            }
            let response = request.send().await.unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(response.headers()["cache-control"], "no-store");
        }
        let response = reqwest::get(format!("{}{path}{suffix}", f.browser.origin))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    sqlx::query("DELETE FROM drive_files WHERE id = ?")
        .bind(&file.id)
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(
        f.browser
            .get(&path)
            .header("origin", &f.browser.origin)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    f.server.stop().await;
}

#[tokio::test]
async fn real_wav_is_classified_from_its_streams() {
    let mut f = fixture().await;
    let device = register(&f, 'a', "preview", Platform::Windows).await;
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&1036_u32.to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&8000_u32.to_le_bytes());
    bytes.extend_from_slice(&16000_u32.to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&1000_u32.to_le_bytes());
    bytes.extend_from_slice(&[0; 1000]);
    let file = upload(&f, &device, &bytes, "audio.dat").await;
    let preview = device
        .drive_preview(&file.id, &device.options())
        .await
        .unwrap();
    // 开发环境没有 ffprobe 时明确不可预览；正式镜像必须安装并实际验证。
    if std::process::Command::new("ffprobe")
        .arg("-version")
        .output()
        .is_ok()
    {
        assert_eq!(preview.info.kind, PreviewKind::Audio);
    } else {
        assert_eq!(preview.info.kind, PreviewKind::Unsupported);
    }
    f.server.stop().await;
}
