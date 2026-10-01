//! Local-store integration tests.
//!
//! These tests live in a real `tests/` target rather than inline `#[cfg(test)]`
//! modules: the lib unittest harness binary would link the same comctl32
//! v6-only imports as the app binary but, unlike the app, would receive no
//! Common-Controls manifest and the loader would refuse it before main()
//! (STATUS_ENTRYPOINT_NOT_FOUND). build.rs embeds the manifest into `tests/`
//! targets via cargo:rustc-link-arg-tests, so this binary loads cleanly.

use relay_lib::database::Database;
use relay_lib::error::AppError;
use relay_lib::models::{AccountConnection, CreateAccount, DraftInput, EmailSummary, OutboundMessage, SendMessageInput};
use relay_lib::repositories::Repositories;
use relay_lib::security::{credential_ref, CredentialStore, OsKeyring};
use relay_lib::sync::{DisabledTransport, SyncEngine};
use relay_lib::verify::{base64_encode, plain_auth_response, protocol_shape_hint, refusal_hint, refusal_hint_for, xoauth2_refusal_hint};
use rusqlite::params;
use std::{fs, path::PathBuf};
use uuid::Uuid;

fn temp_database() -> (Database, PathBuf) {
  let directory = std::env::temp_dir().join(format!("relay-test-{}", Uuid::new_v4()));
  (Database::open(&directory).expect("database should initialize"), directory)
}

fn close_temp(database: Database, directory: PathBuf) {
  drop(database);
  fs::remove_dir_all(&directory).expect("test directory cleanup");
}

#[test]
fn initializes_current_schema_and_development_cache() {
  let (database, directory) = temp_database();
  assert_eq!(database.schema_version().expect("schema version"), 11);
  let emails: i64 = database.connection().query_row("SELECT COUNT(*) FROM emails", [], |row| row.get(0)).expect("email count");
  assert!(emails > 0);
  close_temp(database, directory);
}

#[test]
fn folder_queries_cover_every_mail_role() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  assert!(!repos.inbox().expect("inbox query").is_empty());
  assert!(!repos.emails_in_folder("drafts").expect("drafts query").is_empty());
  assert!(!repos.emails_in_folder("sent").expect("sent query").is_empty());
  assert!(repos.emails_in_folder("archive").expect("archive query").is_empty());
  assert!(repos.emails_in_folder("trash").expect("trash query").is_empty());
  assert!(!repos.starred().expect("starred query").is_empty());
  close_temp(database, directory);
}

#[test]
fn email_detail_exposes_recipients_for_drafts() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let draft = repos.email("dev-draft").expect("fetch draft").unwrap();
  let mut found = false;
  for recipient in draft.to { if recipient == "maya@northstar.test" { found = true; } }
  assert!(found);
  assert_eq!(draft.delivery_state, "draft");
  close_temp(database, directory);
}

#[test]
fn star_toggle_persists_and_reverts() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  repos.set_email_star("dev-email-0", true).expect("star on");
  let mut found = false;
  for item in repos.starred().expect("starred list") { if item.id == "dev-email-0" { found = true; } }
  assert!(found);
  repos.set_email_star("dev-email-0", false).expect("star off");
  let mut gone = true;
  for item in repos.starred().expect("starred list") { if item.id == "dev-email-0" { gone = false; } }
  assert!(gone);
  close_temp(database, directory);
}

#[test]
fn threading_groups_related_mail_and_isolates_others() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let thread = repos.email_thread("dev-email-0").expect("thread query");
  assert_eq!(thread.len(), 2);
  let mut has_sent = false;
  for item in thread { if item.id == "dev-sent-1" { has_sent = true; } }
  assert!(has_sent);
  assert!(repos.email_thread("dev-email-2").expect("unthreaded email").is_empty());
  assert!(repos.email_thread("does-not-exist").expect("unknown email").is_empty());
  close_temp(database, directory);
}

#[test]
fn fts_search_indexes_mail_and_messages() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let email_hits = repos.search("planning").expect("search email");
  let mut found_email = false;
  for hit in email_hits { if hit.kind == "email" && hit.id == "dev-email-0" { found_email = true; } }
  assert!(found_email, "expected the Q3 email in search results");
  let message_hits = repos.search("maintenance").expect("search messages");
  let mut found_message = false;
  for hit in message_hits { if hit.kind == "message" { found_message = true; } }
  assert!(found_message, "expected the maintenance message in search results");
  assert!(repos.search("   ").is_err());
  close_temp(database, directory);
}

#[test]
fn message_reactions_toggle_cleanly() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let after_add = repos.toggle_message_reaction("dev-message-0", "👍").expect("add reaction");
  assert_eq!(after_add.len(), 1);
  assert!(after_add[0].reacted_by_me);
  let after_remove = repos.toggle_message_reaction("dev-message-0", "👍").expect("remove reaction");
  assert!(after_remove.is_empty());
  assert!(repos.toggle_message_reaction("dev-message-0", "bad emoji value with spaces").is_err());
  close_temp(database, directory);
}

#[test]
fn settings_and_presence_round_trip_with_validation() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  repos.set_setting("theme", "dark").expect("set theme");
  let mut saw_theme = false;
  for entry in repos.all_settings().expect("list settings") { if entry.key == "theme" && entry.value == "dark" { saw_theme = true; } }
  assert!(saw_theme);
  // "Forget" actions remove the row, and removing a row twice stays a no-op.
  repos.delete_setting("theme").expect("delete theme");
  assert!(!repos.all_settings().expect("list settings").iter().any(|entry| entry.key == "theme"));
  repos.delete_setting("theme").expect("deleting a missing setting is idempotent");
  assert!(repos.delete_setting("").is_err());
  let presence = repos.set_presence("dnd").expect("set presence");
  assert_eq!(presence.status, "dnd");
  assert_eq!(repos.presence().expect("read presence").status, "dnd");
  assert!(repos.set_presence("bogus").is_err());
  close_temp(database, directory);
}

#[test]
fn retry_marks_failed_rows_pending_again() {
  let (database, directory) = temp_database();
  database.connection().execute("INSERT INTO sync_queue (id, entity_type, entity_id, operation, payload, status, created_at, updated_at) VALUES ('test-item', 'test', 'entity-1', 'upsert', '{}', 'failed', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')", params![]).expect("seed failed row");
  let engine = SyncEngine::new(database.connection(), DisabledTransport);
  assert_eq!(engine.overview().expect("overview").failed, 1);
  let retried = engine.retry_failed().expect("retry failed rows");
  assert!(retried >= 1);
  assert_eq!(engine.overview().expect("overview after retry").failed, 0);
  assert_eq!(engine.overview().expect("overview after retry").pending, 1);
  close_temp(database, directory);
}

#[test]
fn disabled_transport_reports_offline() {
  let (database, directory) = temp_database();
  let state = SyncEngine::new(database.connection(), DisabledTransport).overview().expect("overview");
  assert_eq!(state.state, "offline");
  close_temp(database, directory);
}

fn sample_account() -> CreateAccount {
  CreateAccount {
    display_name: "Test User".into(),
    email_address: format!("relay-{}@example.test", Uuid::new_v4()),
    imap_host: "imap.example.test".into(),
    imap_port: 993,
    smtp_host: "smtp.example.test".into(),
    smtp_port: 465,
    encryption: "ssl".into(),
    auth_kind: "password".into(),
  }
}

#[test]
fn verified_sign_in_persists_state_and_keeps_secret_out_of_the_database() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let input = sample_account();
  let password = "S3cret-Password-Do-Not-Store!";
  let account = repos.add_account_verified(&input, &credential_ref(&input.email_address)).expect("verified account");
  assert_eq!(account.connection_state, "connected");
  assert!(account.last_verified_at.is_some());
  assert!(account.connection_error.is_none());
  // No column anywhere on the stored row may contain the password.
  {
    let mut statement = database.connection().prepare("SELECT * FROM accounts WHERE id = ?1").expect("select account row");
    let column_count = statement.column_count();
    let mut rows = statement.query(params![account.id]).expect("account row");
    let row = rows.next().expect("row iteration").expect("row present");
    for index in 0..column_count {
      if let rusqlite::types::ValueRef::Text(text) = row.get_ref(index).expect("column value") {
        assert_ne!(String::from_utf8_lossy(text), password, "password leaked into the accounts table");
      }
    }
  }
  close_temp(database, directory);
}

#[test]
fn connection_state_updates_are_validated() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let input = sample_account();
  let account = repos.add_account_verified(&input, &credential_ref(&input.email_address)).expect("verified account");
  let failed = repos.set_account_connection_state(&account.id, "error", Some("Unable to reach the mail server"), false).expect("record failure");
  assert_eq!(failed.connection_state, "error");
  assert_eq!(failed.connection_error.as_deref(), Some("Unable to reach the mail server"));
  assert!(failed.last_verified_at.is_some(), "a failed attempt keeps the previous verification timestamp");
  let recovered = repos.set_account_connection_state(&account.id, "connected", None, true).expect("record recovery");
  assert_eq!(recovered.connection_state, "connected");
  assert!(recovered.connection_error.is_none());
  assert!(recovered.last_verified_at.is_some());
  assert!(repos.set_account_connection_state(&account.id, "bogus", None, false).is_err());
  close_temp(database, directory);
}

#[test]
fn removed_account_reports_address_for_credential_purge() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let input = sample_account();
  let account = repos.add_account_verified(&input, &credential_ref(&input.email_address)).expect("verified account");
  assert_eq!(repos.remove_account(&account.id).expect("removal").as_deref(), Some(input.email_address.as_str()));
  assert!(repos.remove_account("does-not-exist").expect("removal of unknown account").is_none());
  close_temp(database, directory);
}

/// Exercises the real OS credential manager (Windows Credential Manager on
/// Windows) with a uniquely named entry that is always cleaned up.
#[test]
fn os_keyring_round_trip() {
  let keyring = OsKeyring;
  let user = format!("relay-test-{}", Uuid::new_v4());
  assert!(keyring.load(&user).expect("empty load").is_none());
  keyring.save(&user, "first secret").expect("save");
  assert_eq!(keyring.load(&user).expect("load").as_deref(), Some("first secret"));
  keyring.save(&user, "second secret").expect("rotate");
  assert_eq!(keyring.load(&user).expect("reload").as_deref(), Some("second secret"));
  keyring.delete(&user).expect("delete");
  assert!(keyring.load(&user).expect("load after delete").is_none());
  keyring.delete(&user).expect("deleting a missing entry stays successful");
}

#[test]
fn message_actions_edit_delete_and_pin() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let edited = repos.edit_message("dev-message-0", "Updated welcome text.").expect("edit message");
  assert!(edited.edited);
  assert_eq!(edited.body, "Updated welcome text.");
  repos.set_message_pin("dev-message-0", true).expect("pin on");
  let pinned = repos.channel_messages("dev-general").expect("messages").into_iter().find(|message| message.id == "dev-message-0").expect("message present");
  assert!(pinned.pinned);
  repos.set_message_pin("dev-message-0", false).expect("pin off");
  let unpinned = repos.channel_messages("dev-general").expect("messages").into_iter().find(|message| message.id == "dev-message-0").expect("message present");
  assert!(!unpinned.pinned);
  repos.delete_message("dev-reply-0").expect("delete reply");
  assert!(repos.message_thread("dev-message-3").expect("thread").into_iter().all(|message| message.id != "dev-reply-0"));
  assert!(repos.edit_message("dev-message-0", "   ").is_err());
  assert!(repos.edit_message("missing-id", "hello").is_err());
  assert!(repos.delete_message("missing-id").is_err());
  close_temp(database, directory);
}

