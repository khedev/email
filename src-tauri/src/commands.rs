use serde::Serialize;
use std::{fs, path::Path, sync::atomic::Ordering};
use tauri::{Manager, State};
use tauri_plugin_notification::NotificationExt;
use crate::{discover, error::AppError, guard, mailbox, models::{Account, AccountConnection, Channel, ChatMessage, Contact, Conversation, CreateAccount, CreateContact, DeliveryFailure, DeliveryReport, Draft, DraftInput, EmailDetail, EmailSummary, MessageReaction, Notification, NotificationPreference, OutboundMessage, Presence, ReactionSummary, SearchResult, SendMessageInput, SettingsEntry, StorageUsage, SyncQueueItem, UpdateNotificationPreference}, oauth, repositories::Repositories, security::{credential_ref, delete_oauth_client_secret, delete_oauth_secret, load_oauth_client_secret, load_oauth_secret, oauth_credential_ref, save_oauth_client_secret, save_oauth_secret, CredentialStore, OsKeyring}, transport, verify::{verify_imap_login, MailCredential, verify_xoauth2}, AppState};
use crate::sync::{DisabledTransport, SyncEngine, SyncOverview};

fn repositories<'state>(state: &'state State<'state, AppState>) -> Result<std::sync::MutexGuard<'state, crate::database::Database>, String> { state.database.lock().map_err(|_| "Application state is unavailable".to_string()) }

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppHealth { state: &'static str, last_sync_at: Option<String> }
/// Health of the local store: whether queued work has failed, and when mail last
/// synced successfully.
///
/// Deliberately **not** a connectivity check. Only the webview knows whether
/// this computer has a network (`navigator.onLine`), and the shell reports that
/// separately; the old hard-coded `offline` here is what made the header claim
/// the machine was offline on every launch, network or not (BUG-015).
#[tauri::command]
pub fn get_app_health(state: State<'_, AppState>) -> Result<AppHealth, String> {
  let database = repositories(&state)?;
  let failed: i64 = database.connection().query_row("SELECT COUNT(*) FROM sync_queue WHERE status = 'failed'", [], |row| row.get(0)).unwrap_or(0);
  let last_sync_at = Repositories::new(database.connection()).sync_metadata("last_successful_sync").unwrap_or(None);
  Ok(AppHealth { state: if failed > 0 { "error" } else { "online" }, last_sync_at })
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseInfo { schema_version: i64, location: String }
#[tauri::command]
pub fn get_database_info(state: State<'_, AppState>) -> Result<DatabaseInfo, String> {
  let database = repositories(&state)?;
  Ok(DatabaseInfo { schema_version: database.schema_version().map_err(|_| "Unable to read local database".to_string())?, location: database.path().display().to_string() })
}
#[tauri::command]
pub fn get_inbox(state: State<'_, AppState>) -> Result<Vec<EmailSummary>, String> { Repositories::new(repositories(&state)?.connection()).inbox().map_err(|_| "Unable to load local inbox".to_string()) }
#[tauri::command]
pub fn get_email(id: String, state: State<'_, AppState>) -> Result<Option<EmailDetail>, String> { Repositories::new(repositories(&state)?.connection()).email(&id).map_err(|_| "Unable to load email".to_string()) }
#[tauri::command]
pub fn get_email_thread(id: String, state: State<'_, AppState>) -> Result<Vec<EmailSummary>, String> { Repositories::new(repositories(&state)?.connection()).email_thread(&id).map_err(|_| "Unable to load the conversation".to_string()) }
#[tauri::command]
pub fn mark_email_read(id: String, is_read: bool, state: State<'_, AppState>) -> Result<(), String> { Repositories::new(repositories(&state)?.connection()).mark_email_read(&id, is_read).map_err(|_| "Unable to update email".to_string()) }
#[tauri::command]
pub fn get_accounts(state: State<'_, AppState>) -> Result<Vec<Account>, String> { Repositories::new(repositories(&state)?.connection()).accounts().map_err(|_| "Unable to load accounts".to_string()) }
/// Signs an email account in: verifies the password against the IMAP server,
/// stores the secret in the OS credential manager, and only then persists the
/// account. A failed verification stores nothing at all. Signing in again with
/// an address that already exists rotates its stored credential instead of
/// failing — the flow is idempotent and never destroys the stored secret.
/// Persists a verified password account: rotates the stored secret and
/// refreshes the connection state when the address already exists (idempotent
/// re-authentication), otherwise inserts the row. A persistence failure rolls
/// the keyring entry back so no orphaned credentials remain.
fn store_password_account(repos: &Repositories, input: &CreateAccount, password: &str) -> Result<Account, String> {
  OsKeyring.save(&input.email_address, password).map_err(|_| "The OS credential store refused to save the password.".to_string())?;
  if let Ok(Some(existing)) = repos.account_by_email(&input.email_address) {
    return repos.set_account_connection_state(&existing.id, "connected", None, true).map_err(|_| "Unable to refresh the signed-in account.".to_string());
  }
  match repos.add_account_verified(input, &credential_ref(&input.email_address)) {
    Ok(account) => Ok(account),
    Err(_) => { let _ = OsKeyring.delete(&input.email_address); Err("The signed-in account could not be saved locally.".to_string()) }
  }
}

#[tauri::command]
pub fn add_email_account(input: CreateAccount, password: String, state: State<'_, AppState>) -> Result<Account, String> {
  if password.trim().is_empty() { return Err("Enter the account password to sign in.".to_string()); }
  // Talks to the IMAP server, so it runs on the IPC thread; keep a panic from
  // taking the whole app down with it.
  guard::guarded(move || add_email_account_inner(input, password, state))
}
fn add_email_account_inner(input: CreateAccount, password: String, state: State<'_, AppState>) -> Result<Account, String> {
  verify_imap_login(&input.email_address, &password, &input.imap_host, input.imap_port, &input.encryption).map_err(|failure| failure.user_message())?;
  let database = repositories(&state)?;
  let repos = Repositories::new(database.connection());
  store_password_account(&repos, &input, &password)
}

/// Thunderbird-style sign-in: discovers the IMAP configuration for the address
/// (built-in provider database → DNS MX hints → autoconfig XML → hostname
/// guesses) and verifies the password against each candidate until one accepts.
/// Progress is streamed to the UI as `auto-sign-in-progress` events, and
/// persistence reuses the idempotent password-account flow.
///
/// The discovery half (DNS + one IMAP handshake per candidate) runs on a worker
/// thread: it takes seconds, and commands are invoked from the webview2 IPC
/// callback on the UI thread, so blocking there would freeze the window (and a
/// panic there aborts the process — see `guard`).
#[tauri::command]
pub async fn auto_sign_in(email: String, password: String, app: tauri::AppHandle, state: State<'_, AppState>) -> Result<Account, String> {
  use tauri::Emitter;
  let address = email.trim().to_string();
  if !address.contains('@') || address.len() > 320 { return Err("Enter a full email address, like name@example.com.".to_string()); }
  if password.trim().is_empty() { return Err("Enter the account password to sign in.".to_string()); }
  let progress = |text: &str| { let _ = app.emit("auto-sign-in-progress", text); };
  progress("Looking up mail server settings…");
  // A plain OS thread rather than `tauri::async_runtime::spawn_blocking`:
  // blocking-pool threads enter the async runtime context, and the resolver's
  // own `Runtime::block_on` would then fail with "Cannot start a runtime from
  // within a runtime".
  let (sender, receiver) = tokio::sync::oneshot::channel();
  let (worker_address, worker_password, worker_app) = (address.clone(), password.clone(), app.clone());
  std::thread::Builder::new()
    .name("relay-discovery".into())
    .spawn(move || {
      let outcome = guard::guarded(|| {
        discover::discover_and_verify(&worker_address, &worker_password, |text| { let _ = worker_app.emit("auto-sign-in-progress", text); })
      });
      let _ = sender.send(outcome);
    })
    .map_err(|_| "Mail server discovery could not be started.".to_string())?;
  let discovered = receiver.await.map_err(|_| "Mail server discovery stopped unexpectedly.".to_string())??;
  progress("Password accepted — saving the account…");
  // The state lock is only taken here, after the await: nothing borrowed from
  // `state` is held across it. Persistence is guarded too, so a panic in the
  // keyring or SQLite path also fails the command instead of aborting the app.
  guard::guarded(|| {
    let database = repositories(&state)?;
    let repos = Repositories::new(database.connection());
    let input = CreateAccount {
      display_name: address.split('@').next().unwrap_or("account").to_string(),
      email_address: address.clone(),
      imap_host: discovered.candidate.imap_host.clone(),
      imap_port: discovered.candidate.imap_port,
      smtp_host: discovered.candidate.smtp_host.clone(),
      smtp_port: discovered.candidate.smtp_port,
      encryption: discovered.candidate.encryption.to_string(),
      auth_kind: "password".into(),
    };
    store_password_account(&repos, &input, &password)
  })
}
/// Re-verifies a stored account against its IMAP server. With a password the
/// stored credential is rotated after a successful login; without one the
/// password is read back from the OS credential manager. Google OAuth accounts
/// are re-verified by refreshing the stored authorization and using XOAUTH2.
#[tauri::command]
pub fn test_email_connection(id: String, password: Option<String>, state: State<'_, AppState>) -> Result<Account, String> {
  // Re-verification contacts IMAP/XOAUTH2 from the IPC thread; a panic here must
  // fail the action, not abort the app.
  guard::guarded(move || test_email_connection_inner(id, password, state))
}
fn test_email_connection_inner(id: String, password: Option<String>, state: State<'_, AppState>) -> Result<Account, String> {
  // Snapshot the account's settings under the lock, then release it. Every
  // command shares this one mutex, so holding it across the network calls below
  // would block all 49 IPC commands and freeze the window (BUG-004 / SEC-003).
  let connection: AccountConnection = {
    let database = repositories(&state)?;
    Repositories::new(database.connection()).account_connection(&id).map_err(|_| "Unable to load the account.".to_string())?.ok_or_else(|| "This account no longer exists.".to_string())?
  };
  // Network phase — no lock held. `reason` is the short, non-sensitive text
  // recorded on the account row; `verified` is the user-facing outcome.
  let (verified, reason): (Result<(), String>, Option<String>) = if connection.auth_kind == "oauth2" {
    let stored = load_oauth_secret(&connection.email_address).map_err(|_| "The OS credential store is unavailable.".to_string())?.ok_or_else(|| "No Google authorization is stored for this account. Sign in again with Google.".to_string())?;
    // A "Web application" client needs its secret for the refresh; a "Desktop
    // app" client has none. A keyring read failure is not fatal here — the
    // refresh simply runs without a secret and Google reports the real verdict.
    let client_secret = load_oauth_client_secret().unwrap_or(None);
    match oauth::refresh_access_token(&stored.0, client_secret.as_deref(), &stored.1) {
      Ok(tokens) => {
        if let Some(new_refresh) = tokens.refresh_token { let _ = save_oauth_secret(&connection.email_address, &stored.0, &new_refresh); }
        match verify_xoauth2(&connection.email_address, &tokens.access_token, &connection.imap_host, connection.imap_port, &connection.encryption) {
          Ok(()) => (Ok(()), None),
          Err(failure) => (Err(failure.user_message()), Some(failure.detail())),
        }
      }
      Err(error) => (Err(error.user_message()), Some("Google authorization refresh failed".to_string())),
    }
  } else {
    let rotated = password.as_deref().filter(|value| !value.trim().is_empty());
    let secret = match rotated {
      Some(value) => value.to_owned(),
      None => OsKeyring.load(&connection.email_address).map_err(|_| "The OS credential store is unavailable.".to_string())?.ok_or_else(|| "No password is stored for this account. Sign in again with a password.".to_string())?,
    };
    match verify_imap_login(&connection.email_address, &secret, &connection.imap_host, connection.imap_port, &connection.encryption) {
      Ok(()) => {
        if let Some(value) = rotated {
          OsKeyring.save(&connection.email_address, value).map_err(|_| "The OS credential store refused the password update.".to_string())?;
        }
        (Ok(()), None)
      }
      Err(failure) => (Err(failure.user_message()), Some(failure.detail())),
    }
  };
  // Recording phase — a short, separate lock section.
  let database = repositories(&state)?;
  let repos = Repositories::new(database.connection());
  match verified {
    Ok(()) => repos.set_account_connection_state(&id, "connected", None, true).map_err(|_| "Unable to record the connection state.".to_string()),
    Err(message) => {
      let _ = repos.set_account_connection_state(&id, "error", reason.as_deref(), false);
      Err(message)
    }
  }
}
/// Google OAuth 2.0 sign-in for Gmail (installed-app flow, RFC 8252). Opens the
/// system browser, waits (bounded) for the loopback redirect, exchanges the
/// code with PKCE S256, proves IMAP access with XOAUTH2, stores the refresh
/// token in the OS credential manager, and persists the account (idempotent
/// for known addresses). Requires the user's own Google Cloud OAuth client
/// (authorized redirect URI http://127.0.0.1): a "Web application" client also
/// needs its client secret, a "Desktop app" client does not.
///
/// The browser half runs on a worker thread (same reasoning as `auto_sign_in`):
/// commands are invoked from the webview2 IPC callback on the UI thread, and
/// this one waits for a *human* — up to `oauth::AUTH_TIMEOUT`. Running it inline
/// froze the window for the whole round-trip. The database lock is taken only in
/// short sections, never across the wait.
#[tauri::command]
pub async fn google_oauth_sign_in(client_id: String, client_secret: Option<String>, app: tauri::AppHandle, state: State<'_, AppState>) -> Result<Account, String> {
  let mut client_id = client_id.trim().to_string();
  if client_id.is_empty() {
    // Remembered from a previous Google sign-in (kept in the settings table so
    // the id is only ever entered once, Thunderbird-style). Scoped so the guard
    // is released before the browser wait below.
    {
      let database = repositories(&state)?;
      client_id = Repositories::new(database.connection())
        .all_settings()
        .map_err(|_| "Unable to read the local settings.".to_string())?
        .into_iter()
        .find(|entry| entry.key == oauth::OAUTH_CLIENT_ID_SETTING)
        .map(|entry| entry.value)
        .unwrap_or_default();
    }
  }
  // Validated here as well as in the UI so an unusable paste fails before a
  // browser round-trip is started; a message never echoes the value back.
  if let Some(issue) = oauth::validate_client_id(&client_id) { return Err(issue.to_string()); }
  // The secret is optional (PKCE/desktop clients have none). An entered secret
  // is used and remembered in the OS credential manager — never in the
  // settings table, which every settings command can read; otherwise the one
  // stored by an earlier sign-in is reused.
  let provided_secret = client_secret.unwrap_or_default().trim().to_string();
  if !provided_secret.is_empty() {
    if let Some(issue) = oauth::validate_client_secret(&provided_secret) { return Err(issue.to_string()); }
  }
  let effective_secret = if provided_secret.is_empty() { load_oauth_client_secret().unwrap_or(None) } else { Some(provided_secret.clone()) };
  // A fresh flag per attempt, so a cancel meant for an earlier round-trip cannot
  // abort this one.
  state.oauth_cancel.store(false, Ordering::Relaxed);
  let cancelled = state.oauth_cancel.clone();
  let (sender, receiver) = tokio::sync::oneshot::channel();
  let (worker_client_id, worker_app) = (client_id.clone(), app.clone());
  std::thread::Builder::new()
    .name("relay-oauth".into())
    .spawn(move || {
      // Guarded on the worker too: this is the network half, and a panic here
      // would otherwise take the process down from a non-IPC thread.
      let outcome = guard::guarded(|| authorize_with_google(&worker_client_id, effective_secret.as_deref(), &worker_app, &cancelled));
      let _ = sender.send(outcome);
    })
    .map_err(|_| "Google sign-in could not be started.".to_string())?;
  let grant = receiver.await.map_err(|_| "Google sign-in stopped unexpectedly.".to_string())??;
  // Persist only what Google actually granted: the refresh token in the OS
  // credential manager, then the client id and the account row.
  save_oauth_secret(&grant.address, &client_id, &grant.refresh_token).map_err(|_| "The OS credential store refused to save the Google authorization.".to_string())?;
  // The secret is remembered only now that Google accepted the exchange, so a
  // rejected secret is never kept. An empty field leaves any stored secret
  // untouched (it may still belong to this client).
  let secret_saved = if provided_secret.is_empty() { false } else { save_oauth_client_secret(&provided_secret).is_ok() };
  let database = repositories(&state)?;
  let repos = Repositories::new(database.connection());
  let _ = repos.set_setting(oauth::OAUTH_CLIENT_ID_SETTING, &client_id);
  let input = CreateAccount {
    display_name: grant.address.split('@').next().unwrap_or("Google account").to_string(),
    email_address: grant.address.clone(),
    imap_host: "imap.gmail.com".into(),
    imap_port: 993,
    smtp_host: "smtp.gmail.com".into(),
    smtp_port: 465,
    encryption: "ssl".into(),
    auth_kind: "oauth2".into(),
  };
  if let Ok(Some(existing)) = repos.account_by_email(&grant.address) {
    // Re-authorization: the refresh token is already rotated above; refresh
    // the connection state on the existing row instead of failing.
    return repos.set_account_connection_state(&existing.id, "connected", None, true).map_err(|_| "Unable to refresh the signed-in account.".to_string());
  }
  match repos.add_account_verified(&input, &oauth_credential_ref(&grant.address)) {
    Ok(account) => Ok(account),
    Err(_) => {
      // The account row is the durable state; without it the stored grant and a
      // freshly entered secret would be orphaned, so both are rolled back.
      let _ = delete_oauth_secret(&grant.address);
      if secret_saved { let _ = delete_oauth_client_secret(); }
      Err("The Google-signed-in account could not be saved locally.".to_string())
    }
  }
}

/// What the browser half produced, before anything is stored.
struct GoogleGrant { address: String, refresh_token: String }

/// The browser half of the Google sign-in: bind the loopback listener, open the
/// system browser, wait for the redirect (a code, a refusal, or cancellation),
/// exchange the code with PKCE S256, and prove the grant unlocks IMAP with
/// XOAUTH2. Pure network work — no database lock is taken here, so the rest of
/// the app stays responsive while the user is in the browser.
fn authorize_with_google(client_id: &str, client_secret: Option<&str>, app: &tauri::AppHandle, cancelled: &std::sync::atomic::AtomicBool) -> Result<GoogleGrant, String> {
  use tauri_plugin_opener::OpenerExt;
  let state_parameter = oauth::random_state();
  let (verifier, challenge) = oauth::pkce_pair();
  let redirect = oauth::LoopbackRedirect::bind(state_parameter.clone()).map_err(|error| error.user_message())?;
  let url = oauth::authorization_url(client_id, &redirect.redirect_uri(), &challenge, &state_parameter);
  app.opener().open_url(url, None::<&str>).map_err(|_| "Unable to open the browser for Google authorization.".to_string())?;
  let code = redirect.wait_for_code(oauth::AUTH_TIMEOUT, cancelled).map_err(|error| error.user_message())?;
  let tokens = oauth::exchange_code(client_id, client_secret, &code, &verifier, &redirect.redirect_uri()).map_err(|error| error.user_message())?;
  let address = tokens.id_token.as_deref().and_then(oauth::email_from_id_token).ok_or_else(|| "Google did not return the account address. Start the sign-in again.".to_string())?;
  // Prove the grant actually unlocks IMAP before anything is stored.
  verify_xoauth2(&address, &tokens.access_token, "imap.gmail.com", 993, "ssl").map_err(|failure| failure.user_message())?;
  let refresh_token = tokens.refresh_token.clone().ok_or_else(|| "Google did not issue a refresh token. Remove Relay's access at myaccount.google.com/permissions and sign in again.".to_string())?;
  Ok(GoogleGrant { address, refresh_token })
}

/// Downloads the newest mail from one or more accounts' INBOX into the local
/// store and returns how many messages were written.
///
/// `account_ids` of `None` (what the UI sends) means **every** signed-in
/// account. That matters: the caller used to pass a single id it had picked as
/// `accounts[0]`, and the account list is ordered by display name — so a seeded
/// demo account, or simply an alphabetically earlier one, could be synced
/// instead of the mailbox the user was looking at.
///
/// Runs on a worker thread for the same reason as `auto_sign_in` and
/// `google_oauth_sign_in`: it holds a network conversation from the webview2 IPC
/// callback, which would freeze the window. The database lock is taken only to
/// snapshot the accounts and to write the results — never across a fetch.
/// Progress is streamed to the UI as `mail-sync-progress` events.
#[tauri::command]
pub async fn sync_mail(account_ids: Option<Vec<String>>, app: tauri::AppHandle, state: State<'_, AppState>) -> Result<i64, String> {
  use tauri::Emitter;
  // Snapshot the work list (and each account's connection details) under one
  // short lock, then release it before any network call.
  let connections: Vec<AccountConnection> = {
    let database = repositories(&state)?;
    let repos = Repositories::new(database.connection());
    let ids = match account_ids {
      Some(ids) => ids,
      None => repos.account_ids().map_err(|_| "Unable to list the signed-in accounts.".to_string())?,
    };
    let mut found = Vec::with_capacity(ids.len());
    for id in ids {
      if let Ok(Some(connection)) = repos.account_connection(&id) { found.push(connection); }
    }
    found
  };
  if connections.is_empty() {
    return Err("No email account is signed in, so there is nothing to fetch.".to_string());
  }
  let mut total = 0i64;
  let mut failures: Vec<String> = Vec::new();
  for connection in connections {
    let label = connection.email_address.clone();
    // One account's failure must not hide the others' mail, so each is tried and
    // reported separately.
    let outcome = match load_mail_credential(&connection) {
      Ok(credential_owned) => {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let (worker_connection, worker_app, worker_label) = (connection.clone(), app.clone(), label.clone());
        std::thread::Builder::new()
          .name("relay-mail-sync".into())
          .spawn(move || {
            let outcome = guard::guarded(|| {
              let credential = match &credential_owned {
                OwnedCredential::Password(password) => MailCredential::Password(password),
                OwnedCredential::AccessToken(token) => MailCredential::AccessToken(token),
              };
              let progress = |text: &str| { let _ = worker_app.emit("mail-sync-progress", format!("{worker_label}: {text}")); };
              mailbox::fetch_inbox(&worker_connection, credential, mailbox::FETCH_CAP, progress)
            });
            let _ = sender.send(outcome);
          })
          .map_err(|_| "Mail sync could not be started.".to_string())?;
        receiver.await.map_err(|_| "Mail sync stopped unexpectedly.".to_string())?
      }
      Err(message) => Err(message),
    };
    let fetched = match outcome {
      Ok(fetched) => fetched,
      Err(message) => { failures.push(format!("{label}: {message}")); continue; }
    };
    // Persist under a short, separate lock. `ensure_account_folders` repairs
    // accounts created before folders were scaffolded, so a mailbox signed in
    // earlier starts working here rather than staying invisible.
    let _ = app.emit("mail-sync-progress", format!("{label}: saving mail locally…"));
    let database = repositories(&state)?;
    let repos = Repositories::new(database.connection());
    repos.ensure_account_folders(&connection.id).map_err(|_| "The account folders could not be prepared.".to_string())?;
    if let Some(validity) = fetched.uid_validity { let _ = repos.set_sync_metadata(&format!("imap.{}.uidvalidity", connection.id), &validity.to_string()); }
    if let Some(next) = fetched.uid_next { let _ = repos.set_sync_metadata(&format!("imap.{}.uidnext", connection.id), &next.to_string()); }
    match repos.upsert_fetched_messages(&connection.id, "inbox", &fetched.messages) {
      Ok(written) => total += written,
      Err(_) => failures.push(format!("{label}: the downloaded mail could not be saved locally")),
    }
  }
  let database = repositories(&state)?;
  let _ = Repositories::new(database.connection()).set_sync_metadata("last_successful_sync", &chrono::Utc::now().to_rfc3339());
  // Every account failed: report the first reason rather than a bare "0 fetched".
  if total == 0 && !failures.is_empty() { return Err(failures.join(" ")); }
  Ok(total)
}

/// Downloads and stores the body of a single message.
///
/// This is the repair path for a message that is stored with an empty body:
/// every row a sync wrote before the body decoded correctly, and any row older
/// than the newest-`mailbox::FETCH_CAP` window a sync covers. Runs on a worker
/// thread for the same reason as `sync_mail` — it holds a network conversation
/// from the webview2 IPC callback — and takes the database lock only to snapshot
/// the work and to write the result, never across the fetch.
#[tauri::command]
pub async fn fetch_email_body(id: String, state: State<'_, AppState>) -> Result<String, String> {
  // Snapshot everything the fetch needs under one short lock.
  let (connection, uid) = {
    let database = repositories(&state)?;
    let repos = Repositories::new(database.connection());
    let Some((account_id, uid)) = repos.email_fetch_target(&id).map_err(|_| "Unable to load the message.".to_string())? else {
      return Err("This message has no copy on the mail server to download from.".to_string());
    };
    let connection = repos.account_connection(&account_id).map_err(|_| "Unable to load the account.".to_string())?
      .ok_or_else(|| "The account this message belongs to is no longer signed in.".to_string())?;
    (connection, uid)
  };
  let credential_owned = load_mail_credential(&connection)?;
  let (sender, receiver) = tokio::sync::oneshot::channel();
  std::thread::Builder::new()
    .name("relay-mail-body".into())
    .spawn(move || {
      let outcome = guard::guarded(|| {
        let credential = match &credential_owned {
          OwnedCredential::Password(password) => MailCredential::Password(password),
          OwnedCredential::AccessToken(token) => MailCredential::AccessToken(token),
        };
        mailbox::fetch_message_body(&connection, credential, uid)
      });
      let _ = sender.send(outcome);
    })
    .map_err(|_| "The message download could not be started.".to_string())?;
  let body = receiver.await.map_err(|_| "The message download stopped unexpectedly.".to_string())??;
  // Persist under a short, separate lock. An empty body is deliberately not
  // written, so the reader can tell "nothing to show" from "not tried yet".
  if !body.is_empty() {
    let database = repositories(&state)?;
    Repositories::new(database.connection()).store_email_body(&id, &body).map_err(|_| "The downloaded message could not be saved locally.".to_string())?;
  }
  Ok(body)
}

/// A credential read out of the OS store on the main thread, so the fetch worker
/// never has to touch application state.
enum OwnedCredential { Password(String), AccessToken(String) }

/// Resolves the credential for an account. OAuth accounts refresh their stored
/// authorization here (the same client-id/secret pair the sign-in used); password
/// accounts read the keyring entry that may hold an App Password.
fn load_mail_credential(connection: &AccountConnection) -> Result<OwnedCredential, String> {
  if connection.auth_kind == "oauth2" {
    let stored = load_oauth_secret(&connection.email_address).map_err(|_| "The OS credential store is unavailable.".to_string())?.ok_or_else(|| "No Google authorization is stored for this account. Sign in again with Google.".to_string())?;
    let client_secret = load_oauth_client_secret().unwrap_or(None);
    let tokens = oauth::refresh_access_token(&stored.0, client_secret.as_deref(), &stored.1).map_err(|error| error.user_message())?;
    if let Some(rotated) = tokens.refresh_token { let _ = save_oauth_secret(&connection.email_address, &stored.0, &rotated); }
    Ok(OwnedCredential::AccessToken(tokens.access_token))
  } else {
    let password = OsKeyring.load(&connection.email_address).map_err(|_| "The OS credential store is unavailable.".to_string())?.ok_or_else(|| "No password is stored for this account. Sign in again.".to_string())?;
    Ok(OwnedCredential::Password(password))
  }
}

/// Cancels an in-flight Google sign-in — the browser tab was closed, or the user
/// changed their mind. The waiting command returns at once instead of sitting out
/// the remaining timeout.
#[tauri::command]
pub fn cancel_google_sign_in(state: State<'_, AppState>) -> Result<(), String> {
  state.oauth_cancel.store(true, Ordering::Relaxed);
  Ok(())
}

/// Forgets the remembered Google OAuth client configuration: the client id row
/// in the settings table and the optional client secret in the OS credential
/// manager. Refresh tokens stored for signed-in accounts are left alone — use
/// remove_account for those.
#[tauri::command]
pub fn forget_oauth_client_id(state: State<'_, AppState>) -> Result<(), String> {
  guard::guarded(move || forget_oauth_client_id_inner(state))
}
fn forget_oauth_client_id_inner(state: State<'_, AppState>) -> Result<(), String> {
  let database = repositories(&state)?;
  Repositories::new(database.connection())
    .delete_setting(oauth::OAUTH_CLIENT_ID_SETTING)
    .map_err(|_| "Unable to clear the remembered Google client id.".to_string())?;
  delete_oauth_client_secret().map_err(|_| "The OS credential store refused to remove the Google client secret.".to_string())
}

#[tauri::command]
pub fn get_contacts(state: State<'_, AppState>) -> Result<Vec<Contact>, String> { Repositories::new(repositories(&state)?.connection()).contacts().map_err(|_| "Unable to load contacts".to_string()) }
#[tauri::command]
pub fn create_contact(input: CreateContact, state: State<'_, AppState>) -> Result<Contact, String> { Repositories::new(repositories(&state)?.connection()).create_contact(input).map_err(|_| "Unable to save contact".to_string()) }
#[tauri::command]
pub fn get_pending_sync_items(state: State<'_, AppState>) -> Result<Vec<SyncQueueItem>, String> { Repositories::new(repositories(&state)?.connection()).pending_sync_items().map_err(|_| "Unable to load sync queue".to_string()) }
#[tauri::command]
pub fn save_draft(input: DraftInput, draft_id: Option<String>, state: State<'_, AppState>) -> Result<Draft, String> { Repositories::new(repositories(&state)?.connection()).save_draft(input, draft_id.as_deref()).map_err(|_| "Unable to save draft".to_string()) }
/// Queueing a send is local-only today: it files the outbound copy into Sent and
/// records a durable `send_smtp` item that no transport consumes yet.
/// `AppError::Validation` is the repository's own "not outbound, or no To
/// recipient" answer, so it is the only failure that may carry the recipient
/// hint — a storage failure reported as a recipient problem misdirected the user
/// (BUG-023).
#[tauri::command]
pub fn queue_email_send(id: String, state: State<'_, AppState>) -> Result<Draft, String> {
  Repositories::new(repositories(&state)?.connection()).queue_send(&id).map_err(|error| match error {
    AppError::Validation => "Add at least one valid recipient before sending.".to_string(),
    AppError::Database(_) | AppError::Io(_) | AppError::SecureStore(_) => "The message could not be saved locally, so nothing was queued.".to_string(),
  })
}
/// What the UI is told about one message after a delivery run: enough to refresh
/// the message it is showing without refetching the whole folder.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeliveryEvent { id: String, state: String, error: Option<String> }

/// Transmits every message waiting in the local send queue.
///
/// The composer files a send locally first (`queue_email_send`), and then this
/// command is what actually reaches the server. Keeping the two apart matters: a
/// crash, a closed window or a dead network between them leaves a durable
/// `send_smtp` item behind, so the message is retryable instead of lost, and the
/// `completed` mark that delivery writes is what stops a message being sent
/// twice. `ids` narrows the run to specific messages (the reading pane's Try
/// again); `None` drains the whole queue, which is what the shell asks for on
/// startup so a send stranded by a quit recovers on its own.
///
/// Runs on a worker thread for the same reason as `sync_mail`: an SMTP session
/// is a network conversation held from the webview2 IPC callback, which would
/// freeze the window. The database lock is taken only to snapshot the work and
/// to write the outcomes — never across a session. Each outcome is streamed to
/// the UI as a `mail-delivery-result` event.
#[tauri::command]
pub async fn deliver_queued_mail(ids: Option<Vec<String>>, app: tauri::AppHandle, state: State<'_, AppState>) -> Result<DeliveryReport, String> {
  use tauri::Emitter;
  // Snapshot the queue under one short lock: every message grouped with the
  // account it leaves through. A message whose account row is gone is set aside
  // and reported rather than dropped, so it cannot sit in the queue forever.
  let (accounts, orphans): (Vec<(AccountConnection, Vec<OutboundMessage>)>, Vec<String>) = {
    let database = repositories(&state)?;
    let repos = Repositories::new(database.connection());
    let sends = repos.queued_sends(ids.as_deref()).map_err(|_| "Unable to read the messages waiting to be sent.".to_string())?;
    let mut accounts: Vec<(AccountConnection, Vec<OutboundMessage>)> = Vec::new();
    let mut orphans: Vec<String> = Vec::new();
    for send in sends {
      match send.connection {
        Some(connection) => match accounts.iter_mut().find(|(existing, _)| existing.id == connection.id) {
          Some((_, messages)) => messages.push(send.message),
          None => accounts.push((connection, vec![send.message])),
        },
        None => orphans.push(send.message.id),
      }
    }
    (accounts, orphans)
  };
  let mut report = DeliveryReport::default();
  for id in orphans {
    let error = "The account this message was queued from is no longer signed in, so it could not be sent.".to_string();
    if let Ok(database) = repositories(&state) {
      let _ = Repositories::new(database.connection()).mark_delivery_failed(&id, &error);
    }
    let _ = app.emit("mail-delivery-result", DeliveryEvent { id: id.clone(), state: "failed".into(), error: Some(error.clone()) });
    report.failed.push(DeliveryFailure { id, error });
  }
  // Credentials are resolved here, on the IPC thread: the keyring read and any
  // OAuth refresh stay off the worker, which only receives the usable secret.
  let mut prepared: Vec<(AccountConnection, Vec<OutboundMessage>, OwnedCredential)> = Vec::new();
  for (connection, messages) in accounts {
    match load_mail_credential(&connection) {
      Ok(credential) => prepared.push((connection, messages, credential)),
      Err(error) => {
        if let Ok(database) = repositories(&state) {
          let repos = Repositories::new(database.connection());
          for message in &messages {
            let _ = repos.mark_delivery_failed(&message.id, &error);
          }
        }
        for message in messages {
          let _ = app.emit("mail-delivery-result", DeliveryEvent { id: message.id.clone(), state: "failed".into(), error: Some(error.clone()) });
          report.failed.push(DeliveryFailure { id: message.id, error: error.clone() });
        }
      }
    }
  }
  if prepared.is_empty() {
    return Ok(report);
  }
  deliver_and_record(prepared, &app, &state, &mut report).await?;
  Ok(report)
}

/// Runs the SMTP session on a worker thread, then records each outcome.
///
/// Split from the command so the delivery is one auditable path: the worker
/// performs network I/O only with the snapshots it was handed (no database, no
/// keyring, no app handle), and every write happens back on the caller's thread
/// under a short lock.
async fn deliver_and_record(prepared: Vec<(AccountConnection, Vec<OutboundMessage>, OwnedCredential)>, app: &tauri::AppHandle, state: &State<'_, AppState>, report: &mut DeliveryReport) -> Result<(), String> {
  use tauri::Emitter;
  let (sender, receiver) = tokio::sync::oneshot::channel();
  std::thread::Builder::new()
    .name("relay-smtp-delivery".into())
    .spawn(move || {
      let outcome = guard::guarded(move || {
        let mut results = Vec::new();
        for (connection, messages, credential) in prepared {
          let credential = match &credential {
            OwnedCredential::Password(password) => MailCredential::Password(password),
            OwnedCredential::AccessToken(token) => MailCredential::AccessToken(token),
          };
          results.extend(transport::deliver(&connection, credential, &messages));
        }
        Ok(results)
      });
      let _ = sender.send(outcome);
    })
    .map_err(|_| "The send could not be started.".to_string())?;
  let outcomes = receiver.await.map_err(|_| "The send stopped unexpectedly.".to_string())??;
  // Persist under a short, separate lock, then tell the UI.
  let database = repositories(state)?;
  let repos = Repositories::new(database.connection());
  for (id, outcome) in outcomes {
    match outcome {
      transport::DeliveryOutcome::Sent => {
        // A delivered message whose local mark fails would be retried on the
        // next run, so the failure is worth a log line — but it is not a send
        // failure, and the report must not claim otherwise.
        if repos.mark_delivery_sent(&id).is_err() {
          tracing::warn!(message = %id, "delivered message could not be marked sent locally");
        }
        let _ = app.emit("mail-delivery-result", DeliveryEvent { id: id.clone(), state: "sent".into(), error: None });
        report.sent.push(id);
      }
      transport::DeliveryOutcome::Failed(error) => {
        let _ = repos.mark_delivery_failed(&id, &error);
        let _ = app.emit("mail-delivery-result", DeliveryEvent { id: id.clone(), state: "failed".into(), error: Some(error.clone()) });
        report.failed.push(DeliveryFailure { id, error });
      }
    }
  }
  Ok(())
}
#[tauri::command]
pub fn get_channels(state: State<'_, AppState>) -> Result<Vec<Channel>, String> { Repositories::new(repositories(&state)?.connection()).channels().map_err(|_| "Unable to load local channels".to_string()) }
#[tauri::command]
pub fn get_channel_messages(channel_id: String, state: State<'_, AppState>) -> Result<Vec<ChatMessage>, String> { Repositories::new(repositories(&state)?.connection()).channel_messages(&channel_id).map_err(|_| "Unable to load local messages".to_string()) }
#[tauri::command]
pub fn send_company_message(input: SendMessageInput, state: State<'_, AppState>) -> Result<ChatMessage, String> { Repositories::new(repositories(&state)?.connection()).send_message(input).map_err(|_| "Message could not be saved locally".to_string()) }
#[tauri::command]
pub fn get_conversations(state: State<'_, AppState>) -> Result<Vec<Conversation>, String> { Repositories::new(repositories(&state)?.connection()).conversations().map_err(|_| "Unable to load conversations".to_string()) }
#[tauri::command]
pub fn open_direct_message(contact_id: String, state: State<'_, AppState>) -> Result<Conversation, String> { Repositories::new(repositories(&state)?.connection()).open_direct_message(&contact_id).map_err(|_| "Unable to open this conversation".to_string()) }
#[tauri::command]
pub fn edit_message(message_id: String, body: String, state: State<'_, AppState>) -> Result<ChatMessage, String> { Repositories::new(repositories(&state)?.connection()).edit_message(&message_id, &body).map_err(|_| "Message could not be edited".to_string()) }
#[tauri::command]
pub fn delete_message(message_id: String, state: State<'_, AppState>) -> Result<(), String> { Repositories::new(repositories(&state)?.connection()).delete_message(&message_id).map_err(|_| "Message could not be deleted".to_string()) }
#[tauri::command]
pub fn set_message_pin(message_id: String, pinned: bool, state: State<'_, AppState>) -> Result<(), String> { Repositories::new(repositories(&state)?.connection()).set_message_pin(&message_id, pinned).map_err(|_| "Unable to update the message".to_string()) }
#[tauri::command]
pub fn get_sync_overview(state: State<'_, AppState>) -> Result<SyncOverview, String> { SyncEngine::new(repositories(&state)?.connection(), DisabledTransport).overview().map_err(|_| "Unable to read sync status".to_string()) }
#[tauri::command]
pub fn retry_failed_sync(state: State<'_, AppState>) -> Result<i64, String> { SyncEngine::new(repositories(&state)?.connection(), DisabledTransport).retry_failed().map_err(|_| "Unable to retry failed changes".to_string()) }
#[tauri::command]
pub fn get_notifications(state: State<'_, AppState>) -> Result<Vec<Notification>, String> { Repositories::new(repositories(&state)?.connection()).notifications().map_err(|_| "Unable to load notifications".to_string()) }
#[tauri::command]
pub fn get_unread_notification_count(state: State<'_, AppState>) -> Result<i64, String> { Repositories::new(repositories(&state)?.connection()).unread_notification_count().map_err(|_| "Unable to load notification count".to_string()) }
#[tauri::command]
pub fn mark_all_notifications_read(state: State<'_, AppState>) -> Result<(), String> { Repositories::new(repositories(&state)?.connection()).mark_notifications_read().map_err(|_| "Unable to update notifications".to_string()) }
#[tauri::command]
pub fn get_notification_preferences(state: State<'_, AppState>) -> Result<Vec<NotificationPreference>, String> { Repositories::new(repositories(&state)?.connection()).notification_preferences().map_err(|_| "Unable to load notification preferences".to_string()) }
#[tauri::command]
pub fn update_notification_preference(input: UpdateNotificationPreference, state: State<'_, AppState>) -> Result<(), String> { Repositories::new(repositories(&state)?.connection()).update_notification_preference(input).map_err(|_| "Unable to update notification preference".to_string()) }
#[tauri::command]
pub fn show_native_notification(app: tauri::AppHandle, title: String, body: String) -> Result<(), String> {
  if title.trim().is_empty() || title.len() > 120 || body.len() > 500 { return Err("Invalid notification content".into()); }
  app.notification().builder().title(&title).body(&body).show().map_err(|_| "Unable to show native notification".to_string())
}
#[tauri::command]
pub fn get_emails_in_folder(role: String, state: State<'_, AppState>) -> Result<Vec<EmailSummary>, String> { Repositories::new(repositories(&state)?.connection()).emails_in_folder(&role).map_err(|_| "Unable to load local folder".to_string()) }
#[tauri::command]
pub fn get_starred(state: State<'_, AppState>) -> Result<Vec<EmailSummary>, String> { Repositories::new(repositories(&state)?.connection()).starred().map_err(|_| "Unable to load starred mail".to_string()) }
#[tauri::command]
pub fn set_email_star(id: String, is_starred: bool, state: State<'_, AppState>) -> Result<(), String> { Repositories::new(repositories(&state)?.connection()).set_email_star(&id, is_starred).map_err(|_| "Unable to update email".to_string()) }
#[tauri::command]
pub fn archive_email(id: String, state: State<'_, AppState>) -> Result<(), String> { Repositories::new(repositories(&state)?.connection()).archive_email(&id).map_err(|_| "Unable to archive email".to_string()) }
#[tauri::command]
pub fn trash_email(id: String, state: State<'_, AppState>) -> Result<(), String> { Repositories::new(repositories(&state)?.connection()).trash_email(&id).map_err(|_| "Unable to move email to trash".to_string()) }
#[tauri::command]
pub fn get_message_thread(root_id: String, state: State<'_, AppState>) -> Result<Vec<ChatMessage>, String> { Repositories::new(repositories(&state)?.connection()).message_thread(&root_id).map_err(|_| "Unable to load thread".to_string()) }
#[tauri::command]
pub fn get_channel_reactions(channel_id: String, state: State<'_, AppState>) -> Result<Vec<MessageReaction>, String> { Repositories::new(repositories(&state)?.connection()).channel_reactions(&channel_id).map_err(|_| "Unable to load reactions".to_string()) }
#[tauri::command]
pub fn toggle_message_reaction(message_id: String, emoji: String, state: State<'_, AppState>) -> Result<Vec<ReactionSummary>, String> { Repositories::new(repositories(&state)?.connection()).toggle_message_reaction(&message_id, &emoji).map_err(|_| "Invalid reaction".to_string()) }
#[tauri::command]
pub fn get_app_settings(state: State<'_, AppState>) -> Result<Vec<SettingsEntry>, String> { Repositories::new(repositories(&state)?.connection()).all_settings().map_err(|_| "Unable to load settings".to_string()) }
#[tauri::command]
pub fn set_app_setting(key: String, value: String, state: State<'_, AppState>) -> Result<(), String> { Repositories::new(repositories(&state)?.connection()).set_setting(&key, &value).map_err(|_| "Invalid setting".to_string()) }
#[tauri::command]
pub fn get_presence(state: State<'_, AppState>) -> Result<Presence, String> { Repositories::new(repositories(&state)?.connection()).presence().map_err(|_| "Unable to load presence".to_string()) }
#[tauri::command]
pub fn set_presence(status: String, state: State<'_, AppState>) -> Result<Presence, String> { Repositories::new(repositories(&state)?.connection()).set_presence(&status).map_err(|_| "Invalid presence status".to_string()) }
#[tauri::command]
pub fn search_global(query: String, state: State<'_, AppState>) -> Result<Vec<SearchResult>, String> { Repositories::new(repositories(&state)?.connection()).search(&query).map_err(|_| "Search could not be completed".to_string()) }
#[tauri::command]
pub fn set_contact_favorite(id: String, favorite: bool, state: State<'_, AppState>) -> Result<(), String> { Repositories::new(repositories(&state)?.connection()).set_contact_favorite(&id, favorite).map_err(|_| "Unable to update contact".to_string()) }
#[tauri::command]
pub fn remove_account(id: String, state: State<'_, AppState>) -> Result<(), String> {
  let database = repositories(&state)?;
  let repos = Repositories::new(database.connection());
  let removed = repos.remove_account(&id).map_err(|_| "Unable to remove the account.".to_string())?;
  if let Some(address) = removed {
    let _ = OsKeyring.delete(&address);
    let _ = delete_oauth_secret(&address);
  }
  Ok(())
}
#[tauri::command]
pub fn get_storage_usage(app: tauri::AppHandle) -> Result<StorageUsage, String> {
  let root = app.path().app_data_dir().map_err(|_| "Data directory unavailable".to_string())?;
  let usage = usage_of(&root);
  Ok(StorageUsage { database: usage[0], attachments: usage[1], avatars: usage[2], cache: usage[3], logs: usage[4], total: usage[0] + usage[1] + usage[2] + usage[3] + usage[4] })
}
#[tauri::command]
pub fn clear_cache(app: tauri::AppHandle) -> Result<i64, String> {
  let root = app.path().app_data_dir().map_err(|_| "Data directory unavailable".to_string())?;
  let cache = root.join("cache");
  let entries = fs::read_dir(&cache).ok();
  if entries.is_none() { return Ok(0); }
  let mut freed: i64 = 0;
  for entry in entries.unwrap() {
    let info = entry.ok();
    if info.is_none() { continue; }
    let target = info.unwrap().path();
    if target.is_file() { freed += file_size(&target); fs::remove_file(&target).ok(); }
    else if target.is_dir() { freed += dir_size(&target); fs::remove_dir_all(&target).ok(); }
  }
  Ok(freed)
}
fn usage_of(root: &Path) -> [i64; 5] { [dir_size(&root.join("database")), dir_size(&root.join("attachments")), dir_size(&root.join("avatars")), dir_size(&root.join("cache")), dir_size(&root.join("logs"))] }
fn file_size(path: &Path) -> i64 { fs::metadata(path).ok().map(|meta| meta.len() as i64).unwrap_or_default() }
fn dir_size(directory: &Path) -> i64 {
  let entries = fs::read_dir(directory).ok();
  if entries.is_none() { return 0; }
  let mut total: i64 = 0;
  for entry in entries.unwrap() {
    let info = entry.ok();
    if info.is_none() { continue; }
    let target = info.unwrap().path();
    if target.is_dir() { total += dir_size(&target); }
    else if target.is_file() { total += file_size(&target); }
  }
  total
}
