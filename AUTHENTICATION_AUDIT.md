# Authentication Audit

Scope: the sign-in path of the Relay desktop app — Google OAuth 2.0 for Gmail,
IMAP/SMTP password authentication, the Thunderbird-style discovery flow, token
storage, the Tauri IPC surface, and the frontend sign-in state.

Method: the whole flow was traced from the login view to the SQLite write, then
each hop was exercised by the test suite or inspected against the pinned
dependency sources. Nothing below is marked FUNCTIONAL merely because code
exists.

---

## 1. Current Authentication Architecture

Email access and application identity are **one operation in this app**, not two:
there is no separate "sign in to Relay" step. Signing in *is* connecting a
mailbox, and the account row is the session. There is no password account
(`auth_kind: "password"`) and no Google account (`auth_kind: "oauth2"`) that
behaves differently at the UI level — both produce the same `accounts` row.

```
React (LoginView)
   │  platform/tauri.ts  — typed wrappers, browser preview fallback
   ▼
Tauri IPC (49 commands, all guarded by `guard::guarded` where they can panic)
   │
   ├── add_email_account ──► verify_imap_login ──► OsKeyring.save ──► accounts row
   ├── auto_sign_in ───────► discover.rs (worker thread) ──► same as above
   ├── google_oauth_sign_in ► oauth.rs (PKCE + loopback) ──► verify_xoauth2
   │                                                   └──► keyring (OAuth service)
   └── test_email_connection ► refresh/XOAUTH2 or LOGIN, then records the state
   │
   ▼
SQLite (accounts row + a keyring *reference* only)  ·  OS credential manager
```

| Component | Status | Notes |
|---|---|---|
| Login UI (email + password) | ✅ FUNCTIONAL | Real IMAP LOGIN, classified failures |
| Login UI (Google OAuth pane) | ✅ FUNCTIONAL | PKCE S256, state, loopback callback |
| Provider detection (provider DB / MX / autoconfig / guesses) | ✅ FUNCTIONAL | Offline-computable logic is spec-tested |
| Authentication-method selection | ✅ FUNCTIONAL (was ❌ BROKEN) | `MailCandidate.auth_method` + host-aware refusal (§3) |
| Google OAuth code exchange | ✅ FUNCTIONAL (not live-verified) | Needs a real client id + account |
| Gmail IMAP via XOAUTH2 | ⚠️ PARTIAL | Proof-of-access works; **no fetch worker**, so no mail is downloaded |
| Gmail SMTP via XOAUTH2 | 🚧 NOT IMPLEMENTED | `send_smtp` stays queued; no transport exists |
| Token storage (refresh token) | ✅ FUNCTIONAL | OS credential manager, separate service |
| Client secret storage | ✅ FUNCTIONAL | Its own keyring entry; never in SQLite |
| Credential deletion | ✅ FUNCTIONAL | `forget_oauth_client_id`, `remove_account` purge both keyrings |
| Token refresh | ✅ FUNCTIONAL | On demand (`test_email_connection`), rotation persisted |
| Sign-out / switch account | ⚠️ PARTIAL | Explicit `AuthState` now exists; no user-facing "Sign out" action (BUG-016) |
| Multi-account isolation | ✅ FUNCTIONAL | Keyring key = email address; accounts are separate rows |
| Auth state machine | ✅ FUNCTIONAL (was ⚠️ PARTIAL) | Typed states + visible fail-open warning (§10) |
| Provider-config redirect (cleartext autoconfig) | ✅ FIXED | Removed (§11, SEC-001) |
| Tauri capabilities / CSP | ✅ FUNCTIONAL | Least privilege, tight CSP (§9) |
| Zombie-port response on transports | ✅ FUNCTIONAL | Browser preview mirrors the command contract |

---

## 2. Current Login Flow

