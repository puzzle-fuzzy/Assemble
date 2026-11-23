-- Assemble 服务端的初始表结构。
--
-- 时间一律存 Unix 纪元起的毫秒数（INTEGER）。STRICT 让类型错位直接报错，
-- 而不是悄悄存进去。

CREATE TABLE accounts (
    id            TEXT PRIMARY KEY,
    -- 用户名不分大小写：`Alex` 与 `alex` 是同一个账户。
    username      TEXT NOT NULL UNIQUE COLLATE NOCASE,
    display_name  TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    created_at    INTEGER NOT NULL
) STRICT;

-- 系统浏览器里的登录状态。应用从不持有它。
CREATE TABLE web_sessions (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    csrf_token TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

CREATE INDEX web_sessions_account ON web_sessions(account_id);

-- 一台登录过账户的电脑。令牌只存摘要。
CREATE TABLE devices (
    id           TEXT PRIMARY KEY,
    account_id   TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    platform     TEXT NOT NULL,
    token_hash   TEXT NOT NULL UNIQUE,
    created_at   INTEGER NOT NULL,
    last_seen_at INTEGER
) STRICT;

CREATE INDEX devices_account ON devices(account_id);

-- 桌面登录时签发的单次授权码，60 秒内有效。
CREATE TABLE desktop_auth_codes (
    code_hash       TEXT PRIMARY KEY,
    account_id      TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    redirect_uri    TEXT NOT NULL,
    code_challenge  TEXT NOT NULL,
    device_name     TEXT NOT NULL,
    device_platform TEXT NOT NULL,
    expires_at      INTEGER NOT NULL,
    used_at         INTEGER
) STRICT;

-- 文件内容按摘要寻址。云盘文件与待接收的发送都只是对它的引用。
CREATE TABLE blobs (
    sha256    TEXT PRIMARY KEY,
    size      INTEGER NOT NULL,
    refcount  INTEGER NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

-- 云盘：平铺的一层，没有文件夹。
CREATE TABLE drive_files (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    size       INTEGER NOT NULL,
    mime_type  TEXT,
    sha256     TEXT NOT NULL REFERENCES blobs(sha256),
    created_at INTEGER NOT NULL
) STRICT;

CREATE INDEX drive_files_account ON drive_files(account_id, created_at DESC);

-- 一次发送。设备被删除后记录仍在，所以设备名是当时的快照，也不设外键。
-- `sha256` 同样不设外键：到达终态后内容已释放，记录还要留一阵。
CREATE TABLE sends (
    id               TEXT PRIMARY KEY,
    account_id       TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    from_device_id   TEXT NOT NULL,
    from_device_name TEXT NOT NULL,
    to_device_id     TEXT NOT NULL,
    to_device_name   TEXT NOT NULL,
    name             TEXT NOT NULL,
    size             INTEGER NOT NULL,
    mime_type        TEXT,
    sha256           TEXT NOT NULL,
    state            TEXT NOT NULL,
    failure          TEXT,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL
) STRICT;

CREATE INDEX sends_account_created ON sends(account_id, created_at DESC);

-- 清理任务按状态与创建时间找过期记录。
CREATE INDEX sends_state_created ON sends(state, created_at);

-- 谁在什么时候做了什么。只增不删。
CREATE TABLE audit_log (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id TEXT,
    device_id  TEXT,
    action     TEXT NOT NULL,
    target     TEXT,
    created_at INTEGER NOT NULL
) STRICT;

CREATE INDEX audit_log_created ON audit_log(created_at);
