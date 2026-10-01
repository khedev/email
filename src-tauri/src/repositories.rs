use std::collections::HashMap;
use chrono::Utc;
use rusqlite::{params, Connection};
use uuid::Uuid;
use crate::{error::AppError, mailbox::FetchedMessage, models::{Account, AccountConnection, Channel, ChatMessage, Contact, Conversation, CreateAccount, CreateContact, Draft, DraftInput, EmailDetail, EmailSummary, MessageReaction, Notification, NotificationPreference, OutboundMessage, Presence, ReactionSummary, SearchResult, SendMessageInput, SettingsEntry, SyncQueueItem, UpdateNotificationPreference}};

/// Stable id of the local user's own contact row. Messenger identity (the
/// "mine" flag, reactions, channel membership) is anchored to this row, so it
/// must exist in every build: debug seeds reuse it, release builds create it
/// on demand (`ensure_self_identity`).
pub const SELF_CONTACT_ID: &str = "dev-self";

/// One outbound message together with the account it must leave through.
///
/// The connection is `None` when the account row is gone (removed after the send
/// was queued): such a message cannot be delivered, and it is reported as failed
/// rather than silently ignored — the same honesty the reading pane applies to
/// every other delivery state.
pub struct QueuedSend {
  pub connection: Option<AccountConnection>,
  pub message: OutboundMessage,
}

/// How long a delivery claim (`sync_queue.status = 'syncing'`) may stand before
/// it is treated as abandoned. A run that dies mid-session leaves its claim
/// behind, and without this the message would never be offered again.
const DELIVERY_CLAIM_MINUTES: u32 = 5;