```
LoginView
  ├─ Thunderbird path:  email + password → auto_sign_in
  │     discover_candidates(address)
  │       provider_candidates → mx_candidates → autoconfig_candidates → guess_candidates
  │       (ranked, de-duplicated, capped at 8, plaintext never offered)
  │     for each candidate: verify_imap_login(address, password, host, port, encryption)
  │       ├─ probe_reachable (5 s)         → Unreachable  (skip candidate)
  │       ├─ TLS/STARTTLS connect
  │       ├─ LOGINDISABLED advertised? → AUTHENTICATE PLAIN
  │       ├─ LOGIN → on failure, retry PLAIN, classify
  │       └─ Ok / Auth / Protocol / Unreachable
  │     first candidate that accepts → store_password_account (keyring + row)
  ├─ Manual path:       display name + email + password + host/port → add_email_account
  └─ Google path:       OAuth pane → google_oauth_sign_in
        validate client id (+ optional secret)
        bind 127.0.0.1:<ephemeral> · open system browser (authorization_url)
        wait_for_code(300 s)   [worker thread]
          ├─ ?code&state  → exchange_code (PKCE verifier + optional client secret)
          ├─ ?error&state → immediate classified refusal
          └─ cancel flag  → immediate cancellation
        email_from_id_token → verify_xoauth2 (imap.gmail.com:993)
        save_oauth_secret (keyring) + settings oauth.client_id → accounts row
```

---

## 3. Exact Failure Point

A Gmail user following the app's *primary* button ("Sign in") got:

> The mail server rejected this sign-in (the credentials were refused). Large
> providers no longer accept normal account passwords for IMAP…

That string is `VerifyFailure::Auth("the credentials were refused").user_message()`
from `src-tauri/src/verify.rs`, and the parenthesised hint is its **generic**
branch — no Google-specific pattern matched. Traced backwards:

| # | Step | Result |
|---|---|---|
| 1 | `LoginView` primary button → `signInAutomatically()` | ✅ |
| 2 | `autoSignIn()` → IPC `auto_sign_in` | ✅ |
| 3 | `discover_and_verify()` | ✅ candidates found |
| 4 | `provider_candidates("gmail.com")` → `imap.gmail.com:993 ssl` | ✅ correct server |
| 5 | `verify_imap_login(address, password, …)` → `client.login()` | ❌ **FIRST FAILED STEP** |
| 6 | Gmail answers `NO [AUTHENTICATIONFAILED] Invalid credentials (failure)` | Google policy, not a typo |
| 7 | `refusal_hint()` → generic branch → `"the credentials were refused"` | ❌ unactionable |
| 8 | `discover_and_verify` aborts the run with that message | ❌ dead end |
| 9 | `LoginView` renders it | symptom reported |

Everything after step 5 was correct; the run simply never had a route that could
succeed for this account type.

## 4. Root Cause

**The flow selected a server but never selected an authentication method.**
`discover_candidates` identified `imap.gmail.com` correctly, and then
`discover_and_verify` unconditionally attempted a **password** IMAP LOGIN for an
account whose provider refuses passwords by policy. `MailCandidate` carried host,
port, encryption, provider and source — but no auth method (`auth_method` had
zero matches repo-wide). The step Thunderbird performs between "provider
configured" and "credentials entered" was missing.

Two contributing defects made the dead end opaque:

1. `classify_login_error(&error, host, port, encryption)` already received the
   host and **discarded it for the message**, using it only for `tracing`. It
   knew the host was `imap.gmail.com` and still reported a verdict
   indistinguishable from a mistyped password.
2. `discover.rs` aborted the whole run on any `Auth` refusal, so the user was
   never told that a different *method* was available.

## 5. Google OAuth Problems

| Problem | Severity | Status |
|---|---|---|
| Browser round-trip ran inline on the WebView2 UI thread, blocking the window for up to `AUTH_TIMEOUT` (300 s) | HIGH | ✅ Fixed — worker thread + `oneshot` |
| Consent refusal (`?error=access_denied`, no `code`) was treated as "nothing arrived yet", so cancellation waited out the full timeout and then reported a *timeout* | HIGH | ✅ Fixed — callback classification |
| No way to cancel a pending sign-in | MEDIUM | ✅ Fixed — `cancel_google_sign_in` + `AtomicBool` |
| A "Web application" client (which Google requires to present its secret) could not complete a sign-in | HIGH | ✅ Fixed earlier — secret field, keyring-stored, joined to exchange + refreshes |
| Remembered client id could not be cleared | MEDIUM | ✅ Fixed — `forget_oauth_client_id` deletes row + secret |
| Generic token-error messaging | LOW | ✅ Fixed — fixed hints (bad id / bad secret / redirect / cancelled / revoked / scope / API not enabled) |
| PKCE S256, `state`, loopback-only callback, TLS exchange, `prompt=consent&access_type=offline` | — | ✅ Already correct, unchanged |

