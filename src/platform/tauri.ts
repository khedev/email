export type SyncState = 'online' | 'offline' | 'connecting' | 'synchronizing' | 'error';
export type MailRole = 'inbox' | 'sent' | 'drafts' | 'archive' | 'trash';
export type PresenceStatus = 'online' | 'away' | 'dnd' | 'offline';
export type ThemePreference = 'light' | 'dark' | 'system';
export type DensityPreference = 'comfortable' | 'compact';

// Store health, not connectivity: `state` is "error" while queued work has
// failed, and `lastSyncAt` is when mail last synced. Whether this computer has a
// network is reported by the webview (`navigator.onLine`), which is what the
// connection indicator uses (BUG-015).
export interface AppHealth { state: SyncState; lastSyncAt: string | null; }
export interface EmailSummary { id: string; senderName: string; subject: string; preview: string; receivedAt: string; isRead: boolean; isStarred: boolean; }
export interface EmailDetail extends EmailSummary { accountId: string; direction: 'inbound' | 'outbound'; deliveryState: 'draft' | 'queued' | 'failed' | 'received' | 'sent'; senderEmail: string; bodyText: string; to: string[]; cc: string[]; bcc: string[]; deliveryError: string | null; }
export interface Account { id: string; displayName: string; emailAddress: string; status: string; connectionState: 'unverified' | 'connected' | 'error'; connectionError: string | null; lastVerifiedAt: string | null; }
export interface CreateAccountInput { displayName: string; emailAddress: string; imapHost: string; imapPort: number; smtpHost: string; smtpPort: number; encryption: string; authKind: string; }
export interface DraftInput { accountId: string; to: string[]; cc: string[]; bcc: string[]; subject: string; bodyText: string; }
export interface Draft { id: string; deliveryState: 'draft' | 'queued'; updatedAt: string; }
export interface DeliveryFailure { id: string; error: string; }
export interface DeliveryReport { sent: string[]; failed: DeliveryFailure[]; }
export interface Channel { id: string; title: string; slug: string | null; description: string | null; memberCount: number; }
export interface Conversation { id: string; kind: 'channel' | 'dm'; title: string; description: string | null; memberCount: number; lastActivityAt: string; }
export interface ChatMessage { id: string; conversationId: string; threadId: string; senderName: string; body: string; sentAt: string; replyCount: number; lastReplyAt: string | null; edited: boolean; pinned: boolean; mine: boolean; reactions: ReactionSummary[]; }
export interface MessageReaction { messageId: string; emoji: string; count: number; reactedByMe: boolean; }
export interface ReactionSummary { emoji: string; count: number; reactedByMe: boolean; }
export interface Contact { id: string; name: string; email: string | null; department: string | null; position: string | null; favorite: boolean; }
export interface ContactInput { name: string; email?: string | null; department?: string | null; position?: string | null; }
export interface SyncOverview { state: SyncState; pending: number; failed: number; conflicts: number; lastSyncAt: string | null; detail: string | null; }
export interface AppNotification { id: string; kind: string; title: string; body: string; entityType: string | null; entityId: string | null; createdAt: string; }
export interface NotificationPreference { key: string; enabled: boolean; }
export interface SettingsEntry { key: string; value: string; }
export interface Presence { status: PresenceStatus; since: string; }
export interface SearchResult { kind: 'email' | 'message' | 'contact' | 'channel'; id: string; title: string; subtitle: string; }
export interface StorageUsage { database: number; attachments: number; avatars: number; cache: number; logs: number; total: number; }

export function isNative(): boolean { return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window; }

async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isNative()) {
    const { previewInvoke } = await import('./preview');
    return previewInvoke<T>(command, args);
  }
  const { invoke: call } = await import('@tauri-apps/api/core');
  return call<T>(command, args);
}

/** Tauri command failures arrive as plain strings, not Error instances. Prefer
 *  the backend's specific (already user-safe) message and fall back otherwise. */
