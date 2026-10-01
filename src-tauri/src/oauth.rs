//! Google OAuth 2.0 (installed-app flow, RFC 8252) for Gmail IMAP access.
//!
//! The whole flow runs native-side:
//! 1. an ephemeral loopback listener is bound and the system browser is opened
//!    with the authorization URL (PKCE S256 + `state`);
//! 2. Google redirects to `http://127.0.0.1:<port>`; the code is validated
//!    against `state` and exchanged for tokens over TLS;
//! 3. the Gmail address comes from the `id_token` (fetched directly from
//!    Google's token endpoint over TLS, so the claim is trusted locally);
//! 4. IMAP access is proven with XOAUTH2 before anything is stored;
//! 5. the refresh token is stored in the OS credential manager as JSON that
//!    also carries the client id, so later refreshes need nothing else.
//!
//! Google requires the user's own OAuth client (type *Web application* with
//! `http://127.0.0.1` as an authorized redirect URI — loopback ports are
//! matched dynamically). Personal clients trigger Google's unverified-app
//! warning for the restricted `https://mail.google.com/` scope; that is
//! Google's policy and cannot be bypassed from the app.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Cooperative cancellation for an in-flight browser authorization.
pub fn new_cancel_flag() -> std::sync::Arc<AtomicBool> { std::sync::Arc::new(AtomicBool::new(false)) }

use crate::verify::base64_encode;

pub const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// `https://mail.google.com/` is the only scope that unlocks IMAP; openid/email
/// provide the account address. Separators are pre-encoded for the query.
pub const GOOGLE_SCOPE: &str = "https%3A%2F%2Fmail.google.com%2F%20openid%20email";

/// Bounded wait for the browser redirect.
pub const AUTH_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
  pub access_token: String,
  #[serde(default)]
  pub refresh_token: Option<String>,
  #[serde(default)]
  pub id_token: Option<String>,
}

#[derive(Debug)]
pub enum OAuthError {
  /// Local or transport failure (bind, browser, network).
  Network(String),
  /// Google answered and refused; the message is a fixed, non-sensitive hint.
  Protocol(String),
}

impl OAuthError {
  pub fn user_message(&self) -> String {
    match self {
      Self::Network(detail) => format!("Google sign-in could not reach the network ({detail}). Check the connection and try again."),
      Self::Protocol(hint) => format!("Google sign-in failed: {hint}"),
    }
  }
}

/// Random `state` value for the authorization round-trip (CSRF protection).
pub fn random_state() -> String { uuid::Uuid::new_v4().to_string() }

/// Minimal base64url (RFC 4648 §5, no padding) for PKCE and JWT payloads.
pub fn base64url_encode(data: &[u8]) -> String {
  const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
  let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
  for chunk in data.chunks(3) {
    let triple = ((chunk[0] as u32) << 16) | ((*chunk.get(1).unwrap_or(&0) as u32) << 8) | (*chunk.get(2).unwrap_or(&0) as u32);
    out.push(ALPHABET[(triple >> 18) as usize & 63] as char);
    out.push(ALPHABET[(triple >> 12) as usize & 63] as char);
    if chunk.len() > 1 { out.push(ALPHABET[(triple >> 6) as usize & 63] as char); }
    if chunk.len() > 2 { out.push(ALPHABET[triple as usize & 63] as char); }
  }
  out
}

/// Minimal base64url decoder (padding tolerated) — enough for JWT payloads.
pub fn base64url_decode(data: &str) -> Option<Vec<u8>> {
  const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
  let mut out = Vec::with_capacity(data.len() * 3 / 4);
  let mut buffer: u32 = 0;
  let mut bits: u32 = 0;
  for symbol in data.bytes() {
    if symbol == b'=' { break; }
    let value = ALPHABET.iter().position(|&candidate| candidate == symbol)? as u32;
    buffer = (buffer << 6) | value;
    bits += 6;
    if bits >= 8 {
      bits -= 8;
      out.push((buffer >> bits) as u8);
    }
  }
  Some(out)
}

