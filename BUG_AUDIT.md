# BUG_AUDIT — Relay Desktop

Audit date: 2026-09-28 · Scope: full repository (React/TypeScript frontend, Tauri 2 IPC, Rust backend, SQLite schema, build/test tooling)
Method: static read of every source file, cross-reference of UI → IPC → repository → database, plus executed toolchain checks. No source file was modified for this audit.

Evidence commands run for this audit:

```text
npm run build      # tsc -b && vite build  -> PASS (1602 modules)
npm test           # vitest run            -> 15/15 PASS (util 9, preview backend 6)
cargo build        # binary links          -> PASS
cargo test         # local_store suite     -> 29/29 PASS
cargo clippy --all-targets                 -> 1 pre-existing warning (oauth.rs:126)
npm audit --json                           -> 0 vulnerabilities (154 packages)
```

Distinctions used throughout: **IMPLEMENTED** = code exists; **FUNCTIONAL** = the feature works end to end; **VERIFIED** = it was exercised here. Nothing in this report is marked verified without a command or a reproduction path.

## Issue summary

| ID | Severity | Module | One-line summary |
|---|---|---|---|
| BUG-001 | CRITICAL | Database / migrations | Migrations are not transactional: an interrupted upgrade can leave the schema half-applied and the next launch fails on `ALTER TABLE ADD COLUMN` |
| BUG-002 | HIGH | Email / composer | Replies and forwards are never attached to the conversation (`thread_id` never written), so the thread strip cannot show them |
| BUG-003 | MEDIUM | Synchronization | Every `sync_queue` row stores an empty payload `{}`, so queued work cannot be transported or replayed |
| BUG-004 | MEDIUM | Tauri IPC / accounts | `test_email_connection` (and OAuth re-verify) hold the single global DB mutex across network I/O: the whole UI freezes |
| BUG-005 | MEDIUM | Settings / theme | Command-palette “Toggle Dark Mode” is not persisted; the theme reverts on restart |
| BUG-006 | MEDIUM | Frontend shell | No React error boundary: any render exception blanks the window with no recovery |
| BUG-007 | MEDIUM | Composer / data safety | Draft autosave is debounced 700 ms and never flushed on close, so the last edits can be lost |
| BUG-008 | MEDIUM | Command palette | “Sync Now” performs no synchronization (it re-reads queue counters only) |
| BUG-009 | MEDIUM | Email / multi-account | Folder queries are not account-scoped: two signed-in accounts interleave in Inbox/Sent/Drafts |
| BUG-010 | LOW | Search | FTS5 special characters produce a syntax error surfaced as a generic failure; no escaping/hint |
| BUG-011 | LOW | Notifications | Defaults disagree between native (`sounds = 1`) and the browser preview (`sounds = false`) |
| BUG-012 | LOW | Messenger / sync | Deleting a thread root soft-deletes replies but enqueues a single queue item (asymmetric sync) |
| BUG-013 | LOW | Notifications | Unread badge is refreshed only when the panel opens/closes; the list is not marked per row |
| BUG-014 | LOW | All lists | Silent truncation (200 folder rows / 250 channel rows / 100 replies / 500 contacts) with no pagination |
| BUG-015 | LOW | Status bar | “Offline” comes from a permanently offline transport + a hard-coded health stub; no `online`/`offline` listeners |
| BUG-016 | LOW | Accounts / session | No sign-out/session invalidation, and shell account state goes stale after removing an account |
| BUG-017 | LOW | Navigation | “Threads” and “Channels” both open the Messages view; `AppView` declares views that are never rendered |
| BUG-018 | LOW | Mail reading pane | After archive/trash the reading pane picks the neighbour by a stale index |
| BUG-019 | LOW | Notifications | “Mark all read” appears to have no effect after reopening the panel (read rows are still listed) |
| BUG-020 | MEDIUM | Accounts / sign-in | Refusals are classified by keyword only, so provider policy alerts and “this endpoint is not IMAP” answers both surface as “the server refused the sign-in without a specific reason” — **fixed in this pass** |
| BUG-021 | MEDIUM | Mail list (layout) | The message list cannot scroll: `.list-pane` is a block box, so `.email-list` has no height constraint and its `overflow: auto` never activates — the rows past the first screenful are clipped and unreachable — **fixed in this pass** |

| BUG-022 | MEDIUM | Mail retrieval / reading pane | Every synced message was stored with an empty body — the fetch asked for `BODY[HEADER]`/`BODY[TEXT]` but read them back with `Fetch::body()`, which matches only the unsectioned `BODY[]` — so the reader showed a subject and no content — **fixed in this pass** |
| BUG-023 | MEDIUM | Email send / composer | A send is local-only by design, but the UI hid it: the success text was a developer note, the composer stayed open over an unchanged list, Send with no account selected did nothing silently, every backend failure was reported as a bad recipient, and a queued message looked exactly like a delivered one — **fixed in this pass** |
| BUG-024 | MEDIUM | Mail lists / local store | An outbound row with no To recipient (a Cc/Bcc-only draft, or one still being typed) makes the list's recipient subquery return SQL `NULL`; the column is read into a `String`, so the whole folder query failed and Drafts could not be opened until the draft was edited elsewhere — **fixed in this pass** |

Issue counts: **Critical 1 · High 1 · Medium 12 · Low 10 — total 24** (BUG-020 was found and fixed while diagnosing a real Gmail sign-in failure; BUG-021 was found while adding scrolling to the mail views; BUG-022 was found by tracing why an opened message showed only its subject; BUG-023 was found by tracing what “Send” actually does end to end for a user who could not tell whether mail had been sent; BUG-024 was found by the regression test written for BUG-023 — a Cc-only draft failed the folder listing).

---

## BUG-001

Severity: CRITICAL
Module: Database / migrations
Feature: Schema migration at application start (`Database::open` → `migrate`)

Expected:
An upgrade is all-or-nothing: if the process dies while a migration runs, the database is left at the previous version or the new one, and the next start re-runs or completes it safely.

Actual:
`migrate()` reads `PRAGMA user_version` once, then executes each migration batch and bumps `user_version` *afterwards*, with no transaction around either step. SQLite does not wrap `execute_batch` in an implicit transaction and the codebase contains no `BEGIN`/`COMMIT`/`transaction()` at all (a search for `transaction|BEGIN|COMMIT|ROLLBACK|savepoint` over `src-tauri/src/*.rs` returns zero matches). A crash between the batch and the version bump leaves a partially applied migration that is retried next launch; for `ALTER TABLE ... ADD COLUMN` migrations (0003, 0005, 0009, 0010, 0011) the retry fails with `duplicate column name`, so `Database::open` errors. Migration 0008 is worse: it drops the four FTS tables and their triggers before recreating them, so a crash inside that batch leaves search silently empty.

Root Cause:
Migration statements are independent non-transactional writes with the version marker written outside any transaction; there is no pre-migration backup and no recovery path.

Affected Files:
- `src-tauri/src/database.rs:28-73` (`migrate`; `execute_batch(<migration>)` then `execute_batch("PRAGMA user_version = N;")`)
- `src-tauri/migrations/0011_message_threads.sql:16-17` (two `ALTER TABLE ADD COLUMN`, taken together non-idempotent)
- `src-tauri/migrations/0008_fts_external_content.sql:10-102` (`DROP TRIGGER`/`DROP TABLE` before recreate)
- `src-tauri/src/database.rs:7-23` (no copy of `relay.db` before migrating)

Reproduction:
1. Start the app against a database at `user_version = 10`.
2. Terminate the process while the 0011 batch runs (a large `messages` table makes the backfill long enough to catch), or during 0008's rebuild.
3. Start the app again: the retry fails with `duplicate column name`, `Database::open` fails, the Tauri `setup` hook aborts and the window never becomes usable, although the database file looks intact.

Recommended Fix:
1. Wrap every step in an explicit transaction: `let tx = connection.transaction()?; tx.execute_batch(include_str!("..."))?; tx.pragma_update(None, "user_version", N)?; tx.commit()?;`.
2. Keep `synchronous = FULL` for the migration connection so the commit is durable before the version becomes visible.
3. Copy `relay.db` (+ `-wal`) to a timestamped backup before migrating, rotate it after a successful start, and offer a recovery action instead of an unstartable app.
4. Make the `ALTER TABLE` migrations idempotent (guard on `PRAGMA table_info(...)`) so a retry cannot be fatal.

