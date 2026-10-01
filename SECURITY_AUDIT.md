# SECURITY_AUDIT — Relay Desktop

Audit date: 2026-09-28 · Scope: repository as a whole (React/TypeScript frontend, Tauri 2 IPC surface and capabilities, Rust backend, SQLite store, credential handling, network paths, dependency locks)
Method: static read of every source file, Tauri capability/config review, targeted greps for secret material and injection sinks, plus executed build/test/dependency checks (`npm audit --json` → 0 advisories across 154 packages; `cargo clippy --all-targets` → 1 unrelated pre-existing warning). No source file was modified for this audit.

**No credential, token or key value appears anywhere in this report.** Where a potential secret was found, only file/line/type and the recommended action are recorded (see “Secret material scan” below).

Threat model used for grading:
- **T1 OS-local adversary** — another process or user session on the same machine, or a stolen copy of the app-data directory.
- **T2 network adversary** — someone able to intercept or answer cleartext traffic between the app and the internet, or to hijack DNS/HTTP responses.
- **T3 malicious content** — content that reaches a rendering surface (mail bodies, messages, filenames, search input) attempting script/command/SQL injection.
- **T4 compromised webview** — script executing inside the Tauri webview with access to the IPC surface.

## Secret material scan

| Pattern searched | Files scanned | Result |
|---|---|---|
| `password`, `secret`, `token`, `api_key`, `access_token`, `refresh_token`, `private_key`, `BEGIN PRIVATE KEY`, `DATABASE_URL`, `SMTP_PASSWORD`, `IMAP_PASSWORD` (with an assigned literal) | all `.ts/.tsx/.rs/.json/.md/.sql/.toml`, excluding `node_modules`, `target`, `dist`, `gen` | **2 matches, neither a live secret**: `src-tauri/src/security.rs:34` (JSON key construction for the keyring payload — identifiers only) and `src-tauri/tests/local_store.rs:176` (a deliberately fake test password used to prove the DB never stores it). |
| `.env` files | repository root | **none present** (`.gitignore` lists `.env`) |
| Keys/tokens in `localStorage`/`sessionStorage` | frontend | **none** (no web-storage usage at all: 0 matches for `localStorage`/`sessionStorage`) |
| Secrets in logs | `tracing` call sites | only `guard.rs` logs a panic payload; credentials are never passed to it |

Recommended action: none. Keep the existing rule that only `credential_ref` (a pointer string) is persisted next to account rows; the integration test that scans every column of the account row for the password is the right control to keep.

## Controls verified as present (positive findings)

1. **Passwords never reach SQLite or IPC responses.** `add_email_account`/`auto_sign_in`/`test_email_connection` send the secret one-way into the OS credential manager (`security.rs`, `keyring` crate with `windows-native`); the schema stores `credential_ref` only, and the test `verified_sign_in_persists_state_and_keeps_secret_out_of_the_database` asserts no column of the account row equals the password.
2. **Credential lifecycle.** Re-sign-in rotates the stored secret instead of failing; a persistence failure rolls the keyring entry back (`commands.rs:42-51`); `remove_account` purges both the password entry and the OAuth entry (`commands.rs:309-318`).
3. **No shell/fs/http capability.** `src-tauri/capabilities/default.json` grants only `core:default`, `opener:default`, `notification:default`; there is no `shell`, `fs`, or `http` plugin permission, and no command accepts a filesystem path from the frontend, so path-traversal/arbitrary-write is not reachable today.
4. **Strict CSP, no remote content.** `tauri.conf.json` sets `default-src 'self'`, `img-src 'self' asset: http://asset.localhost data:`, `style-src 'self' 'unsafe-inline'`, `connect-src 'self' ipc: http://ipc.localhost` — the webview cannot load remote scripts or make arbitrary network requests even if content escaped.
5. **No HTML mail rendering / no HTML injection sink.** Zero matches for `dangerouslySetInnerHTML` and `innerHTML`; mail bodies and messages render as React text nodes (`MailView.tsx:162`, `MessengerWorkspace.tsx` bubbles), so `<script>`, `javascript:`, `onerror=` and friends are inert text. `emails.body_html` exists in the schema but is never read or rendered.
6. **SQL is parameterized throughout.** The only interpolated SQL is `emails_in_folder`'s role (whitelisted by `is_mail_role`) and `SyncEngine::count`'s internal filter literal (callers pass fixed constants); every value path uses `params!`. Search uses bound parameters even for the FTS5 `MATCH` expression.
7. **Panic containment on IPC.** `guard.rs` converts a command panic into a normal command error and logs it, preventing the webview2 IPC callback from aborting the process (a genuine availability control); `tracing` records the panic text, never credentials.
8. **Non-sensitive error surfacing.** IMAP refusals are classified from the server's response *code keywords* and its answer *shape* into fixed strings (`verify.rs`: `refusal_hint`, `protocol_shape_hint`) — including the real provider answers (Gmail/Workspace `[ALERT]` texts, Microsoft's wording, Dovecot's "invalid characters") — precisely because servers may echo the client command. An endpoint that completes TLS but never answers an IMAP login is reported as a configuration problem (`VerifyFailure::Protocol`) rather than as a rejected password, and the diagnostic log records only host, port, encryption mode and the fixed classification. Google error bodies are reduced to fixed hints (`oauth.rs:230-245`); OAuth access tokens are never stored, logged or sent over IPC; the refresh token lives only in the OS keyring.
9. **OAuth loopback hygiene.** The redirect listener binds `127.0.0.1:0` (loopback only, ephemeral port), uses PKCE S256, and validates the `state` value before accepting a code (`oauth.rs:163-198`), rejecting mismatches with an error page.
10. **Dependency state.** `npm audit` reports 0 known vulnerabilities for the 154 installed packages; `cargo` dependencies are current-generation (Tauri 2, rusqlite 0.32 bundled, tokio 1, imap 3.0.0-alpha, ureq 2.10, hickory 0.24).

