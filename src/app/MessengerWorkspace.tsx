import { useEffect, useRef, useState } from 'react';
import { Check, Copy, Hash, MessageCircleReply, MessageSquare, Pencil, Pin, PinOff, SendHorizontal, Trash2, Users, X } from 'lucide-react';
import { deleteMessage, editMessage, getChannelMessages, getChannels, getConversations, getMessageThread, sendCompanyMessage, setMessagePin, toggleMessageReaction, type Channel, type ChatMessage, type Conversation, type ReactionSummary } from '../platform/tauri';
import { formatTime, initials } from './util';

const QUICK_REACTIONS = ['👍', '❤️', '🎉', '👀', '🙏'];

/** Mirrors the backend ordering of thread rows: pinned threads first (most
 *  recently pinned on top), then the rest chronologically. */
function sortThreads(rows: ChatMessage[]): ChatMessage[] {
  return [...rows].sort((a, b) => {
    if (a.pinned !== b.pinned) return a.pinned ? -1 : 1;
    if (a.pinned) return a.sentAt < b.sentAt ? 1 : -1;
    return a.sentAt > b.sentAt ? 1 : -1;
  });
}

/** "3 replies · last 14:20" — the summary a collapsed thread shows. */
function threadLabel(message: ChatMessage): string {
  const count = message.replyCount === 1 ? '1 reply' : `${message.replyCount} replies`;
  return message.lastReplyAt ? `${count} · last ${formatTime(message.lastReplyAt)}` : count;
}

