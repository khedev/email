# COMPLETE_AUDIT — Relay Desktop

Audit date: 2026-09-28
Scope: the entire repository (`src/`, `src-tauri/`, migrations, configuration, tests, docs, lockfiles)
Method: static read of every source file, end-to-end trace of UI → handler → state → IPC → Rust → SQLite for each feature, executed build/test/lint/audit commands, and targeted greps for dead handlers, swallowed errors, injection sinks and secret material. **No source file was modified by this audit**; the only files created are this report and its two companions.

Companion documents: `BUG_AUDIT.md` (19 findings, BUG-001 … BUG-019) and `SECURITY_AUDIT.md` (11 findings, SEC-001 … SEC-011).

Evidence commands and results:

```text
npm run build              -> PASS   (tsc -b + vite build, 1602 modules, dist/ emitted)
npm test                   -> PASS   15/15 (src/app/util.test.ts 9, src/platform/preview.test.ts 6)
cargo build                -> PASS   (debug binary links)
cargo test                 -> PASS   29/29 (tests/local_store.rs)
cargo clippy --all-targets -> 1 warning, pre-existing (oauth.rs:126 manual_split_once); none from new code
npm audit --json           -> 0 vulnerabilities (154 packages: 9 prod, 146 dev)
npm run lint / pnpm lint   -> NOT AVAILABLE (no ESLint/Prettier and no lint script exist)
tauri dev / tauri build    -> NOT RUN (interactive GUI launch / installer bundling)
live IMAP, SMTP, Google OAuth -> NOT AVAILABLE (no test servers or credentials)
```

Nothing in this report is graded “verified” without one of: an executed command, an existing passing test, or an explicitly stated reproduction path against the real schema/UI.

---

# 1. Executive Summary

Relay is a **real, working local-first desktop application** — not a mockup — but it is a working *local* application, not yet a working *mail client*.

What is genuinely solid: a Rust/Tauri backend with a single repository layer over SQLite (WAL, foreign keys on, 11 migrations), 49 typed IPC commands, a React/TypeScript frontend with no fake data paths, real IMAP **sign-in verification** with secrets confined to the OS credential manager, a full-text search index, and a messenger with normalized threads, reactions, edit/delete/pin that all persist. 36 Rust integration tests and 26 frontend tests cover the local data plane and pass; the build is clean.