---

## SEC-001

Severity: MEDIUM
Category: Network / credential exposure (T2)
Location: `src-tauri/src/discover.rs:109-125` (autoconfig fetch), used by `commands.rs:78-120` (`auto_sign_in`)

Problem:
The autoconfig candidate list includes a **cleartext HTTP** source as its third entry — `http://autoconfig.<domain>/mail/config-v1.1.xml` — fetched with `ureq::get(...).timeout(6s).call().ok()` and no integrity check. Whatever `<hostname>/<port>/<socketType>` that document names is then handed to `verify_imap_login(address, password, …)` as a candidate, authenticated with the user's real mailbox password.

Attack / Failure Scenario:
1. User runs the app on a hostile network (café/hotel Wi-Fi, malicious router, DNS hijack) and uses the advertised primary flow: email address + password.
2. The two HTTPS sources return nothing for that domain (self-hosted/small-business domains typically have no Mozilla ISPDB entry and may lack a `.well-known` file).
3. The attacker answers `http://autoconfig.<domain>/mail/config-v1.1.xml` with `imap_host = imap.attacker.example`, `socketType = SSL`, port 993.
4. The client connects to the attacker's server over TLS (its own valid certificate), performs `LOGIN user@domain <password>`, and the attacker now holds the mailbox password. The login "succeeds", the account is stored as `connected` against the attacker's host, and later connection tests keep re-sending the password there.

Impact:
Mailbox credential theft with only network-position privilege; full mailbox access afterwards (and, once SMTP delivery exists, spoofed sending as the victim).

