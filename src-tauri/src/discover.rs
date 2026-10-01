//! Thunderbird-style email-server discovery for automatic sign-in.
//!
//! Given only an email address, candidate IMAP configurations are collected
//! from four ordered sources — the built-in provider database, DNS MX records
//! (which reveal Google Workspace / Microsoft 365 custom domains), the
//! Thunderbird-compatible autoconfig XML (Mozilla's database and the domain's
//! own well-known locations), and hostname guesses. The command layer then
//! authenticates against each candidate with the real password, so discovery
//! never claims a configuration that was not actually verified.
//!
//! Every source degrades gracefully offline: failed lookups simply yield no
//! candidates, and manual setup always remains available.

use crate::verify::{verify_imap_login, VerifyFailure};
use std::time::Duration;

/// Upper bound on candidates tried per sign-in, so a stubborn domain cannot
/// keep the login screen waiting for minutes.
pub const MAX_CANDIDATES: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct MailCandidate {
  pub imap_host: String,
  pub imap_port: u16,
  /// "ssl" (implicit TLS) or "tls" (STARTTLS). Plaintext is never discovered.
  pub encryption: &'static str,
  /// Known hosted provider behind this host, when identified.
  pub provider: Option<&'static str>,
  /// Which discovery source produced this candidate (for UI diagnostics).
  pub source: &'static str,
  /// How this provider expects the account to authenticate over IMAP:
  /// `"password"` (LOGIN/AUTHENTICATE PLAIN with the stored secret, which may be
  /// an App Password) or `"oauth2"` (XOAUTH2 with a Google authorization).
  ///
  /// This is the step Thunderbird performs between "provider configured" and
  /// "credentials entered": for a consumer Google mailbox a password sign-in is
  /// refused by policy, so the account has to be routed to Google sign-in
  /// instead of collecting a password that can never work.
  pub auth_method: &'static str,
  pub smtp_host: String,
  pub smtp_port: u16,
}

/// True when the address is a Google consumer mailbox, where Google refuses
/// IMAP password sign-ins outright (custom Workspace domains are not guessed
/// here — the server's own answer decides, so an admin who permits App
/// Passwords keeps working).
pub fn requires_oauth(address: &str) -> bool {
  match domain_of(address).as_str() {
    "gmail.com" | "googlemail.com" => true,
    _ => false,
  }
}

/// Well-known providers, mirroring Thunderbird's ISPDB snapshot. Only the
/// highest-security configuration is offered per provider (implicit TLS).
pub fn provider_candidates(domain: &str) -> Vec<MailCandidate> {
  let domain = domain.to_ascii_lowercase();
  let known: Option<(&str, &str, &str, u16)> = match domain.as_str() {
    "gmail.com" | "googlemail.com" => Some(("imap.gmail.com", "smtp.gmail.com", "google", 993)),
    "outlook.com" | "hotmail.com" | "hotmail.co.uk" | "hotmail.de" | "live.com" | "live.de" | "msn.com" | "outlook.de" | "outlook.fr" | "outlook.co.uk" => Some(("outlook.office365.com", "smtp.office365.com", "microsoft", 993)),
    "yahoo.com" | "ymail.com" | "rocketmail.com" => Some(("imap.mail.yahoo.com", "smtp.mail.yahoo.com", "yahoo", 993)),
    "icloud.com" | "me.com" | "mac.com" => Some(("imap.mail.me.com", "smtp.mail.me.com", "apple", 993)),
    "fastmail.com" => Some(("imap.fastmail.com", "smtp.fastmail.com", "fastmail", 993)),
    "zoho.com" | "zohomail.com" => Some(("imap.zoho.com", "smtp.zoho.com", "zoho", 993)),
    "gmx.com" => Some(("imap.gmx.com", "mail.gmx.com", "gmx", 993)),
    "gmx.de" | "gmx.net" => Some(("imap.gmx.net", "mail.gmx.net", "gmx", 993)),
    "web.de" => Some(("imap.web.de", "smtp.web.de", "webde", 993)),
    "yandex.com" | "yandex.ru" => Some(("imap.yandex.com", "smtp.yandex.com", "yandex", 993)),
    "aol.com" => Some(("imap.aol.com", "smtp.aol.com", "aol", 993)),
    _ => None,
  };
  match known {
    // Consumer Google mailboxes are flagged for OAuth: Google no longer accepts
    // a normal account password for IMAP on them, so the flow must offer
    // "Sign in with Google" (or an App Password) rather than a plain password.
    Some((imap_host, smtp_host, provider, port)) => vec![MailCandidate {
      imap_host: imap_host.into(),
      imap_port: port,
      encryption: "ssl",
      provider: Some(provider),
      source: "provider-db",
      auth_method: if provider == "google" && matches!(domain.as_str(), "gmail.com" | "googlemail.com") { "oauth2" } else { "password" },
      smtp_host: smtp_host.into(),
      smtp_port: 465,
    }],
    None => vec![],
  }
}