## 6. IMAP Problems

| Problem | Severity | Status |
|---|---|---|
| No auth-method selection for providers that require OAuth | **CRITICAL** (root cause) | ✅ Fixed — `auth_method` on the discovered candidate |
| Gmail password refusal classified as an ordinary bad password | HIGH | ✅ Fixed — `refusal_hint_for(host, …)` refines it on Google hosts |
| Discovery aborted on the refusal with no route forward | HIGH | ✅ Fixed — Google mailboxes get an explicit "sign in with Google" route |
| `test_email_connection` held the global DB mutex across HTTPS refresh + IMAP, blocking all 49 commands (BUG-004 / SEC-003) | HIGH | ✅ Fixed — snapshot → unlock → network → relock |
| Cleartext `http://autoconfig.<domain>` fallback could redirect the password to an attacker's IMAP host (SEC-001) | MEDIUM | ✅ Fixed — HTTPS only |
| XOAUTH2 refusal reported as a rejected password | MEDIUM | ✅ Fixed earlier — `VerifyFailure::GoogleAuth` |
| No IMAP fetch worker: nothing is downloaded, so an authenticated account still shows an empty inbox | ✅ FIXED | `mailbox.rs` fetches the newest 50 inbox messages; the Mail view has a Sync action and one automatic fetch after sign-in |
| A signed-in account had **no folder rows**, and `emails_in_folder` filters on the folder role — so its inbox was empty by construction | ✅ FIXED | `Repositories::ensure_account_folders` creates the five role folders on sign-in and again before each sync (repairs existing accounts) |
| A sync could drag a locally archived/trashed message back into the inbox | ✅ FIXED | The upsert refreshes content and flags but never re-files: the user's folder assignment wins |
| Message bodies from the server are untrusted input | ✅ Handled | MIME decoded to plain text only; HTML is reduced to text (never rendered), `<script>`/`<style>` content dropped, 512 KB body cap, 6-level MIME depth cap, no raw body in logs or errors |
| A sync downloaded nothing but reported success: the FETCH range came from `inbox.exists` (sequence space) and was sent to `UID FETCH` (UID space), which matches nothing once a mailbox's UIDs exceed its size | ✅ FIXED | `newest_sequence_range` feeds the sequence-based `Client::fetch`; a short FETCH now logs requested/received counts (numbers only) |
| The sync targeted one account chosen as `accounts[0]` from a **display-name-ordered** list, so a seed or alphabetically earlier account could be synced instead of the mailbox on screen | ✅ FIXED | `sync_mail` takes optional ids and defaults to **every** signed-in account, with per-account failures reported |
| The automatic first sync failed into the console only | ✅ FIXED | It now runs in the Mail view and renders in the note/progress UI |

## 7. SMTP Problems

| Problem | Severity | Status |
|---|---|---|
| No SMTP transport at all | 🚧 NOT IMPLEMENTED | `send_smtp` queue items stay `pending` by design |
| Gmail SMTP XOAUTH2 | 🚧 NOT IMPLEMENTED | Required for a production Gmail client |
| Account row already stores `smtp_host`/`smtp_port` (465/587) | ✅ Data present | The discovered config matches what Thunderbird would store |
| No Google password is ever requested or stored | ✅ Correct | Sign-in uses OAuth/XOAUTH2; App Passwords go to the keyring, not SQLite |

Sending mail cannot be made to work by fixing sign-in: the transport is absent.
This is reported as a scope gap, not as a login defect.

## 8. Token Storage Problems

| Checked | Result |
|---|---|
| Refresh token | OS credential manager, service `Relay Mail OAuth`, keyed by email address |
| Access token | Memory only; never persisted, never logged |
| Client secret | Its own keyring entry (`oauth.client_secret`), never in SQLite |
| Client id | `settings` row (`oauth.client_id`) — not a secret, and clearable |
| `localStorage` / `sessionStorage` / cookies | **0 matches** repo-wide |
| SQLite plaintext secrets | Only a keyring *reference* (`keyring:…`) is stored |
| `.env` / `VITE_*` | **None present**; a desktop frontend is not secret storage |
| Logs | No credential is ever passed to `tracing`; only fixed classifications (§11) |

