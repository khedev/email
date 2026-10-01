-- Application preferences and presence are stored as durable key/value settings.
CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

-- Local-first global search. FTS5 contentless indexes are kept in sync by
-- triggers, so every write path (repositories and development seed) stays indexed.
CREATE VIRTUAL TABLE IF NOT EXISTS fts_emails USING fts5(uuid UNINDEXED, sender_name, subject, body_text);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_messages USING fts5(uuid UNINDEXED, body);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_contacts USING fts5(uuid UNINDEXED, name, email, department, position);
CREATE VIRTUAL TABLE IF NOT EXISTS fts_conversations USING fts5(uuid UNINDEXED, title, slug, description);

CREATE TRIGGER IF NOT EXISTS fts_emails_ai AFTER INSERT ON emails BEGIN
  INSERT INTO fts_emails(rowid, uuid, sender_name, subject, body_text)
  VALUES (new.rowid, new.id, new.sender_name, new.subject, new.body_text);
END;
CREATE TRIGGER IF NOT EXISTS fts_emails_ad AFTER DELETE ON emails BEGIN
  INSERT INTO fts_emails(fts_emails, rowid) VALUES ('delete', old.rowid);
END;
CREATE TRIGGER IF NOT EXISTS fts_emails_au AFTER UPDATE ON emails BEGIN
  INSERT INTO fts_emails(fts_emails, rowid) VALUES ('delete', old.rowid);
  INSERT INTO fts_emails(rowid, uuid, sender_name, subject, body_text)
  VALUES (new.rowid, new.id, new.sender_name, new.subject, new.body_text);
END;

CREATE TRIGGER IF NOT EXISTS fts_messages_ai AFTER INSERT ON messages BEGIN
  INSERT INTO fts_messages(rowid, uuid, body) VALUES (new.rowid, new.id, new.body);
END;
CREATE TRIGGER IF NOT EXISTS fts_messages_ad AFTER DELETE ON messages BEGIN
  INSERT INTO fts_messages(fts_messages, rowid) VALUES ('delete', old.rowid);
END;
CREATE TRIGGER IF NOT EXISTS fts_messages_au AFTER UPDATE ON messages BEGIN
  INSERT INTO fts_messages(fts_messages, rowid) VALUES ('delete', old.rowid);
  INSERT INTO fts_messages(rowid, uuid, body) VALUES (new.rowid, new.id, new.body);
END;

CREATE TRIGGER IF NOT EXISTS fts_contacts_ai AFTER INSERT ON contacts BEGIN
  INSERT INTO fts_contacts(rowid, uuid, name, email, department, position)
  VALUES (new.rowid, new.id, new.name, new.email, new.department, new.position);
END;
CREATE TRIGGER IF NOT EXISTS fts_contacts_ad AFTER DELETE ON contacts BEGIN
  INSERT INTO fts_contacts(fts_contacts, rowid) VALUES ('delete', old.rowid);
END;
CREATE TRIGGER IF NOT EXISTS fts_contacts_au AFTER UPDATE ON contacts BEGIN
  INSERT INTO fts_contacts(fts_contacts, rowid) VALUES ('delete', old.rowid);
  INSERT INTO fts_contacts(rowid, uuid, name, email, department, position)
  VALUES (new.rowid, new.id, new.name, new.email, new.department, new.position);
END;

CREATE TRIGGER IF NOT EXISTS fts_conversations_ai AFTER INSERT ON conversations BEGIN
  INSERT INTO fts_conversations(rowid, uuid, title, slug, description)
  VALUES (new.rowid, new.id, new.title, new.channel_slug, new.description);
END;
CREATE TRIGGER IF NOT EXISTS fts_conversations_ad AFTER DELETE ON conversations BEGIN
  INSERT INTO fts_conversations(fts_conversations, rowid) VALUES ('delete', old.rowid);
END;
CREATE TRIGGER IF NOT EXISTS fts_conversations_au AFTER UPDATE ON conversations BEGIN
  INSERT INTO fts_conversations(fts_conversations, rowid) VALUES ('delete', old.rowid);
  INSERT INTO fts_conversations(rowid, uuid, title, slug, description)
  VALUES (new.rowid, new.id, new.title, new.channel_slug, new.description);
END;

-- Backfill indexes for databases created by earlier migrations.
INSERT INTO fts_emails(rowid, uuid, sender_name, subject, body_text)
  SELECT rowid, id, sender_name, subject, body_text FROM emails;
INSERT INTO fts_messages(rowid, uuid, body)
  SELECT rowid, id, body FROM messages;
INSERT INTO fts_contacts(rowid, uuid, name, email, department, position)
  SELECT rowid, id, name, email, department, position FROM contacts;
INSERT INTO fts_conversations(rowid, uuid, title, slug, description)
  SELECT rowid, id, title, channel_slug, description FROM conversations;