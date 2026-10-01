use std::{fs, path::{Path, PathBuf}};
use rusqlite::Connection;
use crate::error::AppError;

pub struct Database { connection: Connection, path: PathBuf }
impl Database {
  pub fn open(app_data: &Path) -> Result<Self, AppError> {
    let database_directory = app_data.join("database");
    fs::create_dir_all(&database_directory)?;
    for directory in ["attachments", "avatars", "cache", "logs"] { fs::create_dir_all(app_data.join(directory))?; }
    let path = database_directory.join("relay.db");
    let connection = Connection::open(&path)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    migrate(&connection)?;
    #[cfg(debug_assertions)]
    crate::seed::seed_development_data(&connection)?;
    // The local user's contact row anchors messenger identity (the "mine"
    // flag, reactions, channel membership). Debug seeds create it; release
    // builds must too, or reactions and direct messages would fail.
    crate::repositories::Repositories::new(&connection).ensure_self_identity()?;
    Ok(Self { connection, path })
  }
  pub fn schema_version(&self) -> Result<i64, AppError> { Ok(self.connection.query_row("PRAGMA user_version", [], |row| row.get(0))?) }
  pub fn path(&self) -> &Path { &self.path }
  pub fn connection(&self) -> &Connection { &self.connection }
}
fn migrate(connection: &Connection) -> Result<(), AppError> {
  let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
  if version < 1 {
    connection.execute_batch(include_str!("../migrations/0001_foundation.sql"))?;
    connection.execute_batch("PRAGMA user_version = 1;")?;
  }
  if version < 2 {
    connection.execute_batch(include_str!("../migrations/0002_email_cache.sql"))?;
    connection.execute_batch("PRAGMA user_version = 2;")?;
  }
  if version < 3 {
    connection.execute_batch(include_str!("../migrations/0003_email_composer.sql"))?;
    connection.execute_batch("PRAGMA user_version = 3;")?;
  }
  if version < 4 {
    connection.execute_batch(include_str!("../migrations/0004_messenger.sql"))?;
    connection.execute_batch("PRAGMA user_version = 4;")?;
  }
  if version < 5 {
    connection.execute_batch(include_str!("../migrations/0005_sync_engine.sql"))?;
    connection.execute_batch("PRAGMA user_version = 5;")?;
  }
  if version < 6 {
    connection.execute_batch(include_str!("../migrations/0006_notifications.sql"))?;
    connection.execute_batch("PRAGMA user_version = 6;")?;
  }
  if version < 7 {
    connection.execute_batch(include_str!("../migrations/0007_presence_search_settings.sql"))?;
    connection.execute_batch("PRAGMA user_version = 7;")?;
  }
  if version < 8 {
    connection.execute_batch(include_str!("../migrations/0008_fts_external_content.sql"))?;
    connection.execute_batch("PRAGMA user_version = 8;")?;
  }
  if version < 9 {
    connection.execute_batch(include_str!("../migrations/0009_message_actions.sql"))?;
    connection.execute_batch("PRAGMA user_version = 9;")?;
  }
  if version < 10 {
    connection.execute_batch(include_str!("../migrations/0010_account_credentials.sql"))?;
    connection.execute_batch("PRAGMA user_version = 10;")?;
  }
  if version < 11 {
    connection.execute_batch(include_str!("../migrations/0011_message_threads.sql"))?;
    connection.execute_batch("PRAGMA user_version = 11;")?;
  }
  Ok(())
}