/// PKCE S256 pair: a high-entropy verifier (128 hex chars from UUIDv4
/// randomness — inside the 43..=128 allowed range) and its base64url
/// SHA-256 challenge.
pub fn pkce_pair() -> (String, String) {
  let mut verifier = String::with_capacity(128);
  for _ in 0..4 { verifier.push_str(&uuid::Uuid::new_v4().simple().to_string()); }
  let challenge = base64url_encode(&Sha256::digest(verifier.as_bytes()));
  (verifier, challenge)
}

/// SASL XOAUTH2 client response (RFC 7628): `user=<a>\u{1}auth=Bearer <t>\u{1}\u{1}`,
/// standard base64 encoded. Contains the access token; never logged.
pub fn xoauth2_payload(address: &str, access_token: &str) -> String {
  base64_encode(format!("user={address}\u{1}auth=Bearer {access_token}\u{1}\u{1}").as_bytes())
}

/// Builds the Google authorization URL (PKCE S256, offline access, consent so
/// a refresh token is issued on every consent).
pub fn authorization_url(client_id: &str, redirect_uri: &str, challenge: &str, state: &str) -> String {
  format!("{GOOGLE_AUTH_URL}?client_id={client_id}&redirect_uri={redirect_uri}&response_type=code&scope={GOOGLE_SCOPE}&access_type=offline&prompt=consent&state={state}&code_challenge={challenge}&code_challenge_method=S256")
}

/// Settings key that remembers the OAuth client id after a successful sign-in.
/// The id is not a secret; the client secret (if any) lives in the OS keyring.
pub const OAUTH_CLIENT_ID_SETTING: &str = "oauth.client_id";

/// Checks a pasted Google OAuth client id before any browser round-trip.
/// Returns a fixed, non-sensitive reason, or `None` when the value looks usable.
/// The value itself is never included in the message. Wording is mirrored by
/// `oauthClientIdIssue` in the UI — keep the two in step.
pub fn validate_client_id(value: &str) -> Option<&'static str> {
  let trimmed = value.trim();
  if trimmed.is_empty() {
    return Some("Paste the Google OAuth client id first (Cloud Console → APIs & Services → Credentials).");
  }
  // A real client id is about 72 characters; the floor only rejects obvious
  // truncations while still letting a partial paste reach the suffix check below.
  if trimmed.len() < 32 || trimmed.len() > 200 {
    return Some("That does not look like an OAuth client id: the value has the wrong length (they are about 72 characters).");
  }
  if trimmed.chars().any(|character| character.is_whitespace()) {
    return Some("The client id contains whitespace — paste only the Client ID value.");
  }
  if trimmed.starts_with("GOCSPX-") {
    return Some("That is the OAuth client secret. Desktop app clients do not need it — use the Client ID, which ends with .apps.googleusercontent.com.");
  }
  if trimmed.starts_with("AIza") {
    return Some("That is a Google API key. Use the OAuth 2.0 Client ID from APIs & Services → Credentials instead.");
  }
  if !trimmed.ends_with(".apps.googleusercontent.com") {
    return Some("The OAuth client id must end with .apps.googleusercontent.com.");
  }
  None
}

/// Checks an optional client secret. Empty is valid (public / PKCE clients do
/// not have one). A non-empty value must look like a secret, not a client id or
/// an API key, and is never echoed back.
pub fn validate_client_secret(value: &str) -> Option<&'static str> {
  let trimmed = value.trim();
  if trimmed.is_empty() { return None; }
  if trimmed.len() < 16 || trimmed.len() > 256 {
    return Some("That does not look like an OAuth client secret: the value has the wrong length.");
  }
  if trimmed.chars().any(|character| character.is_whitespace() || character.is_control()) {
    return Some("The client secret contains whitespace — paste only the Client secret value.");
  }
  if trimmed.ends_with(".apps.googleusercontent.com") {
    return Some("That is the OAuth client id, not the client secret. Put it in the Client ID field.");
  }
  if trimmed.starts_with("AIza") {
    return Some("That is a Google API key, not an OAuth client secret.");
  }
  None
}