No token-storage defect was found. The remaining gap is orthogonal: cached mail in
SQLite is unencrypted (SEC-002, documented, unchanged by this work).

## 9. Tauri Security Problems

| Checked | Result |
|---|---|
| Capabilities | `core:default`, `opener:default`, `notification:default` — least privilege, no `allow-all` |
| Commands exposed | 50 typed commands; all arguments validated natively (length, empty, `@`, ports, enum values) — the frontend is never trusted |
| Command panics | `guard::guarded` on every command that touches network/IMAP/keyring |
| CSP | `default-src 'self'`; `connect-src 'self' ipc: http://ipc.localhost`; no remote script/connect |
| Filesystem / shell / HTTP plugins | Not enabled |
| Deep links / custom URL scheme | Not used — the loopback listener is bound to `127.0.0.1` only |
| Blocking work on the IPC thread | ✅ Fixed for `google_oauth_sign_in` (worker thread) and `test_email_connection` (lock released). `add_email_account` already verified before locking |
| Native command error text | Fixed, non-sensitive strings; raw server responses are never returned or logged |

## 10. Frontend Authentication Problems

| Problem | Severity | Status |
|---|---|---|
| Sign-in state was one boolean plus a fail-open `catch`, so "checking", "no account", "signed in" and "store unreadable" were indistinguishable | MEDIUM | ✅ Fixed — typed `AuthState` |
| Fail-open was silent: a broken store presented a normal workspace | MEDIUM | ✅ Fixed — visible "Sign-in unverified" badge with the reason |
| Removing an account left the gate/avatar stale until restart (BUG-016) | MEDIUM | ✅ Fixed — `refreshAccounts` re-runs after every account mutation |
| The Gmail password path was the *primary* action for a Gmail address, inviting a guaranteed refusal | HIGH | ✅ Fixed — "Continue with Google" is primary for Google consumer mailboxes; "Use an App Password instead" remains |
| No way out of a pending browser wait | MEDIUM | ✅ Fixed — Cancel action |
| Token/secret access from the frontend | ✅ Correct | Long-term tokens never reach React; only typed command results do |
| Vague errors | ✅ Already good | `commandError` surfaces the backend's classified message |

## 11. Security Vulnerabilities

Reviewed specifically against the requested checklist:

| Vector | Finding |
|---|---|
| OAuth CSRF | ✅ `state` is a fresh UUIDv4 per attempt, compared before the code is accepted; a mismatched state is answered and ignored |
| Callback replay / stray callbacks | ✅ Only a `state` match is accepted; any local process can reach the loopback port, so that check is what protects the exchange — and the refusal path never echoes the value it received |
| Callback for another provider | ✅ The listener is created per attempt on a random `127.0.0.1` port; the request must match its own state |
| PKCE | ✅ S256, verifier from the OS RNG inside the 43–128 range, single use, never logged, never persisted |
| Token leakage in URLs | ✅ Tokens travel in the POST body over TLS; only the authorization *code* appears in a URL, and it is single-use |
| Authorization code expiry/reuse | ✅ Surfaced as a fixed hint (`invalid_grant` → re-authorize) |
| Token storage / credential leakage | ✅ See §8 — keyring only |
| Sensitive logs | ✅ `tracing` receives host/port/encryption and fixed classifications only. The loopback refusal logs a fixed sentence, never the received value |
| Multiple accounts | ✅ Keyring key is the email address; refresh tokens never mix |
| XSS | ✅ No `dangerouslySetInnerHTML` / `innerHTML` anywhere; CSP blocks remote script |
| HTML email / script in mail body | ✅ Not currently reachable (no fetch transport; the reader renders `bodyText` as text). **Must be treated as untrusted input when the fetch worker lands** |
| SQL injection | ✅ Parameterised throughout; the only interpolated SQL is a whitelisted role/enum |
| Path traversal / arbitrary file access | ✅ No command accepts a path; `clear_cache` derives its path from `app_data_dir` |
| Command injection | ✅ No shell is used |
| IPC abuse / unauthorized commands | ✅ Every command validates its own input; the surface is fixed at build time |
| Excessive capabilities | ✅ None — three `*:default` permission sets |
| Insecure redirects (SEC-001) | ✅ **Fixed** — the cleartext `http://autoconfig.<domain>` fallback was removed, since a network attacker answering for it could redirect the password to their own IMAP host |
| Password over plaintext IMAP | ✅ Plaintext is never auto-tried by discovery, and is an explicit "not recommended" choice |
| Dependency vulnerabilities | ⚠️ NOT AUDITED in this pass (no `cargo audit`/`npm audit`); `imap` is a 3.0.0-alpha |
| Data at rest (SEC-002) | ⚠️ OPEN — cached mail, contacts and messages are unencrypted in SQLite; documented, not changed here |
| Logout / session teardown | ⚠️ PARTIAL — `remove_account` purges both keyring entries, but there is no user-facing "Sign out" and the Google grant is not revoked at Google (§15) |

