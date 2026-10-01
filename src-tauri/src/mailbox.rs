//! IMAP mail retrieval — the fetch half of the email transport.
//!
//! Sign-in only *proves* that the stored credential unlocks the mailbox
//! (`verify.rs`); this module is what actually downloads mail into the local
//! store. It reuses `verify::open_mail_session` so both paths authenticate
//! identically, then:
//!
//! 1. `SELECT INBOX` and record `UIDVALIDITY`/`UIDNEXT` for incremental syncs;
//! 2. `FETCH` the newest [`FETCH_CAP`] messages by sequence range, asking for
//!    `UID`, `FLAGS`, `INTERNALDATE`, `ENVELOPE` and `BODY.PEEK[]` (`PEEK` so
//!    fetching never marks mail as read on the server);
//! 3. turn each message into a [`FetchedMessage`] with decoded text.
//!
//! A single message's body can also be pulled on demand, by UID
//! ([`fetch_message_body`]), which repairs a row that is already stored with an
//! empty body.
//!
//! The parsing helpers are pure functions with fixture tests: MIME is untrusted
//! input, so nothing here echoes a raw server body into a log or an error.
//! Body text is extracted as plain text — HTML is reduced to text, never
//! rendered — which keeps the reader's no-HTML rendering guarantee intact.

use chrono::Utc;
use imap::types::{Flag, Fetches};

use crate::models::AccountConnection;
use crate::verify::{open_mail_session, MailCredential};

/// Upper bound on messages downloaded per sync, so one click cannot pull a
/// decade of mail over a slow connection. The newest messages win.
pub const FETCH_CAP: usize = 50;

/// The FETCH item that returns one whole message.
///
/// `BODY[]` is the *entire* RFC 5322 message — header block and body together —
/// which is exactly what [`text_body_of`] expects to decode. Asking for
/// `BODY[HEADER]` and `BODY[TEXT]` separately looks equivalent but is not: those
/// responses are keyed `Some(Full(Header))` / `Some(Full(Text))` and are read
/// back with `Fetch::header()` / `Fetch::text()`, whereas `Fetch::body()` matches
/// the **unsectioned** `BODY[]` only. Requesting the two sections and then
/// reading `body()` therefore yielded nothing but the header — a message with no
/// readable text — for every single message.
pub const FETCH_FULL_MESSAGE: &str = "BODY.PEEK[]";

// Note on timeouts: the *connect* phase is bounded by the 5 s reachability probe
// in `verify`, but the fetch response itself is not separately bounded — the
// pinned `imap` crate exposes no read timeout on an established session (only
// `idle` can set one on the underlying stream). A server that goes silent
// mid-fetch therefore stalls this worker thread until the OS-level TCP timeout.
// That is survivable by design: the sync runs on its own worker thread, so the
// window stays responsive and the user can leave the view. Bounding it properly
// needs an upstream API (or an idle-based fetch).

/// One message as read from the server, already decoded to plain text.
pub struct FetchedMessage {
  pub uid: u32,
  pub sender_name: String,
  pub sender_email: String,
  pub subject: String,
  pub body_text: String,
  pub received_at: String,
  pub is_read: bool,
  pub is_starred: bool,
  pub message_id: Option<String>,
}

/// The result of one inbox sync.
pub struct FetchedInbox {
  pub messages: Vec<FetchedMessage>,
  /// Server identity of the mailbox; when it changes the cached UIDs are
  /// meaningless and the next sync must refetch.
  pub uid_validity: Option<u32>,
  pub uid_next: Option<u32>,
}

// ---------------------------------------------------------------------------
// MIME decoding. Pure functions, fixture-tested: this data comes from the
// network and is untrusted, so nothing here is allowed to hit a log or panic.
// ---------------------------------------------------------------------------

/// Hard cap on a decoded body, so a hostile server cannot exhaust memory.
const MAX_BODY_BYTES: usize = 512 * 1024;

/// Nesting guard for pathological MIME trees.
const MAX_MIME_DEPTH: usize = 6;

/// A decoded entity, kept apart until the caller picks the best one: a
/// `multipart/alternative` message offers both a plain-text and an HTML version.
enum BodyPart { Plain(String), Html(String) }

