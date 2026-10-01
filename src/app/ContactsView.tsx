import { useEffect, useState } from 'react';
import { Building2, MessageSquareText, Plus, Search, Send, Star, UserPlus, X } from 'lucide-react';
import { createContact, getContacts, setContactFavorite, type Contact, type ContactInput } from '../platform/tauri';
import { initials } from './util';
import { type ComposeInitial } from './MailView';

export function ContactsView({ onCompose, onMessageContact }: { onCompose: (initial?: ComposeInitial) => void; onMessageContact: (contactId: string) => void }) {
  const [contacts, setContacts] = useState<Contact[]>([]);
  const [query, setQuery] = useState('');
  const [favoritesOnly, setFavoritesOnly] = useState(false);
  const [adding, setAdding] = useState(false);
  const [form, setForm] = useState<ContactInput>({ name: '', email: '', department: '', position: '' });
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');

  useEffect(() => { getContacts().then(setContacts).catch(err => { console.error('[relay] contacts load failed:', err); setError('Unable to load the company directory.'); }); }, []);

  const visible = contacts.filter(contact => {
    if (favoritesOnly && !contact.favorite) return false;
    const needle = query.trim().toLowerCase();
    if (!needle) return true;
    return `${contact.name} ${contact.email ?? ''} ${contact.department ?? ''} ${contact.position ?? ''}`.toLowerCase().includes(needle);
  });

  const toggleFavorite = async (contact: Contact) => {
    const next = !contact.favorite;
    setContacts(items => items.map(item => item.id === contact.id ? { ...item, favorite: next } : item));
    setContactFavorite(contact.id, next).catch(() => setError('Could not update the contact.'));
  };

  const submitContact = async () => {
    setError(''); setNotice('');
    if (!form.name?.trim()) { setError('A name is required to add a contact.'); return; }
    try {
      const created = await createContact({ name: form.name, email: form.email || null, department: form.department || null, position: form.position || null });
      setContacts(items => [...items, created]);
      setForm({ name: '', email: '', department: '', position: '' });
      setAdding(false);
      setNotice('Contact added to the local directory.');
    } catch { setError('Contact could not be saved locally.'); }
  };

  return <section className="contacts-workspace">
    <div className="contacts-toolbar">
      <div><p className="eyebrow">COMPANY</p><h1>Contacts</h1></div>
      <label className="contact-search"><Search size={16} /><input value={query} onChange={event => setQuery(event.target.value)} placeholder="Search by name, department, position, or email" aria-label="Search contacts" /></label>
      <button className="toggle-favorites" onClick={() => setFavoritesOnly(open => !open)} aria-pressed={favoritesOnly}><Star size={15} className={favoritesOnly ? 'starred' : ''} /> Favorites</button>
      <button className="add-contact" onClick={() => { setAdding(open => !open); setError(''); }}><UserPlus size={16} /> Add contact</button>
    </div>
    {notice && <p className="settings-notice">{notice}</p>}
    {adding && <div className="contact-form">
      <strong>New contact</strong>
      <div className="contact-fields">
        <input placeholder="Full name" value={form.name ?? ''} onChange={event => setForm({ ...form, name: event.target.value })} aria-label="Full name" />
        <input placeholder="Email" value={form.email ?? ''} onChange={event => setForm({ ...form, email: event.target.value })} aria-label="Email" />
        <input placeholder="Department" value={form.department ?? ''} onChange={event => setForm({ ...form, department: event.target.value })} aria-label="Department" />
        <input placeholder="Position" value={form.position ?? ''} onChange={event => setForm({ ...form, position: event.target.value })} aria-label="Position" />
      </div>
      <button className="contact-save" onClick={submitContact}><Plus size={15} /> Save contact</button>
      <button className="contact-cancel" onClick={() => setAdding(false)}><X size={15} /> Cancel</button>
    </div>}
    {error && <p className="load-error">{error}</p>}
    {visible.length === 0 && !error ? <div className="empty"><Building2 size={30} /><h2>No contacts found</h2><p>Add someone to the directory or adjust your search.</p></div>
      : <div className="contacts-grid">{visible.map(contact => (
        <article className="contact-card" key={contact.id}>
          <div className="contact-avatar">{initials(contact.name)}</div>
          <div className="contact-copy">
            <strong>{contact.name}</strong>
            <p>{contact.position ?? '—'}<em>{contact.department ?? ''}</em></p>
            <span>{contact.email ?? 'No email on file'}</span>
          </div>
          <div className="contact-actions">
            <button aria-label={contact.favorite ? 'Remove from favorites' : 'Add to favorites'} title="Favorite" onClick={() => void toggleFavorite(contact)}><Star size={16} className={contact.favorite ? 'starred' : ''} /></button>
            <button aria-label="Send email" title="Send email" disabled={!contact.email} onClick={() => contact.email && onCompose({ to: [contact.email], cc: [], bcc: [], subject: '', bodyText: '' })}><Send size={16} /></button>
            <button aria-label="Open direct message" title="Open direct message" onClick={() => onMessageContact(contact.id)}><MessageSquareText size={16} /></button>
          </div>
        </article>
      ))}</div>}
  </section>;
}