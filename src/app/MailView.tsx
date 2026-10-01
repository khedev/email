import { useEffect, useRef, useState } from 'react';
import { Archive, ArrowLeft, CornerDownLeft, Forward, Mail, MoreHorizontal, RefreshCw, Send, Star, Trash2 } from 'lucide-react';
import { archiveEmail, commandError, deliverQueuedMail, fetchEmailBody, getEmail, getEmailThread, getEmailsInFolder, getStarred, markEmailRead, setEmailStar, syncMail, trashEmail, type EmailDetail, type EmailSummary, type MailRole } from '../platform/tauri';
import { formatDate, initials } from './util';
import { PaneSplitter } from './PaneSplitter';

export type AppView = 'Inbox' | 'Starred' | 'Sent' | 'Drafts' | 'Archive' | 'Trash' | 'Messages' | 'Threads' | 'Channels' | 'Contacts' | 'Directory' | 'Settings';
export interface ComposeInitial { draftId?: string; to: string[]; cc: string[]; bcc: string[]; subject: string; bodyText: string; }
export type MailViewKind = 'Inbox' | 'Starred' | 'Sent' | 'Drafts' | 'Archive' | 'Trash';

const roleFor: Record<MailViewKind, MailRole | null> = { Inbox: 'inbox', Starred: null, Sent: 'sent', Drafts: 'drafts', Archive: 'archive', Trash: 'trash' };

/**
 * The one-line preview shown in the list, derived the same way the store does
 * (`substr(body_text, 1, 120)`) so a body repaired on open reads like a synced one.
 */
function previewFrom(body: string): string {
  return body.replace(/\s+/g, ' ').trim().slice(0, 120);
}