/// Classifies an MX host into a hosted provider, Thunderbird-style: Google
/// Workspace MX hosts (aspmx.l.google.com, googlemail.com), Microsoft 365
/// (…mail.protection.outlook.com), iCloud, Fastmail and Yandex.
pub fn classify_mx(mx_host: &str) -> Option<(&'static str, &'static str, &'static str)> {
  let host = mx_host.trim_end_matches('.').to_ascii_lowercase();
  if host.ends_with(".google.com") || host.ends_with(".googlemail.com") {
    Some(("imap.gmail.com", "smtp.gmail.com", "google"))
  } else if host.ends_with(".mail.protection.outlook.com") || host.ends_with(".olc.protection.outlook.com") {
    Some(("outlook.office365.com", "smtp.office365.com", "microsoft"))
  } else if host.ends_with(".icloud.com") {
    Some(("imap.mail.me.com", "smtp.mail.me.com", "apple"))
  } else if host.ends_with(".messagingengine.com") {
    Some(("imap.fastmail.com", "smtp.fastmail.com", "fastmail"))
  } else if host.ends_with(".yandex.net") {
    Some(("imap.yandex.com", "smtp.yandex.com", "yandex"))
  } else {
    None
  }
}

/// Candidates derived from MX records (only hosted providers are detectable
/// this way; plain MTAs fall through to autoconfig and hostname guesses).
pub fn mx_candidates(mx_hosts: &[String]) -> Vec<MailCandidate> {
  let mut out = Vec::new();
  for host in mx_hosts {
    if let Some((imap_host, smtp_host, provider)) = classify_mx(host) {
      // A Workspace custom domain is *not* forced onto OAuth: whether IMAP
      // passwords are refused depends on the domain's admin policy, so the
      // server's own answer decides (and the refusal is classified with
      // Google-specific guidance).
      out.push(MailCandidate { imap_host: imap_host.into(), imap_port: 993, encryption: "ssl", provider: Some(provider), source: "mx", auth_method: "password", smtp_host: smtp_host.into(), smtp_port: 465 });
      break; // the lowest-preference (primary) MX is the meaningful one
    }
  }
  out
}

/// Hostname-pattern guesses for the remaining custom domains.
pub fn guess_candidates(domain: &str) -> Vec<MailCandidate> {
  let domain = domain.trim();
  if domain.is_empty() { return Vec::new(); }
  let mut out = Vec::new();
  for host in [format!("imap.{domain}"), format!("mail.{domain}")] {
    out.push(MailCandidate { imap_host: host.clone(), imap_port: 993, encryption: "ssl", provider: None, source: "guess", auth_method: "password", smtp_host: format!("smtp.{domain}"), smtp_port: 465 });
  }
  for host in [format!("imap.{domain}"), format!("mail.{domain}")] {
    out.push(MailCandidate { imap_host: host.clone(), imap_port: 143, encryption: "tls", provider: None, source: "guess", auth_method: "password", smtp_host: format!("smtp.{domain}"), smtp_port: 587 });
  }
  out
}

/// Fetches Thunderbird-compatible autoconfig for the domain — from Mozilla's
/// database, then from the domain's own well-known location. Both are HTTPS:
/// the former cleartext `http://autoconfig.<domain>/…` fallback was removed
/// (SEC-001) because a network attacker answering for it could point the
/// account at their own IMAP host and, at that point in the flow, receive the
/// user's mailbox password. Bounded per request; failures yield no candidates.
pub fn autoconfig_candidates(domain: &str) -> Vec<MailCandidate> {
  let domain = domain.trim().trim_start_matches('.').to_ascii_lowercase();
  if domain.is_empty() { return Vec::new(); }
  let urls = [
    format!("https://autoconfig.thunderbird.net/v1.1/{domain}"),
    format!("https://{domain}/.well-known/autoconfig/mail/config-v1.1.xml"),
  ];
  for url in urls {
    let fetched = ureq::get(&url).timeout(Duration::from_secs(6)).call().ok().and_then(|response| response.into_string().ok());
    if let Some(body) = fetched {
      let parsed = parse_autoconfig_xml(&body);
      if !parsed.is_empty() { return parsed; }
    }
  }
  Vec::new()
}

