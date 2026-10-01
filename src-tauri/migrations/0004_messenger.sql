CREATE TABLE IF NOT EXISTS channel_members (
  channel_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  contact_id TEXT NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
  role TEXT NOT NULL DEFAULT 'member',
  joined_at TEXT NOT NULL,
  PRIMARY KEY(channel_id, contact_id)
);
CREATE TABLE IF NOT EXISTS message_reactions (
  id TEXT PRIMARY KEY,
  message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  contact_id TEXT NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
  emoji TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE(message_id, contact_id, emoji)
);
CREATE INDEX IF NOT EXISTS idx_conversations_kind_updated ON conversations(kind, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_messages_reply ON messages(reply_to_id, sent_at);
