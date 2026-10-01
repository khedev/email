-- Message actions (pin) and direct-message conversations.
ALTER TABLE messages ADD COLUMN pinned_at TEXT;
CREATE INDEX IF NOT EXISTS idx_messages_pinned ON messages(conversation_id, pinned_at);
