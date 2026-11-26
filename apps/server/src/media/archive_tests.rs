use super::*;
use assemble_protocol::media::*;
use serde_json::json;
use std::io::Cursor;
use std::sync::Arc;

async fn bundle() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    tokio::fs::create_dir(directory.path().join("files"))
        .await
        .unwrap();
    let mut image = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(4, 6)
        .write_to(&mut image, image::ImageFormat::Png)
        .unwrap();
    let bytes = image.into_inner();
    let sha = hash(&bytes);
    tokio::fs::write(directory.path().join("files").join(&sha), &bytes)
        .await
        .unwrap();
    let archive = b"original source archive";
    let archive_sha = hash(archive);
    tokio::fs::write(directory.path().join("files").join(&archive_sha), archive)
        .await
        .unwrap();
    let entry = json!({"id":"movie-one","version":7,"scope":"shared","mediaType":"movie","title":"原电影","originalTitle":"Original","year":2020,"releaseDate":null,"overview":"保留简介","genres":["剧情"],"episodeCount":null,"status":"completed","watchedAt":"2026-11-25","review":"原观后感","notes":"仅管理员可读","magnetUri":"magnet:?xt=urn:btih:0123456789012345678901234567890123456789","cast":[],"identities":[{"provider":"tmdb","mediaType":"movie","externalId":"42"}],"poster":{"assetId":"image-one","width":4,"height":6,"contentType":"image/png"},"magnetAvailable":true,"createdAt":"2020-01-01T00:00:00.000Z","updatedAt":"2026-11-25T00:00:00.000Z"});
    let mut cached = entry.clone();
    cached["id"] = json!("cached-one");
    cached["status"] = json!("planned");
    cached["notes"] = Value::Null;
    cached["review"] = Value::Null;
    cached["watchedAt"] = Value::Null;
    cached["magnetUri"] = Value::Null;
    cached["magnetAvailable"] = json!(false);
    cached["identities"][0]["externalId"] = json!("43");
    let manifest = json!({"version":1,"source":"jarvis-media","capturedAt":1795600000000_i64,"sourceTables":{"media_entries":[{}],"media_jobs":[{"id":"old-job","status":"completed"}]},"files":[{"key":"assets/original.png","sha256":sha,"size":bytes.len()},{"key":"archives/source.json","sha256":archive_sha,"size":archive.len()}],"assets":[{"id":"image-one","sha256":sha,"createdAt":1,"variants":{"original":sha}}],"entries":[{"entry":entry,"gallery":["image-one"]}],"collections":[{"id":"collection-one","version":3,"title":"原合集","description":"说明","coverEntryId":"movie-one","entryIds":["movie-one"],"createdAt":1,"updatedAt":2}],"catalog":[{"entry":cached,"expiresAt":1}]});
    tokio::fs::write(
        directory.path().join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .await
    .unwrap();
    directory
}
async fn change(directory: &Path, mutate: impl FnOnce(&mut Value)) {
    let file = directory.join("manifest.json");
    let mut value: Value = serde_json::from_slice(&tokio::fs::read(&file).await.unwrap()).unwrap();
    mutate(&mut value);
    tokio::fs::write(file, serde_json::to_vec(&value).unwrap())
        .await
        .unwrap();
}
async fn count(pool: &SqlitePool, table: &str) -> i64 {
    let query = match table {
        "accounts" => "SELECT count(*) FROM accounts",
        "media_entries" => "SELECT count(*) FROM media_entries",
        "media_import_runs" => "SELECT count(*) FROM media_import_runs",
        "media_import_files" => "SELECT count(*) FROM media_import_files",
        "media_assets" => "SELECT count(*) FROM media_assets",
        _ => panic!("unexpected test table"),
    };
    sqlx::query_scalar(query).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn import_preserves_history_covers_cache_and_original_archives_without_touching_accounts() {
    let source = bundle().await;
    let target = tempfile::tempdir().unwrap();
    let pool = crate::db::open(target.path()).await.unwrap();
    let inspected = inspect(source.path()).await.unwrap();
    assert_eq!(inspected.source_files, 2);
    let report = import(&pool, source.path()).await.unwrap();
    assert!(!report.already_imported);
    assert_eq!(count(&pool, "accounts").await, 0);
    assert_eq!(count(&pool, "media_entries").await, 1);
    assert_eq!(count(&pool, "media_import_files").await, 2);
    let hashes: Vec<String> =
        sqlx::query_scalar("SELECT sha256 FROM media_asset_variants ORDER BY variant")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(hashes.len(), 4);
    assert!(hashes.iter().all(|sha| sha == &hashes[0]));
    let stored: String = sqlx::query_scalar("SELECT data FROM media_entries")
        .fetch_one(&pool)
        .await
        .unwrap();
    let entry: MediaEntry = super::super::decode(&stored).unwrap();
    assert_eq!(entry.version, 7);
    assert_eq!(entry.content.watched_at.as_deref(), Some("2026-11-25"));
    assert_eq!(entry.content.review.as_deref(), Some("原观后感"));
    let anonymous = store::load(&pool, MediaScope::Shared, None, "movie-one")
        .await
        .unwrap();
    let anonymous = store::project(anonymous, None);
    assert!(anonymous.content.notes.is_none());
    assert!(anonymous.content.magnet_uri.is_none());
    assert!(anonymous.magnet_available);
    let collection = super::super::collections::get(&pool, None, "collection-one")
        .await
        .unwrap();
    assert_eq!(collection.poster.unwrap().asset_id, "image-one");
    assert_eq!(collection.version, 3);
    let identity = MediaIdentity {
        provider: MediaProvider::Tmdb,
        media_type: MediaType::Movie,
        external_id: "43".into(),
    };
    assert!(cached(&pool, &identity).await.unwrap().is_some());
    assets::cleanup(&pool).await.unwrap();
    assert_eq!(count(&pool, "media_assets").await, 1);
    let config = crate::config::Config::from_lookup(|key| match key {
        "ASSEMBLE_DATA_DIR" => Some(target.path().to_string_lossy().into_owned()),
        "ASSEMBLE_PUBLIC_ORIGIN" => Some("http://127.0.0.1:8787".into()),
        _ => None,
    })
    .unwrap();
    let state = Arc::new(crate::http::AppState::new(pool.clone(), config));
    let result = state
        .media
        .catalog
        .details(&state, &axum::http::HeaderMap::new(), &identity)
        .await
        .unwrap();
    assert_eq!(result.title, "原电影");
    assert_eq!(
        assets::content(&state, None, "image-one", "360")
            .await
            .unwrap()
            .status(),
        axum::http::StatusCode::OK
    );
    assert!(import(&pool, source.path()).await.unwrap().already_imported);
}

#[tokio::test]
async fn supplied_thumbnails_are_preserved_and_missing_small_sizes_reuse_original_bytes() {
    let source = bundle().await;
    let mut cursor = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        2,
        3,
        image::Rgba([1, 2, 3, 255]),
    ))
    .write_to(&mut cursor, image::ImageFormat::Png)
    .unwrap();
    let bytes = cursor.into_inner();
    let sha = hash(&bytes);
    tokio::fs::write(source.path().join("files").join(&sha), &bytes)
        .await
        .unwrap();
    change(source.path(), |value| {
        value["files"]
            .as_array_mut()
            .unwrap()
            .push(json!({"key":"assets/thumb.png","sha256":sha,"size":bytes.len()}));
        value["assets"][0]["variants"]["200"] = json!(sha);
    })
    .await;
    let target = tempfile::tempdir().unwrap();
    let pool = crate::db::open(target.path()).await.unwrap();
    import(&pool, source.path()).await.unwrap();
    let saved: Vec<u8> =
        sqlx::query_scalar("SELECT content FROM media_asset_variants WHERE variant='200'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(saved, bytes);
    let distinct: i64 = sqlx::query_scalar("SELECT count(DISTINCT sha256) FROM media_asset_variants WHERE variant IN ('original','360','480')")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(distinct, 1);
}