## BUG-002

Severity: HIGH
Module: Email / composer / threads
Feature: Reply, Reply all and Forward from the reading pane

Expected:
A reply belongs to the same email conversation: it is stored with the original's thread identity (`thread_id`, `In-Reply-To`), appears in the reading pane's “Conversation · N messages” strip and groups with the original.

Actual:
`reply`/`replyAll`/`forward` only prefill the composer (To/Cc/Subject/Body). The draft written by `save_draft` inserts `id, account_id, folder_id, subject, body_text, received_at, is_read, created_at, updated_at, direction, delivery_state, sync_status` — `thread_id` and `message_id_header` are never set, and the UPDATE branch for an existing draft never sets them either. `email_thread(id)` returns nothing when `thread_id` is empty, so the reply is permanently a standalone row. The thread columns only hold values for the debug seed (`seed.rs` writes `thread_id = 'thread-q3'`), which is why the seeded demo thread looks right while real replies do not.

Root Cause:
The composer's initial state carries no reference to the message being answered and `DraftInput`/`save_draft` have no parameter to inherit `thread_id`, so email threading exists only as a read-side feature (column + query).

Affected Files:
- `src/app/MailView.tsx:82-85` (`reply`, `replyAll`, `forward` → `onCompose({ to, cc, bcc, subject, bodyText })`; no draft id, no thread reference)
- `src/app/Composer.tsx:11-18, 33-41` (`DraftInput` has no thread field)
- `src-tauri/src/models.rs:29` (`DraftInput`)
- `src-tauri/src/repositories.rs:126-134` (`save_draft` INSERT/UPDATE)
- `src-tauri/src/repositories.rs:17-25` (`email_thread` needs a non-empty `thread_id`)
- `src-tauri/src/seed.rs:21` (only the seed populates `thread_id`)

Reproduction:
1. Open an inbound email, click “Reply”, type a line, wait for “Saved locally”, queue the send.
2. Reopen the original: no reply appears in its conversation strip.
3. Open the reply from Drafts/Sent: no related mail is listed, and `SELECT thread_id FROM emails WHERE id = '<reply-id>'` is NULL.

Recommended Fix:
1. Add optional `threadId`/`inReplyToMessageId` to `ComposeInitial`, `DraftInput` and `save_draft`; pass the original's `thread_id` (falling back to its id when empty) and `message_id_header` from `reply`/`replyAll`/`forward`.
2. Write `emails.thread_id` and `emails.message_id_header` in `save_draft` inside the same transaction as the recipient writes.
3. Add an integration test that replies to a stored email and asserts `email_thread(original)` contains the reply.

## BUG-003

Severity: MEDIUM
Module: Synchronization
Feature: Durable sync queue (`sync_queue`) and every mutation that enqueues work

Expected:
Each queued operation carries the data needed to perform it remotely — at minimum the entity's current values or a change set — so a transport can deliver the change and a retry can replay it.

Actual:
`enqueue()` writes a literal empty payload for every operation (`payload` column = `'{}'`), recording only `entity_type`, `entity_id`, `operation` and timestamps. Consequences: (1) a future transport cannot send what changed without re-reading and re-deriving it from the local tables (and cannot detect a local row that was hard-deleted); (2) `attempt_count` is never incremented and `payload` never updated, so the queue is a *list of intents*, not a replayable log; (3) dedupe via `UNIQUE(entity_type, entity_id, operation)` + `ON CONFLICT DO UPDATE` collapses repeated edits correctly but also erases any record of what the local state was when the row was first queued.

Root Cause:
The queue was implemented as an intent registry (`enqueue(entity_type, entity_id, operation)`) rather than an outbox, so no payload is ever serialized.

Affected Files:
- `src-tauri/src/repositories.rs:384-388` (`enqueue`, `'{}'` payload)
- `src-tauri/src/sync.rs:16-23` (`overview`/`retry_failed`; nothing ever consumes or fills the queue)
- callers: `repositories.rs` (`send_message`, `edit_message`, `delete_message`, `set_message_pin`, `toggle_message_reaction`, `save_draft`, `queue_send`, `move_email_to_folder`, `add_account_verified`, `set_account_connection_state`, `remove_account`, `create_contact`, `set_contact_favorite`, `open_direct_message`)

Reproduction:
1. Send a chat message or save a draft in the desktop app.
2. `SELECT entity_type, entity_id, operation, payload FROM sync_queue;` → `payload` is `{}` for every row, `attempt_count` is 0.
3. There is no code path that reads `payload` (no transport exists), so the row can never be delivered as written.

Recommended Fix:
1. Serialize the entity snapshot (or a JSON-Patch style change set) into `payload` at enqueue time; keep the row's `sync_version` so a transport can detect divergence.
2. Increment `attempt_count`, record `last_error`/`next_attempt_at`, and move rows to `completed`/`failed` in the transport worker (the plumbing exists in `sync_queue`).
3. For deletions, enqueue the tombstone payload *before* the row stops being readable so the transport can send an identifier-only delete.

## BUG-004

Severity: MEDIUM
Module: Tauri IPC / accounts
Feature: “Test connection” (Settings → Accounts) and Google OAuth re-verification

Expected:
A long-running network check does not block unrelated work: while the server is being contacted, navigation, search and mail browsing keep working, or the UI clearly reports that it is busy.

Actual:
`test_email_connection_inner` takes the application-state mutex on its first line (`let database = repositories(&state)?`) and keeps the guard for the whole function, through `load_oauth_secret` → `oauth::refresh_access_token` (HTTPS) and `verify_imap_login`/`verify_xoauth2` (TCP connect with a 5 s probe, DNS resolution) — including `AUTH_TIMEOUT`-bounded waits in the OAuth path. Every one of the 49 commands locks the same `Mutex<Database>` (`lib.rs:23`), so during the check every other IPC call blocks: view switches appear frozen, search does nothing, and the window does not repaint results until the network call returns. `add_email_account` avoids the problem by verifying *before* locking (good pattern, `commands.rs:60-65`), which shows the fix is local.

Root Cause:
The database guard is acquired before the network work instead of after it, and the state is a single global mutex with no per-command granularity.

Affected Files:
- `src-tauri/src/commands.rs:126-169` (`test_email_connection_inner`: guard at :132, network at :137 and :156)
- `src-tauri/src/lib.rs:22-23` (`AppState { database: Mutex<Database> }`)
- `src-tauri/src/commands.rs:8` (`repositories()` helper locking the mutex)
- `src-tauri/src/commands.rs:177-233` (OAuth sign-in takes the guard twice; the second window is bounded, but the flow still occupies the IPC thread)

Reproduction:
1. Settings → Accounts → “Test connection” for an account whose host is unreachable (e.g. temporarily edit the account to `imap.example.invalid`).
2. While the spinner runs, click Inbox/Starred, open the command palette, type a search.
3. The clicks queue up; the UI only reacts after the network call fails (seconds later).

Recommended Fix (implemented):
1. `test_email_connection_inner` now reads the connection data with a short-lived guard, drops it, performs the network I/O, then takes the guard again to persist the result — mirroring `add_email_account_inner`. Every other command can proceed during the check.
2. `google_oauth_sign_in` was made `async` and now runs its blocking browser half (loopback wait, PKCE exchange, XOAUTH2 proof) on a `relay-oauth` worker thread awaiting a `tokio::sync::oneshot` — the pattern already used by `auto_sign_in` — so the webview's IPC thread stays free for the whole (human-paced, up to 300 s) round-trip. It also takes the database guard only in two short sections.
3. The single global `Mutex<Database>` itself is unchanged; splitting it into per-domain guards remains an option if the lock is ever held long-term again.

Regression note: the timing behaviour is not directly unit-testable (it needs a slow server and a live IPC thread); the fix is verified by inspection of the lock scopes and by the suite still passing (`cargo test` → 34/34).

## BUG-005