/// Extracts the email domain ("user@example.com" → "example.com").
pub fn domain_of(address: &str) -> String {
  address.trim().rsplit('@').next().unwrap_or("").trim().to_ascii_lowercase()
}

/// Extracts IMAP candidates from a Thunderbird `config-v1.1.xml`. Hand-rolled
/// on purpose: the document is small and flat, and this avoids a new XML
/// dependency. Only implicit-TLS/STARTTLS IMAP servers are considered — plain
/// is never auto-tried — and POP/SMTP sections are ignored here.
pub fn parse_autoconfig_xml(xml: &str) -> Vec<MailCandidate> {
  let mut out = Vec::new();
  for section in xml.split("<incomingServer") {
    let attributes = match section.split('>').next() { Some(header) => header, None => continue };
    if !attributes.contains("imap") { continue; }
    let block = match section.find("</incomingServer>") { Some(end) => &section[..end], None => continue };
    let hostname = match extract_tag(block, "hostname") { Some(value) => value, None => continue };
    let port = extract_tag(block, "port").and_then(|value| value.parse::<u16>().ok());
    let encryption = match extract_tag(block, "socketType").as_deref().map(str::to_ascii_lowercase).as_deref() {
      Some("ssl") => Some("ssl"),
      Some("starttls") | Some("tls") => Some("tls"),
      _ => None,
    };
    if let Some(encryption) = encryption {
      let default_port = if encryption == "ssl" { 993 } else { 143 };
      out.push(MailCandidate { imap_host: hostname, imap_port: port.unwrap_or(default_port), encryption, provider: None, source: "autoconfig", auth_method: "password", smtp_host: String::new(), smtp_port: 0 });
    }
  }
  out
}

/// Extracts the primary SMTP submission server from a `config-v1.1.xml` so a
/// discovered account stores the same settings Thunderbird would.
pub fn parse_autoconfig_smtp(xml: &str) -> Option<(String, u16)> {
  for section in xml.split("<outgoingServer") {
    let attributes = match section.split('>').next() { Some(header) => header, None => continue };
    if !attributes.contains("smtp") { continue; }
    let block = match section.find("</outgoingServer>") { Some(end) => &section[..end], None => continue };
    let hostname = extract_tag(block, "hostname")?;
    let port = extract_tag(block, "port").and_then(|value| value.parse::<u16>().ok());
    let socket = extract_tag(block, "socketType").map(|value| value.to_ascii_lowercase());
    // Prefer implicit-TLS submission (465); STARTTLS (587) is also fine since
    // delivery is a future phase and the settings are stored either way.
    if matches!(socket.as_deref(), Some("ssl") | Some("starttls") | Some("tls")) {
      return Some((hostname, port.unwrap_or(465)));
    }
  }
  None
}

fn extract_tag(block: &str, tag: &str) -> Option<String> {
  let open = format!("<{tag}>");
  let close = format!("</{tag}>");
  let start = block.find(&open)? + open.len();
  let end = start + block[start..].find(&close)?;
  let value = block[start..end].trim();
  if value.is_empty() { None } else { Some(value.to_string()) }
}

/// Builds the minimal Tokio runtime hickory's async resolver runs on. The IO
/// driver serves the DNS sockets; the timer driver is just as mandatory, because
/// the resolver wraps every name-server connection in `TokioTime::timeout`
/// (tokio's `time::timeout`). A Tokio context without timers panics with "A Tokio
/// 1.x context was found, but timers are disabled" — and here that panic is
/// fatal, not merely noisy: discovery runs inside the webview2 IPC callback on
/// the UI thread, whose `extern "system"` frame cannot unwind, so the process
/// aborted instead of failing the command (see `guard::guarded`).
pub fn resolver_runtime() -> std::io::Result<tokio::runtime::Runtime> {
  tokio::runtime::Builder::new_current_thread().enable_io().enable_time().build()
}

/// Resolves MX hosts for the domain using the system DNS configuration.
/// Network is optional: failures return an empty list and the remaining
/// candidate sources still run. hickory's async resolver runs on a minimal
/// local current-thread tokio runtime (see `resolver_runtime`) so the rest of
/// the app stays synchronous.
fn lookup_mx_hosts(domain: &str) -> Vec<String> {
  let runtime = match resolver_runtime() {
    Ok(runtime) => runtime,
    Err(_) => return Vec::new(),
  };
  runtime.block_on(async {
    let resolver = match hickory_resolver::TokioAsyncResolver::tokio_from_system_conf() {
      Ok(resolver) => resolver,
      Err(_) => return Vec::new(),
    };
    match resolver.lookup(domain, hickory_proto::rr::RecordType::MX).await {
      Ok(lookup) => lookup
        .record_iter()
        .filter_map(|record| match record.data() {
          Some(hickory_proto::rr::RData::MX(mx)) => Some(mx.exchange().to_string()),
          _ => None,
        })
        .collect(),
      Err(_) => Vec::new(),
    }
  })
}

