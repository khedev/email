-- Threaded conversations (migration 0011).
--
-- Before this migration a reply was reachable only through `reply_to_id`, so a
-- reply to a reply silently started a *new* thread and a reaction landed on
-- whichever row was clicked. Two columns make the grouping explicit and
-- queryable:
--
--   thread_id    -> id of the thread's root message. A root points at itself,
--                   every reply points at the same root no matter how deep the
--                   reply chain gets. This is the grouping key.
--   in_reply_to  -> the exact message that was answered (quotation context for
--                   the UI); it never changes the thread a message belongs to.
--
-- `reply_to_id` stays as the durable parent pointer so the sync payload keeps
-- the original server-side relationship.
ALTER TABLE messages ADD COLUMN thread_id TEXT REFERENCES messages(id) ON DELETE CASCADE;
ALTER TABLE messages ADD COLUMN in_reply_to TEXT REFERENCES messages(id) ON DELETE CASCADE;

-- Backfill: walk each row up the reply chain until it reaches a message with no
-- parent; that ancestor is the thread root. Rows without a parent (the roots)
-- are fixed up by the second statement.
WITH RECURSIVE ancestry(message_id, ancestor) AS (
  SELECT id, id FROM messages
  UNION ALL
  SELECT ancestry.message_id, parent.reply_to_id
  FROM ancestry JOIN messages parent ON parent.id = ancestry.ancestor
  WHERE parent.reply_to_id IS NOT NULL
)
UPDATE messages SET thread_id = (
  SELECT ancestry.ancestor
  FROM ancestry JOIN messages root ON root.id = ancestry.ancestor
  WHERE ancestry.message_id = messages.id AND root.reply_to_id IS NULL
  LIMIT 1
);
UPDATE messages SET thread_id = id WHERE thread_id IS NULL;
UPDATE messages SET in_reply_to = reply_to_id WHERE in_reply_to IS NULL AND reply_to_id IS NOT NULL;

-- One row per thread is the channel history query shape; replies are fetched per
-- thread, so both access paths need their own index.
CREATE INDEX IF NOT EXISTS idx_messages_thread ON messages(thread_id, sent_at);
CREATE INDEX IF NOT EXISTS idx_messages_thread_root ON messages(conversation_id, thread_id) WHERE reply_to_id IS NULL;
