//! 强区间能力仍在原云盘权限边界内，撤销访问不能借 `ETag` 或检查点绕过。
mod common;
use assemble_client::{OutgoingFile, TransferOptions};
use assemble_protocol::primitives::Platform;
use common::*;
use reqwest::{StatusCode, header};

#[tokio::test]
async fn strong_ranges_metadata_and_final_access_are_checked_against_real_server() {
    let mut fixture = fixture().await;
    let pc = register(&fixture, 'r', "Range PC", Platform::Windows).await;
    let bytes = vec![41_u8; 2 * 1024 * 1024 + 7];
    let source = fixture.dir.path().join("source.bin");
    tokio::fs::write(&source, &bytes).await.unwrap();
    let file = OutgoingFile::open(&source, &pc.cancel).await.unwrap();
    let stored = pc
        .drive_upload(
            "中文资料.bin",
            &file,
            TransferOptions::new(&pc.cancel),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        pc.drive_metadata(&stored.id, &pc.options()).await.unwrap(),
        stored
    );
    let mut recovered = Vec::new();
    while recovered.len() < bytes.len() {
        recovered.extend(
            pc.drive_chunk(&stored, recovered.len() as u64, &pc.options())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    assert_eq!(recovered, bytes);
    let http = reqwest::Client::new();
    let url = format!(
        "{}/v1/drive/files/{}/content",
        fixture.server.origin.as_str(),
        stored.id
    );
    let expected = format!("\"{}\"", stored.sha256);
    for (validator, status) in [
        (expected.as_str(), StatusCode::PARTIAL_CONTENT),
        ("W/\"different\"", StatusCode::OK),
        ("\"different\"", StatusCode::OK),
    ] {
        let response = http
            .get(&url)
            .bearer_auth(pc.token.expose())
            .header(header::RANGE, "bytes=3-5")
            .header(header::IF_RANGE, validator)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[header::ETAG], expected);
        if status == StatusCode::PARTIAL_CONTENT {
            assert_eq!(
                response.headers()[header::CONTENT_RANGE],
                format!("bytes 3-5/{}", stored.size)
            );
            assert_eq!(response.bytes().await.unwrap().len(), 3);
        }
    }
    assert_eq!(
        http.get(&url)
            .header(header::RANGE, "bytes=3-5")
            .header(header::IF_RANGE, &expected)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    pc.drive_delete(&stored.id, &pc.options()).await.unwrap();
    assert_eq!(
        pc.drive_metadata(&stored.id, &pc.options())
            .await
            .unwrap_err()
            .status(),
        Some(404)
    );
    assert_eq!(
        pc.drive_chunk(&stored, 0, &pc.options())
            .await
            .unwrap_err()
            .status(),
        Some(404)
    );
    fixture.server.stop().await;
}
