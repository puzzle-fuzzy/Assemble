-- 日志只存安全的对象元数据：删除业务记录后仍能识别历史操作，不保留内容或路径。
CREATE TABLE activity_subjects (
 kind TEXT NOT NULL CHECK(kind IN ('file','transfer','media','collection','remote')),
 id TEXT NOT NULL,
 owner_id TEXT,
 label TEXT NOT NULL,
 data TEXT NOT NULL CHECK(json_valid(data) AND json_extract(data,'$.kind')=kind),
 created_at INTEGER NOT NULL,
 updated_at INTEGER NOT NULL,
 deleted_at INTEGER,
 PRIMARY KEY(kind,id)
) STRICT;
ALTER TABLE audit_log ADD COLUMN actor_name TEXT;
ALTER TABLE audit_log ADD COLUMN actor_device_name TEXT;
ALTER TABLE audit_log ADD COLUMN category TEXT NOT NULL DEFAULT 'other';
ALTER TABLE audit_log ADD COLUMN subject_kind TEXT;
ALTER TABLE audit_log ADD COLUMN subject_id TEXT;
ALTER TABLE audit_log ADD COLUMN subject_name TEXT;
ALTER TABLE audit_log ADD COLUMN subject_owner TEXT;
ALTER TABLE audit_log ADD COLUMN snapshot TEXT CHECK(snapshot IS NULL OR json_valid(snapshot));
ALTER TABLE audit_log ADD COLUMN event_key TEXT;
CREATE UNIQUE INDEX audit_event_key ON audit_log(event_key) WHERE event_key IS NOT NULL;
CREATE INDEX audit_category_time ON audit_log(category,created_at DESC,id DESC);
CREATE INDEX audit_subject_time ON audit_log(subject_kind,subject_id,created_at DESC,id DESC);

-- drive_files 的名称/归属/状态快照，不包含原文、文件内容或远控数据。
INSERT INTO activity_subjects SELECT 'file',R.id,R.account_id,R.name,json_object('kind','file','name',R.name,'size',R.size,'mimeType',R.mime_type),R.created_at,R.created_at,NULL FROM drive_files R;
CREATE TRIGGER activity_drive_files_insert AFTER INSERT ON drive_files
BEGIN
 INSERT INTO activity_subjects(kind,id,owner_id,label,data,created_at,updated_at,deleted_at) VALUES('file',NEW.id,NEW.account_id,NEW.name,json_object('kind','file','name',NEW.name,'size',NEW.size,'mimeType',NEW.mime_type),NEW.created_at,NEW.created_at,NULL)
 ON CONFLICT(kind,id) DO UPDATE SET owner_id=excluded.owner_id,label=excluded.label,data=excluded.data,updated_at=excluded.updated_at,deleted_at=NULL;
END;
CREATE TRIGGER activity_drive_files_update AFTER UPDATE ON drive_files
BEGIN
 INSERT INTO activity_subjects(kind,id,owner_id,label,data,created_at,updated_at,deleted_at) VALUES('file',NEW.id,NEW.account_id,NEW.name,json_object('kind','file','name',NEW.name,'size',NEW.size,'mimeType',NEW.mime_type),NEW.created_at,NEW.created_at,NULL)
 ON CONFLICT(kind,id) DO UPDATE SET owner_id=excluded.owner_id,label=excluded.label,data=excluded.data,updated_at=CAST(unixepoch('subsec')*1000 AS INTEGER),deleted_at=NULL;
END;
CREATE TRIGGER activity_drive_files_delete BEFORE DELETE ON drive_files
BEGIN
 UPDATE activity_subjects SET deleted_at=CAST(unixepoch('subsec')*1000 AS INTEGER),updated_at=CAST(unixepoch('subsec')*1000 AS INTEGER) WHERE kind='file' AND id=OLD.id;
END;

