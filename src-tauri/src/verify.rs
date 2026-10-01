//! Email sign-in verification (Phase 3 accounts).
//!
//! A real IMAP LOGIN is attempted against the configured server. The transport
//! honors the account's encryption setting: implicit TLS (`ssl`), a STARTTLS
//! upgrade (`tls`), or plaintext — only when explicitly chosen. A server that
//! refuses inline LOGIN but accepts SASL (the `LOGINDISABLED` case) is retried
//! with AUTHENTICATE PLAIN. Failures are classified from the server's response
//! *code keywords* — raw server text is never surfaced or logged, because some
//! servers echo parts of the command — so the UI can show the right recovery
//! hint instead of a stack trace.

use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Bounded reachability probe (5s) so an unreachable host fails fast with a
/// clear message instead of hanging through the OS TCP timeout inside the IMAP
/// handshake. This is user feedback, not a security check.
pub fn probe_reachable(host: &str, port: u16) -> Result<(), VerifyFailure> {
  let mut last: Option<std::io::Error> = None;
  let addresses = (host, port).to_socket_addrs().map_err(|error| VerifyFailure::Unreachable(format!("cannot resolve {host} ({error})")))?;
  for address in addresses {
    match TcpStream::connect_timeout(&address, Duration::from_secs(5)) {
      Ok(stream) => { drop(stream); return Ok(()); }
      Err(error) => last = Some(error),
    }
  }
  Err(VerifyFailure::Unreachable(match last {
    Some(error) => error.to_string(),
    None => "no addresses were available".into(),
  }))
}

