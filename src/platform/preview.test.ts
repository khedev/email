import { describe, expect, it } from 'vitest';
import { previewInvoke } from './preview';
import type { Account, ChatMessage, Draft, EmailDetail, EmailSummary, MessageReaction, ReactionSummary, SettingsEntry } from './tauri';

// The browser preview backend has to mirror the native thread contract
// (`thread_id` grouping, reactions on the thread root, delete cascade), or the
// UI would behave differently in the browser preview than in the desktop app.
// Every test restores the state it changed so the shared in-memory store stays
// deterministic whatever the run order.
function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> { return previewInvoke<T>(command, args); }
function history(conversationId: string): Promise<ChatMessage[]> { return invoke<ChatMessage[]>('get_channel_messages', { channelId: conversationId }); }
function threadOf(messageId: string): Promise<ChatMessage[]> { return invoke<ChatMessage[]>('get_message_thread', { rootId: messageId }); }

describe('preview messenger threads', () => {
  it('lists one row per thread with its reply summary and aggregate reactions', async () => {
    const rows = await history('pv-it-support');
    expect(rows).toHaveLength(1);
    const root = rows[0];
    expect(root.id).toBe('pv-msg-4');
    expect(root.threadId).toBe('pv-msg-4');
    expect(root.replyCount).toBe(3);
    expect(root.lastReplyAt).not.toBeNull();
    expect(root.reactions.map(reaction => reaction.emoji)).toEqual(['👍']);
    const general = await history('pv-general');
    expect(general.map(row => row.id)).toEqual(['pv-msg-1', 'pv-msg-2', 'pv-msg-3']);
    expect(general.every(row => row.replyCount === 0 && row.lastReplyAt === null)).toBe(true);
  });

  it('resolves any thread member to the same replies', async () => {
    expect((await threadOf('pv-msg-4')).map(reply => reply.id)).toEqual(['pv-reply-1', 'pv-reply-2', 'pv-reply-3']);
    // Asking from a reply returns the same thread without duplicating that reply.
    expect((await threadOf('pv-reply-1')).map(reply => reply.id)).toEqual(['pv-reply-2', 'pv-reply-3']);
  });

  it('keeps a reply to a reply inside the original thread', async () => {
    const reply = await invoke<ChatMessage>('send_company_message', { input: { conversationId: 'pv-it-support', body: 'nested check', replyToId: 'pv-reply-1' } });
    expect(reply.threadId).toBe('pv-msg-4');
    expect(reply.replyCount).toBe(0);
    expect(await history('pv-it-support')).toHaveLength(1);
    expect((await threadOf('pv-msg-4')).some(item => item.id === reply.id)).toBe(true);
    await invoke('delete_message', { id: reply.id });
    expect((await threadOf('pv-msg-4')).map(item => item.id)).toEqual(['pv-reply-1', 'pv-reply-2', 'pv-reply-3']);
  });

  it('stores reactions on the thread root even when they are clicked on a reply', async () => {
    const summary = await invoke<ReactionSummary[]>('toggle_message_reaction', { messageId: 'pv-reply-2', emoji: '🎉' });
    expect(summary.map(row => row.emoji)).toEqual(['👍', '🎉']);
    const aggregates = await invoke<MessageReaction[]>('get_channel_reactions', { channelId: 'pv-it-support' });
    expect(aggregates.every(row => row.messageId === 'pv-msg-4')).toBe(true);
    expect((await history('pv-it-support'))[0].reactions.map(row => row.emoji)).toEqual(['👍', '🎉']);
    // Toggling again removes the reaction, restoring the seeded state.
    await invoke('toggle_message_reaction', { messageId: 'pv-reply-2', emoji: '🎉' });
    expect((await history('pv-it-support'))[0].reactions.map(row => row.emoji)).toEqual(['👍']);
  });

  it('rejects a reply whose target does not exist', async () => {
    await expect(invoke('send_company_message', { input: { conversationId: 'pv-it-support', body: 'orphan', replyToId: 'missing-id' } })).rejects.toThrow(/not found/i);
  });

  it('deletes a whole thread when its root is deleted', async () => {
    const root = await invoke<ChatMessage>('send_company_message', { input: { conversationId: 'pv-dm-maya', body: 'disposable thread' } });
    await invoke<ChatMessage>('send_company_message', { input: { conversationId: 'pv-dm-maya', body: 'disposable reply', replyToId: root.id } });
    expect((await history('pv-dm-maya')).map(row => row.id)).toContain(root.id);
    await invoke('delete_message', { id: root.id });
    expect((await history('pv-dm-maya')).map(row => row.id)).not.toContain(root.id);
    expect(await threadOf(root.id)).toHaveLength(0);
  });
});

