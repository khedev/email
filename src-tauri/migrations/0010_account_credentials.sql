-- Account sign-in state (migration 0010).
--
-- Passwords are NEVER stored in this database. They live in the OS credential
-- manager (Windows Credential Manager on Windows), managed by `security.rs`.
-- `credential_ref` records which keyring entry belongs to the account so the
-- secret can be rotated and purged when the account is removed. The IMAP
-- LOGIN that decides `connection_state` is performed by `verify.rs`.
ALTER TABLE accounts ADD COLUMN connection_state TEXT NOT NULL DEFAULT 'unverified';
ALTER TABLE accounts ADD COLUMN connection_error TEXT;
ALTER TABLE accounts ADD COLUMN credential_ref TEXT;
ALTER TABLE accounts ADD COLUMN last_verified_at TEXT;
