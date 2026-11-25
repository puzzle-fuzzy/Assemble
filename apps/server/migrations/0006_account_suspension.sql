-- 停用的账户：这一列有值就表示停用。数据原样留着，只是哪里都登不进去；
-- 写下时间是为了在管理端看得出是什么时候停的。
ALTER TABLE accounts ADD COLUMN suspended_at INTEGER;