export function MessengerWorkspace({ focusChannel, focusConversation, initialError }: { focusChannel?: string; focusConversation?: string; initialError?: string }) {
  const [channels, setChannels] = useState<Channel[]>([]);
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [active, setActive] = useState<string>();
  const [copiedId, setCopiedId] = useState('');
  const [editing, setEditing] = useState<{ id: string; body: string } | null>(null);
  const [items, setItems] = useState<ChatMessage[]>([]);
  const [openThreadId, setOpenThreadId] = useState('');
  const [threads, setThreads] = useState<Record<string, ChatMessage[]>>({});
  const [body, setBody] = useState('');
  const [threadBody, setThreadBody] = useState('');
  const [error, setError] = useState(initialError ?? '');
  const historyRef = useRef<HTMLDivElement | null>(null);
  const loadedRef = useRef(false);

  useEffect(() => {
    getChannels().then(result => {
      setChannels(result);
      const preferred = focusChannel && result.some(channel => channel.id === focusChannel) ? focusChannel : undefined;
      if (preferred) setActive(preferred);
    }).catch(err => { console.error('[relay] load channels failed:', err); setError('Unable to load local channels.'); });
    getConversations().then(result => {
      setConversations(result);
      const dm = focusConversation && result.some(item => item.id === focusConversation) ? focusConversation : undefined;
      if (dm) setActive(dm); else setActive(current => current ?? result[0]?.id);
    }).catch(err => { console.error('[relay] load conversations failed:', err); });
  }, [focusChannel, focusConversation]);

  useEffect(() => {
    if (!active) return;
    setItems([]); setThreads({}); setOpenThreadId(''); setEditing(null); setThreadBody('');
    // The first conversation selection keeps an initial error (e.g. a direct
    // message that failed to open) visible; later switches dismiss it.
    if (loadedRef.current) setError(''); else loadedRef.current = true;
    getChannelMessages(active).then(messages => {
      setItems(messages);
      requestAnimationFrame(() => { historyRef.current?.scrollTo({ top: historyRef.current.scrollHeight, behavior: 'instant' }); });
    }).catch(err => { console.error('[relay] load messages failed:', active, err); setError('Unable to load local messages.'); });
  }, [active]);

  const channel = channels.find(item => item.id === active);
  const conversation = conversations.find(item => item.id === active);
  const isDirect = Boolean(conversation);
  // Only true direct messages belong under the DIRECT MESSAGES heading; the
  // conversations query also returns channel rows (their titles update from
  // member activity), which would otherwise appear twice in this sidebar.
  const directChats = conversations.filter(item => item.kind === 'dm');

  const toggleReaction = async (message: ChatMessage, emoji: string) => {
    try {
      // Reactions are stored on the thread root, so the returned aggregate always
      // belongs to the root row of this thread — even when the button was clicked
      // on a reply inside the thread panel.
      const summary = await toggleMessageReaction(message.id, emoji);
      setItems(rows => rows.map(row => row.id === message.threadId ? { ...row, reactions: summary } : row));
    } catch (err) { console.error('[relay] reaction toggle failed:', message.id, emoji, err); setError('Reaction could not be saved locally.'); }
  };

  const toggleThread = async (root: ChatMessage) => {
    const show = openThreadId !== root.id;
    setOpenThreadId(show ? root.id : '');
    setThreadBody('');
    if (show && !threads[root.id]) {
      try { const replies = await getMessageThread(root.id); setThreads(current => ({ ...current, [root.id]: replies })); }
      catch (err) { console.error('[relay] thread load failed:', root.id, err); setError('Thread could not be loaded.'); }
    }
  };

  const togglePin = async (message: ChatMessage) => {
    try {
      await setMessagePin(message.id, !message.pinned);
      setItems(rows => sortThreads(rows.map(row => row.id === message.id ? { ...row, pinned: !message.pinned } : row)));
    } catch (err) { console.error('[relay] pin toggle failed:', message.id, err); setError('Pin could not be saved locally.'); }
  };

  const removeMessage = async (message: ChatMessage) => {
    const isRoot = message.threadId === message.id;
    try {
      await deleteMessage(message.id);
      // The native delete cascades a thread root (the history only lists roots),
      // while deleting a reply leaves the rest of the thread in place.
      setItems(rows => isRoot
        ? rows.filter(row => row.id !== message.id)
        : rows.map(row => row.id === message.threadId ? { ...row, replyCount: Math.max(row.replyCount - 1, 0) } : row));
      setThreads(current => {
        const next: Record<string, ChatMessage[]> = {};
        for (const [rootId, replies] of Object.entries(current)) {
          if (isRoot && rootId === message.id) continue;
          next[rootId] = replies.filter(reply => reply.id !== message.id);
        }
        return next;
      });
      if (isRoot && openThreadId === message.id) setOpenThreadId('');
    } catch (err) { console.error('[relay] message delete failed:', message.id, err); setError('Message could not be deleted.'); }
  };

  const saveEdit = async () => {
    const target = editing?.id;
    const next = editing?.body.trim() ?? '';
    if (!target || !next) return;
    try {
      const updated = await editMessage(target, next);
      setItems(rows => rows.map(row => row.id === target ? { ...row, ...updated } : row));
      setThreads(current => {
        const next: Record<string, ChatMessage[]> = {};
        for (const [rootId, replies] of Object.entries(current)) next[rootId] = replies.map(reply => reply.id === target ? { ...reply, ...updated } : reply);
        return next;
      });
      setEditing(null);
    } catch (err) { console.error('[relay] message edit failed:', target, err); setError('Edit could not be saved locally.'); }
  };

  const copyMessage = async (message: ChatMessage) => {
    try { await navigator.clipboard.writeText(message.body); setCopiedId(message.id); setTimeout(() => setCopiedId(current => (current === message.id ? '' : current)), 1500); }
    catch (err) { console.error('[relay] clipboard copy failed:', message.id, err); setError('Copy is unavailable in this context.'); }
  };

  const send = async (event?: { preventDefault?: () => void }) => {
    event?.preventDefault?.();
    if (!active || !body.trim()) return;
    try {
      // A top-level message starts a thread of its own, so it is appended to the
      // history as a new thread row.
      const message = await sendCompanyMessage({ conversationId: active, body });
      setItems(current => sortThreads([...current, message]));
      setBody('');
      requestAnimationFrame(() => { historyRef.current?.scrollTo({ top: historyRef.current.scrollHeight, behavior: 'instant' }); });
    } catch (err) { console.error('[relay] message send failed:', active, err); setError('Message could not be saved locally.'); }
  };

  // Replies are written from the thread panel only: the thread a reply belongs to
  // is then always explicit, so a reply can never open a second thread.
  const sendReply = async (event?: { preventDefault?: () => void }) => {
    event?.preventDefault?.();
    const root = openThreadId;
    if (!active || !root || !threadBody.trim()) return;
    try {
      const reply = await sendCompanyMessage({ conversationId: active, body: threadBody, replyToId: root });
      setThreads(current => ({ ...current, [root]: [...(current[root] ?? []), reply] }));
      setItems(rows => rows.map(row => row.id === root ? { ...row, replyCount: row.replyCount + 1, lastReplyAt: reply.sentAt } : row));
      setThreadBody('');
    } catch (err) { console.error('[relay] reply send failed:', root, err); setError('Reply could not be saved locally.'); }
  };

  return <section className="messenger-workspace"><aside className="channel-list">
    <p className="eyebrow">DIRECT MESSAGES</p>
    {directChats.map(item => (
      <button key={item.id} className={item.id === active ? 'channel active-channel' : 'channel'} onClick={() => setActive(item.id)} title={item.kind === 'dm' ? 'Direct message' : 'Conversation'}>
        <span className="dm-avatar">{initials(item.title)}</span><span className="dm-name">{item.title}</span>
      </button>
    ))}
    {directChats.length === 0 && <p className="muted">No direct messages yet — start one from Contacts.</p>}
    <p className="eyebrow">CHANNELS</p>
    {channels.map(item => <button key={item.id} className={item.id === active ? 'channel active-channel' : 'channel'} onClick={() => setActive(item.id)}><Hash size={16} /><span>{item.slug ?? item.title}</span></button>)}
    {channels.length === 0 && <p className="muted">No local channels yet.</p>}
  </aside>
  <article className="chat">
    <header className="chat-heading">
      <div>{isDirect ? <h1>{conversation?.title ?? 'Direct message'}</h1> : <h1><Hash size={20} />{channel?.slug ?? 'Messages'}</h1>}<p>{isDirect ? 'Direct message · private conversation' : channel?.description ?? 'Local company conversation'}</p></div>
      {!isDirect && <span><Users size={16} />{channel?.memberCount ?? 0}</span>}
    </header>
    <div className="chat-history" ref={historyRef}>
      {error && <p className="load-error">{error}</p>}
      {items.length === 0 && !error && <div className="empty"><MessageCircleReply size={26} /><h2>No messages yet</h2><p>Start a conversation in this channel.</p></div>}
      {items.map(message => (
        <div className={`thread-root${openThreadId === message.id ? ' thread-open' : ''}`} key={message.id}>
          {message.pinned && <p className="pin-flag"><Pin size={12} /> Pinned</p>}
          {editing?.id === message.id
            ? <InlineEdit value={editing.body} onChange={value => setEditing({ id: message.id, body: value })} onSave={saveEdit} onCancel={() => setEditing(null)} label="Edit message" />
            : <ChatBubble message={message} />}
          <div className="message-tools">
            {reactionChips(message.reactions ?? [], emoji => void toggleReaction(message, emoji))}
            <button className="react" onClick={() => void toggleReaction(message, '👍')} title="React with 👍">👍 React</button>
            <button className="react" onClick={() => void copyMessage(message)} title="Copy message text">{copiedId === message.id ? <Check size={13} /> : <Copy size={13} />}{copiedId === message.id ? ' Copied' : ' Copy'}</button>
            {message.mine && <button className="react" onClick={() => setEditing({ id: message.id, body: message.body })} title="Edit message"><Pencil size={13} /> Edit</button>}
            <button className="react" onClick={() => void togglePin(message)} title={message.pinned ? 'Unpin thread' : 'Pin thread'}>{message.pinned ? <PinOff size={13} /> : <Pin size={13} />}{message.pinned ? ' Unpin' : ' Pin'}</button>
            {message.mine && <button className="react danger" onClick={() => void removeMessage(message)} title="Delete message"><Trash2 size={13} /> Delete</button>}
            <button className={`replies${message.replyCount === 0 ? ' quiet' : ''}`} onClick={() => void toggleThread(message)} title={message.replyCount === 0 ? 'Open this thread' : 'Show this thread'}><MessageSquare size={13} />{openThreadId === message.id ? ' Hide thread' : ` ${threadLabel(message)}`}</button>
          </div>
          {openThreadId === message.id && <div className="thread-panel" aria-label="Thread">
            <header className="thread-heading">
              <span><MessageSquare size={14} />{message.replyCount === 1 ? '1 reply' : `${message.replyCount} replies`}</span>
              <div className="quick-reactions">{QUICK_REACTIONS.map(emoji => <button key={emoji} onClick={() => void toggleReaction(message, emoji)} title={`React to this thread with ${emoji}`}>{emoji}</button>)}</div>
            </header>
            {(threads[message.id] ?? []).map(reply => (
              <div className="thread-reply" key={reply.id}>
                {editing?.id === reply.id
                  ? <InlineEdit value={editing.body} onChange={value => setEditing({ id: reply.id, body: value })} onSave={saveEdit} onCancel={() => setEditing(null)} label="Edit reply" />
                  : <ChatBubble message={reply} reply />}
                <div className="message-tools">
                  <button className="react" onClick={() => void toggleReaction(reply, '👍')} title="React with 👍">👍 React</button>
                  <button className="react" onClick={() => void copyMessage(reply)} title="Copy reply text">{copiedId === reply.id ? <Check size={13} /> : <Copy size={13} />}{copiedId === reply.id ? ' Copied' : ' Copy'}</button>
                  {reply.mine && <button className="react" onClick={() => setEditing({ id: reply.id, body: reply.body })} title="Edit reply"><Pencil size={13} /> Edit</button>}
                  {reply.mine && <button className="react danger" onClick={() => void removeMessage(reply)} title="Delete reply"><Trash2 size={13} /> Delete</button>}
                </div>
              </div>
            ))}
            {!threads[message.id] && <p className="muted">Loading replies…</p>}
            {threads[message.id]?.length === 0 && <p className="muted">No replies yet.</p>}
            <form className="thread-composer" onSubmit={sendReply}>
              <textarea value={threadBody} onChange={event => setThreadBody(event.target.value)} placeholder="Reply in thread" aria-label="Reply in thread" onKeyDown={event => { if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); if (threadBody.trim()) void sendReply(); } }} />
              <button type="submit" aria-label="Send reply" disabled={!threadBody.trim()}><SendHorizontal size={16} /></button>
            </form>
          </div>}
        </div>
      ))}
    </div>
    <form className="chat-composer" onSubmit={send}>
      <textarea value={body} onChange={event => setBody(event.target.value)} placeholder={active ? `Message #${channel?.slug ?? channel?.title ?? 'conversation'}` : 'Select a channel'} disabled={!active} onKeyDown={event => { if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); if (body.trim()) void send(event); } }} />
      <button type="submit" aria-label="Send message" disabled={!body.trim() || !active}><SendHorizontal size={18} /></button>
    </form>
  </article></section>;
}