export function commandError(err: unknown, fallback: string): string {
  if (typeof err === 'string' && err.trim()) return err;
  if (err instanceof Error && err.message) return err.message;
  return fallback;
}

export async function getAppHealth(): Promise<AppHealth> {
  if (!isNative()) return { state: navigator.onLine ? 'online' : 'offline', lastSyncAt: null };
  return invoke<AppHealth>('get_app_health');
}
export async function getInbox(): Promise<EmailSummary[]> { return invoke<EmailSummary[]>('get_inbox'); }
export async function getEmailsInFolder(role: MailRole): Promise<EmailSummary[]> { return invoke<EmailSummary[]>('get_emails_in_folder', { role }); }
export async function getStarred(): Promise<EmailSummary[]> { return invoke<EmailSummary[]>('get_starred'); }
export async function getEmail(id: string): Promise<EmailDetail | null> { return invoke<EmailDetail | null>('get_email', { id }); }
export async function getEmailThread(id: string): Promise<EmailSummary[]> { return invoke<EmailSummary[]>('get_email_thread', { id }); }
/**
 * Downloads one message's body straight from the mail server and stores it.
 *
 * The repair path for a message that is already in the local store with an empty
 * body — rows written before the body decoded, and rows older than the newest
 * fetch window. Resolves with the decoded text (`''` when the message genuinely
 * has no readable text part), and rejects when the message has left the server.
 */
export async function fetchEmailBody(id: string): Promise<string> { return invoke<string>('fetch_email_body', { id }); }
export async function markEmailRead(id: string, isRead: boolean): Promise<void> { return invoke<void>('mark_email_read', { id, isRead }); }
export async function setEmailStar(id: string, isStarred: boolean): Promise<void> { return invoke<void>('set_email_star', { id, isStarred }); }
export async function archiveEmail(id: string): Promise<void> { return invoke<void>('archive_email', { id }); }
export async function trashEmail(id: string): Promise<void> { return invoke<void>('trash_email', { id }); }
export async function getAccounts(): Promise<Account[]> { return invoke<Account[]>('get_accounts'); }
export async function addEmailAccount(input: CreateAccountInput, password: string): Promise<Account> { return invoke<Account>('add_email_account', { input, password }); }
export async function testEmailConnection(id: string, password?: string): Promise<Account> { return invoke<Account>('test_email_connection', { id, password: password ?? null }); }
export async function googleOAuthSignIn(clientId: string, clientSecret?: string): Promise<Account> { return invoke<Account>('google_oauth_sign_in', { clientId, clientSecret: clientSecret ?? null }); }
export async function forgetOAuthClientId(): Promise<void> { return invoke<void>('forget_oauth_client_id'); }
export async function cancelGoogleSignIn(): Promise<void> { return invoke<void>('cancel_google_sign_in'); }
/** Fetches new mail for every signed-in account (or the given ids). */
export async function syncMail(accountIds?: string[]): Promise<number> { return invoke<number>('sync_mail', { accountIds: accountIds ?? null }); }
export async function autoSignIn(email: string, password: string): Promise<Account> { return invoke<Account>('auto_sign_in', { email, password }); }
export async function removeAccount(id: string): Promise<void> { return invoke<void>('remove_account', { id }); }
export async function saveDraft(input: DraftInput, draftId?: string): Promise<Draft> { return invoke<Draft>('save_draft', { input, draftId }); }
export async function queueEmailSend(id: string): Promise<Draft> { return invoke<Draft>('queue_email_send', { id }); }
/**
 * Transmits outbound mail that is waiting in the local send queue.
 *
 * Called by the shell right after a send is queued (so transmission starts as
 * the message opens in Sent), once on startup to drain anything a quit left
 * behind, and by the reading pane's Try again. Omitting `ids` means "everything
 * waiting"; per-message failures come back in the report rather than as a
 * rejection, because one refused recipient must not hide the accepted ones.
 */