#[derive(Debug)]
pub enum VerifyFailure {
  /// The server was reachable but refused the sign-in. The hint is a fixed,
  /// non-sensitive classification of the server's response (never raw text).
  Auth(&'static str),
  /// The server answered, but not with an IMAP login response: the endpoint
  /// completed a TCP/TLS session and then spoke something that is not an IMAP
  /// LOGIN exchange (wrong service on the port, proxy or captive portal, a
  /// gateway, a dropped socket). This is normally a configuration problem — it
  /// is reported separately so a wrong host/port is not presented as a refused
  /// password.
  Protocol(&'static str),
  /// A Google OAuth (XOAUTH2) refusal: the server rejected the *authorization*
  /// itself (revoked or expired grant, or mail access withdrawn). Reported
  /// separately from a password refusal so the remedy shown is "sign in with
  /// Google again" rather than "create an App Password".
  GoogleAuth(&'static str),
  /// The server could not be reached at all (DNS, TCP, TLS handshake).
  Unreachable(String),
}

/// Classifies a LOGIN/AUTHENTICATE refusal from the server's response codes.
/// Only fixed, non-sensitive messages are produced; the raw response is never
/// echoed because some servers include parts of the client command in it.
///
/// The provider alerts are matched first because they are the answers whose
/// remedy differs from "check the password": Google reports a missing App
/// Password, a blocked password sign-in and a disabled IMAP service as
/// `NO [ALERT] <text>`. Note that `imap-proto` (0.16.x) only understands a
/// handful of bracketed codes (`ALERT`, `PARSE`, capabilities, UID/flag codes),
/// so `[AUTHENTICATIONFAILED]` and friends arrive as plain text — matching
/// text is the only option this dependency offers.
pub fn refusal_hint(lowered_response: &str) -> &'static str {
  if lowered_response.contains("application-specific password") {
    "Google requires an App Password for this mailbox: turn on 2-Step Verification, then create one at myaccount.google.com/apppasswords"
  } else if lowered_response.contains("less secure app") {
    "Google blocked this app because Less secure app access is off for the account: enable 2-Step Verification and sign in with an App Password (or use Sign in with Google)"
  } else if lowered_response.contains("username and password not accepted") {
    "Google did not accept the account password for IMAP (\"Username and Password not accepted\"): enable 2-Step Verification and sign in with an App Password, or use Sign in with Google"
  } else if lowered_response.contains("web browser") {
    "Google blocked the password sign-in for this account: enable 2-Step Verification and use an App Password, or sign in with Google"
  } else if lowered_response.contains("imap access is disabled") || lowered_response.contains("imap is disabled") {
    "IMAP access is disabled for this mailbox or domain: a Google Workspace administrator must enable it (this also blocks Google sign-in)"
  } else if lowered_response.contains("logindisabled") {
    "the server disabled inline LOGIN and refused AUTHENTICATE PLAIN as well"
  } else if lowered_response.contains("invalid characters") {
    "the server rejected the password's characters (some servers refuse passwords containing spaces or special characters)"
  } else if lowered_response.contains("privacyrequired") || lowered_response.contains("privacy required") {
    "the server requires a privacy-protected connection (enable SSL/TLS for this account)"
  } else if lowered_response.contains("authorizationfailed") || lowered_response.contains("not authorized") {
    "this account is not authorized to use IMAP on this server"
  } else if lowered_response.contains("authenticationfailed") || lowered_response.contains("invalid credentials") || lowered_response.contains("authentication failed") || lowered_response.contains("authenticate failed") || lowered_response.contains("login failed") || lowered_response.contains("invalid username or password") {
    "the credentials were refused"
  } else if lowered_response.contains("ratelimit") || lowered_response.contains("rate limit") || lowered_response.contains("too many") {
    "the server is rate-limiting sign-ins for this account"
  } else if lowered_response.contains("unavailable") || lowered_response.contains("temporary") || lowered_response.contains("system error") || lowered_response.contains("server error") {
    "the server is temporarily unable to authenticate"
  } else {
    "the server refused the sign-in without a specific reason"
  }
}

/// Classifies an XOAUTH2 refusal. Google answers a bad/expired/revoked grant
/// with the same `[AUTHENTICATIONFAILED] Invalid credentials` shape it uses for
/// a wrong App Password, so the wording has to come from the *flow*: for OAuth
/// the actionable remedy is to re-authorize (or have the Workspace admin enable
/// IMAP), never to create an App Password.
pub fn xoauth2_refusal_hint(lowered_response: &str) -> &'static str {
  if lowered_response.contains("imap access is disabled") || lowered_response.contains("imap is disabled") {
    "IMAP access is disabled for this mailbox or domain: a Google Workspace administrator must enable IMAP for it"
  } else if lowered_response.contains("invalid credentials")
    || lowered_response.contains("authenticationfailed")
    || lowered_response.contains("authentication failed")
    || lowered_response.contains("authenticate failed")
    || lowered_response.contains("invalid_grant")
    || lowered_response.contains("expired")
    || lowered_response.contains("revoked")
    || lowered_response.contains("login failed") {
    "the Google authorization was refused — it may be expired, revoked, or belong to another account, so Relay's access has to be granted again"
  } else if lowered_response.contains("ratelimit") || lowered_response.contains("rate limit") || lowered_response.contains("too many") {
    "the server is rate-limiting sign-ins for this account"
  } else if lowered_response.contains("unavailable") || lowered_response.contains("temporary") || lowered_response.contains("system error") || lowered_response.contains("server error") {
    "the server is temporarily unable to authenticate"
  } else {
    "the server refused the Google authorization without a specific reason"
  }
}

/// Recognises the IMAP hosts Google operates, where a password sign-in is
/// refused by policy rather than because the password is wrong.
fn is_google_mail_host(host: &str) -> bool {
  let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
  host == "imap.gmail.com" || host.ends_with(".gmail.com") || host.ends_with(".googlemail.com")
}

/// Provider-aware wrapper around `refusal_hint`, used when the host is known.
/// `refusal_hint` stays untouched (its wording is contract-tested); this only
/// refines the verdict the host itself explains: on a Google mail host the
/// generic "the credentials were refused" is accurate but not actionable —
/// Google is refusing *passwords as a class*, not this one password. Still a
/// fixed `&'static str`: the raw server text is never echoed.
pub fn refusal_hint_for(host: &str, lowered_response: &str) -> &'static str {
  let hint = refusal_hint(lowered_response);
  if is_google_mail_host(host)
    && (hint == "the credentials were refused" || hint == "the server refused the sign-in without a specific reason")
  {
    return "Google refused the password for this mailbox: Gmail does not accept account passwords for IMAP — sign in with Google, or create an App Password at myaccount.google.com/apppasswords";
  }
  hint
}