/** Shared inline editor for both thread rows and replies. */
function InlineEdit({ value, onChange, onSave, onCancel, label }: { value: string; onChange: (value: string) => void; onSave: () => void; onCancel: () => void; label: string }) {
  return <div className="inline-edit">
    <textarea value={value} onChange={event => onChange(event.target.value)} aria-label={label} onKeyDown={event => { if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); if (value.trim()) onSave(); } }} />
    <div>
      <button onClick={onSave} disabled={!value.trim()} title="Save changes"><Check size={14} /> Save</button>
      <button onClick={onCancel} title="Cancel editing"><X size={14} /> Cancel</button>
    </div>
  </div>;
}

function ChatBubble({ message, reply }: { message: ChatMessage; reply?: boolean }) {
  return <div className={`chat-message${reply ? ' reply' : ''}`}><span className="chat-avatar">{initials(message.senderName)}</span><div><strong>{message.senderName}</strong><time>{formatTime(message.sentAt)}{message.edited ? ' · edited' : ''}</time><p>{message.body}</p></div></div>;
}

/** Thread reactions always render on the root row, so one chip list is enough. */
function reactionChips(rows: ReactionSummary[], onToggle: (emoji: string) => void) {
  return <div className="reaction-chips">{rows.map(row => <button key={row.emoji} className={row.reactedByMe ? 'mine' : ''} onClick={() => onToggle(row.emoji)} title={`${row.count} reaction${row.count === 1 ? '' : 's'}`}>{row.emoji} {row.count}</button>)}</div>;
}