pub struct Repositories<'a> { connection: &'a Connection }
impl<'a> Repositories<'a> {
  pub fn new(connection: &'a Connection) -> Self { Self { connection } }
  pub fn inbox(&self) -> Result<Vec<EmailSummary>, AppError> { self.emails_in_folder("inbox") }
  pub fn emails_in_folder(&self, role: &str) -> Result<Vec<EmailSummary>, AppError> { if !is_mail_role(role) { return Err(AppError::Validation); } let filter = format!("e.folder_id IN (SELECT folder.id FROM email_folders folder WHERE folder.account_id = e.account_id AND folder.role = '{}' AND folder.deleted_at IS NULL)", role); self.list_emails(&filter) }
  pub fn email_thread(&self, id: &str) -> Result<Vec<EmailSummary>, AppError> {
    if id.is_empty() || id.len() > 128 { return Err(AppError::Validation); }
    let tid: String = self.connection.query_row("SELECT COALESCE(NULLIF(thread_id, ''), '') FROM emails WHERE id = ?1 AND deleted_at IS NULL", params![id], |row| row.get(0)).ok().unwrap_or_default();
    if tid.is_empty() { return Ok(vec![]); }
    let sql = "SELECT e.id, COALESCE(NULLIF(e.sender_name, ''), NULLIF(e.sender_email, ''), a.display_name, 'Unknown sender'), e.subject, substr(e.body_text, 1, 120), COALESCE(e.received_at, e.created_at), e.is_read, e.is_starred FROM emails e JOIN accounts a ON a.id = e.account_id WHERE e.deleted_at IS NULL AND e.thread_id = ?1 AND e.account_id = (SELECT account_id FROM emails WHERE id = ?2) ORDER BY COALESCE(e.received_at, e.created_at) ASC LIMIT 100";
    let mut statement = self.connection.prepare(sql)?;
    let rows = statement.query_map(params![tid, id], |row| Ok(EmailSummary { id: row.get(0)?, sender_name: row.get(1)?, subject: row.get(2)?, preview: row.get(3)?, received_at: row.get(4)?, is_read: row.get::<_, i64>(5)? != 0, is_starred: row.get::<_, i64>(6)? != 0 }))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
  }
  pub fn starred(&self) -> Result<Vec<EmailSummary>, AppError> { self.list_emails("e.is_starred = 1") }
  pub fn email(&self, id: &str) -> Result<Option<EmailDetail>, AppError> {
    // `delivery_error` is read from the durable `send_smtp` queue item rather
    // than a column on `emails`: the classifier already stores its fixed,
    // non-sensitive wording there, and the reading pane shows it when a send
    // failed.
    let mut statement = self.connection.prepare("SELECT id, account_id, direction, delivery_state, COALESCE(sender_name, sender_email, 'Unknown sender'), COALESCE(sender_email, ''), subject, body_text, COALESCE(received_at, created_at), is_read, is_starred, (SELECT last_error FROM sync_queue WHERE entity_type = 'email' AND entity_id = emails.id AND operation = 'send_smtp') FROM emails WHERE id = ?1 AND deleted_at IS NULL")?;
    let mut rows = statement.query(params![id])?;
    match rows.next()? { Some(row) => {
      let mut detail = EmailDetail { id: row.get(0)?, account_id: row.get(1)?, direction: row.get(2)?, delivery_state: row.get(3)?, sender_name: row.get(4)?, sender_email: row.get(5)?, subject: row.get(6)?, body_text: row.get(7)?, received_at: row.get(8)?, is_read: row.get::<_, i64>(9)? != 0, is_starred: row.get::<_, i64>(10)? != 0, delivery_error: row.get(11)?, to: vec![], cc: vec![], bcc: vec![] };
      self.fill_recipients(&mut detail)?;
      Ok(Some(detail))
    }, None => Ok(None) }
  }
  pub fn set_email_star(&self, id: &str, starred: bool) -> Result<(), AppError> { self.connection.execute("UPDATE emails SET is_starred = ?2, updated_at = ?3, sync_status = 'pending', sync_version = sync_version + 1 WHERE id = ?1 AND deleted_at IS NULL", params![id, starred as i64, now()])?; self.enqueue("email", id, "update")?; Ok(()) }
  pub fn archive_email(&self, id: &str) -> Result<(), AppError> { self.move_email_to_folder(id, "archive") }
  pub fn trash_email(&self, id: &str) -> Result<(), AppError> { self.move_email_to_folder(id, "trash") }
  pub fn mark_email_read(&self, id: &str, is_read: bool) -> Result<(), AppError> { self.connection.execute("UPDATE emails SET is_read = ?2, updated_at = ?3, sync_status = 'pending', sync_version = sync_version + 1 WHERE id = ?1 AND deleted_at IS NULL", params![id, is_read as i64, now()])?; self.enqueue("email", id, "update")?; Ok(()) }
  /// Records a body that was decoded from the server.
  ///
  /// Deliberately **no** `sync_status`/`enqueue` here, unlike the flag setters
  /// above: a body read back from IMAP is remote truth, not a local edit, so it
  /// must not queue an outbound change.
  pub fn store_email_body(&self, id: &str, body_text: &str) -> Result<(), AppError> {
    self.connection.execute("UPDATE emails SET body_text = ?2, updated_at = ?3 WHERE id = ?1 AND deleted_at IS NULL", params![id, body_text, now()])?;
    Ok(())
  }
  /// The account and IMAP UID needed to (re)fetch one message's body.
  ///
  /// `None` when the row has no server counterpart — a draft or a sent message —
  /// or when `server_id` is not a UID, which is also how those rows are stored.
  pub fn email_fetch_target(&self, id: &str) -> Result<Option<(String, u32)>, AppError> {
    let mut statement = self.connection.prepare("SELECT account_id, server_id FROM emails WHERE id = ?1 AND deleted_at IS NULL")?;
    let mut rows = statement.query(params![id])?;
    let Some(row) = rows.next()? else { return Ok(None) };
    let account_id: String = row.get(0)?;
    let server_id: Option<String> = row.get(1)?;
    let uid = server_id.and_then(|value| value.trim().parse::<u32>().ok());
    Ok(uid.map(|uid| (account_id, uid)))
  }
  pub fn accounts(&self) -> Result<Vec<Account>, AppError> {
    let mut statement = self.connection.prepare("SELECT id, display_name, email_address, status, connection_state, connection_error, last_verified_at FROM accounts WHERE deleted_at IS NULL ORDER BY display_name")?;
    let rows = statement.query_map([], |row| Ok(Account { id: row.get(0)?, display_name: row.get(1)?, email_address: row.get(2)?, status: row.get(3)?, connection_state: row.get(4)?, connection_error: row.get(5)?, last_verified_at: row.get(6)? }))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
  }
  /// Persists an account whose password has already been verified against the
  /// IMAP server (`verify::verify_imap_login`) and stored in the OS keyring.
  /// The password itself is never passed here and never written to the database.
  pub fn add_account_verified(&self, input: &CreateAccount, credential_ref: &str) -> Result<Account, AppError> {
    validate_required(&input.display_name)?; validate_email(&input.email_address)?; validate_required(&input.imap_host)?; validate_required(&input.smtp_host)?;
    let id = Uuid::new_v4().to_string(); let created_at = now();
    self.connection.execute("INSERT INTO accounts (id, display_name, email_address, imap_host, imap_port, smtp_host, smtp_port, encryption, auth_kind, status, connection_state, credential_ref, last_verified_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending', 'connected', ?10, ?11, ?11, ?11)", params![id, input.display_name, input.email_address, input.imap_host, input.imap_port, input.smtp_host, input.smtp_port, input.encryption, input.auth_kind, credential_ref, created_at])?;
    // Keep the local user's messenger identity in step with the signed-in
    // account so sent messages and reactions display the real name.
    self.connection.execute("INSERT INTO contacts (id, name, email, favorite, created_at, updated_at, sync_status, sync_version) VALUES (?1, ?2, ?3, 0, ?4, ?4, 'synced', 1) ON CONFLICT(id) DO UPDATE SET name = excluded.name, email = excluded.email, updated_at = excluded.updated_at", params![SELF_CONTACT_ID, input.display_name, input.email_address, created_at])?;
    self.enqueue("account", &id, "create")?;
    // A real account must own its folders before any mail can be filed into
    // them, or the folder views have nothing to match on.
    self.ensure_account_folders(&id)?;
    Ok(Account { id, display_name: input.display_name.clone(), email_address: input.email_address.clone(), status: "pending".into(), connection_state: "connected".into(), connection_error: None, last_verified_at: Some(created_at) })
  }
  /// Creates the account's default folders if they are missing.
  ///
  /// Only the debug seed and `move_email_to_folder` ever created folder rows, so
  /// a real account had **none** — and `emails_in_folder` filters on the folder
  /// role, which made a freshly signed-in mailbox permanently empty regardless
  /// of what was stored. Called on sign-in and before every sync, so existing
  /// accounts are repaired too. Idempotent.
  pub fn ensure_account_folders(&self, account_id: &str) -> Result<(), AppError> {
    if account_id.is_empty() || account_id.len() > 128 { return Err(AppError::Validation); }
    let timestamp = now();
    for (role, name) in [("inbox", "Inbox"), ("sent", "Sent"), ("drafts", "Drafts"), ("archive", "Archive"), ("trash", "Trash")] {
      let existing: Option<String> = self.connection
        .query_row("SELECT id FROM email_folders WHERE account_id = ?1 AND role = ?2 AND deleted_at IS NULL LIMIT 1", params![account_id, role], |row| row.get(0))
        .ok();
      if existing.is_some() { continue; }
      self.connection.execute(
        "INSERT INTO email_folders (id, account_id, name, role, created_at, updated_at, sync_status) VALUES (?1, ?2, ?3, ?4, ?5, ?5, 'synced')",
        params![Uuid::new_v4().to_string(), account_id, name, role, timestamp],
      )?;
    }
    Ok(())
  }
  /// Stores fetched messages for one account inside a single transaction.
  ///
  /// Rows are keyed by (account, IMAP UID) through a deterministic id, so a
  /// repeated sync updates the existing row instead of duplicating the mailbox.
  /// A locally filed message keeps its folder: archiving or trashing a message
  /// must not be undone by the next sync (only its content and flags refresh).
  /// A row marked deleted is skipped entirely, so a sync never resurrects it.
  /// Returns the number of messages written.
  pub fn upsert_fetched_messages(&self, account_id: &str, folder_role: &str, messages: &[FetchedMessage]) -> Result<i64, AppError> {
    if account_id.is_empty() || account_id.len() > 128 || !is_mail_role(folder_role) { return Err(AppError::Validation); }
    let folder_id: String = self.connection
      .query_row("SELECT id FROM email_folders WHERE account_id = ?1 AND role = ?2 AND deleted_at IS NULL LIMIT 1", params![account_id, folder_role], |row| row.get(0))
      .map_err(|_| AppError::Validation)?;
    let timestamp = now();
    self.connection.execute_batch("BEGIN")?;
    let outcome = (|| -> Result<i64, AppError> {
      let mut written = 0i64;
      for message in messages {
        let id = format!("imap-{account_id}-{}", message.uid);
        let deleted: Option<String> = self.connection
          .query_row("SELECT deleted_at FROM emails WHERE id = ?1", params![id], |row| row.get(0))
          .ok()
          .flatten();
        if deleted.is_some() { continue; }
        self.connection.execute(
          "INSERT INTO emails (id, account_id, folder_id, sender_name, sender_email, subject, body_text, message_id_header, received_at, is_read, is_starred, created_at, updated_at, direction, delivery_state, server_id, sync_status)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12, 'inbound', 'received', ?13, 'synced')
           ON CONFLICT(id) DO UPDATE SET sender_name = excluded.sender_name, sender_email = excluded.sender_email, subject = excluded.subject, body_text = excluded.body_text, message_id_header = excluded.message_id_header, received_at = excluded.received_at, is_read = excluded.is_read, is_starred = excluded.is_starred, updated_at = excluded.updated_at, server_id = excluded.server_id",
          params![id, account_id, folder_id, message.sender_name, message.sender_email, message.subject, message.body_text, message.message_id, message.received_at, message.is_read as i64, message.is_starred as i64, timestamp, message.uid.to_string()],
        )?;
        written += 1;
      }
      Ok(written)
    })();
    match outcome {
      Ok(written) => { self.connection.execute_batch("COMMIT")?; Ok(written) }
      Err(error) => { let _ = self.connection.execute_batch("ROLLBACK"); Err(error) }
    }
  }
  /// Reads one sync-metadata value (used for the per-folder UIDVALIDITY/UIDNEXT
  /// bookkeeping that makes repeat syncs incremental).
  pub fn sync_metadata(&self, key: &str) -> Result<Option<String>, AppError> {
    if key.is_empty() || key.len() > 128 { return Err(AppError::Validation); }
    Ok(self.connection.query_row("SELECT value FROM sync_metadata WHERE key = ?1", params![key], |row| row.get(0)).ok())
  }
  pub fn set_sync_metadata(&self, key: &str, value: &str) -> Result<(), AppError> {
    if key.is_empty() || key.len() > 128 || value.len() > 512 { return Err(AppError::Validation); }
    self.connection.execute(
      "INSERT INTO sync_metadata (key, value, updated_at) VALUES (?1, ?2, ?3) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
      params![key, value, now()],
    )?;
    Ok(())
  }
  /// Ids of every account that has not been removed, so a sync can cover all of
  /// them instead of guessing a "primary" account.
  pub fn account_ids(&self) -> Result<Vec<String>, AppError> {
    let mut statement = self.connection.prepare("SELECT id FROM accounts WHERE deleted_at IS NULL ORDER BY created_at")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
  }
  fn account(&self, id: &str) -> Result<Option<Account>, AppError> {
    let mut statement = self.connection.prepare("SELECT id, display_name, email_address, status, connection_state, connection_error, last_verified_at FROM accounts WHERE id = ?1 AND deleted_at IS NULL")?;
    let mut rows = statement.query(params![id])?;
    match rows.next()? {
      Some(row) => Ok(Some(Account { id: row.get(0)?, display_name: row.get(1)?, email_address: row.get(2)?, status: row.get(3)?, connection_state: row.get(4)?, connection_error: row.get(5)?, last_verified_at: row.get(6)? })),
      None => Ok(None),
    }
  }
  /// Looks up a non-deleted account by email address. Sign-in uses this to make
  /// re-authentication idempotent instead of failing on the UNIQUE index.
  pub fn account_by_email(&self, email_address: &str) -> Result<Option<Account>, AppError> {
    let mut statement = self.connection.prepare("SELECT id, display_name, email_address, status, connection_state, connection_error, last_verified_at FROM accounts WHERE email_address = ?1 AND deleted_at IS NULL")?;
    let mut rows = statement.query(params![email_address])?;
    match rows.next()? {
      Some(row) => Ok(Some(Account { id: row.get(0)?, display_name: row.get(1)?, email_address: row.get(2)?, status: row.get(3)?, connection_state: row.get(4)?, connection_error: row.get(5)?, last_verified_at: row.get(6)? })),
      None => Ok(None),
    }
  }
  /// The subset of the account row needed to re-verify a stored sign-in.
  pub fn account_connection(&self, id: &str) -> Result<Option<AccountConnection>, AppError> {
    let mut statement = self.connection.prepare("SELECT id, email_address, imap_host, imap_port, smtp_host, smtp_port, encryption, auth_kind FROM accounts WHERE id = ?1 AND deleted_at IS NULL")?;
    let mut rows = statement.query(params![id])?;
    match rows.next()? {
      Some(row) => Ok(Some(AccountConnection { id: row.get(0)?, email_address: row.get(1)?, imap_host: row.get(2)?, imap_port: row.get(3)?, smtp_host: row.get(4)?, smtp_port: row.get(5)?, encryption: row.get(6)?, auth_kind: row.get(7)? })),
      None => Ok(None),
    }
  }
  /// Records the outcome of a sign-in attempt. `bump_verified` also refreshes
  /// `last_verified_at`; a failing attempt keeps the previous timestamp.
  pub fn set_account_connection_state(&self, id: &str, state: &str, error: Option<&str>, bump_verified: bool) -> Result<Account, AppError> {
    if state != "unverified" && state != "connected" && state != "error" { return Err(AppError::Validation); }
    let timestamp = now();
    let verified_at: Option<String> = if bump_verified { Some(timestamp.clone()) } else { None };
    self.connection.execute("UPDATE accounts SET connection_state = ?2, connection_error = ?3, last_verified_at = COALESCE(?4, last_verified_at), updated_at = ?5, sync_status = 'pending', sync_version = sync_version + 1 WHERE id = ?1 AND deleted_at IS NULL", params![id, state, error, verified_at, timestamp])?;
    self.enqueue("account", id, "update")?;
    self.account(id)?.ok_or(AppError::Validation)
  }
  pub fn contacts(&self) -> Result<Vec<Contact>, AppError> {
    let mut statement = self.connection.prepare("SELECT id, name, email, department, position, favorite FROM contacts WHERE deleted_at IS NULL ORDER BY favorite DESC, name LIMIT 500")?;
    let rows = statement.query_map([], |row| Ok(Contact { id: row.get(0)?, name: row.get(1)?, email: row.get(2)?, department: row.get(3)?, position: row.get(4)?, favorite: row.get::<_, i64>(5)? != 0 }))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
  }
  pub fn create_contact(&self, input: CreateContact) -> Result<Contact, AppError> {
    validate_required(&input.name)?; if let Some(email) = &input.email { validate_email(email)?; }
    let id = Uuid::new_v4().to_string(); let timestamp = now();
    self.connection.execute("INSERT INTO contacts (id, name, email, department, position, favorite, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?6)", params![id, input.name, input.email, input.department, input.position, timestamp])?;
    self.enqueue("contact", &id, "create")?;
    Ok(Contact { id, name: input.name, email: input.email, department: input.department, position: input.position, favorite: false })
  }
  /// Guarantees the local user's contact row exists and returns its display
  /// name. Reactions and channel membership carry a foreign key into
  /// `contacts`, so messenger writes fail without this row; release builds
  /// (which never run the debug seed) create it here, preferring the display
  /// name of the first signed-in account.
  pub fn ensure_self_identity(&self) -> Result<String, AppError> {
    let existing: Option<String> = self.connection.query_row("SELECT name FROM contacts WHERE id = ?1", params![SELF_CONTACT_ID], |row| row.get(0)).ok();
    if let Some(name) = existing { return Ok(name); }
    let (name, email): (String, Option<String>) = self.connection.query_row("SELECT display_name, email_address FROM accounts WHERE deleted_at IS NULL ORDER BY created_at LIMIT 1", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap_or_else(|_| ("Me".into(), None));
    let timestamp = now();
    self.connection.execute("INSERT INTO contacts (id, name, email, favorite, created_at, updated_at, sync_status, sync_version) VALUES (?1, ?2, ?3, 0, ?4, ?4, 'synced', 1)", params![SELF_CONTACT_ID, name, email, timestamp])?;
    Ok(name)
  }
  pub fn pending_sync_items(&self) -> Result<Vec<SyncQueueItem>, AppError> {
    let mut statement = self.connection.prepare("SELECT id, entity_type, entity_id, operation, attempt_count, last_error FROM sync_queue WHERE status != 'completed' ORDER BY created_at LIMIT 100")?;
    let rows = statement.query_map([], |row| Ok(SyncQueueItem { id: row.get(0)?, entity_type: row.get(1)?, entity_id: row.get(2)?, operation: row.get(3)?, attempt_count: row.get(4)?, last_error: row.get(5)? }))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
  }
  pub fn save_draft(&self, input: DraftInput, existing_id: Option<&str>) -> Result<Draft, AppError> {
    self.validate_draft(&input)?;
    let timestamp = now(); let id = existing_id.map(str::to_owned).unwrap_or_else(|| Uuid::new_v4().to_string());
    let draft_folder: String = self.connection.query_row("SELECT id FROM email_folders WHERE account_id = ?1 AND role = 'drafts' AND deleted_at IS NULL LIMIT 1", params![input.account_id], |row| row.get(0)).unwrap_or_else(|_| "local-drafts".into());
    if existing_id.is_some() { self.connection.execute("UPDATE emails SET subject = ?2, body_text = ?3, updated_at = ?4, sync_status = 'pending', sync_version = sync_version + 1 WHERE id = ?1 AND direction = 'outbound'", params![id, input.subject, input.body_text, timestamp])?; self.connection.execute("DELETE FROM email_recipients WHERE email_id = ?1", params![id])?; }
    else { self.connection.execute("INSERT INTO emails (id, account_id, folder_id, subject, body_text, received_at, is_read, created_at, updated_at, direction, delivery_state, sync_status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?6, ?6, 'outbound', 'draft', 'pending')", params![id, input.account_id, draft_folder, input.subject, input.body_text, timestamp])?; }
    self.save_recipients(&id, "to", &input.to, &timestamp)?; self.save_recipients(&id, "cc", &input.cc, &timestamp)?; self.save_recipients(&id, "bcc", &input.bcc, &timestamp)?;
    self.enqueue("email", &id, "save_draft")?;
    Ok(Draft { id, delivery_state: "draft".into(), updated_at: timestamp })
  }
  pub fn queue_send(&self, id: &str) -> Result<Draft, AppError> {
    let recipients: i64 = self.connection.query_row("SELECT COUNT(*) FROM email_recipients WHERE email_id = ?1 AND recipient_type = 'to'", params![id], |row| row.get(0))?;
    if recipients == 0 { return Err(AppError::Validation); }
    let timestamp = now();
    let queued = self.connection.execute("UPDATE emails SET delivery_state = 'queued', updated_at = ?2, sync_status = 'pending', sync_version = sync_version + 1 WHERE id = ?1 AND direction = 'outbound'", params![id, timestamp])?;
    if queued == 0 { return Err(AppError::Validation); }
    // The outbound copy lives in the account's Sent folder so queued mail is
    // visible in the Sent view; actual delivery stays pending until an SMTP
    // transport consumes the durable `send_smtp` queue item.
    self.move_email_to_folder(id, "sent")?;
    self.enqueue("email", id, "send_smtp")?;
    Ok(Draft { id: id.into(), delivery_state: "queued".into(), updated_at: timestamp })
  }
  /// Resolves every outbound message that still needs delivering, claiming each
  /// one for the caller.
  ///
  /// A message qualifies while its delivery state is `queued` (never attempted)
  /// or `failed` (another attempt is allowed) and its durable `send_smtp` item is
  /// unfinished. Claiming marks that item `syncing` with a `locked_at` stamp, so
  /// two overlapping runs cannot transmit the same message — the second snapshot
  /// simply no longer matches it. A claim older than
  /// [`DELIVERY_CLAIM_MINUTES`] is treated as abandoned (a run that died
  /// mid-session) and becomes eligible again. The terminal marks are what make
  /// delivery idempotent: `completed` on success stops any future run, `failed`
  /// on a refusal invites one. `ids` narrows the work to the messages the UI
  /// asked about (the reading pane's Try again).
  pub fn queued_sends(&self, ids: Option<&[String]>) -> Result<Vec<QueuedSend>, AppError> {
    let timestamp = now();
    // An abandoned claim is one an earlier run never finished; without this a
    // crash mid-session would strand the message until the queue row was cleared.
    let stale_before = (Utc::now() - chrono::Duration::minutes(DELIVERY_CLAIM_MINUTES as i64)).to_rfc3339();
    self.connection.execute_batch("BEGIN")?;
    let outcome = (|| -> Result<Vec<QueuedSend>, AppError> {
      let mut statement = self.connection.prepare("SELECT e.id, e.account_id, e.sender_name, e.sender_email, e.subject, e.body_text FROM emails e WHERE e.direction = 'outbound' AND e.deleted_at IS NULL AND e.delivery_state IN ('queued', 'failed') AND EXISTS (SELECT 1 FROM sync_queue q WHERE q.entity_type = 'email' AND q.entity_id = e.id AND q.operation = 'send_smtp' AND (q.status IN ('pending', 'failed') OR (q.status = 'syncing' AND (q.locked_at IS NULL OR q.locked_at < ?1)))) ORDER BY e.created_at")?;
      let rows = statement.query_map(params![stale_before], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, Option<String>>(3)?, row.get::<_, String>(4)?, row.get::<_, String>(5)?)))?;
      let mut sends = Vec::new();
      for row in rows {
        let (id, account_id, sender_name, sender_email, subject, body_text) = row?;
        if let Some(wanted) = ids {
          if !wanted.iter().any(|candidate| candidate == &id) {
            continue;
          }
        }
        let connection = self.account_connection(&account_id)?;
        // The from identity prefers what the row stored (an account can be
        // re-configured after a send was queued) and falls back to the account's
        // own address.
        let (from_name, from_address) = match &connection {
          Some(account) => (sender_name.unwrap_or_else(|| account.email_address.clone()), sender_email.unwrap_or_else(|| account.email_address.clone())),
          None => (sender_name.unwrap_or_default(), sender_email.unwrap_or_default()),
        };
        let mut to = Vec::new();
        let mut cc = Vec::new();
        let mut bcc = Vec::new();
        let mut recipients = self.connection.prepare("SELECT recipient_type, email_address FROM email_recipients WHERE email_id = ?1 ORDER BY created_at, rowid")?;
        let rows = recipients.query_map(params![id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
        for recipient in rows {
          let (kind, address) = recipient?;
          match kind.as_str() {
            "to" => to.push(address),
            "cc" => cc.push(address),
            "bcc" => bcc.push(address),
            _ => {}
          }
        }
        sends.push(QueuedSend { connection, message: OutboundMessage { id, from_name, from_address, to, cc, bcc, subject, body_text } });
      }
      // Claim before the transaction commits, so the claim and the snapshot are
      // one atomic step: either a run sees the message and owns it, or it does
      // not see it at all.
      for send in &sends {
        self.connection.execute("UPDATE sync_queue SET status = 'syncing', locked_at = ?2, updated_at = ?2 WHERE entity_type = 'email' AND entity_id = ?1 AND operation = 'send_smtp'", params![send.message.id, timestamp])?;
      }
      Ok(sends)
    })();
    match outcome {
      Ok(sends) => { self.connection.execute_batch("COMMIT")?; Ok(sends) }
      Err(error) => { let _ = self.connection.execute_batch("ROLLBACK"); Err(error) }
    }
  }
  /// Returns claims left behind by a previous process to the queue.
  ///
  /// A claim (`status = 'syncing'`) cannot outlive the process that made it: an
  /// SMTP session is never in flight at startup, so every `syncing` row found
  /// here was abandoned by a crash or a kill. Without this reset such a message
  /// would look in-flight (and be skipped by [`Self::queued_sends`]) until the
  /// stale window expired, while nothing was actually sending it. Delivery
  /// claims are the only writer of `syncing` today, and the reset is deliberately
  /// unscoped so any future worker gets the same recovery.
  pub fn release_abandoned_delivery_claims(&self) -> Result<i64, AppError> {
    let released = self.connection.execute("UPDATE sync_queue SET status = 'pending', locked_at = NULL, updated_at = ?1 WHERE status = 'syncing'", params![now()])?;
    Ok(released as i64)
  }
  /// Records a delivered message: the Sent copy is real mail now, and the
  /// durable `send_smtp` item is finished so nothing retries it.
  pub fn mark_delivery_sent(&self, id: &str) -> Result<(), AppError> {
    let timestamp = now();
    self.connection.execute("UPDATE emails SET delivery_state = 'sent', updated_at = ?2 WHERE id = ?1 AND direction = 'outbound'", params![id, timestamp])?;
    self.connection.execute("UPDATE sync_queue SET status = 'completed', completed_at = ?2, last_error = NULL, locked_at = NULL, updated_at = ?2 WHERE entity_type = 'email' AND entity_id = ?1 AND operation = 'send_smtp'", params![id, timestamp])?;
    Ok(())
  }
  /// Records a failed delivery attempt. The queue row keeps the classifier's
  /// fixed wording (so the reading pane can show it) and counts one more
  /// attempt, while the message stays retryable.
  pub fn mark_delivery_failed(&self, id: &str, error: &str) -> Result<(), AppError> {
    let timestamp = now();
    self.connection.execute("UPDATE emails SET delivery_state = 'failed', updated_at = ?2 WHERE id = ?1 AND direction = 'outbound'", params![id, timestamp])?;
    self.connection.execute("UPDATE sync_queue SET status = 'failed', attempt_count = attempt_count + 1, last_error = ?2, locked_at = NULL, updated_at = ?3 WHERE entity_type = 'email' AND entity_id = ?1 AND operation = 'send_smtp'", params![id, error, timestamp])?;
    Ok(())
  }
  pub fn channels(&self) -> Result<Vec<Channel>, AppError> {
    let mut statement = self.connection.prepare("SELECT c.id, COALESCE(c.title, c.channel_slug, 'Untitled'), c.channel_slug, c.description, COUNT(cm.contact_id) FROM conversations c LEFT JOIN channel_members cm ON cm.channel_id = c.id WHERE c.kind = 'channel' AND c.deleted_at IS NULL GROUP BY c.id ORDER BY c.updated_at DESC")?;
    let rows = statement.query_map([], |row| Ok(Channel { id: row.get(0)?, title: row.get(1)?, slug: row.get(2)?, description: row.get(3)?, member_count: row.get(4)? }))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
  }
  /// Channel history: exactly one row per thread (its root), pinned threads
  /// first, then the chronological rest. Replies are never flattened into this
  /// list — a root carries `reply_count`/`last_reply_at` plus the aggregated
  /// reactions of its whole thread, and `message_thread` loads the replies on
  /// demand.
  pub fn channel_messages(&self, channel_id: &str) -> Result<Vec<ChatMessage>, AppError> {
    let mut statement = self.connection.prepare("SELECT m.id, m.conversation_id, COALESCE(NULLIF(m.thread_id, ''), m.id), COALESCE(c.name, 'Unknown'), m.body, COALESCE(m.sent_at, m.created_at), (SELECT COUNT(*) FROM messages r WHERE r.thread_id = m.id AND r.id != m.id AND r.deleted_at IS NULL), (SELECT MAX(COALESCE(r.sent_at, r.created_at)) FROM messages r WHERE r.thread_id = m.id AND r.id != m.id AND r.deleted_at IS NULL), m.edited_at IS NOT NULL, m.pinned_at IS NOT NULL, m.sender_id = ?2 FROM messages m LEFT JOIN contacts c ON c.id = m.sender_id WHERE m.conversation_id = ?1 AND m.deleted_at IS NULL AND m.reply_to_id IS NULL AND COALESCE(m.thread_id, m.id) = m.id ORDER BY m.pinned_at IS NOT NULL DESC, m.pinned_at DESC, COALESCE(m.sent_at, m.created_at) ASC LIMIT 250")?;
    let rows = statement.query_map(params![channel_id, SELF_CONTACT_ID], |row| Ok(ChatMessage { id: row.get(0)?, conversation_id: row.get(1)?, thread_id: row.get(2)?, sender_name: row.get(3)?, body: row.get(4)?, sent_at: row.get(5)?, reply_count: row.get(6)?, last_reply_at: row.get(7)?, edited: row.get(8)?, pinned: row.get(9)?, mine: row.get(10)?, reactions: vec![] }))?;
    let mut messages = rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)?;
    let mut aggregates = self.thread_reactions(channel_id)?;
    for message in messages.iter_mut() { if let Some(summary) = aggregates.remove(&message.id) { message.reactions = summary; } }
    Ok(messages)
  }
  /// Aggregated reactions of every thread in a conversation, keyed by thread root
  /// id: one query instead of one per row, because the history holds up to 250
  /// roots. Reactions always live on the root (`toggle_message_reaction`
  /// normalizes the target), so the root id is the only key needed.
  fn thread_reactions(&self, conversation_id: &str) -> Result<HashMap<String, Vec<ReactionSummary>>, AppError> {
    let mut statement = self.connection.prepare("SELECT r.message_id, r.emoji, COUNT(*), SUM(CASE WHEN r.contact_id = ?2 THEN 1 ELSE 0 END) FROM message_reactions r JOIN messages m ON m.id = r.message_id WHERE m.conversation_id = ?1 AND m.deleted_at IS NULL GROUP BY r.message_id, r.emoji ORDER BY r.emoji")?;
    let rows = statement.query_map(params![conversation_id, SELF_CONTACT_ID], |row| Ok((row.get::<_, String>(0)?, ReactionSummary { emoji: row.get(1)?, count: row.get(2)?, reacted_by_me: row.get::<_, i64>(3)? != 0 })))?;
    let mut grouped: HashMap<String, Vec<ReactionSummary>> = HashMap::new();
    for row in rows { let (message_id, summary) = row?; grouped.entry(message_id).or_default().push(summary); }
    Ok(grouped)
  }
  /// Reactions of a single thread root, ordered by emoji.
  fn reaction_groups(&self, root_id: &str) -> Result<Vec<ReactionSummary>, AppError> {
    let mut statement = self.connection.prepare("SELECT emoji, COUNT(*), SUM(CASE WHEN contact_id = ?2 THEN 1 ELSE 0 END) FROM message_reactions WHERE message_id = ?1 GROUP BY emoji ORDER BY emoji")?;
    let rows = statement.query_map(params![root_id, SELF_CONTACT_ID], |row| Ok(ReactionSummary { emoji: row.get(0)?, count: row.get(1)?, reacted_by_me: row.get::<_, i64>(2)? != 0 }))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
  }
  pub fn send_message(&self, input: SendMessageInput) -> Result<ChatMessage, AppError> {
    validate_required(&input.conversation_id)?; validate_required(&input.body)?; if input.body.len() > 20_000 { return Err(AppError::Validation); }
    let id = Uuid::new_v4().to_string(); let timestamp = now();
    // A reply never starts a thread of its own: it joins the thread of the
    // message it answers (`thread_id`), and the target must be a live message of
    // the same conversation so a stale client cannot graft history across
    // conversations. `in_reply_to` keeps the exact parent for UI context.
    let thread_id = match input.reply_to_id.as_deref() {
      Some(parent_id) => {
        let parent: (String, String) = self.connection.query_row("SELECT conversation_id, COALESCE(NULLIF(thread_id, ''), id) FROM messages WHERE id = ?1 AND deleted_at IS NULL", params![parent_id], |row| Ok((row.get(0)?, row.get(1)?))).map_err(|_| AppError::Validation)?;
        if parent.0 != input.conversation_id { return Err(AppError::Validation); }
        parent.1
      }
      None => id.clone(),
    };
    self.connection.execute("INSERT INTO messages (id, conversation_id, sender_id, body, reply_to_id, in_reply_to, thread_id, sent_at, created_at, updated_at, sync_status) VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?7, ?7, 'pending')", params![id, input.conversation_id, SELF_CONTACT_ID, input.body, input.reply_to_id, thread_id, timestamp])?;
    self.connection.execute("UPDATE conversations SET updated_at = ?2, sync_status = 'pending', sync_version = sync_version + 1 WHERE id = ?1", params![input.conversation_id, timestamp])?;
    self.enqueue("message", &id, "send_message")?;
    // Read the row back so sender identity comes from the self contact
    // instead of a hard-coded name.
    self.stored_message(&id)
  }
  pub fn conversations(&self) -> Result<Vec<Conversation>, AppError> {
    let mut statement = self.connection.prepare("SELECT c.id, c.kind, CASE WHEN c.kind = 'dm' THEN COALESCE((SELECT c2.name FROM channel_members cm JOIN contacts c2 ON c2.id = cm.contact_id WHERE cm.channel_id = c.id AND cm.contact_id != ?1 LIMIT 1), COALESCE(NULLIF(c.title, ''), 'Direct message')) ELSE COALESCE(NULLIF(c.title, ''), c.channel_slug, 'Channel') END, c.description, (SELECT COUNT(*) FROM channel_members cm3 WHERE cm3.channel_id = c.id), COALESCE((SELECT MAX(COALESCE(m.sent_at, m.created_at)) FROM messages m WHERE m.conversation_id = c.id AND m.deleted_at IS NULL), c.updated_at) FROM conversations c WHERE c.deleted_at IS NULL AND c.kind IN ('dm', 'channel') ORDER BY 6 DESC LIMIT 100")?;
    let rows = statement.query_map(params![SELF_CONTACT_ID], |row| Ok(Conversation { id: row.get(0)?, kind: row.get(1)?, title: row.get(2)?, description: row.get(3)?, member_count: row.get(4)?, last_activity_at: row.get(5)? }))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
  }
  pub fn open_direct_message(&self, contact_id: &str) -> Result<Conversation, AppError> {
    validate_required(contact_id)?;
    let name: String = self.connection.query_row("SELECT name FROM contacts WHERE id = ?1 AND deleted_at IS NULL", params![contact_id], |row| row.get(0)).map_err(|_| AppError::Validation)?;
    let timestamp = now();
    let existing: String = self.connection.query_row("SELECT c.id FROM conversations c WHERE c.kind = 'dm' AND c.deleted_at IS NULL AND (EXISTS (SELECT 1 FROM channel_members a WHERE a.channel_id = c.id AND a.contact_id = ?1) OR c.title = ?2) LIMIT 1", params![contact_id, name], |row| row.get(0)).unwrap_or_default();
    let id = if existing.is_empty() {
      let id = Uuid::new_v4().to_string();
      self.connection.execute("INSERT INTO conversations (id, kind, title, created_at, updated_at, sync_status) VALUES (?1, 'dm', ?2, ?3, ?3, 'pending')", params![id, name, timestamp])?;
      self.connection.execute("INSERT INTO channel_members (channel_id, contact_id, role, joined_at) VALUES (?1, ?2, 'member', ?3), (?1, ?4, 'member', ?3)", params![id, contact_id, timestamp, SELF_CONTACT_ID])?;
      self.enqueue("conversation", &id, "create")?;
      id
    } else { existing };
    Ok(Conversation { id, kind: "dm".into(), title: name, description: None, member_count: 2, last_activity_at: timestamp })
  }
  pub fn edit_message(&self, message_id: &str, body: &str) -> Result<ChatMessage, AppError> {
    validate_required(body)?;
    let timestamp = now();
    let updated = self.connection.execute("UPDATE messages SET body = ?2, edited_at = ?3, updated_at = ?3, sync_status = 'pending', sync_version = sync_version + 1 WHERE id = ?1 AND deleted_at IS NULL", params![message_id, body, timestamp])?;
    if updated == 0 { return Err(AppError::Validation); }
    self.enqueue("message", message_id, "edit")?;
    self.stored_message(message_id)
  }
  pub fn delete_message(&self, message_id: &str) -> Result<(), AppError> {
    let timestamp = now();
    // Deleting a thread root removes the whole thread: the channel history only
    // lists roots, so replies left behind would become unreachable. Deleting a
    // reply removes just that reply.
    let updated = self.connection.execute("UPDATE messages SET deleted_at = ?2, updated_at = ?2, sync_status = 'pending', sync_version = sync_version + 1 WHERE deleted_at IS NULL AND (id = ?1 OR (reply_to_id IS NOT NULL AND COALESCE(thread_id, id) = ?1))", params![message_id, timestamp])?;
    if updated == 0 { return Err(AppError::Validation); }
    self.enqueue("message", message_id, "delete")?;
    Ok(())
  }
  pub fn set_message_pin(&self, message_id: &str, pinned: bool) -> Result<(), AppError> {
    let timestamp = now();
    let updated = self.connection.execute("UPDATE messages SET pinned_at = ?2, updated_at = ?3, sync_status = 'pending', sync_version = sync_version + 1 WHERE id = ?1 AND deleted_at IS NULL", params![message_id, if pinned { Some(timestamp.clone()) } else { None }, timestamp])?;
    if updated == 0 { return Err(AppError::Validation); }
    self.enqueue("message", message_id, if pinned { "pin" } else { "unpin" })?;
    Ok(())
  }
  fn stored_message(&self, id: &str) -> Result<ChatMessage, AppError> {
    let mut statement = self.connection.prepare("SELECT m.id, m.conversation_id, COALESCE(NULLIF(m.thread_id, ''), m.id), COALESCE(c.name, 'Unknown'), m.body, COALESCE(m.sent_at, m.created_at), (SELECT COUNT(*) FROM messages r WHERE r.thread_id = m.id AND r.id != m.id AND r.deleted_at IS NULL), (SELECT MAX(COALESCE(r.sent_at, r.created_at)) FROM messages r WHERE r.thread_id = m.id AND r.id != m.id AND r.deleted_at IS NULL), m.edited_at IS NOT NULL, m.pinned_at IS NOT NULL, m.sender_id = ?2 FROM messages m LEFT JOIN contacts c ON c.id = m.sender_id WHERE m.id = ?1 AND m.deleted_at IS NULL")?;
    let mut rows = statement.query_map(params![id, SELF_CONTACT_ID], |row| Ok(ChatMessage { id: row.get(0)?, conversation_id: row.get(1)?, thread_id: row.get(2)?, sender_name: row.get(3)?, body: row.get(4)?, sent_at: row.get(5)?, reply_count: row.get(6)?, last_reply_at: row.get(7)?, edited: row.get(8)?, pinned: row.get(9)?, mine: row.get(10)?, reactions: vec![] }))?;
    let mut message = rows.next().transpose()?.ok_or(AppError::Validation)?;
    // The row is handed back to the caller as the thread row it belongs to, so
    // the client can splice it into the history without a second query.
    message.reactions = self.reaction_groups(&message.thread_id)?;
    Ok(message)
  }
  pub fn notifications(&self) -> Result<Vec<Notification>, AppError> {
    let mut statement = self.connection.prepare("SELECT id, kind, title, body, entity_type, entity_id, created_at FROM notifications WHERE deleted_at IS NULL ORDER BY is_read ASC, created_at DESC LIMIT 100")?;
    let rows = statement.query_map([], |row| Ok(Notification { id: row.get(0)?, kind: row.get(1)?, title: row.get(2)?, body: row.get(3)?, entity_type: row.get(4)?, entity_id: row.get(5)?, created_at: row.get(6)? }))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
  }
  pub fn unread_notification_count(&self) -> Result<i64, AppError> { Ok(self.connection.query_row("SELECT COUNT(*) FROM notifications WHERE is_read = 0 AND deleted_at IS NULL", [], |row| row.get(0))?) }
  pub fn mark_notifications_read(&self) -> Result<(), AppError> { self.connection.execute("UPDATE notifications SET is_read = 1, read_at = ?1 WHERE is_read = 0 AND deleted_at IS NULL", params![now()])?; Ok(()) }
  pub fn notification_preferences(&self) -> Result<Vec<NotificationPreference>, AppError> {
    let mut statement = self.connection.prepare("SELECT key, enabled FROM notification_preferences ORDER BY key")?;
    let rows = statement.query_map([], |row| Ok(NotificationPreference { key: row.get(0)?, enabled: row.get::<_, i64>(1)? != 0 }))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
  }
  pub fn update_notification_preference(&self, input: UpdateNotificationPreference) -> Result<(), AppError> { validate_required(&input.key)?; self.connection.execute("UPDATE notification_preferences SET enabled = ?2, updated_at = ?3 WHERE key = ?1", params![input.key, input.enabled as i64, now()])?; Ok(()) }
  /// Replies of a thread in chronological order. Any member of the thread can be
  /// passed in: the query resolves its thread root and returns that thread's
  /// replies (the root row itself is rendered by the channel history, so it is
  /// excluded here).
  pub fn message_thread(&self, message_id: &str) -> Result<Vec<ChatMessage>, AppError> {
    let mut statement = self.connection.prepare("SELECT m.id, m.conversation_id, COALESCE(NULLIF(m.thread_id, ''), m.id), COALESCE(c.name, 'Unknown'), m.body, COALESCE(m.sent_at, m.created_at), 0, NULL, m.edited_at IS NOT NULL, m.pinned_at IS NOT NULL, m.sender_id = ?2 FROM messages m LEFT JOIN contacts c ON c.id = m.sender_id WHERE m.deleted_at IS NULL AND m.id != ?1 AND m.reply_to_id IS NOT NULL AND COALESCE(m.thread_id, m.id) = COALESCE((SELECT COALESCE(NULLIF(parent.thread_id, ''), parent.id) FROM messages parent WHERE parent.id = ?1 AND parent.deleted_at IS NULL), '') ORDER BY COALESCE(m.sent_at, m.created_at) ASC LIMIT 100")?;
    let rows = statement.query_map(params![message_id, SELF_CONTACT_ID], |row| Ok(ChatMessage { id: row.get(0)?, conversation_id: row.get(1)?, thread_id: row.get(2)?, sender_name: row.get(3)?, body: row.get(4)?, sent_at: row.get(5)?, reply_count: row.get(6)?, last_reply_at: row.get(7)?, edited: row.get(8)?, pinned: row.get(9)?, mine: row.get(10)?, reactions: vec![] }))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
  }
  pub fn channel_reactions(&self, channel_id: &str) -> Result<Vec<MessageReaction>, AppError> {
    // Reactions live on thread roots, so the aggregate is keyed by root id; ids
    // are emitted in a stable order for deterministic output.
    let mut grouped = self.thread_reactions(channel_id)?;
    let mut roots: Vec<String> = grouped.keys().cloned().collect();
    roots.sort();
    let mut rows: Vec<MessageReaction> = vec![];
    for root in roots {
      if let Some(summaries) = grouped.remove(&root) {
        for summary in summaries { rows.push(MessageReaction { message_id: root.clone(), emoji: summary.emoji, count: summary.count, reacted_by_me: summary.reacted_by_me }); }
      }
    }
    Ok(rows)
  }
  pub fn toggle_message_reaction(&self, message_id: &str, emoji: &str) -> Result<Vec<ReactionSummary>, AppError> {
    if emoji.is_empty() || emoji.len() > 16 || emoji.contains(" ") || emoji.contains("\t") || emoji.contains("\n") || emoji.contains("\r") { return Err(AppError::Validation); }
    // Reactions belong to the thread, not to one bubble: clicking the button on a
    // reply stores the reaction on the root, so the thread keeps a single
    // aggregate that the history row and the thread panel both read.
    let root_id: String = self.connection.query_row("SELECT COALESCE(NULLIF(thread_id, ''), id) FROM messages WHERE id = ?1 AND deleted_at IS NULL", params![message_id], |row| row.get(0)).map_err(|_| AppError::Validation)?;
    let erased: i64 = self.connection.execute("DELETE FROM message_reactions WHERE message_id = ?1 AND contact_id = ?2 AND emoji = ?3", params![root_id, SELF_CONTACT_ID, emoji])? as i64;
    if erased == 0 { self.connection.execute("INSERT INTO message_reactions (id, message_id, contact_id, emoji, created_at) VALUES (?1, ?2, ?3, ?4, ?5)", params![Uuid::new_v4().to_string(), root_id, SELF_CONTACT_ID, emoji, now()])?; }
    let timestamp = now();
    self.connection.execute("UPDATE messages SET updated_at = ?2, sync_status = 'pending', sync_version = sync_version + 1 WHERE id = ?1", params![root_id, timestamp])?;
    self.enqueue("message", &root_id, "reaction")?;
    self.reaction_groups(&root_id)
  }
  pub fn all_settings(&self) -> Result<Vec<SettingsEntry>, AppError> {
    let mut statement = self.connection.prepare("SELECT key, value FROM settings ORDER BY key")?;
    let rows = statement.query_map([], |row| Ok(SettingsEntry { key: row.get(0)?, value: row.get(1)? }))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
  }
  pub fn set_setting(&self, key: &str, value: &str) -> Result<(), AppError> { if key.is_empty() || key.len() > 64 || value.len() > 512 { return Err(AppError::Validation); } self.connection.execute("INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at", params![key, value, now()])?; Ok(()) }
  /// Removes one settings row. Missing keys are a no-op so "forget" actions stay
  /// idempotent. The key is validated the same way as `set_setting`.
  pub fn delete_setting(&self, key: &str) -> Result<(), AppError> {
    if key.is_empty() || key.len() > 64 { return Err(AppError::Validation); }
    self.connection.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
    Ok(())
  }
  pub fn presence(&self) -> Result<Presence, AppError> {
    let status: String = self.connection.query_row("SELECT value FROM settings WHERE key = 'presence.status'", [], |row| row.get(0)).ok().unwrap_or_default();
    let since: String = self.connection.query_row("SELECT value FROM settings WHERE key = 'presence.since'", [], |row| row.get(0)).ok().unwrap_or_default();
    let resolved = if status.is_empty() { "offline".into() } else { status };
    Ok(Presence { status: resolved, since })
  }
  pub fn set_presence(&self, status: &str) -> Result<Presence, AppError> {
    if status != "online" && status != "away" && status != "dnd" && status != "offline" { return Err(AppError::Validation); }
    let timestamp = now();
    self.set_setting("presence.status", status)?;
    self.set_setting("presence.since", &timestamp)?;
    Ok(Presence { status: status.into(), since: timestamp })
  }
  pub fn search(&self, query: &str) -> Result<Vec<SearchResult>, AppError> {
    let trimmed = query.trim();
    if trimmed.is_empty() || trimmed.len() > 200 { return Err(AppError::Validation); }
    let expression = fts_match_expression(trimmed);
    // Punctuation alone leaves nothing to search for: an empty result set, not a
    // failed search (the user typed something the index cannot contain).
    if expression.is_empty() { return Ok(vec![]); }
    let mut results: Vec<SearchResult> = vec![];
    let mut emails = self.connection.prepare("SELECT e.id, COALESCE(NULLIF(e.subject, ''), '(no subject)'), COALESCE(NULLIF(e.sender_name, ''), e.sender_email, '') FROM fts_emails f JOIN emails e ON e.id = f.id WHERE e.deleted_at IS NULL AND fts_emails MATCH ?1 ORDER BY rank LIMIT 8")?;
    for (id, title, subtitle) in emails.query_map(params![expression], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect::<Result<Vec<_>, _>>().map_err(AppError::from)? { results.push(SearchResult { kind: "email".into(), id, title, subtitle }); }
    let mut conversations = self.connection.prepare("SELECT c.id, COALESCE(NULLIF(c.channel_slug, ''), 'channel'), substr(m.body, 1, 80) FROM fts_messages f JOIN messages m ON m.id = f.id JOIN conversations c ON c.id = m.conversation_id WHERE m.deleted_at IS NULL AND c.deleted_at IS NULL AND fts_messages MATCH ?1 ORDER BY rank LIMIT 8")?;
    for (id, title, subtitle) in conversations.query_map(params![expression], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect::<Result<Vec<_>, _>>().map_err(AppError::from)? { results.push(SearchResult { kind: "message".into(), id, title, subtitle }); }
    let mut people = self.connection.prepare("SELECT c.id, c.name, COALESCE(NULLIF(c.email, ''), NULLIF(c.department, ''), '') FROM fts_contacts f JOIN contacts c ON c.id = f.id WHERE c.deleted_at IS NULL AND fts_contacts MATCH ?1 ORDER BY rank LIMIT 8")?;
    for (id, title, subtitle) in people.query_map(params![expression], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect::<Result<Vec<_>, _>>().map_err(AppError::from)? { results.push(SearchResult { kind: "contact".into(), id, title, subtitle }); }
    let mut rooms = self.connection.prepare("SELECT c.id, COALESCE(NULLIF(c.title, ''), c.channel_slug, 'channel'), COALESCE(NULLIF(c.description, ''), '') FROM fts_conversations f JOIN conversations c ON c.id = f.id WHERE c.deleted_at IS NULL AND fts_conversations MATCH ?1 ORDER BY rank LIMIT 8")?;
    for (id, title, subtitle) in rooms.query_map(params![expression], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect::<Result<Vec<_>, _>>().map_err(AppError::from)? { results.push(SearchResult { kind: "channel".into(), id, title, subtitle }); }
    Ok(results)
  }
  pub fn set_contact_favorite(&self, id: &str, favorite: bool) -> Result<(), AppError> { self.connection.execute("UPDATE contacts SET favorite = ?2, updated_at = ?3, sync_status = 'pending', sync_version = sync_version + 1 WHERE id = ?1 AND deleted_at IS NULL", params![id, favorite as i64, now()])?; self.enqueue("contact", id, "update")?; Ok(()) }
  /// Soft-deletes the account and returns its email address when a row was
  /// removed, so the caller can purge the matching OS keyring credential.
  pub fn remove_account(&self, id: &str) -> Result<Option<String>, AppError> {
    let address: Option<String> = self.connection.query_row("SELECT email_address FROM accounts WHERE id = ?1 AND deleted_at IS NULL", params![id], |row| row.get(0)).ok();
    let removed = self.connection.execute("UPDATE accounts SET deleted_at = ?2, updated_at = ?2, sync_status = 'pending', sync_version = sync_version + 1 WHERE id = ?1 AND deleted_at IS NULL", params![id, now()])?;
    if removed > 0 { self.enqueue("account", id, "delete")?; Ok(address) } else { Ok(None) }
  }
  fn validate_draft(&self, input: &DraftInput) -> Result<(), AppError> { validate_required(&input.account_id)?; if input.subject.len() > 998 || input.body_text.len() > 2_000_000 { return Err(AppError::Validation); } for email in input.to.iter().chain(input.cc.iter()).chain(input.bcc.iter()) { validate_email(email)?; } Ok(()) }
  fn move_email_to_folder(&self, id: &str, role: &str) -> Result<(), AppError> {
    // Outbound mail moves too: queue_send files the outbound copy into Sent
    // and drafts can be trashed like any other message.
    let account_id: String = self.connection.query_row("SELECT account_id FROM emails WHERE id = ?1 AND deleted_at IS NULL", params![id], |row| row.get(0)).ok().unwrap_or_default();
    if account_id.is_empty() { return Err(AppError::Validation); }
    let mut folder_id: String = self.connection.query_row("SELECT id FROM email_folders WHERE account_id = ?1 AND role = ?2 AND deleted_at IS NULL LIMIT 1", params![account_id, role], |row| row.get(0)).ok().unwrap_or_default();
    if folder_id.is_empty() {
      let folder = Uuid::new_v4().to_string();
      self.connection.execute("INSERT INTO email_folders (id, account_id, name, role, created_at, updated_at, sync_status) VALUES (?1, ?2, ?3, ?3, ?4, ?4, 'pending')", params![folder, account_id, role, now()])?;
      folder_id = folder;
    }
    self.connection.execute("UPDATE emails SET folder_id = ?2, updated_at = ?3, sync_status = 'pending', sync_version = sync_version + 1 WHERE id = ?1 AND deleted_at IS NULL", params![id, folder_id, now()])?;
    self.enqueue("email", id, "move")?;
    Ok(())
  }
  fn list_emails(&self, filter: &str) -> Result<Vec<EmailSummary>, AppError> {
    // The row title for outbound mail is the address it was addressed to. A draft
    // may legitimately have no To recipient yet (Cc/Bcc only, or not typed at
    // all), and the scalar subquery then returns NULL — which used to fail the
    // whole listing with a column-type error and made Drafts unopenable for as
    // long as that draft existed (BUG-024). Prefer To, then Cc/Bcc, then the same
    // sender fallback inbound mail uses.
    let sender_fallback = "COALESCE(NULLIF(e.sender_name, ''), NULLIF(e.sender_email, ''), a.display_name, 'Unknown sender')";
    let addressed_to = "COALESCE((SELECT COALESCE(NULLIF(r.display_name, ''), r.email_address) FROM email_recipients r WHERE r.email_id = e.id AND r.recipient_type IN ('to', 'cc', 'bcc') ORDER BY CASE r.recipient_type WHEN 'to' THEN 0 WHEN 'cc' THEN 1 ELSE 2 END, r.created_at LIMIT 1), ";
    let sql = format!("SELECT e.id, CASE WHEN e.direction = 'outbound' THEN {addressed_to}{sender_fallback}) ELSE {sender_fallback} END, e.subject, substr(e.body_text, 1, 120), COALESCE(e.received_at, e.created_at), e.is_read, e.is_starred FROM emails e JOIN accounts a ON a.id = e.account_id WHERE e.deleted_at IS NULL AND {filter} ORDER BY COALESCE(e.received_at, e.created_at) DESC LIMIT 200");
    let mut statement = self.connection.prepare(&sql)?;
    let rows = statement.query_map([], |row| Ok(EmailSummary { id: row.get(0)?, sender_name: row.get(1)?, subject: row.get(2)?, preview: row.get(3)?, received_at: row.get(4)?, is_read: row.get::<_, i64>(5)? != 0, is_starred: row.get::<_, i64>(6)? != 0 }))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)
  }
  fn fill_recipients(&self, detail: &mut EmailDetail) -> Result<(), AppError> {
    let mut statement = self.connection.prepare("SELECT recipient_type, COALESCE(NULLIF(display_name, ''), ''), email_address FROM email_recipients WHERE email_id = ?1 ORDER BY created_at")?;
    let rows = statement.query_map(params![detail.id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))?;
    for (kind, display, address) in rows.collect::<Result<Vec<_>, _>>().map_err(AppError::from)? {
      let label = if display.is_empty() { address } else { format!("{} <{}>", display, address) };
      if kind == "to" { detail.to.push(label); } else if kind == "cc" { detail.cc.push(label); } else { detail.bcc.push(label); }
    }
    Ok(())
  }
  fn save_recipients(&self, email_id: &str, kind: &str, recipients: &[String], timestamp: &str) -> Result<(), AppError> { for address in recipients { self.connection.execute("INSERT INTO email_recipients (id, email_id, recipient_type, email_address, created_at) VALUES (?1, ?2, ?3, ?4, ?5)", params![Uuid::new_v4().to_string(), email_id, kind, address.trim(), timestamp])?; } Ok(()) }
  fn enqueue(&self, entity_type: &str, entity_id: &str, operation: &str) -> Result<(), AppError> {
    let timestamp = now();
    self.connection.execute("INSERT INTO sync_queue (id, entity_type, entity_id, operation, payload, status, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, '{}', 'pending', ?5, ?5) ON CONFLICT(entity_type, entity_id, operation) DO UPDATE SET status = 'pending', updated_at = excluded.updated_at, last_error = NULL", params![Uuid::new_v4().to_string(), entity_type, entity_id, operation, timestamp])?;
    Ok(())
  }
}
fn now() -> String { Utc::now().to_rfc3339() }
fn validate_required(value: &str) -> Result<(), AppError> { if value.trim().is_empty() || value.len() > 255 { Err(AppError::Validation) } else { Ok(()) } }
fn validate_email(value: &str) -> Result<(), AppError> { if value.len() > 320 || !value.contains('@') { Err(AppError::Validation) } else { Ok(()) } }
fn is_mail_role(role: &str) -> bool { role == "inbox" || role == "sent" || role == "drafts" || role == "archive" || role == "trash" }

/// Builds an FTS5 `MATCH` expression from whatever the user typed.
///
/// A search box is not a query language. Characters FTS5 treats as syntax
/// (`"`, `(`, `)`, `:`, `*`, `^`, `-`) and the bare `AND`/`OR`/`NOT`/`NEAR`
/// keywords must be searched as data, or a stray quote or a pasted address fails
/// the whole search with a syntax error that reads as "search is broken"
/// (BUG-010). Terms are reduced to their searchable characters and quoted, and a
/// single term keeps its prefix match so "maya" still finds "Maya Chen".
pub fn fts_match_expression(query: &str) -> String {
  let kept: Vec<String> = query
    .split_whitespace()
    .map(|term| term.chars().filter(|character| character.is_alphanumeric() || matches!(character, '_' | '@' | '.' | '\'')).collect::<String>())
    .filter(|term| !term.is_empty())
    .collect();
  match kept.len() {
    0 => String::new(),
    1 => format!("\"{}\"*", kept[0]),
    // Several words stay a phrase, so they must appear together (the previous
    // behaviour, minus the unescaped input that could break the parser).
    _ => format!("\"{}\"", kept.join(" ")),
  }
}

