ALTER TABLE emails ADD COLUMN direction TEXT NOT NULL DEFAULT 'inbound';
ALTER TABLE emails ADD COLUMN delivery_state TEXT NOT NULL DEFAULT 'received';
CREATE TABLE IF NOT EXISTS email_recipients (
  id TEXT PRIMARY KEY,
  email_id TEXT NOT NULL REFERENCES emails(id) ON DELETE CASCADE,
  recipient_type TEXT NOT NULL CHECK (recipient_type IN ('to', 'cc', 'bcc')),
  display_name TEXT,
  email_address TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS email_attachments (
  id TEXT PRIMARY KEY,
  email_id TEXT NOT NULL REFERENCES emails(id) ON DELETE CASCADE,
  file_name TEXT NOT NULL,
  mime_type TEXT,
  byte_size INTEGER NOT NULL,
  storage_key TEXT NOT NULL,
  content_hash TEXT,
  download_state TEXT NOT NULL DEFAULT 'local',
  created_at TEXT NOT NULL,
  deleted_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_email_recipients_email ON email_recipients(email_id);
CREATE INDEX IF NOT EXISTS idx_email_attachments_email ON email_attachments(email_id);