// The preview host has no loopback listener or OS keyring, so Google sign-in is
// simulated — but its validation, the remembered client id and the "forget"
// action must behave exactly like the desktop contract, or the login screen
// would diverge between the two hosts.
describe('preview Google OAuth parity', () => {
  const clientId = '1234567890-abcdefghijklmnopqrstuvwxyz123456.apps.googleusercontent.com';

  it('validates the client id before recording anything', async () => {
    await expect(invoke('google_oauth_sign_in', { clientId: '' })).rejects.toThrow(/client id/i);
    await expect(invoke('google_oauth_sign_in', { clientId: 'abc.apps.googleusercontent.com' })).rejects.toThrow(/wrong length/i);
    await expect(invoke('google_oauth_sign_in', { clientId: '1234567890-abcdefghijklmnopqrstuvwxyz12' })).rejects.toThrow(/apps\.googleusercontent\.com/);
    await expect(invoke('google_oauth_sign_in', { clientId, clientSecret: 'abc.apps.googleusercontent.com' })).rejects.toThrow(/not the client secret/i);
  });

  // The preview has no loopback listener, so there is nothing to cancel — but the
  // command must exist in both hosts or the login screen's Cancel action would
  // throw in the browser preview.
  it('accepts a cancel request', async () => {
    await expect(invoke('cancel_google_sign_in')).resolves.toBeUndefined();
  });

  // The Mail view wires a Sync button to `sync_mail`; the preview has no IMAP
  // connection, so it must report "nothing fetched" rather than throwing.
  it('reports no mail fetched for a sync request', async () => {
    await expect(invoke<number>('sync_mail', { accountId: 'pv-account' })).resolves.toBe(0);
    // No ids means "every account" — the UI no longer picks one.
    await expect(invoke<number>('sync_mail', { accountIds: null })).resolves.toBe(0);
  });

  it('remembers the client id, forgets it on request and is idempotent', async () => {
    const account = await invoke<Account>('google_oauth_sign_in', { clientId });
    expect(account.emailAddress).toBe('preview@gmail.com');
    const remembered = await invoke<SettingsEntry[]>('get_app_settings');
    expect(remembered.find(entry => entry.key === 'oauth.client_id')?.value).toBe(clientId);
    // Signing in again returns the same row instead of duplicating it.
    const again = await invoke<Account>('google_oauth_sign_in', { clientId });
    expect(again.id).toBe(account.id);
    await invoke('forget_oauth_client_id');
    const after = await invoke<SettingsEntry[]>('get_app_settings');
    expect(after.some(entry => entry.key === 'oauth.client_id')).toBe(false);
    // Restore the shared store for other tests.
    await invoke('remove_account', { id: account.id });
    expect((await invoke<Account[]>('get_accounts')).some(row => row.emailAddress === 'preview@gmail.com')).toBe(false);
  });
});

// The browser preview has to mirror the native send contract too: queueing files
// the copy into Sent and reports it as queued, never as delivered (BUG-023).
// The preview store has no delete command, so the rows these tests create stay in
// Sent — they are appended at the end of the file so nothing later depends on the
// fixture folder contents.
describe('preview local-only send', () => {
  it('files a queued send into Sent and never reports it as delivered', async () => {
    const draft = await invoke<Draft>('save_draft', { input: { accountId: 'pv-account', to: ['maya@northstar.test'], cc: [], bcc: [], subject: 'Preview send', bodyText: 'Body' }, draftId: null });
    const queued = await invoke<Draft>('queue_email_send', { id: draft.id });
    expect(queued.deliveryState).toBe('queued');
    expect((await invoke<EmailSummary[]>('get_emails_in_folder', { role: 'sent' })).some(row => row.id === draft.id)).toBe(true);
    expect((await invoke<EmailSummary[]>('get_emails_in_folder', { role: 'drafts' })).some(row => row.id === draft.id)).toBe(false);
    expect((await invoke<EmailDetail | null>('get_email', { id: draft.id }))?.deliveryState).toBe('queued');
  });

  it('refuses to queue a send with no To recipient', async () => {
    const draft = await invoke<Draft>('save_draft', { input: { accountId: 'pv-account', to: [], cc: ['maya@northstar.test'], bcc: [], subject: 'Cc only', bodyText: 'Body' }, draftId: null });
    await expect(invoke('queue_email_send', { id: draft.id })).rejects.toThrow('Add at least one valid recipient before sending.');
    expect((await invoke<EmailSummary[]>('get_emails_in_folder', { role: 'drafts' })).some(row => row.id === draft.id)).toBe(true);
  });
});