#[tokio::test]
async fn corrupted_files_credentials_private_data_and_foreign_covers_abort_before_publishing() {
    let target = tempfile::tempdir().unwrap();
    let pool = crate::db::open(target.path()).await.unwrap();
    for kind in [
        "private",
        "credential",
        "cover",
        "image-reference",
        "duplicate",
        "non-media-table",
        "nested-credential",
        "unlinked-poster",
        "original-variant",
    ] {
        let source = bundle().await;
        change(source.path(), |value| match kind {
            "private" => value["entries"][0]["entry"]["scope"] = json!("private"),
            "credential" => {
                value["sourceTables"]["media_jobs"][0]["session_id"] =
                    json!("synthetic-forbidden-session")
            }
            "cover" => value["collections"][0]["coverEntryId"] = json!("another-entry"),
            "image-reference" => value["entries"][0]["entry"]["poster"]["width"] = json!(500),
            "non-media-table" => value["sourceTables"]["accounts"] = json!([]),
            "nested-credential" => {
                value["sourceTables"]["media_jobs"][0]["nested"] = json!({"token":"synthetic"})
            }
            "unlinked-poster" => value["entries"][0]["gallery"] = json!([]),
            "original-variant" => {
                value["assets"][0]["variants"]["original"] = value["files"][1]["sha256"].clone()
            }
            _ => {
                let item = value["entries"][0].clone();
                value["entries"].as_array_mut().unwrap().push(item);
            }
        })
        .await;
        assert!(import(&pool, source.path()).await.is_err(), "{kind}");
        assert_eq!(count(&pool, "media_import_runs").await, 0);
        assert_eq!(count(&pool, "media_assets").await, 0);
    }
    let source = bundle().await;
    let value: Value = serde_json::from_slice(
        &tokio::fs::read(source.path().join("manifest.json"))
            .await
            .unwrap(),
    )
    .unwrap();
    let file = value["files"][0]["sha256"].as_str().unwrap();
    tokio::fs::write(source.path().join("files").join(file), b"bad")
        .await
        .unwrap();
    assert!(import(&pool, source.path()).await.is_err());
    assert_eq!(count(&pool, "media_entries").await, 0);
}