-- sends 的名称/归属/状态快照，不包含原文、文件内容或远控数据。
INSERT INTO activity_subjects SELECT 'transfer',R.id,R.account_id,R.name,json_object('kind','transfer','name',R.name,'size',R.size,'fromDeviceId',R.from_device_id,'fromDeviceName',R.from_device_name,'toDeviceId',R.to_device_id,'toDeviceName',R.to_device_name,'state',R.state,'failure',R.failure),R.created_at,R.updated_at,NULL FROM sends R;
CREATE TRIGGER activity_sends_insert AFTER INSERT ON sends
BEGIN
 INSERT INTO activity_subjects(kind,id,owner_id,label,data,created_at,updated_at,deleted_at) VALUES('transfer',NEW.id,NEW.account_id,NEW.name,json_object('kind','transfer','name',NEW.name,'size',NEW.size,'fromDeviceId',NEW.from_device_id,'fromDeviceName',NEW.from_device_name,'toDeviceId',NEW.to_device_id,'toDeviceName',NEW.to_device_name,'state',NEW.state,'failure',NEW.failure),NEW.created_at,NEW.updated_at,NULL)
 ON CONFLICT(kind,id) DO UPDATE SET owner_id=excluded.owner_id,label=excluded.label,data=excluded.data,updated_at=excluded.updated_at,deleted_at=NULL;
END;
CREATE TRIGGER activity_sends_update AFTER UPDATE ON sends
BEGIN
 INSERT INTO activity_subjects(kind,id,owner_id,label,data,created_at,updated_at,deleted_at) VALUES('transfer',NEW.id,NEW.account_id,NEW.name,json_object('kind','transfer','name',NEW.name,'size',NEW.size,'fromDeviceId',NEW.from_device_id,'fromDeviceName',NEW.from_device_name,'toDeviceId',NEW.to_device_id,'toDeviceName',NEW.to_device_name,'state',NEW.state,'failure',NEW.failure),NEW.created_at,NEW.updated_at,NULL)
 ON CONFLICT(kind,id) DO UPDATE SET owner_id=excluded.owner_id,label=excluded.label,data=excluded.data,updated_at=excluded.updated_at,deleted_at=NULL;
END;
CREATE TRIGGER activity_sends_delete BEFORE DELETE ON sends
BEGIN
 UPDATE activity_subjects SET deleted_at=CAST(unixepoch('subsec')*1000 AS INTEGER),updated_at=CAST(unixepoch('subsec')*1000 AS INTEGER) WHERE kind='transfer' AND id=OLD.id;
END;

-- media_entries 的名称/归属/状态快照，不包含原文、文件内容或远控数据。
INSERT INTO activity_subjects SELECT 'media',R.id,R.owner_id,json_extract(R.data,'$.title'),json_object('kind','media','title',json_extract(R.data,'$.title'),'scope',R.scope),R.created_at,R.updated_at,NULL FROM media_entries R;
CREATE TRIGGER activity_media_entries_insert AFTER INSERT ON media_entries
BEGIN
 INSERT INTO activity_subjects(kind,id,owner_id,label,data,created_at,updated_at,deleted_at) VALUES('media',NEW.id,NEW.owner_id,json_extract(NEW.data,'$.title'),json_object('kind','media','title',json_extract(NEW.data,'$.title'),'scope',NEW.scope),NEW.created_at,NEW.updated_at,NULL)
 ON CONFLICT(kind,id) DO UPDATE SET owner_id=excluded.owner_id,label=excluded.label,data=excluded.data,updated_at=excluded.updated_at,deleted_at=NULL;
END;
CREATE TRIGGER activity_media_entries_update AFTER UPDATE ON media_entries
BEGIN
 INSERT INTO activity_subjects(kind,id,owner_id,label,data,created_at,updated_at,deleted_at) VALUES('media',NEW.id,NEW.owner_id,json_extract(NEW.data,'$.title'),json_object('kind','media','title',json_extract(NEW.data,'$.title'),'scope',NEW.scope),NEW.created_at,NEW.updated_at,NULL)
 ON CONFLICT(kind,id) DO UPDATE SET owner_id=excluded.owner_id,label=excluded.label,data=excluded.data,updated_at=excluded.updated_at,deleted_at=NULL;
END;
CREATE TRIGGER activity_media_entries_delete BEFORE DELETE ON media_entries
BEGIN
 UPDATE activity_subjects SET deleted_at=CAST(unixepoch('subsec')*1000 AS INTEGER),updated_at=CAST(unixepoch('subsec')*1000 AS INTEGER) WHERE kind='media' AND id=OLD.id;
END;