/// Recognises failures whose *shape* says the endpoint never completed an IMAP
/// LOGIN exchange, as opposed to answering it with a refusal. The patterns are
/// the display strings `imap::Error` produces for those variants (verified
/// against the pinned `imap` 3.0.0-alpha.15 source), so this stays a pure
/// function the tests can exercise. Returns `None` for anything that looks like
/// a server verdict.
pub fn protocol_shape_hint(lowered_response: &str) -> Option<&'static str> {
  if lowered_response.contains("missing status response") {
    Some("the server never answered the login command with a status line")
  } else if lowered_response.contains("unexpected response") {
    Some("the server sent a response that is not part of an IMAP login")
  } else if lowered_response.contains("mismatched tag") {
    Some("the server's reply did not match the command that was sent")
  } else if lowered_response.contains("starttls is not available") {
    Some("the server does not offer STARTTLS on this port (use SSL / implicit TLS, usually port 993)")
  } else if lowered_response.contains("tls was requested") {
    Some("TLS was requested but the connection is not encrypted")
  } else if lowered_response.contains("connection lost") || lowered_response.contains("unexpected end of file") || lowered_response.contains("connection reset") {
    Some("the server closed the connection while the login was in progress")
  } else {
    None
  }
}

/// Turns a failed LOGIN/AUTHENTICATE into a user-safe failure. Only the
/// classification is logged — never the server's text, which can echo the
/// command (and therefore the credentials).
fn classify_login_error(error: &imap::Error, host: &str, port: u16, encryption: &str) -> VerifyFailure {
  let lowered = error.to_string().to_lowercase();
  if let Some(hint) = protocol_shape_hint(&lowered) {
    tracing::warn!(host, port, encryption, classification = hint, "imap login did not complete");
    return VerifyFailure::Protocol(hint);
  }
  let hint = refusal_hint_for(host, &lowered);
  tracing::warn!(host, port, encryption, classification = hint, "imap login refused");
  VerifyFailure::Auth(hint)
}

impl VerifyFailure {
  /// Message for the user; never contains credentials or raw server text.
  pub fn user_message(&self) -> String {
    match self {
      Self::Auth(hint) => format!(
        "The mail server rejected this sign-in ({hint}). Large providers no longer accept normal account passwords for IMAP: Gmail, Yahoo and iCloud require an App Password (Gmail: turn on 2-Step Verification, then create one at myaccount.google.com/apppasswords), and Microsoft 365 often disables password sign-in for IMAP entirely. Also check the host, port, and encryption settings."
      ),
      Self::Protocol(hint) => format!(
        "The server answered, but not with an IMAP login response ({hint}). This is usually a configuration problem rather than a rejected password: check that the host, port and encryption point at the mailbox's IMAP service — for Gmail that is imap.gmail.com, port 993, SSL / implicit TLS."
      ),
      Self::Unreachable(detail) => format!("Unable to reach the mail server ({detail}). Check the host, port, and connection."),
      Self::GoogleAuth(hint) => format!(
        "The mail server rejected Google sign-in ({hint}). Start the Google sign-in again and grant Relay access to Gmail. If it keeps failing, remove Relay at myaccount.google.com/permissions first; a Google Workspace administrator can also disable IMAP for the domain, which blocks Google sign-in as well."
      ),
    }
  }
  /// Short reason recorded on the account row for later diagnosis.
  pub fn detail(&self) -> String {
    match self {
      Self::Auth(hint) => format!("The server rejected the sign-in: {hint}"),
      Self::Protocol(hint) => format!("The server did not answer an IMAP login: {hint}"),
      Self::GoogleAuth(hint) => format!("The server rejected the Google authorization: {hint}"),
      Self::Unreachable(detail) => format!("Server unreachable: {detail}"),
    }
  }
}

