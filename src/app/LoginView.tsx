import { useEffect, useState } from 'react';
import { Eye, EyeOff, Inbox, Loader2, LockKeyhole, ShieldCheck } from 'lucide-react';
import { addEmailAccount, autoSignIn, cancelGoogleSignIn, commandError, forgetOAuthClientId, getAppSettings, googleOAuthSignIn, isNative, type Account, type CreateAccountInput } from '../platform/tauri';
import { looksLikeGoogleMailbox, oauthClientIdIssue, oauthClientSecretIssue } from './util';

const PROVIDERS: { id: string; label: string; imapHost: string; imapPort: number; smtpHost: string; smtpPort: number; encryption: string; hint?: string }[] = [
  { id: 'gmail', label: 'Gmail', imapHost: 'imap.gmail.com', imapPort: 993, smtpHost: 'smtp.gmail.com', smtpPort: 465, encryption: 'ssl',
    hint: 'Gmail refuses normal account passwords for IMAP. Turn on 2-Step Verification, create an App Password at myaccount.google.com/apppasswords, and sign in with it here. Google Workspace may also need IMAP enabled by the admin.' },
  { id: 'outlook', label: 'Outlook / Microsoft 365', imapHost: 'outlook.office365.com', imapPort: 993, smtpHost: 'smtp.office365.com', smtpPort: 587, encryption: 'tls',
    hint: 'Microsoft 365 often disables password sign-in for IMAP entirely. Use an App Password if your administrator allows one.' },
  { id: 'yahoo', label: 'Yahoo', imapHost: 'imap.mail.yahoo.com', imapPort: 993, smtpHost: 'smtp.mail.yahoo.com', smtpPort: 465, encryption: 'ssl',
    hint: 'Yahoo requires an App Password (Account security → Generate app password).' },
  { id: 'custom', label: 'Custom IMAP/SMTP', imapHost: '', imapPort: 993, smtpHost: '', smtpPort: 465, encryption: 'ssl' },
];