No remotely exploitable defect was found in the sign-in path.

## 12. Recommended Architecture

The flow now implements the Thunderbird model end to end:

```
Email address
   ↓
Provider detection            provider DB → MX → autoconfig (HTTPS) → guesses
   ↓
Authentication method         candidate.auth_method: "oauth2" | "password"
   ↓
OAuth when required           consumer Google mailbox → browser authorization
   ↓
Browser + PKCE S256 + state   system browser, loopback callback on 127.0.0.1
   ↓
Callback validation           code → exchange · error → immediate refusal · cancel → immediate
   ↓
Token exchange over TLS       client id (+ optional secret from the keyring)
   ↓
IMAP proof before storage     XOAUTH2 against imap.gmail.com:993
   ↓
OS credential storage         refresh token in the keyring; client id in settings
   ↓
Account row + explicit state  AuthState: INITIALIZING → AUTHENTICATED / UNAUTHENTICATED
```

## 13. Changes Implemented

| File | Change |
|---|---|
| `src-tauri/src/discover.rs` | `MailCandidate.auth_method` (+ `requires_oauth`); Gmail flagged `"oauth2"`, Workspace and everything else `"password"`; progress text names the method; a Google refusal is returned with the OAuth route; **cleartext autoconfig fallback removed (SEC-001)** |
| `src-tauri/src/verify.rs` | `refusal_hint_for(host, …)` + `is_google_mail_host`: a generic refusal on a Google host becomes "Google does not accept account passwords for IMAP — sign in with Google, or create an App Password". Used by `classify_login_error`; `refusal_hint` itself is unchanged (its wording is contract-tested) |
| `src-tauri/src/oauth.rs` | `LoopbackCallback` + `parse_loopback_callback` (code / refusal / unknown) + `loopback_refusal_hint`; `wait_for_code(timeout, cancelled)` returns immediately on a refusal or a cancel; `new_cancel_flag()` |
| `src-tauri/src/commands.rs` | `google_oauth_sign_in` is now `async` and runs the browser half on a `relay-oauth` worker thread (guarded inside); `authorize_with_google` helper; new `cancel_google_sign_in`; `test_email_connection_inner` snapshots under the lock, releases it for the network phase, and re-locks only to record |
| `src-tauri/src/lib.rs` | `AppState.oauth_cancel: Arc<AtomicBool>`; `cancel_google_sign_in` registered |
| `src/platform/tauri.ts` | `cancelGoogleSignIn()` |
| `src/platform/preview.ts` | `cancel_google_sign_in` handler (parity with the native host) |
| `src/app/LoginView.tsx` | For a Google mailbox: **Continue with Google** is the primary action, "Use an App Password instead" secondary, and Cancel while waiting; guidance text updated to match |
| `src/app/App.tsx` | Typed `AuthState`, `refreshAccounts` shared by the gate and every mutation, visible "Sign-in unverified" badge instead of a silent fail-open |
| `src/app/SettingsView.tsx` | `onAccountsChanged` fired after adding, re-verifying and removing an account |
| `src/styles/app.css` | `.auth-warning` badge style |
| `src-tauri/tests/local_store.rs` | `gmail_password_refusal_is_routed_to_google_sign_in`, `oauth_loopback_separates_grants_from_refusals` |
| `src/platform/preview.test.ts` | Cancel-command parity test |
| `AUTHENTICATION_AUDIT.md`, `LOGIN_FLOW.md` | This audit and the flow document |

