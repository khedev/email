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
