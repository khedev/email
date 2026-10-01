//! SMTP mail delivery — the send half of the email transport.
//!
//! `mailbox.rs` downloads what the server holds; this module is what actually
//! puts a message into the outgoing mail stream. It deliberately mirrors that
//! module's discipline:
//!
//! 1. the caller (`commands::deliver_queued_mail`) resolves the account, the
//!    credential and the message text and hands them here — this module never
//!    touches SQLite and never reads the credential store;
//! 2. one SMTP session per account carries every message queued for it;
//! 3. failures are classified into fixed, non-sensitive sentences. The server's
//!    own text is never echoed: a relay may quote the message or the command
//!    back at us (the same rule as `verify.rs` and `mailbox::fetch_failure`).
//!
//! The account's stored `encryption` selects the connection shape exactly as it
//! does for IMAP: `ssl` is implicit TLS, `tls` is a STARTTLS upgrade, and `none`
//! is plaintext — only when explicitly chosen. A Google account sends with
//! XOAUTH2 through the same short-lived access token the fetch path uses, so no
//! account password is ever required for a Google mailbox.
//!
//! Message assembly is delegated to `lettre`'s builder: MIME encoding, header
//! folding and transfer encoding are its job, and Bcc recipients are placed in
//! the SMTP envelope without being written into the transmitted headers.

use std::time::Duration;

use lettre::message::{Mailbox, Message};
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::SmtpTransport;
use lettre::Transport;

use crate::models::{AccountConnection, OutboundMessage};
use crate::verify::{probe_reachable, MailCredential};

/// How long one SMTP command may take before the attempt is abandoned. Without
/// it, a relay that stops answering holds the delivery worker thread until the
/// OS-level TCP timeout.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// The result of one delivery attempt.
#[derive(Debug)]
pub enum DeliveryOutcome {
  Sent,
  /// A fixed, non-sensitive explanation of why the attempt failed.
  Failed(String),
}

/// Sends every message queued for one account over a single SMTP session.
///
/// Returns one outcome per message, in the order given. The account is resolved
/// once: a connection that cannot be built (unreachable host, refused session)
/// fails every message with the same verdict, while a message the relay rejects
/// individually (bad recipient) fails alone and does not stop the rest.
pub fn deliver(connection: &AccountConnection, credential: MailCredential<'_>, messages: &[OutboundMessage]) -> Vec<(String, DeliveryOutcome)> {
  let transport = match build_transport(connection, &credential) {
    Ok(transport) => transport,
    Err(error) => return messages.iter().map(|message| (message.id.clone(), DeliveryOutcome::Failed(error.clone()))).collect(),
  };
  let oauth = matches!(credential, MailCredential::AccessToken(_));
  messages
    .iter()
    .map(|message| {
      let outcome = match build_message(message) {
        Ok(built) => match transport.send(&built) {
          Ok(_) => DeliveryOutcome::Sent,
          Err(error) => DeliveryOutcome::Failed(classify_send_failure(&error, oauth)),
        },
        Err(error) => DeliveryOutcome::Failed(error),
      };
      (message.id.clone(), outcome)
    })
    .collect()
}

/// Builds the SMTP session for one account, honoring its stored encryption and
/// credential kind.
fn build_transport(connection: &AccountConnection, credential: &MailCredential<'_>) -> Result<SmtpTransport, String> {
  // Fail fast on an unreachable host through the same bounded probe sign-in and
  // fetch use, so a wrong SMTP host reports a connection problem instead of
  // sitting out the OS TCP timeout inside the handshake.
  probe_reachable(&connection.smtp_host, connection.smtp_port)
    .map_err(|_| "The mail server could not be reached for sending. Check the connection and try again.".to_string())?;
  let host = connection.smtp_host.as_str();
  let builder = match connection.encryption.as_str() {
    "ssl" => SmtpTransport::relay(host),
    "tls" => SmtpTransport::starttls_relay(host),
    "none" => Ok(SmtpTransport::builder_dangerous(host)),
    // An unrecognized mode follows the port the way `verify::connect_imap`
    // does: 465 is implicit TLS, everything else is expected to offer STARTTLS.
    _ if connection.smtp_port == 465 => SmtpTransport::relay(host),
    _ => SmtpTransport::starttls_relay(host),
  }
  .map_err(|_| "The sending server for this account has no usable address. Check the account settings.".to_string())?;
  let secret = match credential {
    MailCredential::Password(password) => *password,
    MailCredential::AccessToken(token) => *token,
  };
  let builder = builder.port(connection.smtp_port).credentials(Credentials::new(connection.email_address.clone(), secret.to_string()));
  // Google (and other OAuth 2.0 providers) refuse a password mechanism for
  // mailboxes they host; the short-lived access token goes through XOAUTH2.
  let builder = match credential {
    MailCredential::AccessToken(_) => builder.authentication(vec![Mechanism::Xoauth2]),
    MailCredential::Password(_) => builder,
  };
  Ok(builder.timeout(Some(COMMAND_TIMEOUT)).build())
}

