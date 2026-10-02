import { useEffect, useState } from 'react';
import { Bell, Database, HardDrive, KeyRound, MonitorCog, Moon, RefreshCw, RotateCcw, Sun, Trash2, UserRound, Wifi } from 'lucide-react';
import {
  addEmailAccount, clearCache, commandError, getAccounts, getAppSettings, getNotificationPreferences, getPresence, getStorageUsage, getSyncOverview,
  removeAccount, retryFailedSync, setAppSetting, setPresenceStatus, testEmailConnection, updateNotificationPreference,
  type Account, type CreateAccountInput, type DensityPreference, type PresenceStatus, type StorageUsage, type SyncOverview, type ThemePreference,
} from '../platform/tauri';
import { formatBytes } from './util';

const THEMES: { value: ThemePreference; label: string; icon: typeof Sun }[] = [
  { value: 'light', label: 'Light', icon: Sun },
  { value: 'dark', label: 'Dark', icon: Moon },
  { value: 'system', label: 'System', icon: MonitorCog },
];
const DENSITIES: { value: DensityPreference; label: string }[] = [
  { value: 'comfortable', label: 'Comfortable' },
  { value: 'compact', label: 'Compact' },
];
const PRESENCES: { value: PresenceStatus; label: string; hint: string }[] = [
  { value: 'online', label: 'Online', hint: 'Available for conversations' },
  { value: 'away', label: 'Away', hint: 'Temporarily unavailable' },
  { value: 'dnd', label: 'Do not disturb', hint: 'Pause mentions and alerts' },
  { value: 'offline', label: 'Offline', hint: 'Appear unavailable' },
];

