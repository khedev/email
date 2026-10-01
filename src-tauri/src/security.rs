//! OS credential storage (Phase 7).
//!
//! Account passwords never touch SQLite, the IPC surface, or logs. They live in
//! the operating system's credential manager (Windows Credential Manager on
//! Windows; the sync-secret-service backend elsewhere), keyed by the account's
//! email address. Only a *reference* to the entry is persisted in the database.

use crate::error::AppError;

/// Keyring service name shared by every Relay credential entry.
pub const KEYRING_SERVICE: &str = "Relay Mail";

/// Minimal credential interface so the OS keyring stays swappable in tests.
pub trait CredentialStore {
  fn save(&self, user: &str, secret: &str) -> Result<(), AppError>;
  fn load(&self, user: &str) -> Result<Option<String>, AppError>;
  fn delete(&self, user: &str) -> Result<(), AppError>;
}

/// The reference stored on the account row - identifies the keyring entry
/// without exposing any secret material.
pub fn credential_ref(user: &str) -> String { format!("keyring:{KEYRING_SERVICE}:{user}") }

/// Separate keyring service for Google OAuth tokens (never mixed with
/// password entries).
pub const OAUTH_KEYRING_SERVICE: &str = "Relay Mail OAuth";

/// Keyring user under which the optional Google OAuth client secret is stored.
/// The secret is a credential, so it never goes into the settings table (that
/// table is readable by every settings command). Forgetting the client id also
/// deletes this entry.
pub const OAUTH_CLIENT_SECRET_USER: &str = "oauth.client_secret";

/// The reference stored on the account row for an OAuth secret entry.
pub fn oauth_credential_ref(address: &str) -> String { format!("keyring:{OAUTH_KEYRING_SERVICE}:{address}") }

/// OAuth secrets are stored as JSON so the client id travels with the token
/// and later refreshes need nothing else.
pub fn save_oauth_secret(address: &str, client_id: &str, refresh_token: &str) -> Result<(), AppError> {
  let payload = serde_json::json!({ "client_id": client_id, "refresh_token": refresh_token }).to_string();
  entry_for(OAUTH_KEYRING_SERVICE, address)?.set_password(&payload).map_err(store_failure)
}

/// Loads the stored `(client_id, refresh_token)` pair, if any.
pub fn load_oauth_secret(address: &str) -> Result<Option<(String, String)>, AppError> {
  match entry_for(OAUTH_KEYRING_SERVICE, address)?.get_password() {
    Ok(payload) => match serde_json::from_str::<serde_json::Value>(&payload) {
      Ok(value) => Ok(Some((value["client_id"].as_str().unwrap_or_default().to_string(), value["refresh_token"].as_str().unwrap_or_default().to_string()))),
      Err(_) => Ok(None),
    },
    Err(keyring::Error::NoEntry) => Ok(None),
    Err(error) => Err(store_failure(error)),
  }
}

pub fn delete_oauth_secret(address: &str) -> Result<(), AppError> {
  match entry_for(OAUTH_KEYRING_SERVICE, address)?.delete_credential() {
    Ok(()) => Ok(()),
    Err(keyring::Error::NoEntry) => Ok(()),
    Err(error) => Err(store_failure(error)),
  }
}

/// Stores the Google OAuth client secret in the OS credential manager. An empty
/// value deletes any previously stored secret so a Web-application client (which
/// has no secret) does not keep a stale one around.
pub fn save_oauth_client_secret(secret: &str) -> Result<(), AppError> {
  let trimmed = secret.trim();
  if trimmed.is_empty() { return delete_oauth_client_secret(); }
  if trimmed.len() > 256 || trimmed.chars().any(|character| character.is_whitespace() || character.is_control()) {
    return Err(AppError::Validation);
  }
  entry_for(OAUTH_KEYRING_SERVICE, OAUTH_CLIENT_SECRET_USER)?.set_password(trimmed).map_err(store_failure)
}

pub fn load_oauth_client_secret() -> Result<Option<String>, AppError> {
  match entry_for(OAUTH_KEYRING_SERVICE, OAUTH_CLIENT_SECRET_USER)?.get_password() {
    Ok(secret) => { let trimmed = secret.trim().to_string(); if trimmed.is_empty() { Ok(None) } else { Ok(Some(trimmed)) } }
    Err(keyring::Error::NoEntry) => Ok(None),
    Err(error) => Err(store_failure(error)),
  }
}

pub fn delete_oauth_client_secret() -> Result<(), AppError> {
  match entry_for(OAUTH_KEYRING_SERVICE, OAUTH_CLIENT_SECRET_USER)?.delete_credential() {
    Ok(()) => Ok(()),
    Err(keyring::Error::NoEntry) => Ok(()),
    Err(error) => Err(store_failure(error)),
  }
}

/// Stores secrets in the operating system's credential manager.
pub struct OsKeyring;

impl CredentialStore for OsKeyring {
  fn save(&self, user: &str, secret: &str) -> Result<(), AppError> { entry(user)?.set_password(secret).map_err(store_failure) }
  fn load(&self, user: &str) -> Result<Option<String>, AppError> {
    match entry(user)?.get_password() {
      Ok(secret) => Ok(Some(secret)),
      Err(keyring::Error::NoEntry) => Ok(None),
      Err(error) => Err(store_failure(error)),
    }
  }
  fn delete(&self, user: &str) -> Result<(), AppError> {
    match entry(user)?.delete_credential() {
      Ok(()) => Ok(()),
      Err(keyring::Error::NoEntry) => Ok(()),
      Err(error) => Err(store_failure(error)),
    }
  }
}

fn entry(user: &str) -> Result<keyring::Entry, AppError> { entry_for(KEYRING_SERVICE, user) }
fn entry_for(service: &str, user: &str) -> Result<keyring::Entry, AppError> { keyring::Entry::new(service, user).map_err(store_failure) }
fn store_failure(error: keyring::Error) -> AppError { AppError::SecureStore(error.to_string()) }
