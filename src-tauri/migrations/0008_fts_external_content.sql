-- Migration 0007 created the search indexes as contentless FTS5 tables. That
-- design has two flaws discovered by the integration tests:
--   1. The 'delete' command form used by the UPDATE/DELETE triggers requires
--      the old column values; without them every UPDATE/DELETE on the indexed
--      tables fails ("SQL logic error"), breaking star/read/reaction writes.
--   2. Contentless tables cannot return column values, so search queries that
--      select display columns (or even the UNINDEXED uuid) from the index fail.
-- External-content tables fix both: triggers supply old values, reads resolve
-- through the content table, and 'rebuild' is the supported backfill command.
DROP TRIGGER IF EXISTS fts_emails_ai;
DROP TRIGGER IF EXISTS fts_emails_ad;
DROP TRIGGER IF EXISTS fts_emails_au;
DROP TABLE IF EXISTS fts_emails;
CREATE VIRTUAL TABLE fts_emails USING fts5(
  id UNINDEXED, sender_name, subject, body_text,
  content='emails', content_rowid='rowid'
);
CREATE TRIGGER fts_emails_ai AFTER INSERT ON emails BEGIN
  INSERT INTO fts_emails(rowid, id, sender_name, subject, body_text)
  VALUES (new.rowid, new.id, new.sender_name, new.subject, new.body_text);
END;
CREATE TRIGGER fts_emails_ad AFTER DELETE ON emails BEGIN
  INSERT INTO fts_emails(fts_emails, rowid, sender_name, subject, body_text)
  VALUES ('delete', old.rowid, old.sender_name, old.subject, old.body_text);
END;
CREATE TRIGGER fts_emails_au AFTER UPDATE ON emails BEGIN
  INSERT INTO fts_emails(fts_emails, rowid, sender_name, subject, body_text)
  VALUES ('delete', old.rowid, old.sender_name, old.subject, old.body_text);
  INSERT INTO fts_emails(rowid, id, sender_name, subject, body_text)
  VALUES (new.rowid, new.id, new.sender_name, new.subject, new.body_text);
END;

DROP TRIGGER IF EXISTS fts_messages_ai;
DROP TRIGGER IF EXISTS fts_messages_ad;
DROP TRIGGER IF EXISTS fts_messages_au;
DROP TABLE IF EXISTS fts_messages;
CREATE VIRTUAL TABLE fts_messages USING fts5(
  id UNINDEXED, body,
  content='messages', content_rowid='rowid'
);
CREATE TRIGGER fts_messages_ai AFTER INSERT ON messages BEGIN
  INSERT INTO fts_messages(rowid, id, body) VALUES (new.rowid, new.id, new.body);
END;
CREATE TRIGGER fts_messages_ad AFTER DELETE ON messages BEGIN
  INSERT INTO fts_messages(fts_messages, rowid, body) VALUES ('delete', old.rowid, old.body);
END;
CREATE TRIGGER fts_messages_au AFTER UPDATE ON messages BEGIN
  INSERT INTO fts_messages(fts_messages, rowid, body) VALUES ('delete', old.rowid, old.body);
  INSERT INTO fts_messages(rowid, id, body) VALUES (new.rowid, new.id, new.body);
END;

DROP TRIGGER IF EXISTS fts_contacts_ai;
DROP TRIGGER IF EXISTS fts_contacts_ad;
DROP TRIGGER IF EXISTS fts_contacts_au;
DROP TABLE IF EXISTS fts_contacts;
CREATE VIRTUAL TABLE fts_contacts USING fts5(
  id UNINDEXED, name, email, department, position,
  content='contacts', content_rowid='rowid'
);
CREATE TRIGGER fts_contacts_ai AFTER INSERT ON contacts BEGIN
  INSERT INTO fts_contacts(rowid, id, name, email, department, position)
  VALUES (new.rowid, new.id, new.name, new.email, new.department, new.position);
END;
CREATE TRIGGER fts_contacts_ad AFTER DELETE ON contacts BEGIN
  INSERT INTO fts_contacts(fts_contacts, rowid, name, email, department, position)
  VALUES ('delete', old.rowid, old.name, old.email, old.department, old.position);
END;
CREATE TRIGGER fts_contacts_au AFTER UPDATE ON contacts BEGIN
  INSERT INTO fts_contacts(fts_contacts, rowid, name, email, department, position)
  VALUES ('delete', old.rowid, old.name, old.email, old.department, old.position);
  INSERT INTO fts_contacts(rowid, id, name, email, department, position)
  VALUES (new.rowid, new.id, new.name, new.email, new.department, new.position);
END;

DROP TRIGGER IF EXISTS fts_conversations_ai;
DROP TRIGGER IF EXISTS fts_conversations_ad;
DROP TRIGGER IF EXISTS fts_conversations_au;
DROP TABLE IF EXISTS fts_conversations;
CREATE VIRTUAL TABLE fts_conversations USING fts5(
  id UNINDEXED, title, channel_slug, description,
  content='conversations', content_rowid='rowid'
);
CREATE TRIGGER fts_conversations_ai AFTER INSERT ON conversations BEGIN
  INSERT INTO fts_conversations(rowid, id, title, channel_slug, description)
  VALUES (new.rowid, new.id, new.title, new.channel_slug, new.description);
END;
CREATE TRIGGER fts_conversations_ad AFTER DELETE ON conversations BEGIN
  INSERT INTO fts_conversations(fts_conversations, rowid, title, channel_slug, description)
  VALUES ('delete', old.rowid, old.title, old.channel_slug, old.description);
END;
CREATE TRIGGER fts_conversations_au AFTER UPDATE ON conversations BEGIN
  INSERT INTO fts_conversations(fts_conversations, rowid, title, channel_slug, description)
  VALUES ('delete', old.rowid, old.title, old.channel_slug, old.description);
  INSERT INTO fts_conversations(rowid, id, title, channel_slug, description)
  VALUES (new.rowid, new.id, new.title, new.channel_slug, new.description);
END;

-- Rebuild all indexes from their content tables (canonical external-content backfill).
INSERT INTO fts_emails(fts_emails) VALUES ('rebuild');
INSERT INTO fts_messages(fts_messages) VALUES ('rebuild');
INSERT INTO fts_contacts(fts_contacts) VALUES ('rebuild');
INSERT INTO fts_conversations(fts_conversations) VALUES ('rebuild');
