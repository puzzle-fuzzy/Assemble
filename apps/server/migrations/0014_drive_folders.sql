-- 目录只组织记录，不成为内容引用者。组合外键阻止跨账户挂载。
CREATE TABLE drive_folders (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    parent_id TEXT,
    name TEXT NOT NULL,
    version INTEGER NOT NULL CHECK(version > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(account_id,id),
    CHECK(id <> 'root' AND id IS NOT parent_id),
    FOREIGN KEY(account_id,parent_id) REFERENCES drive_folders(account_id,id) DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE UNIQUE INDEX drive_folders_siblings ON drive_folders(account_id,coalesce(parent_id,''),name);
CREATE INDEX drive_folders_parent ON drive_folders(account_id,parent_id);
CREATE TABLE drive_files_tree (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    size INTEGER NOT NULL,
    mime_type TEXT,
    sha256 TEXT NOT NULL REFERENCES blobs(sha256),
    created_at INTEGER NOT NULL,
    parent_id TEXT,
    version INTEGER NOT NULL DEFAULT 1 CHECK(version > 0),
    updated_at INTEGER,
    FOREIGN KEY(account_id,parent_id) REFERENCES drive_folders(account_id,id) DEFERRABLE INITIALLY DEFERRED
) STRICT;
INSERT INTO drive_files_tree(id,account_id,name,size,mime_type,sha256,created_at)
SELECT id,account_id,name,size,mime_type,sha256,created_at FROM drive_files ORDER BY rowid;
DROP TABLE drive_files;
ALTER TABLE drive_files_tree RENAME TO drive_files;
CREATE INDEX drive_files_account ON drive_files(account_id,created_at DESC);
CREATE INDEX drive_files_parent ON drive_files(account_id,parent_id);
-- 不以外键绑定目录：删除后保留回执，重放不能重新创建。
CREATE TABLE drive_folder_requests (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    request_id TEXT NOT NULL,
    folder_id TEXT NOT NULL,
    parent_id TEXT,
    name TEXT NOT NULL,
    PRIMARY KEY(account_id,request_id)
) STRICT;