#[test]
fn direct_messages_are_idempotent_and_listed() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let first = repos.open_direct_message("dev-priya").expect("open dm");
  assert_eq!(first.kind, "dm");
  assert_eq!(first.title, "Priya Nair");
  let second = repos.open_direct_message("dev-priya").expect("reopen dm");
  assert_eq!(second.id, first.id);
  assert!(repos.open_direct_message("missing-contact").is_err());
  repos.send_message(SendMessageInput { conversation_id: first.id.clone(), body: "Hello Priya".into(), reply_to_id: None }).expect("dm message");
  let messages = repos.channel_messages(&first.id).expect("dm messages");
  assert_eq!(messages.len(), 1);
  assert!(messages[0].mine);
  let conversations = repos.conversations().expect("conversation list");
  assert!(conversations.iter().any(|conversation| conversation.id == first.id && conversation.kind == "dm"));
  assert!(conversations.iter().any(|conversation| conversation.kind == "channel" && conversation.id == "dev-general"));
  assert!(conversations.iter().any(|conversation| conversation.id == "dev-dm-maya" && conversation.title == "Maya Chen"));
  close_temp(database, directory);
}

/// Queueing a send must file the outbound copy into the account's Sent folder:
/// the Sent view must show it and Drafts must no longer list it, while the
/// durable `send_smtp` queue item keeps actual delivery pending.
#[test]
fn queued_send_moves_mail_into_the_sent_folder() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let draft = repos.save_draft(DraftInput { account_id: "dev-account".into(), to: vec!["maya@northstar.test".into()], cc: vec![], bcc: vec![], subject: "Queue me".into(), body_text: "body".into() }, None).expect("draft");
  repos.queue_send(&draft.id).expect("queue send");
  let sent = repos.emails_in_folder("sent").expect("sent view");
  assert!(sent.iter().any(|item| item.id == draft.id), "queued mail must appear in Sent");
  let drafts = repos.emails_in_folder("drafts").expect("drafts view");
  assert!(!drafts.iter().any(|item| item.id == draft.id), "queued mail must leave Drafts");
  let detail = repos.email(&draft.id).expect("detail").unwrap();
  assert_eq!(detail.delivery_state, "queued");
  close_temp(database, directory);
}

/// Queueing a send records exactly one durable `send_smtp` item and leaves it
/// pending until a delivery run consumes it: re-queueing the same message must
/// not create a second piece of work. This is the contract the composer relies
/// on when it reports a send as queued rather than delivered (BUG-023).
#[test]
fn queued_send_records_one_pending_delivery_item() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let draft = repos.save_draft(DraftInput { account_id: "dev-account".into(), to: vec!["maya@northstar.test".into()], cc: vec![], bcc: vec![], subject: "Queue me twice".into(), body_text: "body".into() }, None).expect("draft");
  repos.queue_send(&draft.id).expect("first queue");
  repos.queue_send(&draft.id).expect("re-queue is accepted");
  let deliveries: i64 = database.connection().query_row("SELECT COUNT(*) FROM sync_queue WHERE operation = 'send_smtp' AND entity_id = ?1", params![&draft.id], |row| row.get(0)).expect("count queued deliveries");
  assert_eq!(deliveries, 1, "one send must be exactly one piece of queued work");
  // Saving the draft is the other piece; it is not a delivery item.
  let saves: i64 = database.connection().query_row("SELECT COUNT(*) FROM sync_queue WHERE operation = 'save_draft' AND entity_id = ?1", params![&draft.id], |row| row.get(0)).expect("count saved drafts");
  assert_eq!(saves, 1);
  let status: String = database.connection().query_row("SELECT status FROM sync_queue WHERE operation = 'send_smtp' AND entity_id = ?1", params![&draft.id], |row| row.get(0)).expect("delivery status");
  assert_eq!(status, "pending", "a freshly queued send waits for the delivery run");
  let pending = repos.pending_sync_items().expect("pending work");
  assert!(pending.iter().any(|item| item.operation == "send_smtp" && item.entity_id == draft.id), "the delivery item must be visible to whatever consumes the queue");
  close_temp(database, directory);
}

/// A send needs a To recipient, and that failure is a validation error rather
/// than a storage error. It must leave the message in Drafts with no queued
/// delivery behind it, because the composer reports the two cases differently
/// (BUG-023).
#[test]
fn queue_send_requires_a_to_recipient() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let draft = repos.save_draft(DraftInput { account_id: "dev-account".into(), to: vec![], cc: vec!["maya@northstar.test".into()], bcc: vec![], subject: "Cc only".into(), body_text: "body".into() }, None).expect("draft");
  assert!(matches!(repos.queue_send(&draft.id), Err(AppError::Validation)));
  assert!(repos.emails_in_folder("sent").expect("sent view").iter().all(|item| item.id != draft.id), "a refused send must not be filed into Sent");
  assert!(repos.emails_in_folder("drafts").expect("drafts view").iter().any(|item| item.id == draft.id), "a refused send must stay in Drafts");
  let deliveries: i64 = database.connection().query_row("SELECT COUNT(*) FROM sync_queue WHERE operation = 'send_smtp'", [], |row| row.get(0)).expect("count queued deliveries");
  assert_eq!(deliveries, 0, "a refused send must queue nothing");
  // An unknown id answers the same way: validation, never a storage failure.
  assert!(matches!(repos.queue_send("does-not-exist"), Err(AppError::Validation)));
  close_temp(database, directory);
}

/// The delivery run receives everything it needs from the store in one snapshot:
/// the account (with its SMTP endpoint), the sender identity, the body and every
/// recipient list. The snapshot also claims the message, and a delivered message
/// is never offered again — together those are what keep the queue from
/// transmitting the same mail twice.
#[test]
fn queued_send_snapshots_everything_delivery_needs() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let draft = repos.save_draft(DraftInput { account_id: "dev-account".into(), to: vec!["maya@northstar.test".into()], cc: vec!["sam@northstar.test".into()], bcc: vec!["ops@northstar.test".into()], subject: "Snapshot me".into(), body_text: "hello".into() }, None).expect("draft");
  repos.queue_send(&draft.id).expect("queue send");
  let sends = repos.queued_sends(None).expect("queued sends");
  assert_eq!(sends.len(), 1, "only the freshly queued message needs delivering");
  let send = &sends[0];
  let account = send.connection.as_ref().expect("the seeded account is signed in");
  assert_eq!(account.smtp_host, "localhost", "the snapshot carries the account's SMTP endpoint");
  assert_eq!(account.email_address, "alex@northstar.test", "the sender identity comes from the account");
  assert_eq!(send.message.id, draft.id);
  assert_eq!(send.message.to, vec!["maya@northstar.test".to_string()]);
  assert_eq!(send.message.cc, vec!["sam@northstar.test".to_string()]);
  assert_eq!(send.message.bcc, vec!["ops@northstar.test".to_string()]);
  assert_eq!(send.message.subject, "Snapshot me");
  assert_eq!(send.message.body_text, "hello");
  // The snapshot claims the message while a run is in flight, so an overlapping
  // run (and this second call) must not receive it again.
  assert!(repos.queued_sends(None).expect("second snapshot").is_empty(), "a claimed message is not handed to a second run");
  // Filtering by id is how the reading pane's Try again targets one message; a
  // filter that matches nothing claims nothing.
  assert!(repos.queued_sends(Some(&["some-other-id".to_string()])).expect("targeted snapshot").is_empty());
  let mut keys: Vec<String> = Vec::new();
  for send in repos.queued_sends(None).expect("third snapshot") { keys.push(send.message.id); }
  assert!(!keys.contains(&draft.id), "the claim must survive an unrelated snapshot");
  // A refusal returns the message to the queue (it is retryable)...
  repos.mark_delivery_failed(&draft.id, "The mail server is temporarily not accepting mail. Try again shortly.").expect("record failure");
  assert!(repos.queued_sends(Some(&[draft.id.clone()])).expect("targeted retry").iter().any(|send| send.message.id == draft.id), "a failed send is offered to the next run");
  // ...while a delivery retires it for good: the completed queue item gates it out.
  repos.mark_delivery_sent(&draft.id).expect("mark sent");
  assert!(repos.queued_sends(None).expect("snapshot after delivery").iter().all(|send| send.message.id != draft.id), "a delivered message must never be queued again");
  close_temp(database, directory);
}

/// A delivery claim cannot outlive the process that made it. Reopening the store
/// must hand an abandoned `syncing` item back to the queue, or a crash mid-send
/// would leave the message looking in flight while nothing sent it.
#[test]
fn reopening_the_store_releases_abandoned_delivery_claims() {
  let (database, directory) = temp_database();
  let draft = Repositories::new(database.connection()).save_draft(DraftInput { account_id: "dev-account".into(), to: vec!["maya@northstar.test".into()], cc: vec![], bcc: vec![], subject: "Abandoned claim".into(), body_text: "body".into() }, None).expect("draft");
  let repos = Repositories::new(database.connection());
  repos.queue_send(&draft.id).expect("queue send");
  // Taking the delivery snapshot claims the message, as a run in flight would.
  assert_eq!(repos.queued_sends(None).expect("snapshot").len(), 1);
  let claimed: String = database.connection().query_row("SELECT status FROM sync_queue WHERE operation = 'send_smtp' AND entity_id = ?1", params![&draft.id], |row| row.get(0)).expect("queue row");
  assert_eq!(claimed, "syncing");
  // The process "dies" (the store closes) and the app starts again.
  drop(database);
  let reopened = Database::open(&directory).expect("reopen the store");
  let after_restart = Repositories::new(reopened.connection());
  let status: String = reopened.connection().query_row("SELECT status FROM sync_queue WHERE operation = 'send_smtp' AND entity_id = ?1", params![&draft.id], |row| row.get(0)).expect("queue row after restart");
  assert_eq!(status, "pending", "an abandoned claim is released at startup");
  assert!(after_restart.queued_sends(None).expect("snapshot after restart").iter().any(|send| send.message.id == draft.id), "the message is offered to the next run");
  close_temp(reopened, directory);
}

