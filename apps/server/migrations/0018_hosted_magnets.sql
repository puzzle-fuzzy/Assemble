-- 来源网址不落库；元数据先校验，缓存预算在文件实际清除后才归还。
CREATE TABLE hosted_magnets (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    id TEXT NOT NULL,
    info_hash TEXT NOT NULL,
    metadata BLOB NOT NULL,
    file_ids TEXT NOT NULL,
    selected_bytes INTEGER NOT NULL CHECK(selected_bytes>=0),
    reserved_bytes INTEGER NOT NULL CHECK(reserved_bytes>=0),
    state TEXT NOT NULL CHECK(state IN ('active','cancelled','released')),
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY(account_id,id)
);
CREATE INDEX hosted_magnets_expiry ON hosted_magnets(state,expires_at);
