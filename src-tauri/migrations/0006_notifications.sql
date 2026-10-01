CREATE TABLE IF NOT EXISTS notifications (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  entity_type TEXT,
  entity_id TEXT,
  is_read INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  read_at TEXT,
  deleted_at TEXT
);
CREATE TABLE IF NOT EXISTS notification_preferences (
  key TEXT PRIMARY KEY,
  enabled INTEGER NOT NULL DEFAULT 1,
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_notifications_unread ON notifications(is_read, created_at DESC);
INSERT OR IGNORE INTO notification_preferences (key, enabled, updated_at) VALUES
  ('email', 1, CURRENT_TIMESTAMP), ('messenger', 1, CURRENT_TIMESTAMP), ('mentions', 1, CURRENT_TIMESTAMP), ('thread_replies', 1, CURRENT_TIMESTAMP), ('sounds', 1, CURRENT_TIMESTAMP);