| `src-tauri/src/mailbox.rs` (new) | IMAP retrieval: `fetch_inbox` (sequence-range FETCH of the newest 50 with `BODY.PEEK[]` — the whole message, read through `Fetch::body()`; asking for `BODY[HEADER]`/`BODY[TEXT]` and reading `body()` is the BUG-022 mismatch that stored an empty body for every message), `FETCH_FULL_MESSAGE` (the item, with the reason it must stay unsectioned), `assemble_raw_message` (rebuilds the raw message from whichever sections were returned), `fetch_message_body` (one message by UID, the on-demand repair path), `newest_sequence_range` (the ID-space helper), `note_partial_fetch`, `collect_messages` (envelope → sender/subject/message-id, flags → read/starred, INTERNALDATE; logs a count when a body decodes empty), and fixture-tested MIME helpers (`text_body_of`, `decode_base64`, `decode_quoted_printable`, `decode_mime_words`, `html_to_text`) |
| `src-tauri/src/verify.rs` | `MailCredential` + `open_mail_session` (one login path shared by verification and fetch); `verify_imap_login` / `verify_xoauth2` became thin wrappers; `connect_imap`/`probe_reachable` exported |
| `src-tauri/src/repositories.rs` | `ensure_account_folders` (called from `add_account_verified`), `account_ids` (the sync work list), `upsert_fetched_messages` (transactional, UID-keyed, preserves local filing, skips deleted rows), `store_email_body` (writes a body decoded from IMAP without queueing an outbound change), `email_fetch_target` (the account + UID a per-message fetch needs), `sync_metadata` / `set_sync_metadata` |
| `src-tauri/src/commands.rs` | `sync_mail` command (optional ids → every account; per-account worker thread, guarded, progress events, lock-free fetch, per-account failure reporting) + `fetch_email_body` (the same pattern for one message, repairing a row stored with an empty body) + `load_mail_credential` |
| `src/app/MailView.tsx` | Sync action with live progress, one automatic fetch per mount when the folder is empty, an inbox empty-state that distinguishes "not synced yet" from "no account", and an on-open body download for a message whose stored body is empty (with a visible note while it runs and an honest "no readable content" state) |
| `src/app/App.tsx` | Typed `AuthState` and account refresh; the console-only auto-sync was removed (it now lives in MailView where failures are visible) |
| `src/platform/tauri.ts` / `preview.ts` | `syncMail(accountIds?)`, `fetchEmailBody(id)` and their preview handlers |
| `src/styles/app.css` | `.mail-sync` / `.mail-sync-note` / `.auth-warning` styles |
| `AUTHENTICATION_AUDIT.md`, `LOGIN_FLOW.md`, `AUDIT_REPORT.md`, `COMPLETE_AUDIT.md`, `DEVELOPMENT.md`, `README.md`, `ARCHITECTURE.md` | This audit and the flow document |

## 14. Tests Performed

| Command | Result |
|---|---|
| `cargo test` (in `src-tauri`) | **38 passed, 0 failed** |
| `cargo check --all-targets` | Clean, no warnings |
| `npx tsc -b` | Exit 0, no diagnostics |
| `npx vitest run` | **26 passed** (2 files) |
| `npm run build` | Production bundle builds |

New/updated coverage:

- `gmail_password_refusal_is_routed_to_google_sign_in` — the Google host refines a
  generic refusal into an actionable one; another provider's host keeps the
  generic verdict; a provider alert that already explains itself is **not**
  overwritten; the server's words are never echoed; `auth_method` is `"oauth2"`
  for `gmail.com` and `"password"` for `yahoo.com` and for Workspace MX.
- `oauth_loopback_separates_grants_from_refusals` — code/refusal/unknown are told
  apart, `error_description` stands in for a missing code, stray requests stay
  unknown (so the wait continues), and refusal classification never echoes the
  value.
