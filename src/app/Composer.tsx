import { useEffect, useRef, useState } from 'react';
import { Paperclip, Send, X } from 'lucide-react';
import { commandError, getAccounts, queueEmailSend, saveDraft, type Account, type DraftInput } from '../platform/tauri';
import { parseAddresses } from './util';
import { type ComposeInitial } from './MailView';

// Sending is local-first and then real: the outbound copy is filed into Sent
// with a durable `send_smtp` item, and the shell transmits that item over SMTP
// (see App's `onQueued`). `onQueued` lets the shell show the message that was
// sent rather than leaving the composer over an unchanged list, which read as a
// failed send (BUG-023).
export function Composer({ onClose, onQueued, initial }: { onClose: () => void; onQueued?: (id: string) => void; initial?: ComposeInitial }) {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [draftId, setDraftId] = useState<string | undefined>(initial?.draftId);
  const [status, setStatus] = useState('');
  const [form, setForm] = useState<DraftInput>({
    accountId: '',
    to: initial?.to ?? [],
    cc: initial?.cc ?? [],
    bcc: initial?.bcc ?? [],
    subject: initial?.subject ?? '',
    bodyText: initial?.bodyText ?? '',
  });
  const timer = useRef<number>();

  useEffect(() => {
    getAccounts().then(items => { setAccounts(items); setForm(current => ({ ...current, accountId: current.accountId || items[0]?.id || '' })); }).catch(() => setStatus('No local mail account is configured.'));
    return () => { if (timer.current) window.clearTimeout(timer.current); };
  }, []);

  const update = (key: keyof DraftInput, value: string) => {
    const next = { ...form, [key]: key === 'to' || key === 'cc' || key === 'bcc' ? parseAddresses(value) : value };
    setForm(next);
    if (timer.current) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => { void persist(next); }, 700);
  };

  const persist = async (input = form): Promise<string | undefined> => {
    if (!input.accountId) return undefined;
    try {
      const draft = await saveDraft(input, draftId);
      setDraftId(draft.id);
      setStatus('Saved locally');
      return draft.id;
    } catch (err) { setStatus(commandError(err, 'Draft could not be saved.')); return undefined; }
  };

  // `persist` is a no-op without an account, so pressing Send used to do nothing
  // and say nothing at all (BUG-023). On success the composer closes and the
  // shell opens the sent message in Sent, where the delivery notice reports the
  // real outcome of the transmission.
  const send = async () => {
    if (!form.accountId) { setStatus('Choose the account to send from.'); return; }
    const id = await persist();
    if (!id) return;
    try { await queueEmailSend(id); onQueued?.(id); onClose(); }
    catch (err) { setStatus(commandError(err, 'The message could not be sent.')); }
  };

  return <div className="composer-backdrop" role="presentation"><section className="composer" role="dialog" aria-modal="true" aria-label="Compose email" onKeyDown={event => {
    if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) { event.preventDefault(); void send(); }
  }}>
    <header><strong>New email</strong><button onClick={onClose} aria-label="Close composer"><X size={18} /></button></header>
    <div className="composer-fields">
      {draftId && <p className="draft-note">Resuming a locally saved draft.</p>}
      <select value={form.accountId} onChange={event => update('accountId', event.target.value)} aria-label="Sending account">
        <option value="">Select account</option>
        {accounts.map(account => <option key={account.id} value={account.id}>{account.displayName} · {account.emailAddress}</option>)}
      </select>
      <input placeholder="To" defaultValue={form.to.join(', ')} onInput={event => update('to', event.currentTarget.value)} aria-label="Recipients" />
      <input placeholder="Cc (optional)" defaultValue={form.cc.join(', ')} onInput={event => update('cc', event.currentTarget.value)} aria-label="CC recipients" />
      <input placeholder="Bcc (optional)" defaultValue={form.bcc.join(', ')} onInput={event => update('bcc', event.currentTarget.value)} aria-label="BCC recipients" />
      <input placeholder="Subject" defaultValue={form.subject} onInput={event => update('subject', event.currentTarget.value)} aria-label="Subject" />
    </div>
    <textarea placeholder="Write your message…" defaultValue={form.bodyText} onInput={event => update('bodyText', event.currentTarget.value)} aria-label="Email message" />
    <footer>
      <button className="send-button" onClick={() => void send()}><Send size={16} /> Send</button>
      <button className="attach-button" disabled title="Attachment import arrives with secure file storage"><Paperclip size={17} /></button>
      <span>{status}<em className="shortcut-hint">Ctrl+Enter to send</em></span>
    </footer>
  </section></div>;
}