Severity: MEDIUM
Module: Settings / appearance
Feature: Command-palette command “Toggle Dark Mode” (Ctrl+K)

Expected:
The theme choice is written to the local settings store, exactly like the Settings → Appearance segmented control, so the window keeps the chosen theme after a restart.

Actual:
`toggleTheme` only flips React state (`setTheme(current => current === 'dark' ? 'light' : 'dark')`); it never calls `setAppSetting('theme', …)`. The document root is updated from state (`data-theme` effect), so the toggle *looks* like it works in the session, but on the next launch `getAppSettings()` restores the previously persisted value and the toggle is silently undone. `SettingsView.applySetting` is the only persisting path, and `App.persistWidth` is the only other `setAppSetting` caller, confirming the palette path is missing its write.

Root Cause:
The palette command and the Settings control are two independent code paths; only the Settings path persists, and `App` owns the theme state for both.

Affected Files:
- `src/app/App.tsx:101` (`toggleTheme` — state only)
- `src/app/App.tsx:124-128` (`onPreferenceChange` — updates state, persistence happens in the child)
- `src/app/SettingsView.tsx:61-65` (`applySetting` — state + `setAppSetting`)
- `src/app/SearchPalette.tsx:50` (`{ id: 'theme', … run: () => { onClose(); onToggleTheme(); } }`)

Reproduction:
1. In Settings → Appearance choose “Light”; restart to confirm it persists.
2. Press Ctrl+K → “Toggle Dark Mode” (the UI turns dark).
3. Close and reopen the app: the theme is Light again — the toggle was never stored (`SELECT value FROM settings WHERE key = 'theme'` still says `light`).

Recommended Fix:
Route both paths through one function (e.g. `applyTheme(value)` that sets state and awaits `setAppSetting('theme', value)`), and have `toggleTheme` call it; ignore-and-report the error like the other preference writes.

## BUG-006

Severity: MEDIUM
Module: Frontend shell
Feature: Application resilience to render-time errors

Expected:
A component throwing during render (malformed stored data, an unexpected `null`, an `undefined` array) shows a recoverable error surface — at minimum “something went wrong” plus a way to reload — and leaves the rest of the app usable.

Actual:
There is no error boundary anywhere (`ErrorBoundary` matches: 0) and `main.tsx` renders `<App />` directly into `#root`. React 18 unmounts the whole tree when a render throws, so the result is a blank window with no message, no reload affordance and no log surface for the user; the only recovery is restarting the process. The Rust side is hardened against exactly this class of failure (`guard.rs` exists because an IPC panic would abort the process), so the frontend is the weaker half.

Root Cause:
Missing top-level (and per-view) error boundaries.

Affected Files:
- `src/main.tsx:15` (`createRoot(...).render(<StrictMode><App /></StrictMode>)`)
- `src/app/App.tsx:148-198` (single tree for the whole authenticated shell)
- `src/app/MailView.tsx:162` (`{detail.bodyText.split('\n')…}` — an unexpected `null` body would throw here, one of several such assumptions)

Reproduction:
1. Corrupt one stored field the UI assumes exists, e.g. `UPDATE emails SET subject = NULL WHERE id = 'dev-email-0';` while `subject TEXT NOT NULL DEFAULT ''` is bypassed by `PRAGMA ignore_check_constraints` or a manual row edit with the constraint temporarily dropped.
2. Select that email in the list.
3. The window goes blank; no error text, no recovery, and the console message is the only trace.

Recommended Fix:
1. Add an `ErrorBoundary` around `<App/>` (and around each workspace view) that renders the existing `.empty`/`.load-error` styling plus a “Reload” button.
2. Report the failure through the same `[relay] …` console contract already used by every catch block so it is diagnosable.

## BUG-007

Severity: MEDIUM
Module: Composer / data safety
Feature: Draft autosave while typing (composer), including closing the composer or the window

Expected:
Whatever the user typed is either saved or explicitly discarded; closing the composer flushes the pending autosave.

Actual:
`update()` debounces persistence by 700 ms; the effect cleanup only clears the timer (`return () => { if (timer.current) window.clearTimeout(timer.current); };`) without calling `persist()`. Closing the composer (X / backdrop / Escape) or the application window inside that window discards the last edits silently — no warning, no “unsaved changes” prompt. `send()` is safe because it awaits `persist()` first, and a resumed draft keeps its earlier content, so only the newest keystrokes are lost. The same pattern exists in `SearchPalette` (harmless there) and `App.persistWidth` (pane width only).

Root Cause:
The debounce is treated as a pure timer with no flush-on-teardown, and there is no `beforeunload`/Tauri close-request hook.

Affected Files:
- `src/app/Composer.tsx:19-23` (cleanup clears the timer without persisting)
- `src/app/Composer.tsx:26-41` (`update`/`persist` debounce)
- `src/app/App.tsx:184` (`{composer.open && <Composer onClose={closeCompose} … />}` — unmounts without a flush)

Reproduction:
1. Open the composer, choose an account, type a sentence.
2. Immediately (within 700 ms) click the composer's X.
3. Reopen the composer / check Drafts: the sentence is missing; the previously autosaved version is what remains.

Recommended Fix:
1. In the cleanup (and in `onClose`), if a timer is pending, call `void persist(currentForm)` before unmounting; keep the timer id in a ref that the latest form is read from.
2. Optionally flush on window close via Tauri's close-request event, and add an “unsaved changes” confirmation when the flush fails.

## BUG-008

