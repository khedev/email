// Local-first browser preview backend.
//
// When the app runs outside the Tauri shell (`npm run dev` in a plain browser),
// there is no native Rust host to answer IPC calls. This module provides an
// in-memory store that mirrors the native command contract (same camelCase
// shapes) so the UI is fully explorable. It is a *development preview only*:
// nothing here is persisted, and the UI shows a "Preview" badge to make the
// context explicit. The desktop app never touches this code path.
import type {
  Account, AppNotification, ChatMessage, Contact, EmailDetail, EmailSummary,
  MessageReaction, NotificationPreference, ReactionSummary, SearchResult,
} from './tauri';

const nowIso = () => new Date().toISOString();
const minsAgo = (minutes: number) => new Date(Date.now() - minutes * 60_000).toISOString();
const uid = () => (globalThis.crypto?.randomUUID ? globalThis.crypto.randomUUID() : `p-${Math.random().toString(36).slice(2)}`);

interface PreviewEmail {
  id: string; accountId: string; direction: 'inbound' | 'outbound';
  deliveryState: 'draft' | 'queued' | 'received' | 'sent';
  folderRole: 'inbox' | 'sent' | 'drafts' | 'archive' | 'trash';
  senderName: string; senderEmail: string; subject: string; bodyText: string;
  threadId: string | null;
  receivedAt: string; isRead: boolean; isStarred: boolean; to: string[]; cc: string[]; bcc: string[];
}
// Mirrors the native thread model: every message belongs to a thread root
// (`threadId`), a root points at itself and replies point at the root no matter
// how deep the reply chain runs. `replyToId` keeps the exact parent.
interface PreviewMessage { id: string; conversationId: string; senderName: string; body: string; sentAt: string; replyToId: string | null; threadId: string; editedAt?: string | null; pinnedAt?: string | null; }
interface PreviewConversation { id: string; kind: 'channel' | 'dm'; title: string; description: string | null; memberCount: number; lastActivityAt: string; }

const SELF = 'Preview User';
// Reactions live on thread roots; a reply bubble never carries its own aggregate.
function toChat(m: PreviewMessage, extra: { replyCount?: number; lastReplyAt?: string | null; reactions?: ReactionSummary[] } = {}): ChatMessage {
  return { id: m.id, conversationId: m.conversationId, threadId: m.threadId, senderName: m.senderName, body: m.body, sentAt: m.sentAt,
    replyCount: extra.replyCount ?? 0, lastReplyAt: extra.lastReplyAt ?? null,
    edited: m.editedAt != null, pinned: m.pinnedAt != null, mine: m.senderName === SELF, reactions: extra.reactions ?? [] };
}
function threadReplies(rootId: string): PreviewMessage[] {
  return messages.filter(m => m.threadId === rootId && m.id !== rootId).sort((a, b) => (a.sentAt > b.sentAt ? 1 : -1));
}

