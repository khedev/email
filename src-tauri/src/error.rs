#[derive(Debug, thiserror::Error)]
pub enum AppError {
  #[error("database operation failed")]
  Database(#[from] rusqlite::Error),
  #[error("filesystem operation failed")]
  Io(#[from] std::io::Error),
  #[error("secure credential store operation failed: {0}")]
  SecureStore(String),
  #[error("input validation failed")]
  Validation,
}
