-- 外部影视快照仅由受保护的维护命令导入；原始业务记录和文件不经网页接口公开。
CREATE TABLE media_import_runs (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    captured_at INTEGER NOT NULL,
    imported_at INTEGER NOT NULL,
    manifest_json TEXT NOT NULL CHECK(json_valid(manifest_json))
) STRICT;
CREATE TABLE media_import_files (
    run_id TEXT NOT NULL REFERENCES media_import_runs(id),
    source_key TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    size INTEGER NOT NULL CHECK(size >= 0),
    content BLOB NOT NULL,
    PRIMARY KEY(run_id, source_key),
    CHECK(length(content) = size)
) STRICT;
CREATE TABLE media_import_asset_refs (
    run_id TEXT NOT NULL REFERENCES media_import_runs(id),
    asset_id TEXT NOT NULL REFERENCES media_assets(id),
    PRIMARY KEY(run_id, asset_id)
) STRICT;
CREATE TABLE media_import_catalog (
    provider TEXT NOT NULL,
    media_type TEXT NOT NULL,
    external_id TEXT NOT NULL,
    run_id TEXT NOT NULL REFERENCES media_import_runs(id),
    data TEXT NOT NULL CHECK(json_valid(data)),
    source_expires_at INTEGER NOT NULL,
    PRIMARY KEY(provider, media_type, external_id)
) STRICT;
CREATE TABLE media_import_catalog_assets (
    run_id TEXT NOT NULL REFERENCES media_import_runs(id),
    asset_id TEXT NOT NULL REFERENCES media_assets(id),
    PRIMARY KEY(run_id, asset_id)
) STRICT;
ALTER TABLE media_collections ADD COLUMN cover_entry_id TEXT REFERENCES media_entries(id) ON DELETE SET NULL;