const emails: PreviewEmail[] = [
  { id: 'pv-inbox-1', accountId: 'pv-account', direction: 'inbound', deliveryState: 'received', folderRole: 'inbox', senderName: 'Maya Chen', senderEmail: 'maya@northstar.test', subject: 'Q3 planning: a few decisions to close', bodyText: 'The updated Q3 planning document is ready for review. I added the notes from our leadership sessions and highlighted the three decisions we need to close this week.', threadId: 'pv-thread-q3', receivedAt: minsAgo(41), isRead: false, isStarred: false, to: [], cc: [], bcc: [] },
  { id: 'pv-inbox-2', accountId: 'pv-account', direction: 'inbound', deliveryState: 'received', folderRole: 'inbox', senderName: 'Diego Alvarez', senderEmail: 'diego@northstar.test', subject: 'Re: Design system audit', bodyText: 'Great catch on the form states. I added a few thoughts in the document for the next pass.', threadId: 'pv-thread-design', receivedAt: minsAgo(187), isRead: false, isStarred: true, to: [], cc: [], bcc: [] },
  { id: 'pv-inbox-3', accountId: 'pv-account', direction: 'inbound', deliveryState: 'received', folderRole: 'inbox', senderName: 'People Operations', senderEmail: 'people@northstar.test', subject: 'Open enrollment starts Monday', bodyText: 'Your benefits enrollment window opens next week. Here is what to know.', threadId: null, receivedAt: minsAgo(333), isRead: true, isStarred: false, to: [], cc: [], bcc: [] },
  { id: 'pv-inbox-4', accountId: 'pv-account', direction: 'inbound', deliveryState: 'received', folderRole: 'inbox', senderName: 'Priya Nair', senderEmail: 'priya@northstar.test', subject: 'Research partnership update', bodyText: 'The team at Solace confirmed a meeting for Thursday afternoon.', threadId: null, receivedAt: minsAgo(850), isRead: true, isStarred: false, to: [], cc: [], bcc: [] },
  { id: 'pv-draft-1', accountId: 'pv-account', direction: 'outbound', deliveryState: 'draft', folderRole: 'drafts', senderName: 'Preview Account', senderEmail: 'preview@relay.local', subject: 'Release notes draft', bodyText: 'Hi team,\n\nHere are the release notes for the next build.', threadId: null, receivedAt: minsAgo(12), isRead: true, isStarred: false, to: ['maya@northstar.test'], cc: [], bcc: [] },
  { id: 'pv-sent-1', accountId: 'pv-account', direction: 'outbound', deliveryState: 'queued', folderRole: 'sent', senderName: 'Preview Account', senderEmail: 'preview@relay.local', subject: 'Re: Q3 planning: a few decisions to close', bodyText: 'Looks good to me. I will update the tracker today.', threadId: 'pv-thread-q3', receivedAt: minsAgo(360), isRead: true, isStarred: false, to: ['maya@northstar.test'], cc: [], bcc: [] },
];

interface PreviewChannel { id: string; title: string; slug: string; description: string; memberCount: number; }
const channels: PreviewChannel[] = [
  { id: 'pv-general', title: 'General', slug: 'general', description: 'Company-wide conversation', memberCount: 5 },
  { id: 'pv-it-support', title: 'IT Support', slug: 'it-support', description: 'Servers, deployments and tooling help', memberCount: 2 },
  { id: 'pv-announcements', title: 'Announcements', slug: 'announcements', description: 'Company news and updates', memberCount: 3 },
];

const messages: PreviewMessage[] = [
  { id: 'pv-msg-1', conversationId: 'pv-general', senderName: 'Maya Chen', body: 'Welcome to the new local-first workspace. Please share feedback here.', sentAt: minsAgo(30), replyToId: null, threadId: 'pv-msg-1' },
  { id: 'pv-msg-2', conversationId: 'pv-general', senderName: 'Diego Alvarez', body: 'I added the deployment notes to the operations folder.', sentAt: minsAgo(21), replyToId: null, threadId: 'pv-msg-2' },
  { id: 'pv-msg-3', conversationId: 'pv-general', senderName: 'Preview User', body: 'Thanks — I will review the notes this afternoon.', sentAt: minsAgo(12), replyToId: null, threadId: 'pv-msg-3' },
  { id: 'pv-msg-4', conversationId: 'pv-it-support', senderName: 'Diego Alvarez', body: 'Server maintenance will start at 10 PM tonight.', sentAt: minsAgo(26), replyToId: null, threadId: 'pv-msg-4' },
  { id: 'pv-reply-1', conversationId: 'pv-it-support', senderName: 'Nina Patel', body: 'Noted, will monitor.', sentAt: minsAgo(16), replyToId: 'pv-msg-4', threadId: 'pv-msg-4' },
  { id: 'pv-reply-2', conversationId: 'pv-it-support', senderName: 'Preview User', body: 'I will handle the database.', sentAt: minsAgo(8), replyToId: 'pv-msg-4', threadId: 'pv-msg-4' },
  // A reply that answers a reply still belongs to the root thread.
  { id: 'pv-reply-3', conversationId: 'pv-it-support', senderName: 'Diego Alvarez', body: 'Agreed, I will confirm once the window closes.', sentAt: minsAgo(5), replyToId: 'pv-reply-1', threadId: 'pv-msg-4' },
];

