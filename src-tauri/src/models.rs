use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailSummary { pub id: String, pub sender_name: String, pub subject: String, pub preview: String, pub received_at: String, pub is_read: bool, pub is_starred: bool }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailDetail { pub id: String, pub account_id: String, pub direction: String, pub delivery_state: String, pub sender_name: String, pub sender_email: String, pub subject: String, pub body_text: String, pub received_at: String, pub is_read: bool, pub is_starred: bool, pub to: Vec<String>, pub cc: Vec<String>, pub bcc: Vec<String> }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account { pub id: String, pub display_name: String, pub email_address: String, pub status: String, pub connection_state: String, pub connection_error: Option<String>, pub last_verified_at: Option<String> }
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountConnection { pub id: String, pub email_address: String, pub imap_host: String, pub imap_port: u16, pub encryption: String, pub auth_kind: String }
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAccount { pub display_name: String, pub email_address: String, pub imap_host: String, pub imap_port: u16, pub smtp_host: String, pub smtp_port: u16, pub encryption: String, pub auth_kind: String }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Contact { pub id: String, pub name: String, pub email: Option<String>, pub department: Option<String>, pub position: Option<String>, pub favorite: bool }
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContact { pub name: String, pub email: Option<String>, pub department: Option<String>, pub position: Option<String> }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncQueueItem { pub id: String, pub entity_type: String, pub entity_id: String, pub operation: String, pub attempt_count: i64, pub last_error: Option<String> }
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftInput { pub account_id: String, pub to: Vec<String>, pub cc: Vec<String>, pub bcc: Vec<String>, pub subject: String, pub body_text: String }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft { pub id: String, pub delivery_state: String, pub updated_at: String }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Channel { pub id: String, pub title: String, pub slug: Option<String>, pub description: Option<String>, pub member_count: i64 }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation { pub id: String, pub kind: String, pub title: String, pub description: Option<String>, pub member_count: i64, pub last_activity_at: String }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
/// One row of messenger history. Channel listings only contain thread roots
/// (`thread_id == id`), each carrying its thread's aggregate reactions and reply
/// summary; `message_thread` returns the individual replies of one thread.
pub struct ChatMessage { pub id: String, pub conversation_id: String, pub thread_id: String, pub sender_name: String, pub body: String, pub sent_at: String, pub reply_count: i64, pub last_reply_at: Option<String>, pub edited: bool, pub pinned: bool, pub mine: bool, pub reactions: Vec<ReactionSummary> }
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageInput { pub conversation_id: String, pub body: String, pub reply_to_id: Option<String> }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification { pub id: String, pub kind: String, pub title: String, pub body: String, pub entity_type: Option<String>, pub entity_id: Option<String>, pub created_at: String }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPreference { pub key: String, pub enabled: bool }
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateNotificationPreference { pub key: String, pub enabled: bool }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactionSummary { pub emoji: String, pub count: i64, pub reacted_by_me: bool }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageReaction { pub message_id: String, pub emoji: String, pub count: i64, pub reacted_by_me: bool }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsEntry { pub key: String, pub value: String }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Presence { pub status: String, pub since: String }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult { pub kind: String, pub id: String, pub title: String, pub subtitle: String }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageUsage { pub database: i64, pub attachments: i64, pub avatars: i64, pub cache: i64, pub logs: i64, pub total: i64 }
