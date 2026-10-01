ALTER TABLE sync_queue ADD COLUMN status TEXT NOT NULL DEFAULT 'pending';
ALTER TABLE sync_queue ADD COLUMN locked_at TEXT;
ALTER TABLE sync_queue ADD COLUMN completed_at TEXT;
CREATE TABLE IF NOT EXISTS sync_conflicts (
  id TEXT PRIMARY KEY,
  entity_type TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  local_version INTEGER NOT NULL,
  remote_version INTEGER NOT NULL,
  local_payload TEXT NOT NULL,
  remote_payload TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'open',
  created_at TEXT NOT NULL,
  resolved_at TEXT
);
CREATE TABLE IF NOT EXISTS sync_metadata (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sync_queue_ready ON sync_queue(status, next_attempt_at, created_at);
CREATE INDEX IF NOT EXISTS idx_sync_conflicts_open ON sync_conflicts(status, created_at);