/// Splits a raw message (or MIME part) into its header block and its body.
fn split_headers(raw: &str) -> (&str, &str) {
  if let Some(index) = raw.find("\r\n\r\n") { return (&raw[..index], &raw[index + 4..]); }
  if let Some(index) = raw.find("\n\n") { return (&raw[..index], &raw[index + 2..]); }
  (raw, "")
}

/// Collects `name: value` header pairs, unfolding continuation lines (RFC 5322
/// allows a header to be wrapped) and lowercasing the names.
fn decode_headers(block: &str) -> Vec<(String, String)> {
  let mut out: Vec<(String, String)> = Vec::new();
  for line in block.lines() {
    if line.starts_with(' ') || line.starts_with('\t') {
      if let Some(last) = out.last_mut() { last.1.push(' '); last.1.push_str(line.trim()); }
      continue;
    }
    if let Some((name, value)) = line.split_once(':') {
      out.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
  }
  out
}

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
  headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
}

/// Extracts the plain-text body of a raw message, preferring `text/plain`
/// anywhere in the tree and falling back to `text/html` reduced to text. An
/// empty string means there was no readable text part.
pub fn text_body_of(raw: &str) -> String {
  let (headers, body) = split_headers(raw);
  let parsed = decode_headers(headers);
  let mut parts: Vec<BodyPart> = Vec::new();
  extract_parts(&parsed, body, 0, &mut parts);
  let plain = parts.iter().find_map(|part| match part {
    BodyPart::Plain(text) if !text.trim().is_empty() => Some(normalise_lines(text)),
    _ => None,
  });
  let chosen = plain.or_else(|| parts.iter().find_map(|part| match part {
    BodyPart::Html(text) if !text.trim().is_empty() => Some(html_to_text(text)),
    _ => None,
  }));
  chosen.map(truncate_body).unwrap_or_default()
}

/// Walks one MIME entity, descending into multipart containers and decoding any
/// text entity it finds. Non-text parts (images, attachments) are ignored.
fn extract_parts(headers: &[(String, String)], body: &str, depth: usize, out: &mut Vec<BodyPart>) {
  if depth > MAX_MIME_DEPTH { return; }
  let content_type = header_value(headers, "content-type").unwrap_or("text/plain");
  let lowered = content_type.to_ascii_lowercase();
  if lowered.contains("multipart/") {
    if let Some(boundary) = boundary_of(content_type) {
      for part in split_parts(body, &boundary) {
        let (part_headers, part_body) = split_headers(&part);
        let parsed = decode_headers(part_headers);
        extract_parts(&parsed, part_body, depth + 1, out);
      }
    }
    return;
  }
  let decoded = decode_transfer(header_value(headers, "content-transfer-encoding"), body);
  if lowered.contains("text/html") {
    out.push(BodyPart::Html(decoded));
  } else if lowered.contains("text/") || lowered.trim().is_empty() {
    out.push(BodyPart::Plain(decoded));
  }
}

/// Splits a multipart body on its boundary, dropping the preamble and epilogue.
fn split_parts(body: &str, boundary: &str) -> Vec<String> {
  let marker = format!("--{boundary}");
  let mut parts = Vec::new();
  for segment in body.split(&marker).skip(1) {
    if segment.starts_with("--") { break; } // the closing delimiter
    parts.push(segment.trim_start_matches(['\r', '\n']).to_string());
  }
  parts
}

/// Reads the `boundary` parameter of a Content-Type header, quoted or bare.
fn boundary_of(content_type: &str) -> Option<String> {
  let index = content_type.to_ascii_lowercase().find("boundary=")?;
  let value = content_type[index + "boundary=".len()..].trim_start();
  if let Some(rest) = value.strip_prefix('"') {
    return rest.find('"').map(|end| rest[..end].to_string());
  }
  Some(value.split(|character: char| character == ';' || character.is_whitespace()).next().unwrap_or("").to_string())
}

fn decode_transfer(encoding: Option<&str>, body: &str) -> String {
  match encoding.map(|value| value.trim().to_ascii_lowercase()) {
    Some(value) if value.contains("base64") => decode_base64(body),
    Some(value) if value.contains("quoted-printable") => decode_quoted_printable(body),
    _ => body.to_string(),
  }
}

