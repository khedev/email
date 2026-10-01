CREATE TABLE IF NOT EXISTS emails (
  id TEXT PRIMARY KEY,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  folder_id TEXT REFERENCES email_folders(id),
  thread_id TEXT,
  message_id_header TEXT,
  sender_name TEXT,
  sender_email TEXT,
  subject TEXT NOT NULL DEFAULT '',
  body_text TEXT NOT NULL DEFAULT '',
  body_html TEXT,
  received_at TEXT,
  is_read INTEGER NOT NULL DEFAULT 0,
  is_starred INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  deleted_at TEXT,
  sync_status TEXT NOT NULL DEFAULT 'pending',
  sync_version INTEGER NOT NULL DEFAULT 1,
  server_id TEXT
);
CREATE INDEX IF NOT EXISTS idx_emails_folder_received ON emails(folder_id, received_at DESC);
CREATE INDEX IF NOT EXISTS idx_emails_thread ON emails(thread_id);
