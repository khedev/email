use chrono::Utc;
use rusqlite::{params, Connection};
use serde::Serialize;
use crate::error::AppError;

/// Transport stays outside repositories so IMAP/SMTP and company API sync remain separate adapters.
pub trait RemoteSyncTransport { fn is_configured(&self) -> bool; }
pub struct DisabledTransport;
impl RemoteSyncTransport for DisabledTransport { fn is_configured(&self) -> bool { false } }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncOverview { pub state: String, pub pending: i64, pub failed: i64, pub conflicts: i64, pub last_sync_at: Option<String>, pub detail: Option<String> }
pub struct SyncEngine<'a, T: RemoteSyncTransport> { connection: &'a Connection, transport: T }
impl<'a, T: RemoteSyncTransport> SyncEngine<'a, T> {
  pub fn new(connection: &'a Connection, transport: T) -> Self { Self { connection, transport } }
  pub fn overview(&self) -> Result<SyncOverview, AppError> {
    let pending = self.count("status IN ('pending', 'syncing')")?; let failed = self.count("status = 'failed'")?; let conflicts = self.connection.query_row("SELECT COUNT(*) FROM sync_conflicts WHERE status = 'open'", [], |row| row.get(0))?;
    let last_sync_at = self.connection.query_row("SELECT value FROM sync_metadata WHERE key = 'last_successful_sync'", [], |row| row.get(0)).ok();
    let (state, detail) = if !self.transport.is_configured() { ("offline".into(), Some("No remote transport is configured. Local changes are safely queued.".into())) } else if failed > 0 { ("error".into(), Some("Some changes need retrying.".into())) } else if pending > 0 { ("synchronizing".into(), None) } else { ("online".into(), None) };
    Ok(SyncOverview { state, pending, failed, conflicts, last_sync_at, detail })
  }
  pub fn retry_failed(&self) -> Result<i64, AppError> { let updated = self.connection.execute("UPDATE sync_queue SET status = 'pending', last_error = NULL, next_attempt_at = NULL, locked_at = NULL, updated_at = ?1 WHERE status = 'failed'", params![Utc::now().to_rfc3339()])?; Ok(updated as i64) }
  fn count(&self, filter: &str) -> Result<i64, AppError> { Ok(self.connection.query_row(&format!("SELECT COUNT(*) FROM sync_queue WHERE {filter}"), [], |row| row.get(0))?) }
}

