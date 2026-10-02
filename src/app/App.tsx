import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Archive, AtSign, Bell, ChevronDown, CircleHelp, Command, FileText, Inbox, Menu, MessageSquareText, PanelLeftClose, PanelLeftOpen, PenLine, Search, Send, Settings, Star, Trash2, Users, X } from 'lucide-react';
import { commandError, getAppHealth, getAppSettings, getAccounts, getPresence, getSyncOverview, getUnreadNotificationCount, isNative, deliverQueuedMail, openDirectMessage as openDirectMessageWith, setAppSetting, setPresenceStatus, syncMail, type Account, type AppHealth, type DensityPreference, type PresenceStatus, type SyncOverview, type ThemePreference } from '../platform/tauri';
import { PaneSplitter } from './PaneSplitter';
import { Composer } from './Composer';
import { ContactsView } from './ContactsView';
import { MailView, type AppView, type ComposeInitial, type MailViewKind } from './MailView';
import { MessengerWorkspace } from './MessengerWorkspace';
import { NotificationPanel } from './NotificationPanel';
import { SearchPalette } from './SearchPalette';
import { SettingsView } from './SettingsView';
import { LoginView } from './LoginView';
import { connectionView, initials } from './util';

const MAIL_VIEWS: { label: MailViewKind; icon: typeof Inbox }[] = [
  { label: 'Inbox', icon: Inbox }, { label: 'Starred', icon: Star }, { label: 'Sent', icon: Send },
  { label: 'Drafts', icon: FileText }, { label: 'Archive', icon: Archive }, { label: 'Trash', icon: Trash2 },
];
const MESSENGER_VIEWS: { label: AppView; icon: typeof AtSign }[] = [
  { label: 'Messages', icon: MessageSquareText }, { label: 'Threads', icon: AtSign }, { label: 'Channels', icon: Users },
];
const MAIL_SET = new Set(['Inbox', 'Starred', 'Sent', 'Drafts', 'Archive', 'Trash']);
// The top-bar presence menu offers the same four states the Settings control
// does — the wording is kept in step with `SettingsView.PRESENCES`.
const PRESENCES: { value: PresenceStatus; label: string; hint: string }[] = [
  { value: 'online', label: 'Online', hint: 'Available for conversations' },
  { value: 'away', label: 'Away', hint: 'Temporarily unavailable' },
  { value: 'dnd', label: 'Do not disturb', hint: 'Pause mentions and alerts' },
  { value: 'offline', label: 'Offline', hint: 'Appear unavailable' },
];
const clampWidth = (value: number, min: number, max: number, fallback: number) => Number.isFinite(value) ? Math.min(max, Math.max(min, value)) : fallback;
const native = isNative();

/// Explicit sign-in lifecycle. This used to be one boolean plus a fail-open
/// catch, so "still checking", "no account yet", "signed in" and "the store
/// could not be read" were indistinguishable to the UI.
type AuthState = 'INITIALIZING' | 'UNAUTHENTICATED' | 'AUTHENTICATED' | 'AUTH_ERROR' | 'SIGNED_OUT';

