-- DHT 观测独立于片库；旧站数据不导入，普通用户无访问入口。
CREATE TABLE dht_control (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    mode TEXT NOT NULL CHECK (mode IN ('stopped', 'collecting', 'draining'))
);
INSERT INTO dht_control (id, mode) VALUES (1, 'stopped');

CREATE TABLE dht_observations (
    id TEXT PRIMARY KEY CHECK (length(id) = 40),
    first_seen INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    announce_count INTEGER NOT NULL DEFAULT 0 CHECK (announce_count >= 0),
    sample_count INTEGER NOT NULL DEFAULT 0 CHECK (sample_count >= 0),
    status TEXT NOT NULL CHECK (status IN ('pending', 'fetching', 'deferred', 'verified', 'not_found', 'invalid', 'failed')),
    name TEXT,
    total_size INTEGER,
    file_count INTEGER,
    fetched_at INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 3),
    last_error TEXT,
    last_attempt_at INTEGER,
    next_attempt_at INTEGER NOT NULL DEFAULT 0,
    piece_length INTEGER,
    private INTEGER,
    comment TEXT,
    source TEXT,
    files_json TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX dht_observations_recent ON dht_observations(last_seen DESC, id);
CREATE INDEX dht_observations_queue ON dht_observations(status, next_attempt_at, announce_count DESC, first_seen);
