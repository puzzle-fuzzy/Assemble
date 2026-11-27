-- 回收批次不释放内容引用；恢复、永久清理回执保留至账户删除，防止迟到重试重复执行。
CREATE TABLE drive_trash (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    request_id TEXT,
    fingerprint TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('file','folder')),
    root_id TEXT NOT NULL,
    name TEXT NOT NULL,
    original_parent_id TEXT,
    original_location TEXT NOT NULL,
    deleted_at INTEGER NOT NULL,
    version INTEGER NOT NULL CHECK(version > 0),
    file_count INTEGER NOT NULL,
    folder_count INTEGER NOT NULL,
    size INTEGER NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('trashed','restored','purged')),
    result TEXT,
    UNIQUE(account_id,request_id)
) STRICT;
CREATE INDEX drive_trash_account ON drive_trash(account_id,state,deleted_at DESC,id DESC);
ALTER TABLE drive_files ADD COLUMN trash_id TEXT;
ALTER TABLE drive_files ADD COLUMN restored_at INTEGER;
ALTER TABLE drive_folders ADD COLUMN trash_id TEXT;
DROP INDEX drive_folders_siblings;
CREATE UNIQUE INDEX drive_folders_siblings ON drive_folders(account_id,coalesce(parent_id,''),name) WHERE trash_id IS NULL;
CREATE INDEX drive_files_trash ON drive_files(account_id,trash_id,parent_id);
CREATE INDEX drive_folders_trash ON drive_folders(account_id,trash_id,parent_id);
CREATE VIEW active_drive_files AS SELECT rowid AS rowid,* FROM drive_files WHERE trash_id IS NULL;
CREATE VIEW active_drive_folders AS SELECT * FROM drive_folders WHERE trash_id IS NULL;
-- ALTER TABLE 无法添加组合外键；触发器守住批次所属，既有父级组合外键仍保留。
CREATE TRIGGER drive_files_trash_owner BEFORE UPDATE OF account_id,trash_id ON drive_files
WHEN NEW.trash_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM drive_trash WHERE id=NEW.trash_id AND account_id=NEW.account_id)
BEGIN SELECT RAISE(ABORT,'invalid trash owner'); END;
CREATE TRIGGER drive_folders_trash_owner BEFORE UPDATE OF account_id,trash_id ON drive_folders
WHEN NEW.trash_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM drive_trash WHERE id=NEW.trash_id AND account_id=NEW.account_id)
BEGIN SELECT RAISE(ABORT,'invalid trash owner'); END;
CREATE TRIGGER drive_files_trash_owner_insert BEFORE INSERT ON drive_files
WHEN NEW.trash_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM drive_trash WHERE id=NEW.trash_id AND account_id=NEW.account_id)
BEGIN SELECT RAISE(ABORT,'invalid trash owner'); END;
CREATE TRIGGER drive_folders_trash_owner_insert BEFORE INSERT ON drive_folders
WHEN NEW.trash_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM drive_trash WHERE id=NEW.trash_id AND account_id=NEW.account_id)
BEGIN SELECT RAISE(ABORT,'invalid trash owner'); END;
CREATE TABLE blob_cleanup (
    sha256 TEXT PRIMARY KEY,
    queued_at INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    account_id TEXT,
    device_id TEXT,
    target_id TEXT,
    target_kind TEXT CHECK(target_kind IN ('file','folder'))
) STRICT;
-- 替换旧快照触发器，不能依赖多个 AFTER UPDATE 触发器的执行顺序。
DROP TRIGGER activity_drive_files_update;
CREATE TRIGGER activity_drive_files_update AFTER UPDATE ON drive_files
BEGIN
 INSERT INTO activity_subjects(kind,id,owner_id,label,data,created_at,updated_at,deleted_at)
 VALUES('file',NEW.id,NEW.account_id,NEW.name,json_object('kind','file','name',NEW.name,'size',NEW.size,'mimeType',NEW.mime_type),NEW.created_at,coalesce(NEW.updated_at,NEW.created_at),CASE WHEN NEW.trash_id IS NULL THEN NULL ELSE (SELECT deleted_at FROM drive_trash WHERE id=NEW.trash_id) END)
 ON CONFLICT(kind,id) DO UPDATE SET owner_id=excluded.owner_id,label=excluded.label,data=excluded.data,updated_at=excluded.updated_at,deleted_at=excluded.deleted_at;
END;