export function App() {
  const [view, setView] = useState<AppView>('Inbox');
  const [collapsed, setCollapsed] = useState(false);
  const [composer, setComposer] = useState<{ open: boolean; initial?: ComposeInitial }>({ open: false });
  const [notificationsOpen, setNotificationsOpen] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [navId, setNavId] = useState(0);
  const [focusEmail, setFocusEmail] = useState<string | undefined>(undefined);
  const [focusChannel, setFocusChannel] = useState<string | undefined>(undefined);
  const [focusConversation, setFocusConversation] = useState<string | undefined>(undefined);
  const [theme, setTheme] = useState<ThemePreference>('system');
  const [density, setDensity] = useState<DensityPreference>('comfortable');
  const [fontSize, setFontSize] = useState('13');
  const [presence, setPresence] = useState<PresenceStatus>('offline');
  // Connectivity is the machine's own fact and the webview is the only place
  // that knows it (`navigator.onLine`), so it is tracked here rather than being
  // guessed from the sync transport (BUG-015). `busy` marks an in-flight fetch,
  // and `statusNote` is the transient line the status bar shows for actions the
  // user cannot otherwise see (a palette sync, a failed preference write).
  const [online, setOnline] = useState(() => (typeof navigator === 'undefined' ? true : navigator.onLine));
  const [busy, setBusy] = useState(false);
  const [statusNote, setStatusNote] = useState('');
  const [workspaceOpen, setWorkspaceOpen] = useState(false);
  const [presenceOpen, setPresenceOpen] = useState(false);
  const [unreadCount, setUnreadCount] = useState(0);
  const [health, setHealth] = useState<AppHealth>({ state: navigator.onLine ? 'online' : 'offline', lastSyncAt: null });
  const [sync, setSync] = useState<SyncOverview | null>(null);
  const [authState, setAuthState] = useState<AuthState>('INITIALIZING');
  const [authWarning, setAuthWarning] = useState('');
  const [skipLogin, setSkipLogin] = useState(false);
  const [navOpen, setNavOpen] = useState(false);
  const [sidebarWidth, setSidebarWidth] = useState(248);
  const [mailListWidth, setMailListWidth] = useState(380);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [dmError, setDmError] = useState('');
  const [helpOpen, setHelpOpen] = useState(false);
  const saveTimer = useRef<number | undefined>(undefined);

  useEffect(() => { getAppHealth().then(setHealth).catch(() => setHealth({ state: 'error', lastSyncAt: null })); }, []);
  // Re-reads the accounts table and the sign-in state it implies. Shared by the
  // boot gate and by every account mutation, so adding/removing an account can
  // no longer leave the gate, the avatar and Settings disagreeing (BUG-016).
  const refreshAccounts = useCallback(async () => {
    try {
      const rows = await getAccounts();
      setAccounts(rows);
      setAuthState(rows.length > 0 ? 'AUTHENTICATED' : 'UNAUTHENTICATED');
      setAuthWarning('');
    } catch (error) {
      console.error('[relay] account check failed:', error);
      // Fail open — a broken store must not lock the user out — but say so
      // instead of presenting a signed-in workspace as if all were well.
      setAuthState('AUTH_ERROR');
      setAuthWarning('Relay could not read the local account store, so the sign-in state is unknown. Open Settings → Accounts to re-verify a connection.');
    }
  }, []);
  // First-run gate: an empty local store means no email account has signed in yet.
  useEffect(() => { void refreshAccounts(); }, [refreshAccounts]);
  useEffect(() => { getSyncOverview().then(setSync).catch(() => undefined); }, []);
  useEffect(() => { getPresence().then(item => setPresence(item.status)).catch(() => undefined); }, []);
  useEffect(() => { getUnreadNotificationCount().then(setUnreadCount).catch(() => undefined); }, [notificationsOpen]);
  // A direct-message error is surfaced once in the freshly mounted workspace,
  // then cleared so it does not reappear on later navigation.
  useEffect(() => { setDmError(''); }, [navId]);
  useEffect(() => {
    getAppSettings().then(entries => { for (const entry of entries) {
      if (entry.key === 'theme' && ['light', 'dark', 'system'].includes(entry.value)) setTheme(entry.value as ThemePreference);
      else if (entry.key === 'density' && ['comfortable', 'compact'].includes(entry.value)) setDensity(entry.value as DensityPreference);
      else if (entry.key === 'font_size') setFontSize(entry.value);
      else if (entry.key === 'ui.sidebar_width') setSidebarWidth(clampWidth(Number(entry.value), 200, 320, 248));
      else if (entry.key === 'ui.mail_list_width') setMailListWidth(clampWidth(Number(entry.value), 280, 560, 380));
    } }).catch(() => undefined);
  }, []);

  // The browser reports network changes; a one-time read at boot is why the old
  // indicator never moved (BUG-015). This is the truth the "Offline" label uses.
  useEffect(() => {
    const goOnline = () => setOnline(true);
    const goOffline = () => setOnline(false);
    window.addEventListener('online', goOnline);
    window.addEventListener('offline', goOffline);
    return () => { window.removeEventListener('online', goOnline); window.removeEventListener('offline', goOffline); };
  }, []);

  // The badge must not go stale while the window stays open: notifications are
  // written by fetches and deliveries without telling the shell (BUG-013).
  useEffect(() => {
    const timer = window.setInterval(() => { getUnreadNotificationCount().then(setUnreadCount).catch(() => undefined); }, 60_000);
    return () => window.clearInterval(timer);
  }, []);

  // A status note reports one action; it clears itself so the bar returns to the
  // standing queue summary.
  useEffect(() => {
    if (!statusNote) return;
    const timer = window.setTimeout(() => setStatusNote(''), 8_000);
    return () => window.clearTimeout(timer);
  }, [statusNote]);

  useEffect(() => {
    const root = document.documentElement;
    const resolved = theme === 'system' ? (window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light') : theme;
    root.dataset.theme = resolved;
    root.dataset.density = density;
    root.style.setProperty('--font-size-base', `${fontSize}px`);
  }, [theme, density, fontSize]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.key === 'k' || event.key === 'K') && (event.ctrlKey || event.metaKey)) { event.preventDefault(); setPaletteOpen(open => !open); }
      else if (event.key === 'm' && (event.ctrlKey || event.metaKey) && event.shiftKey) { event.preventDefault(); openCompose(); }
      else if (event.key === 'n' && (event.ctrlKey || event.metaKey) && !event.shiftKey) { event.preventDefault(); openCompose(); }
      else if (event.key === 'Escape') { setNavOpen(false); setWorkspaceOpen(false); setPresenceOpen(false); }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const openCompose = (initial?: ComposeInitial) => setComposer({ open: true, initial });
  const closeCompose = () => setComposer({ open: false });
  const refreshSync = () => getSyncOverview().then(setSync).catch(() => undefined);
  // The palette's "Sync Now" used to re-read the queue counters and call that
  // syncing (BUG-008). It now performs both real transports — fetch new mail
  // over IMAP and drain queued outbound mail over SMTP — and reports what
  // happened in the status bar, while `busy` makes the connection pill say so.
  const syncNow = async () => {
    if (busy) return;
    setBusy(true); setStatusNote('Fetching mail from the server…');
    try {
      const fetched = await syncMail();
      const delivered = await deliverQueuedMail();
      const parts = [`Downloaded ${fetched} message${fetched === 1 ? '' : 's'}`];
      if (delivered.sent.length > 0) parts.push(`sent ${delivered.sent.length}`);
      if (delivered.failed.length > 0) parts.push(`${delivered.failed.length} need attention`);
      setStatusNote(parts.join(' · '));
    } catch (err) {
      console.error('[relay] sync now failed:', err);
      setStatusNote(commandError(err, 'Mail could not be fetched from the server.'));
    } finally {
      setBusy(false);
      void refreshSync();
    }
  };
  // A send that was queued but never transmitted — the window was closed before
  // delivery ran, the network was down, or a previous attempt failed — is drained
  // once the workspace knows an account is signed in, so it cannot sit in Sent
  // looking sent. `queued_sends` only returns unfinished work, so a run with
  // nothing waiting makes no connection.
  useEffect(() => {
    if (authState !== 'AUTHENTICATED') return;
    let live = true;
    void deliverQueuedMail().then(() => { if (live) void refreshSync(); }).catch(() => undefined);
    return () => { live = false; };
  }, [authState]);
  // The palette's theme toggle must survive a restart, so it writes through the
  // same store the Settings control uses (BUG-005).
  const toggleTheme = () => {
    const next: ThemePreference = theme === 'dark' ? 'light' : 'dark';
    setTheme(next);
    setAppSetting('theme', next).catch(() => setStatusNote('The theme could not be saved on this device.'));
  };
  // Presence is set from the top bar now; a refusal reverts the dot instead of
  // leaving the UI claiming a state the store did not accept.
  const changePresence = async (status: PresenceStatus) => {
    const previous = presence;
    setPresence(status);
    try { await setPresenceStatus(status); }
    catch (err) {
      console.error('[relay] presence update failed:', err);
      setPresence(previous);
      setStatusNote('Presence could not be updated.');
    }
  };

  const navigate = (next: AppView, selectEmailId?: string, channelId?: string, conversationId?: string) => {
    setView(next);
    setNavId(count => count + 1);
    setNavOpen(false);
    setFocusEmail(selectEmailId);
    setFocusChannel(next === 'Messages' && channelId ? channelId : undefined);
    setFocusConversation(next === 'Messages' && conversationId ? conversationId : undefined);
  };
  // Opens (or creates) the direct-message conversation with a contact through
  // the native command, then focuses the conversation it returns. Contact ids
  // are not conversation ids, so the lookup must go through the backend first.
  const openDirectMessage = (contactId: string) => {
    setDmError('');
    openDirectMessageWith(contactId).then(conversation => navigate('Messages', undefined, conversation.id))
      .catch(error => {
        console.error('[relay] opening direct message failed:', contactId, error);
        setDmError('This direct message could not be opened. The contact may have been removed.');
        navigate('Messages');
      });
  };

  const onPreferenceChange = (key: string, value: string) => {
    if (key === 'theme' && ['light', 'dark', 'system'].includes(value)) setTheme(value as ThemePreference);
    else if (key === 'density' && ['comfortable', 'compact'].includes(value)) setDensity(value as DensityPreference);
    else if (key === 'font_size') setFontSize(value);
  };

  // Pane widths are persisted to the local settings store, debounced so a drag
  // writes once instead of per pointer-move event.
  const persistWidth = (key: string, width: number) => {
    window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => { setAppSetting(key, String(Math.round(width))).catch(error => console.error(`[relay] saving ${key} failed:`, error)); }, 400);
  };

  // One display model for both indicators (the top-bar pill and the status bar),
  // built from the machine's connectivity plus the store's sync facts — the two
  // facts the old label used to run together (BUG-015).
  const connection = useMemo(() => connectionView({ online, busy, sync, health }), [online, busy, sync, health]);

  const isMail = MAIL_SET.has(view);
  const mailView = view as MailViewKind;

  if (authState === 'INITIALIZING') return <div className="login-screen boot" aria-busy="true"><div className="login-card"><p className="muted">Opening the local workspace…</p></div></div>;
  // AUTH_ERROR falls through deliberately: the workspace stays reachable and the
  // warning below explains why the sign-in state is unknown.
  if ((authState === 'UNAUTHENTICATED' || authState === 'SIGNED_OUT') && !skipLogin) {
    return <LoginView onSignedIn={() => { setSkipLogin(false); void refreshAccounts(); }} onSkip={() => setSkipLogin(true)} />;
  }

  return <main className={collapsed ? 'shell compact' : 'shell'} style={{ '--shell-sidebar': `${sidebarWidth}px` } as React.CSSProperties}>
    <header className="topbar">
      <button className="icon-button mobile-only" aria-label="Open navigation" aria-expanded={navOpen} onClick={() => setNavOpen(open => !open)}><Menu size={20} /></button>
      <div className="brand"><span className="brand-mark">R</span><span>relay</span></div>
      {/* The workspace control used to jump to Messages while looking like a
          switcher. It now opens what it claims to be: the workspace and the
          account signed in to it, with the places that belong to it. */}
      <div className="actions-anchor">
        <button className="workspace" aria-haspopup="menu" aria-expanded={workspaceOpen} onClick={() => setWorkspaceOpen(open => !open)}>Northstar <ChevronDown size={15} /></button>
        {workspaceOpen && <div className="actions-menu workspace-menu" role="menu">
          <div className="menu-heading"><strong>Northstar</strong><span>{accounts[0] ? `${accounts[0].displayName} · ${accounts[0].emailAddress}` : 'No account is signed in yet'}</span></div>
          <button role="menuitem" onClick={() => { setWorkspaceOpen(false); navigate('Inbox'); }}>Mail</button>
          <button role="menuitem" onClick={() => { setWorkspaceOpen(false); navigate('Messages'); }}>Messenger</button>
          <button role="menuitem" onClick={() => { setWorkspaceOpen(false); navigate('Settings'); }}>Manage accounts</button>
        </div>}
      </div>
      <label className="search"><Search size={17} /><input aria-label="Global search" placeholder="Search mail, messages, and people" onFocus={() => setPaletteOpen(true)} readOnly /><kbd>Ctrl K</kbd></label>
      <div className="top-actions">
        {authWarning && <span className="preview-badge auth-warning" role="status" title={authWarning}>Sign-in unverified</span>}
        {!native && <span className="preview-badge" title="Browser preview mode — no native backend is attached; data is in-memory and not saved.">Preview</span>}
        {/* Connectivity first, then the store's own facts — the reason the label
            can no longer say "Offline" on a machine that is online (BUG-015). */}
        <span className={`connection ${connection.tone}`} title={connection.hint} role="status"><i /> {connection.label}</span>
        <div className="actions-anchor">
          <button className={`presence presence-${presence}`} aria-haspopup="menu" aria-expanded={presenceOpen} title={`Presence: ${presence}`} onClick={() => setPresenceOpen(open => !open)}><i /></button>
          {presenceOpen && <div className="actions-menu presence-menu" role="menu">
            {PRESENCES.map(item => (
              <button key={item.value} role="menuitem" aria-current={presence === item.value} onClick={() => { setPresenceOpen(false); void changePresence(item.value); }}>
                <span className={`presence-icon presence-${item.value}`}><i /></span>
                <span className="menu-copy"><strong>{item.label}</strong><em>{item.hint}</em></span>
              </button>
            ))}
          </div>}
        </div>
        <button className="icon-button" aria-label="Notifications" onClick={() => setNotificationsOpen(open => !open)}><Bell size={19} />{unreadCount > 0 && <b>{unreadCount > 9 ? '9+' : unreadCount}</b>}</button>
        <button className="avatar" aria-label="Profile and settings" title="Profile and settings" onClick={() => navigate('Settings')}>{initials(accounts[0]?.displayName || 'Relay')}</button>
      </div>
    </header>
    {(workspaceOpen || presenceOpen) && <button className="menu-backdrop" aria-label="Close menu" onClick={() => { setWorkspaceOpen(false); setPresenceOpen(false); }} />}
    <aside className={`sidebar${navOpen ? ' open' : ''}`}>
      {/* The control stays reachable in both states: it is the only way back out
          of the collapsed rail, where the splitter is hidden. Its label and glyph
          follow the state so it never announces "Collapse" while expanding. */}
      <div className="compose-row"><button className="compose" onClick={() => openCompose()}><PenLine size={18} /><span>Compose</span></button><button className="collapse" aria-label={collapsed ? 'Expand navigation' : 'Collapse navigation'} aria-expanded={!collapsed} onClick={() => setCollapsed(open => !open)}>{collapsed ? <PanelLeftOpen size={16} /> : <PanelLeftClose size={16} />}</button></div>
      <NavSection title="MAIL" items={MAIL_VIEWS} active={isMail ? mailView : null} onSelect={label => navigate(label)} />
      <NavSection title="MESSENGER" items={MESSENGER_VIEWS} active={view} onSelect={label => navigate(label === 'Threads' || label === 'Channels' ? 'Messages' : label)} />
      <NavSection title="COMPANY" items={[{ label: 'Contacts', icon: Users }]} active={view} onSelect={label => navigate(label)} />
      <div className="sidebar-bottom">
        <button className="nav-item" onClick={() => navigate('Settings')}><Settings size={18} /><span>Settings</span></button>
        <button className="nav-item" aria-expanded={helpOpen} onClick={() => setHelpOpen(open => !open)}><CircleHelp size={18} /><span>Help &amp; feedback</span></button>
      </div>
    </aside>
    <PaneSplitter className="sidebar-splitter" value={sidebarWidth} min={200} max={320} defaultValue={248} label="Adjust navigation pane width" onChange={setSidebarWidth} onCommit={width => persistWidth('ui.sidebar_width', width)} />
    {navOpen && <button className="sidebar-backdrop" aria-label="Close navigation" onClick={() => setNavOpen(false)} />}
    {isMail ? <MailView key={navId} view={mailView} accountId={accounts[0]?.id} focusEmail={focusEmail} onCompose={openCompose} listWidth={mailListWidth} onListWidthChange={setMailListWidth} onListWidthCommit={width => persistWidth('ui.mail_list_width', width)} />
      : view === 'Messages' ? <MessengerWorkspace key={navId} focusChannel={focusChannel} focusConversation={focusConversation} initialError={dmError} />
      : view === 'Contacts' ? <ContactsView onCompose={openCompose} onMessageContact={openDirectMessage} />
      : view === 'Settings' ? <SettingsView onPreferenceChange={onPreferenceChange} onAccountsChanged={refreshAccounts} />
      : null}
    <footer className="statusbar">
      <span title={connection.hint}><i className={`dot ${connection.tone}`} /> {sync && sync.pending > 0 ? `${sync.pending} queued · ` : ''}{connection.label}</span>
      {/* One line, two jobs: it reports the last action the user ran from outside
          a mail view (a palette sync, a failed preference write) and otherwise
          the standing queue summary. */}
      <span aria-live="polite">{statusNote || (sync && sync.failed > 0 ? `${sync.failed} change${sync.failed === 1 ? '' : 's'} need retry` : 'All changes saved locally')}</span>
      <span><Command size={13} /> Ctrl K</span>
    </footer>
    {composer.open && <Composer onClose={closeCompose} initial={composer.initial} onQueued={queuedId => {
      // Show the message first, then transmit it: the pane the user lands on
      // reports the real outcome (delivered, or why it failed) as it happens.
      navigate('Sent', queuedId);
      void deliverQueuedMail([queuedId]).then(() => refreshSync()).catch(() => undefined);
    }} />}
    {notificationsOpen && <NotificationPanel onClose={() => setNotificationsOpen(false)} />}
    {helpOpen && <section className="help-popover" role="dialog" aria-label="Help and keyboard shortcuts">
      <header><strong>About Relay</strong><button onClick={() => setHelpOpen(false)} aria-label="Close help"><X size={16} /></button></header>
      <p>Relay stores your mail and conversations locally, offline. Mail is fetched over IMAP and outbound mail is delivered over SMTP using the signed-in account; company-server sync is still a planned phase.</p>
      <dl className="shortcut-list">
        <div><dt>Command palette</dt><dd>Ctrl K</dd></div>
        <div><dt>Compose email</dt><dd>Ctrl N</dd></div>
        <div><dt>Send email</dt><dd>Ctrl Enter</dd></div>
        <div><dt>Send chat message</dt><dd>Enter</dd></div>
        <div><dt>Close panel</dt><dd>Esc</dd></div>
      </dl>
    </section>}
    <SearchPalette open={paletteOpen} onClose={() => setPaletteOpen(false)} onNavigate={navigate} onCompose={openCompose} onToggleTheme={toggleTheme} onSyncNow={syncNow} />
  </main>;
  }

function NavSection({ title, items, active, onSelect }: { title: string; items: { label: AppView; icon: typeof Inbox }[]; active: AppView | null; onSelect: (view: AppView) => void }) {
  return <nav aria-label={title}><p className="nav-title">{title}</p>{items.map(({ label, icon: Icon }) => <button key={label} className={active === label ? 'nav-item active' : 'nav-item'} onClick={() => onSelect(label)}><Icon size={18} /><span>{label}</span></button>)}</nav>;
}