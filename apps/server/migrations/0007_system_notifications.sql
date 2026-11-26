-- 系统通知独立于日程；共享问题状态与各管理员的已读记录分开保存。
CREATE TABLE system_notification_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    state_json TEXT NOT NULL
) STRICT;

CREATE TABLE system_notification_reads (
    notification_id TEXT NOT NULL,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL CHECK (revision >= 1),
    read_at INTEGER NOT NULL,
    PRIMARY KEY (notification_id, account_id)
) STRICT;