const reactionMap: Record<string, ReactionSummary[]> = {
  'pv-msg-4': [{ emoji: '👍', count: 2, reactedByMe: true }],
};

const contacts: Contact[] = [
  { id: 'pv-contact-maya', name: 'Maya Chen', email: 'maya@northstar.test', department: 'Product', position: 'Product Lead', favorite: true },
  { id: 'pv-contact-diego', name: 'Diego Alvarez', email: 'diego@northstar.test', department: 'Operations', position: 'Supervisor', favorite: false },
  { id: 'pv-contact-priya', name: 'Priya Nair', email: 'priya@northstar.test', department: 'Research', position: 'Researcher', favorite: false },
  { id: 'pv-contact-nina', name: 'Nina Patel', email: 'nina@northstar.test', department: 'Accounting', position: 'Accountant', favorite: false },
];

const accounts: Account[] = [
  { id: 'pv-account', displayName: 'Preview Account', emailAddress: 'preview@relay.local', status: 'offline', connectionState: 'unverified', connectionError: null, lastVerifiedAt: null },
];

const notificationPrefs: NotificationPreference[] = [
  { key: 'email', enabled: true }, { key: 'messenger', enabled: true }, { key: 'mentions', enabled: true },
  { key: 'thread_replies', enabled: true }, { key: 'sounds', enabled: false },
];

const settingsStore: Record<string, string> = { theme: 'system', density: 'comfortable', font_size: '13', 'presence.status': 'online', 'presence.since': nowIso() };
const notifications: AppNotification[] = [
  { id: 'pv-notif-1', kind: 'message', title: 'New message in #general', body: 'Maya Chen: Welcome to the new local-first workspace.', entityType: 'conversation', entityId: 'pv-general', createdAt: minsAgo(30) },
];

function summaryFor(email: PreviewEmail): EmailSummary {
  const display = email.direction === 'outbound' ? (email.to[0] ?? email.senderName) : email.senderName;
  return { id: email.id, senderName: display, subject: email.subject, preview: email.bodyText.slice(0, 120), receivedAt: email.receivedAt, isRead: email.isRead, isStarred: email.isStarred };
}
function detailFor(email: PreviewEmail): EmailDetail {
  return { ...summaryFor(email), accountId: email.accountId, direction: email.direction, deliveryState: email.deliveryState, senderEmail: email.senderEmail, bodyText: email.bodyText, to: email.to, cc: email.cc, bcc: email.bcc };
}
function mailList(role: string): EmailSummary[] {
  return emails
    .filter(email => role === 'drafts' || role === 'sent'
      ? email.direction === 'outbound' && email.folderRole === role
      : email.direction === 'inbound' && email.folderRole === role)
    .map(summaryFor).sort((a, b) => (b.receivedAt > a.receivedAt ? 1 : -1));
}
// Channel history mirrors the native query: one row per thread root, pinned
// threads first, then chronological. Each row carries its thread's reply summary
// and aggregate reactions; replies load lazily through `get_message_thread`.
function channelMessages(channelId: string): ChatMessage[] {
  return messages
    .filter(m => m.conversationId === channelId && m.threadId === m.id)
    .map(m => { const replies = threadReplies(m.id); return toChat(m, { replyCount: replies.length, lastReplyAt: replies.length > 0 ? replies[replies.length - 1].sentAt : null, reactions: reactionMap[m.id] ?? [] }); })
    .sort((a, b) => (Number(b.pinned) - Number(a.pinned)) || (a.pinned ? (a.sentAt < b.sentAt ? 1 : -1) : (a.sentAt > b.sentAt ? 1 : -1)));
}