export function MailView({ view, accountId, focusEmail, onCompose, listWidth, onListWidthChange, onListWidthCommit }: { view: MailViewKind; accountId?: string; focusEmail?: string | null; onCompose: (initial?: ComposeInitial) => void; listWidth: number; onListWidthChange: (width: number) => void; onListWidthCommit: (width: number) => void }) {
  const [items, setItems] = useState<EmailSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [syncing, setSyncing] = useState(false);
  const [syncNote, setSyncNote] = useState('');
  const [hasSynced, setHasSynced] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<EmailDetail | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  // True while a failed send is being retried, so the note's button says so
  // instead of accepting a second click.
  const [sending, setSending] = useState(false);
  // Shown in the reading pane while a missing body is downloaded, or when it
  // turns out there is nothing to show.
  const [bodyNote, setBodyNote] = useState('');
  const [thread, setThread] = useState<EmailSummary[]>([]);
  const [threadOpen, setThreadOpen] = useState(false);
  const [actionsOpen, setActionsOpen] = useState(false);
  const readNotified = useRef(new Set<string>());
  const threadSeq = useRef(0);
  // The delivery listener is attached once, so it reads the open message from a
  // ref rather than closing over a stale selection.
  const selectedRef = useRef<string | null>(null);
  useEffect(() => { selectedRef.current = selectedId; }, [selectedId]);
  // One automatic fetch per mount, so folder browsing cannot repeat it.
  const autoSynced = useRef(false);
  // Messages whose body has already been requested this session. Without it a
  // genuinely empty message (or one the server no longer holds) would be fetched
  // again on every open.
  const bodyRequested = useRef(new Set<string>());
  // Narrow windows switch the workspace to a master-detail flow: the list and
  // the reading pane share one column and swap on selection (see mail.css).
  const [narrow, setNarrow] = useState(() => typeof window !== 'undefined' && window.matchMedia('(max-width: 800px)').matches);
  useEffect(() => {
    const query = window.matchMedia('(max-width: 800px)');
    const onChange = (event: MediaQueryListEvent) => setNarrow(event.matches);
    query.addEventListener('change', onChange);
    return () => query.removeEventListener('change', onChange);
  }, []);

  // Live progress from the native fetch worker (no-op in browser preview).
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    import('@tauri-apps/api/event').then(({ listen }) => listen<string>('mail-sync-progress', event => setSyncNote(event.payload)))
      .then(stop => { unlisten = stop; })
      .catch(() => undefined);
    return () => { unlisten?.(); };
  }, []);

  // Live outcomes from the native send worker. A message opened from Sent starts
  // as "Not delivered yet"; when its transmission finishes, the pane that is
  // showing it must say so (delivered, or the reason it failed) without the user
  // reloading. Only the open message is refreshed, so this cannot disturb any
  // other part of the view. No-op in browser preview.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    import('@tauri-apps/api/event').then(({ listen }) => listen<{ id: string }>('mail-delivery-result', event => {
      const open = selectedRef.current;
      if (!open || open !== event.payload.id) return;
      getEmail(open)
        .then(email => setDetail(current => (email && current && current.id === email.id) ? email : current))
        .catch(() => undefined);
    }))
      .then(stop => { unlisten = stop; })
      .catch(() => undefined);
    return () => { unlisten?.(); };
  }, []);

  // Loads the current folder. Extracted so the Sync action can refetch after a
  // download without duplicating the loader.
  const reload = (openFocus: boolean) => {
    setLoading(true); setLoadError(null);
    const loader = roleFor[view] === null ? getStarred() : getEmailsInFolder(roleFor[view] as MailRole);
    return loader.then(items => { setItems(items); setLoading(false); // Only deep-linked selections (search / jump) open an email automatically;
      // browsing a folder never marks anything read as a side effect.
      if (openFocus && focusEmail && items.some(item => item.id === focusEmail)) setSelectedId(focusEmail); })
      .catch(err => { console.error('[relay] load mail view failed:', view, err); setItems([]); setLoading(false); setLoadError(`Unable to load your ${view.toLowerCase()} from the local store.`); });
  };

  useEffect(() => {
    setSelectedId(null); setDetail(null); setActionError(null); readNotified.current = new Set();
    void reload(true);
  }, [view]);

  // Downloads new mail from the server. This is the step sign-in does not do:
  // it only verifies access, so the local store stays empty until a sync runs.
  // All signed-in accounts are covered — no account has to be picked as "the"
  // mailbox, which is what used to send the fetch at the wrong account.
  const runSync = async (announce = false) => {
    if (syncing) return;
    setSyncing(true); setSyncNote('Preparing to fetch mail…'); setLoadError(null);
    try {
      const count = await syncMail();
      setHasSynced(true);
      setSyncNote(count > 0 ? `Downloaded ${count} message${count === 1 ? '' : 's'}.` : 'No new mail on the server.');
      await reload(false);
    } catch (err) {
      console.error('[relay] mail sync failed:', err);
      setSyncNote('');
      // An automatic sync must not replace the folder with an error banner; the
      // message still has to surface somewhere, so it becomes a note.
      if (announce) setSyncNote(commandError(err, 'Mail could not be downloaded from the server.'));
      else setLoadError(commandError(err, 'Mail could not be downloaded from the server.'));
    } finally {
      setSyncing(false);
    }
  };

  // Fetch once per mount when the view opens on an empty folder. This replaces the
  // silent fire-and-forget sync that used to live in App, whose failures reached
  // only the console — here they render in the UI the user is looking at. The ref
  // keeps it to a single request, so browsing between empty folders cannot turn
  // into a stream of server calls.
  useEffect(() => {
    if (autoSynced.current || loading || items.length > 0 || syncing) return;
    autoSynced.current = true;
    void runSync(true);
  }, [loading, items.length, syncing]);

  /**
   * Downloads a body that is missing from the local store and shows it as soon
   * as it arrives.
   *
   * A stored body can legitimately be absent: rows written before bodies decoded,
   * and rows older than the newest fetch window. Failures are reported in the
   * reading pane instead of the console — a silent failure here is exactly the
   * blank reader this path exists to fix.
   */
  const loadBody = async (email: EmailDetail, seq: number) => {
    if (bodyRequested.current.has(email.id)) return;
    bodyRequested.current.add(email.id);
    setBodyNote('Downloading this message…');
    try {
      const body = await fetchEmailBody(email.id);
      if (seq !== threadSeq.current) return;
      if (!body.trim()) { setBodyNote('This message has no readable content.'); return; }
      setBodyNote('');
      setDetail(current => current && current.id === email.id ? { ...current, bodyText: body } : current);
      setItems(cached => cached.map(item => item.id === email.id ? { ...item, preview: previewFrom(body) } : item));
    } catch (err) {
      if (seq !== threadSeq.current) return;
      // Let a later open try again: the cause may be a dropped connection.
      bodyRequested.current.delete(email.id);
      setBodyNote(commandError(err, 'This message could not be downloaded.'));
    }
  };

  useEffect(() => {
    if (!selectedId) { setDetail(null); setThread([]); setBodyNote(''); return; }
    const seq = ++threadSeq.current;
    getEmail(selectedId).then(email => {
      setDetail(email);
      if (email && email.direction === 'inbound' && !email.isRead && !readNotified.current.has(email.id)) {
        readNotified.current.add(email.id);
        markEmailRead(email.id, true).catch(() => undefined);
        setItems(cached => cached.map(item => item.id === email.id ? { ...item, isRead: true } : item));
      }
      // Outbound rows have no server copy to re-read, so only inbound mail is
      // asked for a body.
      if (email && email.direction === 'inbound' && !email.bodyText.trim()) void loadBody(email, seq);
      else setBodyNote('');
    }).catch(err => { console.error('[relay] load email failed:', selectedId, err); setDetail(null); setLoadError('Unable to open this locally stored email.'); });
    getEmailThread(selectedId).then(rows => {
      if (seq !== threadSeq.current) return;
      setThread(rows);
      setThreadOpen(rows.length > 1);
    }).catch(err => console.error('[relay] load email thread failed:', selectedId, err));
  }, [selectedId]);

  const updateItem = (id: string, patch: Partial<EmailSummary>) => setItems(cached => cached.map(item => item.id === id ? { ...item, ...patch } : item));

  const toggleStar = async (id: string) => {
    const target = detail?.id === id ? detail : items.find(item => item.id === id);
    if (!target) return;
    const next = !target.isStarred;
    updateItem(id, { isStarred: next });
    setDetail(detail && detail.id === id ? { ...detail, isStarred: next } : detail);
    setEmailStar(id, next).catch(() => { setActionError('Could not update the star.'); updateItem(id, { isStarred: !next }); });
  };

