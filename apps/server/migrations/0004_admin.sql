-- 管理权限只由受保护配置或服务器命令授予，不按注册顺序推断。
ALTER TABLE accounts ADD COLUMN is_super_admin INTEGER NOT NULL DEFAULT 0 CHECK (is_super_admin IN (0, 1));
ALTER TABLE accounts ADD COLUMN last_login_at INTEGER;

-- 历史审计没有可靠的登录方式，不能倒推补齐。只登记升级后真正建成的网页登录会话。
CREATE TABLE login_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    method TEXT NOT NULL CHECK (method IN ('email', 'github')),
    occurred_at INTEGER NOT NULL
) STRICT;
CREATE INDEX login_events_time ON login_events(occurred_at DESC, id DESC);
CREATE INDEX login_events_account_time ON login_events(account_id, occurred_at DESC, id DESC);
CREATE INDEX accounts_created ON accounts(created_at DESC, id DESC);
CREATE INDEX devices_created ON devices(created_at DESC, id DESC);
