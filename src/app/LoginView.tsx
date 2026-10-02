import { useEffect, useState } from 'react';
import { CircleAlert, Eye, EyeOff, Inbox, Loader2, LockKeyhole, Mail, ShieldCheck, Zap } from 'lucide-react';
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
/// Google's sign-in mark in the four official brand colours. Google's branding
/// rules require the multi-colour "G" on a sign-in button rather than a
/// monochrome glyph or the app's accent colour, so it is inlined instead of
/// being approximated with a lucide icon. Purely decorative — the button's own
/// label is the accessible name.
function GoogleMark() {
  return (
    <svg className="google-mark" width="18" height="18" viewBox="0 0 48 48" aria-hidden="true" focusable="false">
      <path fill="#EA4335" d="M24 9.5c3.54 0 6.71 1.22 9.21 3.6l6.85-6.85C35.9 2.38 30.47 0 24 0 14.62 0 6.51 5.38 2.56 13.22l7.98 6.19C12.43 13.72 17.74 9.5 24 9.5z" />
      <path fill="#4285F4" d="M46.98 24.55c0-1.57-.15-3.09-.38-4.55H24v9.02h12.94c-.58 2.96-2.26 5.48-4.78 7.18l7.73 6c4.51-4.18 7.09-10.36 7.09-17.65z" />
      <path fill="#FBBC05" d="M10.53 28.59c-.48-1.45-.76-2.99-.76-4.59s.27-3.14.76-4.59l-7.98-6.19C.92 16.46 0 20.12 0 24c0 3.88.92 7.54 2.56 10.78l7.97-6.19z" />
      <path fill="#34A853" d="M24 48c6.48 0 11.93-2.13 15.89-5.81l-7.73-6c-2.15 1.45-4.92 2.3-8.16 2.3-6.26 0-11.57-4.22-13.47-9.91l-7.98 6.19C6.51 42.62 14.62 48 24 48z" />
    </svg>
  );
}

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

  // Bento composition: the brand column is decorative, while every control lives
  // in the login card beside it, so no decoration can push the form out of the
  // viewport. Presentation only — the sign-in flow above is untouched.
  return <section className="login-screen">
    <div className="bento">
      <aside className="bento-brand bento-card">
        <div className="brand-topline">
          <span className="login-mark"><Inbox size={20} /></span>
          <span className="brand-wordmark">relay</span>
        </div>
        <div className="brand-copy">
          <h2>Your communication workspace</h2>
          <p>Mail and conversations, beautifully organized — stored locally on this device.</p>
        </div>
        <div className="bento-features">
          {/* Parallel, equal-length copy: the two cards sit side by side, so
              uneven descriptions made them wrap to different heights. */}
          <div className="feature-card">
            <span className="feature-icon"><ShieldCheck size={16} /></span>
            <strong>Secure</strong>
            <p>Stored in the OS credential manager.</p>
          </div>
          <div className="feature-card">
            <span className="feature-icon"><Zap size={16} /></span>
            <strong>Local-first</strong>
            <p>Available offline on this device.</p>
          </div>
        </div>
        <span className="brand-orb" aria-hidden="true" />
      </aside>
      <form className="login-card bento-card" onSubmit={event => { event.preventDefault(); void submit(); }}>
        <header className="login-header">
          <h1>Welcome back</h1>
          <p>Sign in to continue to Relay.</p>
        </header>
        <div className="login-primary">
          <div className="login-field">
            <label htmlFor="relay-email">Email address</label>
            <div className="field-shell">
              <Mail size={16} aria-hidden="true" />
              <input id="relay-email" placeholder="you@example.com" type="email" value={autoEmail} onChange={event => setAutoEmail(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); if (autoEmail.trim() && autoPassword) void signInAutomatically(); } }} aria-label="Email address" autoComplete="email" />
            </div>
          </div>
          <div className="login-field">
            <label htmlFor="relay-password">Password</label>
            <div className="password-row">
              <input id="relay-password" placeholder="Your account password" type={autoShowPassword ? 'text' : 'password'} value={autoPassword} onChange={event => setAutoPassword(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); if (autoEmail.trim() && autoPassword) void signInAutomatically(); } }} aria-label="Password" autoComplete="current-password" />
              <button type="button" className="icon-button" aria-label={autoShowPassword ? 'Hide password' : 'Show password'} onClick={() => setAutoShowPassword(visible => !visible)}>{autoShowPassword ? <EyeOff size={16} /> : <Eye size={16} />}</button>
            </div>
          </div>
          {progress && <p className="login-progress" aria-live="polite">{autoBusy && <Loader2 size={14} className="spin" />}{progress}</p>}
        {googleMailbox ? <>
          {/* Google first, because Google refuses a normal password for these
              mailboxes: leading with the password invited a refusal the user
              could do nothing about. The App Password route stays available for
              anyone who has created one. */}
          <button type="button" className="login-submit login-google" disabled={oauthBusy} onClick={() => void signInWithGoogle()}>{oauthBusy ? <Loader2 size={16} className="spin" /> : <GoogleMark />} {oauthBusy ? 'Waiting for Google…' : 'Continue with Google'}</button>
          {oauthBusy && <button type="button" className="oauth-forget" onClick={() => void cancelGoogle()}>Cancel</button>}
          <button type="button" className="login-advanced" disabled={autoBusy} onClick={() => void signInAutomatically()}>{autoBusy ? 'Signing in…' : 'Use an App Password instead'}</button>
        </> : <>
          {/* The Google button below calls the same `signInWithGoogle` as the
              one above, so the OAuth route is one click away for every mailbox.
              With no client id configured it opens the OAuth panel further
              down, exactly as before — no second sign-in implementation. */}
          <button type="button" className="login-submit" disabled={autoBusy} onClick={() => void signInAutomatically()}>{autoBusy ? <Loader2 size={16} className="spin" /> : <LockKeyhole size={16} />} {autoBusy ? 'Signing in…' : 'Continue'}</button>
          <div className="login-divider"><span>or</span></div>
          <button type="button" className="login-submit login-google" disabled={oauthBusy} onClick={() => void signInWithGoogle()}>{oauthBusy ? <Loader2 size={16} className="spin" /> : <GoogleMark />} {oauthBusy ? 'Waiting for Google…' : 'Continue with Google'}</button>
        </>}
      </div>
      {error && <p className="login-error" role="alert"><CircleAlert size={16} aria-hidden="true" />{error}</p>}
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
      <footer className="login-foot">
        <span><ShieldCheck size={14} /> Your password goes into the OS credential manager — never into this app's database.</span>
        <button type="button" className="login-skip" onClick={onSkip}>Skip for now — explore the workspace</button>
      </footer>
      </form>
    </div>
  </section>;
}
