# Security

## Credential storage (email sign-in)

Account passwords are verified against the real IMAP server during sign-in (`verify.rs`) and then stored **only** in the operating system credential manager (Windows Credential Manager on Windows), keyed by the account's email address under the `Relay Mail` service (`security.rs`). The SQLite schema stores no password and no secret material — only `credential_ref`, a pointer to the keyring entry — which is verified by an integration test that scans every column of the account row. Passwords are never returned over IPC, never logged, and the keyring entry is purged when the account is removed.

The IMAP transport honors the account's configured encryption: implicit TLS for `ssl`, a STARTTLS upgrade for `tls`, and plaintext only when explicitly chosen (labeled "not recommended" in the UI). Unknown values fall back to TLS-on-993 / STARTTLS auto-negotiation. Servers that disable inline LOGIN are retried with AUTHENTICATE PLAIN — the retry is triggered by the server's advertised `LOGINDISABLED` capability (a reliable signal) rather than by guessing from an error text. Authentication refusals are classified from the server's response *code keywords* and its answer *shape* into fixed, non-sensitive messages — the raw server response is never surfaced or logged, because some servers echo parts of the client command (which contain the credentials). An endpoint that answers but never completes an IMAP login (the wrong service on a port, a proxy or captive portal, a gateway, a dropped socket) is reported as a configuration problem instead of a rejected password. Diagnostics record only the host, port, encryption mode and the fixed classification phrase.

## Google OAuth 2.0 (Gmail sign-in)

The Google flow runs entirely native-side: PKCE S256 with a randomly generated verifier, a `state` parameter checked on the loopback redirect (CSRF protection), and the token exchange over TLS. Only the **refresh token** is persisted — in the OS credential manager under the `Relay Mail OAuth` service, stored as JSON together with the (public) OAuth client id. **Access tokens are short-lived and never stored, logged, or sent over IPC.** Google error bodies are reduced to fixed hints and never echoed raw. The Gmail address is read from the `id_token` issued directly by Google's token endpoint (a trusted local source; not a signature-verification boundary). No password is stored for OAuth accounts, and the IMAP XOAUTH2 proof must succeed before any account row is written.

A failed sign-in stores nothing: no account row, no credential. `test_email_connection` re-verifies against the server and can rotate the stored credential after a successful login; failures are recorded as a `connection_state` of `error` with a short non-sensitive reason.

Credentials are not represented in this schema and must never be persisted by the frontend or in plaintext. Tauri permissions remain narrow, filesystem access is Rust-controlled, and remote HTML must be sanitized before rendering.

Native notifications are limited to validated title/body lengths. Notifications must not include credentials, unredacted sensitive content, or private attachment data. The user-visible local notification history remains subject to normal local-data protections.
