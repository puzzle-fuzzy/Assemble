-- 公开是私人记录的显式选择；已有私人内容绝不因升级被公开。
UPDATE media_entries SET data = json_set(
    data,
    '$.published', json(CASE WHEN scope = 'shared' THEN 'true' ELSE 'false' END),
    '$.owner', json(CASE WHEN owner_id IS NULL THEN 'null' ELSE (
        SELECT json_object('id', a.id, 'displayName', a.display_name, 'avatarUrl', a.avatar_url)
        FROM accounts a WHERE a.id = media_entries.owner_id
    ) END)
);
CREATE INDEX media_entries_publication ON media_entries(scope, json_extract(data, '$.published'), owner_id, updated_at DESC);