const removeFromList = (id: string) => {
    setItems(cached => cached.filter(item => item.id !== id));
    if (selectedId === id) { const index = items.findIndex(item => item.id === id); setSelectedId(items[Math.max(0, index - 1)]?.id ?? null); }
  };

  const guard = (action: () => Promise<void>, message: string) => () => { if (!selectedId) return; setActionError(null); action().catch(() => setActionError(message)); };
  const archive = guard(async () => { const id = selectedId!; await archiveEmail(id); removeFromList(id); }, 'Could not archive this email.');
  const trash = guard(async () => { const id = selectedId!; await trashEmail(id); removeFromList(id); }, 'Could not move this email to trash.');

  const reply = () => { if (!detail) return; const subject = /^Re:/i.test(detail.subject) ? detail.subject : `Re: ${detail.subject}`; onCompose({ to: [detail.senderEmail].filter(Boolean), cc: [], bcc: [], subject, bodyText: '' }); };
  const replyAll = () => { if (!detail) return; const recipients = [detail.senderEmail, ...detail.to].filter(Boolean); const subject = /^Re:/i.test(detail.subject) ? detail.subject : `Re: ${detail.subject}`; onCompose({ to: recipients, cc: detail.cc, bcc: [], subject, bodyText: '' }); };
  const forward = () => { if (!detail) return; onCompose({ to: [], cc: [], bcc: [], subject: /^Fwd:/i.test(detail.subject) ? detail.subject : `Fwd: ${detail.subject}`, bodyText: `---------- Forwarded message ----------\nFrom: ${detail.senderName} <${detail.senderEmail}>\nSubject: ${detail.subject}\n\n${detail.bodyText}\n` }); };
  const resumeDraft = () => { if (!detail) return; onCompose({ draftId: detail.id, to: detail.to, cc: detail.cc, bcc: detail.bcc, subject: detail.subject, bodyText: detail.bodyText }); };
  // Retrying re-runs the durable queue item for this message. The pane is then
  // refreshed from the store, so it lands on the real outcome whichever way the
  // attempt went (the native send worker also emits the same refresh).
  const retryDelivery = async () => {
    if (!detail) return;
    const id = detail.id;
    setSending(true);
    setActionError(null);
    try {
      await deliverQueuedMail([id]);
      const refreshed = await getEmail(id);
      if (refreshed) setDetail(current => (current && current.id === refreshed.id) ? refreshed : current);
    } catch (err) {
      console.error('[relay] resend failed:', id, err);
      setActionError(commandError(err, 'The message could not be sent.'));
    } finally {
      setSending(false);
    }
  };

  const closeReading = () => { setSelectedId(null); setDetail(null); setThread([]); };

  return <section className={`mail-workspace${narrow && selectedId ? ' show-detail' : ''}`} style={{ '--mail-list-width': `${listWidth}px` } as React.CSSProperties}>
    <div className="list-pane">
      <div className="pane-heading"><div><p className="eyebrow">MAIL</p><h1>{view}</h1></div>
        <div className="pane-heading-actions">
          <span className="filter">{items.length} conversations</span>
          <button className="mail-sync" disabled={!accountId || syncing} title={accountId ? 'Download new mail from the server' : 'Sign in an account to fetch mail'} onClick={() => void runSync()} aria-label="Sync mail">
            <RefreshCw size={15} className={syncing ? 'spin' : ''} /> {syncing ? 'Syncing…' : 'Sync'}
          </button>
        </div>
      </div>
      {(syncNote || syncing) && <p className="mail-sync-note" aria-live="polite">{syncNote || 'Fetching mail from the server…'}</p>}
      <div className="list-tools"><span>{items.length} items</span></div>
      {loading ? <SkeletonRows count={7} /> : <div className="email-list">
        {loadError && <p className="load-error">{loadError}</p>}
        {items.map((email, index) => (
          <button className={`email-row ${selectedId === email.id ? 'selected' : ''}`} key={email.id} onClick={() => setSelectedId(email.id)} aria-label={email.subject}>
            <span className={`mail-avatar avatar-${index % 5}`}>{initials(email.senderName)}</span>
            <span className="email-copy">
              <span className="email-top"><strong>{email.senderName}</strong><time>{formatDate(email.receivedAt)}</time></span>
              <span className="subject">{email.subject}</span>
              <span className="preview">{email.preview}</span>
            </span>
            <Star size={16} className={email.isStarred ? 'starred' : ''} />
          </button>
        ))}
        {!loadError && !loading && items.length === 0 && <EmptyFolder view={view} onCompose={onCompose} synced={hasSynced} onSync={accountId ? () => void runSync() : undefined} />}
      </div>}
    </div>
    <PaneSplitter className="mail-splitter" value={listWidth} min={280} max={560} defaultValue={380} label="Adjust message list width" onChange={onListWidthChange} onCommit={onListWidthCommit} />
    <article className="reading-pane" tabIndex={0} onKeyDown={event => {
      if (event.altKey || event.ctrlKey || event.metaKey || !detail) return;
      if (event.key === 'r' || event.key === 'R') { reply(); event.preventDefault(); }
      else if (event.key === 'f' || event.key === 'F') { forward(); event.preventDefault(); }
      else if (event.key === 'e' || event.key === 'E') { archive(); event.preventDefault(); }
      else if (event.key === 'Delete') { trash(); event.preventDefault(); }
    }}>
      {actionError && <p className="load-error">{actionError}</p>}
      {detail ? (
        <>
        {thread.length > 1 && (
          <div className="email-thread">
            <button className="thread-toggle" onClick={() => setThreadOpen(open => !open)}>Conversation · {thread.length} messages{threadOpen ? ' ▾' : ' ▸'}</button>
            {threadOpen && <div className="thread-list">{thread.map((item, index) => (
              <button key={item.id} className={item.id === detail.id ? 'thread-active' : ''} onClick={() => setSelectedId(item.id)} aria-label={item.subject}>
                <span className={`mail-avatar avatar-${index % 5}`}>{initials(item.senderName)}</span>
                <span className="thread-copy"><strong>{item.senderName}</strong><em>{item.subject}</em></span>
                <time>{formatDate(item.receivedAt)}</time>
              </button>
            ))}</div>}
          </div>
        )}
        <div className="message-content">
          <div className="message-actions">
            <button className="icon-button mobile-only back-to-list" aria-label="Back to the message list" onClick={closeReading}><ArrowLeft size={18} /></button>
            {detail.direction === 'inbound' && <button className="icon-button" aria-label="Archive" title="Archive (E)" onClick={archive}><Archive size={18} /></button>}
            <button className="icon-button" aria-label="Move to trash" title="Move to trash" onClick={trash}><Trash2 size={18} /></button>
            <button className="icon-button" aria-label={detail.isStarred ? 'Remove star' : 'Star'} onClick={() => void toggleStar(detail.id)}><Star size={18} className={detail.isStarred ? 'starred' : ''} /></button>
            <div className="actions-anchor">
              <button className="icon-button" aria-label="More actions" aria-haspopup="menu" aria-expanded={actionsOpen} onClick={() => setActionsOpen(open => !open)}><MoreHorizontal size={18} /></button>
              {actionsOpen && <div className="actions-menu" role="menu">
                <button role="menuitem" onClick={() => {
                  setActionsOpen(false);
                  if (!detail) return;
                  // Forget the auto-read guard so reopening the email marks it read again.
                  readNotified.current.delete(detail.id);
                  markEmailRead(detail.id, false).then(() => {
                    setDetail(current => current ? { ...current, isRead: false } : current);
                    setItems(rows => rows.map(row => row.id === detail.id ? { ...row, isRead: false } : row));
                  }).catch(err => { console.error('[relay] mark unread failed:', detail.id, err); setActionError('This email could not be marked unread.'); });
                }}>Mark as unread</button>
              </div>}
            </div>
          </div>
          <div className="message-label">MAIL <span>·</span> {detail.direction === 'outbound' ? 'OUTBOX' : 'INBOX'} <span>·</span> LOCAL</div>
          <h2>{detail.subject}</h2>
          <div className="sender">
            <span className="mail-avatar">{initials(detail.senderName)}</span>
            <div><strong>{detail.senderName}</strong><p>{detail.senderEmail}</p></div>
            <time>{formatDate(detail.receivedAt)}</time>
          </div>
          {(detail.to.length > 0 || detail.cc.length > 0) && <div className="recipients"><span>To</span>{detail.to.map(address => <em key={address}>{address}</em>)}{detail.cc.length > 0 && <span>CC</span>}{detail.cc.map(address => <em key={address}>{address}</em>)}</div>}
          {/* Delivery state is stated on the message itself. Queued mail has not
              left the machine yet; failed mail says why and offers a retry; a
              message that was accepted by the relay carries no note. Keeping
              those states distinguishable from delivered mail is what BUG-023
              asked for — the transport now exists, so the states differ instead
              of being merged into one blanket warning. */}
          {detail.direction === 'outbound' && detail.deliveryState === 'queued' && (
            <p className="email-delivery-note" role="status">
              <strong>Not delivered yet.</strong> This message is saved in Sent on this computer and is being sent from here.
            </p>
          )}
          {detail.direction === 'outbound' && detail.deliveryState === 'failed' && (
            <p className="email-delivery-note" role="status">
              <strong>Sending failed.</strong> {detail.deliveryError ?? 'The message could not be sent from this computer.'}
              <button className="delivery-retry" disabled={sending} onClick={() => void retryDelivery()}>{sending ? 'Retrying…' : 'Try again'}</button>
            </p>
          )}
          {/* A message with no readable text part, or one whose body is still
              downloading, says so instead of leaving a blank column that reads
              as a rendering fault. */}
          <div className="email-body">
            {detail.bodyText
              ? detail.bodyText.split('\n').map((paragraph, index) => <p key={index}>{paragraph || ' '}</p>)
              : <p className="email-body-note">{bodyNote || 'This message has no readable content.'}</p>}
          </div>
          <div className="reply-bar">
            {detail.direction === 'outbound' && detail.deliveryState === 'draft' && <button className="primary" onClick={resumeDraft}><CornerDownLeft size={17} /> Resume editing</button>}
            {detail.direction === 'inbound' && <>
              <button className="primary" onClick={reply}><CornerDownLeft size={17} /> Reply</button>
              <button onClick={replyAll}>Reply all</button>
              <button onClick={forward}><Forward size={16} /> Forward</button>
            </>}
          </div>
        </div>
        </>
      ) : !loading && <div className="empty-detail"><Mail size={30} /><h2>{view} is quiet</h2><p>Select a message to read it here.</p></div>}
    </article>
  </section>;
}