export function SettingsView({ onPreferenceChange, onAccountsChanged }: { onPreferenceChange: (key: string, value: string) => void; onAccountsChanged?: () => void }) {
  const [prefs, setPrefs] = useState<Record<string, string>>({});
  const [notificationPrefs, setNotificationPrefs] = useState<Record<string, boolean>>({});
  const [presence, setPresence] = useState<PresenceStatus>('offline');
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [accountForm, setAccountForm] = useState<CreateAccountInput>({ displayName: '', emailAddress: '', imapHost: '', imapPort: 993, smtpHost: '', smtpPort: 465, encryption: 'tls', authKind: 'password' });
  const [accountPassword, setAccountPassword] = useState('');
  const [testingId, setTestingId] = useState<string | null>(null);
  const [usage, setUsage] = useState<StorageUsage | null>(null);
  const [sync, setSync] = useState<SyncOverview | null>(null);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');

  useEffect(() => { void refresh(); }, []);

  const refresh = async () => {
    setError(''); setNotice('');
    try {
      const entries = await getAppSettings();
      const values: Record<string, string> = {};
      for (const entry of entries) values[entry.key] = entry.value;
      setPrefs(values);
      try { const next = await getPresence(); setPresence(next.status); } catch { /* keep default */ }
      try {
        const rows = await getNotificationPreferences();
        const map: Record<string, boolean> = {};
        for (const row of rows) map[row.key] = row.enabled;
        setNotificationPrefs(map);
      } catch { /* keep default */ }
      setAccounts(await getAccounts());
      setUsage(await getStorageUsage());
      setSync(await getSyncOverview());
    } catch (err) { console.error('[relay] settings load failed:', err); setError('Settings could not be loaded from the local store.'); }
  };

  const applySetting = async (key: string, value: string) => {
    setPrefs(current => ({ ...current, [key]: value }));
    onPreferenceChange(key, value);
    try { await setAppSetting(key, value); } catch { setError('This setting could not be saved.'); }
  };

  const toggleNotification = async (key: string, enabled: boolean) => {
    setNotificationPrefs(current => ({ ...current, [key]: enabled }));
    try { await updateNotificationPreference(key, enabled); } catch { setError('Notification preference could not be saved.'); }
  };

  const changePresence = async (status: PresenceStatus) => {
    setPresence(status);
    try { await setPresenceStatus(status); setNotice('Presence updated.'); } catch { setError('Presence could not be updated.'); }
  };

  const addAccount = async () => {
    setError(''); setNotice('');
    if (!accountForm.displayName.trim() || !accountForm.emailAddress.includes('@') || !accountForm.imapHost.trim() || !accountForm.smtpHost.trim() || !accountPassword) { setError('Fill in every field, including the account password.'); return; }
    setNotice('Signing in to the mail server…');
    try {
      const created = await addEmailAccount(accountForm, accountPassword);
      // Re-signing into a known address returns the existing account — update
      // it in place instead of duplicating the list entry.
      setAccounts(items => items.some(item => item.id === created.id) ? items.map(item => item.id === created.id ? created : item) : [...items, created]);
      setAccountForm(current => ({ ...current, displayName: '', emailAddress: '', imapHost: '', smtpHost: '' }));
      setAccountPassword('');
      setNotice(`${created.displayName} signed in. The password lives in the OS credential manager — never in this database.`);
      // Let the shell's sign-in gate and avatar follow the change (BUG-016).
      onAccountsChanged?.();
    } catch (err) {
      console.error('[relay] account sign-in failed:', err);
      setError(commandError(err, 'The account could not be signed in. Check the server details.'));
    }
  };

  const runConnectionTest = async (id: string, displayName: string) => {
    setError(''); setNotice(''); setTestingId(id);
    try {
      const updated = await testEmailConnection(id);
      setAccounts(items => items.map(item => item.id === id ? updated : item));
      setNotice(`${displayName}: connection verified.`);
      onAccountsChanged?.();
    } catch (err) {
      console.error('[relay] connection test failed:', err);
      try { setAccounts(await getAccounts()); } catch { /* keep the current list */ }
      setError(commandError(err, `The connection test for ${displayName} failed.`));
    } finally { setTestingId(null); }
  };

  const dropAccount = async (id: string, displayName: string) => {
    setError(''); setNotice('');
    try { await removeAccount(id); setAccounts(items => items.filter(item => item.id !== id)); setNotice(`${displayName} was removed from this device.`); onAccountsChanged?.(); } catch { setError('The account could not be removed.'); }
  };

  const runClearCache = async () => {
    setError('');
    try { const freed = await clearCache(); setUsage(await getStorageUsage()); setNotice(`Cache cleared - ${formatBytes(freed)} freed.`); } catch { setError('The cache could not be cleared.'); }
  };

  const runRetry = async () => {
    setError('');
    try {
      const updated = await retryFailedSync();
      setSync(await getSyncOverview());
      setNotice(updated > 0 ? `${updated} queued change${updated === 1 ? '' : 's'} marked for retry.` : 'Nothing needed a retry.');
    } catch { setError('Retry could not be started.'); }
  };

  return <section className="settings-workspace">
    <header className="settings-header"><div><p className="eyebrow">PREFERENCES</p><h1>Settings</h1></div><button className="settings-refresh" onClick={() => void refresh()}><RotateCcw size={15} /> Refresh</button></header>
    {error && <p className="load-error">{error}</p>}
    {notice && <p className="settings-notice">{notice}</p>}

    <Card icon={MonitorCog} title="Appearance">
      <Row label="Theme" hint="Applies immediately">
        <div className="segmented">{THEMES.map(theme => <button key={theme.value} className={(prefs.theme ?? 'system') === theme.value ? 'active' : ''} onClick={() => void applySetting('theme', theme.value)}><theme.icon size={15} /> {theme.label}</button>)}</div>
      </Row>
      <Row label="Density" hint="Controls spacing across the interface">
        <div className="segmented">{DENSITIES.map(item => <button key={item.value} className={(prefs.density ?? 'comfortable') === item.value ? 'active' : ''} onClick={() => void applySetting('density', item.value)}>{item.label}</button>)}</div>
      </Row>
      <Row label="Font size" hint="Base text size">
        <select value={prefs.font_size ?? '13'} onChange={event => void applySetting('font_size', event.target.value)} aria-label="Font size">
          {['12', '13', '14', '15'].map(size => <option key={size} value={size}>{size}px</option>)}
        </select>
      </Row>
    </Card>

    <Card icon={UserRound} title="Presence">
      <Row label="Current status" hint="Presence syncs through the future company connection">
        <div className="segmented presence">{PRESENCES.map(option => <button key={option.value} className={presence === option.value ? 'active' : ''} title={option.hint} onClick={() => void changePresence(option.value)}>{option.label}</button>)}</div>
      </Row>
    </Card>

    <Card icon={Bell} title="Notifications">
      {Object.keys(notificationPrefs).length === 0 ? <p className="muted">No notification preferences configured yet.</p> :
        Object.entries(notificationPrefs).map(([key, enabled]) => (
          <Row key={key} label={labelFor(key)}>
            <button className="toggle" role="switch" aria-checked={enabled} onClick={() => void toggleNotification(key, !enabled)}><i className={enabled ? 'on' : ''} /></button>
          </Row>
        ))}
    </Card>

    <Card icon={KeyRound} title="Accounts">
      {accounts.length === 0 && <p className="muted">No mail accounts configured. Add one to start receiving and sending email.</p>}
      {accounts.map(account => (
        <div className="account-row" key={account.id}>
          <span className="mail-avatar">{initials(account.displayName)}</span>
          <span className="account-copy"><strong>{account.displayName}</strong><p>{account.emailAddress}</p></span>
          <em className={`account-state ${account.connectionState}`} title={account.connectionError ?? undefined}>{account.connectionState === 'connected' ? 'Connected' : account.connectionState === 'error' ? 'Error' : 'Not verified'}</em>
          <button className="ghost-button" aria-label={`Test connection for ${account.displayName}`} title="Test connection" disabled={testingId === account.id} onClick={() => void runConnectionTest(account.id, account.displayName)}><RefreshCw size={15} className={testingId === account.id ? 'spin' : undefined} /></button>
          <button className="danger-button" aria-label={`Remove ${account.displayName}`} title="Remove account" onClick={() => { if (window.confirm(`Remove ${account.displayName} from this device?`)) void dropAccount(account.id, account.displayName); }}><Trash2 size={15} /></button>
        </div>
      ))}
      <div className="account-form">
        <div className="contact-fields">
          <input placeholder="Display name" value={accountForm.displayName} onChange={event => setAccountForm({ ...accountForm, displayName: event.target.value })} aria-label="Display name" />
          <input placeholder="Email address" value={accountForm.emailAddress} onChange={event => setAccountForm({ ...accountForm, emailAddress: event.target.value })} aria-label="Email address" />
          <input type="password" placeholder="Password" value={accountPassword} onChange={event => setAccountPassword(event.target.value)} aria-label="Account password" autoComplete="current-password" />
          <input placeholder="IMAP host" value={accountForm.imapHost} onChange={event => setAccountForm({ ...accountForm, imapHost: event.target.value })} aria-label="IMAP host" />
          <input type="number" placeholder="IMAP port" value={accountForm.imapPort} onChange={event => setAccountForm({ ...accountForm, imapPort: Number(event.target.value) })} aria-label="IMAP port" />
          <input placeholder="SMTP host" value={accountForm.smtpHost} onChange={event => setAccountForm({ ...accountForm, smtpHost: event.target.value })} aria-label="SMTP host" />
          <input type="number" placeholder="SMTP port" value={accountForm.smtpPort} onChange={event => setAccountForm({ ...accountForm, smtpPort: Number(event.target.value) })} aria-label="SMTP port" />
          <select value={accountForm.encryption} onChange={event => setAccountForm({ ...accountForm, encryption: event.target.value })} aria-label="Encryption">
            <option value="tls">TLS (STARTTLS)</option>
            <option value="ssl">SSL / implicit TLS</option>
            <option value="none">None (not recommended)</option>
          </select>
        </div>
        <button className="contact-save" onClick={() => void addAccount()}><Database size={15} /> Sign in &amp; add account</button>
        <span className="muted">Signing in verifies the password against your IMAP server. The password is stored only in the OS credential manager (Windows Credential Manager) — never in this database.</span>
      </div>
    </Card>

    <Card icon={HardDrive} title="Storage">
      {usage ? <div className="usage-grid">
        <span>Database <em>{formatBytes(usage.database)}</em></span>
        <span>Attachments <em>{formatBytes(usage.attachments)}</em></span>
        <span>Avatars <em>{formatBytes(usage.avatars)}</em></span>
        <span>Cache <em>{formatBytes(usage.cache)}</em></span>
        <span>Logs <em>{formatBytes(usage.logs)}</em></span>
        <span>Total <em className="strong">{formatBytes(usage.total)}</em></span>
      </div> : <p className="muted">Measuring local storage…</p>}
      <button className="contact-save" onClick={() => void runClearCache()}><RefreshCw size={15} /> Clear cache</button>
    </Card>

    <Card icon={Wifi} title="Synchronization">
      {sync ? <>
        <Row label="Status" hint={sync.detail ?? undefined}>
          <span className={`connection ${sync.state}`}><i /> {stateLabel(sync.state)}</span>
        </Row>
        <Row label="Local queue">
          <span className="queue-stats"><em>{sync.pending}</em> pending · <em>{sync.failed}</em> failed · <em>{sync.conflicts}</em> conflicts</span>
        </Row>
        <button className="contact-save" onClick={() => void runRetry()} disabled={sync.failed === 0}><RotateCcw size={15} /> Retry failed changes</button>
      </> : <p className="muted">Reading synchronization state…</p>}
    </Card>
  </section>;
  }

function Card({ icon: Icon, title, children }: { icon: typeof Sun; title: string; children: React.ReactNode }) {
  return <section className="settings-card"><header><Icon size={16} /><h2>{title}</h2></header><div className="settings-body">{children}</div></section>;
}

function Row({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return <div className="settings-row"><span><strong>{label}</strong>{hint && <p>{hint}</p>}</span><div>{children}</div></div>;
}

function labelFor(key: string): string {
  const labels: Record<string, string> = { email: 'Email notifications', messenger: 'Messenger notifications', mentions: 'Mentions', thread_replies: 'Thread replies', sounds: 'Sounds' };
  return labels[key] ?? key.replaceAll('_', ' ');
}

function stateLabel(state: string): string {
  return state === 'online' ? 'Connected' : state === 'synchronizing' ? 'Syncing' : state === 'offline' ? 'Offline' : state === 'error' ? 'Sync error' : 'Connecting';
}

function initials(name: string) { const parts = name.split(/\s+/); return parts.slice(0, 2).map(part => part[0]).join('').toUpperCase(); }