/// A delivered message becomes `sent` and its queue item completes. That
/// completion is the idempotency guard, and clearing the error is what removes
/// the reading pane's failure note.
#[test]
fn delivered_send_is_marked_sent_and_completes_the_queue() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let draft = repos.save_draft(DraftInput { account_id: "dev-account".into(), to: vec!["maya@northstar.test".into()], cc: vec![], bcc: vec![], subject: "Deliver me".into(), body_text: "body".into() }, None).expect("draft");
  repos.queue_send(&draft.id).expect("queue send");
  repos.mark_delivery_failed(&draft.id, "The mail server is temporarily not accepting mail. Try again shortly.").expect("first attempt fails");
  repos.mark_delivery_sent(&draft.id).expect("retry succeeds");
  let detail = repos.email(&draft.id).expect("detail").expect("row");
  assert_eq!(detail.delivery_state, "sent");
  assert_eq!(detail.delivery_error, None, "a delivered message carries no failure note");
  let (status, completed): (String, Option<String>) = database.connection().query_row("SELECT status, completed_at FROM sync_queue WHERE operation = 'send_smtp' AND entity_id = ?1", params![&draft.id], |row| Ok((row.get(0)?, row.get(1)?))).expect("queue row");
  assert_eq!(status, "completed");
  assert!(completed.is_some(), "completion is timestamped");
  assert!(repos.emails_in_folder("sent").expect("sent view").iter().any(|item| item.id == draft.id), "delivered mail stays in Sent");
  close_temp(database, directory);
}

/// A failed attempt stays retryable and records the classifier's fixed wording
/// where the reading pane can show it, while counting the attempt. A later run
/// can still deliver the message, which clears the note.
#[test]
fn failed_delivery_surfaces_a_classified_error_and_retries() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let draft = repos.save_draft(DraftInput { account_id: "dev-account".into(), to: vec!["maya@northstar.test".into()], cc: vec![], bcc: vec![], subject: "Fail me once".into(), body_text: "body".into() }, None).expect("draft");
  repos.queue_send(&draft.id).expect("queue send");
  let reason = "The mail server refused the stored sign-in for sending. Check the account password, or use an app password.";
  repos.mark_delivery_failed(&draft.id, reason).expect("record failure");
  let detail = repos.email(&draft.id).expect("detail").expect("row");
  assert_eq!(detail.delivery_state, "failed");
  assert_eq!(detail.delivery_error.as_deref(), Some(reason), "the reason reaches the reading pane");
  let (status, attempts): (String, i64) = database.connection().query_row("SELECT status, attempt_count FROM sync_queue WHERE operation = 'send_smtp' AND entity_id = ?1", params![&draft.id], |row| Ok((row.get(0)?, row.get(1)?))).expect("queue row");
  assert_eq!(status, "failed");
  assert_eq!(attempts, 1);
  // A failed message is still work for the next run — that is what makes Try
  // again meaningful.
  assert!(repos.queued_sends(None).expect("queued sends").iter().any(|send| send.message.id == draft.id));
  // A second attempt is counted, and success clears the note.
  repos.mark_delivery_failed(&draft.id, reason).expect("second failure");
  let attempts_after: i64 = database.connection().query_row("SELECT attempt_count FROM sync_queue WHERE operation = 'send_smtp' AND entity_id = ?1", params![&draft.id], |row| row.get(0)).expect("attempt count");
  assert_eq!(attempts_after, 2, "every attempt is counted");
  repos.mark_delivery_sent(&draft.id).expect("retry succeeds");
  assert_eq!(repos.email(&draft.id).expect("detail").expect("row").delivery_error, None);
  close_temp(database, directory);
}

/// A draft may have no To recipient (Cc/Bcc only, or nothing typed yet). The
/// folder listing must still render: the outbound row title falls back through
/// Cc/Bcc and then the sender identity instead of returning NULL and failing the
/// whole query, which made Drafts unopenable (BUG-024).
#[test]
fn draft_without_a_to_recipient_still_lists() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let cc_only = repos.save_draft(DraftInput { account_id: "dev-account".into(), to: vec![], cc: vec!["maya@northstar.test".into()], bcc: vec![], subject: "Cc only".into(), body_text: "body".into() }, None).expect("draft");
  let drafts = repos.emails_in_folder("drafts").expect("a cc-only draft must not fail the folder listing");
  let row = drafts.iter().find(|item| item.id == cc_only.id).expect("the cc-only draft is listed");
  assert_eq!(row.sender_name, "maya@northstar.test", "the row is titled with the address it was addressed to");
  let bare = repos.save_draft(DraftInput { account_id: "dev-account".into(), to: vec![], cc: vec![], bcc: vec![], subject: "No recipient yet".into(), body_text: "body".into() }, None).expect("draft");
  let listed = repos.emails_in_folder("drafts").expect("a recipient-less draft must not fail the folder listing");
  let bare_row = listed.iter().find(|item| item.id == bare.id).expect("the recipient-less draft is listed");
  assert!(!bare_row.sender_name.is_empty(), "the row still carries a title");
  assert!(repos.inbox().expect("the Inbox listing still loads").iter().all(|item| item.id != bare.id), "a draft belongs to Drafts, not the Inbox");
  close_temp(database, directory);
}

/// Outbound mail (drafts, queued sends) is movable: trashing a draft must work
/// like trashing any other message.
#[test]
fn trash_accepts_outbound_mail() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  repos.trash_email("dev-draft").expect("trash draft");
  let trash = repos.emails_in_folder("trash").expect("trash view");
  assert!(trash.iter().any(|item| item.id == "dev-draft"));
  close_temp(database, directory);
}

/// Messenger identity is anchored to the self contact row. Release builds never
/// run the debug seed, so the same startup code path has to restore the row;
/// reactions and channel membership (foreign keys into `contacts`) must work
/// again afterwards, and sent messages must carry the real display name.
#[test]
fn local_user_identity_survives_without_the_development_seed() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let name = repos.ensure_self_identity().expect("identity");
  assert_eq!(name, "Alex Morgan");
  database.connection().execute("DELETE FROM contacts WHERE id = 'dev-self'", params![]).expect("delete self row");
  let restored = repos.ensure_self_identity().expect("restored identity");
  assert_eq!(restored, "Alex Morgan");
  let message = repos.send_message(SendMessageInput { conversation_id: "dev-general".into(), body: "identity check".into(), reply_to_id: None }).expect("send after restore");
  assert!(message.mine);
  assert_eq!(message.sender_name, "Alex Morgan");
  repos.toggle_message_reaction(&message.id, "👍").expect("reaction after restore");
  close_temp(database, directory);
}

#[test]
fn threaded_replies_stay_in_one_thread_and_react_on_the_root() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  // Seeded thread: dev-message-3 with four replies plus one nested reply that
  // answers dev-reply-0 instead of the root.
  let history = repos.channel_messages("dev-it-support").expect("history");
  assert_eq!(history.len(), 1, "a channel lists one row per thread");
  let root = &history[0];
  assert_eq!(root.id, "dev-message-3");
  assert_eq!(root.thread_id, "dev-message-3", "a root is its own thread");
  assert_eq!(root.reply_count, 5, "nested replies count towards the root");
  assert!(root.last_reply_at.is_some(), "the thread reports its latest reply");
  assert!(root.pinned, "pinned threads are listed first");
  let replies = repos.message_thread("dev-message-3").expect("thread");
  assert_eq!(replies.len(), 5);
  assert!(replies.iter().any(|reply| reply.id == "dev-reply-nested" && reply.thread_id == "dev-message-3"));
  // A reply to a reply joins the existing thread instead of starting a new one.
  let nested = repos.send_message(SendMessageInput { conversation_id: "dev-it-support".into(), body: "Reply to a reply".into(), reply_to_id: Some("dev-reply-nested".into()) }).expect("nested reply");
  assert_eq!(nested.thread_id, "dev-message-3");
  assert_eq!(nested.reply_count, 0, "a reply is never a thread root");
  assert_eq!(repos.message_thread("dev-message-3").expect("thread").len(), 6);
  assert_eq!(repos.channel_messages("dev-it-support").expect("history").len(), 1, "the history gained no extra row");
  // Reactions clicked on a reply are stored on the thread root.
  let summary = repos.toggle_message_reaction("dev-reply-nested", "👍").expect("react to a reply");
  assert_eq!(summary.len(), 1);
  assert!(summary[0].reacted_by_me);
  let aggregates = repos.channel_reactions("dev-it-support").expect("aggregates");
  assert_eq!(aggregates.len(), 1);
  assert_eq!(aggregates[0].message_id, "dev-message-3", "the aggregate hangs off the root");
  assert_eq!(aggregates[0].emoji, "👍");
  let refreshed = repos.channel_messages("dev-it-support").expect("history").remove(0);
  assert_eq!(refreshed.reactions.len(), 1, "the history row carries the thread reactions");
  let root_of_reply = repos.send_message(SendMessageInput { conversation_id: "dev-it-support".into(), body: "Answer the root".into(), reply_to_id: Some("dev-message-3".into()) }).expect("reply to root");
  assert_eq!(root_of_reply.thread_id, "dev-message-3");
  // Reply targets are validated: unknown rows and foreign conversations fail.
  assert!(repos.send_message(SendMessageInput { conversation_id: "dev-it-support".into(), body: "orphan".into(), reply_to_id: Some("missing-id".into()) }).is_err());
  assert!(repos.send_message(SendMessageInput { conversation_id: "dev-general".into(), body: "cross conversation".into(), reply_to_id: Some("dev-message-3".into()) }).is_err());
  close_temp(database, directory);
}

/// Deleting a thread root has to take its replies with it: the channel history
/// only lists roots, so replies left behind would be unreachable. Deleting one
/// reply keeps the rest of the thread alive.
#[test]
fn deleting_a_thread_root_removes_its_replies() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  repos.delete_message("dev-reply-nested").expect("delete nested reply");
  assert_eq!(repos.message_thread("dev-message-3").expect("thread").len(), 4, "deleting a reply keeps the thread");
  repos.delete_message("dev-message-3").expect("delete thread root");
  assert!(repos.message_thread("dev-message-3").expect("thread").is_empty(), "deleting the root clears the thread");
  assert!(repos.channel_messages("dev-it-support").expect("history").is_empty());
  assert!(repos.delete_message("dev-message-3").is_err(), "the root is already deleted");
  close_temp(database, directory);
}

/// Legacy databases threaded messages through `reply_to_id` alone. The migration
/// has to walk the whole reply chain, so a reply to a reply lands in the original
/// thread instead of starting a second one. This exercises the migration file
/// against a pre-0011 schema, which a freshly created database never does.
#[test]
fn thread_migration_backfills_legacy_reply_chains() {
  let directory = std::env::temp_dir().join(format!("relay-migration-{}", Uuid::new_v4()));
  fs::create_dir_all(&directory).expect("scratch directory");
  let connection = rusqlite::Connection::open(directory.join("legacy.db")).expect("scratch database");
  connection.execute_batch("CREATE TABLE messages (id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, body TEXT NOT NULL, reply_to_id TEXT REFERENCES messages(id), sent_at TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT);").expect("legacy schema");
  connection.execute_batch("INSERT INTO messages (id, conversation_id, body, reply_to_id, created_at, updated_at) VALUES ('root', 'c1', 'root', NULL, '2026-01-01', '2026-01-01'), ('reply', 'c1', 'reply', 'root', '2026-01-02', '2026-01-02'), ('deep', 'c1', 'deep', 'reply', '2026-01-03', '2026-01-03'), ('other', 'c1', 'other', NULL, '2026-01-04', '2026-01-04');").expect("legacy rows");
  connection.execute_batch(include_str!("../migrations/0011_message_threads.sql")).expect("apply thread migration");
  let mut statement = connection.prepare("SELECT id, thread_id FROM messages ORDER BY id").expect("select threads");
  let threads = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))).expect("thread rows").collect::<Result<Vec<_>, _>>().expect("collect threads");
  assert_eq!(threads, vec![("deep".into(), "root".into()), ("other".into(), "other".into()), ("reply".into(), "root".into()), ("root".into(), "root".into())]);
  let in_reply_to: Option<String> = connection.query_row("SELECT in_reply_to FROM messages WHERE id = 'deep'", [], |row| row.get(0)).expect("in_reply_to");
  assert_eq!(in_reply_to.as_deref(), Some("reply"), "the exact parent survives for UI quoting");
  drop(statement);
  drop(connection);
  fs::remove_dir_all(&directory).expect("scratch cleanup");
}

