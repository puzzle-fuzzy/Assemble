-- 回执保留到账户删除；取消先于初始化时也占稳定请求键，避免迟到请求复活任务。
CREATE TABLE drive_uploads (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    input_json TEXT,
    state TEXT NOT NULL CHECK(state IN ('receiving','completed','cancelled','expired')),
    size INTEGER NOT NULL CHECK(size>=0),
    created_at INTEGER NOT NULL,
    progressed_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    file_json TEXT,
    UNIQUE(account_id,device_id,request_id)
);
CREATE INDEX drive_uploads_expiry ON drive_uploads(state,expires_at);
CREATE TABLE drive_upload_parts (
    upload_id TEXT NOT NULL REFERENCES drive_uploads(id) ON DELETE CASCADE,
    part_index INTEGER NOT NULL CHECK(part_index>=0 AND part_index<48),
    size INTEGER NOT NULL CHECK(size>0 AND size<=2097152),
    sha256 TEXT NOT NULL,
    PRIMARY KEY(upload_id,part_index)
);
-- 新旧入口共同预留；旧入口有独立的一小时请求时限，预留两小时后才允许回收。
CREATE TABLE upload_reservations (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    logical_bytes INTEGER NOT NULL CHECK(logical_bytes>=0),
    bytes INTEGER NOT NULL CHECK(bytes>=0),
    expires_at INTEGER NOT NULL
);
CREATE TABLE upload_cleanup (
    upload_id TEXT PRIMARY KEY,
    attempts INTEGER NOT NULL DEFAULT 0,
    failed_at INTEGER
);