/// Minimal RFC 4648 decoder (standard or URL-safe alphabet; padding, newlines
/// and spaces are skipped). A single left-to-right bit accumulator, so it
/// tolerates the line breaks that base64 bodies always contain.
pub fn decode_base64(input: &str) -> String {
  let mut out: Vec<u8> = Vec::with_capacity(input.len() / 4 * 3);
  let mut buffer: u32 = 0;
  let mut bits: u32 = 0;
  for byte in input.bytes() {
    let value = match byte {
      b'A'..=b'Z' => byte - b'A',
      b'a'..=b'z' => byte - b'a' + 26,
      b'0'..=b'9' => byte - b'0' + 52,
      b'+' | b'-' => 62,
      b'/' | b'_' => 63,
      _ => continue,
    } as u32;
    buffer = (buffer << 6) | value;
    bits += 6;
    if bits >= 8 {
      bits -= 8;
      out.push((buffer >> bits) as u8);
    }
  }
  String::from_utf8_lossy(&out).into_owned()
}

/// Quoted-printable decoder: `=XX` hex escapes and soft line breaks (`=` at the
/// end of a line).
pub fn decode_quoted_printable(input: &str) -> String {
  let bytes = input.as_bytes();
  let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
  let mut index = 0;
  while index < bytes.len() {
    if bytes[index] != b'=' {
      out.push(bytes[index]);
      index += 1;
      continue;
    }
    // Soft line break: the "=" swallows the line ending that follows it.
    if index + 1 < bytes.len() && (bytes[index + 1] == b'\r' || bytes[index + 1] == b'\n') {
      index += 1;
      if index < bytes.len() && bytes[index] == b'\r' { index += 1; }
      if index < bytes.len() && bytes[index] == b'\n' { index += 1; }
      continue;
    }
    if index + 2 < bytes.len() {
      if let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3]) {
        if let Ok(byte) = u8::from_str_radix(hex, 16) {
          out.push(byte);
          index += 3;
          continue;
        }
      }
    }
    out.push(b'=');
    index += 1;
  }
  String::from_utf8_lossy(&out).into_owned()
}

/// Decodes RFC 2047 encoded words (`=?UTF-8?B?…?=`), which is how providers
/// carry non-ASCII subjects and display names. Malformed words are left as they
/// arrived rather than dropped.
pub fn decode_mime_words(input: &str) -> String {
  let mut out = String::with_capacity(input.len());
  let mut rest = input;
  while let Some(start) = rest.find("=?") {
    out.push_str(&rest[..start]);
    let after = &rest[start + 2..];
    let Some(end) = after.find("?=") else {
      out.push_str(&rest[start..]);
      return out;
    };
    let encoded = &after[..end];
    let mut fields = encoded.splitn(3, '?');
    let _charset = fields.next();
    let mode = fields.next();
    let text = fields.next();
    let raw = &rest[start..start + 2 + end + 2];
    let decoded = match (mode, text) {
      (Some(mode), Some(text)) if mode.eq_ignore_ascii_case("B") => decode_base64(text),
      // In Q encoding an underscore stands for a space (RFC 2047 section 4.2).
      (Some(mode), Some(text)) if mode.eq_ignore_ascii_case("Q") => decode_quoted_printable(&text.replace('_', " ")),
      _ => String::new(),
    };
    if decoded.is_empty() { out.push_str(raw); } else { out.push_str(&decoded); }
    rest = &after[end + 2..];
  }
  out.push_str(rest);
  out
}

/// Decodes a raw header value that arrived as bytes (the envelope sends raw
/// bytes, not a Rust string).
pub fn decode_header_bytes(bytes: &[u8]) -> String {
  decode_mime_words(String::from_utf8_lossy(bytes).as_ref())
}