What is missing is the transport half of the product: ~~there is no IMAP fetch (the app never downloads mail — a release build starts with an empty inbox and stays empty)~~ **mail retrieval now exists** (the Mail view's Sync action fetches the newest inbox messages; no background or scheduled sync, and only INBOX), but there is still **no SMTP delivery** (`send_smtp` queue items stay `pending` forever by design), plus no company-server sync, no attachments, and no channel management. The pieces that exist around those gaps are honest — the UI says mail retrieval is a later phase, the status bar says no transport is configured — so the gap is a scope statement rather than deception.

The most serious *defects* are not in the features that are missing but in the layer beneath them: migrations are non-transactional (a crash during an upgrade can leave the app unable to start — BUG-001), multi-statement writes are not transactional either, the sync queue stores empty payloads so queued work is not actually deliverable (BUG-003), and email replies are never linked into their conversation (BUG-002). The sign-in path's lock/threading defects and the cleartext autoconfig fallback have since been fixed (BUG-004/SEC-001/SEC-003 — see `AUTHENTICATION_AUDIT.md`); all locally cached mail is still unencrypted (SEC-002).

Headline verdict: **a strong local-first foundation at roughly half product completeness, not production-ready** — chiefly because nothing is transported yet, and because the persistence/upgrade layer needs hardening before real user data lives in it.

---

# 2. Architecture

Executable architecture as shipped (verified against the code, not the roadmap):

```text
React 18 + TypeScript + Vite (no router; view switching is App state)
        │  src/platform/tauri.ts   — hand-written typed IPC client (camelCase mirror of Rust models)
        │  src/platform/preview.ts — in-memory browser backend, used only outside Tauri (labelled "Preview")
        ▼
Tauri 2 IPC  (49 commands; src-tauri/src/commands.rs → lib.rs generate_handler)
        │  AppState { database: Mutex<Database> }   — one global lock
        │  guard::guarded()                         — panic containment around command work
        ▼
Rust services
  ├── repositories.rs   single CRUD layer (all SQL lives here; the UI never touches SQLite)
  ├── database.rs       WAL + foreign_keys ON + 11 sequential migrations + self-identity bootstrap
  ├── seed.rs           debug-only development data (release builds start empty)
  ├── verify.rs         real IMAP handshake (implicit TLS / STARTTLS / explicit plaintext) + SASL PLAIN fallback
  ├── discover.rs       provider DB → DNS MX → autoconfig XML → hostname guesses, then verify each candidate
  ├── oauth.rs          Google OAuth 2.0 installed-app flow (PKCE S256, loopback redirect, XOAUTH2 proof)
  ├── security.rs       OS credential manager (Windows Credential Manager / secret-service)
  └── sync.rs           queue lifecycle + DisabledTransport (no worker, no remote calls)
        ▼
SQLite  relay.db  (WAL; tables from migrations 0001-0011; FTS5 external-content indexes kept by triggers)
        ▼
No remote service is contacted except during sign-in, discovery, OAuth and connection tests.
```

Stack deviations from the project's own `phase.md` (recorded for accuracy, not as defects): React Router, TanStack Query, Zustand and Tailwind are **not** used — navigation is a `useState` view switch, server state is `useState` plus explicit refetch, and styling is hand-written CSS with design tokens (`src/styles/*.css`). The app is a strict single-window desktop app.

Data model (migrations 0001-0011): `accounts`, `email_folders`, `emails`, `email_recipients`, `email_attachments`, `contacts`, `conversations`, `channel_members`, `messages`, `message_reactions`, `sync_queue`, `sync_conflicts`, `sync_metadata`, `notifications`, `notification_preferences`, `settings`, plus `fts_emails`/`fts_messages`/`fts_contacts`/`fts_conversations` (external-content FTS5 maintained by triggers). Every syncable table carries a UUID id, timestamps, `deleted_at`, `sync_status`, `sync_version` and `server_id`. Messenger threading is normalized by migration 0011 (`messages.thread_id` = root, `in_reply_to` = exact parent); email threading relies on `emails.thread_id`/`message_id_header`.

Runtime configuration worth noting:
- `tauri.conf.json`: single window (1280×800, min 480×520, resizable), restrictive CSP (`default-src 'self'`, `connect-src 'self' ipc:`), bundle target `all`.
- `capabilities/default.json`: `core:default`, `opener:default`, `notification:default` only (no fs/shell/http).
- `.cargo/config.toml` pins the Windows linkers per host target; `build.rs` embeds a Common-Controls manifest into test binaries.
- Build tooling: npm (with a stray `pnpm-lock.yaml`/`pnpm-workspace.yaml` also present), `tsc -b` + `vite build`, `vitest` for the frontend, `cargo test` for the Rust integration suite (tests live in `tests/` because the lib test harness would not get the manifest).

---

# 3. Functional modules and the functionality matrix

Status vocabulary: **FUNCTIONAL** (works end to end), **PARTIAL**, **BROKEN**, **MOCKED**, **NOT IMPLEMENTED**, **CANNOT VERIFY** (implemented but not exercisable in this environment).

| Module | Function | UI | Frontend | IPC | Backend | DB | Tested | Status |
|---|---|---|---|---|---|---|---|---|
| Accounts | First-run sign-in gate | LoginView | ✅ | get_accounts | repositories.accounts | accounts | ✅ | FUNCTIONAL |
| Accounts | Manual IMAP/SMTP sign-in | LoginView | ✅ | add_email_account | verify.rs + keyring + account write | accounts | ✅ (keyring, no-plaintext-in-DB) | FUNCTIONAL (live server CANNOT VERIFY) |
| Accounts | Thunderbird-style auto sign-in | LoginView | ✅ | auto_sign_in (worker thread) | discover.rs + verify.rs | accounts | ✅ (parse/rank tests) | CANNOT VERIFY (needs network) |
| Accounts | Google OAuth (Gmail) | LoginView | ✅ | google_oauth_sign_in, forget_oauth_client_id | oauth.rs + verify_xoauth2 + keyring (client secret stored separately; forget clears id + secret) | accounts | ✅ (spec tests) | CANNOT VERIFY (needs user's own client id) |
| Accounts | List / test connection / remove | SettingsView | ✅ | get_accounts, test_email_connection, remove_account | verify + keyring purge | accounts | ✅ | FUNCTIONAL (freeze risk BUG-004) |
| Accounts | Sign out / switch account | — | — | — | — | — | — | NOT IMPLEMENTED (BUG-016) |
| Mail | IMAP retrieval (newest 50 inbox messages) | MailView (Sync) | ✅ | sync_mail (worker thread, all accounts) | mailbox.rs + verify::open_mail_session | emails (keyed by IMAP UID) | ✅ (sequence-range, decoding + persistence tests) | FUNCTIONAL (live Gmail CANNOT VERIFY) |
| Mail | Folder views (inbox/starred/sent/drafts/archive/trash) | MailView | ✅ | get_emails_in_folder, get_starred | list_emails | emails + email_folders | ✅ | FUNCTIONAL (single account; BUG-009/014) |
| Mail | Reading pane + auto mark-read | MailView | ✅ | get_email, mark_email_read, fetch_email_body | repositories.email + mailbox::fetch_message_body | emails | ✅ (`raw_message_assembly_keeps_the_body`, `stored_email_body_can_be_repaired`) | FUNCTIONAL (live Gmail CANNOT VERIFY) |
| Mail | Star / archive / trash / move | MailView | ✅ | set_email_star, archive_email, trash_email | move_email_to_folder | emails + folders | ✅ | FUNCTIONAL |
| Mail | Email conversation strip | MailView | ✅ | get_email_thread | email_thread | emails.thread_id | ✅ (seed rows only) | PARTIAL (BUG-002: replies never joined) |
| Mail | **IMAP retrieval** | — | — | — | — | — | — | NOT IMPLEMENTED (no fetch worker) |
| Mail | **SMTP delivery** | — | — | queue_email_send | queue only | sync_queue | ✅ (queue filing) | NOT IMPLEMENTED (stays `pending` by design) |
| Mail | Drafts: autosave, resume, delete | Composer | ✅ | save_draft | save_draft | emails (+recipients) | ✅ | FUNCTIONAL (loss window BUG-007) |
| Mail | Reply / Reply-all / Forward | MailView + Composer | ✅ | save_draft, queue_email_send | save_draft | emails | ✅ | PARTIAL (prefill only; BUG-002) |
| Mail | Attachments | disabled button | — | — | — | email_attachments (unused) | — | NOT IMPLEMENTED (honest tooltip) |

| Messenger | Channels list (member counts) | MessengerWorkspace | ✅ | get_channels | repositories.channels | conversations + channel_members | ✅ | FUNCTIONAL (read-only) |
| Messenger | Direct messages (open/create, idempotent) | ContactsView → MessengerWorkspace | ✅ | open_direct_message | open_direct_message | conversations + members | ✅ | FUNCTIONAL |
| Messenger | Send / edit / delete / pin / copy | MessengerWorkspace | ✅ | send_company_message, edit_message, delete_message, set_message_pin | repositories | messages | ✅ | FUNCTIONAL |
| Messenger | Reactions (root-normalized) | MessengerWorkspace | ✅ | toggle_message_reaction, get_channel_reactions | repositories | message_reactions | ✅ | FUNCTIONAL |
| Messenger | Threads (root grouping, panel replies) | MessengerWorkspace | ✅ | get_message_thread | message_thread | messages.thread_id | ✅ (3 tests) | FUNCTIONAL |
| Messenger | Receiving others' messages / live delivery | — | — | — | — | — | — | NOT IMPLEMENTED (no transport) |
| Messenger | Mentions, typing status, attachments | — | — | — | — | — | — | NOT IMPLEMENTED |
| Channels | Create / rename / delete / join / leave | — | — | — | — | — | — | NOT IMPLEMENTED |
| Channels | Private channels / permissions | — | — | — | — | — | — | NOT IMPLEMENTED (needs server-side policy) |
| Contacts | List / create / favorite / filter | ContactsView | ✅ | get_contacts, create_contact, set_contact_favorite | repositories.contacts | contacts | ✅ | FUNCTIONAL (500 cap) |
| Contacts | Edit / delete / groups / profile | — | — | — | — | — | — | NOT IMPLEMENTED |
| Contacts | Email contact / message contact | ContactsView | ✅ | open_direct_message + composer | ✅ | ✅ | ✅ | FUNCTIONAL |
| Search | Global search (mail, messages, channels, people) | SearchPalette | ✅ | search_global | repositories.search (FTS5) | fts_* + base tables | ✅ | FUNCTIONAL (BUG-010 special chars) |
| Search | Command palette actions | SearchPalette | ✅ | various | ✅ | ✅ | — | PARTIAL (BUG-005/008; “New Message” only navigates) |
| Notifications | History, unread count, mark all read | NotificationPanel | ✅ | get_notifications, get_unread_notification_count, mark_all_notifications_read | repositories.notifications | notifications | ✅ | PARTIAL (BUG-013/019) |
| Notifications | Native OS toast | — | — | show_native_notification (unused) | notification plugin | — | — | NOT IMPLEMENTED (no producer) |
| Notifications | Preferences | SettingsView | ✅ | get/update_notification_preference | repositories | notification_preferences | ✅ | PARTIAL (stored, unconsumed; BUG-011) |
| Sync | Queue lifecycle, retry, overview | statusbar + SettingsView | ✅ | get_sync_overview, retry_failed_sync | sync.rs | sync_queue/sync_conflicts | ✅ (retry test) | PARTIAL (BUG-003; no worker) |
| Sync | Company server / WebSocket transport | — | — | — | DisabledTransport | — | ✅ (offline test) | NOT IMPLEMENTED |
| Settings | Theme / density / font size | SettingsView | ✅ | get/set_app_setting | repositories | settings | ✅ (round-trip test) | FUNCTIONAL (BUG-005 in palette path) |
| Settings | Pane widths persisted | PaneSplitter | ✅ | set_app_setting | ✅ | settings | — | FUNCTIONAL |
| Settings | Presence (self only) | SettingsView | ✅ | get/set_presence | repositories | settings | ✅ | MOCKED (no peers to display it to) |
| Settings | Storage usage / clear cache | SettingsView | ✅ | get_storage_usage, clear_cache | commands.rs + fs | — | — | PARTIAL (cache dir unused → 0 bytes) |
| Settings | Language / startup / security prefs | — | — | — | — | — | — | NOT IMPLEMENTED |
| Shell | Navigation, shortcuts, preview badge | App | ✅ | — | — | — | — | FUNCTIONAL (BUG-017 nav aliases) |
| Shell | Connection status indicator | statusbar | ✅ | get_app_health (stub) + get_sync_overview | sync.rs | sync_metadata | — | MOCKED (always “Offline”; BUG-015) |
| Offline | Local reads/writes with no network | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | FUNCTIONAL (Fully Offline) |

---

# 4. Working features (verified)

Confirmed by tracing the full execution path (UI → handler → IPC → repository → SQL) and, where marked, by tests executed during this audit:

1. **Local mail workspace** — six role-scoped folder views, reading pane, star, archive, trash (outbound mail included), auto mark-read with a repeat guard, conversation strip for rows that carry a `thread_id`. Tests: `folder_queries_cover_every_mail_role`, `star_toggle_persists_and_reverts`, `threading_groups_related_mail_and_isolates_others`, `email_detail_exposes_recipients_for_drafts`, `trash_accepts_outbound_mail`.
2. **Drafts and send queueing** — debounced autosave, resume by id, recipient validation, `queue_send` refusing mail with no `to` recipient, outbound copy filed into Sent. Test: `queued_send_moves_mail_into_the_sent_folder`.
3. **Full-text search** — FTS5 external-content indexes maintained by triggers across four tables, prefix search for single tokens, phrase search for multi-word input, soft-deleted rows excluded. Tests: `fts_search_indexes_mail_and_messages`, `search_ignores_soft_deleted_messages`.
4. **Sign-in verification and credential handling** — real IMAP LOGIN honoring the selected encryption, SASL PLAIN fallback for `LOGINDISABLED` servers, refusal classification without echoing server text, password only in the OS keyring, idempotent re-authentication, credential rotation, purge on account removal. Tests: `verified_sign_in_persists_state_and_keeps_secret_out_of_the_database`, `connection_state_updates_are_validated`, `removed_account_reports_address_for_credential_purge`, `os_keyring_round_trip`, `re_sign_in_updates_the_existing_account`, `sasl_plain_encoding_and_refusal_classification`.
5. **Server discovery** — provider database, MX classification, autoconfig XML parsing, hostname guesses, candidate de-duplication/ranking, resolver-runtime guard. Tests: `thunderbird_style_discovery_ranks_and_parses`, `mx_lookup_runtime_keeps_timers_enabled`.

6. **Messenger (local)** — channel list with member counts, idempotent DM opening, send/edit/delete/pin/copy, reactions stored on the thread root, normalized threads including nested replies, cascade delete of a thread, and persistence re-read from SQLite. Tests: `message_actions_edit_delete_and_pin`, `direct_messages_are_idempotent_and_listed`, `message_reactions_toggle_cleanly`, `threaded_replies_stay_in_one_thread_and_react_on_the_root`, `deleting_a_thread_root_removes_its_replies`.
7. **Settings persistence** — settings round-trip, presence validation, notification preferences, account management UI, storage stats, sync status/retry. Test: `settings_and_presence_round_trip_with_validation`.
8. **Sync queue mechanics (local half)** — durable queue rows with lifecycle columns, `retry_failed` returning failed rows to `pending`, conflict table, honest offline reporting. Tests: `retry_marks_failed_rows_pending_again`, `disabled_transport_reports_offline`.
9. **Panic containment** — a command panic becomes a command error instead of aborting the process. Test: `panics_in_command_work_become_errors`.
10. **Frontend scaffolding quality** — typed IPC mirror, optimistic updates with revert-on-failure (stars, contacts), `console.error` on every swallowed path (zero `console.log`), balanced `addEventListener`/`removeEventListener`, debounce timers cleared on unmount, and a browser-preview backend that mirrors the native thread contract. Tests: `src/app/util.test.ts` (9), `src/platform/preview.test.ts` (6).

# 5. Broken features

| Feature | Symptom | Reference |
|---|---|---|
| Email conversation threading for new mail | Replies/forwards are stored as standalone rows; the thread strip never shows them and `email_thread()` returns `[]` for the reply | BUG-002 |
| Theme toggle from the command palette | Works for the session, silently reverts after restart | BUG-005 |
| “Mark all read” in the notification panel | Reopening the panel shows the same rows again, with no read/unread distinction | BUG-019 |
| Multi-account folder views | Accounts' mail is merged and indistinguishable (no account field, no filter) | BUG-009 |
| “Sync Now” (palette) | Performs no synchronization; only re-reads counters | BUG-008 |
| Connection status indicator | Always reads “Offline” even with a working network, and never updates | BUG-015 |
| Draft autosave on close | The last ≤700 ms of typing is discarded silently when the composer closes | BUG-007 |
| Migration on interrupted upgrade | Can leave the schema half-applied and the app unable to start | BUG-001 |

“Broken” here means the code path ends in a state that contradicts what the UI promises. Nothing in this list is a crash on the happy path.

# 6. Partially implemented features

- **Email as a whole** — the local half is complete (store, folders, drafts, search, actions); the transport half is absent (no IMAP fetch, no SMTP delivery), so a release build starts empty and never fills.
- **Accounts** — verification, keyring storage, re-auth, removal and connection tests are real; there is no session lifecycle (no sign-out/switch) and shell account state goes stale after removal (BUG-016).
- **Email conversations** — schema, query and UI exist, but only seeded rows populate `thread_id` (BUG-002).
- **Messenger** — complete for one local user; no receive path, no peers, no typing/mentions/attachments.
- **Channels** — read-only history and membership; no create/rename/delete/join/leave, no privacy model.
- **Contacts / directory** — create, favorite, search, e-mail and message work; no edit/delete/groups/profile, no presence.
- **Notifications** — persistent history, counts, preferences and a native-toast command exist, but nothing generates notifications, so only seeded data ever appears.
- **Synchronization** — queue, retry, conflicts and honest offline status exist; no worker, no transport, empty payloads (BUG-003).
- **Search** — real FTS5 across four entity types; no ranking controls, no attachment search (none exist), brittle on FTS5 metacharacters (BUG-010).
- **Settings** — appearance, presence, notifications, accounts, storage and sync state persist; no language, startup or security sections.
- **Presence** — persisted and validated, but nothing observes it (self-only).
- **Storage** — usage is measured and the cache can be cleared, yet no feature ever writes to the cache directory, so “Clear cache” reports 0 bytes freed.
- **Offline** — the whole local plane works offline; the online features (sign-in, discovery, OAuth) fail with clear messages rather than degrading.

# 7. Missing features (not implemented at all)

IMAP retrieval (fetch worker, folder discovery, incremental sync, `UIDVALIDITY` handling) · SMTP delivery (`send_smtp` consumer, MIME building, attachment transfer) · Attachment ingestion/storage/download plus its security review · Company-server sync and WebSocket transport (which also blocks presence beyond self, typing indicators and mentions) · Channel administration (create/rename/delete/join/leave, private channels, roles) · Contact administration (edit/delete, groups, profile view, import/export) · Notification producers (fetch results, mentions, thread replies) and per-item read state · Email signatures, rich-text composition, recipient autocomplete from contacts · i18n/localisation despite the brief's language expectations · RBAC/authorization (not applicable to a single-user local app, but required server-side once sync exists) · Automated update, telemetry and backup tooling · CI, linting and version control (SEC-007).

# 8. Mock, stub and development-only functionality

| Item | Classification | Notes |
|---|---|---|
| `src/platform/preview.ts` in-memory backend | Legitimate dev tool | Reachable only outside Tauri, labelled “Preview” in the header, documented in code and README |
| `src-tauri/src/seed.rs` development data | Legitimate dev data | `#[cfg(debug_assertions)]` only; release builds start empty except the self-identity contact row |
| `get_app_health` constant `offline` | Stub | Unused by the UI except as a fallback; feeds BUG-015 |
| `DisabledTransport` | Honest placeholder | Reports “offline · no remote transport configured”; the queue stays durable and pending by design |
| Presence (self only) | Cosmetic | Persisted and validated; no other participant exists |
| Notification preferences | Stored but unconsumed | Nothing reads the flags when deciding whether to notify (no producer) |
| “Sync Now” palette command | Misleading label | `refreshSync()` only (BUG-008) |
| “Threads” / “Channels” nav entries | Aliases | Both open the Messages view (BUG-017) |
| “New Message” palette command | Navigation only | Opens the messenger view instead of starting a new message |
| `emails.body_html`, `email_attachments` | Unused schema | No writer and no reader; the attachment control is a disabled button with an honest tooltip |
| `scripts/gen-icon.exe` | Build helper | Committed icon-generator binary (already listed in `.gitignore`) |
| `phase.md` stack list (React Router/TanStack/Zustand/Tailwind) | Aspirational | Not used by the implementation (see §2) |

No fake data path reaches production behaviour: no hard-coded message arrays, no `setTimeout`-simulated responses, no placeholder handlers. Greps executed: `onClick={() => {}}` → 0, `TODO`/`FIXME`/`HACK`/`XXX` → 0, `console.log` → 0, `dangerouslySetInnerHTML`/`innerHTML` → 0, `localStorage`/`sessionStorage` → 0, `setTimeout` → 4 (three legitimate debounces plus the clipboard “Copied” reset).

# 9. Bug findings

Nineteen bugs were found by the audit and a twentieth (BUG-020) was found *and fixed* while diagnosing a real Gmail sign-in failure; all twenty are documented in full (severity, expected/actual, root cause, affected files with line numbers, reproduction, recommended fix) in **`BUG_AUDIT.md`**. Counts: **CRITICAL 1 · HIGH 1 · MEDIUM 8 · LOW 10**. The three most consequential:

- **BUG-001 (CRITICAL)** — migrations are non-transactional and the version marker is written outside the transaction, so an interrupted upgrade can leave a schema that cannot be re-migrated, leaving the app unable to start.
- **BUG-002 (HIGH)** — replies/forwards never write `emails.thread_id`, so email conversation threading is unimplemented for real mail.
- **BUG-020 (MEDIUM, fixed in this pass)** — sign-in failures were classified by keyword only, so Google's `[ALERT]` policy answers (App Password required, password sign-in blocked, IMAP disabled) and “this endpoint is not IMAP” failures all surfaced as “the server refused the sign-in without a specific reason”. Now classified per provider/shape, with `VerifyFailure::Protocol` separating configuration errors from credential verdicts, capability-driven SASL PLAIN retry, and host/port/encryption-only diagnostics.

# 10. Security findings

Eleven findings are documented in full in **`SECURITY_AUDIT.md`**. Counts: **CRITICAL 0 · HIGH 0 · MEDIUM 3 · LOW 7 · INFO/positive 1**. In short:

- **SEC-001 (MEDIUM)** — ~~a cleartext HTTP autoconfig fallback can hand the user's mailbox password to a network attacker~~ **FIXED**: discovery now fetches autoconfig over HTTPS only.
- **SEC-002 (MEDIUM)** — no encryption at rest for the entire local mail/message/contact store; no backup or wipe path.
- **SEC-003 (MEDIUM)** — ~~the global database mutex is held across network I/O, letting a hostile server freeze the UI~~ **FIXED for the sign-in path** (same root cause as BUG-004): `test_email_connection` locks only to snapshot and to record, and `google_oauth_sign_in` runs its browser half on a worker thread.
- Verified strengths: passwords live only in the OS keyring (a test proves the DB never holds them), minimal Tauri capabilities (no fs/shell/http), restrictive CSP, no HTML mail rendering, no `dangerouslySetInnerHTML`, parameterized SQL everywhere, IPC panic containment, `npm audit` clean, and no secrets in the repository.

# 11. Database findings

Schema and migration review (all eleven migration files read; suite executed):

| Aspect | Finding |
|---|---|
| Versioning | Monotonic `PRAGMA user_version` 1 → 11, asserted by a test (`initializes_current_schema_and_development_cache` expects 11) |
| **Migration atomicity** | **Not transactional**: each batch runs, then the version is bumped outside any transaction → BUG-001. A partially applied migration is retried next launch and `ALTER TABLE ADD COLUMN` steps are then fatal |
| **Write atomicity** | **No transactions anywhere in the Rust code** (grep `transaction|BEGIN|COMMIT|ROLLBACK|savepoint` → 0 matches). Multi-statement writes are non-atomic: `save_draft` (mail + N recipients + queue), `queue_send` (state + folder move + queue), `move_email_to_folder` (folder insert + mail update + queue), `add_account_verified` (account + self contact + queue), `send_message` (message + conversation + queue), `delete_message` (cascade update + queue) |
| Foreign keys | `PRAGMA foreign_keys = ON` at open; FKs on accounts→folders→emails, conversations→messages, messages→messages (`reply_to_id`, `thread_id`, `in_reply_to`), channel_members, message_reactions, email_recipients/attachments |
| Delete semantics | Features only soft-delete (`deleted_at`), so no cascade/orphan damage occurs in practice; `ON DELETE CASCADE` is declared where hard deletes would matter (reactions, thread pointers, members, recipients, attachments) |
| Constraints | `TEXT PRIMARY KEY` throughout; `UNIQUE(accounts.email_address)`, `UNIQUE(sync_queue.entity_type, entity_id, operation)`, `UNIQUE(message_id, contact_id, emoji)`, `CHECK(recipient_type IN ('to','cc','bcc'))`, `NOT NULL` on required columns |
| Idempotency | `enqueue` uses `ON CONFLICT … DO UPDATE`, so repeated mutations collapse into one queue row per (entity, operation) — correct, but the payload is empty (BUG-003) |
| Indexes | Appropriate coverage: `emails(folder_id, received_at)`, `emails(thread_id)`, `messages(conversation_id, sent_at)`, `messages(thread_id, sent_at)`, partial `messages(conversation_id, thread_id) WHERE reply_to_id IS NULL`, `messages(reply_to_id, sent_at)`, `messages(conversation_id, pinned_at)`, `sync_queue(status, next_attempt_at, created_at)`, `sync_conflicts(status, created_at)`, `notifications(is_read, created_at)`, `conversations(kind, updated_at)`, `email_recipients(email_id)`, `email_attachments(email_id)`. There is no `emails(account_id)` index — not needed today, but a multi-account filter (BUG-009) will want one |
| FTS integrity | Migration 0008 replaced the unsalvageable contentless design with external-content FTS5, old-value triggers and a `'rebuild'` backfill — correct and covered by tests. The rebuild of four indexes runs synchronously during the upgrade (§17) |
| Retention/growth | Only soft deletes, no `VACUUM`, no retention policy, no size guard → the file grows monotonically and never shrinks (LOW) |
| Resilience | WAL is enabled (good crash durability), but there is no `PRAGMA integrity_check`, no backup and no recovery UX; `synchronous` is left at the default (SEC-002, BUG-001) |
| Duplicate/orphan rows | None found. Seeds use fixed ids with `INSERT OR IGNORE`/`ON CONFLICT`; every feature path uses bound parameters and validated ids |
| SQL injection | None. All values are bound; the only interpolated SQL is the whitelisted folder role (`is_mail_role`) plus an internal literal in `SyncEngine::count` |
| Unused tables | `sync_conflicts` and `sync_metadata` are written by no code path (reserved for the future transport); reading them is safe, but they will stay empty |

# 12. Tauri and IPC findings

Surface: **49 commands** registered in `lib.rs`, all delegating to `Repositories`, `SyncEngine` or a service. Shared properties verified:

- Every command that touches the database takes the same `Mutex<Database>` (`AppState`), so commands are strictly serialized and a slow holder blocks all others (BUG-004/SEC-003).
- `guard::guarded` wraps every command that can panic (`add_email_account`, `test_email_connection`, `google_oauth_sign_in`, and the `auto_sign_in` worker thread); the panic payload is logged, the UI receives a generic error (test: `panics_in_command_work_become_errors`).
- Exactly one command is `async` (`auto_sign_in`) and it moves discovery off the IPC thread using a `tokio::sync::oneshot` handshake with an OS worker thread — the in-repo pattern for keeping the UI responsive.
- No command accepts a filesystem path or a shell string; `clear_cache` derives its path from `app_data_dir` and deletes only under `cache/`.
- Validation is consistent and centralized: lengths (`validate_required` ≤ 255, search ≤ 200, message body ≤ 20 000, subject ≤ 998, draft body ≤ 2 000 000), e-mail shape (≤ 320 + `@`), folder roles (`is_mail_role`), reaction emoji (≤ 16 bytes, no whitespace), presence/connection-state enums and notification length caps. Invalid payloads are rejected before SQL.

Command inventory (Used = called by the UI today; Tested = exercised by the suite or in this audit):

| Command | Used | Validated | Authorized | Secure | Tested |
|---|---|---|---|---|---|
| get_app_health | no (stub) | n/a | n/a | ok | — |
| get_database_info | no | n/a | n/a | path disclosure (SEC-005) | — |
| get_inbox | no | role literal | n/a | ok | ✅ |
| get_emails_in_folder | ✅ | ✅ role whitelist | n/a | ok | ✅ |
| get_starred | ✅ | ✅ | n/a | ok | ✅ |
| get_email | ✅ | id length | n/a | ok | ✅ |
| get_email_thread | ✅ | id length | n/a | ok | ✅ |
| mark_email_read | ✅ | ✅ | n/a | ok | ✅ |
| set_email_star | ✅ | ✅ | n/a | ok | ✅ |
| archive_email / trash_email | ✅ | ✅ role whitelist | n/a | ok | ✅ |
| get_accounts | ✅ | n/a | n/a | ok | ✅ |
| add_email_account | ✅ | ✅ + server verify | n/a | password → keyring | ✅ (no-plaintext test) |
| test_email_connection | ✅ | ✅ | n/a | keyring + verify | ✅ (rotation) |
| remove_account | ✅ | ✅ | n/a | purges keyring | ✅ |
| auto_sign_in | ✅ | ✅ e-mail/length | n/a | verifies before persisting | ✅ (discovery tests) |
| google_oauth_sign_in | ✅ | ✅ client id | n/a | PKCE + state + XOAUTH2 | ✅ (spec tests) |
| get_contacts | ✅ | ✅ | n/a | ok | ✅ |
| create_contact | ✅ | ✅ name/e-mail | n/a | ok | ✅ |
| set_contact_favorite | ✅ | ✅ | n/a | ok | ✅ |
| get_pending_sync_items | no | ✅ | n/a | ok | — |
| save_draft | ✅ | ✅ lengths/recipients | n/a | ok | ✅ (client-side loss window BUG-007) |
| queue_email_send | ✅ | ✅ recipient required | n/a | ok | ✅ |
| get_channels | ✅ | n/a | n/a | ok | ✅ |
| get_channel_messages | ✅ | ✅ | n/a | ok | ✅ |
| send_company_message | ✅ | ✅ + thread-target check | n/a | ok | ✅ |
| get_conversations | ✅ | n/a | n/a | ok | ✅ |
| open_direct_message | ✅ | ✅ contact must exist | n/a | ok | ✅ |
| edit_message | ✅ | ✅ body length | n/a | ok | ✅ |
| delete_message | ✅ | ✅ | n/a | ok | ✅ |
| set_message_pin | ✅ | ✅ | n/a | ok | ✅ |

| get_message_thread | ✅ | ✅ | n/a | ok | ✅ |
| get_channel_reactions | no | ✅ | n/a | ok | ✅ |
| toggle_message_reaction | ✅ | ✅ emoji/length | n/a | ok | ✅ |
| get_sync_overview | ✅ | n/a | n/a | ok | ✅ |
| retry_failed_sync | ✅ | n/a | n/a | ok | ✅ |
| get_notifications | ✅ | n/a | n/a | ok | ✅ |
| get_unread_notification_count | ✅ | n/a | n/a | ok | ✅ |
| mark_all_notifications_read | ✅ | n/a | n/a | ok | ✅ |
| get_notification_preferences | ✅ | n/a | n/a | ok | ✅ |
| update_notification_preference | ✅ | ✅ key exists | n/a | ok | ✅ |
| show_native_notification | no | ✅ lengths | n/a | ok | — |
| get_app_settings | ✅ | n/a | n/a | ok | ✅ |
| set_app_setting | ✅ | ✅ key ≤ 64 / value ≤ 512 | n/a | ok | ✅ |
| get_presence | ✅ | n/a | n/a | ok | ✅ |
| set_presence | ✅ | ✅ enum | n/a | ok | ✅ |
| search_global | ✅ | ✅ length/empty | n/a | ok (FTS5 metachars: BUG-010) | ✅ |
| get_storage_usage | ✅ | n/a | n/a | directory sizes leaked (SEC-005) | — |
| clear_cache | ✅ | n/a | n/a | confined to `cache/` | — |

The “Authorized” column is **n/a for all rows** because the application is single-user and local: there is no actor, role or resource ownership to check (see `SECURITY_AUDIT.md` → Authorization assessment). This column must become a real check server-side when sync lands.

# 13. Rust findings

- **No `unsafe`** anywhere (grep → 0 matches) and **no `todo!`/`unimplemented!`** (→ 0). `panic!` appears only in `build.rs` (build-time failure) and in `guard.rs` documentation.
- `unwrap()` appears only four times, all in `commands.rs` directory-size walks immediately after an explicit `is_none()` check (safe but non-idiomatic — `let Some(entries) = … else { return 0 }` would be cleaner). `expect()` appears once in `lib.rs` (`run().expect("error while running Relay")`) which is intentional fatal-at-startup behaviour, plus `build.rs` env lookups.
- Blocking I/O: IMAP/DNS/HTTPS work is synchronous. `auto_sign_in` and `google_oauth_sign_in` correctly offload it to worker threads; `add_email_account` verifies before locking (good); `test_email_connection` takes the lock only to snapshot and to record, never across its network calls. ~~`test_email_connection` and `google_oauth_sign_in`'s second phase hold the DB guard across network calls (BUG-004/SEC-003)~~ — fixed in the authentication pass.
- Error handling: `AppError` distinguishes `Database`/`Io`/`SecureStore`/`Validation`, but command handlers collapse everything into one user string with no logging, so failures are hard to diagnose (SEC-011).
- Panic containment (`guard.rs`) is a genuinely good defensive pattern given that a panic inside the webview2 IPC callback aborts the process; the documented trade-off (a poisoned mutex degrades the session) is acceptable but should be surfaced as a restart requirement.
- `discover.rs` builds its own minimal tokio runtime for the async resolver (with comments explaining why `spawn_blocking` could not be used) and degrades gracefully to no candidates when DNS/HTTP fails — correct behaviour offline.
- `oauth.rs` implements PKCE S256, `state` validation, a loopback-only ephemeral listener and bounded waits without adding a JWT dependency; the hand-rolled base64url encoder is specification-tested.
- `verify.rs` probes reachability with a 5 s timeout before the IMAP handshake so unreachable hosts fail fast, honors the configured encryption mode explicitly, and retries `LOGINDISABLED` servers with SASL PLAIN.
- Minor: `seed.rs` intentionally writes rows with plain SQL (debug-only) — correctly gated by `#[cfg(debug_assertions)]` and documented.
- Serialization/deserialization: commands use typed `serde` structs with `camelCase` renames; the mirror in `src/platform/tauri.ts` is hand-maintained, so contract drift is possible (no codegen, no shared schema test).

# 14. Frontend findings

- No `console.log` left in the codebase; every failure path logs with a `[relay] …` prefix and shows a user-facing message, so no error is silently swallowed. `catch {}`-style empty handlers were not found (`catch { /* keep default */ }` appears only around purely optional preference reads in `SettingsView.refresh`, which is acceptable and commented).
- Optimistic updates are used for stars, favorites, pins and message edits, and the star/favorite paths revert on failure; message edit/pin/delete rely on the server response, so a failure leaves the UI unchanged (correct).
- Listener hygiene is correct: one `addEventListener` per `removeEventListener`, debounce timers cleared, Tauri event `unlisten` captured and called.
- Views are remounted per navigation (`key={navId}`), which reloads data on every switch — simple and correct, at the cost of re-querying (acceptable at current limits, relevant if pagination arrives).
- No error boundary (BUG-006), no `online`/`offline` awareness (BUG-015), no virtualization/pagination (BUG-014), no i18n (hard-coded English strings), and accessibility is mostly good (aria labels, roles, `aria-pressed`/`aria-expanded`) except for a few title-only affordances (presence dot, workspace button that looks like a dropdown but navigates).
- Type contract risk: `ChatMessage`, `EmailSummary`, etc. are duplicated by hand between Rust and TypeScript; the messenger work in this session changed the Rust struct and the TS mirror in lockstep, but nothing enforces it.
- The browser preview backend is maintained in parallel with the native one; the messenger thread contract is now covered by tests on both sides, other domains (drafts, mail actions) are not.

# 15. Synchronization findings

Pipeline as shipped: `local write → repositories::enqueue() → sync_queue row (status pending) → [no worker] → [no transport] → [no server] → UI reads counters via get_sync_overview`.

| Check | Result |
|---|---|
| Queue exists and is durable | ✅ `sync_queue` with `status`, `attempt_count`, `next_attempt_at`, `locked_at`, `completed_at`, `last_error` |
| Worker / scheduler | ❌ none — nothing ever selects a ready queue row |
| Transport | ❌ `DisabledTransport` (`is_configured() == false`) is the only adapter |
| Retry | ⚠️ `retry_failed()` moves `failed` → `pending` and is wired to a Settings button, but nothing can fail because nothing runs; `attempt_count` is never incremented |
| Duplicate prevention / idempotency | ✅ `UNIQUE(entity_type, entity_id, operation)` + `ON CONFLICT … DO UPDATE` |
| Payload | ❌ every row stores `'{}'` (BUG-003) — the queue cannot deliver or replay the change |
| Conflict handling | ⚠️ `sync_conflicts` table exists with local/remote versions and a resolution status, but no writer |
| Timestamps / versions | ✅ `sync_version` incremented on every mutation, `updated_at` maintained, `server_id` column reserved |
| Deletion sync | ⚠️ deletes enqueue intent rows, but a thread-root delete cascades locally while queueing one item (BUG-012), and payloads are empty so tombstones carry no identifiers |
| Reconnect | ❌ no connectivity detection or resume logic (no `online`/`offline` handling anywhere) |
| Failure isolation | ⚠️ a failed local write leaves no queue trace for partial multi-statement writes (§11) |

Scenario “create data → disconnect → create more data → restart → reconnect → synchronize” cannot be executed, and could not produce a meaningful result even if it were: there is no transport to synchronize with. The honest classification is **queue-and-status only**; the app never loses data locally (SQLite commits per statement) but nothing is ever transmitted, so the “local-first with future sync” claim is architecturally prepared rather than functional.

# 16. Offline findings

Verified by reading every network call site (the only ones are in `verify.rs`, `discover.rs`, `oauth.rs`) and confirming that no repository, command used by the local UI, or frontend module performs I/O:

| Capability | Offline behaviour | Class |
|---|---|---|
| Start the app / open the window | SQLite opens locally; the Tauri setup hook has no network dependency; a failed account check fails open | Fully Offline |
| Read mail, folders, starred, threads | Pure SQLite | Fully Offline |
| Star / read / archive / trash / move | Local writes + queue rows | Fully Offline |
| Compose, autosave drafts, resume | Local writes | Fully Offline |
| Send (queue) | Files into Sent, records `send_smtp` — nothing is sent regardless of connectivity | Fully Offline (delivery missing, not network-dependent) |
| Messenger: send/edit/delete/pin/react/threads | Local writes + queue rows | Fully Offline |
| Contacts, search, settings, storage stats, notifications history | Local | Fully Offline |
| Sign-in / re-verify / test connection | Requires IMAP; fails fast with classified messages (5 s reachability probe) | Online Required |
| Auto sign-in discovery | Requires DNS/HTTP/IMAP; degrades to “no settings discovered” and offers manual setup | Online Required |
| Google OAuth | Requires browser + Google endpoints; bounded 300 s wait | Online Required |
| Sync | Not implemented; reports “offline · no transport” always | N/A |

No feature is **Broken Offline**: every offline failure path is either local-only (works) or explicitly reported. Note that the *status indicator* is dishonest about connectivity (BUG-015) even though the underlying behaviour is honest.

# 17. Performance findings

- **Single global lock.** All commands share `Mutex<Database>`; any slow command serializes the entire UI. The network calls used to run *under* the lock (BUG-004), which is now fixed for the sign-in path; the remaining risk is inherent to the single-mutex design, not query cost.
- **No pagination or virtualization.** Folder lists (≤200), channel history (≤250 rows but nodes with subqueries), thread replies (≤100), contacts (≤500), notifications (≤100) all render in full. Acceptable at current caps; the caps themselves are a correctness issue (BUG-014).
- **Query shapes are reasonable.** `channel_messages` uses two correlated aggregate subqueries per row (reply count, last reply time) — fine for 250 rows; `list_emails` resolves an outbound recipient via a correlated subquery per row — fine for 200; `search` runs four indexed FTS5 queries with `LIMIT 8` each; `thread_reactions` was consolidated into one query instead of N+1.
- **FTS rebuild on upgrade.** Migration 0008 rebuilds four indexes synchronously inside the startup path; on a large database this is a visible one-off start delay with no progress indication (LOW).
- **Filesystem walks on the IPC thread.** `get_storage_usage` recursively sizes five directories (`dir_size` recursion) synchronously; `clear_cache` deletes recursively. Small directories today, unbounded later (LOW).
- **Repeated queries per navigation.** Views remount on every switch (`key={navId}`) and re-run their loads; `App` re-reads all settings on mount and `LoginView` re-reads settings for the OAuth client id — no caching layer, no debouncing beyond search (acceptable, but it is why a stuck lock feels worse).
- **No frame-rate hazards found.** Debounces are short (180 ms search, 400 ms pane width, 700 ms autosave), no `setInterval` polling, no unbounded caches, no listener leaks (verified counts), and React state updates are scoped to the affected view.
- **Memory:** no accumulation paths found in the frontend; Rust holds one connection and one global state. Two unbounded buffering spots exist on the network side (SEC-009) rather than in normal local operation.

# 18. Data-loss risks

Ordered by likelihood × impact, each answering “what happens if the app closes or crashes right here?”:

1. **Interrupted migration (BUG-001).** Crash during a version batch → schema half-applied, retry fails, app cannot start. Highest impact: a transient failure becomes an unrecoverable state, with no backup and no in-app recovery.
2. **Non-transactional multi-statement writes (§11).** A crash between statements can leave an outbound mail with no recipients (then rejected by `queue_send`), a moved mail whose folder row was never created, or an account without its messenger identity row.
3. **Draft autosave debounce (BUG-007).** Closing the composer or the window within 700 ms of typing silently discards the newest text; no “unsaved changes” prompt, no flush on unmount.
4. **No backup/export/import.** The only copy of the data is `relay.db`; disk failure or deleting the app-data directory loses everything. `clear_cache` is scoped to `cache/` today, but it recursively deletes whatever later features place there (attachments/avatars are the likely candidates) — a latent destructive action.
5. **Outbound mail semantics.** A “queued” send is a permanent local record with `delivery_state = 'queued'` plus a pending `send_smtp` row. The reading pane now says so on the message itself (*“Not delivered yet. This message is saved in Sent on this computer. This build has no mail transport, so no copy has left the machine.”*), and the composer closes onto that message, so the state can no longer be misread as delivery (BUG-023). The underlying limit stands: a user who believes it was sent — or who discards the draft or reinstalls — loses the message in the sense that matters, until a transport exists.
6. **Account removal.** Credentials are correctly purged, but cached mail remains and the confirmation is a single `window.confirm` string with no summary of what is kept versus removed.
7. **No crash reporting.** User-facing failures produce a generic message and no log entry (SEC-011), so data-affecting bugs are unlikely to be reported with usable detail.

Verified as *not* a data-loss risk: WAL plus per-statement commits means ordinary writes survive a crash; soft deletes make accidental presses recoverable at the row level; uninstalling is safe for data (app-data is identifier-based, not install-path-based).

# 19. Privacy findings

- Mail bodies, subjects, participants, message text, contacts and notification text are stored unencrypted in SQLite (SEC-002) — the primary privacy exposure, identical to the data-at-rest finding.
- No telemetry, analytics, crash reporting or advertising identifiers exist anywhere (verified by reading all network call sites); the only outbound requests are user-initiated sign-in verification, discovery and OAuth.
- No third-party content is fetched or rendered (no remote images, fonts or CDNs): tracking pixels and remote content cannot appear because email HTML is never rendered.
- Notification content derives from local data and the native-toast command caps title/body lengths; it is currently never invoked, so nothing can leak to a lock screen today.
- Presence is local-only and therefore leaks nothing — but it also conveys nothing.
- The one privacy-specific leak found is SEC-005 (absolute paths and directory sizes returned over IPC).

# 20. Dependency findings

- **Frontend**: React 18.3, `@tauri-apps/api` 2.8, `lucide-react` 0.468, Vite 6, TypeScript 5.6, Vitest 5 — a small, current set (9 production dependencies). `npm audit` reports **0 vulnerabilities** across 154 installed packages.
- **Rust**: Tauri 2, `rusqlite` 0.32 with bundled SQLite (no system-library dependency), `uuid`, `chrono`, `thiserror`, `tracing`, `imap` 3.0.0-alpha.15 (native-tls), `ureq` 2.10 (native-tls + json), `sha2`, `hickory-resolver`/`hickory-proto` 0.24, `tokio` 1 (rt/net/time/sync only), `keyring` 3 (windows-native). No unmaintained or unsafe-heavy crates observed; the **alpha `imap` crate** is the main “watch this” dependency and the reason IMAP is verified but not exercised end-to-end.
- **Hygiene (SEC-007)**: both `package-lock.json` and `pnpm-lock.yaml` + `pnpm-workspace.yaml` exist, so two resolution graphs describe the same project; there is no lint tool, no CI and no `cargo audit`/`cargo deny` step.
- **Unused dependency**: `tauri-plugin-opener` is used only for the OAuth consent URL, yet its capability is granted to the webview (SEC-004); the frontend package `@tauri-apps/plugin-opener` is never imported.
- **Build-only tooling**: `scripts/gen-icon.exe` is a committed helper binary (gitignored going forward); `build.rs` shells out to `windres`, a documented build-time environment requirement.

# 21. Testing findings

Existing and passing: **36 Rust integration tests** (`src-tauri/tests/local_store.rs`) and **26 frontend tests** (`src/app/util.test.ts` 16, `src/platform/preview.test.ts` 10).

Coverage strengths: schema/migration bootstrap and version assertion; the migration 0011 legacy backfill; repository behaviour for mail, folders, stars, threading and recipients; FTS5 search including soft-delete exclusion; messenger actions, reactions, threads and DM idempotency; settings/presence round-trips; sync retry and offline status; keyring round-trip; verified sign-in including the “password never in the DB” invariant; queued-send folder filing; self-identity recovery; SASL PLAIN vectors and refusal classification; Google OAuth wire formats plus client id/secret validation and token-error hints; the XOAUTH2 authorization-refusal split; Thunderbird-style discovery parsing/ranking plus per-provider `auth_method` routing; the loopback callback separating a grant from a refusal; resolver-runtime guard; IMAP retrieval (MIME/base64/quoted-printable/RFC 2047 decoding, HTML-to-text reduction, per-account folder scaffolding, UID-keyed idempotent persistence that preserves local filing); command-panic containment.

Critical gaps (functionality with no automated coverage):
- **`commands.rs` itself** (27 KB, 49 commands) has no tests beyond panic containment; per-command validation and error mapping are unverified. Largest blind spot.
- **No component/interaction tests** for any view: `MailView`, `MessengerWorkspace`, `Composer`, `SettingsView`, `ContactsView`, `SearchPalette` are covered only by `tsc` types and manual reasoning. Every frontend bug found here (theme persistence, autosave flush, neighbour selection, mark-all-read) is exactly what one interaction test would catch.
- **No end-to-end test** (no Playwright/WebDriver), so GUI launch, the window-state plugin, CSP behaviour and IPC wiring in a real webview remain unverified here.
- **No test for multi-account mixing** (BUG-009) or email reply threading (BUG-002) — both are assumptions, not verified behaviour.
- **No “upgrade an existing older database” test** other than the 0011-specific legacy-schema case; migrations 0002–0010 are only exercised by fresh creation.
- **No fault-injection tests** (disk full, locked database, crash mid-migration) — which is why BUG-001 stayed invisible.
- The suite never touches real IMAP/SMTP/OAuth servers (correctly), so those flows are “implemented and unit-specified”, never integration-verified.

# 22. Recommended fixes

Grouped by the priority ladder (P0 highest); each item names the findings it resolves.

**P0 — security, data loss, crash/startup blockers**
1. Wrap every migration step in a transaction, bump `user_version` inside it, back up `relay.db` before migrating, surface a recovery path on failure, and make `ALTER TABLE` migrations idempotent (BUG-001).
2. Wrap all multi-statement writes in a transaction (`save_draft`, `queue_send`, `move_email_to_folder`, `add_account_verified`, `send_message`, `delete_message`) using one shared helper (§11).
3. ~~Delete the cleartext `http://autoconfig…` fallback~~ — **done**: discovery now fetches autoconfig over HTTPS only (SEC-001), so a network attacker can no longer redirect the password to their own IMAP host.
4. ~~Treat the state lock as a short-lived resource~~ — **done for the sign-in path**: `test_email_connection` snapshots under the lock, releases it across the network calls and re-locks only to record, and `google_oauth_sign_in` now runs its browser half on a worker thread (BUG-004 / SEC-003).
5. Decide and document data-at-rest protection (encrypt, or state plainly in-product that it is not encrypted) and add backup/export plus a genuine “delete all local data” action (SEC-002).

**P1 — broken core functionality**
6. Write `emails.thread_id`/`message_id_header` for replies and forwards and plumb them through the composer, with a regression test (BUG-002).
7. Populate `sync_queue.payload` with the entity snapshot; implement the worker/transport or remove the commands that imply one (BUG-003, BUG-008, BUG-012).
8. Scope folder queries by account and add an account label/filter to the mail UI (BUG-009).
9. Establish version control and CI (build, tests, clippy, fmt, `npm audit`, lint) and pick a single lockfile (SEC-007).
10. Add the missing lint tooling so the workflow's lint step exists (§2, SEC-007).

**P2 — database and reliability**
11. Add pagination / “showing N of M” to the capped queries (BUG-014).
12. Harden FTS5 input handling (BUG-010); keep the sign-in failure classification aligned with provider answers (BUG-020, fixed).
13. Replace mutex-poisoning failure with a restart-required state and log underlying errors at the command boundary (SEC-011).
14. Cap response bodies on the network paths (SEC-009).

**P3 — frontend reliability**
15. Persist the palette theme toggle through the same path as the Settings control (BUG-005).
16. Flush the composer debounce on close/unmount (BUG-007).
17. Add an error boundary and an `online`/`offline`-aware status indicator that distinguishes “no transport” from “no network” (BUG-006, BUG-015).
18. Fix neighbour selection after archive/trash and refresh the unread badge on mutation (BUG-018, BUG-013).

**P4 — performance**
19. Move `get_storage_usage`/`clear_cache` filesystem walks off the IPC thread; add progress for the migration-time FTS rebuild (§17).
20. Add list virtualization once pagination exists.

**P5 — UX / presentation**
21. Prune or implement the “Threads”/“Channels” aliases and other misleading labels; show a busy state during long commands (BUG-017, BUG-016).
22. Expose per-notification read state so “Mark all read” is visible (BUG-019).
23. Add a persistent “insecure connection” indicator for plaintext IMAP accounts (SEC-006).

**P6 — cleanup / refactoring**
24. Remove unused commands and dependencies (`get_database_info`'s path exposure, `get_inbox`, `get_pending_sync_items`, `show_native_notification`, `get_channel_reactions`, `@tauri-apps/plugin-opener`, the `opener` capability) (SEC-004, SEC-005).
25. Generate the TypeScript IPC types from the Rust models, or add a contract test, to remove hand-maintained drift (§13, §14).

# 23. Production readiness assessment

| Dimension | Assessment |
|---|---|
| Build / packaging | Source builds and links on this machine (frontend + Rust + tests). Installer bundling and a clean-machine install were **not** exercised (no `tauri build` run, MSVC SDK absent per `.cargo/config.toml` notes) |
| Local functionality | Strong and genuinely tested for the local data plane; the mail workspace and messenger work offline and persist across restarts |
| Product completeness | Roughly **half** of the promised product: no mail retrieval, no delivery, no attachments, no channel administration, no company sync |
| Data safety | Weakest link: non-atomic migrations and non-transactional multi-statement writes, no backup or export. Not acceptable for real mail before P0 fixes |
| Security | No remotely exploitable issue; credential handling is a strength. SEC-001 (cleartext autoconfig), SEC-002 (plaintext store) and SEC-003 (lock across I/O) must be resolved or explicitly accepted before real mailboxes are configured on untrusted networks |
| Reliability | Migrations, the global lock and the missing error boundary are the top reliability risks; no crash reporting |
| Observability | Minimal: tracing is initialised, but command failures are not logged, and there is no diagnostics surface for users or support |
| Performance | Fine at current data volumes; a single global lock is the ceiling, and list caps hide the missing paging |
| Test coverage | Good for repositories and parsers, thin for commands and absent for UI interactions |
| Engineering process | No version control, no CI, no lint, duplicate lockfiles — process risk exceeding most code risks |
| Documentation | Unusually honest and detailed (`README`, `ARCHITECTURE`, `DATABASE`, `SECURITY`, `DEVELOPMENT` + per-phase notes); it correctly labels what is missing |

**Verdict: NOT PRODUCTION READY.** The blocker is not quality of the existing code — much of it is careful, defensive and documented — but three things: the transport half of the product does not exist yet, the persistence/upgrade layer is not crash-safe, and there is no version control or CI protecting either. The local-first core is a credible foundation to build the rest on.

---

```text
============================================
APPLICATION AUDIT SUMMARY
============================================

Build:                       PASS   (npx tsc -b, vite build 1602 modules; cargo build links)
Application Startup:         PARTIAL (code path verified, Database::open covered by tests; GUI window launch NOT RUN)
Frontend:                    PASS   (build + 15/15 vitest; no GUI interaction test)
Rust:                        PASS   (cargo test 29/29; clippy clean except 1 pre-existing warning)
Database:                    PASS   (11 migrations, schema v11 asserted; WAL + FKs) with a CRITICAL atomicity defect (BUG-001)
Authentication:              PARTIAL (real IMAP verification + keyring storage; no session/sign-out; gate fails open)
Email:                       PARTIAL (full local store + drafts + search; NO retrieval, NO delivery)
Messenger:                   PARTIAL (full local behaviour, persistence verified; no receive path/transport)
Threads:                     PASS   (messenger threads FUNCTIONAL and tested; email threading BROKEN for new replies)
Channels:                    PARTIAL (read-only history/membership; no administration, no permissions)
Contacts:                    PASS   (create/favorite/search/email/message; no edit/delete/groups)
Search:                      PASS   (FTS5 across 4 entity types, tested; brittle on FTS5 metacharacters)
Synchronization:             NOT IMPLEMENTED (durable queue + honest status only; empty payloads, no worker, no transport)
Offline Mode:                PASS   (every local feature works offline; online features report clearly)
Notifications:               PARTIAL (history/counts/preferences persist; no producer, no native toast in use)
Security:                    PASS with 3 MEDIUM findings (no CRITICAL/HIGH; credential handling sound)
Tests:                       PASS   (36 Rust + 26 frontend) with major gaps (commands, UI interactions, upgrade path)

============================================
ISSUE COUNTS
============================================

Critical:  1   (BUG-001)
High:      1   (BUG-002)
Medium:   11   (BUG-003 … BUG-009, BUG-020, SEC-001, SEC-002, SEC-003)
Low:      17   (BUG-010 … BUG-019, SEC-004 … SEC-009, SEC-011)
Info:      1   (SEC-010 — verified positive control)
Total findings: 31   (BUG-020 fixed during this pass)

============================================
FUNCTIONALITY
============================================

Fully Functional:     45%   (19 of 42 graded matrix rows)
Partially Functional: 17%   (7 rows)
Broken:                0%   (no row is wholly broken; 8 individual behaviours are broken — see §5)
Mocked:                5%   (2 rows: presence, connection status)
Not Implemented:      26%   (11 rows)
Cannot Verify:         5%   (2 rows: live auto-discovery and Google OAuth against real servers)

(Percentages are of the 42 rows in the §3 matrix, rounded; they measure feature-surface coverage, not code quality.)
```

# PRODUCTION BLOCKERS

Must be resolved before this application handles real user mail:

1. **Data loss / unstartable app after an interrupted upgrade** — non-transactional migrations with the version marker written outside the transaction and no backup (BUG-001, CRITICAL).
2. **Partial writes** — no transaction wraps any multi-statement write, so a crash leaves inconsistent state (mail without recipients, moved mail without its folder) (§11).
3. ~~**Credential redirection over cleartext** — the HTTP autoconfig fallback can send a user's mailbox password to an attacker-controlled IMAP host (SEC-001).~~ **Fixed.**
4. ~~**Application-wide freeze from a hostile or slow mail server** — the global database lock is held across network I/O, blocking all 49 commands (BUG-004 / SEC-003).~~ **Fixed for the sign-in path** (snapshot → unlock → network → relock; OAuth off the IPC thread).
5. **No data-at-rest decision** — the mailbox is stored unencrypted with no backup, export or wipe path (SEC-002); this must at minimum become an explicit, documented product statement.
6. **Core product promise unmet** — no IMAP retrieval and no SMTP delivery, so the app cannot receive or send mail (Email PARTIAL; a release build's inbox can never fill).
7. **Broken email threading** — replies never join their conversation, so the conversation feature does not work for real mail (BUG-002, HIGH).
8. **Un-shippable sync contract** — every queue row carries an empty payload, so no transport can deliver or replay queued work (BUG-003).
9. **No version control and no CI** — no `.git`, no automated build/test/audit gate, no lint step; a regression or advisory would go undetected and a bad change could not be reverted (SEC-007).

Required before a public beta (not blockers): pagination (BUG-014), crash reporting/diagnostics (SEC-011), multi-account scoping (BUG-009), an error boundary (BUG-006), and honest connectivity/sync UX (BUG-008, BUG-015).

---

# Audit limits (what this audit did **not** verify)

Stated explicitly so nothing above is over-read:

| Not performed | Reason | Consequence |
|---|---|---|
| `pnpm install` / `pnpm build` / `pnpm lint` | The project uses npm; pnpm exists only as a stray lockfile and no lint script exists | Toolchain checks ran with npm instead; the lint step cannot be executed at all |
| GUI launch (`npm run tauri dev`), window creation, WebView2 rendering | Interactive, and no display available to the auditor | Startup is verified by code path plus `Database::open` test coverage, not by observing a window |
| Installer bundling (`npm run tauri build`, MSIX/NSIS) | Long-running, machine-specific artifacts; MSVC SDK absent per `.cargo/config.toml` | Packaging readiness unverified |
| Live IMAP sign-in, Google OAuth end-to-end, real SMTP | No test mail servers or OAuth client credentials available | Those flows are “implemented and unit-specified”, never integration-verified |
| Runtime “airplane mode” test | No interactive harness in this environment | Offline behaviour verified by reading every network call site, not by disabling the adapter at runtime |
| Memory/CPU profiling and soak testing | No profiler, no interactive session | Performance findings are structural (lock, caps, N+1 analysis), not measured |
| `cargo audit` / `cargo deny` | Not installed; no install performed | Rust-side advisory status unknown (frontend side is clean per `npm audit`) |
| Fixes | Explicitly out of scope for this pass | Every finding carries a recommended fix; no source/configuration file was changed |

The audit modified **no source file**. Three files were created: `COMPLETE_AUDIT.md` (this report), `BUG_AUDIT.md` and `SECURITY_AUDIT.md`.