- Preview OAuth parity — including the new cancel command.

**Executed by reasoning/inspection only (cannot be automated here):** the live
Google round-trip, live IMAP/SMTP, and any GUI interaction.

## 15. Remaining Issues

| Issue | Severity | Why it is not fixed here |
|---|---|---|
| No IMAP fetch worker | ✅ FIXED | Retrieval now exists (newest 50 INBOX messages, manual Sync + one auto-fetch after sign-in). Attachments, other folders and a scheduled sync are still absent |
| No SMTP transport / Gmail SMTP XOAUTH2 | HIGH (product) | Absent by design (`send_smtp` stays queued) — sending is now the only missing transport half |
| No user-facing "Sign out" / switch account | MEDIUM | `AuthState` now models it; the action itself is product work (BUG-016) |
| The Google grant is not revoked at Google on account removal | LOW | The token is deleted locally; revoking remotely is a separate API call, planned |
| Fetch itself has no read timeout | LOW | The pinned `imap` crate exposes no read timeout on an established session, so a server that goes silent mid-fetch stalls that worker until the OS TCP timeout. It runs off the UI thread, so the window stays responsive; bounding it needs an upstream API or an idle-based fetch |
| The MIME decoder is hand-rolled and minimal | LOW | Handles the shapes real providers send (plain, base64, quoted-printable, RFC 2047, nested multipart, HTML-to-text) and is fixture-tested, but it is not a full MIME implementation (no charset conversion beyond UTF-8 lossy, no inline images) |
| `test_email_connection` still runs on the IPC thread | LOW | It no longer holds the DB lock (the freeze in BUG-004), but a slow server still blocks the UI for that user-initiated action; the same worker-thread pattern would apply |
| Gmail consumer mailboxes cannot use a plain password at all | BY DESIGN | Google policy; the UI now leads with Google sign-in and still offers an App Password |
| No dependency audit | MEDIUM | `cargo audit` / `npm audit` were not run in this pass |
| Cached mail unencrypted at rest (SEC-002) | MEDIUM | Pre-existing, documented, orthogonal to authentication |
| Live Google/IMAP/SMTP verification | — | Requires a real OAuth client, a real account and a GUI session |

## Summary

**Root cause.** The discovery flow selected a mail server but never selected an
*authentication method*, so every Gmail user was sent down a password IMAP LOGIN
that Google refuses by policy — and that refusal was then classified as an
ordinary bad password, with no route to the flow that could succeed.

**Follow-up (same session, after the report "the mails did not load"):** sign-in
verified access but nothing ever *downloaded* mail, and a signed-in account had no
folder rows, so its inbox was empty by construction. Both are now fixed:
`mailbox.rs` performs the IMAP retrieval and the account owns its folders.

**Files changed.** `discover.rs`, `verify.rs`, `oauth.rs`, `commands.rs`, `lib.rs`
(Rust); `tauri.ts`, `preview.ts`, `LoginView.tsx`, `App.tsx`, `SettingsView.tsx`,
`app.css`, `preview.test.ts` (frontend); `local_store.rs` (tests). Plus the two
new documents.

**Security issues found.** One real vulnerability fixed (SEC-001: the cleartext
`http://autoconfig.<domain>` fallback could have redirected the mailbox password
to an attacker-controlled IMAP host). Two open items documented (SEC-002 data at
rest; no dependency audit). No token-leakage, CSRF, PKCE or capability defect.

**Tests passed.** `cargo test` 36/36 · `cargo check` clean · `tsc -b` clean ·
`vitest` 26/26 · `npm run build` succeeds.

**Not performed.** Live Google sign-in, live IMAP/SMTP, GUI interaction,
`cargo audit` / `npm audit`.

**Remaining risks.** Mail *retrieval* now works but **sending does not**: there is
still no SMTP transport, so `send_smtp` items stay queued. Retrieval covers INBOX
only (newest 50) with no background sync and no attachments. `test_email_connection`
still blocks the UI thread for its own duration (it no longer blocks other
commands). The MIME decoder is hand-rolled and deliberately minimal — it handles
the shapes real providers send and is fixture-tested, but it is not a full MIME
implementation.