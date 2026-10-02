/**
 * The one width at which the shell stops being a three-column desktop layout
 * and becomes a phone layout (drawer nav, list/detail swap, fullscreen
 * composer). Mail and Messenger both read it from JS to swap panes, so it is
 * declared once here: a second, hand-written copy of the query in either
 * component could drift from this one and from the `@media` rules without any
 * error until the panes stopped swapping.
 */
export const NARROW_QUERY = '(max-width: 800px)';

/** What the connection indicator needs to say, and how it should look.
 *
 *  Connectivity ("does this computer have a network?") and configuration ("is a
 *  company-server transport set up?") are different facts. The old indicator ran
 *  them together: it rendered "Offline" on every launch because the sync
 *  transport is never configured, even on a machine with working internet
 *  (BUG-015). `online` is the machine's own answer (`navigator.onLine`, kept
 *  live by the shell's listeners); the transport and queue facts come from the
 *  sync overview. */
export interface ConnectionInputs {
  online: boolean;
  busy: boolean;
  sync: { state: string; failed: number; detail?: string | null } | null;
  health: { state: string } | null;
}

export interface ConnectionView {
  label: string;
  tone: 'offline' | 'error' | 'synchronizing' | 'local' | 'connecting';
  hint: string;
}

/** No company transport is configured — a configuration fact, stated as one. */
const LOCAL_ONLY_HINT = 'No company-server transport is configured. Mail is fetched over IMAP and delivered over SMTP on demand; changes are saved on this device.';

export function connectionView({ online, busy, sync, health }: ConnectionInputs): ConnectionView {
  // A missing network is the only case that justifies the word "Offline".
  if (!online) return { label: 'Offline', tone: 'offline', hint: 'This computer has no network connection.' };
  // Work in progress outranks a stale failure count: something is happening now.
  if (busy || sync?.state === 'synchronizing') return { label: 'Syncing', tone: 'synchronizing', hint: 'Fetching mail and sending queued messages.' };
  if (sync?.state === 'error' || health?.state === 'error' || (sync?.failed ?? 0) > 0) return { label: 'Sync error', tone: 'error', hint: sync?.detail ?? 'Some changes could not be sent and need a retry.' };
  if (!sync && !health) return { label: 'Connecting', tone: 'connecting', hint: 'Reading the local status…' };
  return { label: 'Local only', tone: 'local', hint: LOCAL_ONLY_HINT };
}

export function initials(name: string): string {
  const parts = name.split(/\s+/).filter(Boolean);
  return parts.slice(0, 2).map(part => part[0]).join('').toUpperCase();
}

export function formatTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.valueOf())) return value;
  return new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit' }).format(date);
}

export function formatDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.valueOf())) return value;
  const today = new Date();
  const sameDay = date.toDateString() === today.toDateString();
  const sameWeek = (today.getTime() - date.getTime()) < 7 * 86_400_000 && date.getDate() !== today.getDate();
  if (sameDay) return formatTime(value);
  if (sameWeek) return new Intl.DateTimeFormat(undefined, { weekday: 'short' }).format(date);
  return new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short', year: date.getFullYear() === today.getFullYear() ? undefined : 'numeric' }).format(date);
}

/**
 * The reader needs the full stamp, not the list's shorthand: `formatDate`
 * collapses today's mail to a bare clock time and this week's to a weekday, so
 * a message opened on its own had no way to say which day it arrived.
 */
export function formatDateTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.valueOf())) return value;
  return new Intl.DateTimeFormat(undefined, {
    weekday: 'short',
    day: 'numeric',
    month: 'short',
    year: date.getFullYear() === new Date().getFullYear() ? undefined : 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  }).format(date);
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = bytes;
  let unit = -1;
  do { value /= 1024; unit += 1; } while (value >= 1024 && unit < units.length - 1);
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[unit]}`;
}

export function stripEmail(address: string): string {
  const match = address.match(/<([^>]+)>/);
  return (match ? match[1] : address).trim();
}

/** Parses a comma-separated recipient string into plain email addresses. */
export function parseAddresses(value: string): string[] {
  return value.split(',').map(address => address.trim()).map(cleanAddressLabel).filter(Boolean);
}

/** Reduces a display label like `Name <email>` (or a bare email) to an address. */
export function cleanAddressLabel(entry: string): string {
  const match = entry.match(/^\s*(.*?)\s*<([^>]+)>\s*$/);
  if (!match) return stripEmail(entry);
  const display = match[1];
  // Only keep the angle-bracket address; "Update: <addr>" and bare names
  // otherwise collapse to the raw address so recipients stay valid.
  return display ? match[2].trim() : stripEmail(entry);
}

/** Google consumer mailboxes, where Google refuses IMAP password sign-ins and
 *  requires an App Password or "Sign in with Google" (XOAUTH2). Custom Google
 *  Workspace domains are not guessed here: the native layer reports those from
 *  the server's own answer. */
export function looksLikeGoogleMailbox(address: string): boolean {
  const domain = address.trim().toLowerCase().split('@')[1];
  return domain === 'gmail.com' || domain === 'googlemail.com';
}

/** Checks the optional Google OAuth client secret before any browser
 *  round-trip. Empty is valid (Desktop app / PKCE clients have none); a pasted
 *  value must not be the client id or an API key. Mirrors
 *  `oauth::validate_client_secret` in the native layer — keep the wording of
 *  both in step. The value itself is never echoed back. */
export function oauthClientSecretIssue(value: string): string | null {
  const trimmed = value.trim();
  if (!trimmed) return null;
  if (trimmed.length < 16 || trimmed.length > 256) return 'That does not look like an OAuth client secret: the value has the wrong length.';
  if (/\s/.test(trimmed)) return 'The client secret contains whitespace — paste only the Client secret value.';
  if (trimmed.endsWith('.apps.googleusercontent.com')) return 'That is the OAuth client id, not the client secret. Put it in the Client ID field.';
  if (trimmed.startsWith('AIza')) return 'That is a Google API key, not an OAuth client secret.';
  return null;
}
/** Checks a pasted Google OAuth client id before any browser round-trip and
 *  returns a user-facing reason, or `null` when the value looks usable.
 *  Mirrors `oauth::validate_client_id` in the native layer — keep the wording
 *  of both in step. The value itself is never echoed back. */
export function oauthClientIdIssue(value: string): string | null {
  const trimmed = value.trim();
  if (!trimmed) return 'Paste the Google OAuth client id first (Cloud Console → APIs & Services → Credentials).';
  // A real client id is about 72 characters; the floor only rejects obvious
  // truncations while still letting a partial paste reach the suffix check below.
  if (trimmed.length < 32 || trimmed.length > 200) return 'That does not look like an OAuth client id: the value has the wrong length (they are about 72 characters).';
  if (/\s/.test(trimmed)) return 'The client id contains whitespace — paste only the Client ID value.';
  if (trimmed.startsWith('GOCSPX-')) return 'That is the OAuth client secret. Desktop app clients do not need it — use the Client ID, which ends with .apps.googleusercontent.com.';
  if (trimmed.startsWith('AIza')) return 'That is a Google API key. Use the OAuth 2.0 Client ID from APIs & Services → Credentials instead.';
  if (!trimmed.endsWith('.apps.googleusercontent.com')) return 'The OAuth client id must end with .apps.googleusercontent.com.';
  return null;
}