function EmptyFolder({ view, onCompose, synced, onSync }: { view: MailViewKind; onCompose: (initial?: ComposeInitial) => void; synced?: boolean; onSync?: () => void }): JSX.Element {
  // The Inbox copy has to be honest about *why* it is empty: nothing has been
  // downloaded until a sync runs, and an account may also have no account at all.
  const inboxBody = !onSync
    ? 'Sign in an email account to fetch mail from its server.'
    : synced
      ? 'No messages are stored locally for this account yet. Sync again later, or check the account in Settings.'
      : 'Relay has not downloaded mail for this account yet. Use Sync to fetch your messages from the server.';
  const copy: Record<MailViewKind, { title: string; body: string; action?: { label: string; run: () => void } }> = {
    Inbox: { title: 'Your inbox is empty', body: inboxBody, action: onSync ? { label: 'Sync mail', run: onSync } : { label: 'Compose email', run: () => onCompose() } },
    Starred: { title: 'No starred mail', body: 'Star important messages to find them quickly.' },
    Sent: { title: 'Nothing sent yet', body: 'Messages you send will appear here after delivery is queued.' },
    Drafts: { title: 'No drafts', body: 'Start a message and it will be auto-saved here.', action: { label: 'New message', run: () => onCompose() } },
    Archive: { title: 'Archive is empty', body: 'Messages you archive are kept for later reference.' },
    Trash: { title: 'Trash is empty', body: 'Deleted messages land here until removed permanently.' },
  };
  const state = copy[view];
  return <div className="empty"><Mail size={30} /><h2>{state.title}</h2><p>{state.body}</p>{state.action && <button className="empty-action" onClick={state.action.run}>{onSync && view === 'Inbox' ? <RefreshCw size={15} /> : <Send size={15} />} {state.action.label}</button>}</div>;
}

function SkeletonRows({ count }: { count: number }) {
  return <div className="skeleton-list">{Array.from({ length: count }, (_, index) => <div className="skeleton-row" key={index}><span className="skeleton skeleton-circle" /><span className="skeleton-copy"><span className="skeleton skeleton-line wide" /><span className="skeleton skeleton-line" /><span className="skeleton skeleton-line short" /></span></div>)}</div>;
}
