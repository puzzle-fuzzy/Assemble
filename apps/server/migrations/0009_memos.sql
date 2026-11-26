-- 备忘录按账户隔离；新建回执在删除后保留，超时重试不能复活已删除的文本。
CREATE TABLE memos (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    text TEXT NOT NULL,
    version INTEGER NOT NULL CHECK(version > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX memos_account_updated ON memos(account_id, updated_at DESC, id DESC);
CREATE TABLE memo_requests (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    request_id TEXT NOT NULL,
    memo_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    PRIMARY KEY(account_id, request_id)
);