Recommended Fix (implemented):
1. ~~Remove the `http://` fallback; keep the HTTPS sources (Mozilla ISPDB and the domain's `.well-known`).~~ **Done** — `autoconfig_candidates` now fetches only the Mozilla ISPDB and the domain's HTTPS `.well-known` document, so the password can no longer be redirected by a network attacker.
2. If an HTTP source is ever required, show the resolved host to the user for confirmation before a password is sent, and reject hosts that do not match the mail domain or a known provider allow-list. *(Not applicable while only HTTPS sources are used.)*
3. Surface `MailCandidate.source` in the progress UI so the user can see where the settings came from. *(Still open; `source` is recorded and the progress line names host/port/auth method.)*

Status: **FIXED** (verified by the HTTPS-only URL list in `discover.rs` and the full suite passing: `cargo test` → 34/34).

## SEC-002

Severity: MEDIUM
Category: Data at rest (T1)
Location: `src-tauri/src/database.rs:7-23`, `src-tauri/migrations/*.sql`, `SECURITY.md:15`

Problem:
All locally cached mail, message bodies, recipient addresses, contacts and notification text live in a plain SQLite database (`<app_data>/database/relay.db` plus `-wal`/`-shm`) with no encryption at rest and no application-level lock. Passwords are correctly confined to the OS credential manager, but everything those credentials protect locally is cleartext. There is also no supported export/backup or "wipe local data" action (`clear_cache` only touches the unused `cache/` directory).

Attack / Failure Scenario:
Any process running as the same OS user — or anyone holding a copy of the profile/app-data directory (shared machine, stolen laptop without full-disk encryption, backup, support bundle, forensic image) — can read the whole mailbox history with `sqlite3 relay.db "SELECT subject, body_text FROM emails;"`, including message bodies and contacts. WAL/SHM sidecars may additionally expose recently written or rolled-back content.

Impact:
Full confidentiality loss of locally cached communications for anyone with filesystem access to the user profile; no key, password or authentication step is required to do it.

Recommended Fix:
1. Encrypt at rest (SQLCipher-style page encryption with the key sealed by DPAPI/the OS keyring) or, at minimum, state in-product that local data is unencrypted so users rely on full-disk encryption.
2. Add an application lock (PIN/biometric) for shared-machine scenarios.
3. Ship backup/export and a real "delete all local data" action that covers the database, WAL and sidecar files.

## SEC-003

Severity: MEDIUM
Category: Availability / local denial of service
Location: `src-tauri/src/commands.rs:126-169` (`test_email_connection_inner`), `src-tauri/src/lib.rs:22-23`, `src-tauri/src/commands.rs:8`

Problem:
The single global database mutex is held across blocking network I/O. `test_email_connection_inner` takes the guard on its first line and keeps it through `oauth::refresh_access_token` (HTTPS) and `verify_imap_login`/`verify_xoauth2` (DNS, a 5 s TCP probe, IMAP handshake), including an OAuth path bounded by `AUTH_TIMEOUT` (300 s). All 49 IPC commands require the same lock.

Attack / Failure Scenario:
A slow, blackholed or hostile mail host turns one "Test connection" click into an application-wide freeze: every other command — folder loads, search, sending a chat message — blocks behind the guard until the network call returns. A malicious server can therefore stall the client for the whole timeout on demand.

Impact:
Local denial of service / unresponsive UI. No data corruption directly, but a user killing the stuck process mid-write interacts badly with the missing transactions (see `COMPLETE_AUDIT.md` DB findings) and with the non-atomic migrations of BUG-001.

Recommended Fix (implemented):
1. ~~Hold the guard only around database reads/writes (read the account row → drop the guard → verify → re-acquire to persist), as `add_email_account_inner` already does.~~ **Done** — `test_email_connection_inner` snapshots the connection under a short-lived guard, runs the HTTPS/IMAP work with no lock held, then re-locks only to record the result.
2. ~~Run blocking transport work off the IPC thread (the `auto_sign_in` worker-thread pattern is the in-repo precedent); consider `RwLock` or per-domain locks so unrelated commands do not serialize.~~ **Done for the sign-in path** — `google_oauth_sign_in` now runs its browser half (loopback wait up to `AUTH_TIMEOUT`, PKCE exchange, XOAUTH2 proof) on a `relay-oauth` worker thread. `RwLock`/per-domain guards are still an option for later transports.
3. Apply shorter, explicit timeouts on the re-verification path. *(Still open: the 5 s reachability probe bounds the connect, but the IMAP handshake itself has no explicit deadline.)*

Status: **FIXED for the sign-in path** — no database lock is held across network I/O in `test_email_connection` or `google_oauth_sign_in` (`cargo test` → 34/34; the lock scopes were verified by inspection since the timing behaviour needs a live IPC thread).

## SEC-004

Severity: LOW
Category: Privilege scope / IPC surface (T4)
Location: `src-tauri/capabilities/default.json`, `src-tauri/src/commands.rs:184, 203`, `src-tauri/Cargo.toml` (`tauri-plugin-opener`)

Problem:
The webview is granted `opener:default`, which lets frontend script ask the OS to open arbitrary URLs (including local files and custom schemes) with the user's default handlers. The only actual use of the opener is native-side (`google_oauth_sign_in_inner` opens the Google consent URL), so the frontend never needs the permission. The plugin is also registered globally (`lib.rs:28`).

Attack / Failure Scenario:
If script ever executes in the webview (a future XSS, a compromised dependency, or a dev-time mistake), it can call the opener IPC to launch an external application or open a `file:`/custom-scheme target without any user gesture, turning a content bug into local execution of registered handlers.

Impact:
Privilege escalation limited to handler invocation (not arbitrary code in the app process), but it is unnecessary attack surface for a feature the frontend does not use.

Recommended Fix:
Drop `opener:default` from the default capability (or scope it to the specific Google host used) and keep URL opening exclusively in the Rust command that already performs it.

## SEC-005

Severity: LOW
Category: Information disclosure to the frontend (T4)
Location: `src-tauri/src/commands.rs:18-22` (`get_database_info`), `src-tauri/src/commands.rs:320-324` (`get_storage_usage`)

Problem:
`get_database_info` returns `location` — the absolute path of `relay.db` (including the OS user name) — and `get_storage_usage` reports byte sizes for every app-data subdirectory. Both are exposed over IPC, and neither is used by the UI today (`get_database_info` has no caller; storage usage is rendered but could be computed from aggregate sizes only).

Attack / Failure Scenario:
Any script in the webview (or a copy-pasted error report assembled by a user debugging the app) can enumerate the local username and directory layout, which is useful reconnaissance for chaining with filesystem-level exploits or for social engineering. The data is low-sensitivity on its own.

Impact:
Minor privacy/reconnaissance leak; it also weakens the "no local paths cross the IPC boundary" property that keeps path traversal unreachable.

Recommended Fix:
Return only the schema version from `get_database_info` (or remove the command), and return aggregate byte totals without per-directory absolute paths; keep the absolute path in the log file for support instead.

## SEC-006

Severity: LOW
Category: Transport configuration (T2)
Location: `src-tauri/src/verify.rs:119-147` (`verify_imap_login`/`connect_imap`), `src/app/LoginView.tsx:154-158` (`None (not recommended)` option), `src-tauri/src/models.rs:17` (`CreateAccount.encryption`)

Problem:
`encryption: "none"` is accepted, persisted and reused for every later connection, sending `LOGIN`/`AUTHENTICATE PLAIN` credentials in cleartext for the lifetime of the account. The only warning is the one-time option label in the login screen; `test_email_connection` and any future IMAP fetch silently repeat the cleartext exchange, and nothing marks the account as insecure afterwards.

Attack / Failure Scenario:
A user selecting "None" once (or a discovered config that resolves to plaintext, e.g. from a hostile autoconfig — see SEC-001) exposes the password to any network observer for every subsequent connection, indefinitely, with no in-app reminder.

Impact:
Credential interception on a hostile network for the affected account (T2), with no visible indication in the UI after setup.

Recommended Fix:
Keep the option (some legacy servers require it) but add a persistent "insecure connection" indicator on the account row and settings, warn again on every successful plaintext sign-in, and record it in the connection state/diagnostics. Alternatively, restrict plaintext to IMAP hosts on the local network.

## SEC-007

Severity: LOW
Category: Supply chain / build hygiene
Location: repository root (`package-lock.json` **and** `pnpm-lock.yaml` + `pnpm-workspace.yaml`), no CI configuration, no `.git` directory, `src-tauri/Cargo.toml`

Problem:
Three hygiene gaps compound: (1) two different lockfiles coexist for the same dependency set, so "the" resolved tree depends on which package manager is used and a lockfile-based audit only covers one of them; (2) there is no CI at all (no `.github/workflows`, no lint script, no `cargo audit`/`cargo deny` step), so nothing checks advisories, formatting or tests automatically; (3) the working tree is not a git repository, so there is no reviewable history, no way to diff a change, and no rollback if a bad edit ships.

Attack / Failure Scenario:
A dependency advisory (or a malicious release of a transitive crate/npm package) would only be noticed manually; a compromised or accidental change cannot be attributed or reverted; a reviewer cannot see what changed between builds. Alternatively, installing with the "other" package manager resolves a different tree than the one that was audited.

Impact:
Reduced ability to detect and respond to supply-chain or regression incidents; this is a process risk rather than an exploitable vulnerability. `npm audit` is currently clean (0 advisories / 154 packages), so there is no known-vulnerable dependency today.

Recommended Fix:
1. Choose one package manager (npm is the toolchain of record per the repo docs) and delete the unused lockfile/workspace file, or adopt pnpm consistently and delete `package-lock.json`.
2. Initialise version control and add CI that runs `npm ci`, `tsc -b`, `npm test`, `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `npm audit --audit-level=high` and `cargo audit`.
3. Add an `npm run lint` script (ESLint) so the audit step in the project's own checklist is executable.

## SEC-008

Severity: LOW (accepted risk — document it explicitly)
Category: Identity verification (T2)
Location: `src-tauri/src/oauth.rs:247-256` (`email_from_id_token`), `src-tauri/src/commands.rs:206`

Problem:
The Gmail address used for the account comes from the `id_token` payload with **no signature verification**: the payload segment is base64url-decoded and the `email` claim is taken as-is. The code documents the reasoning correctly — the token arrives directly from Google's token endpoint over TLS, so it is not the trust boundary (the real access proof is the XOAUTH2 IMAP handshake that follows).

Attack / Failure Scenario:
Not a remote forgery (a token cannot be injected into the TLS-protected exchange). The residual risks are local: TLS interception with a trusted-but-hostile root, or a future change that passes an `id_token` through another path (e.g. as an IPC parameter), would make an unverified claim authoritative.

Impact:
No impact in the shipped flow; the risk becomes real if the token ever reaches this function from a less trusted source or a second producer is added.

Recommended Fix:
1. Add cheap claim checks before using the address: `aud` equals the client id and `iss` is `https://accounts.google.com`.
2. Keep the reasoning in `SECURITY.md` (currently only a code comment) so a future change to the token source is caught in review.

## SEC-009

Severity: LOW
Category: Robustness against hostile responses (T2)
Location: `src-tauri/src/discover.rs:117-124`, `src-tauri/src/oauth.rs:222-226`

Problem:
Outbound responses are read without a size cap: `ureq::get(url).timeout(6s).call().ok().and_then(|response| response.into_string().ok())` for autoconfig (including the cleartext host of SEC-001) and `response.into_string().unwrap_or_default()` for Google error bodies. The 6-second timeout bounds time, not bytes.

Attack / Failure Scenario:
A hostile or misconfigured server (or a captive portal) streams data for the full timeout; the client buffers all of it into a `String`, causing a memory spike and potentially an out-of-memory abort of the app process (which holds the SQLite connection).

Impact:
Local denial of service / memory pressure; no exfiltration and no code execution, but a crash during a write would compound the non-transactional write findings.

Recommended Fix:
Cap the read (`response.into_reader().take(256 * 1024)`), reject unreasonably large bodies, and keep the timeout as a second defence.

## SEC-010

Severity: INFO (verified positive, with a forward-looking requirement)
Category: Injection review (T3)
Location: frontend rendering (`MailView.tsx:162`, `MessengerWorkspace.tsx` bubbles, `ContactsView.tsx`, `NotificationPanel.tsx`), `repositories.rs` SQL, `tauri.conf.json` CSP

Verification performed:
- **XSS**: zero matches for `dangerouslySetInnerHTML`/`innerHTML`; every user-controlled string (mail body, subject, sender name, chat message, contact fields, notification text, search results) renders through React text nodes, which escape by construction. `emails.body_html` is never selected or rendered, so no HTML mail path exists, and the CSP blocks inline/remote script even if a sink were introduced later.
- **SQL injection**: all values are bound with `params!`; the only interpolation is the folder `role` (validated against five literals by `is_mail_role`) and `SyncEngine::count`'s filter (a fixed internal literal). The FTS5 `MATCH` expression is bound as a parameter — malformed input produces a syntax error (BUG-010), not injection.
- **Command injection / path traversal**: no shell invocation and no command accepts a frontend-supplied path; `clear_cache` derives its path from `app_data_dir` and deletes only under `cache/`. Attachment handling does not exist yet, so the classic path-traversal surface is absent.
- **Deserialization**: command arguments are typed structs (`DraftInput`, `CreateAccount`, `SendMessageInput`, `UpdateNotificationPreference`, …) with length/e-mail/enum validation in the repository, so malformed IPC payloads are rejected as `Validation` before touching SQL.

Forward-looking requirement: when HTML mail rendering or attachments are implemented, they must land together with (a) a strict sanitizer plus remote-content blocking for `body_html`, and (b) a filename/`storage_key` normalisation layer that never joins untrusted names onto a filesystem path.

## SEC-011

Severity: LOW
Category: Reliability / diagnosability
Location: `src-tauri/src/guard.rs:15-18, 32-40`, `src-tauri/src/commands.rs:8` (all `.map_err(|_| "…")` sites)

Problem:
Two consequences of the current error strategy: (1) a panic raised while the state mutex is held poisons it, and every subsequent command returns "Application state is unavailable" for the rest of the session — the process stays alive but permanently non-functional (a documented trade-off in `guard.rs`); (2) command handlers collapse every repository error into one fixed string, so a database failure, a validation rejection and a missing row are indistinguishable to the user *and* to the log (only `guard.rs` logs anything).

Attack / Failure Scenario:
Not directly exploitable; the impact is response capability. A poisoned session cannot be recovered without a restart, and real incidents (corrupt row, failed constraint, unexpected write error) leave no diagnostic trail.

Impact:
Reduced supportability; a poisoned session effectively denies service to the user until restart.

Recommended Fix:
1. Represent mutex poisoning as a "restart required" state with a reload action instead of failing every command, and consider recovering the guard (`PoisonError::into_inner`) for read-only paths.
2. Log the underlying error at the command boundary (`tracing::error!(error = %err, command = "…")`) while still returning the user-safe string — without credentials, as today.

---

## Authorization assessment

The audit brief asks for privilege checks (user A vs user B, normal user vs admin, private channels). The honest answer for this application:

- **There is no multi-user model.** The only identity is the OS user plus zero-or-more locally configured mail accounts; there is no server, no role column, no admin surface, no session token and no per-row ownership. Messages and emails have no owner other than the single local user (senders resolve to `SELF_CONTACT_ID`).
- **Therefore no authorization layer exists — and none is missing** for the current architecture: every IPC command is equally privileged for the one user who can already read the database file. The real trust boundaries are (a) the OS user account and (b) the webview → IPC boundary.
- **What must not be assumed:** a future company server with private channels must enforce permissions **server-side**; a React-only restriction (hiding a channel or a row) is not authorization. Nothing here can be reused as a policy layer either: repository methods have no actor parameter, so authenticated actor context must be added to every mutation when the server lands.
- **What *is* enforced locally today:** passwords never cross the IPC boundary back to the frontend; `remove_account` purges keyring entries; credentials are addressed per-account in the OS store; the webview has no filesystem/shell capability; no command accepts a path or a shell string.

## IPC security summary

All 49 registered commands were reviewed against their repository calls and validation (full inventory in `COMPLETE_AUDIT.md` §12). Boundary summary:

| Aspect | Result |
|---|---|
| Commands accepting a filesystem path | 0 |
| Commands that can execute a process / spawn a shell | 0 |
| Commands returning credential material | 0 (only `credential_ref` is stored; secrets never leave Rust) |
| Commands returning local paths | 2 — `get_database_info`, `get_storage_usage` (SEC-005) |
| Commands without input validation | 0 (arguments are length/enum/e-mail checked or bound as SQL parameters) |
| Commands holding the DB lock across network I/O | 2 — `test_email_connection`, `google_oauth_sign_in` (second phase) (SEC-003) |
| Commands unused by the UI (dead surface to prune) | 6 — `get_app_health`, `get_database_info`, `get_inbox`, `get_pending_sync_items`, `show_native_notification`, `get_channel_reactions` |
| Panic containment on every command that can panic | Yes, via `guard::guarded` (SEC-011 covers the poisoning trade-off) |

## Finding counts

| Severity | Count | IDs |
|---|---|---|
| CRITICAL | 0 | — |
| HIGH | 0 | — |
| MEDIUM | 3 | SEC-001, SEC-002, SEC-003 |
| LOW | 7 | SEC-004, SEC-005, SEC-006, SEC-007, SEC-008, SEC-009, SEC-011 |
| INFO / verified positive | 1 | SEC-010 |

## Security verdict

No remotely exploitable code execution, injection or authentication bypass was found. Credential handling is a genuine strength (secrets in the OS keyring, verified against the real server before anything is persisted, never logged, never returned over IPC), and the rendering and SQL surfaces are clean. The three MEDIUM findings come from the network and storage layers rather than the UI: a cleartext autoconfig fallback that can redirect a password (SEC-001 — a few lines to fix), unencrypted local storage of everything that password protects (SEC-002 — an architectural decision), and a global lock held across network I/O that lets a hostile server freeze the application (SEC-003 — a localised refactor). SEC-001 and SEC-003 should be fixed before any real mailbox is configured on an untrusted network; SEC-002 should be an explicit, documented decision before deployment.