/// Reduces HTML to readable plain text.
///
/// Deliberately minimal, and *not* a general-purpose renderer: the reader shows
/// `body_text` as text (never as HTML, and the CSP blocks remote content), so
/// the purpose here is legibility — and stripping every tag means no markup from
/// a message can reach the UI. `<script>`/`<style>` contents are dropped
/// entirely; block elements become line breaks; the common entities are decoded.
pub fn html_to_text(html: &str) -> String {
  let mut out = String::with_capacity(html.len());
  let mut suppressed: u32 = 0;
  let mut in_tag = false;
  let mut tag = String::new();
  for character in html.chars() {
    match character {
      '<' => { in_tag = true; tag.clear(); }
      '>' if in_tag => {
        in_tag = false;
        let closing = tag.trim_start().starts_with('/');
        let name = tag.trim_start_matches('/')
          .split(|character: char| character.is_whitespace() || character == '/')
          .next()
          .unwrap_or("")
          .to_ascii_lowercase();
        if matches!(name.as_str(), "script" | "style") {
          suppressed = if closing { suppressed.saturating_sub(1) } else { suppressed + 1 };
        } else if name == "br" {
          out.push('\n');
        } else if closing && matches!(name.as_str(), "p" | "div" | "tr" | "li" | "blockquote" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6") {
          out.push('\n');
        }
      }
      _ if in_tag => tag.push(character),
      _ if suppressed > 0 => {}
      _ => out.push(character),
    }
  }
  collapse_blank_lines(&decode_html_entities(&out))
}

fn decode_html_entities(input: &str) -> String {
  let mut out = String::with_capacity(input.len());
  let mut index = 0;
  while let Some(offset) = input[index..].find('&') {
    let start = index + offset;
    out.push_str(&input[index..start]);
    let tail = &input[start..];
    match tail[1..].find(';').filter(|end| *end <= 10).map(|end| end + 1) {
      Some(end) => {
        let entity = &tail[1..end];
        let known = match entity {
          "amp" => Some("&"), "lt" => Some("<"), "gt" => Some(">"), "quot" => Some("\""),
          "apos" | "#39" => Some("'"), "nbsp" => Some(" "), "mdash" => Some("—"),
          "ndash" => Some("–"), "hellip" => Some("…"), "rsquo" => Some("’"), "lsquo" => Some("‘"),
          "ldquo" => Some("“"), "rdquo" => Some("”"), _ => None,
        };
        if let Some(replacement) = known { out.push_str(replacement); }
        else if let Some(character) = numeric_entity(entity) { out.push(character); }
        else { out.push_str(&tail[..end + 1]); }
        index = start + end + 1;
      }
      None => { out.push('&'); index = start + 1; }
    }
  }
  out.push_str(&input[index..]);
  out
}

fn numeric_entity(entity: &str) -> Option<char> {
  let digits = entity.strip_prefix('#')?;
  let code = match digits.strip_prefix(['x', 'X']) {
    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
    None => digits.parse::<u32>().ok()?,
  };
  char::from_u32(code)
}

/// Normalises line endings and trims the edges of a `text/plain` body. The
/// author's own blank lines are preserved: only the transfer-encoding line
/// endings are rewritten.
fn normalise_lines(input: &str) -> String {
  input.replace("\r\n", "\n").replace('\r', "\n").trim_end().to_string()
}

/// Normalises line endings and drops runs of blank lines so the reading pane
/// does not show a wall of empty space from a heavily-formatted message.
fn collapse_blank_lines(input: &str) -> String {
  let mut out = String::with_capacity(input.len());
  let mut blanks = 0;
  for line in input.replace('\r', "").lines() {
    let trimmed = line.trim_end();
    if trimmed.trim().is_empty() {
      blanks += 1;
      if blanks > 1 { continue; }
      out.push('\n');
    } else {
      blanks = 0;
      out.push_str(trimmed);
      out.push('\n');
    }
  }
  out.trim().to_string()
}

/// Caps the stored body so one message cannot bloat the database, cutting on a
/// character boundary.
fn truncate_body(text: String) -> String {
  if text.len() <= MAX_BODY_BYTES { return text; }
  let mut end = MAX_BODY_BYTES;
  while end > 0 && !text.is_char_boundary(end) { end -= 1; }
  format!("{}…", &text[..end])
}

/// The sequence-number range covering the newest `cap` messages of a mailbox
/// holding `exists` messages, or `None` when the mailbox is empty.
///
/// This is deliberately its own function, and deliberately named for *sequence*
/// numbers, because getting the ID space wrong is a **silent** bug: `inbox.exists`
/// is a count in sequence space (RFC 3501 §6.4.6), while `UID FETCH` takes UIDs
/// (§6.4.8). Deriving one and sending it as the other returns zero messages for
/// a mailbox whose UID numbering has ever advanced past its current size — which
/// looks exactly like an empty mailbox. `fetch_inbox` therefore calls the
/// sequence-based `Client::fetch`.
pub fn newest_sequence_range(exists: u32, cap: u32) -> Option<(u32, u32)> {
  if exists == 0 || cap == 0 { return None; }
  let first = exists.saturating_sub(cap).saturating_add(1);
  Some((first.max(1), exists))
}