/// Minimal RFC 4648 standard base64 (with padding). Inlined so the SASL PLAIN
/// support does not pull in a new dependency for a single encode call.
pub fn base64_encode(data: &[u8]) -> String {
  const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
  for chunk in data.chunks(3) {
    let b0 = chunk[0] as u32;
    let b1 = *chunk.get(1).unwrap_or(&0) as u32;
    let b2 = *chunk.get(2).unwrap_or(&0) as u32;
    let triple = (b0 << 16) | (b1 << 8) | b2;
    out.push(ALPHABET[(triple >> 18) as usize & 63] as char);
    out.push(ALPHABET[(triple >> 12) as usize & 63] as char);
    out.push(if chunk.len() > 1 { ALPHABET[(triple >> 6) as usize & 63] as char } else { '=' });
    out.push(if chunk.len() > 2 { ALPHABET[triple as usize & 63] as char } else { '=' });
  }
  out
}

/// SASL PLAIN client response (RFC 4612): `\0address\0password`, base64
/// encoded. Kept pure so the encoding is testable without a server.
pub fn plain_auth_response(address: &str, password: &str) -> String {
  let mut payload = Vec::with_capacity(address.len() + password.len() + 2);
  payload.push(0);
  payload.extend_from_slice(address.as_bytes());
  payload.push(0);
  payload.extend_from_slice(password.as_bytes());
  base64_encode(&payload)
}

struct PlainAuth<'a> { address: &'a str, password: &'a str }
impl imap::Authenticator for PlainAuth<'_> {
  type Response = String;
  fn process(&self, _challenge: &[u8]) -> String { plain_auth_response(self.address, self.password) }
}

