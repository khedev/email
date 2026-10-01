# Login Flow

How a mailbox becomes an authenticated account, and where each decision is made.

## End-to-end

```
User
 ↓
Login UI                              src/app/LoginView.tsx
 ↓
Provider Detection                    src-tauri/src/discover.rs
   provider DB → DNS MX → autoconfig XML (HTTPS) → hostname guesses
   ranked · de-duplicated · capped at 8 · plaintext never offered
 ↓
Authentication Method                 candidate.auth_method
   "oauth2"  → consumer Google mailbox: Google refuses IMAP passwords by policy
   "password" → everyone else, and Workspace domains (the admin decides)
 ↓
┌──────────────────────────── oauth2 ────────────────────────────┐
│ Google OAuth                                                  │
│   PKCE: verifier (OS RNG) + S256 challenge + random state     │
│   https://accounts.google.com/o/oauth2/v2/auth                │
│   scope: https://mail.google.com/ openid email                │
│   access_type=offline · prompt=consent                        │
│ ↓                                                             │
│ Browser  (system browser, never an embedded webview)          │
│ ↓                                                             │
│ Google  (consent: the user's own client shows unverified-app) │
│ ↓                                                             │
│ Callback  http://127.0.0.1:<ephemeral>                        │
│   ?code&state  → grant                                        │
│   ?error&state → immediate, classified refusal                │
│   anything else → answered and ignored (keep waiting)         │
│ ↓                                                             │
│ State Validation  (must equal this attempt's state)           │
│ ↓                                                             │
│ Token Exchange  POST oauth2.googleapis.com/token  over TLS    │
│   client_id + code_verifier (+ client_secret when configured) │
│ ↓                                                             │
│ IMAP OAuth proof  XOAUTH2   user=<addr>\x01auth=Bearer <tok>  │
│   imap.gmail.com:993 SSL — proven before anything is stored   │
│ ↓                                                             │
│ Secure Token Storage                                          │
│   refresh token → OS credential manager (Relay Mail OAuth)     │
│   client secret → OS credential manager (own entry)            │
│   client id     → settings row (not a secret)                  │
└───────────────────────────────────────────────────────────────┘
                  │
┌───────────────── password ─────────────────┐
│ verify_imap_login per candidate            │
│   probe_reachable (5 s)                    │
│   TLS (993) or STARTTLS (143)              │
│   LOGIN → LOGINDISABLED? AUTHENTICATE PLAIN│
│   refusal → classified, run stops          │
│   → OS credential manager (Relay Mail)     │
└────────────────────────────────────────────┘
 ↓
Account row  (keyring *reference* only, never a secret)
 ↓
Folders scaffolded   inbox · sent · drafts · archive · trash
 ↓
AuthState: INITIALIZING → AUTHENTICATED (or UNAUTHENTICATED)
 ↓
Mail retrieval   src-tauri/src/mailbox.rs      ← the step sign-in does NOT do
   SELECT INBOX · FETCH newest 50 by SEQUENCE range (BODY.PEEK — never marks mail read)
   ENVELOPE → sender/subject/message-id · FLAGS → read/starred · INTERNALDATE
   decode base64 / quoted-printable / RFC 2047 · HTML reduced to text
 ↓
Local store   emails keyed by IMAP UID (imap-<account>-<uid>), one transaction
   a repeat sync updates in place; local filing (archive/trash) is preserved
 ↓
Workspace  (Inbox shows the retrieved mail; Sync re-runs on demand)
```

## Decision points

| Decision | Where | Rule |
|---|---|---|
| Which server to try | `discover_candidates` | Ranked sources; provider DB wins, MX classifies hosted domains, autoconfig refines, guesses last; capped at 8 |
| Which auth method | `MailCandidate.auth_method` | `"oauth2"` only for `gmail.com` / `googlemail.com`; never guessed for Workspace custom domains |
| Whether to abort the run | `discover_and_verify` | A credential *verdict* (`Auth`) stops it; an unreachable host or a non-IMAP endpoint (`Protocol`) skips to the next candidate — a wrong host cannot mask the right one |
| What the user is told | `refusal_hint` / `refusal_hint_for` | Fixed, non-sensitive text. The *host* refines a generic refusal: on Google's own hosts, "the credentials were refused" becomes "Google does not accept account passwords for IMAP" |
| Whether the browser wait continues | `wait_for_code` | A refusal or a cancel returns at once; only a stray request keeps waiting |
| When anything is stored | `google_oauth_sign_in` / `store_password_account` | Only after the mailbox has actually authenticated; a failed persistence rolls the credential back |
| Which folders exist | `ensure_account_folders` | Five roles (inbox/sent/drafts/archive/trash) created on sign-in and again before each sync, so `emails_in_folder` always has something to match — an account with no folder rows would show an empty mailbox no matter what was stored |
| Which accounts a sync covers | `sync_mail` | Every signed-in account when no ids are given (the UI gives none). Never "the first account" — that list is ordered by display name, so a seed or alphabetically earlier account could be synced instead of the mailbox on screen |
| When mail is downloaded | `sync_mail` | The Mail view's **Sync** button, plus one automatic fetch per mount when the folder is empty (so the first failure is visible rather than console-only) |
| What a sync may overwrite | `upsert_fetched_messages` | Content and flags refresh; the **folder is left alone**, so archiving or trashing a message locally is never undone by the next sync |
| What an opened message with no body does | `fetch_email_body` → `mailbox::fetch_message_body` | A sync only covers the newest 50, so a row stored before bodies decoded (or outside that window) would never be repaired by syncing. Opening it downloads that one message by UID and stores the text, which does **not** queue an outbound change — remote truth, not a local edit. A message the server no longer holds reports that instead of showing a blank reader |
| How a message is matched across syncs | `emails.server_id` + id `imap-<account>-<uid>` | Keyed by IMAP UID, so a repeat sync updates in place instead of duplicating the mailbox |

## Cancellation and failures

| Outcome | Message | State left behind |
|---|---|---|
| User cancels in the browser (`access_denied`) | "Google sign-in was cancelled, or this account did not grant Relay access to Gmail." | Nothing stored |
| User presses Cancel in the app | "the Google sign-in was cancelled" | Nothing stored |
| Browser never returns | "timed out waiting for Google's browser response" (300 s bound) | Nothing stored |
| Stale/wrong `state` | answered and ignored; the wait continues | Nothing stored |
| Revoked/expired grant at IMAP | "…Relay's access has to be granted again" | Account marked `error` with a short reason |
| Gmail refuses a password | "…sign in with Google, or create an App Password…" | Nothing stored |
| Wrong host / proxy on the port | "…not with an IMAP login response…" + next candidate | Candidate skipped |
| Fetch fails: credential refused | "The mail server refused the stored credential while fetching mail. Sign in again." | Nothing written; the account row keeps its last state |
| Fetch fails: connection dropped | "The connection to the mail server dropped while fetching mail. Try again." | Nothing written |

## What is deliberately absent

- **No SMTP delivery**: `send_smtp` items stay queued. Retrieval exists; sending is the remaining transport half.
- **INBOX only**: the newest 50 messages, on demand. No Sent/Archive sync, no background or scheduled sync, no attachments.
- **No HTML rendering**: message bodies are decoded to text (HTML is reduced to text, `<script>`/`<style>` dropped), which keeps the reader's no-HTML guarantee and the CSP meaningful.
- **No embedded Google page**: authorization always happens in the system browser,
  which is what Google's policy for native clients requires.
- **No bundled client id**: each user supplies their own Google Cloud OAuth client
  (Google requires verified-app registration for the mail scope).