const conversations: PreviewConversation[] = [
  { id: 'pv-general', kind: 'channel', title: 'General', description: 'Company-wide announcements and discussion', memberCount: 4, lastActivityAt: minsAgo(30) },
  { id: 'pv-it-support', kind: 'channel', title: 'IT Support', description: 'Infrastructure, access and maintenance', memberCount: 3, lastActivityAt: minsAgo(95) },
  { id: 'pv-dm-maya', kind: 'dm', title: 'Maya Chen', description: null, memberCount: 2, lastActivityAt: minsAgo(18) },
];
messages.push({ id: 'pv-dm-msg-1', conversationId: 'pv-dm-maya', senderName: 'Maya Chen', body: 'Are you joining the Q3 review call?', sentAt: minsAgo(18), replyToId: null, threadId: 'pv-dm-msg-1', pinnedAt: null });

export async function previewInvoke<T>(command: string, args: Record<string, unknown> | undefined): Promise<T> {
  const arg = (key: string): unknown => (args ?? {})[key];
  const foundEmail = (id: unknown) => emails.find(email => email.id === id);
  const notFound = (what: string): Error => new Error(`Preview mode: ${what} was not found.`);

  switch (command) {
    case 'get_inbox': return mailList('inbox') as T;
    case 'get_starred': return emails.filter(e => e.isStarred).map(summaryFor) as T;
    case 'get_emails_in_folder': return mailList(String(arg('role'))) as T;
    case 'get_email': {
      const email = foundEmail(arg('id'));
      return (email ? detailFor(email) : null) as T;
    }
    case 'get_email_thread': {
      const current = foundEmail(arg('id'));
      if (!current || !current.threadId) return [] as T;
      return emails.filter(email => email.threadId !== null && email.threadId === current.threadId && email.accountId === current.accountId)
        .map(summaryFor).sort((x, y) => (x.receivedAt > y.receivedAt ? 1 : -1)) as T;
    }
    // Preview mail is local fixture data: there is no server to download a body
    // from, and every preview message already carries its text.
    case 'fetch_email_body': return '' as T;
    case 'mark_email_read': { foundEmail(arg('id'))!.isRead = arg('isRead') === true; return undefined as T; }
    case 'set_email_star': { foundEmail(arg('id'))!.isStarred = arg('isStarred') === true; return undefined as T; }
    case 'archive_email': { foundEmail(arg('id'))!.folderRole = 'archive'; return undefined as T; }
    case 'trash_email': { foundEmail(arg('id'))!.folderRole = 'trash'; return undefined as T; }
    case 'get_accounts': return accounts as T;
    // Preview cannot perform a real IMAP sign-in from a browser; the account is
    // recorded as connected and no secret is kept (none ever reaches this store).
    case 'add_email_account': {
      const input = arg('input') as { displayName: string; emailAddress: string };
      if (!String(arg('password') ?? '').trim()) throw new Error('Enter the account password to sign in.');
      const created: Account = { id: uid(), displayName: input.displayName, emailAddress: input.emailAddress, status: 'offline', connectionState: 'connected', connectionError: null, lastVerifiedAt: nowIso() };
      accounts.push(created);
      return created as T;
    }
    case 'test_email_connection': {
      const account = accounts.find(item => item.id === arg('id'));
      if (!account) throw new Error('This account no longer exists.');
      account.connectionState = 'connected';
      account.connectionError = null;
      account.lastVerifiedAt = nowIso();
      return { ...account } as T;
    }
    case 'remove_account': {
      const index = accounts.findIndex(a => a.id === arg('id'));
      if (index >= 0) accounts.splice(index, 1);
      return undefined as T;
    }
    // Google sign-in cannot run in a browser (no native loopback listener and no
    // OS credential manager), so preview records the account and keeps no
    // secret. The client id/secret checks mirror the native ones so the entry
    // experience is the same in both hosts.
    case 'google_oauth_sign_in': {
      const clientId = String(arg('clientId') ?? '').trim();
      if (!clientId) throw new Error('Paste the Google OAuth client id first (Cloud Console → APIs & Services → Credentials).');
      if (clientId.length < 32 || clientId.length > 200) throw new Error('That does not look like an OAuth client id: the value has the wrong length (they are about 72 characters).');
      if (!clientId.endsWith('.apps.googleusercontent.com')) throw new Error('The OAuth client id must end with .apps.googleusercontent.com.');
      const secret = String(arg('clientSecret') ?? '').trim();
      if (secret.endsWith('.apps.googleusercontent.com')) throw new Error('That is the OAuth client id, not the client secret. Put it in the Client ID field.');
      settingsStore['oauth.client_id'] = clientId;
      const address = 'preview@gmail.com';
      const known = accounts.find(account => account.emailAddress === address);
      if (known) { known.connectionState = 'connected'; known.connectionError = null; known.lastVerifiedAt = nowIso(); return { ...known } as T; }
      const created: Account = { id: uid(), displayName: 'Preview User', emailAddress: address, status: 'offline', connectionState: 'connected', connectionError: null, lastVerifiedAt: nowIso() };
      accounts.push(created);
      return created as T;
    }
    // Forgetting the client removes the remembered id (the browser preview has no
    // keyring, so there is no stored secret to delete).
    case 'forget_oauth_client_id': {
      delete settingsStore['oauth.client_id'];
      return undefined as T;
    }
    // The browser preview has no loopback listener, so there is nothing to
    // cancel — accepted so the login screen's Cancel action behaves the same in
    // both hosts.
    case 'cancel_google_sign_in': return undefined as T;
    // The browser preview has no IMAP connection, so there is nothing to fetch.
    // Returning 0 (rather than throwing) keeps the Mail view's Sync button
    // usable in preview mode, where the store is in-memory only. As natively,
    // omitting the ids means "every account".
    case 'sync_mail': return 0 as T;
    // Thunderbird-style sign-in: the browser preview records the address the user
    // typed; real host/port discovery only exists in the native build.
    case 'auto_sign_in': {
      const email = String(arg('email') ?? '').trim();
      if (!email.includes('@') || !String(arg('password') ?? '').trim()) throw new Error('Enter your email address and password — Relay finds the rest.');
      const known = accounts.find(account => account.emailAddress === email);
      if (known) { known.connectionState = 'connected'; known.connectionError = null; known.lastVerifiedAt = nowIso(); return { ...known } as T; }
      const created: Account = { id: uid(), displayName: email.split('@')[0], emailAddress: email, status: 'offline', connectionState: 'connected', connectionError: null, lastVerifiedAt: nowIso() };
      accounts.push(created);
      return created as T;
    }

    case 'save_draft': {
      const input = arg('input') as { accountId: string; to: string[]; cc: string[]; bcc: string[]; subject: string; bodyText: string };
      const existing = emails.find(email => email.id === arg('draftId'));
      const email = existing ?? { id: uid(), accountId: input.accountId, direction: 'outbound' as const, deliveryState: 'draft' as const, folderRole: 'drafts' as const, senderName: 'Preview Account', senderEmail: 'preview@relay.local', subject: '', bodyText: '', threadId: null, receivedAt: nowIso(), isRead: true, isStarred: false, to: [], cc: [], bcc: [] };
      if (!existing) emails.push(email);
      email.subject = input.subject; email.bodyText = input.bodyText; email.to = input.to; email.cc = input.cc; email.bcc = input.bcc; email.receivedAt = nowIso();
      return { id: email.id, deliveryState: 'draft', updatedAt: nowIso() } as T;
    }
    // Queueing mirrors the native store: only an outbound record with at least
    // one To recipient can be queued (the same `Validation` the Rust repository
    // raises), and the copy moves into Sent — otherwise a send would silently
    // disappear from the browser preview (BUG-023).
    case 'queue_email_send': {
      const email = foundEmail(arg('id'));
      if (!email) throw notFound('email');
      if (email.direction !== 'outbound' || email.to.length === 0) throw new Error('Add at least one valid recipient before sending.');
      email.deliveryState = 'queued';
      email.folderRole = 'sent';
      return { id: email.id, deliveryState: 'queued', updatedAt: nowIso() } as T;
    }
    case 'get_channels': return channels.map(channel => ({ id: channel.id, title: channel.title, slug: channel.slug, description: channel.description, memberCount: channel.memberCount })) as T;
    case 'get_conversations': return [...conversations].sort((a, b) => (b.lastActivityAt > a.lastActivityAt ? 1 : -1)) as T;
    case 'open_direct_message': {
      const contact = contacts.find(item => item.id === arg('contactId'));
      if (!contact) throw notFound('contact');
      let dm = conversations.find(item => item.kind === 'dm' && item.title === contact.name);
      if (!dm) {
        dm = { id: uid(), kind: 'dm', title: contact.name, description: null, memberCount: 2, lastActivityAt: nowIso() };
        conversations.push(dm);
      }
      return { ...dm, lastActivityAt: nowIso() } as T;
    }
    case 'get_channel_messages': return channelMessages(String(arg('channelId'))) as T;
    case 'send_company_message': {
      const input = arg('input') as { conversationId: string; body: string; replyToId?: string };
      const parent = input.replyToId ? messages.find(m => m.id === input.replyToId && m.conversationId === input.conversationId) : undefined;
      if (input.replyToId && !parent) throw notFound('message');
      const id = uid();
      // A reply always joins the thread of the message it answers; a top-level
      // message starts a thread of its own.
      const message: PreviewMessage = { id, conversationId: input.conversationId, senderName: SELF, body: input.body, sentAt: nowIso(), replyToId: input.replyToId ?? null, threadId: parent ? parent.threadId : id };
      messages.push(message);
      const conversation = conversations.find(item => item.id === input.conversationId);
      if (conversation) conversation.lastActivityAt = message.sentAt;
      return toChat(message) as T;
    }
    case 'edit_message': {
      const message = messages.find(m => m.id === arg('id'));
      if (!message) throw notFound('message');
      message.body = String(arg('body'));
      message.editedAt = nowIso();
      // An edited reply still reports its thread's aggregate reactions.
      return toChat(message, { reactions: reactionMap[message.threadId] ?? [] }) as T;
    }
    case 'delete_message': {
      const id = String(arg('id'));
      const target = messages.find(m => m.id === id);
      if (!target) throw notFound('message');
      // Deleting a thread root removes the whole thread (the history only lists
      // roots, so orphaned replies would be unreachable); deleting a reply keeps
      // the rest of the thread.
      const doomed = new Set(target.threadId === id ? messages.filter(m => m.threadId === id).map(m => m.id) : [id]);
      for (let index = messages.length - 1; index >= 0; index -= 1) { if (doomed.has(messages[index].id)) messages.splice(index, 1); }
      return undefined as T;
    }
    case 'set_message_pin': {
      const message = messages.find(m => m.id === arg('id'));
      if (!message) throw notFound('message');
      message.pinnedAt = arg('pinned') === true ? nowIso() : null;
      return undefined as T;
    }
    case 'get_message_thread': {
      const messageId = String(arg('rootId'));
      const target = messages.find(m => m.id === messageId);
      const rootId = target?.threadId ?? messageId;
      return threadReplies(rootId).filter(reply => reply.id !== messageId).map(reply => toChat(reply)) as T;
    }
    case 'get_channel_reactions': {
      const channelId = String(arg('channelId'));
      const rows: MessageReaction[] = [];
      // Reactions hang off thread roots, so only those rows can carry aggregates.
      for (const message of messages.filter(m => m.conversationId === channelId && m.threadId === m.id)) {
        for (const reaction of reactionMap[message.id] ?? []) rows.push({ messageId: message.id, emoji: reaction.emoji, count: reaction.count, reactedByMe: reaction.reactedByMe });
      }
      return rows as T;
    }
    case 'toggle_message_reaction': {
      const messageId = String(arg('messageId'));
      const emoji = String(arg('emoji'));
      const target = messages.find(m => m.id === messageId);
      if (!target) throw notFound('message');
      // Reactions belong to the thread: a reaction on a reply is stored on the root.
      const rootId = target.threadId;
      const current = reactionMap[rootId] ?? [];
      const mine = current.findIndex(r => r.emoji === emoji && r.reactedByMe);
      if (mine >= 0) { current.splice(mine, 1); }
      else if (current.some(r => r.emoji === emoji)) { const existing = current.find(r => r.emoji === emoji)!; existing.count += 1; }
      else { current.push({ emoji, count: 1, reactedByMe: true }); }
      reactionMap[rootId] = current;
      return current as T;
    }

    case 'get_sync_overview': return ({ state: 'offline', pending: 0, failed: 0, conflicts: 0, lastSyncAt: minsAgo(2), detail: 'Preview mode: browser-only store, no native sync.' }) as T;
    case 'retry_failed_sync': return 0 as T;
    case 'get_notifications': return notifications as T;
    case 'get_unread_notification_count': return notifications.filter(n => !(n.id.startsWith('read-'))).length as T;
    case 'mark_all_notifications_read': return undefined as T;
    case 'get_notification_preferences': return notificationPrefs as T;
    case 'update_notification_preference': {
      const input = arg('input') as { key: string; enabled: boolean };
      const pref = notificationPrefs.find(p => p.key === input.key);
      if (pref) pref.enabled = input.enabled;
      return undefined as T;
    }
    case 'show_native_notification': return undefined as T;
    case 'get_app_settings': return Object.entries(settingsStore).map(([key, value]) => ({ key, value })) as T;
    case 'set_app_setting': { settingsStore[String(arg('key'))] = String(arg('value')); return undefined as T; }
    case 'get_presence': return { status: settingsStore['presence.status'] ?? 'offline', since: settingsStore['presence.since'] ?? nowIso() } as T;
    case 'set_presence': {
      const status = String(arg('status'));
      settingsStore['presence.status'] = status; settingsStore['presence.since'] = nowIso();
      return { status, since: nowIso() } as T;
    }
    case 'search_global': return searchPreview(String(arg('query'))) as T;
    case 'get_contacts': return contacts as T;
    case 'create_contact': {
      const input = arg('input') as { name: string; email?: string | null; department?: string | null; position?: string | null };
      const created: Contact = { id: uid(), name: input.name, email: input.email ?? null, department: input.department ?? null, position: input.position ?? null, favorite: false };
      contacts.push(created);
      return created as T;
    }
    case 'set_contact_favorite': {
      const contact = contacts.find(item => item.id === arg('id'));
      if (contact) contact.favorite = arg('favorite') === true;
      return undefined as T;
    }
    case 'get_storage_usage': return { database: 0, attachments: 0, avatars: 0, cache: 0, logs: 0, total: 0 } as T;
    case 'clear_cache': return 0 as T;
    default: { throw new Error(`Preview mode has no handler for "${command}".`); }
  }
}

function searchPreview(query: string): SearchResult[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [];
  const results: SearchResult[] = [];
  for (const email of emails) {
    if ((email.subject + ' ' + email.senderName + ' ' + email.senderEmail).toLowerCase().includes(needle)) results.push({ kind: 'email', id: email.id, title: email.subject, subtitle: email.senderName });
  }
  for (const message of messages) {
    if (!message.body.toLowerCase().includes(needle)) continue;
    const channel = channels.find(c => c.id === message.conversationId);
    results.push({ kind: 'message', id: message.conversationId, title: `#${channel?.slug ?? 'channel'}`, subtitle: message.body.slice(0, 80) });
  }
  for (const contact of contacts) {
    if ((contact.name + ' ' + (contact.email ?? '') + ' ' + (contact.department ?? '') + ' ' + (contact.position ?? '')).toLowerCase().includes(needle)) results.push({ kind: 'contact', id: contact.id, title: contact.name, subtitle: contact.email ?? contact.department ?? '' });
  }
  for (const channel of channels) {
    if ((channel.title + ' ' + channel.slug + ' ' + channel.description).toLowerCase().includes(needle)) results.push({ kind: 'channel', id: channel.id, title: channel.title, subtitle: channel.description });
  }
  return results.slice(0, 16);
}