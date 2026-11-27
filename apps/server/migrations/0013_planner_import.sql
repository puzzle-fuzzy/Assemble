-- 地点是用户显式指定的业务资料，不能从任意文本推测。正文改变时撤下旧绑定，
-- 单纯改时间或完成状态仍保留；删除后也不残留可供导航的资料。
CREATE TABLE record_locations (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    resource TEXT NOT NULL CHECK(resource IN ('schedule','reminder','memo')),
    resource_id TEXT NOT NULL,
    locations TEXT NOT NULL CHECK(json_valid(locations)),
    PRIMARY KEY(account_id,resource,resource_id)
) STRICT;
CREATE TABLE planner_import_runs (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    source_origin TEXT NOT NULL,
    source_account_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    archive TEXT NOT NULL CHECK(json_valid(archive)),
    imported_at INTEGER NOT NULL,
    PRIMARY KEY(account_id,source_origin,source_account_id)
) STRICT;
CREATE TABLE planner_import_baselines (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    schedule_id TEXT NOT NULL REFERENCES schedules(id) ON DELETE CASCADE,
    skip_through_at INTEGER NOT NULL,
    PRIMARY KEY(account_id,schedule_id)
) STRICT;
CREATE TRIGGER schedule_locations_changed AFTER UPDATE OF title,notes,location_label,location_address ON schedules
WHEN OLD.title IS NOT NEW.title OR OLD.notes IS NOT NEW.notes OR OLD.location_label IS NOT NEW.location_label OR OLD.location_address IS NOT NEW.location_address
BEGIN DELETE FROM record_locations WHERE account_id=OLD.account_id AND resource='schedule' AND resource_id=OLD.id; END;
CREATE TRIGGER reminder_locations_changed AFTER UPDATE OF title,notes ON reminders
WHEN OLD.title IS NOT NEW.title OR OLD.notes IS NOT NEW.notes
BEGIN DELETE FROM record_locations WHERE account_id=OLD.account_id AND resource='reminder' AND resource_id=OLD.id; END;
CREATE TRIGGER memo_locations_changed AFTER UPDATE OF title,text ON memos
WHEN OLD.title IS NOT NEW.title OR OLD.text IS NOT NEW.text
BEGIN DELETE FROM record_locations WHERE account_id=OLD.account_id AND resource='memo' AND resource_id=OLD.id; END;
CREATE TRIGGER schedule_locations_deleted AFTER DELETE ON schedules
BEGIN DELETE FROM record_locations WHERE account_id=OLD.account_id AND resource='schedule' AND resource_id=OLD.id; END;
CREATE TRIGGER reminder_locations_deleted AFTER DELETE ON reminders
BEGIN DELETE FROM record_locations WHERE account_id=OLD.account_id AND resource='reminder' AND resource_id=OLD.id; END;
CREATE TRIGGER memo_locations_deleted AFTER DELETE ON memos
BEGIN DELETE FROM record_locations WHERE account_id=OLD.account_id AND resource='memo' AND resource_id=OLD.id; END;