#[tokio::test]
async fn another_snapshot_never_overwrites_an_existing_library_and_orphans_stay_unreadable() {
    let first = bundle().await;
    let target = tempfile::tempdir().unwrap();
    let pool = crate::db::open(target.path()).await.unwrap();
    import(&pool, first.path()).await.unwrap();
    let second = bundle().await;
    change(second.path(), |value| {
        value["capturedAt"] = json!(1795600000001_i64)
    })
    .await;
    assert_eq!(
        import(&pool, second.path()).await.unwrap_err().code,
        "MEDIA_IMPORT_TARGET_NOT_EMPTY"
    );
    sqlx::query("DELETE FROM media_import_catalog_assets")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM media_entry_assets")
        .execute(&pool)
        .await
        .unwrap();
    assets::cleanup(&pool).await.unwrap();
    assert_eq!(count(&pool, "media_assets").await, 1);
    let config = crate::config::Config::from_lookup(|key| match key {
        "ASSEMBLE_DATA_DIR" => Some(target.path().to_string_lossy().into_owned()),
        "ASSEMBLE_PUBLIC_ORIGIN" => Some("http://127.0.0.1:8787".into()),
        _ => None,
    })
    .unwrap();
    let state = Arc::new(crate::http::AppState::new(pool.clone(), config));
    assert_eq!(
        assets::content(&state, None, "image-one", "original")
            .await
            .unwrap_err()
            .code,
        "NOT_FOUND"
    );
}