/// What arrived at the loopback redirect. Google answers a successful consent
/// with `?code=…&state=…` and a refusal (user cancelled, mail permission
/// denied, client not permitted) with `?error=…&state=…` and **no code**. The
/// two must be told apart: treating the second as "nothing arrived yet" is what
/// made a cancellation sit out the whole 300 s timeout and then report a
/// timeout instead of a cancellation.
#[derive(Debug, Clone, PartialEq)]
pub enum LoopbackCallback {
  /// `?code=…&state=…` — a grant to exchange.
  Code { code: String, state: String },
  /// `?error=…&state=…` — a final refusal; no code will follow.
  Refusal { error: String, state: String },
  /// Anything else (favicon probes, browsers reconnecting, a malformed target).
  Unknown,
}

/// Parses a loopback GET request into the callback it carries, percent-decoding
/// the values (Google authorization codes contain `/`, arriving as `%2F`).
pub fn parse_loopback_callback(request: &str) -> LoopbackCallback {
  let line = match request.lines().next() { Some(line) => line, None => return LoopbackCallback::Unknown };
  let target = match line.split_whitespace().nth(1) { Some(target) => target, None => return LoopbackCallback::Unknown };
  let query = match target.splitn(2, '?').nth(1) { Some(query) => query, None => return LoopbackCallback::Unknown };
  let mut code: Option<String> = None;
  let mut state: Option<String> = None;
  let mut error: Option<String> = None;
  for pair in query.split('&') {
    if let Some((key, value)) = pair.split_once('=') {
      match key {
        "code" => code = Some(percent_decode(value)),
        "state" => state = Some(percent_decode(value)),
        "error" => error = Some(percent_decode(value)),
        // Only used when the `error` code itself was absent.
        "error_description" => { if error.is_none() { error = Some(percent_decode(value)); } }
        _ => {}
      }
    }
  }
  match (code, error, state) {
    (Some(code), _, Some(state)) => LoopbackCallback::Code { code, state },
    (None, Some(error), Some(state)) => LoopbackCallback::Refusal { error, state },
    _ => LoopbackCallback::Unknown,
  }
}

/// Classifies Google's redirect refusal. Fixed, non-sensitive strings only: the
/// value arrived over a loopback request that any local process could send, so
/// it is never echoed back or logged.
pub fn loopback_refusal_hint(error: &str) -> String {
  let lowered = error.to_ascii_lowercase();
  if lowered.contains("access_denied") || lowered.contains("denied") || lowered.contains("cancel") {
    "Google sign-in was cancelled, or this account did not grant Relay access to Gmail. Start again and choose Allow.".into()
  } else if lowered.contains("invalid_scope") || lowered.contains("scope") {
    "Google refused the requested mail permission — enable the Gmail API for this Cloud project and consent to mail access.".into()
  } else if lowered.contains("unauthorized_client") || lowered.contains("invalid_client") {
    "Google rejected this OAuth client — check its type (Web application or Desktop app) and that the Gmail API is enabled.".into()
  } else if lowered.contains("redirect_uri") {
    "Google rejected the redirect — the OAuth client must list http://127.0.0.1 as an authorized redirect URI.".into()
  } else {
    "Google did not authorize Relay (no authorization code was returned). Start the sign-in again.".into()
  }
}

/// Extracts (code, state) from a loopback GET request line. Kept as the narrow
/// accessor over `parse_loopback_callback` for callers that only accept a grant.
pub fn parse_loopback_query(request: &str) -> Option<(String, String)> {
  match parse_loopback_callback(request) {
    LoopbackCallback::Code { code, state } => Some((code, state)),
    _ => None,
  }
}