-- media_collections 的名称/归属/状态快照，不包含原文、文件内容或远控数据。
INSERT INTO activity_subjects SELECT 'collection',R.id,NULL,R.title,json_object('kind','collection','title',R.title),R.created_at,R.updated_at,NULL FROM media_collections R;
CREATE TRIGGER activity_media_collections_insert AFTER INSERT ON media_collections
BEGIN
 INSERT INTO activity_subjects(kind,id,owner_id,label,data,created_at,updated_at,deleted_at) VALUES('collection',NEW.id,NULL,NEW.title,json_object('kind','collection','title',NEW.title),NEW.created_at,NEW.updated_at,NULL)
 ON CONFLICT(kind,id) DO UPDATE SET owner_id=excluded.owner_id,label=excluded.label,data=excluded.data,updated_at=excluded.updated_at,deleted_at=NULL;
END;
CREATE TRIGGER activity_media_collections_update AFTER UPDATE ON media_collections
BEGIN
 INSERT INTO activity_subjects(kind,id,owner_id,label,data,created_at,updated_at,deleted_at) VALUES('collection',NEW.id,NULL,NEW.title,json_object('kind','collection','title',NEW.title),NEW.created_at,NEW.updated_at,NULL)
 ON CONFLICT(kind,id) DO UPDATE SET owner_id=excluded.owner_id,label=excluded.label,data=excluded.data,updated_at=excluded.updated_at,deleted_at=NULL;
END;
CREATE TRIGGER activity_media_collections_delete BEFORE DELETE ON media_collections
BEGIN
 UPDATE activity_subjects SET deleted_at=CAST(unixepoch('subsec')*1000 AS INTEGER),updated_at=CAST(unixepoch('subsec')*1000 AS INTEGER) WHERE kind='collection' AND id=OLD.id;
END;

-- 旧记录只分类和解析既有标识；不伪造当时未保存的历史名称或对象快照。
UPDATE audit_log SET category=CASE WHEN action LIKE 'remote.%' THEN 'remote' WHEN action LIKE 'drive.%' THEN 'files' WHEN action LIKE 'send.%' THEN 'transfers' WHEN action LIKE 'media.%' THEN 'media' ELSE 'other' END,subject_kind=CASE WHEN action LIKE 'remote.%' THEN 'remote' WHEN action LIKE 'drive.%' AND action NOT LIKE 'drive.folder%' THEN 'file' WHEN action LIKE 'send.%' THEN 'transfer' WHEN action LIKE 'media.collection.%' THEN 'collection' WHEN action LIKE 'media.%' THEN 'media' ELSE NULL END,subject_id=CASE WHEN action LIKE 'media.%' AND action NOT LIKE 'media.collection.%' THEN substr(substr(target,instr(target,':')+1),instr(substr(target,instr(target,':')+1),':')+1) ELSE target END;
CREATE TRIGGER activity_audit_insert AFTER INSERT ON audit_log
BEGIN
 UPDATE audit_log SET category=CASE WHEN NEW.action LIKE 'remote.%' THEN 'remote' WHEN NEW.action LIKE 'drive.%' THEN 'files' WHEN NEW.action LIKE 'send.%' THEN 'transfers' WHEN NEW.action LIKE 'media.%' THEN 'media' ELSE 'other' END,subject_kind=CASE WHEN NEW.action LIKE 'remote.%' THEN 'remote' WHEN NEW.action LIKE 'drive.%' AND NEW.action NOT LIKE 'drive.folder%' THEN 'file' WHEN NEW.action LIKE 'send.%' THEN 'transfer' WHEN NEW.action LIKE 'media.collection.%' THEN 'collection' WHEN NEW.action LIKE 'media.%' THEN 'media' ELSE NULL END,subject_id=CASE WHEN NEW.action LIKE 'media.%' AND NEW.action NOT LIKE 'media.collection.%' THEN substr(substr(NEW.target,instr(NEW.target,':')+1),instr(substr(NEW.target,instr(NEW.target,':')+1),':')+1) ELSE NEW.target END,
 actor_name=(SELECT display_name FROM accounts WHERE id=NEW.account_id),
 actor_device_name=(SELECT name FROM devices WHERE id=NEW.device_id AND account_id=NEW.account_id)
 WHERE id=NEW.id;
 UPDATE audit_log SET
 subject_name=(SELECT label FROM activity_subjects s WHERE s.kind=audit_log.subject_kind AND s.id=audit_log.subject_id),
 subject_owner=(SELECT owner_id FROM activity_subjects s WHERE s.kind=audit_log.subject_kind AND s.id=audit_log.subject_id),
 snapshot=coalesce(audit_log.snapshot,(SELECT data FROM activity_subjects s WHERE s.kind=audit_log.subject_kind AND s.id=audit_log.subject_id))
 WHERE id=NEW.id;
END;