Severity: MEDIUM
Module: Command palette / synchronization
Feature: “Sync Now” command (also surfaced in the help popover's shortcut list)

Expected:
A command labelled “Sync Now” either performs a synchronization (pushes the queue, reports progress and outcome) or is not offered at all while no transport exists.

Actual:
`syncNow` calls `refreshSync()`, which is `getSyncOverview()` — a read-only status query. Nothing is pushed, no queue item is touched, and the command is indistinguishable from doing nothing except for the refreshed counters. The status bar simultaneously explains “No remote transport is configured. Local changes are safely queued.”, so the copy is honest about the missing transport while the command label promises the opposite.

Root Cause:
The command was wired to the status refresh as a placeholder for the future transport.

Affected Files:
- `src/app/App.tsx:99-100` (`refreshSync`/`syncNow`)
- `src/app/App.tsx:197` (`SearchPalette … onSyncNow={syncNow}`)
- `src/app/SearchPalette.tsx:49` (command definition “Sync Now”)
- `src-tauri/src/sync.rs:16-23` (a transport is required before a real sync can exist)

Reproduction:
1. Make a local change (send a chat message) → status bar shows “1 queued”.
2. Ctrl+K → “Sync Now”.
3. The counter is unchanged and no network activity occurs (`sync_queue.status` stays `pending`); the only visible effect is a re-render of counters.

Recommended Fix:
Either disable/hide the command while `sync.detail` reports no transport (with a tooltip), or label it “Refresh sync status” — and keep the real sync command for when `RemoteSyncTransport::is_configured()` can be true.

## BUG-009

Severity: MEDIUM
Module: Email / multi-account
Feature: Folder views (Inbox, Starred, Sent, Drafts, Archive, Trash) with more than one signed-in account

Expected:
Each folder view shows mail for the selected account, or explicitly merges accounts with a visible account label per row (Thunderbird's “unified folders” pattern with account identification).

Actual:
`emails_in_folder` builds `e.folder_id IN (SELECT folder.id FROM email_folders folder WHERE folder.account_id = e.account_id AND folder.role = '<role>' AND folder.deleted_at IS NULL)`. The correlated subquery matches *every* account's folder with that role, and `list_emails` has no account predicate, so all accounts' mail appears in one list ordered purely by date. Rows carry no account identity in `EmailSummary` (`id, senderName, subject, preview, receivedAt, isRead, isStarred`), so two accounts' mail is indistinguishable in the list; `starred()` shares the same unfiltered query. The composer does let the user choose the sending account, which makes the mixing reachable rather than theoretical.

Root Cause:
Role-scoped queries were written before multi-account sign-in existed; `EmailSummary` has no account field to group by and no UI filter was added.

Affected Files:
- `src-tauri/src/repositories.rs:16` (`emails_in_folder`, role-only filter)
- `src-tauri/src/repositories.rs:368-373` (`list_emails`, no account predicate, `LIMIT 200`)
- `src-tauri/src/models.rs:5` (`EmailSummary` — no account fields)
- `src/app/MailView.tsx:35-42` (view load; no account selector)
- `src/app/App.tsx:49, 159` (multiple accounts are supported and shown in the header avatar)

Reproduction:
1. Sign in a second account (or seed a second account row with its own folders and one email).
2. Open Inbox: messages from both accounts are listed interleaved by date, with no way to tell them apart or filter to one.
3. `SELECT e.id, a.email_address FROM emails e JOIN accounts a ON a.id = e.account_id WHERE e.deleted_at IS NULL ORDER BY e.received_at DESC;` shows the mixed set that the view renders.

Recommended Fix:
1. Add `accountId`/`accountLabel` to `EmailSummary` and an account filter (or a unified-view toggle) in the mail workspace; default to the first account.
2. Pass an optional `accountId` through `get_emails_in_folder`/`get_starred` into `list_emails`'s filter and index usage (`emails(folder_id, received_at)` already supports it).

## BUG-010

Severity: LOW
Module: Search
Feature: Global search (command palette / FTS5 `MATCH`)

Expected:
Any user input either returns results or an understandable “no matches” state; FTS5 syntax characters (`"`, `*`, `-`, `(`, `)`, `:`, `^`, `NEAR`, `AND`/`OR`) are escaped or explained.

Actual:
`search()` trims, rejects empty and >200 chars, then builds the match expression as `format!("{}*", terms[0])` for a single token and `format!("\"{}\"", trimmed.replace('"', "\"\""))` for several. The value is bound as a parameter (so there is no SQL injection), but FTS5 parses it as *query language*: input such as `-`, `*`, `"`, `(`, `AND`, `mail:` raises `fts5: syntax error near …`, the repository returns `AppError::Validation`/`Database`, and the palette replaces its results with the generic “Search could not be completed.” The user gets no results and no hint that the characters were the problem, and any search containing a hyphenated word inside a multi-word query (`"well-known issue"`) can fail the same way.

Root Cause:
The FTS5 query string is assembled without escaping or quoting per token.

Affected Files:
- `src-tauri/src/repositories.rs:327-332` (match-expression construction)
- `src/app/SearchPalette.tsx:34-41` (debounced call; generic error message only)

Reproduction:
1. Ctrl+K, type `-` (or `*`, or `AND`).
2. The list shows “Search could not be completed.” instead of results or “no matches”.

Recommended Fix:
Quote every token (`"…"` with internal quote doubling) and append `*` *inside* the quotes for prefix matching, or reject/sanitise FTS5 control characters and surface “Search ignores special characters like - or *” instead of a generic error.

## BUG-011

Severity: LOW
Module: Notifications / preview parity
Feature: Notification preference defaults

Expected:
The browser preview backend mirrors the native defaults so a preference reads the same in both environments.

Actual:
Migration 0006 seeds `notification_preferences` with `('sounds', 1, …)` (enabled) while the preview store seeds `{ key: 'sounds', enabled: false }`. The same “Sounds” switch therefore starts ON in the desktop app and OFF in `npm run dev`, and flipping it in one environment says nothing about the other.

Root Cause:
Two independent seed lists for the same domain; the preview is maintained by hand.

Affected Files:
- `src-tauri/migrations/0006_notifications.sql:15-19` (native defaults, `sounds` = 1)
- `src/platform/preview.ts:74-77` (preview defaults, `sounds` = false)
- `src/app/SettingsView.tsx:152-159` (renders whatever the store returns)

Reproduction:
1. `npm run dev` (browser preview) → Settings → Notifications: “Sounds” is off.
2. Run the desktop app against a fresh database → Settings → Notifications: “Sounds” is on.

Recommended Fix:
Pick one default (recommend OFF: nothing consumes the flag yet) and align both lists, ideally by exporting the native default list to a single shared constant or by asserting parity in a test.

## BUG-012

Severity: LOW
Module: Messenger / synchronization
Feature: Deleting a thread root (cascade soft-delete) and the queue entries it produces

Expected:
Whatever changed locally is represented in the durable queue so a future transport can reproduce the same end state remotely.

Actual:
`delete_message` soft-deletes the root *and* every reply of its thread in one statement, then enqueues exactly one item (`message/<root-id>/delete`). The reply rows change state locally with no queue entry of their own, so a transport that respects the queue would delete only the root remotely and leave the replies orphaned on the server (or, if the server cascades on its own, produce a divergent history).

Root Cause:
The cascade was added on the local read path without extending the outbox contract.

Affected Files:
- `src-tauri/src/repositories.rs:230-240` (`delete_message`, cascading UPDATE + single `enqueue`)
- `src-tauri/src/repositories.rs:384-388` (`enqueue`)

Reproduction:
1. Delete a seeded thread root in `dev-it-support` (5 replies).
2. `SELECT COUNT(*) FROM messages WHERE deleted_at IS NOT NULL;` → 6; `SELECT COUNT(*) FROM sync_queue WHERE operation = 'delete';` → 1.

Recommended Fix:
Enqueue one delete per affected entity (or a single composite payload carrying the affected ids) — see BUG-003, which should be fixed together with this.

## BUG-013

Severity: LOW
Module: Notifications
Feature: Unread badge and per-item read state

Expected:
The badge reflects reality within a moment of the user's action, and “read” is a per-notification state the user can see.

Actual:
`unreadCount` is fetched by an effect whose only dependency is `notificationsOpen`, so the badge changes only when the panel is opened or closed. “Mark all read” empties the panel optimistically and the badge stays stale until the panel is closed. There is no per-notification mark-read action and no read/unread styling: `NotificationPanel` renders title/body/time only, so once reopened, read and unread rows are visually identical.

Root Cause:
The badge is derived from a single effect keyed on panel visibility instead of a refresh trigger tied to the mutating action.

Affected Files:
- `src/app/App.tsx:64` (`useEffect(… getUnreadNotificationCount() …, [notificationsOpen])`)
- `src/app/NotificationPanel.tsx:8` (`markRead` clears local state only)
- `src-tauri/src/repositories.rs:219-220` (`unread_notification_count`, `mark_notifications_read` exist and work)

Reproduction:
1. Open the bell panel → badge shows 2.
2. Click “Mark all read” (list empties, DB rows now `is_read = 1`).
3. The badge still shows 2 until the panel is closed and reopened; reopening the panel lists the same rows with no visual difference between read and unread.

Recommended Fix:
Have the panel notify the shell after a mutation (callback or shared state) and refresh the count there; add `isRead` to the `Notification` model so the panel can style rows and offer per-row read.

## BUG-014

Severity: LOW
Module: All list queries
Feature: Folder lists, channel history, thread replies, contacts, notifications

Expected:
A list either shows everything or tells the user that more exists and how to reach it (paging, “load more”, or the size of the remainder).

Actual:
Every list query is hard-capped with no paging affordance and no “showing N” indicator: folder views `LIMIT 200` (while `MailView` labels the truncated count as if it were the total), channel history `LIMIT 250`, thread replies `LIMIT 100`, contacts `LIMIT 500`, pending/unread lists `LIMIT 100`. Older mail, deeper history and larger directories become unreachable, and because the caps exceed the debug seed the situation is invisible in development.

Root Cause:
Caps were added as a safety bound without any paging contract in models, commands or UI.

Affected Files:
- `src-tauri/src/repositories.rs:369` (`list_emails` … `LIMIT 200`)
- `src-tauri/src/repositories.rs:160` (`channel_messages` … `LIMIT 250`)
- `src-tauri/src/repositories.rs:277` (`message_thread` … `LIMIT 100`)
- `src-tauri/src/repositories.rs:97` (`contacts` … `LIMIT 500`)
- `src-tauri/src/repositories.rs:215` (`notifications` … `LIMIT 100`)
- `src/app/MailView.tsx:91-92` (“{items.length} items” presented as the folder total)

Reproduction:
1. Insert 300 emails into one folder (or 600 contacts).
2. Open that folder/Contacts: only 200/500 rows are listed, the header still reads “200 items” as if complete, and the UI offers no way to reach the rest.

Recommended Fix:
1. Add `(limit, offset)`/keyset pagination to the affected queries plus a “Load more” control (or a virtualised list) in the views.
2. Where paging is not implemented yet, return the total count with the page so the UI can say “showing 200 of 1 437”.

## BUG-015

Severity: LOW
Module: Status bar / health
Feature: Connection status indicator (“Connected / Offline”) and the `get_app_health` command

Expected:
The indicator tells the truth about the current sync/network situation and updates when that situation changes.

Actual:
Two stubs combine into a permanently misleading label: `get_app_health` returns a hard-coded `{ state: "offline", last_sync_at: null }`, and `SyncEngine::overview()` reports `offline` whenever `DisabledTransport::is_configured()` is false — which is always true today. The status bar prefers `sync?.state ?? health.state`, so it always renders “Offline”, even on a machine with working internet, and no `online`/`offline` listener exists (the frontend registers exactly two `addEventListener` calls: the global keydown handler and a `matchMedia` listener). `navigator.onLine` is read once for the initial state and never again.

Root Cause:
The disabled transport doubles as the connectivity indicator and the health command was never implemented (labelled a benign stub in the earlier audit).

Affected Files:
- `src-tauri/src/commands.rs:13-14` (`get_app_health` — static values)
- `src-tauri/src/sync.rs:16-21` (`overview` returns `offline` for a disabled transport)
- `src/app/App.tsx:41, 54, 137-140, 156, 180-181` (initial `navigator.onLine`, health fallback, label, footer)
- `src/app/App.tsx:86-95` (only the global keydown listener; no online/offline subscription)

Reproduction:
1. Run the desktop app with the network connected → the header shows “Offline”.
2. Disconnect the network → the display does not change, which proves the indicator does not track connectivity.

Recommended Fix:
1. Distinguish “no transport configured” (a configuration fact) from “network unreachable” (a connectivity fact) and label the former explicitly (e.g. “Local only · no transport”).
2. Implement `get_app_health` from a real source (transport state or a reachability probe) and refresh it on `window.addEventListener('online'/'offline')`.

## BUG-016

Severity: LOW
Module: Accounts / session
Feature: Sign-in gate, session lifecycle, “Skip for now”, removing an account

Expected:
The user can tell whether the app is signed in and can leave that state; removing the last account returns the app to the sign-in screen; account-dependent UI (avatar, gate) reflects account changes immediately.

Actual:
There is no sign-out or “switch account” action anywhere, and no session concept beyond “at least one account row exists”. The first-run gate is local component state that is (a) true when any account exists, (b) forced true when the account query fails (fail-open by design) and (c) permanently bypassed by “Skip for now”, whose flag is never persisted and never re-evaluated. The shell's `accounts` array is fetched only at boot and after a successful sign-in, so removing an account in Settings updates only `SettingsView`'s own list: the header avatar keeps showing the removed account and the gate state stays stale until restart. Credential handling itself is correct — `remove_account` purges both the password and OAuth keyring entries (`commands.rs:309-318`) — only the UI/session state is inconsistent.

Root Cause:
Session state lives in two unsynchronised places (App state and the accounts table) with no shared store or refetch after mutations, and no teardown path was implemented.

Affected Files:
- `src/app/App.tsx:44-45, 57-61, 146` (`signedIn`/`skipLogin` gate, fail-open catch)
- `src/app/LoginView.tsx:177` (“Skip for now — explore the workspace”)
- `src/app/SettingsView.tsx:108-111` (`dropAccount` updates the local list only)
- `src-tauri/src/commands.rs:309-318`, `src-tauri/src/repositories.rs:347-351` (remove + credential purge)

Reproduction:
1. Sign in one account; Settings → Accounts → “Remove” (confirm).
2. The Settings list updates, but the top-right avatar still shows the removed account and the app keeps behaving as signed in.
3. Restart: the sign-in screen appears, proving the previous state was stale, not merely latent.

Recommended Fix:
1. Add an explicit “Sign out” (clears session state; optionally purges the credential after confirmation) and a “switch account” affordance.
2. Lift accounts into shared state or refetch them after every account mutation so the shell, gate and Settings agree.
3. Show the fail-open case as a warning instead of silently presenting a signed-in workspace.

## BUG-017

Severity: LOW
Module: Navigation
Feature: Sidebar navigation and the `AppView` union

Expected:
Every navigation entry leads to a distinct destination that exists; the type union matches what is actually rendered.

Actual:
“Threads” and “Channels” are both mapped to `Messages` (`onSelect={label => navigate(label === 'Threads' || label === 'Channels' ? 'Messages' : label)}`), so two of the three MESSENGER entries are aliases of the third and the active state highlights “Messages” while the user believes they opened Threads or Channels. `AppView` still declares `'Threads' | 'Channels' | 'Directory'`, and `App` renders nothing for those values, so any future code path that navigates to them would produce a blank content area rather than an error.

Root Cause:
The union and sidebar were written against the planned navigation chart; the views were consolidated into the messenger workspace without pruning the chart.

Affected Files:
- `src/app/App.tsx:19-21` (`MESSENGER_VIEWS` incl. Threads/Channels)
- `src/app/App.tsx:165` (aliasing to `Messages`)
- `src/app/App.tsx:174-178` (render switch: only Mail/Messages/Contacts/Settings exist)
- `src/app/MailView.tsx:7` (`AppView` union with unused members)

Reproduction:
1. Click “Threads”, then “Channels”, then “Messages”: the same screen appears each time and the same nav item is highlighted.
2. Navigating programmatically to `'Directory'` or `'Threads'` renders `null` (empty content region).

Recommended Fix:
Either implement the dedicated views (threads list, channels list, directory) or remove the entries and the unused union members so the navigation chart matches the product.

## BUG-018

Severity: LOW
Module: Mail reading pane
Feature: Selecting the next message after archive/trash

Expected:
After archiving or trashing the open message, the reading pane selects the neighbouring row in the current (order-stable) list, or closes cleanly.

Actual:
`removeFromList` removes the row and then chooses `items[Math.max(0, index - 1)]` from the *pre-removal* array closure. Because removal happens from the same stale array, index arithmetic drifts by one whenever the removed row is not the last one, so the pane can jump to the wrong neighbour (skipping one message) and can also select the just-removed id when indices coincide. The same helper is shared by both archive and trash, so both are affected.

Root Cause:
Selection uses a stale snapshot of the list plus positional arithmetic instead of resolving the neighbour after removal.

Affected Files:
- `src/app/MailView.tsx:73-80` (`removeFromList`, `archive`, `trash` guards)

Reproduction:
1. Open Inbox with several messages and select the 3rd.
2. Press “E” (archive) or delete it.
3. The reading pane opens the 1st message instead of the 4th (the next remaining one); repeat to observe skipping.

Recommended Fix:
Compute the neighbour from the post-removal array (`const next = items.filter(item => item.id !== id); pick next[index] ?? next[index - 1]`) or keep an explicit ordered cursor.

## BUG-019

Severity: LOW
Module: Notifications
Feature: Notification history list after “Mark all read”

Expected:
After marking everything read, reopening the panel shows the history with read rows either hidden or visibly marked as read.

Actual:
`notifications()` returns every non-deleted row ordered by `is_read ASC, created_at DESC` and the panel renders whatever it receives, so after “Mark all read” (which empties the client list and sets `is_read = 1` in the DB) reopening the panel shows the same rows again with no read/unread distinction — visually identical to the state before the action. Combined with the seeded rows this makes the feature look like it silently ignored the click.

Root Cause:
The list query has no read filtering/flag exposure and the panel has no per-row state.

Affected Files:
- `src-tauri/src/repositories.rs:214-218` (`notifications`, no `is_read` in the projection, no filter)
- `src-tauri/src/models.rs:47` (`Notification` has no `isRead`)
- `src/app/NotificationPanel.tsx:7-9` (renders list; `markRead` clears state)

Reproduction:
1. Open the bell panel (seeded history: 2 items).
2. Click “Mark all read” → the panel shows “You are all caught up.”
3. Close and reopen the panel → both items are listed again, unchanged.

Recommended Fix:
Add `isRead` to the `Notification` model and dim/annotate read rows (plus optionally hide them behind a “Show read” toggle), so the state change is visible after reopening.

---

## BUG-020

Severity: MEDIUM
Module: Accounts / sign-in (diagnosability)
Feature: IMAP sign-in failure reporting (`verify_imap_login`, `verify_xoauth2`, Google Workspace/Gmail sign-in)

Expected:
When a sign-in fails, the message names the cause the server actually gave, and distinguishes a *credential/policy* refusal from a *configuration* failure (wrong service on the port, proxy, captive portal, dropped socket). No server text is echoed.

Actual:
`refusal_hint()` matched a five-keyword list against the lowercased error string, and **every** login-phase error — including `imap::Error::MissingStatusResponse`, `Unexpected`, `TagMismatch`, `Io`, `ConnectionLost` — was mapped to `VerifyFailure::Auth`. Consequences seen in practice: a Gmail mailbox answered `NO [ALERT] Application-specific password required: …` (2-Step Verification on, normal password used) and `NO [ALERT] Please log in via your web browser: …`, and a Workspace mailbox can answer `NO [ALERT] IMAP access is disabled …` — none of these contain the keywords, so all three produced the same generic “the server refused the sign-in without a specific reason”. An endpoint that answers TLS but never completes an IMAP login produced the *same* message, so a configuration mistake was presented as a rejected password. Additionally the SASL PLAIN retry keyed off `"logindisabled"` inside an error *text*; that token exists only in `CAPABILITY` responses, so the branch could not fire (verified against `imap-3.0.0-alpha.15`: the string appears only in capability parsing/tests). Note also that `imap-proto` 0.16.x recognises only `ALERT`/`PARSE`/capability/UID codes, so `[AUTHENTICATIONFAILED]` and friends arrive as plain text — a code-based classifier is not available with the pinned dependency.

Root Cause:
Two different failure classes were collapsed into one variant, and the classifier depended on a handful of substrings that real providers do not use (Google uses `[ALERT]` text, Microsoft uses “AUTHENTICATE failed.”).

Affected Files:
- `src-tauri/src/verify.rs` (`refusal_hint`, `VerifyFailure`, `verify_imap_login`, `verify_xoauth2`)
- `src-tauri/src/discover.rs` (`discover_and_verify` — must handle the new variant and keep trying candidates)

Reproduction:
1. Enter a Gmail address with 2-Step Verification enabled and the normal account password (no App Password) and sign in.
2. Observed: “The mail server rejected this sign-in (the server refused the sign-in without a specific reason)…” — the App-Password alert is discarded.
3. Point the account at a non-IMAP service on the port (or a proxy that answers) and retry: the identical message appears, because protocol-shaped failures are also reported as refusals.

Recommended Fix (implemented):
1. Added `VerifyFailure::Protocol(&'static str)` with `protocol_shape_hint()`, which recognises the display strings of `MissingStatusResponse`, `Unexpected`, `TagMismatch`, `StartTlsNotAvailable`, `TlsNotConfigured` and dropped connections, and reports them as a configuration problem instead of a refused password.
2. Extended `refusal_hint()` with the real provider answers: Gmail/Workspace `[ALERT]` text (App Password required, web-browser sign-in blocked, IMAP disabled), Microsoft's “authenticate failed”, Dovecot's “invalid characters”, `[PRIVACYREQUIRED]`, `[AUTHORIZATIONFAILED]`, and `invalid username or password` — each with its own fixed, non-sensitive remedy.
3. The SASL PLAIN retry now triggers on the server's advertised `LOGINDISABLED` capability (`imap::types::Capabilities::has_str`), with the text check kept as a secondary signal.
4. Diagnostics (`tracing::warn!`) record host, port, encryption mode and the fixed classification only — never the server's text. `XOAUTH2` failures keep the shared protocol/configuration split, but a *refusal* is classified on its own: Google answers a revoked grant and a wrong App Password with the same code, so `classify_xoauth2_error` reports `VerifyFailure::GoogleAuth(xoauth2_refusal_hint(…))` and the user is told to re-authorize (or that a Workspace administrator disabled IMAP) instead of being sent after an App Password.
5. `discover_and_verify` now treats a protocol-shaped candidate as “wrong host” and continues discovery instead of aborting on it.

Regression tests:
`refusal_and_protocol_classification_cover_real_provider_answers` in `src-tauri/tests/local_store.rs` asserts the three Google alerts, Microsoft's wording, Dovecot's “invalid characters”, privacy/authorization codes, the code-only refusal, the protocol-shape patterns (and that a genuine `[AUTHENTICATIONFAILED]` refusal is *not* treated as protocol-shaped), and that the protocol message never blames the password. The XOAUTH2 additions are pinned by `gmail_refusal_diagnostics_cover_password_and_oauth_paths` (Gmail's older wordings, the revoked-grant/disabled-IMAP/rate-limit verdicts, and that the Google message never mentions an App Password). `cargo test` → 32/32.

---

## BUG-021

Severity: MEDIUM
Module: Mail list (layout / CSS)
Feature: Scrolling the message list in `MailView`

Expected:
A folder holding more conversations than fit on screen scrolls vertically, so every message that was synced can be reached.

Actual:
`.email-list` declared `overflow: auto`, but `.list-pane` was a plain block box with `overflow: hidden`. In block layout the height of `.email-list` is `auto` — that is, its full content height — and `overflow: auto` on a box with no height constraint never produces a scrollbar. The rows past the first screenful were therefore clipped by the parent’s `overflow: hidden` and were unreachable by wheel, scrollbar, keyboard or touch. The fold tracked the viewport height rather than the content, so roughly 7–9 of the newest 50 messages (the sync window) were visible and the rest appeared to have never been downloaded — the same symptom the earlier investigation chased through the sync path. The loading state had the same defect: `<SkeletonRows count={7} />` replaces `.email-list` while fetching, and `.skeleton-list` carried no flex or overflow rule at all. The reading pane was *not* affected and needed no change: `.reading-pane` is a grid item of `.mail-workspace` (`grid-template-rows: minmax(0, 1fr)` inside `.shell { height: 100vh; grid-template-rows: 64px 1fr 28px }`), so it has a definite height and its own `overflow: auto` activates correctly. The messenger view shows the same intent implemented correctly (`.chat { grid-template-rows: auto 1fr auto; min-height: 0 }` bounds `.chat-history`), which is why only the mail list was broken.

Root Cause:
`overflow: auto` was applied to the scroll child without giving it a definite height (or a flex/grid track to fill); the parent then hid the overflow instead of delegating it to the child.

Affected Files:
- `src/styles/mail.css:3` (`.list-pane`)
- `src/styles/mail.css:11` (`.email-list`)
- `src/styles/mail.css:30` (`.skeleton-list`)

Reproduction:
1. Sign in, press **Sync**, and open a folder with more than ~10 conversations (the sync window is the newest 50).
2. Only the first screenful is listed and the list has no scrollbar; the wheel, `Page Down` and `Tab` do nothing because the clipped rows sit in no scrollable box.
3. Resize the window shorter: fewer rows remain visible, confirming the fold follows the viewport instead of the content.
4. While the folder is loading, the seven skeleton rows are clipped the same way on a short window.

Recommended Fix (implemented):
1. `.list-pane` is now a flex column (`display: flex; flex-direction: column; min-height: 0`) so `.pane-heading` (86px) and `.list-tools` (35px) keep their fixed heights and the remaining space is handed to the list; `.mail-sync-note`, when shown during a sync, simply shortens the list.
2. `.email-list` is now `flex: 1 1 auto; min-height: 0; overflow: auto`. The `min-height: 0` matters as much as the `overflow`: a flex item otherwise refuses to shrink below its content size, which reproduces the identical bug through a different mechanism. `-webkit-overflow-scrolling: touch` restores momentum scrolling where it is supported.
3. `.skeleton-list` receives the same `flex: 1 1 auto; min-height: 0; overflow: auto` treatment so the loading state scrolls too.
4. No markup change was needed, and no `overflow: hidden` was removed from `.list-pane` — the list now scrolls *inside* the pane. The `@media (max-width: 800px)` rules are unaffected: `.mail-workspace.show-detail .list-pane { display: none }` still wins on specificity, so the narrow-screen list/detail swap keeps working.

Verification:
CSS-only change, so the suites were re-run to prove nothing else moved: `npx tsc -b` clean, `npx vitest run` 26/26, `npm run build` PASS, and the compiled rule is present in the shipped bundle (`dist/assets/index-*.css` → `.list-pane{display:flex;flex-direction:column;min-height:0;overflow:hidden;…}` and `.email-list{flex:1 1 auto;min-height:0;overflow:auto;…}`). No test asserts on styles, and the fix could not be confirmed against a live mailbox here (no GUI/OAuth available in this environment) — the scrollbar itself still needs an eyeball on a real account.

---

## BUG-022

Severity: MEDIUM
Module: Mail retrieval (`mailbox.rs`) / reading pane
Feature: Opening a message that was downloaded by a sync

Expected:
Opening an inbox message shows its text. A sync downloads each message in full and `text_body_of` decodes the MIME body to plain text, so a stored row carries content as well as its envelope.

Actual:
Every message a sync wrote had `body_text = ''`, so the reading pane showed the label, the subject, the sender and the date and then nothing — and the list preview (`substr(body_text, 1, 120)`) was blank too. `fetch_inbox` requested `(UID FLAGS INTERNALDATE ENVELOPE BODY.PEEK[HEADER] BODY.PEEK[TEXT])` and then assembled the message from `fetch.header()` and `fetch.body()`. In `imap-3.0.0-alpha.15`, `Fetch::body()` matches only `AttributeValue::BodySection { section: None, .. }` — the **unsectioned** `BODY[]` / `RFC822` item — while a sectioned response is keyed `Some(SectionPath::Full(MessageSection::Header))` / `Some(Text)` and read back through `Fetch::header()` / `Fetch::text()`. Because `BODY[]` was never requested, `body()` returned `None` for every message, so the assembled raw message was the header block plus one blank line. `text_body_of` split at that blank line, found an empty body, and returned `""`. The defect was universal and silent: the fetch reported success, the row was written, the subject (from `ENVELOPE`) rendered correctly, and only the content was missing — which is why it read as a rendering fault rather than a retrieval one. It also could not be caught by the existing fixtures, which exercise `text_body_of` with *complete* raw messages; the bug lived one layer up, in which response field was read, while `collect_messages` was private and `Fetch`'s fields are `pub(crate)`, so no integration test could build the input.

Root Cause:
A mismatch between the FETCH item that was requested and the accessor used to read it. `BODY[HEADER]` + `BODY[TEXT]` are equivalent to `BODY[]` only at the protocol level; the crate exposes them through different accessors, and the body accessor returns `None` instead of erroring.

Affected Files:
- `src-tauri/src/mailbox.rs:428` (the FETCH item list — `BODY.PEEK[HEADER] BODY.PEEK[TEXT]`)
- `src-tauri/src/mailbox.rs:444-447` (the assembly that kept only `header()` and never emitted the text section)
- `src/app/MailView.tsx:226` (reading pane — correct, and left as a plain-text renderer)

Reproduction:
1. Sign in with a working IMAP account and press **Sync**.
2. The inbox lists the messages with subjects and senders, so retrieval looks healthy.
3. Open any message: `SELECT body_text FROM emails WHERE id = 'imap-<account>-<uid>'` returns `''`, and the reading pane renders no body — the same for every message, including simple text/plain ones.
4. Confirm the reading path is innocent by writing any text into `body_text` by hand: it renders immediately.

Recommended Fix (implemented):
1. The FETCH now asks for one complete message — `BODY.PEEK[]` (RFC 3501 §6.4.5; `PEEK` still never sets `\Seen`) — held in the `FETCH_FULL_MESSAGE` constant so the item and the reason it must stay unsectioned live together. It transfers the same bytes as the two sections combined and is unambiguously the whole RFC 5322 message.
2. `assemble_raw_message(full, header, text)` builds the raw message from whatever the server answered with: `BODY[]` is used as-is, otherwise header + text are stitched, and the stitch is skipped when the text section already begins with the header (a server that includes it would otherwise have every `Content-Type` parsed twice). Reading all three accessors means either response shape decodes.
3. `fetch_message_body(connection, credential, uid)` downloads a single message by UID, and the new `fetch_email_body` command writes the decoded text back — the repair path for rows already stored empty, which a `FETCH_CAP`-window sync would never revisit. It uses the same worker-thread/one-shot pattern as `sync_mail`, and `store_email_body` deliberately does **not** touch `sync_status` or enqueue: a body read from IMAP is remote truth, not a local edit.
4. `MailView` calls it when an inbound message opens with an empty body, then shows the downloaded text and patches the list preview. A message that genuinely has no readable text now says so (`This message has no readable content.`) instead of leaving a blank column. The request is made once per message per session, and a failed attempt (a dropped connection, for example) is allowed to retry on the next open.
5. `collect_messages` logs a count when messages arrive with no decoded body — counts only, never content — so a future regression is visible in the log rather than inferred from a blank pane.

Verification:
`cargo test` → 40/40 PASS (was 38); `npx tsc -b` clean; `npx vitest run` → 26/26 PASS; `npm run build` PASS.

Regression tests:
`raw_message_assembly_keeps_the_body` in `src-tauri/tests/local_store.rs` pins both response shapes (a complete `BODY[]` message, and a header+text stitch with and without the header duplicated inside the text) as decoding to `Hello there`, and pins the exact failure mode by asserting that a header on its own decodes to `""`. `stored_email_body_can_be_repaired` stores a row with an empty body — the shape the defect produced — then asserts that `email_fetch_target` resolves the account and UID the per-message fetch needs, that `store_email_body` fills the body without adding a `sync_queue` row, and that an unknown id has no server target.

Verification limit:
The IMAP conversation itself could not be exercised here (no GUI/OAuth and no live mailbox in this environment). The fetch item and the accessor it must match were verified against the pinned dependency sources (`imap-3.0.0-alpha.15/src/types/fetch.rs`, `imap-proto-0.16.7/src/types.rs`), and the decode half is fixture-tested; a real sign-in followed by **Sync** is still what confirms it. Rows already stored with an empty body need one further sync — or simply opening the message, which now repairs it on demand.

## BUG-023

Severity: MEDIUM
Module: Email send (`Composer.tsx`, `App.tsx`, `MailView.tsx`, `commands.rs`, `preview.ts`)
Feature: “Send” in the composer, and reading a message that was sent from Relay

Expected:
Pressing Send either delivers the message or says plainly that it did not, and shows where the message went. A queued send is a local record with `delivery_state = 'queued'` plus a pending `send_smtp` row; no transport consumes that row yet, so the honest outcome is “saved in Sent on this computer, not delivered”.

Actual:
The local send path worked, but it was unreadable — and in one case completely silent:

1. The success text was a developer note: *“Queued for SMTP delivery when a transport is configured. Keep it local for now.”* That was the only feedback the user got.
2. The composer stayed open on top of the unchanged Inbox after a successful queue, so the visible effect of pressing Send was that note. The message had in fact moved to Sent, which the user could not see because the list behind the dialog still showed Inbox.
3. Pressing Send with no sending account selected did nothing at all and said nothing: `persist()` returns `undefined` when `form.accountId` is empty, and `send()` returned on `!id` before touching the status line.
4. Every backend failure was reported as *“Add at least one valid recipient before sending.”* — a storage or folder failure while filing the copy into Sent produced a false instruction about recipients, because the handler collapsed all four `AppError` variants with `map_err(|_| …)`.
5. A queued message was indistinguishable from a delivered one. The reading pane rendered it exactly like received mail even though `delivery_state` was already in the UI, so a user who believed it had been sent and then discarded the draft lost the message in the only sense that matters (`COMPLETE_AUDIT.md` limitation 5 called this “easy to misread”).

Root Cause:
The missing transport is a deliberate scope decision (`COMPLETE_AUDIT.md` §7); the UI was written as if queueing were the end of the flow. The status string explained the gap to a developer rather than to the user, the composer never handed the queued id back to the shell, no surface distinguished `queued` from `sent`, and one failure mode produced no output at all — so the honest state of the data was never shown where the user was looking.

Affected Files:
- `src/app/Composer.tsx:43-47` (the developer-facing success text, and the `catch` that reported every failure as a recipient problem)
- `src/app/Composer.tsx:33-34,44-45` (the silent no-op when no account is selected)
- `src/app/App.tsx:206` (the composer was rendered with no completion callback, so a queued send had nowhere to land)
- `src/app/MailView.tsx:273-280` (the reading pane rendered a queued message exactly like a delivered one)
- `src-tauri/src/commands.rs:492` (`map_err(|_| …)` collapsed `Validation`, `Database`, `Io` and `SecureStore` into one recipient hint)
- `src/platform/preview.ts:229` (the preview backend queued without filing into Sent, so a send disappeared from the folder the UI had just navigated to)

Reproduction:
1. Press **Compose**, fill in a recipient and a subject, press **Queue send**.
2. The composer remains open over the Inbox and the status line shows the developer note; nothing else changes. `SELECT folder_id, delivery_state FROM emails WHERE subject = '<subject>';` shows the account's Sent folder and `queued`, and `sync_queue` holds one pending `send_smtp` row.
3. Clear *Sending account* and press **Queue send** again: no request is made and nothing is reported — the button appears broken.
4. Open the message from **Sent**: nothing on screen says it was never delivered, although the build contains no SMTP client at all (`Cargo.toml` has `imap` and no SMTP crate; `sync.rs` reports permanently offline).

Recommended Fix (implemented):
1. The composer hands the queued id to the shell (`onQueued`), which closes it and opens that message in Sent — `navigate('Sent', id)` bumps `navId`, so `MailView` remounts, reloads from the store and focuses the row. The visible result of Send is now the queued message itself.
2. The reading pane states the delivery state of outbound queued mail: *“Not delivered yet. This message is saved in Sent on this computer. This build has no mail transport, so no copy has left the machine.”* This needed no backend change — `EmailDetail.deliveryState` was already reaching the UI.
3. The developer note is gone. The honest copy lives where the user lands instead of in a status line that disappears with the dialog.
4. An empty account selection is reported (`Choose the account to send from.`) instead of doing nothing.
5. `queue_email_send` matches `AppError` exhaustively: `Validation` keeps the recipient hint (it is the repository's own “not outbound, or no To recipient” answer), and a storage failure says *“The message could not be saved locally, so nothing was queued.”*
6. The composer's save failure uses `commandError`, so the backend's own wording reaches the user instead of a guessed cause.
7. The preview backend mirrors the native contract — recipient validation and the move into Sent — so `npm run dev` no longer loses a queued send.

Verification:
`cargo test --test local_store` → 43/43 PASS (was 40); `npx tsc -b` clean; `npx vitest run` → 28/28 PASS (was 26); `npm run build` PASS.

Regression tests:
`queued_send_records_one_pending_delivery_item` pins the queue contract the UI now navigates on: exactly one `send_smtp` row, still `pending`, visible through `pending_sync_items`, and idempotent across a re-queue. `queue_send_requires_a_to_recipient` pins the failure the composer reports differently: `Err(AppError::Validation)` for a Cc-only draft and for an unknown id, with the message left in Drafts and no delivery item queued. `preview local-only send` in `src/platform/preview.test.ts` pins the same two behaviours in the browser backend (files into Sent, stays `queued`, never `sent`; refuses a send with no To).

Verification limit:
Delivery is still not implemented, so none of this makes mail leave the machine — the `send_smtp` item stays pending by design, and the copy now says so at the point of use. A live SMTP run is out of reach here in any case (no GUI and no credentials in this environment).

---

## BUG-024

Severity: MEDIUM
Module: Mail lists / local store (`repositories.rs`)
Feature: Opening a folder while an outbound message has no To recipient

Expected:
A draft with only Cc/Bcc recipients — or none yet, which is the state of every draft while it is being typed — lists normally, titled with the address it was addressed to (or, failing that, the account identity), and the folder stays openable.

Actual:
`list_emails` built the row title for outbound mail from a scalar subquery over `email_recipients` filtered to `recipient_type = 'to'`, read into a non-nullable column. With no To recipient the subquery returns SQL `NULL`, so `row.get::<_, String>(1)` failed with `InvalidColumnType(1, "CASE WHEN e.direction = 'outbound' THEN (SELECT …)", Null)` — and because the failure is raised while collecting rows, **the whole listing failed**, not just that row. `emails_in_folder("drafts")` returned `Err(Database(…))`, the IPC command answered *“Unable to load local folder”*, and the Mail view rendered its load-error state (`Unable to load your drafts from the local store.`) with no rows and no empty state. Drafts was unusable for as long as such a draft existed, and the draft could not be selected, resumed or trashed from the UI to recover — the autosave that created it was already done.

Root Cause:
The subquery was written for the common case (an outbound message addressed to somebody) and read as non-nullable. `COALESCE` was applied *inside* the subquery, to a recipient that need not exist, instead of around the subquery as a whole.

Affected Files:
- `src-tauri/src/repositories.rs:483` (the `CASE … THEN (SELECT … recipient_type = 'to' …) ELSE …` title expression in `list_emails`)
- `src-tauri/src/repositories.rs:485` (the `row.get(1)?` that surfaced the NULL as a column-type error)
- `src-tauri/src/commands.rs:540` (`get_emails_in_folder` — correct; it reported the failure honestly, which is how the cause was traced)

Reproduction:
1. Start **Compose**, type a Cc address and a subject, and leave *To* empty.
2. Wait for the 700 ms autosave, then check the newest outbound draft: its `email_recipients` rows are all of type `cc`.
3. Open **Drafts** (or restart the app): the list reports *“Unable to load your drafts from the local store.”* while `SELECT COUNT(*) FROM email_recipients WHERE email_id = '<id>' AND recipient_type = 'to';` is `0`.
4. Giving the draft a To recipient makes the folder load again, which confirms the row — not the store — was the trigger.

Recommended Fix (implemented):
1. The title expression now wraps the subquery in `COALESCE` and falls back through Cc/Bcc before the sender identity: prefer `to`, then `cc`, then `bcc` (ordered by kind, then `created_at`), then `sender_name`, `sender_email`, the account display name and finally `'Unknown sender'` — the same chain inbound mail already used.
2. The two SQL fragments are named locals (`addressed_to`, `sender_fallback`), so the intent is visible: a mail client titles a row with the first recipient it can find, and never fails a listing over one.
3. Behaviour is unchanged for everything that already worked: an outbound message with a To recipient still lists that recipient, and inbound mail still lists its sender.

Verification:
`cargo test --test local_store` → 43/43 PASS; `npx tsc -b` clean; `npx vitest run` → 28/28 PASS; `npm run build` PASS.

Regression tests:
`draft_without_a_to_recipient_still_lists` saves a Cc-only draft and a recipient-less draft, then asserts that `emails_in_folder("drafts")` still returns a list containing both (the Cc-only row titled with the Cc address), that the recipient-less row still carries a title, and that the Inbox listing loads without either draft.

Verification limit:
Reproduced and verified against the real schema in a temporary database, which is where it originates; no live account is needed to trigger it, because autosave alone creates the shape.

## Audit notes and limits

- Every bug above was identified by reading the shipped code and, where marked, by a stated reproduction path against the real schema/UI. None of the 19 findings required modifying a source file; the audit itself made no changes except creating this report.
- Bug IDs are stable references for the fix phase; `COMPLETE_AUDIT.md` (functional matrix, production blockers, fix order) and `SECURITY_AUDIT.md` (SEC-001 … SEC-011) are the companion documents.
- Findings that are *not* bugs but unimplemented product surface (IMAP retrieval, SMTP delivery, attachments, channel management, contact editing, mentions/typing, notification producers, company sync, i18n, RBAC) are catalogued in `COMPLETE_AUDIT.md` §7 rather than here.
- Two items deliberately excluded from the bug list because they are honest and documented: the disabled attachment button (with tooltip) and the `send_smtp` queue items that stay pending with no transport (a queued message is now labelled as not delivered where the user reads it — BUG-023).