/// Signing in with an address that already exists must be idempotent: no
/// duplicate row, and the connection state is refreshed on the existing row.
#[test]
fn re_sign_in_updates_the_existing_account() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let input = CreateAccount { display_name: "Repeat User".into(), email_address: "reauth@northstar.test".into(), imap_host: "imap.northstar.test".into(), imap_port: 993, smtp_host: "smtp.northstar.test".into(), smtp_port: 465, encryption: "ssl".into(), auth_kind: "password".into() };
  let first = repos.add_account_verified(&input, "keyring:test:reauth").expect("first sign-in");
  let again = repos.account_by_email("reauth@northstar.test").expect("lookup").expect("existing account");
  assert_eq!(again.id, first.id);
  let refreshed = repos.set_account_connection_state(&again.id, "connected", None, true).expect("re-auth refresh");
  assert_eq!(refreshed.connection_state, "connected");
  assert!(refreshed.last_verified_at.is_some());
  assert!(repos.account_by_email("missing@nowhere.test").expect("lookup").is_none());
  let count: i64 = database.connection().query_row("SELECT COUNT(*) FROM accounts WHERE email_address = 'reauth@northstar.test'", [], |row| row.get(0)).expect("count");
  assert_eq!(count, 1, "re-sign-in must not create a duplicate row");
  close_temp(database, directory);
}

/// Soft-deleted messages must disappear from global search even though the FTS
/// index retains the row (external-content tables keep indexed text).
#[test]
fn search_ignores_soft_deleted_messages() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  repos.delete_message("dev-message-0").expect("delete message");
  let results = repos.search("workspace").expect("search");
  assert!(results.iter().all(|result| !(result.kind == "message" && result.subtitle.contains("local-first workspace"))));
  close_temp(database, directory);
}

/// The SASL PLAIN response encoding must match the RFC 4648 test vectors, and
/// login refusals must be classified into fixed, non-sensitive hints that carry
/// provider guidance without ever echoing the raw server text.
#[test]
fn sasl_plain_encoding_and_refusal_classification() {
  // RFC 4648 test vectors for the inlined base64 encoder.
  assert_eq!(base64_encode(b""), "");
  assert_eq!(base64_encode(b"f"), "Zg==");
  assert_eq!(base64_encode(b"fo"), "Zm8=");
  assert_eq!(base64_encode(b"foo"), "Zm9v");
  assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
  assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
  assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
  // PLAIN payload: \0 address \0 password.
  assert_eq!(plain_auth_response("user@example.com", "secret"), base64_encode(b"\0user@example.com\0secret"));
  // Refusal classification (responses are lowercased before matching).
  assert_eq!(refusal_hint("no [authenticationfailed] invalid credentials (failure)"), "the credentials were refused");
  assert_eq!(refusal_hint("no [unavailable] temporary system error"), "the server is temporarily unable to authenticate");
  assert_eq!(refusal_hint("no [ratelimit] too many invalid logins"), "the server is rate-limiting sign-ins for this account");
  assert_eq!(refusal_hint("no logindisabled"), "the server disabled inline LOGIN and refused AUTHENTICATE PLAIN as well");
  assert_eq!(refusal_hint("no something unexpected"), "the server refused the sign-in without a specific reason");
  // The user-facing message must carry actionable provider guidance...
  let refused = relay_lib::verify::VerifyFailure::Auth(refusal_hint("no [authenticationfailed] x"));
  assert!(refused.user_message().contains("App Password"));
  assert!(refused.user_message().contains("myaccount.google.com/apppasswords"));
  // ...and must never echo the raw server text (which can contain user input).
  assert!(!refused.user_message().contains("[authenticationfailed]"));
}

/// Providers answer refusals in their own words, and an endpoint that never
/// completed an IMAP login answers with a different shape entirely. The two
/// cases need different remedies, so they must not share one message (that is
/// what made a Gmail policy refusal indistinguishable from a wrong host).
#[test]
fn refusal_and_protocol_classification_cover_real_provider_answers() {
  // Google reports a missing App Password, a blocked password sign-in and a
  // disabled IMAP service as `NO [ALERT] <text>`; imap-proto keeps that text
  // (unknown codes such as [AUTHENTICATIONFAILED] stay in it verbatim).
  let app_password = refusal_hint("no response: [alert] application-specific password required: https://support.google.com/accounts/answer/185833 (failure)");
  assert!(app_password.contains("App Password"), "got: {app_password}");
  let web_login = refusal_hint("no response: [alert] please log in via your web browser: https://support.google.com/mail/answer/78754 (failure)");
  assert!(web_login.contains("sign in with Google"), "got: {web_login}");
  let imap_disabled = refusal_hint("no response: [alert] imap access is disabled for your domain");
  assert!(imap_disabled.contains("IMAP access is disabled"), "got: {imap_disabled}");
  // Other providers' wordings.
  assert_eq!(refusal_hint("no response: authenticate failed."), "the credentials were refused");
  assert_eq!(refusal_hint("bad response: invalid characters in atom"), "the server rejected the password's characters (some servers refuse passwords containing spaces or special characters)");
  assert_eq!(refusal_hint("no response: [authorizationfailed] not authorized"), "this account is not authorized to use IMAP on this server");
  assert_eq!(refusal_hint("no response: [privacyrequired]"), "the server requires a privacy-protected connection (enable SSL/TLS for this account)");
  // A code-only refusal carries no reason that can be classified.
  assert_eq!(refusal_hint("no response: "), "the server refused the sign-in without a specific reason");
  // Answers whose shape is not an IMAP login are a configuration problem, not a
  // verdict on the password.
  assert!(protocol_shape_hint("missing status response").is_some());
  assert!(protocol_shape_hint("unexpected response: done { ... }").is_some());
  assert!(protocol_shape_hint("mismatched tag: ...").is_some());
  assert!(protocol_shape_hint("starttls is not available on the server").is_some());
  assert!(protocol_shape_hint("no [authenticationfailed] invalid credentials (failure)").is_none());
  let protocol = relay_lib::verify::VerifyFailure::Protocol(protocol_shape_hint("missing status response").expect("protocol hint"));
  assert!(protocol.user_message().contains("not with an IMAP login"));
  assert!(!protocol.user_message().contains("App Password"), "a configuration failure must not be blamed on the password");
  assert!(protocol.detail().contains("did not answer an IMAP login"));
}

/// Gmail's own wordings must be diagnosed on both sign-in paths: a password
/// refusal points at App Passwords / Google sign-in, while the same Google code
/// on the XOAUTH2 path means the authorization itself has to be granted again.
#[test]
fn gmail_refusal_diagnostics_cover_password_and_oauth_paths() {
  // Gmail's classic IMAP answers.
  let not_accepted = refusal_hint("no response: [alert] username and password not accepted. learn more at https://support.google.com/mail/?p=BadCredentials");
  assert!(not_accepted.contains("App Password"), "got: {not_accepted}");
  assert!(not_accepted.contains("Sign in with Google"), "got: {not_accepted}");
  let less_secure = refusal_hint("no [alert] less secure app access is turned off for this account");
  assert!(less_secure.contains("App Password"), "got: {less_secure}");
  assert!(less_secure.contains("2-Step Verification"), "got: {less_secure}");
  // The identical Google answer on the OAuth path is a re-authorization, not a
  // password problem.
  let oauth_refused = xoauth2_refusal_hint("no [authenticationfailed] invalid credentials (failure)");
  assert!(oauth_refused.contains("granted again"), "got: {oauth_refused}");
  let oauth_expired = xoauth2_refusal_hint("no [authenticationfailed] token has been expired or revoked");
  assert!(oauth_expired.contains("granted again"), "got: {oauth_expired}");
  let oauth_disabled = xoauth2_refusal_hint("no [alert] imap access is disabled for your domain");
  assert!(oauth_disabled.contains("administrator"), "got: {oauth_disabled}");
  let oauth_limited = xoauth2_refusal_hint("no [ratelimit] too many invalid logins");
  assert!(oauth_limited.contains("rate-limiting"), "got: {oauth_limited}");
  let oauth_unknown = xoauth2_refusal_hint("no response: ");
  assert!(oauth_unknown.contains("without a specific reason"), "got: {oauth_unknown}");
  let failure = relay_lib::verify::VerifyFailure::GoogleAuth(oauth_refused);
  assert!(failure.user_message().contains("sign-in again"), "got: {}", failure.user_message());
  assert!(failure.user_message().contains("myaccount.google.com/permissions"));
  // A revoked grant must never be answered with "create an App Password".
  assert!(!failure.user_message().contains("App Password"), "got: {}", failure.user_message());
  assert!(failure.detail().contains("Google authorization"), "got: {}", failure.detail());
}