/// A credential to authenticate an IMAP session with. Both are short-lived in
/// memory only: the password never leaves the keyring/command boundary and the
/// access token is used for one session.
pub enum MailCredential<'a> {
  Password(&'a str),
  /// XOAUTH2 with a short-lived access token (Google sign-in).
  AccessToken(&'a str),
}

/// Opens an authenticated IMAP session and *keeps* it, so sign-in verification
/// and mail fetch share exactly one login path (and cannot drift apart).
/// Verification wraps this and logs straight out.
pub fn open_mail_session(address: &str, credential: MailCredential<'_>, host: &str, port: u16, encryption: &str) -> Result<imap::Session<imap::Connection>, VerifyFailure> {
  let mut client = connect_imap(host, port, encryption)?;
  match credential {
    MailCredential::Password(password) => {
      // A server that advertises LOGINDISABLED cannot accept inline LOGIN at all.
      // The advertised capability is the reliable signal (the refusal text is
      // not), so go straight to SASL PLAIN instead of provoking a refusal and
      // guessing from its wording.
      if advertises_login_disabled(&mut client) {
        return authenticate_plain(client, address, password, host, port, encryption);
      }
      match client.login(address, password) {
        Ok(session) => Ok(session),
        Err((error, client)) => {
          // Some servers (and several corporate gateways) disable inline LOGIN
          // and only accept SASL mechanisms without advertising it up front. The
          // failed login hands the client back, so retry with AUTHENTICATE PLAIN
          // before giving up.
          if error.to_string().to_lowercase().contains("logindisabled") {
            return authenticate_plain(client, address, password, host, port, encryption);
          }
          Err(classify_login_error(&error, host, port, encryption))
        }
      }
    }
    MailCredential::AccessToken(access_token) => {
      let payload = crate::oauth::xoauth2_payload(address, access_token);
      client.authenticate("XOAUTH2", &Xoauth2Auth { payload }).map_err(|(error, _client)| classify_xoauth2_error(&error, host, port, encryption))
    }
  }
}

/// Performs an actual IMAP LOGIN honoring the account's configured encryption:
/// `ssl` = implicit TLS (usually port 993), `tls` = STARTTLS upgrade, `none` =
/// plaintext (labeled "not recommended" in the UI). Unknown values fall back to
/// the crate's auto-negotiation (TLS on port 993, otherwise STARTTLS when the
/// server offers it). The session is closed immediately; mail synchronization
/// is a later phase built on top of this same handshake.
pub fn verify_imap_login(address: &str, password: &str, host: &str, port: u16, encryption: &str) -> Result<(), VerifyFailure> {
  let mut session = open_mail_session(address, MailCredential::Password(password), host, port, encryption)?;
  let _ = session.logout();
  Ok(())
}

/// True when the server's pre-auth CAPABILITY list forbids inline LOGIN. A
/// failed CAPABILITY read is treated as "no information" so the login is still
/// attempted.
fn advertises_login_disabled(client: &mut imap::Client<imap::Connection>) -> bool {
  client.capabilities().map(|capabilities| capabilities.has_str("LOGINDISABLED")).unwrap_or(false)
}

/// SASL PLAIN fallback used when inline LOGIN is unavailable. Returns the live
/// session, so the fetch path can reuse it.
fn authenticate_plain(client: imap::Client<imap::Connection>, address: &str, password: &str, host: &str, port: u16, encryption: &str) -> Result<imap::Session<imap::Connection>, VerifyFailure> {
  client.authenticate("PLAIN", &PlainAuth { address, password }).map_err(|(error, _client)| classify_login_error(&error, host, port, encryption))
}

pub fn connect_imap(host: &str, port: u16, encryption: &str) -> Result<imap::Client<imap::Connection>, VerifyFailure> {
  probe_reachable(host, port)?;
  let mode = match encryption {
    "ssl" => imap::ConnectionMode::Tls,
    "tls" => imap::ConnectionMode::StartTls,
    "none" => imap::ConnectionMode::Plaintext,
    _ => imap::ConnectionMode::AutoTls,
  };
  imap::ClientBuilder::new(host, port).mode(mode).connect().map_err(|error| VerifyFailure::Unreachable(error.to_string()))
}

struct Xoauth2Auth { payload: String }
impl imap::Authenticator for Xoauth2Auth {
  type Response = String;
  fn process(&self, _challenge: &[u8]) -> String { self.payload.clone() }
}

/// Verifies a Google OAuth grant by logging into IMAP with XOAUTH2 (SASL). The
/// access token is short-lived and used only here; the refresh token never
/// travels through this path. The protocol *shape* check is shared with the
/// password path (a wrong host still looks the same), but a refusal is reported
/// as a Google-authorization verdict: the remedy is to sign in with Google
/// again, not to create an App Password.
pub fn verify_xoauth2(address: &str, access_token: &str, host: &str, port: u16, encryption: &str) -> Result<(), VerifyFailure> {
  let mut session = open_mail_session(address, MailCredential::AccessToken(access_token), host, port, encryption)?;
  let _ = session.logout();
  Ok(())
}

/// Turns a failed XOAUTH2 exchange into a user-safe failure, keeping the
/// password path's configuration/verdict split intact.
fn classify_xoauth2_error(error: &imap::Error, host: &str, port: u16, encryption: &str) -> VerifyFailure {
  let lowered = error.to_string().to_lowercase();
  if let Some(hint) = protocol_shape_hint(&lowered) {
    tracing::warn!(host, port, encryption, classification = hint, "xoauth2 login did not complete");
    return VerifyFailure::Protocol(hint);
  }
  let hint = xoauth2_refusal_hint(&lowered);
  tracing::warn!(host, port, encryption, classification = hint, "xoauth2 login refused");
  VerifyFailure::GoogleAuth(hint)
}

