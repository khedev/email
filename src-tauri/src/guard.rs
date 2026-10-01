//! Panic containment for command work.
//!
//! Synchronous Tauri commands are executed inline inside the webview2 IPC
//! callback on the UI thread. A panic unwinding into that `extern "system"`
//! frame cannot be caught by the runtime ("panic in a function that cannot
//! unwind"), so the process aborts with `STATUS_STACK_BUFFER_OVERRUN`: the user
//! loses the whole application instead of seeing one failed action. Catching the
//! panic at our own boundary turns it into the command's ordinary error result
//! and keeps the process alive.
//!
//! This is a safety net, not the error-handling strategy: every failure that can
//! be anticipated is still classified and returned as a `Result` by the code
//! doing the work. Reaching this net means a bug, so it is logged as one.
//!
//! A panic raised while the application state lock is held poisons that mutex;
//! every later command then reports "Application state is unavailable" instead
//! of dying, which is the deliberate trade-off of not aborting (`repositories`
//! already maps poisoning to that message).

use std::any::Any;

/// The message shown when a command panicked. Deliberately generic: the panic
/// payload is developer-facing and goes to the log instead of the UI.
pub const PANIC_MESSAGE: &str = "Something went wrong while running this action. Please try again, and check the log if it keeps happening.";

/// Runs `work` and returns its result, or an error when it panicked.
///
/// `AssertUnwindSafe` is sound for this use: the work is a self-contained
/// request whose only shared state is the application state mutex, and on a
/// panic nothing observes whatever the work left behind — the caller reports the
/// failure and the app keeps running.
pub fn guarded<T>(work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
  match std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)) {
    Ok(result) => result,
    Err(payload) => {
      tracing::error!(panic = %panic_message(&payload), "command panicked");
      Err(PANIC_MESSAGE.to_string())
    }
  }
}

/// Best-effort rendering of a panic payload: `panic!("literal")` carries a
/// `&str`, `panic!("{}", value)` a `String`.
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
  if let Some(text) = payload.downcast_ref::<&str>() { return (*text).to_string(); }
  if let Some(text) = payload.downcast_ref::<String>() { return text.clone(); }
  "unknown panic payload".to_string()
}