/// Assembles the transmitted message for one outbound row.
///
/// Exposed for testing: the raw output of `lettre::Message::formatted()` is what
/// a relay receives, so a fixture test can assert the headers and body that
/// leave the machine without needing a server.
pub fn build_message(message: &OutboundMessage) -> Result<Message, String> {
  let from = mailbox(&message.from_name, &message.from_address)?;
  let mut builder = Message::builder().from(from).subject(message.subject.clone());
  for address in &message.to {
    builder = builder.to(mailbox("", address)?);
  }
  for address in &message.cc {
    builder = builder.cc(mailbox("", address)?);
  }
  for address in &message.bcc {
    builder = builder.bcc(mailbox("", address)?);
  }
  builder
    .message_id(Some(message_id_for(&message.from_address)))
    .body(message.body_text.clone())
    .map_err(|_| "The message could not be prepared for sending.".to_string())
}

/// A display name is only attached when there is one; a bare address is left as
/// a bare address instead of an empty quoted string.
fn mailbox(name: &str, address: &str) -> Result<Mailbox, String> {
  let parsed = address.trim().parse::<lettre::Address>().map_err(|_| "An address on this message is not valid, so it was not sent.".to_string())?;
  let name = name.trim();
  Ok(if name.is_empty() { Mailbox::new(None, parsed) } else { Mailbox::new(Some(name.to_string()), parsed) })
}

/// `<uuid@domain>` from the sender's own domain, so a client that threads by
/// Message-ID can group the delivered copy wherever it lands.
fn message_id_for(from_address: &str) -> String {
  let domain = from_address.rsplit('@').next().unwrap_or("relay.local");
  format!("<{}@{}>", uuid::Uuid::new_v4(), domain)
}

/// Turns a failed SMTP attempt into a fixed, non-sensitive sentence.
///
/// Only the response *code* and the transport's own machine-readable flags are
/// read; the server's text never leaves this function.
pub fn classify_send_failure(error: &lettre::transport::smtp::Error, oauth: bool) -> String {
  let code = error.status().map(u16::from);
  // Codes, counts and flags only — never the server's wording.
  tracing::warn!(code = ?code, oauth, transient = error.is_transient(), tls = error.is_tls(), "smtp delivery failed");
  failure_message(code, oauth, error.is_response(), error.is_timeout() || error.is_transport_shutdown())
}

/// The classifier proper, expressed as a plain function of the facts so every
/// branch can be tested without a server.
pub fn failure_message(code: Option<u16>, oauth: bool, response: bool, timeout: bool) -> String {
  if let Some(code) = code {
    match code {
      // The standard "authentication rejected" answers.
      530 | 534 | 535 | 538 => {
        return if oauth {
          "The mail server refused the stored Google authorization for sending. Sign in with Google again to renew it.".to_string()
        } else {
          "The mail server refused the stored sign-in for sending. Check the account password, or use an app password.".to_string()
        }
      }
      // An unknown or refused mailbox, and a relay-side block.
      550 | 551 | 552 | 553 | 554 => return "The mail server refused the message. Check the recipient addresses and try again.".to_string(),
      // The transient "try again later" answers.
      421 | 450 | 451 | 452 => return "The mail server is temporarily not accepting mail. Try again shortly.".to_string(),
      _ => {}
    }
  }
  if timeout || !response {
    return "The connection to the mail server dropped while sending. Try again.".to_string();
  }
  "Sending failed. Check the connection and try again.".to_string()
}