fn percent_decode(value: &str) -> String {
  let bytes = value.as_bytes();
  let mut out = Vec::with_capacity(bytes.len());
  let mut index = 0;
  while index < bytes.len() {
    if bytes[index] == b'%' && index + 2 < bytes.len() {
      if let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
        out.push(byte);
        index += 3;
        continue;
      }
    }
    out.push(bytes[index]);
    index += 1;
  }
  String::from_utf8_lossy(&out).into_owned()
}

/// Single-use loopback listener for Google's redirect.
pub struct LoopbackRedirect { listener: TcpListener, port: u16, state: String }

impl LoopbackRedirect {
  pub fn bind(state: String) -> Result<Self, OAuthError> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|error| OAuthError::Network(format!("cannot open a local port ({error})")))?;
    let port = listener.local_addr().map_err(|error| OAuthError::Network(error.to_string()))?.port();
    listener.set_nonblocking(true).map_err(|error| OAuthError::Network(error.to_string()))?;
    Ok(Self { listener, port, state })
  }

  pub fn redirect_uri(&self) -> String { format!("http://127.0.0.1:{}", self.port) }

  /// Waits (bounded) for Google's redirect, validates `state`, and answers the
  /// browser with a short HTML page. Returns as soon as the outcome is known:
  /// an authorization code, a refusal (cancelled / denied — no code will ever
  /// follow), the user cancelling from the app, or the deadline.
  pub fn wait_for_code(&self, timeout: Duration, cancelled: &AtomicBool) -> Result<String, OAuthError> {
    let deadline = Instant::now() + timeout;
    loop {
      if cancelled.load(Ordering::Relaxed) {
        return Err(OAuthError::Protocol("the Google sign-in was cancelled".into()));
      }
      if Instant::now() >= deadline {
        return Err(OAuthError::Protocol("timed out waiting for Google's browser response — nothing arrived at the local redirect".into()));
      }
      match self.listener.accept() {
        Ok((mut stream, _)) => {
          let mut buffer = [0u8; 4096];
          stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
          let read = stream.read(&mut buffer).unwrap_or(0);
          let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
          match parse_loopback_callback(&request) {
            LoopbackCallback::Code { code, state } if state == self.state => {
              respond(&mut stream, "<html><body style=\"font-family:sans-serif\"><h2>Relay is authorized</h2><p>You can close this tab and return to Relay.</p></body></html>");
              return Ok(code);
            }
            // A refusal is final: return it now instead of waiting for a code
            // that Google is not going to send. Only the classification is
            // logged — never the value, which any local process could have sent.
            LoopbackCallback::Refusal { error, state } if state == self.state => {
              respond(&mut stream, "<html><body style=\"font-family:sans-serif\"><h2>Relay was not authorized</h2><p>Return to Relay and start the Google sign-in again.</p></body></html>");
              tracing::warn!("google authorization refused at the loopback redirect");
              return Err(OAuthError::Protocol(loopback_refusal_hint(&error)));
            }
            // A stale or unrecognised request (a favicon probe, a replay with the
            // wrong state): answer it and keep waiting — it says nothing about
            // whether the real redirect is still coming.
            _ => respond(&mut stream, "<html><body style=\"font-family:sans-serif\"><h2>That link did not authorize Relay</h2><p>Return to Relay and start the Google sign-in again.</p></body></html>"),
          }
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(60)),
        Err(error) => return Err(OAuthError::Network(error.to_string())),
      }
    }
  }
}

fn respond(stream: &mut std::net::TcpStream, body: &str) {
  let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
  let _ = stream.write_all(response.as_bytes());
  let _ = stream.flush();
}

