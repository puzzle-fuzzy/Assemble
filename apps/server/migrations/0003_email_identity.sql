-- 旧账户原地保留；验证邮箱后仍使用同一 id，设备、云盘和日程不迁移。
ALTER TABLE accounts ADD COLUMN email TEXT COLLATE NOCASE;
ALTER TABLE accounts ADD COLUMN email_verified_at INTEGER;
ALTER TABLE accounts ADD COLUMN password_enabled INTEGER NOT NULL DEFAULT 1;
ALTER TABLE accounts ADD COLUMN avatar_url TEXT;
CREATE UNIQUE INDEX accounts_email ON accounts(email) WHERE email IS NOT NULL;

-- 第三方身份只按不可变的提供商 id 认人，绝不按邮箱自动合并。
CREATE TABLE github_identities (
    github_id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL UNIQUE REFERENCES accounts(id) ON DELETE CASCADE,
    login TEXT NOT NULL
) STRICT;

-- 邮件凭据只存摘要。GET 只显示确认表单，POST 才原子消费，避免邮件扫描器误操作。
CREATE TABLE email_actions (
    token_hash TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('register', 'reset', 'bind', 'change_old', 'change_new', 'github')),
    account_id TEXT REFERENCES accounts(id) ON DELETE CASCADE,
    email TEXT NOT NULL,
    previous_email TEXT,
    proof_hash TEXT,
    browser_hash TEXT,
    github_id TEXT,
    github_login TEXT,
    display_name TEXT,
    next TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;
CREATE INDEX email_actions_expiry ON email_actions(expires_at);
CREATE INDEX email_actions_account ON email_actions(account_id);

-- 发信限额在数据库中预留，重启和并发请求不能绕过；不保存原始 IP 或邮箱作为限流键。
CREATE TABLE email_rate_limits (
    bucket TEXT PRIMARY KEY,
    started_at INTEGER NOT NULL,
    requests INTEGER NOT NULL
) STRICT;