export async function deliverQueuedMail(ids?: string[]): Promise<DeliveryReport> { return invoke<DeliveryReport>('deliver_queued_mail', { ids: ids ?? null }); }
export async function getChannels(): Promise<Channel[]> { return invoke<Channel[]>('get_channels'); }
export async function getConversations(): Promise<Conversation[]> { return invoke<Conversation[]>('get_conversations'); }
export async function openDirectMessage(contactId: string): Promise<Conversation> { return invoke<Conversation>('open_direct_message', { contactId }); }
export async function getChannelMessages(channelId: string): Promise<ChatMessage[]> { return invoke<ChatMessage[]>('get_channel_messages', { channelId }); }
export async function sendCompanyMessage(input: { conversationId: string; body: string; replyToId?: string }): Promise<ChatMessage> { return invoke<ChatMessage>('send_company_message', { input }); }
export async function editMessage(id: string, body: string): Promise<ChatMessage> { return invoke<ChatMessage>('edit_message', { id, body }); }
export async function deleteMessage(id: string): Promise<void> { return invoke<void>('delete_message', { id }); }
export async function setMessagePin(id: string, pinned: boolean): Promise<void> { return invoke<void>('set_message_pin', { id, pinned }); }
export async function getMessageThread(rootId: string): Promise<ChatMessage[]> { return invoke<ChatMessage[]>('get_message_thread', { rootId }); }
export async function getChannelReactions(channelId: string): Promise<MessageReaction[]> { return invoke<MessageReaction[]>('get_channel_reactions', { channelId }); }
export async function toggleMessageReaction(messageId: string, emoji: string): Promise<ReactionSummary[]> { return invoke<ReactionSummary[]>('toggle_message_reaction', { messageId, emoji }); }
export async function getSyncOverview(): Promise<SyncOverview> { return invoke<SyncOverview>('get_sync_overview'); }
export async function retryFailedSync(): Promise<number> { return invoke<number>('retry_failed_sync'); }
export async function getNotifications(): Promise<AppNotification[]> { return invoke<AppNotification[]>('get_notifications'); }
export async function getUnreadNotificationCount(): Promise<number> { return invoke<number>('get_unread_notification_count'); }
export async function markAllNotificationsRead(): Promise<void> { return invoke<void>('mark_all_notifications_read'); }
export async function getNotificationPreferences(): Promise<NotificationPreference[]> { return invoke<NotificationPreference[]>('get_notification_preferences'); }
export async function updateNotificationPreference(key: string, enabled: boolean): Promise<void> { return invoke<void>('update_notification_preference', { input: { key, enabled } }); }
export async function showNativeNotification(title: string, body: string): Promise<void> { return invoke<void>('show_native_notification', { title, body }); }
export async function getAppSettings(): Promise<SettingsEntry[]> { return invoke<SettingsEntry[]>('get_app_settings'); }
export async function setAppSetting(key: string, value: string): Promise<void> { return invoke<void>('set_app_setting', { key, value }); }
export async function getPresence(): Promise<Presence> { return invoke<Presence>('get_presence'); }
export async function setPresenceStatus(status: PresenceStatus): Promise<Presence> { return invoke<Presence>('set_presence', { status }); }
export async function searchGlobal(query: string): Promise<SearchResult[]> { return invoke<SearchResult[]>('search_global', { query }); }
export async function getContacts(): Promise<Contact[]> { return invoke<Contact[]>('get_contacts'); }
export async function createContact(input: ContactInput): Promise<Contact> { return invoke<Contact>('create_contact', { input }); }
export async function setContactFavorite(id: string, favorite: boolean): Promise<void> { return invoke<void>('set_contact_favorite', { id, favorite }); }
export async function getStorageUsage(): Promise<StorageUsage> { return invoke<StorageUsage>('get_storage_usage'); }
export async function clearCache(): Promise<number> { return invoke<number>('clear_cache'); }