/// First-run email sign-in (shown until at least one account has signed in).
/// The password is verified against the real IMAP server by the native layer
/// and then stored in the OS credential manager; it is never kept in app state.
export function LoginView({ onSignedIn, onSkip }: { onSignedIn: (account: Account) => void; onSkip: () => void }) {
  const [provider, setProvider] = useState('gmail');
  const [form, setForm] = useState<CreateAccountInput>({ displayName: '', emailAddress: '', imapHost: 'imap.gmail.com', imapPort: 993, smtpHost: 'smtp.gmail.com', smtpPort: 465, encryption: 'ssl', authKind: 'password' });
  const [password, setPassword] = useState('');
  const [showPassword, setShowPassword] = useState(false);
  const [advanced, setAdvanced] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [oauthClientId, setOauthClientId] = useState('');
  const [oauthClientSecret, setOauthClientSecret] = useState('');
  const [oauthOpen, setOauthOpen] = useState(false);
  const [oauthRemembered, setOauthRemembered] = useState(false);
  const [showOauthSecret, setShowOauthSecret] = useState(false);
  const [oauthNotice, setOauthNotice] = useState('');
  const [oauthBusy, setOauthBusy] = useState(false);
  const [autoEmail, setAutoEmail] = useState('');
  const [autoPassword, setAutoPassword] = useState('');
  const [autoShowPassword, setAutoShowPassword] = useState(false);
  const [autoBusy, setAutoBusy] = useState(false);
  const [progress, setProgress] = useState('');

  // The Google OAuth client id is remembered after the first successful
  // sign-in (Thunderbird-style: configured once, never again).
  useEffect(() => {
    getAppSettings().then(entries => {
      const remembered = entries.find(entry => entry.key === 'oauth.client_id')?.value;
      if (remembered) { setOauthClientId(remembered); setOauthRemembered(true); }
    }).catch(() => undefined);
  }, []);

  // Live progress from the native discovery engine (no-op in browser preview).
  useEffect(() => {
    if (!isNative()) return;
    let unlisten: (() => void) | undefined;
    import('@tauri-apps/api/event').then(({ listen }) => listen<string>('auto-sign-in-progress', event => setProgress(event.payload)))
      .then(stop => { unlisten = stop; })
      .catch(() => undefined);
    return () => { unlisten?.(); };
  }, []);

  const applyProvider = (id: string) => {
    const preset = PROVIDERS.find(item => item.id === id) ?? PROVIDERS[PROVIDERS.length - 1];
    setProvider(id);
    setForm(current => ({ ...current, imapHost: preset.imapHost, imapPort: preset.imapPort, smtpHost: preset.smtpHost, smtpPort: preset.smtpPort, encryption: preset.encryption }));
  };

  const providerHint = PROVIDERS.find(item => item.id === provider)?.hint;
  // Google refuses IMAP password sign-ins for its consumer mailboxes, so the
  // login screen points those users at "Sign in with Google" instead of letting
  // them fill in a password that the server will always reject.
  const googleMailbox = looksLikeGoogleMailbox(autoEmail) || looksLikeGoogleMailbox(form.emailAddress);
  const clientIdIssue = oauthClientIdIssue(oauthClientId);
  const clientSecretIssue = oauthClientSecretIssue(oauthClientSecret);

  const signInWithGoogle = async () => {
    setError('');
    // Checked in the UI for instant feedback and again in the native layer
    // before a browser round-trip is started. The secret is optional, but a
    // value pasted into the wrong field is caught here.
    const issue = oauthClientIdIssue(oauthClientId) ?? oauthClientSecretIssue(oauthClientSecret);
    if (issue) { setOauthOpen(true); setError(issue); return; }
    setOauthNotice('');
    setOauthBusy(true);
    try {
      const account = await googleOAuthSignIn(oauthClientId.trim(), oauthClientSecret.trim() || undefined);
      onSignedIn(account);
    } catch (err) {
      console.error('[relay] google sign-in failed:', err);
      setError(commandError(err, 'Google sign-in failed. Check the OAuth client id and try again.'));
    } finally {
      setOauthBusy(false);
    }
  };

  // Stops waiting for the browser immediately, so a closed tab does not leave
  // the sign-in pending until the native timeout.
  const cancelGoogle = async () => {
    try { await cancelGoogleSignIn(); } catch (err) { console.error('[relay] cancelling the Google sign-in failed:', err); }
  };

  // Deletes the remembered client id from the settings table and the optional
  // client secret from the OS credential manager; already-signed-in accounts
  // keep their stored authorization.
  const forgetClientId = async () => {
    setError('');
    try {
      await forgetOAuthClientId();
      setOauthNotice('The saved Google client id and any stored client secret were removed from this device.');
    } catch (err) {
      console.error('[relay] clearing the remembered client id failed:', err);
      setError(commandError(err, 'The saved Google client id could not be cleared.'));
    }
    setOauthClientId(''); setOauthClientSecret(''); setOauthRemembered(false);
  };

  /// Thunderbird-style primary path: email + password, everything else is
  /// discovered and verified by the native engine.
  const signInAutomatically = async () => {
    setError('');
    setProgress('Looking up mail server settings…');
    if (!autoEmail.includes('@') || !autoPassword) {
      setError('Enter your email address and password — Relay finds the rest.');
      setProgress('');
      return;
    }
    setAutoBusy(true);
    try {
      const account = await autoSignIn(autoEmail.trim(), autoPassword);
      onSignedIn(account);
    } catch (err) {
      console.error('[relay] automatic sign-in failed:', err);
      setError(commandError(err, 'Sign-in failed. Try the manual setup or Google sign-in below.'));
      setProgress('');
    } finally {
      setAutoBusy(false);
    }
  };

  const submit = async () => {
    setError('');
    if (!form.displayName.trim() || !form.emailAddress.includes('@') || !form.imapHost.trim() || !form.smtpHost.trim() || !password) {
      setError('Fill in your name, email address, password, and server details.');
      return;
    }
    setBusy(true);
    try {
      const account = await addEmailAccount(form, password);
      onSignedIn(account);
    } catch (err) {
      console.error('[relay] email sign-in failed:', err);
      setError(commandError(err, 'Sign-in failed. Check the details and try again.'));
    } finally {
      setBusy(false);
    }
  };

  return <section className="login-screen">
    <form className="login-card" onSubmit={event => { event.preventDefault(); void submit(); }}>
      <header className="login-header">
        <span className="login-mark"><Inbox size={22} /></span>
        <h1>Sign in to Relay</h1>
        <p>Connect your email account. Everything is stored locally on this device.</p>
      </header>
      <div className="login-primary">
        <input placeholder="Email address" type="email" value={autoEmail} onChange={event => setAutoEmail(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); if (autoEmail.trim() && autoPassword) void signInAutomatically(); } }} aria-label="Email address" autoComplete="email" />
        <div className="password-row">
          <input placeholder="Password" type={autoShowPassword ? 'text' : 'password'} value={autoPassword} onChange={event => setAutoPassword(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); if (autoEmail.trim() && autoPassword) void signInAutomatically(); } }} aria-label="Password" autoComplete="current-password" />
          <button type="button" className="icon-button" aria-label={autoShowPassword ? 'Hide password' : 'Show password'} onClick={() => setAutoShowPassword(visible => !visible)}>{autoShowPassword ? <EyeOff size={16} /> : <Eye size={16} />}</button>
        </div>
        {progress && <p className="login-progress" aria-live="polite">{progress}</p>}
        {googleMailbox ? <>
          {/* Google first, because Google refuses a normal password for these
              mailboxes: leading with the password invited a refusal the user
              could do nothing about. The App Password route stays available for
              anyone who has created one. */}
          <button type="button" className="login-submit" disabled={oauthBusy} onClick={() => void signInWithGoogle()}>{oauthBusy ? <Loader2 size={16} className="spin" /> : <LockKeyhole size={16} />} {oauthBusy ? 'Waiting for Google…' : 'Continue with Google'}</button>
          {oauthBusy && <button type="button" className="oauth-forget" onClick={() => void cancelGoogle()}>Cancel</button>}
          <button type="button" className="login-advanced" disabled={autoBusy} onClick={() => void signInAutomatically()}>{autoBusy ? 'Signing in…' : 'Use an App Password instead'}</button>
        </> : <button type="button" className="login-submit" disabled={autoBusy} onClick={() => void signInAutomatically()}>{autoBusy ? <Loader2 size={16} className="spin" /> : <LockKeyhole size={16} />} {autoBusy ? 'Signing in…' : 'Sign in'}</button>}
      </div>
      <details className="login-more">
        <summary>Manual server settings (IMAP / SMTP)</summary>
      <div className="login-providers" role="radiogroup" aria-label="Email provider">
        {PROVIDERS.map(item => (
          <button type="button" key={item.id} role="radio" aria-checked={provider === item.id} className={provider === item.id ? 'active' : ''} onClick={() => applyProvider(item.id)}>{item.label}</button>
        ))}
      </div>
      {providerHint && <p className="login-hint">{providerHint}</p>}
      <div className="login-fields">
        <input placeholder="Display name" value={form.displayName} onChange={event => setForm({ ...form, displayName: event.target.value })} aria-label="Display name" autoComplete="name" />
        <input placeholder="Email address" type="email" value={form.emailAddress} onChange={event => setForm({ ...form, emailAddress: event.target.value })} aria-label="Email address" autoComplete="email" />
        <div className="password-row">
          <input placeholder="Password" type={showPassword ? 'text' : 'password'} value={password} onChange={event => setPassword(event.target.value)} aria-label="Password" autoComplete="current-password" />
          <button type="button" className="icon-button" aria-label={showPassword ? 'Hide password' : 'Show password'} onClick={() => setShowPassword(visible => !visible)}>{showPassword ? <EyeOff size={16} /> : <Eye size={16} />}</button>
        </div>
        <button type="button" className="login-advanced" aria-expanded={advanced} onClick={() => setAdvanced(open => !open)}>Server settings (IMAP / SMTP)</button>
        {advanced && <div className="login-server">
          <input placeholder="IMAP host" value={form.imapHost} onChange={event => setForm({ ...form, imapHost: event.target.value })} aria-label="IMAP host" />
          <input type="number" placeholder="IMAP port" value={form.imapPort} onChange={event => setForm({ ...form, imapPort: Number(event.target.value) })} aria-label="IMAP port" />
          <input placeholder="SMTP host" value={form.smtpHost} onChange={event => setForm({ ...form, smtpHost: event.target.value })} aria-label="SMTP host" />
          <input type="number" placeholder="SMTP port" value={form.smtpPort} onChange={event => setForm({ ...form, smtpPort: Number(event.target.value) })} aria-label="SMTP port" />
          <select value={form.encryption} onChange={event => setForm({ ...form, encryption: event.target.value })} aria-label="Encryption">
            <option value="ssl">SSL / implicit TLS</option>
            <option value="tls">TLS (STARTTLS)</option>
            <option value="none">None (not recommended)</option>
          </select>
        </div>}
      </div>
        <button className="login-submit" type="submit" disabled={busy}>{busy ? <Loader2 size={16} className="spin" /> : <LockKeyhole size={16} />} {busy ? 'Signing in…' : 'Sign in manually'}</button>
      </details>
      {googleMailbox && <p className="login-google-guidance" role="note">
        <strong>Gmail needs one extra step.</strong> Google does not accept a normal account password for Gmail over IMAP, so <strong>Continue with Google</strong> above is the way in — Relay never sees your password. To use a password instead, turn on 2-Step Verification, create an App Password at myaccount.google.com/apppasswords, and choose <em>Use an App Password instead</em>.
      </p>}
      <details className="login-more" open={oauthOpen} onToggle={event => setOauthOpen(event.currentTarget.open)}>
        <summary>Sign in with Google (OAuth 2.0)</summary>
        <div className="login-oauth">
          <strong>Uses your own Google Cloud OAuth client</strong>
          <p>Create one under <em>APIs &amp; Services → Credentials</em> (type <strong>Web application</strong>, authorized redirect URI <strong>http://127.0.0.1</strong>) and paste its client id below. For personal clients Google shows an unverified-app warning for the mail scope — choose <em>Advanced → Go to Relay</em>. The client id is remembered after a successful sign-in.</p>
          <div className="oauth-row">
            <input placeholder="Google OAuth client id (…apps.googleusercontent.com)" value={oauthClientId} onChange={event => setOauthClientId(event.target.value)} aria-label="Google OAuth client id" autoComplete="off" />
          </div>
          <div className="oauth-secret-row">
            <input placeholder="OAuth client secret (optional for Desktop app clients)" type={showOauthSecret ? 'text' : 'password'} value={oauthClientSecret} onChange={event => setOauthClientSecret(event.target.value)} aria-label="Google OAuth client secret" autoComplete="off" />
            <button type="button" className="icon-button" aria-label={showOauthSecret ? 'Hide client secret' : 'Show client secret'} onClick={() => setShowOauthSecret(visible => !visible)}>{showOauthSecret ? <EyeOff size={16} /> : <Eye size={16} />}</button>
            <button type="button" className="oauth-button" disabled={oauthBusy || !oauthClientId.trim()} onClick={() => void signInWithGoogle()}>{oauthBusy ? 'Waiting for Google…' : 'Continue with Google'}</button>
          </div>
          {(oauthClientId.trim() ? clientIdIssue : null) && <p className="oauth-issue">{clientIdIssue}</p>}
          {clientSecretIssue && <p className="oauth-issue">{clientSecretIssue}</p>}
          {oauthNotice && <p className="oauth-notice" role="status">{oauthNotice}</p>}
          <ul className="oauth-steps">
            <li>Google Cloud Console, APIs &amp; Services, Credentials: create an OAuth client of type Web application (which has a secret) or Desktop app.</li>
            <li>Add <em>http://127.0.0.1</em> as an authorized redirect URI and enable the Gmail API for the project.</li>
            <li>Personal clients show Google's unverified-app warning: choose <em>Advanced</em>, then <em>Go to Relay</em>.</li>
            <li>Neither value leaves this device: the client id is stored in local settings, the client secret in the OS credential manager.</li>
          </ul>
          {oauthRemembered && <button type="button" className="oauth-forget" onClick={() => void forgetClientId()}>Forget the saved Google client (id and secret)</button>}
        </div>
      </details>
      {error && <p className="login-error" role="alert">{error}</p>}
      <footer className="login-foot">
        <span><ShieldCheck size={14} /> Your password goes into the OS credential manager — never into this app's database.</span>
        <button type="button" className="login-skip" onClick={onSkip}>Skip for now — explore the workspace</button>
      </footer>
    </form>
  </section>;
}
