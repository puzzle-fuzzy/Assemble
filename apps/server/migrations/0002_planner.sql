-- 日程规划的第一批：日程、提醒，以及它们到点后生成的通知。
--
-- 时间一律存 Unix 纪元起的毫秒数（INTEGER），与发送、云盘一致。

CREATE TABLE schedules (
    id               TEXT PRIMARY KEY,
    account_id       TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    title            TEXT NOT NULL,
    location_label   TEXT,
    location_address TEXT,
    start_at         INTEGER NOT NULL,
    end_at           INTEGER,
    -- none / daily / weekly / monthly
    repeat           TEXT NOT NULL,
    repeat_until     INTEGER,
    -- 提前多少分钟提醒；零与没填一样存成空。
    remind_before    INTEGER,
    notes            TEXT,
    -- upcoming / done / cancelled
    status           TEXT NOT NULL,
    -- 每改一次加一。修改要带上看到的那个版本。
    version          INTEGER NOT NULL,
    -- 建立它的那台设备当时的名字。设备被删除后记录仍在，所以不设外键。
    created_by       TEXT NOT NULL,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL
) STRICT;

-- 调度按状态找还在进行中的；名单按更新时间找最近结束的。
CREATE INDEX schedules_account_status ON schedules(account_id, status);
CREATE INDEX schedules_account_updated ON schedules(account_id, updated_at DESC);

CREATE TABLE reminders (
    id         TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    title      TEXT NOT NULL,
    due_at     INTEGER NOT NULL,
    notes      TEXT,
    -- scheduled / fired / dismissed / cancelled
    status     TEXT NOT NULL,
    version    INTEGER NOT NULL,
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

CREATE INDEX reminders_due ON reminders(status, due_at);
CREATE INDEX reminders_account_updated ON reminders(account_id, updated_at DESC);

-- 到点后生成的通知。日程的通知编号由"日程 + 场次时刻"决定，主键保证同一场只有一行。
-- 记录会留着：读过与作废的只打上时间，不删——删了的话，同一场又会被当成没通知过。
CREATE TABLE notifications (
    id              TEXT PRIMARY KEY,
    account_id      TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    -- schedule / reminder
    kind            TEXT NOT NULL,
    title           TEXT NOT NULL,
    body            TEXT,
    schedule_id     TEXT,
    reminder_id     TEXT,
    -- 日程的哪一场；提醒没有这一项。
    occurrence_at   INTEGER,
    raised_at       INTEGER NOT NULL,
    read_at         INTEGER,
    -- 日程取消、改期之后不再成立的通知。不再补发给任何设备。
    invalidated_at  INTEGER
) STRICT;

CREATE INDEX notifications_account_raised ON notifications(account_id, raised_at DESC);
CREATE INDEX notifications_schedule ON notifications(schedule_id) WHERE schedule_id IS NOT NULL;
CREATE INDEX notifications_reminder ON notifications(reminder_id) WHERE reminder_id IS NOT NULL;

-- 新建时带的幂等键：同一个键重试，拿回原来那一条，不会建出两条。
CREATE TABLE planner_mutations (
    account_id  TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    key         TEXT NOT NULL,
    -- schedule / reminder
    resource    TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    PRIMARY KEY (account_id, key)
) STRICT;