/// Ordered, de-duplicated candidate list for the address. `mx_lookup` is
/// injected so the ranking logic is testable without a network.
pub fn discover_candidates(address: &str, mx_lookup: &dyn Fn(&str) -> Vec<String>) -> Vec<MailCandidate> {
  let domain = domain_of(address);
  if domain.is_empty() { return Vec::new(); }
  let mut out: Vec<MailCandidate> = Vec::new();
  let push = |mut candidate: MailCandidate, out: &mut Vec<MailCandidate>| {
    if out.len() >= MAX_CANDIDATES { return; }
    if candidate.smtp_host.is_empty() { candidate.smtp_host = format!("smtp.{domain}"); candidate.smtp_port = 465; }
    if !out.iter().any(|existing| existing.imap_host == candidate.imap_host && existing.imap_port == candidate.imap_port && existing.encryption == candidate.encryption) {
      out.push(candidate);
    }
  };
  for candidate in provider_candidates(&domain) { push(candidate, &mut out); }
  for candidate in mx_candidates(&mx_lookup(&domain)) { push(candidate, &mut out); }
  for candidate in autoconfig_candidates(&domain) { push(candidate, &mut out); }
  for candidate in guess_candidates(&domain) { push(candidate, &mut out); }
  out
}

/// The verified outcome of a discovery run.
pub struct Discovered { pub candidate: MailCandidate }

/// Authenticates the password against each discovered candidate until one
/// accepts. A server that answers and refuses the credentials stops the run
/// immediately: the configuration was right, so trying the same password on
/// other hosts would be noise — the classified error (with provider guidance)
/// is returned instead. Unreachable candidates and candidates that never
/// completed an IMAP login (a different service on the port, a gateway) are
/// skipped, so a wrong host cannot mask the right one.
pub fn discover_and_verify(address: &str, password: &str, progress: impl Fn(&str)) -> Result<Discovered, String> {
  let candidates = discover_candidates(address, &lookup_mx_hosts);
  if candidates.is_empty() {
    return Err("No mail server settings could be discovered for this address. Check the spelling, or open the manual setup below.".into());
  }
  let mut skipped: Vec<String> = Vec::new();
  for (index, candidate) in candidates.iter().enumerate() {
    // The method is part of what is being tried, so it belongs in the progress
    // text: "imap.gmail.com:993 (OAuth)" tells the user why a refused password
    // is not a typo.
    let method = if candidate.auth_method == "oauth2" { "OAuth" } else { "password" };
    progress(&format!("Checking {}:{} ({}, {} of {})…", candidate.imap_host, candidate.imap_port, method, index + 1, candidates.len()));
    match verify_imap_login(address, password, &candidate.imap_host, candidate.imap_port, candidate.encryption) {
      Ok(()) => return Ok(Discovered { candidate: candidate.clone() }),
      Err(VerifyFailure::Auth(hint)) => {
        // A provider that requires OAuth is not a wrong-password situation, so
        // the run stops with the route to the flow that can actually succeed
        // (the password attempt above still ran, so an App Password user is
        // never blocked by this).
        return Err(if candidate.auth_method == "oauth2" {
          format!("{} Sign in with Google (recommended), or use an App Password if you have created one.", VerifyFailure::Auth(hint).user_message())
        } else {
          VerifyFailure::Auth(hint).user_message()
        });
      }
      // A candidate that answers but never completes an IMAP login is the wrong
      // host (or a gateway in front of it), not proof that the password is
      // wrong — keep trying the remaining candidates.
      Err(VerifyFailure::Protocol(hint)) => skipped.push(format!("{}:{} ({})", candidate.imap_host, candidate.imap_port, hint)),
      Err(VerifyFailure::Unreachable(detail)) => skipped.push(format!("{}:{} ({})", candidate.imap_host, candidate.imap_port, detail)),
      // Cannot be produced by a password login (only the XOAUTH2 path reports
      // it), so it is handled exactly like an unusable candidate rather than
      // aborting the run.
      Err(VerifyFailure::GoogleAuth(hint)) => skipped.push(format!("{}:{} ({})", candidate.imap_host, candidate.imap_port, hint)),
    }
  }
  Err(format!(
    "None of the {} discovered mail servers completed an IMAP login ({}). Check the network connection, or open the manual setup below.",
    candidates.len(),
    skipped.first().map(String::as_str).unwrap_or("no details")
  ))
}