pub fn exchange_code(client_id: &str, client_secret: Option<&str>, code: &str, verifier: &str, redirect_uri: &str) -> Result<TokenResponse, OAuthError> {
  let mut form = vec![
    ("code", code),
    ("client_id", client_id),
    ("redirect_uri", redirect_uri),
    ("grant_type", "authorization_code"),
    ("code_verifier", verifier),
  ];
  if let Some(secret) = client_secret.filter(|value| !value.is_empty()) { form.push(("client_secret", secret)); }
  token_request(&form)
}

pub fn refresh_access_token(client_id: &str, client_secret: Option<&str>, refresh_token: &str) -> Result<TokenResponse, OAuthError> {
  let mut form = vec![("client_id", client_id), ("grant_type", "refresh_token"), ("refresh_token", refresh_token)];
  if let Some(secret) = client_secret.filter(|value| !value.is_empty()) { form.push(("client_secret", secret)); }
  token_request(&form)
}

fn token_request(form: &[(&str, &str)]) -> Result<TokenResponse, OAuthError> {
  let response = ureq::post(GOOGLE_TOKEN_URL).send_form(form).map_err(|error| match error {
    ureq::Error::Status(_, response) => OAuthError::Protocol(google_error_hint(&response.into_string().unwrap_or_default())),
    ureq::Error::Transport(transport) => OAuthError::Network(transport.to_string()),
  })?;
  response.into_json::<TokenResponse>().map_err(|error| OAuthError::Protocol(format!("unreadable token response ({error})")))
}

/// Maps Google's structured error responses to fixed, non-sensitive hints.
/// The raw body is never returned: some responses echo request fields.
pub fn google_error_hint(body: &str) -> String {
  let value: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
  let code = value.get("error").and_then(|v| v.as_str()).unwrap_or_default().to_lowercase();
  let description = value.get("error_description").and_then(|v| v.as_str()).unwrap_or_default().to_lowercase();
  if description.contains("client_secret") || description.contains("client secret") || code.contains("invalid_client_secret") {
    "Google rejected the OAuth client secret — paste the Client secret from the same credential, or leave it empty for a Desktop app client.".into()
  } else if code.contains("invalid_client") || description.contains("invalid client") {
    "Google rejected the OAuth client id — check that the full id was pasted and the client is of type 'Web application' (or 'Desktop app' if you are not using a secret).".into()
  } else if description.contains("redirect_uri") || code.contains("redirect_uri") {
    "Google rejected the redirect — the OAuth client must list http://127.0.0.1 as an authorized redirect URI.".into()
  } else if code.contains("access_denied") || description.contains("access_denied") {
    "Google sign-in was cancelled, or the account denied the mail permission. Start again and allow access.".into()
  } else if code.contains("invalid_grant") || description.contains("expired") || description.contains("already") || description.contains("revoked") {
    "the authorization expired, was already used, or was revoked — remove Relay at myaccount.google.com/permissions and start the Google sign-in again".into()
  } else if code.contains("unauthorized_client") || description.contains("unauthorized") {
    "the OAuth client is not allowed to use this flow — in Google Cloud, set the client type to Web application (with a secret) or Desktop app, and enable the Gmail API.".into()
  } else if code.contains("invalid_scope") || description.contains("scope") {
    "Google refused the requested mail scope — enable the Gmail API for this Cloud project and consent to mail access.".into()
  } else if description.contains("disabled") || description.contains("not enabled") || description.contains("has not been used") {
    "the Gmail API is not enabled for this Google Cloud project — enable it under APIs & Services, then sign in again.".into()
  } else {
    "Google refused the token exchange — verify the OAuth client configuration and try again".into()
  }
}

/// Reads the Gmail address out of the id_token's payload segment. The token
/// came directly from Google's token endpoint over TLS, so the claim is
/// trusted locally; no signature check is needed because it is not a trust
/// boundary here.
pub fn email_from_id_token(id_token: &str) -> Option<String> {
  let payload = id_token.split('.').nth(1)?;
  let bytes = base64url_decode(payload)?;
  let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
  value.get("email")?.as_str().map(str::to_string)
}