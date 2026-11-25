-- 资料与观看记录共同受所属范围约束；私人范围必须有所有者。
CREATE TABLE media_entries (
    id TEXT PRIMARY KEY,
    scope TEXT NOT NULL CHECK(scope IN ('shared','private')),
    owner_id TEXT REFERENCES accounts(id) ON DELETE CASCADE,
    version INTEGER NOT NULL CHECK(version > 0),
    data TEXT NOT NULL CHECK(json_valid(data)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK((scope = 'shared' AND owner_id IS NULL) OR (scope = 'private' AND owner_id IS NOT NULL))
) STRICT;
CREATE INDEX media_entries_owner ON media_entries(scope, owner_id, updated_at DESC);
CREATE TABLE media_identities (
    entry_id TEXT NOT NULL REFERENCES media_entries(id) ON DELETE CASCADE,
    scope_key TEXT NOT NULL,
    provider TEXT NOT NULL,
    media_type TEXT NOT NULL,
    external_id TEXT NOT NULL,
    PRIMARY KEY(scope_key, provider, media_type, external_id)
) STRICT;
-- 图像内容及所有衍生版本同一事务发布，外部调用方永远拿不到内部路径。
CREATE TABLE media_assets (
    id TEXT PRIMARY KEY,
    owner_key TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    width INTEGER NOT NULL CHECK(width > 0),
    height INTEGER NOT NULL CHECK(height > 0),
    content_type TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(owner_key, sha256)
) STRICT;
CREATE TABLE media_asset_variants (
    asset_id TEXT NOT NULL REFERENCES media_assets(id) ON DELETE CASCADE,
    variant TEXT NOT NULL CHECK(variant IN ('original','200','360','480')),
    content_type TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    size INTEGER NOT NULL CHECK(size > 0),
    content BLOB NOT NULL,
    PRIMARY KEY(asset_id, variant),
    CHECK(length(content) = size)
) STRICT;
CREATE TABLE media_entry_assets (
    entry_id TEXT NOT NULL REFERENCES media_entries(id) ON DELETE CASCADE,
    asset_id TEXT NOT NULL REFERENCES media_assets(id),
    role TEXT NOT NULL CHECK(role IN ('gallery','cast')),
    position INTEGER NOT NULL,
    PRIMARY KEY(entry_id, asset_id, role)
) STRICT;
CREATE TABLE media_catalog_assets (
    asset_id TEXT PRIMARY KEY REFERENCES media_assets(id) ON DELETE CASCADE,
    expires_at INTEGER NOT NULL
) STRICT;
CREATE INDEX media_entry_assets_asset ON media_entry_assets(asset_id, entry_id);
CREATE TABLE media_mutations (
    actor_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    request_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    PRIMARY KEY(actor_id, request_id)
) STRICT;
CREATE TABLE media_jobs (
    id TEXT PRIMARY KEY,
    actor_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    owner_key TEXT NOT NULL,
    credential_kind TEXT NOT NULL CHECK(credential_kind IN ('web','device')),
    credential_id TEXT NOT NULL,
    data TEXT NOT NULL CHECK(json_valid(data)),
    input TEXT NOT NULL CHECK(json_valid(input)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;
CREATE INDEX media_jobs_actor ON media_jobs(actor_id, updated_at DESC);
CREATE TABLE media_collections (
    id TEXT PRIMARY KEY,
    version INTEGER NOT NULL CHECK(version > 0),
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;
CREATE TABLE media_collection_entries (
    collection_id TEXT NOT NULL REFERENCES media_collections(id) ON DELETE CASCADE,
    entry_id TEXT NOT NULL REFERENCES media_entries(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    PRIMARY KEY(collection_id, entry_id),
    UNIQUE(collection_id, position)
) STRICT;
