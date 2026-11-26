-- 手机扫码配对：从网页上生成二维码开始，到手机拿到令牌或超时为止。
-- 票据（二维码里的那一串）只存摘要；确认码是给人核对的六位数字，不是秘密。
-- 状态：pending（等手机扫）→ claimed（手机认领了，等网页上的人决定）→ approved / denied；
-- 手机取走令牌之后是 consumed。拿错校验器来试过的那一张直接变成 consumed。
CREATE TABLE device_pairings (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    ticket_hash TEXT NOT NULL UNIQUE,
    code_challenge TEXT,
    device_name TEXT,
    confirmation_code TEXT,
    state TEXT NOT NULL,
    device_id TEXT,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    decided_at INTEGER
);
CREATE INDEX device_pairings_account ON device_pairings(account_id, created_at);
