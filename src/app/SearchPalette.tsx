import { useEffect, useRef, useState } from 'react';
import { AtSign, Hash, Inbox, Mail, MessageSquareText, Moon, RefreshCw, Search, Send, Settings as SettingsIcon, UserRound, Users } from 'lucide-react';
import { searchGlobal, type SearchResult } from '../platform/tauri';
import { type AppView, type ComposeInitial } from './MailView';

interface Command { id: string; kind: 'command'; label: string; icon: typeof Inbox; run: () => void; }
type PaletteItem = SearchResult | Command;
type Entry = { item: PaletteItem; artist: string };

const GROUP_LABELS: Record<string, string> = { email: 'EMAIL', message: 'MESSAGES', contact: 'CONTACTS', channel: 'CHANNELS' };

export function SearchPalette({ open, onClose, onNavigate, onCompose, onToggleTheme, onSyncNow }: {
  open: boolean;
  onClose: () => void;
  onNavigate: (view: AppView, selectEmailId?: string, channelId?: string) => void;
  onCompose: (initial?: ComposeInitial) => void;
  onToggleTheme: () => void;
  onSyncNow: () => void;
}) {
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<SearchResult[]>([]);
  const [active, setActive] = useState(0);
  const [error, setError] = useState('');
  const inputRef = useRef<HTMLInputElement | null>(null);
  const timer = useRef<number>();

  useEffect(() => {
    if (!open) { setQuery(''); setResults([]); setActive(0); return; }
    inputRef.current?.focus();
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const run = () => {
      const needle = query.trim();
      if (!needle) { setResults([]); setActive(0); setError(''); return; }
      searchGlobal(needle).then(items => { setResults(items); setActive(0); setError(''); }).catch(() => setError('Search could not be completed.'));
    };
    if (timer.current) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(run, 180);
    return () => { if (timer.current) window.clearTimeout(timer.current); };
  }, [query, open]);

  const commands = (): Command[] => [
    { id: 'compose', kind: 'command', label: 'Compose Email', icon: Send, run: () => { onClose(); onCompose(); } },
    { id: 'message', kind: 'command', label: 'New Message', icon: MessageSquareText, run: () => { onClose(); onNavigate('Messages'); } },
    { id: 'inbox', kind: 'command', label: 'Go to Inbox', icon: Inbox, run: () => { onClose(); onNavigate('Inbox'); } },
    { id: 'contacts', kind: 'command', label: 'Go to Contacts', icon: UserRound, run: () => { onClose(); onNavigate('Contacts'); } },
    { id: 'sync', kind: 'command', label: 'Sync Now', icon: RefreshCw, run: () => { onClose(); onSyncNow(); } },
    { id: 'theme', kind: 'command', label: 'Toggle Dark Mode', icon: Moon, run: () => { onClose(); onToggleTheme(); } },
    { id: 'settings', kind: 'command', label: 'Open Settings', icon: SettingsIcon, run: () => { onClose(); onNavigate('Settings'); } },
  ];

  const entries = (): Entry[] => {
    if (query.trim().length === 0) return commands().map(item => ({ item, artist: 'COMMANDS' }));
    const grouped: Entry[] = [];
    for (const kind of ['email', 'message', 'contact', 'channel']) {
      const items = results.filter(item => item.kind === kind);
      if (items.length > 0) grouped.push(...items.map(item => ({ item, artist: GROUP_LABELS[kind] })));
    }
    return grouped;
  };

  const selectAt = (index: number) => {
    const list = entries();
    const entry = list[index];
    if (!entry) return;
    if (entry.item.kind === 'command') { entry.item.run(); return; }
    const result = entry.item;
    onClose();
    if (result.kind === 'email') onNavigate('Inbox', result.id);
    else if (result.kind === 'message' || result.kind === 'channel') onNavigate('Messages', undefined, result.id);
    else onNavigate('Contacts');
  };

  const onKeyDown = (event: { key: string; ctrlKey: boolean; metaKey: boolean; preventDefault: () => void }) => {
    const list = entries();
    if (event.key === 'Escape') { onClose(); return; }
    if (event.key === 'ArrowDown') { event.preventDefault(); setActive((active + 1) % Math.max(1, list.length)); }
    else if (event.key === 'ArrowUp') { event.preventDefault(); setActive((active - 1 + Math.max(1, list.length)) % Math.max(1, list.length)); }
    else if (event.key === 'Enter') { event.preventDefault(); selectAt(active); }
    else if (event.key === 'k' && (event.ctrlKey || event.metaKey)) { event.preventDefault(); }
  };

  if (!open) return null;
  const list = entries();
  return <div className="palette-backdrop" role="presentation" onMouseDown={event => { if (event.target === event.currentTarget) onClose(); }}>
    <section className="palette" role="dialog" aria-modal="true" aria-label="Global search and commands" onKeyDown={onKeyDown}>
      <div className="palette-input"><Search size={17} /><input ref={inputRef} value={query} onChange={event => setQuery(event.target.value)} placeholder="Search mail, messages, people, or run a command…" aria-label="Global search" /><kbd>Esc</kbd></div>
      {error && <p className="load-error">{error}</p>}
      <div className="palette-list">
        {list.map(({ item, artist }, index) => (
          <button key={`${item.id}-${index}`} className={`palette-row ${index === active ? 'active' : ''}`} onClick={() => selectAt(index)} onMouseEnter={() => setActive(index)}>
            <span className="palette-icon">{iconFor(item)}</span>
            <span className="palette-copy"><strong>{item.kind === 'command' ? item.label : item.title}</strong><p>{item.kind === 'command' ? 'Command' : item.subtitle || ' '}</p></span>
            <em>{artist}</em>
          </button>
        ))}
      </div>
    </section>
  </div>;
  }

function iconFor(item: PaletteItem) {
  if (item.kind === 'command') return <item.icon size={16} />;
  if (item.kind === 'email') return <Mail size={16} />;
  if (item.kind === 'message') return <AtSign size={16} />;
  if (item.kind === 'channel') return <Hash size={16} />;
  return <Users size={16} />;
}