/// The OAuth client is validated (id and optional secret) before any browser
/// round-trip, and Google's token errors are mapped to fixed, non-sensitive
/// hints that name the right remedy without echoing the response body.
#[test]
fn google_oauth_client_validation_and_error_hints() {
  use relay_lib::oauth::{google_error_hint, validate_client_id, validate_client_secret};
  let valid = "1234567890-abcdefghijklmnopqrstuvwxyz123456.apps.googleusercontent.com";
  assert!(validate_client_id(valid).is_none());
  assert!(validate_client_id(&format!("  {valid}  ")).is_none());
  assert!(validate_client_id("").expect("empty").contains("Paste the Google OAuth client id"));
  assert!(validate_client_id("abc.apps.googleusercontent.com").expect("short").contains("wrong length"));
  assert!(validate_client_id("GOCSPX-abcdefghijklmnopqrstuvwxyz").expect("secret pasted as id").contains("client secret"));
  assert!(validate_client_id("AIzaSyA1234567890abcdefghijklmnopqrstu").expect("api key").contains("API key"));
  assert!(validate_client_id("1234567890-abcdefghijklmnopqrstuvwxyz12").expect("partial paste").contains(".apps.googleusercontent.com"));
  assert!(validate_client_id(&format!("{valid} extra")).expect("whitespace").contains("whitespace"));
  // The secret is optional; when present it must not be the id or an API key.
  assert!(validate_client_secret("").is_none());
  assert!(validate_client_secret("   ").is_none());
  assert!(validate_client_secret("GOCSPX-abcdefghijklmnopqrstuvwxyz").is_none());
  assert!(validate_client_secret(valid).expect("id in the secret field").contains("client id"));
  assert!(validate_client_secret("AIzaSyA1234567890abcdefghijklmnopqrstu").expect("api key").contains("API key"));
  assert!(validate_client_secret("short").expect("length").contains("wrong length"));
  assert!(validate_client_secret("GOCSPX-secret with space").expect("whitespace").contains("whitespace"));
  // Google's structured errors, without echoing the body.
  let bad_secret = google_error_hint(r#"{"error":"invalid_client","error_description":"The provided client secret is invalid."}"#);
  assert!(bad_secret.contains("client secret"), "got: {bad_secret}");
  let bad_id = google_error_hint(r#"{"error":"invalid_client"}"#);
  assert!(bad_id.contains("client id"), "got: {bad_id}");
  let redirect = google_error_hint(r#"{"error":"invalid_request","error_description":"redirect_uri_mismatch"}"#);
  assert!(redirect.contains("127.0.0.1"), "got: {redirect}");
  let denied = google_error_hint(r#"{"error":"access_denied"}"#);
  assert!(denied.contains("cancelled"), "got: {denied}");
  let revoked = google_error_hint(r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#);
  assert!(revoked.contains("myaccount.google.com/permissions"), "got: {revoked}");
  let scope = google_error_hint(r#"{"error":"invalid_scope"}"#);
  assert!(scope.contains("Gmail API"), "got: {scope}");
  let disabled = google_error_hint(r#"{"error":"invalid_request","error_description":"Gmail API has not been used in project 123 before or it is disabled"}"#);
  assert!(disabled.contains("Gmail API"), "got: {disabled}");
  let unknown = google_error_hint("not json at all");
  assert!(unknown.contains("refused the token exchange"), "got: {unknown}");
}


/// The Gmail routing defect: a Google mailbox reported the same generic
/// "credentials were refused" verdict as a mistyped password, so nothing told
/// the user that Google refuses IMAP passwords by policy. The host now refines
/// that verdict, and discovery records which method a provider requires.
#[test]
fn gmail_password_refusal_is_routed_to_google_sign_in() {
  // A generic refusal on Google's own IMAP host is a policy answer, not a typo.
  let google = refusal_hint_for("imap.gmail.com", "no [authenticationfailed] invalid credentials (failure)");
  assert!(google.contains("sign in with Google"), "got: {google}");
  assert!(google.contains("App Password"), "got: {google}");
  assert!(google.contains("myaccount.google.com/apppasswords"), "got: {google}");
  // The same response on another provider's host keeps the generic verdict.
  assert_eq!(refusal_hint_for("imap.northstar.test", "no [authenticationfailed] invalid credentials (failure)"), "the credentials were refused");
  // A refusal the provider already explained is never overwritten.
  let app_password = refusal_hint_for("imap.gmail.com", "no [alert] application-specific password required");
  assert!(app_password.contains("App Password"), "got: {app_password}");
  assert!(!app_password.contains("sign in with Google"), "a specific alert must not be replaced: {app_password}");
  // Even an unclassifiable refusal on a Google host is routed, because Google's
  // refusal code is not one this classifier can read. Configuration failures are
  // still safe: `classify_login_error` checks the protocol shape first and never
  // reaches this wrapper for them.
  assert!(refusal_hint_for("imap.gmail.com", "no response: ").contains("sign in with Google"));
  // Fixed text only — the server's words are never echoed.
  assert!(!google.contains("authenticationfailed"), "got: {google}");
  // Discovery carries the method, so the flow can route a Google mailbox.
  let gmail = relay_lib::discover::provider_candidates("gmail.com");
  assert_eq!(gmail[0].auth_method, "oauth2");
  assert!(relay_lib::discover::requires_oauth("user@gmail.com"));
  assert!(relay_lib::discover::requires_oauth("  User@GoogleMail.com "));
  assert!(!relay_lib::discover::requires_oauth("user@northstar.test"));
  // Other providers — and Workspace custom domains, where the admin decides —
  // stay on the password path.
  assert_eq!(relay_lib::discover::provider_candidates("yahoo.com")[0].auth_method, "password");
  assert_eq!(relay_lib::discover::mx_candidates(&["aspmx.l.google.com".into()])[0].auth_method, "password");
}

/// A refusal at the loopback redirect is final and must be reported at once.
/// Treating "no code" as "nothing yet" made a cancelled sign-in wait out the
/// whole timeout and then report a timeout instead of a cancellation.
#[test]
fn oauth_loopback_separates_grants_from_refusals() {
  use relay_lib::oauth::{loopback_refusal_hint, parse_loopback_callback, parse_loopback_query, LoopbackCallback};
  // A grant.
  assert_eq!(
    parse_loopback_callback("GET /?code=4%2F0Axx&state=s1 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"),
    LoopbackCallback::Code { code: "4/0Axx".into(), state: "s1".into() }
  );
  // A refusal: no code, which is why the old parser answered None and the wait
  // carried on to the deadline.
  assert_eq!(
    parse_loopback_callback("GET /?error=access_denied&state=s1 HTTP/1.1"),
    LoopbackCallback::Refusal { error: "access_denied".into(), state: "s1".into() }
  );
  assert!(parse_loopback_query("GET /?error=access_denied&state=s1 HTTP/1.1").is_none());
  // A description stands in when no error code is sent.
  match parse_loopback_callback("GET /?error_description=access_denied&state=s2 HTTP/1.1") {
    LoopbackCallback::Refusal { error, state } => { assert_eq!(error, "access_denied"); assert_eq!(state, "s2"); }
    other => panic!("expected a refusal, got {other:?}"),
  }
  // Stray or stateless requests stay unknown, so the wait keeps going.
  assert_eq!(parse_loopback_callback("GET /favicon.ico HTTP/1.1"), LoopbackCallback::Unknown);
  assert_eq!(parse_loopback_callback("GET /?code=x HTTP/1.1"), LoopbackCallback::Unknown);
  assert_eq!(parse_loopback_callback(""), LoopbackCallback::Unknown);
  // Refusal classification: fixed text, and the value itself is never echoed
  // (any local process could have sent it).
  assert!(loopback_refusal_hint("access_denied").contains("cancelled"));
  assert!(loopback_refusal_hint("invalid_scope").contains("Gmail API"));
  assert!(loopback_refusal_hint("unauthorized_client").contains("OAuth client"));
  assert!(loopback_refusal_hint("redirect_uri_mismatch").contains("127.0.0.1"));
  let other = loopback_refusal_hint("some_future_code");
  assert!(other.contains("did not authorize"), "got: {other}");
  assert!(!other.contains("some_future_code"), "the value must never be echoed: {other}");
}

/// A sync with no explicit ids covers every signed-in account. It used to take
/// one id chosen by the caller as `accounts[0]`, and that list is ordered by
/// display name — so the seeded demo account (or simply an alphabetically
/// earlier one) could be synced instead of the mailbox being viewed.
#[test]
fn sync_targets_every_signed_in_account() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let make = |name: &str, address: &str| CreateAccount { display_name: name.into(), email_address: address.into(), imap_host: "imap.example.test".into(), imap_port: 993, smtp_host: "smtp.example.test".into(), smtp_port: 465, encryption: "ssl".into(), auth_kind: "password".into() };
  // The debug seed may also be present; compare as a superset.
  let before = repos.account_ids().expect("ids");
  let first = repos.add_account_verified(&make("Zoe Real", "zoe@example.test"), "keyring:test:zoe").expect("first account");
  let second = repos.add_account_verified(&make("Aaron Early", "aaron@example.test"), "keyring:test:aaron").expect("second account");
  let ids = repos.account_ids().expect("ids after sign-in");
  assert_eq!(ids.len(), before.len() + 2, "both new accounts are included");
  assert!(ids.contains(&first.id) && ids.contains(&second.id));
  // Insertion order, not display order: "Aaron Early" must not be treated as the
  // one account to sync just because it sorts first.
  let mine: Vec<&String> = ids.iter().filter(|id| **id == first.id || **id == second.id).collect();
  assert_eq!(mine, vec![&first.id, &second.id], "accounts are ordered by creation, so the sync covers both in a stable order");
  // A removed account disappears from the sync work list.
  repos.remove_account(&second.id).expect("remove");
  let after = repos.account_ids().expect("ids after removal");
  assert!(!after.contains(&second.id), "a removed account is not synced");
  assert!(after.contains(&first.id));
  close_temp(database, directory);
}

/// The defect that made a sync appear to succeed while downloading nothing:
/// the FETCH range is built from `inbox.exists`, which is a count in *sequence*
/// space, but the call used to be `UID FETCH` — which reads the range as UIDs.
/// On a mailbox whose UID numbering has advanced past its current size (any
/// mailbox with deletions) that matches nothing and returns an empty response,
/// indistinguishable from an empty mailbox.
#[test]
fn newest_sequence_range_covers_the_tail_of_the_mailbox() {
  use relay_lib::mailbox::newest_sequence_range;
  // An empty mailbox fetches nothing at all.
  assert_eq!(newest_sequence_range(0, 50), None);
  // A cap of zero is a programming error, not a request for everything.
  assert_eq!(newest_sequence_range(100, 0), None);
  // Fewer messages than the cap: the whole mailbox, starting at sequence 1.
  assert_eq!(newest_sequence_range(1, 50), Some((1, 1)));
  assert_eq!(newest_sequence_range(10, 50), Some((1, 10)));
  // Exactly the cap, and one more than the cap (the boundary).
  assert_eq!(newest_sequence_range(50, 50), Some((1, 50)));
  assert_eq!(newest_sequence_range(51, 50), Some((2, 51)));
  // A large mailbox: the tail only, and the range always ends at `exists`.
  assert_eq!(newest_sequence_range(120, 50), Some((71, 120)));
  assert_eq!(newest_sequence_range(500, 50), Some((451, 500)));
  // Every range is well-formed for IMAP and never underflows to 0.
  for exists in [1u32, 7, 49, 50, 51, 999, 100_000] {
    let (first, last) = newest_sequence_range(exists, 50).expect("non-empty mailbox has a range");
    assert!(first >= 1, "sequence numbers are 1-based (got {first})");
    assert!(first <= last, "range must be ascending (got {first}:{last})");
    assert_eq!(last, exists, "the range must end at the newest message");
    assert!(last - first + 1 <= 50, "the cap must hold (got {} messages)", last - first + 1);
  }
}

/// Mail retrieval: the MIME decoding helpers are the part that runs on
/// untrusted input, so they are pinned against realistic fixtures (base64 and
/// quoted-printable bodies, RFC 2047 subjects, nested multipart and HTML).
#[test]
fn mailbox_decodes_real_message_shapes() {
  use relay_lib::mailbox::{decode_base64, decode_mime_words, decode_quoted_printable, html_to_text, text_body_of};
  // RFC 4648 vectors, then the line-broken form that bodies actually use.
  assert_eq!(decode_base64("Zm9vYmFy"), "foobar");
  assert_eq!(decode_base64("Zm9v\nYmFy"), "foobar");
  assert_eq!(decode_base64(""), "");
  // Quoted-printable: hex escapes and a soft line break that joins two lines.
  assert_eq!(decode_quoted_printable("caf=C3=A9"), "café");
  assert_eq!(decode_quoted_printable("one=\r\ntwo"), "onetwo");
  assert_eq!(decode_quoted_printable("100% = 50%"), "100% = 50%");
  // RFC 2047 subjects (the B and Q encodings), plus an untouched plain subject.
  assert_eq!(decode_mime_words("=?UTF-8?B?SGVsbG8=?="), "Hello");
  assert_eq!(decode_mime_words("=?UTF-8?Q?caf=C3=A9_break?="), "café break");
  assert_eq!(decode_mime_words("Plain subject"), "Plain subject");
  // A malformed encoded word is left as it arrived rather than swallowed.
  assert_eq!(decode_mime_words("=?UTF-8?B?not-closed"), "=?UTF-8?B?not-closed");
  assert_eq!(decode_mime_words("=?UTF-8?Z?SGVsbG8=?="), "=?UTF-8?Z?SGVsbG8=?=");
  // A simple text/plain message.
  assert_eq!(text_body_of("Subject: Hi\r\n\r\nHello there\r\n"), "Hello there");
  // Charset-encoded base64 body.
  assert_eq!(text_body_of("Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\nY2Fmw6k=\r\n"), "café");
  // multipart/alternative: text/plain wins over the HTML version.
  let alternative = concat!(
    "Content-Type: multipart/alternative; boundary=\"b1\"\r\n\r\n",
    "--b1\r\nContent-Type: text/plain\r\n\r\nplain choice\r\n",
    "--b1\r\nContent-Type: text/html\r\n\r\n<p>html choice</p>\r\n",
    "--b1--\r\n"
  );
  assert_eq!(text_body_of(alternative), "plain choice");
  // A nested multipart whose only readable part is HTML: reduced to text, with
  // script and style content dropped entirely.
  let nested = concat!(
    "Content-Type: multipart/mixed; boundary=\"outer\"\r\n\r\n",
    "--outer\r\nContent-Type: multipart/related; boundary=\"inner\"\r\n\r\n",
    "--inner\r\nContent-Type: text/html\r\n\r\n",
    "<html><head><style>p{color:red}</style><script>alert('x')</script></head>",
    "<body><p>Hello <b>world</b></p><p>Second&nbsp;line &amp; more</p></body></html>\r\n",
    "--inner--\r\n--outer--\r\n"
  );
  let text = text_body_of(nested);
  assert!(text.contains("Hello world"), "got: {text}");
  assert!(text.contains("Second line & more"), "got: {text}");
  assert!(!text.contains("alert"), "script content must be dropped: {text}");
  assert!(!text.contains("color:red"), "style content must be dropped: {text}");
  assert!(!text.contains('<'), "no markup may survive: {text}");
  // A message with no readable part yields an empty body, never a panic.
  assert_eq!(text_body_of("Content-Type: application/octet-stream\r\n\r\n\x01\x02"), "");
  assert_eq!(text_body_of(""), "");
  // HTML helpers: entities (named, decimal and hex) and block-level breaks.
  assert_eq!(html_to_text("a<br>b"), "a\nb");
  assert_eq!(html_to_text("<div>one</div><div>two</div>"), "one\ntwo");
  assert_eq!(html_to_text("5 &lt; 6 &#65; &#x42;"), "5 < 6 A B");
  assert_eq!(html_to_text("&unknown; &amp"), "&unknown; &amp");
}

/// The folder-scaffolding defect: `emails_in_folder` filters on the folder role,
/// but a signed-in account had no folder rows at all, so its inbox could never
/// show anything — even with mail stored. Creating them on sign-in (and on sync,
/// for accounts that predate the fix) is what makes retrieved mail visible.
#[test]
fn accounts_own_their_folders_and_fetched_mail_becomes_visible() {
  use relay_lib::mailbox::FetchedMessage;
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let input = CreateAccount { display_name: "Fetch User".into(), email_address: "fetch@example.test".into(), imap_host: "imap.example.test".into(), imap_port: 993, smtp_host: "smtp.example.test".into(), smtp_port: 465, encryption: "ssl".into(), auth_kind: "password".into() };
  let account = repos.add_account_verified(&input, "keyring:test:fetch").expect("sign-in");
  // Sign-in scaffolds every role the views query, and is idempotent.
  let roles: Vec<String> = {
    let mut statement = database.connection().prepare("SELECT role FROM email_folders WHERE account_id = ?1 AND deleted_at IS NULL ORDER BY role").expect("prepare");
    statement.query_map(params![account.id], |row| row.get(0)).expect("query").collect::<Result<Vec<_>, _>>().expect("roles")
  };
  assert_eq!(roles, vec!["archive", "drafts", "inbox", "sent", "trash"]);
  repos.ensure_account_folders(&account.id).expect("second scaffold is a no-op");
  let count: i64 = database.connection().query_row("SELECT COUNT(*) FROM email_folders WHERE account_id = ?1", params![account.id], |row| row.get(0)).expect("count");
  assert_eq!(count, 5, "scaffolding must not duplicate folders");
  let first_id = format!("imap-{}-1", account.id);
  // Debug builds also seed a demo account, so every assertion below is scoped to
  // the account this test created.
  let mine = |rows: Vec<EmailSummary>| -> Vec<EmailSummary> { rows.into_iter().filter(|email| email.id.starts_with(&format!("imap-{}-", account.id))).collect() };
  assert!(mine(repos.inbox().expect("inbox before fetch")).is_empty(), "nothing is visible before a fetch");
  let fetched = vec![
    FetchedMessage { uid: 1, sender_name: "Maya Chen".into(), sender_email: "maya@example.test".into(), subject: "Quarterly planning".into(), body_text: "The document is ready.".into(), received_at: "2026-02-01T10:00:00+00:00".into(), is_read: false, is_starred: true, message_id: Some("id-1@example.test".into()) },
    FetchedMessage { uid: 2, sender_name: "Diego".into(), sender_email: "diego@example.test".into(), subject: "Re: audit".into(), body_text: "Added my notes.".into(), received_at: "2026-02-02T10:00:00+00:00".into(), is_read: true, is_starred: false, message_id: None },
  ];
  assert_eq!(repos.upsert_fetched_messages(&account.id, "inbox", &fetched).expect("store fetched mail"), 2);
  let inbox = mine(repos.inbox().expect("inbox"));
  assert_eq!(inbox.len(), 2, "fetched mail must appear in the inbox");
  assert_eq!(inbox[0].subject, "Re: audit", "newest first");
  assert!(inbox[0].is_read);
  assert!(inbox.iter().any(|email| email.id == first_id && email.is_starred));
  // Re-syncing the same UIDs updates in place instead of duplicating the mailbox.
  let revised = vec![FetchedMessage { uid: 1, sender_name: "Maya Chen".into(), sender_email: "maya@example.test".into(), subject: "Quarterly planning (revised)".into(), body_text: "The revised document is ready.".into(), received_at: "2026-02-01T10:00:00+00:00".into(), is_read: true, is_starred: true, message_id: Some("id-1@example.test".into()) }];
  assert_eq!(repos.upsert_fetched_messages(&account.id, "inbox", &revised).expect("re-sync"), 1);
  let after_resync = mine(repos.inbox().expect("inbox after re-sync"));
  assert_eq!(after_resync.len(), 2, "a repeat sync must not duplicate rows");
  assert!(after_resync.iter().any(|email| email.id == first_id && email.subject == "Quarterly planning (revised)"));
  // Trashing is a local folder move, not a delete: a later sync must refresh the
  // content but leave the message where the user put it, rather than dragging it
  // back into the inbox.
  repos.trash_email(&first_id).expect("trash");
  assert_eq!(repos.upsert_fetched_messages(&account.id, "inbox", &fetched).expect("sync after trashing"), 2);
  let inbox_after = mine(repos.inbox().expect("inbox after trash"));
  assert_eq!(inbox_after.len(), 1, "the trashed message must stay out of the inbox");
  assert!(inbox_after.iter().all(|email| email.id != first_id));
  let trashed = repos.emails_in_folder("trash").expect("trash view");
  assert!(trashed.iter().any(|email| email.id == first_id), "the trashed message stays in trash");
  // ...while its content is still refreshed by the sync.
  assert!(trashed.iter().any(|email| email.id == first_id && email.subject == "Quarterly planning"), "the sync still refreshes content");
  // Sync bookkeeping round-trips, so UIDVALIDITY/UIDNEXT can be persisted.
  assert!(repos.sync_metadata("imap.test.uidvalidity").expect("read").is_none());
  repos.set_sync_metadata("imap.test.uidvalidity", "42").expect("write");
  assert_eq!(repos.sync_metadata("imap.test.uidvalidity").expect("read back").as_deref(), Some("42"));
  assert!(repos.set_sync_metadata("", "x").is_err());
  // An unknown folder role is rejected rather than written.
  assert!(repos.upsert_fetched_messages(&account.id, "bogus", &fetched).is_err());
  close_temp(database, directory);
}

/// The empty-body defect: `fetch_inbox` asked the server for `BODY[HEADER]` and
/// `BODY[TEXT]`, but read the response with `Fetch::body()` — which matches the
/// **unsectioned** `BODY[]` item only. Every message therefore arrived as its
/// header alone, `text_body_of` found nothing after the blank line, and the
/// reader showed a message with a subject and no content.
#[test]
fn raw_message_assembly_keeps_the_body() {
  use relay_lib::mailbox::{assemble_raw_message, text_body_of};
  let header = b"Subject: Hi\r\nContent-Type: text/plain; charset=utf-8\r\n";
  let body = b"Hello there\r\n";
  // The shape requested today: one complete message, used as it arrives.
  let full = b"Subject: Hi\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello there\r\n";
  assert_eq!(text_body_of(&String::from_utf8_lossy(&assemble_raw_message(Some(full), None, None))), "Hello there");
  // A server that answers with the sections instead still decodes, in both of the
  // shapes `BODY[TEXT]` may take: body alone, and body with the header included.
  let stitched = assemble_raw_message(None, Some(header), Some(body));
  assert_eq!(text_body_of(&String::from_utf8_lossy(&stitched)), "Hello there");
  let mut header_and_body = header.to_vec();
  header_and_body.extend_from_slice(b"\r\n");
  header_and_body.extend_from_slice(body);
  let deduped = assemble_raw_message(None, Some(header), Some(&header_and_body));
  assert_eq!(text_body_of(&String::from_utf8_lossy(&deduped)), "Hello there");
  assert_eq!(deduped.windows(11).filter(|window| *window == b"Subject: Hi").count(), 1, "the header must not be duplicated into the body");
  // The failure mode this test exists to prevent: a header on its own carries no
  // readable text, so a fetch that keeps only `header()` renders nothing at all.
  assert_eq!(text_body_of(&String::from_utf8_lossy(&assemble_raw_message(None, Some(header), None))), "");
  assert!(assemble_raw_message(None, None, None).is_empty());
}

/// The repair path for a row that was already stored with an empty body: the
/// reading pane asks the server for that single message by UID and writes the
/// decoded text back — without queueing an outbound change, because a body read
/// from IMAP is remote truth rather than a local edit.
#[test]
fn stored_email_body_can_be_repaired() {
  use relay_lib::mailbox::FetchedMessage;
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  let input = CreateAccount { display_name: "Repair User".into(), email_address: "repair@example.test".into(), imap_host: "imap.example.test".into(), imap_port: 993, smtp_host: "smtp.example.test".into(), smtp_port: 465, encryption: "ssl".into(), auth_kind: "password".into() };
  let account = repos.add_account_verified(&input, "keyring:test:repair").expect("sign-in");
  let id = format!("imap-{}-7", account.id);
  // A row exactly as the broken fetch wrote it: envelope content present, body empty.
  let blank = vec![FetchedMessage { uid: 7, sender_name: "Maya Chen".into(), sender_email: "maya@example.test".into(), subject: "Quarterly planning".into(), body_text: String::new(), received_at: "2026-02-01T10:00:00+00:00".into(), is_read: false, is_starred: false, message_id: None }];
  assert_eq!(repos.upsert_fetched_messages(&account.id, "inbox", &blank).expect("store blank row"), 1);
  assert_eq!(repos.email(&id).expect("detail").expect("row").body_text, "", "the defect leaves a stored body empty");
  // The owning account and the IMAP UID are what address a single-message fetch.
  let (target_account, uid) = repos.email_fetch_target(&id).expect("target").expect("a fetched message has a server uid");
  assert_eq!(target_account, account.id);
  assert_eq!(uid, 7);
  let queued = |id: &str| -> i64 { database.connection().query_row("SELECT COUNT(*) FROM sync_queue WHERE entity_id = ?1", params![id], |row| row.get(0)).expect("count queued work") };
  let queued_before = queued(&id);
  repos.store_email_body(&id, "The document is ready.").expect("repair");
  assert_eq!(queued(&id), queued_before, "downloading a body must not queue an outbound change");
  let repaired = repos.email(&id).expect("detail").expect("row");
  assert_eq!(repaired.body_text, "The document is ready.");
  assert_eq!(repaired.subject, "Quarterly planning", "the repair must leave the rest of the row alone");
  // A draft, or an unknown id, has no server counterpart to download from.
  assert!(repos.email_fetch_target("does-not-exist").expect("missing row").is_none());
  close_temp(database, directory);
}

/// Google OAuth helpers must match the wire specifications: base64url (RFC
/// 4648 §5, unpadded) for PKCE and JWT payloads, the PKCE S256 challenge as
/// base64url(SHA-256(verifier)), the XOAUTH2 SASL payload (RFC 7628), loopback
/// callback parsing with percent-decoded codes, and id_token email extraction.
#[test]
fn google_oauth_helpers_match_specifications() {
  use relay_lib::oauth::{base64url_encode, email_from_id_token, parse_loopback_query, pkce_pair, xoauth2_payload};
  use sha2::Digest;
  // RFC 4648 §5 test vectors (unpadded).
  assert_eq!(base64url_encode(b""), "");
  assert_eq!(base64url_encode(b"f"), "Zg");
  assert_eq!(base64url_encode(b"fo"), "Zm8");
  assert_eq!(base64url_encode(b"foo"), "Zm9v");
  assert_eq!(base64url_encode(b"foobar"), "Zm9vYmFy");
  // PKCE: verifier within the allowed range/charset, challenge = S256(verifier).
  let (verifier, challenge) = pkce_pair();
  assert!(verifier.len() >= 43 && verifier.len() <= 128);
  assert!(verifier.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'));
  assert_eq!(challenge, base64url_encode(&sha2::Sha256::digest(verifier.as_bytes())));
  // XOAUTH2 payload: base64("user=<a>\x01auth=Bearer <t>\x01\x01").
  assert_eq!(xoauth2_payload("u@x.test", "tok"), base64_encode(b"user=u@x.test\x01auth=Bearer tok\x01\x01"));
  // Loopback callback parsing, including a percent-decoded authorization code.
  assert_eq!(parse_loopback_query("GET /?code=4%2F0Axx&state=s1 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_ref().map(|(code, state)| (code.as_str(), state.as_str())), Some(("4/0Axx", "s1")));
  assert_eq!(parse_loopback_query("GET /?state=s2 HTTP/1.1"), None);
  assert_eq!(parse_loopback_query("GET / HTTP/1.1"), None);
  // id_token email extraction on an (unsigned) token: only the payload matters.
  let payload = base64url_encode(br#"{"email":"me@gmail.com","sub":"1"}"#);
  let token = format!("ignored.{payload}.signature");
  assert_eq!(email_from_id_token(&token).as_deref(), Some("me@gmail.com"));
  assert_eq!(email_from_id_token("not-a-jwt"), None);
}

/// Thunderbird-style discovery: provider-database mappings, MX classification
/// (Google Workspace / Microsoft 365), hostname guesses, autoconfig XML
/// parsing, and candidate ordering with de-duplication and the size cap.
#[test]
fn thunderbird_style_discovery_ranks_and_parses() {
  use relay_lib::discover::{discover_candidates, guess_candidates, mx_candidates, parse_autoconfig_smtp, parse_autoconfig_xml, provider_candidates};
  // Provider database (highest-security configuration per provider).
  let gmail = provider_candidates("GMAIL.com");
  assert_eq!(gmail.len(), 1);
  assert_eq!(gmail[0].imap_host, "imap.gmail.com");
  assert_eq!(gmail[0].imap_port, 993);
  assert_eq!(gmail[0].encryption, "ssl");
  assert_eq!(gmail[0].provider, Some("google"));
  assert!(provider_candidates("unknown-domain.test").is_empty());
  // MX classification detects Workspace and Microsoft 365 custom domains.
  let mx = mx_candidates(&["aspmx.l.google.com".into()]);
  assert_eq!(mx.len(), 1);
  assert_eq!(mx[0].imap_host, "imap.gmail.com");
  assert_eq!(mx[0].provider, Some("google"));
  let m365 = mx_candidates(&["example-com.mail.protection.outlook.com".into()]);
  assert_eq!(m365[0].imap_host, "outlook.office365.com");
  assert_eq!(m365[0].provider, Some("microsoft"));
  assert!(mx_candidates(&["mail.unknown-host.test".into()]).is_empty());
  // Hostname guesses for remaining custom domains (SSL first, then STARTTLS).
  let guesses = guess_candidates("example.test");
  assert_eq!(guesses.len(), 4);
  assert_eq!(guesses[0].imap_host, "imap.example.test");
  assert_eq!(guesses[0].encryption, "ssl");
  assert!(guesses.iter().any(|candidate| candidate.imap_port == 143 && candidate.encryption == "tls"));
  // Autoconfig XML: IMAP SSL + STARTTLS kept, plain and POP dropped, ports defaulted.
  let xml = concat!(
    "<clientConfig version=\"1.1\">",
    "<emailprovider id=\"example.test\">",
    "<incomingServer type=\"imap\"><hostname>imap2.example.test</hostname><port>993</port><socketType>SSL</socketType></incomingServer>",
    "<incomingServer type=\"imap\"><hostname>imap3.example.test</hostname><socketType>STARTTLS</socketType></incomingServer>",
    "<incomingServer type=\"imap\"><hostname>plain.example.test</hostname><port>143</port><socketType>plain</socketType></incomingServer>",
    "<incomingServer type=\"pop3\"><hostname>pop.example.test</hostname><port>995</port><socketType>SSL</socketType></incomingServer>",
    "<outgoingServer type=\"smtp\"><hostname>smtp2.example.test</hostname><port>465</port><socketType>SSL</socketType></outgoingServer>",
    "</emailprovider></clientConfig>"
  );
  let parsed = parse_autoconfig_xml(xml);
  assert_eq!(parsed.len(), 2);
  assert_eq!(parsed[0].imap_host, "imap2.example.test");
  assert_eq!(parsed[0].encryption, "ssl");
  assert_eq!(parsed[1].imap_host, "imap3.example.test");
  assert_eq!(parsed[1].imap_port, 143);
  assert_eq!(parsed[1].encryption, "tls");
  assert_eq!(parse_autoconfig_smtp(xml).as_ref().map(|(host, port)| (host.as_str(), *port)), Some(("smtp2.example.test", 465)));
  // Full ranking for a known provider: provider DB first, MX deduped into it,
  // then guesses; no plaintext candidate ever enters the list, and the cap holds.
  let candidates = discover_candidates("user@gmail.com", &|_domain| vec!["aspmx.l.google.com".into()]);
  assert!(candidates.len() >= 2);
  assert_eq!(candidates[0].imap_host, "imap.gmail.com");
  assert_eq!(candidates[0].source, "provider-db");
  assert!(candidates.iter().all(|candidate| candidate.encryption != "none"));
  let mut keys: Vec<(String, u16, &'static str)> = candidates.iter().map(|candidate| (candidate.imap_host.clone(), candidate.imap_port, candidate.encryption)).collect();
  keys.sort();
  keys.dedup();
  assert_eq!(keys.len(), candidates.len(), "candidates must be de-duplicated");
  assert!(candidates.len() <= relay_lib::discover::MAX_CANDIDATES);
}
/// The MX lookup runtime must keep Tokio's timer driver enabled. hickory wraps
/// every name-server connection in `TokioTime::timeout` (tokio::time), so a
/// Tokio context without timers panics on the first query — and that panic used
/// to travel into the webview2 IPC callback, which cannot unwind, aborting the
/// whole app during sign-in (STATUS_STACK_BUFFER_OVERRUN). The test shares the
/// production builder (`discover::resolver_runtime`) so the two cannot drift.
#[test]
fn mx_lookup_runtime_keeps_timers_enabled() {
  let runtime = relay_lib::discover::resolver_runtime().expect("the resolver runtime should build");
  runtime.block_on(async {
    // The shape of the call hickory makes per query: without the timer driver
    // this panics with "timers are disabled" instead of returning.
    let slept = tokio::time::timeout(std::time::Duration::from_secs(1), tokio::time::sleep(std::time::Duration::from_millis(1))).await;
    assert!(slept.is_ok(), "the resolver runtime must be able to drive tokio timers");
  });
}

/// A panic in command work must come back as a command error. Synchronous
/// commands run inside the webview2 IPC callback on the UI thread, whose
/// `extern "system"` frame cannot unwind: an escaping panic kills the process
/// (STATUS_STACK_BUFFER_OVERRUN) and the user never sees the failed action.
#[test]
fn panics_in_command_work_become_errors() {
  // A thrown panic is converted into the generic command error...
  let exploded = relay_lib::guard::guarded(|| -> Result<(), String> { panic!("discovery exploded") });
  assert_eq!(exploded, Err(relay_lib::guard::PANIC_MESSAGE.to_string()));
  // ...while real results travel through untouched, message included.
  assert_eq!(relay_lib::guard::guarded(|| Ok::<u8, String>(7)), Ok(7));
  assert_eq!(relay_lib::guard::guarded(|| Err::<u8, String>("Server refused the password.".into())), Err("Server refused the password.".to_string()));
  // The panic payload stays developer-facing: it is logged, never surfaced.
  let literal: Box<dyn std::any::Any + Send> = Box::new("boom");
  assert_eq!(relay_lib::guard::panic_message(&*literal), "boom");
  let formatted: Box<dyn std::any::Any + Send> = Box::new(String::from("boom 2"));
  assert_eq!(relay_lib::guard::panic_message(&*formatted), "boom 2");
}


/// The classifier turns a failed attempt into fixed wording rather than the
/// server's own text, which can quote the message or the command back at us. A
/// Google authorization refusal is kept distinct from a password refusal because
/// the remedy the user needs is different.
#[test]
fn smtp_failure_classification_is_fixed_and_specific() {
  use relay_lib::transport::failure_message;
  // Authentication refusals: the password remedy and the Google remedy differ.
  let password = failure_message(Some(535), false, true, false);
  let google = failure_message(Some(535), true, true, false);
  assert!(password.contains("Check the account password"), "password refusal points at the credential: {password}");
  assert!(google.contains("Sign in with Google again"), "an OAuth refusal points at Google: {google}");
  assert_ne!(password, google);
  // A refused message names the likely cause instead of blaming the credentials.
  assert!(failure_message(Some(550), false, true, false).contains("recipient addresses"));
  // A transient refusal asks for a retry rather than a configuration change.
  assert!(failure_message(Some(451), false, true, false).contains("Try again shortly"));
  // A dropped connection and a timeout share the same honest answer.
  assert!(failure_message(None, false, false, false).contains("dropped while sending"));
  assert!(failure_message(None, false, true, true).contains("dropped while sending"));
  // An unrecognized code still gets an answer; it never falls through to nothing.
  let unknown = failure_message(Some(555), false, true, false);
  assert!(unknown.contains("Sending failed"), "unexpected answer: {unknown}");
}

/// The assembled message must carry the sender, the recipients, the subject and
/// the body, and a Bcc recipient must reach the envelope without being written
/// into the transmitted headers — a leaked Bcc is a privacy bug, not a cosmetic
/// one. A non-ASCII subject is encoded rather than sent as raw bytes.
#[test]
fn outbound_message_encodes_recipients_and_body() {
  use relay_lib::transport::build_message;
  let built = build_message(&OutboundMessage {
    id: "encode-1".into(),
    from_name: "Alex Morgan".into(),
    from_address: "alex@northstar.test".into(),
    to: vec!["maya@northstar.test".into()],
    cc: vec!["sam@northstar.test".into()],
    bcc: vec!["ops@northstar.test".into()],
    subject: "Delivery check".into(),
    body_text: "No spool, no relay, just a test.".into(),
  }).expect("assemble");
  let raw = String::from_utf8(built.formatted()).expect("utf-8 message");
  assert!(raw.contains("Alex Morgan") && raw.contains("<alex@northstar.test>"), "sender transmitted: {raw}");
  assert!(raw.contains("maya@northstar.test") && raw.contains("sam@northstar.test"), "visible recipients transmitted: {raw}");
  assert!(raw.contains("Subject: Delivery check"), "subject transmitted: {raw}");
  assert!(raw.contains("No spool, no relay, just a test."), "body transmitted: {raw}");
  assert!(!raw.to_lowercase().contains("bcc"), "a Bcc recipient must never appear in the headers: {raw}");
  // The envelope is what the relay is told to deliver to, so it still carries it.
  let envelope = built.envelope();
  assert!(envelope.to().iter().any(|address| address.to_string() == "ops@northstar.test"), "the Bcc recipient stays on the envelope");
  // A subject outside ASCII is encoded (RFC 2047) instead of being sent raw.
  let encoded = build_message(&OutboundMessage { id: "encode-2".into(), from_name: String::new(), from_address: "alex@northstar.test".into(), to: vec!["maya@northstar.test".into()], cc: vec![], bcc: vec![], subject: "Grüße aus Berlin".into(), body_text: "body".into() }).expect("assemble");
  let encoded_raw = String::from_utf8(encoded.formatted()).expect("utf-8 message");
  assert!(encoded_raw.to_lowercase().contains("=?utf-8?"), "a non-ASCII subject is encoded: {encoded_raw}");
  assert!(!encoded_raw.contains("Grüße aus Berlin"), "the raw non-ASCII subject must not be transmitted: {encoded_raw}");
}


/// A minimal plaintext SMTP relay for one session: greeting, EHLO, AUTH,
/// MAIL/RCPT/DATA/QUIT. The DATA payload is reported through `capture` the moment
/// it arrives — after the relay has accepted it — so the test never waits out the
/// session's dismantling; the session itself runs on until the client says QUIT
/// or the socket closes. A connection that ends without mail (the client's
/// reachability probe opens and drops one before the real session) simply ends.
fn serve_smtp(stream: std::net::TcpStream, capture: &std::sync::mpsc::Sender<String>) {
  use std::io::{BufRead, BufReader, Write};
  // A socket accepted from a non-blocking listener can itself arrive
  // non-blocking (Windows does this), which would make every read return
  // `WouldBlock` instead of waiting for the client's commands.
  stream.set_nonblocking(false).ok();
  stream.set_read_timeout(Some(std::time::Duration::from_secs(10))).ok();
  let mut writer = stream.try_clone().expect("clone relay socket");
  let mut reader = BufReader::new(stream);
  let reply = |writer: &mut std::net::TcpStream, line: &str| { let _ = writer.write_all(line.as_bytes()); };
  reply(&mut writer, "220 relay.test ESMTP ready\r\n");
  let mut line = String::new();
  loop {
    line.clear();
    match reader.read_line(&mut line) {
      Ok(0) | Err(_) => return,
      Ok(_) => {}
    }
    let command = line.trim_end().to_ascii_uppercase();
    if command.starts_with("EHLO") || command.starts_with("HELO") {
      reply(&mut writer, "250-relay.test\r\n250-AUTH PLAIN LOGIN\r\n250-8BITMIME\r\n250 SMTPUTF8\r\n");
    } else if command.starts_with("AUTH") {
      // Any credentials are accepted: what is under test is the delivery path,
      // not the relay's opinion of the password.
      reply(&mut writer, "235 2.7.0 authentication successful\r\n");
    } else if command.starts_with("DATA") {
      reply(&mut writer, "354 end data with <CRLF>.<CRLF>\r\n");
      let mut message = String::new();
      loop {
        let mut chunk = String::new();
        match reader.read_line(&mut chunk) {
          Ok(0) | Err(_) => break,
          Ok(_) => {}
        }
        if chunk == ".\r\n" || chunk == ".\n" { break; }
        message.push_str(&chunk);
      }
      reply(&mut writer, "250 2.0.0 message accepted\r\n");
      let _ = capture.send(message);
    } else if command.starts_with("QUIT") {
      reply(&mut writer, "221 2.0.0 bye\r\n");
      return;
    } else {
      reply(&mut writer, "250 2.1.0 ok\r\n");
    }
  }
}

/// The whole send path over a real socket: a loopback relay speaks plaintext SMTP
/// (the account's encryption mode is `none`, exactly what the store holds for
/// such a server), the client authenticates, and the message that arrives is the
/// one the store assembled. This is the delivery counterpart of the fetch
/// fixtures: protocol behaviour proven without live credentials.
#[test]
fn smtp_delivery_reaches_a_loopback_relay() {
  use relay_lib::transport::{deliver, DeliveryOutcome};
  use relay_lib::verify::MailCredential;
  use std::io::ErrorKind;
  use std::net::TcpListener;
  use std::sync::mpsc;
  use std::time::{Duration, Instant};

  let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback relay");
  let port = listener.local_addr().expect("relay address").port();
  let (relay_tx, relay_rx) = mpsc::channel::<String>();
  // The relay thread is deliberately detached: the payload arrives on the channel
  // as soon as the relay accepts it, and waiting for the session to be torn down
  // afterwards would only add the client's own QUIT timing to the test.
  let _relay = std::thread::spawn(move || {
    listener.set_nonblocking(true).expect("non-blocking accept");
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
      match listener.accept() {
        Ok((stream, _)) => serve_smtp(stream, &relay_tx),
        Err(ref error) if error.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
        Err(_) => return,
      }
    }
  });
  let connection = AccountConnection { id: "loopback".into(), email_address: "alex@northstar.test".into(), imap_host: "127.0.0.1".into(), imap_port: port, smtp_host: "127.0.0.1".into(), smtp_port: port, encryption: "none".into(), auth_kind: "password".into() };
  let message = OutboundMessage { id: "loopback-1".into(), from_name: "Alex Morgan".into(), from_address: "alex@northstar.test".into(), to: vec!["maya@northstar.test".into()], cc: vec![], bcc: vec![], subject: "Loopback delivery".into(), body_text: "This sentence must arrive intact.".into() };
  let outcomes = deliver(&connection, MailCredential::Password("app-password"), &[message]);
  assert_eq!(outcomes.len(), 1);
  assert_eq!(outcomes[0].0, "loopback-1");
  assert!(matches!(outcomes[0].1, DeliveryOutcome::Sent), "the relay accepted the message: {:?}", outcomes[0].1);
  // The relay hands the payload over as soon as it accepts the message.
  let received = relay_rx.recv_timeout(Duration::from_secs(30)).expect("the relay received the message");
  assert!(received.contains("Subject: Loopback delivery"), "subject transmitted: {received}");
  assert!(received.contains("This sentence must arrive intact."), "body transmitted: {received}");
  assert!(received.contains("maya@northstar.test"), "recipient transmitted: {received}");
}


/// The search box is not an FTS5 query language: punctuation, quotes and the
/// bare AND/OR/NOT/NEAR keywords must be searched as data. A stray quote or a
/// pasted address used to fail the whole search with a syntax error that the
/// palette reported as "search could not be completed" (BUG-010).
#[test]
fn search_treats_operator_characters_as_data() {
  let (database, directory) = temp_database();
  let repos = Repositories::new(database.connection());
  // Ordinary terms keep working, including the prefix match for one word.
  assert!(!repos.search("planning").expect("plain term").is_empty(), "a normal word still finds seeded mail");
  assert!(!repos.search("plan").expect("short prefix").is_empty(), "a single term keeps its prefix match");
  // Hostile input must always answer — with results or none, never an error.
  for hostile in ["a\"b", "(", ")", ":\"x", "*", "^^^", "-x", "NEAR/2", "AND", "OR", "NOT", "maya@northstar.test", "\"\"\"", ")(*:"] {
    let outcome = repos.search(hostile);
    assert!(outcome.is_ok(), "{hostile} must not fail the search: {:?}", outcome.err());
  }
  // Punctuation alone leaves nothing searchable: an empty result set, not a
  // failure. A blank/oversized query is still a validation error.
  assert!(repos.search("():").expect("punctuation only").is_empty());
  assert!(repos.search("   ").is_err());
  close_temp(database, directory);
}

/// The MATCH expression is built from the user's input, never from its syntax:
/// operators are stripped, terms are quoted (so keywords are words), and a lone
/// term keeps the prefix match that makes type-ahead useful.
#[test]
fn search_expressions_quote_and_strip_the_query() {
  use relay_lib::repositories::fts_match_expression;
  assert_eq!(fts_match_expression("maya"), "\"maya\"*");
  assert_eq!(fts_match_expression("maya chen"), "\"maya chen\"");
  assert_eq!(fts_match_expression("AND"), "\"AND\"*", "a keyword becomes a word");
  assert_eq!(fts_match_expression("a\"b"), "\"ab\"*");
  assert_eq!(fts_match_expression("NEAR/2"), "\"NEAR2\"*");
  assert_eq!(fts_match_expression("a.b@c.d"), "\"a.b@c.d\"*");
  assert_eq!(fts_match_expression("():"), "", "nothing searchable is left");
  assert_eq!(fts_match_expression("  "), "");
}