/// Reports FETCH coverage when the server returns fewer responses than the
/// range covered. Counts only — a message subject or body is never logged.
fn note_partial_fetch(requested: u32, received: usize) {
  if (received as u32) < requested {
    tracing::warn!(requested, received, "imap fetch returned fewer messages than the sequence range covered");
  }
}

/// Downloads the newest messages from INBOX.
///
/// `progress` receives short status lines for the UI. Nothing is written to the
/// database here — the caller persists under its own lock/transaction.
pub fn fetch_inbox(connection: &AccountConnection, credential: MailCredential<'_>, cap: usize, progress: impl Fn(&str)) -> Result<FetchedInbox, String> {
  progress("Connecting to the mail server…");
  let mut session = open_mail_session(&connection.email_address, credential, &connection.imap_host, connection.imap_port, &connection.encryption)
    .map_err(|failure| failure.user_message())?;
  let inbox = session.select("INBOX").map_err(|error| fetch_failure(&error))?;
  let (uid_validity, uid_next, exists) = (inbox.uid_validity, inbox.uid_next, inbox.exists);
  let Some((first, last)) = newest_sequence_range(exists, cap.clamp(1, u32::MAX as usize) as u32) else {
    let _ = session.logout();
    return Ok(FetchedInbox { messages: Vec::new(), uid_validity, uid_next });
  };
  let sequence_range = format!("{first}:{last}");
  progress(&format!("Downloading {} message(s)…", last - first + 1));
  // `fetch` (not `uid_fetch`): the range above is in *sequence* space, which is
  // what `exists` describes. Asking for `UID FLAGS` alongside keeps the UID —
  // used to key the stored row — in the same response.
  let fetches = session
    .fetch(&sequence_range, format!("(UID FLAGS INTERNALDATE ENVELOPE {FETCH_FULL_MESSAGE})"))
    .map_err(|error| fetch_failure(&error))?;
  note_partial_fetch(last - first + 1, fetches.len());
  let messages = collect_messages(&fetches);
  let _ = session.logout();
  Ok(FetchedInbox { messages, uid_validity, uid_next })
}

/// Rebuilds the raw RFC 5322 message that [`text_body_of`] decodes, out of
/// whichever sections the server actually answered with.
///
/// A `BODY[]` response already *is* the whole message, so it is returned as-is.
/// Otherwise the header and the text are stitched back together: a header on its
/// own can only ever decode to an empty body, which is how every synced message
/// lost its content. The prefix check tolerates a server that also returns the
/// header inside `BODY[TEXT]`, which would otherwise be parsed twice.
pub fn assemble_raw_message(full: Option<&[u8]>, header: Option<&[u8]>, text: Option<&[u8]>) -> Vec<u8> {
  if let Some(bytes) = full { return bytes.to_vec(); }
  let header = header.unwrap_or(&[]);
  let Some(text) = text else { return header.to_vec() };
  if !header.is_empty() && text.starts_with(header) { return text.to_vec(); }
  let mut raw = header.to_vec();
  if !raw.is_empty() && !raw.ends_with(b"\r\n\r\n") {
    if raw.ends_with(b"\r\n") { raw.extend_from_slice(b"\r\n"); } else { raw.extend_from_slice(b"\r\n\r\n"); }
  }
  raw.extend_from_slice(text);
  raw
}

