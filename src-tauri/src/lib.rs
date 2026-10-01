mod commands;
// The store layer is the library's public API surface (repository pattern);
// the integration tests in `tests/` exercise it directly.
pub mod database;
pub mod discover;
pub mod error;
// Panic containment for command work: synchronous commands run inside the
// webview2 IPC callback, where an unwinding panic cannot be caught and aborts
// the whole process (see `guard`).
pub mod guard;
pub mod mailbox;
pub mod models;
pub mod oauth;
pub mod repositories;
pub mod security;
pub mod sync;
pub mod verify;
mod seed;

use std::sync::{atomic::AtomicBool, Arc, Mutex};
use tauri::Manager;
use database::Database;

pub struct AppState {
  database: Mutex<Database>,
  /// Set by `cancel_google_sign_in` and polled by the waiting browser
  /// round-trip, so a closed browser tab or a change of mind returns at once
  /// instead of waiting out `oauth::AUTH_TIMEOUT`.
  oauth_cancel: Arc<AtomicBool>,
}

pub fn run() {
  tracing_subscriber::fmt().with_env_filter("relay=info,warn").with_target(true).init();
  tauri::Builder::default()
    .plugin(tauri_plugin_opener::init())
    .plugin(tauri_plugin_notification::init())
    // Restores each window's size, position and maximized state on launch and
    // saves them on exit, so the user's chosen geometry survives restarts.
    .plugin(tauri_plugin_window_state::Builder::default().build())
    .setup(|app| {
      let app_data = app.path().app_data_dir()?;
      let database = Database::open(&app_data)?;
      app.manage(AppState { database: Mutex::new(database), oauth_cancel: oauth::new_cancel_flag() });
      Ok(())
    })
    .invoke_handler(tauri::generate_handler![commands::get_app_health, commands::get_database_info, commands::get_inbox, commands::get_emails_in_folder, commands::get_starred, commands::get_email, commands::get_email_thread, commands::mark_email_read, commands::set_email_star, commands::archive_email, commands::trash_email, commands::get_accounts, commands::add_email_account, commands::test_email_connection, commands::remove_account, commands::get_contacts, commands::create_contact, commands::set_contact_favorite, commands::get_pending_sync_items, commands::save_draft, commands::queue_email_send, commands::get_channels, commands::get_channel_messages, commands::send_company_message, commands::get_conversations, commands::open_direct_message, commands::edit_message, commands::delete_message, commands::set_message_pin, commands::get_message_thread, commands::get_channel_reactions, commands::toggle_message_reaction, commands::get_sync_overview, commands::retry_failed_sync, commands::get_notifications, commands::get_unread_notification_count, commands::mark_all_notifications_read, commands::get_notification_preferences, commands::update_notification_preference, commands::show_native_notification, commands::get_app_settings, commands::set_app_setting, commands::get_presence, commands::set_presence, commands::search_global, commands::get_storage_usage, commands::clear_cache, commands::google_oauth_sign_in, commands::forget_oauth_client_id, commands::cancel_google_sign_in, commands::sync_mail, commands::fetch_email_body, commands::auto_sign_in])
    .run(tauri::generate_context!())
    .expect("error while running Relay");
}