/// Turns a `FETCH` response into decoded messages. Kept separate from the
/// network so the conversion is covered by fixture tests.
fn collect_messages(fetches: &Fetches) -> Vec<FetchedMessage> {
  let mut messages = Vec::with_capacity(fetches.len());
  for fetch in fetches.iter() {
    // Without a UID the row cannot be de-duplicated on the next sync, so such a
    // response is skipped rather than stored as a duplicate.
    let Some(uid) = fetch.uid else { continue };
    let raw = assemble_raw_message(fetch.body(), fetch.header(), fetch.text());
    let envelope = fetch.envelope();
    let subject = envelope.and_then(|value| value.subject.as_deref()).map(decode_header_bytes).unwrap_or_default();
    let (sender_name, sender_email) = envelope
      .and_then(|value| value.from.as_ref())
      .and_then(|addresses| addresses.first())
      .map(|address| {
        let name = address.name.as_deref().map(decode_header_bytes).unwrap_or_default();
        let mailbox = address.mailbox.as_deref().map(|value| String::from_utf8_lossy(value).into_owned()).unwrap_or_default();
        let host = address.host.as_deref().map(|value| String::from_utf8_lossy(value).into_owned()).unwrap_or_default();
        let email = if mailbox.is_empty() { String::new() } else if host.is_empty() { mailbox } else { format!("{mailbox}@{host}") };
        (name, email)
      })
      .unwrap_or_default();
    let message_id = envelope
      .and_then(|value| value.message_id.as_deref())
      .map(|value| String::from_utf8_lossy(value).trim().trim_matches(['<', '>']).to_string())
      .filter(|value| !value.is_empty());
    messages.push(FetchedMessage {
      uid,
      sender_name,
      sender_email,
      subject,
      body_text: text_body_of(&String::from_utf8_lossy(&raw)),
      received_at: fetch.internal_date().map(|date| date.with_timezone(&Utc).to_rfc3339()).unwrap_or_else(|| Utc::now().to_rfc3339()),
      is_read: fetch.flags().iter().any(|flag| matches!(flag, Flag::Seen)),
      is_starred: fetch.flags().iter().any(|flag| matches!(flag, Flag::Flagged)),
      message_id,
    });
  }
  // A message whose body will not decode is invisible to the reader even though
  // its row exists, so surface the count. Counts only — never content.
  let blank = messages.iter().filter(|message| message.body_text.is_empty()).count();
  if blank > 0 {
    tracing::warn!(blank, total = messages.len(), "imap fetch returned messages with no readable text body");
  }
  messages
}

/// Downloads one message's complete body by IMAP UID.
///
/// Repairs a message that is already stored locally with an empty body: either
/// it was fetched before the body could be decoded, or it sits outside the
/// newest-[`FETCH_CAP`] window a sync covers. `PEEK` keeps it unread on the
/// server, and the decoded text is *returned* rather than written, so the caller
/// keeps ownership of the database lock (the same discipline as `fetch_inbox`).
pub fn fetch_message_body(connection: &AccountConnection, credential: MailCredential<'_>, uid: u32) -> Result<String, String> {
  let mut session = open_mail_session(&connection.email_address, credential, &connection.imap_host, connection.imap_port, &connection.encryption)
    .map_err(|failure| failure.user_message())?;
  // A UID is only meaningful inside a selected mailbox, and every row this store
  // holds came from INBOX.
  session.select("INBOX").map_err(|error| fetch_failure(&error))?;
  let fetches = session
    .uid_fetch(uid.to_string(), format!("({FETCH_FULL_MESSAGE})"))
    .map_err(|error| fetch_failure(&error))?;
  let body = fetches.iter().next().map(|fetch| {
    let raw = assemble_raw_message(fetch.body(), fetch.header(), fetch.text());
    text_body_of(&String::from_utf8_lossy(&raw))
  });
  let _ = session.logout();
  // No response at all means the message has left INBOX on the server. Saying so
  // is the honest outcome; an empty reader would look like a rendering fault.
  body.ok_or_else(|| "This message is no longer in the mailbox on the server.".to_string())
}

/// Maps an IMAP error to a fixed, non-sensitive message. The server's text is
/// never echoed: it can contain the mailbox name or parts of the command.
fn fetch_failure(error: &imap::Error) -> String {
  let lowered = error.to_string().to_lowercase();
  tracing::warn!("imap fetch failed");
  if lowered.contains("no such mailbox") || lowered.contains("does not exist") {
    "The mailbox has no INBOX folder. Check the account configuration.".to_string()
  } else if lowered.contains("authenticationfailed") || lowered.contains("invalid credentials") || lowered.contains("expired") || lowered.contains("revoked") {
    "The mail server refused the stored credential while fetching mail. Sign in again.".to_string()
  } else if lowered.contains("connection lost") || lowered.contains("unexpected end of file") || lowered.contains("connection reset") || lowered.contains("timed out") {
    "The connection to the mail server dropped while fetching mail. Try again.".to_string()
  } else {
    "Mail could not be downloaded from the server. Check the connection and try again.".to_string()
  }
}