//! IMAP (read, search, flags, move, append) and SMTP (send) for Chance of PIM.
//!
//! One short-lived IMAP session per tool call: simple and stateless; a pool and the local
//! index come later (PROGRESS.md).

use std::fmt;
use std::sync::{Arc, OnceLock};

use futures_util::TryStreamExt;
use lettre::message::Mailbox as Address;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use mail_parser::MessageParser;

pub use lettre::Message as Outgoing;

#[derive(Debug, Clone)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn ctx<E: fmt::Display>(what: &'static str) -> impl Fn(E) -> Error {
    move |e| Error(format!("{what}: {e}"))
}

/// Where and how to sign in to one IMAP or SMTP server.
#[derive(Debug, Clone)]
pub struct Server {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}

type Tls = tokio_rustls::client::TlsStream<tokio::net::TcpStream>;
pub type Session = async_imap::Session<Tls>;

fn tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .expect("ring supports the default TLS versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

/// Opens an implicit-TLS IMAP session and signs in.
pub async fn connect(server: &Server) -> Result<Session> {
    let tcp = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::net::TcpStream::connect((server.host.as_str(), server.port)),
    )
    .await
    .map_err(|_| Error(format!("IMAP connect to {} timed out", server.host)))?
    .map_err(ctx("IMAP connect"))?;
    let name = rustls::pki_types::ServerName::try_from(server.host.clone())
        .map_err(ctx("IMAP host name"))?;
    let tls = tokio_rustls::TlsConnector::from(tls_config())
        .connect(name, tcp)
        .await
        .map_err(ctx("IMAP TLS"))?;
    let mut client = async_imap::Client::new(tls);
    client
        .read_response()
        .await
        .map_err(ctx("IMAP greeting"))?
        .ok_or_else(|| Error("IMAP server closed the connection".into()))?;
    client
        .login(&server.user, &server.password)
        .await
        .map_err(|(e, _)| Error(format!("IMAP sign-in: {e}")))
}

#[derive(Debug, Clone)]
pub struct MailboxInfo {
    pub name: String,
    /// `trash`, `sent`, `drafts`, `junk`, `archive`, `inbox`, from SPECIAL-USE or the name.
    pub role: Option<&'static str>,
    pub selectable: bool,
    pub total: Option<u32>,
    pub unseen: Option<u32>,
}

fn role_of(name: &str, attrs: &str) -> Option<&'static str> {
    for (needle, role) in [
        ("Trash", "trash"),
        ("Sent", "sent"),
        ("Drafts", "drafts"),
        ("Junk", "junk"),
        ("Archive", "archive"),
    ] {
        if attrs.contains(needle) {
            return Some(role);
        }
    }
    match name.to_ascii_lowercase().as_str() {
        "inbox" => Some("inbox"),
        "trash" | "deleted items" | "deleted messages" => Some("trash"),
        "sent" | "sent items" | "sent messages" => Some("sent"),
        "drafts" => Some("drafts"),
        "spam" | "junk" | "junk mail" => Some("junk"),
        "archive" => Some("archive"),
        _ => None,
    }
}

pub async fn list_mailboxes(sess: &mut Session, with_counts: bool) -> Result<Vec<MailboxInfo>> {
    let names: Vec<_> = sess
        .list(Some(""), Some("*"))
        .await
        .map_err(ctx("LIST"))?
        .try_collect()
        .await
        .map_err(ctx("LIST"))?;
    let mut out = Vec::new();
    for n in &names {
        let attrs = format!("{:?}", n.attributes());
        let selectable = !attrs.contains("NoSelect") && !attrs.contains("NonExistent");
        out.push(MailboxInfo {
            name: n.name().to_owned(),
            role: role_of(n.name(), &attrs),
            selectable,
            total: None,
            unseen: None,
        });
    }
    if with_counts {
        for m in out.iter_mut().filter(|m| m.selectable) {
            if let Ok(st) = sess.status(&m.name, "(MESSAGES UNSEEN)").await {
                m.total = Some(st.exists);
                m.unseen = st.unseen;
            }
        }
    }
    out.sort_by(|a, b| (a.role != Some("inbox"), &a.name).cmp(&(b.role != Some("inbox"), &b.name)));
    Ok(out)
}

/// The mailbox with a role (`trash`, `sent`, `drafts`), if the account has one.
pub async fn role_mailbox(sess: &mut Session, role: &str) -> Result<String> {
    let boxes = list_mailboxes(sess, false).await?;
    boxes
        .into_iter()
        .find(|m| m.role == Some(role) && m.selectable)
        .map(|m| m.name)
        .ok_or_else(|| Error(format!("this account has no {role} mailbox")))
}

#[derive(Debug, Clone, Default)]
pub struct Query {
    pub text: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub subject: Option<String>,
    /// `YYYY-MM-DD`, inclusive.
    pub since: Option<String>,
    /// `YYYY-MM-DD`, exclusive.
    pub before: Option<String>,
    pub unseen: bool,
    pub flagged: bool,
}

fn quote(s: &str) -> String {
    let cleaned: String = s.chars().filter(|c| !c.is_control()).collect();
    format!("\"{}\"", cleaned.replace('\\', "\\\\").replace('"', "\\\""))
}

fn imap_date(ymd: &str) -> Result<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let parts: Vec<&str> = ymd.get(..10).unwrap_or(ymd).split('-').collect();
    let bad = || Error(format!("date must be YYYY-MM-DD, got {ymd:?}"));
    if parts.len() != 3 {
        return Err(bad());
    }
    let y: u32 = parts[0].parse().map_err(|_| bad())?;
    let m: usize = parts[1].parse().map_err(|_| bad())?;
    let d: u32 = parts[2].parse().map_err(|_| bad())?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(bad());
    }
    Ok(format!("{d}-{}-{y}", MONTHS[m - 1]))
}

impl Query {
    fn to_imap(&self) -> Result<String> {
        let mut parts = Vec::new();
        let mut utf8 = false;
        let mut add = |key: &str, v: &Option<String>| {
            if let Some(v) = v.as_deref().filter(|v| !v.trim().is_empty()) {
                utf8 |= !v.is_ascii();
                parts.push(format!("{key} {}", quote(v.trim())));
            }
        };
        add("TEXT", &self.text);
        add("FROM", &self.from);
        add("TO", &self.to);
        add("SUBJECT", &self.subject);
        if let Some(s) = &self.since {
            parts.push(format!("SINCE {}", imap_date(s)?));
        }
        if let Some(s) = &self.before {
            parts.push(format!("BEFORE {}", imap_date(s)?));
        }
        if self.unseen {
            parts.push("UNSEEN".into());
        }
        if self.flagged {
            parts.push("FLAGGED".into());
        }
        if parts.is_empty() {
            parts.push("ALL".into());
        }
        let q = parts.join(" ");
        Ok(if utf8 {
            format!("CHARSET UTF-8 {q}")
        } else {
            q
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub uid: u32,
    pub date: Option<String>,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub flags: Vec<String>,
    pub size: Option<u32>,
}

fn addresses(list: Option<mail_parser::AddressList<'_>>) -> String {
    let Some(list) = list else {
        return String::new();
    };
    list.mailboxes()
        .map(|m| match (m.name(), m.address()) {
            (Some(n), Some(a)) => format!("{n} <{a}>"),
            (None, Some(a)) => a.to_owned(),
            (Some(n), None) => n.to_owned(),
            (None, None) => String::new(),
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn flag_names(fetch: &async_imap::types::Fetch) -> Vec<String> {
    fetch
        .flags()
        .map(|f| match f {
            async_imap::types::Flag::Custom(c) => c.to_string(),
            other => format!("{other:?}").to_ascii_lowercase(),
        })
        .filter(|f| f != "recent")
        .collect()
}

fn uid_set(uids: &[u32]) -> String {
    uids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Newest first. Returns the page and how many messages matched in all.
pub async fn search(
    sess: &mut Session,
    mailbox: &str,
    query: &Query,
    limit: usize,
    offset: usize,
) -> Result<(Vec<Summary>, usize)> {
    sess.examine(mailbox).await.map_err(ctx("open mailbox"))?;
    let mut uids: Vec<u32> = sess
        .uid_search(query.to_imap()?)
        .await
        .map_err(ctx("SEARCH"))?
        .into_iter()
        .collect();
    uids.sort_unstable_by_key(|&u| std::cmp::Reverse(u));
    let total = uids.len();
    let page: Vec<u32> = uids.into_iter().skip(offset).take(limit).collect();
    if page.is_empty() {
        return Ok((Vec::new(), total));
    }
    let fetched: Vec<_> = sess
        .uid_fetch(uid_set(&page), "(UID FLAGS RFC822.SIZE BODY.PEEK[HEADER])")
        .await
        .map_err(ctx("FETCH"))?
        .try_collect()
        .await
        .map_err(ctx("FETCH"))?;
    let parser = MessageParser::new();
    let mut out: Vec<Summary> = fetched
        .iter()
        .filter_map(|f| {
            let uid = f.uid?;
            let mut s = Summary {
                uid,
                flags: flag_names(f),
                size: f.size,
                ..Default::default()
            };
            if let Some(msg) = f.header().and_then(|h| parser.parse_headers(h)) {
                s.date = msg.date().map(|d| d.to_rfc3339());
                s.from = addresses(msg.from());
                s.to = addresses(msg.to());
                s.subject = msg.subject().unwrap_or_default().to_owned();
            }
            Some(s)
        })
        .collect();
    out.sort_unstable_by_key(|m| std::cmp::Reverse(m.uid));
    Ok((out, total))
}

#[derive(Debug, Clone, Default)]
pub struct Attachment {
    pub index: usize,
    pub name: String,
    pub content_type: String,
    pub size: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Full {
    pub uid: u32,
    pub flags: Vec<String>,
    pub date: Option<String>,
    pub from: String,
    pub to: String,
    pub cc: String,
    pub reply_to: String,
    pub subject: String,
    pub message_id: Option<String>,
    pub references: Vec<String>,
    pub text: String,
    pub attachments: Vec<Attachment>,
}

/// The whole message as stored, without marking it read.
pub async fn fetch_raw(
    sess: &mut Session,
    mailbox: &str,
    uid: u32,
) -> Result<(Vec<u8>, Vec<String>)> {
    sess.examine(mailbox).await.map_err(ctx("open mailbox"))?;
    let fetched: Vec<_> = sess
        .uid_fetch(uid.to_string(), "(UID FLAGS BODY.PEEK[])")
        .await
        .map_err(ctx("FETCH"))?
        .try_collect()
        .await
        .map_err(ctx("FETCH"))?;
    let f = fetched
        .iter()
        .find(|f| f.uid == Some(uid))
        .ok_or_else(|| Error(format!("no message with uid {uid} in {mailbox}")))?;
    let body = f
        .body()
        .ok_or_else(|| Error("the server returned no message body".into()))?;
    Ok((body.to_vec(), flag_names(f)))
}

fn content_type_of(part: &mail_parser::MessagePart<'_>) -> String {
    part.content_type()
        .map(|c| match c.subtype() {
            Some(s) => format!("{}/{}", c.ctype(), s),
            None => c.ctype().to_owned(),
        })
        .unwrap_or_else(|| "application/octet-stream".into())
}

pub fn parse_full(uid: u32, flags: Vec<String>, raw: &[u8]) -> Result<Full> {
    let msg = MessageParser::new()
        .parse(raw)
        .ok_or_else(|| Error("the message could not be parsed".into()))?;
    let text = msg
        .body_text(0)
        .map(|t| t.into_owned())
        .or_else(|| msg.body_html(0).map(|h| mail_parser::html_to_text(&h)))
        .unwrap_or_default();
    let mut references: Vec<String> = Vec::new();
    if let Some(r) = msg.references() {
        references.extend(r.iter().map(str::to_owned));
    }
    if let Some(r) = msg.in_reply_to() {
        for id in r.iter() {
            if !references.iter().any(|x| x == id) {
                references.push(id.to_owned());
            }
        }
    }
    let attachments = msg
        .attachments()
        .enumerate()
        .map(|(index, p)| Attachment {
            index,
            name: p.attachment_name().unwrap_or("(unnamed)").to_owned(),
            content_type: content_type_of(&p),
            size: p.decoded_len(),
        })
        .collect();
    Ok(Full {
        uid,
        flags,
        date: msg.date().map(|d| d.to_rfc3339()),
        from: addresses(msg.from()),
        to: addresses(msg.to()),
        cc: addresses(msg.cc()),
        reply_to: addresses(msg.reply_to()),
        subject: msg.subject().unwrap_or_default().to_owned(),
        message_id: msg.message_id().map(str::to_owned),
        references,
        text,
        attachments,
    })
}

/// One attachment, by its index in [`Full::attachments`]: what it is, its bytes, and its text
/// decoded from its charset when the part is text.
pub fn attachment(raw: &[u8], index: usize) -> Result<(Attachment, Vec<u8>, Option<String>)> {
    let msg = MessageParser::new()
        .parse(raw)
        .ok_or_else(|| Error("the message could not be parsed".into()))?;
    let count = msg.attachments().count();
    let part = msg.attachments().nth(index).ok_or_else(|| {
        Error(match count {
            0 => "the message has no attachments".to_owned(),
            n => format!("the message has no attachment {index}; it has {n}, numbered from 0"),
        })
    })?;
    let bytes = part.decoded().into_owned();
    let text = part.text().map(|t| t.into_owned());
    Ok((
        Attachment {
            index,
            name: part.attachment_name().unwrap_or("(unnamed)").to_owned(),
            content_type: content_type_of(&part),
            size: bytes.len(),
        },
        bytes,
        text,
    ))
}

/// `change` is an IMAP STORE item such as `+FLAGS (\Seen)`.
pub async fn store(sess: &mut Session, mailbox: &str, uids: &[u32], change: &str) -> Result<()> {
    sess.select(mailbox).await.map_err(ctx("open mailbox"))?;
    let _: Vec<_> = sess
        .uid_store(uid_set(uids), change)
        .await
        .map_err(ctx("STORE"))?
        .try_collect()
        .await
        .map_err(ctx("STORE"))?;
    Ok(())
}

pub async fn move_to(sess: &mut Session, mailbox: &str, uids: &[u32], dest: &str) -> Result<()> {
    sess.select(mailbox).await.map_err(ctx("open mailbox"))?;
    sess.uid_mv(uid_set(uids), dest).await.map_err(ctx("MOVE"))
}

pub async fn append(
    sess: &mut Session,
    mailbox: &str,
    flags: Option<&str>,
    raw: &[u8],
) -> Result<()> {
    sess.append(mailbox, flags, None, raw)
        .await
        .map_err(ctx("APPEND"))
}

/// A message to draft or send.
#[derive(Debug, Clone, Default)]
pub struct Compose {
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub body: String,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
}

fn angle(id: &str) -> String {
    let id = id.trim();
    if id.starts_with('<') {
        id.to_owned()
    } else {
        format!("<{id}>")
    }
}

/// Builds the RFC 5322 message. `keep_bcc` for drafts, so the Bcc line survives in the copy.
pub fn build(c: &Compose, keep_bcc: bool) -> Result<Outgoing> {
    let addr = |s: &String| {
        s.parse::<Address>()
            .map_err(|e| Error(format!("address {s:?}: {e}")))
    };
    let mut b = Outgoing::builder()
        .from(addr(&c.from)?)
        .subject(c.subject.clone());
    for a in &c.to {
        b = b.to(addr(a)?);
    }
    for a in &c.cc {
        b = b.cc(addr(a)?);
    }
    for a in &c.bcc {
        b = b.bcc(addr(a)?);
    }
    if keep_bcc {
        b = b.keep_bcc();
    }
    if let Some(id) = &c.in_reply_to {
        b = b.in_reply_to(angle(id));
    }
    if !c.references.is_empty() {
        b = b.references(
            c.references
                .iter()
                .map(|r| angle(r))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    b.header(lettre::message::header::ContentType::TEXT_PLAIN)
        .body(c.body.clone())
        .map_err(ctx("build message"))
}

/// Every envelope recipient of a built message, lower-cased.
pub fn recipients(c: &Compose) -> Vec<String> {
    c.to.iter()
        .chain(&c.cc)
        .chain(&c.bcc)
        .filter_map(|s| s.parse::<Address>().ok())
        .map(|a| a.email.to_string().to_ascii_lowercase())
        .collect()
}

/// The bare address of a `Name <addr>` string, lower-cased.
pub fn bare_address(s: &str) -> Option<String> {
    s.parse::<Address>()
        .ok()
        .map(|a| a.email.to_string().to_ascii_lowercase())
}

/// Sends over SMTP: implicit TLS on 465, STARTTLS on any other port.
pub async fn send(server: &Server, msg: Outgoing) -> Result<()> {
    let builder = if server.port == 465 {
        AsyncSmtpTransport::<Tokio1Executor>::relay(&server.host)
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&server.host)
    }
    .map_err(ctx("SMTP setup"))?;
    let transport = builder
        .port(server.port)
        .credentials(lettre::transport::smtp::authentication::Credentials::new(
            server.user.clone(),
            server.password.clone(),
        ))
        .timeout(Some(std::time::Duration::from_secs(60)))
        .build();
    transport
        .send(msg)
        .await
        .map(|_| ())
        .map_err(ctx("SMTP send"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_to_imap() {
        let q = Query {
            from: Some("ann@example.com".into()),
            since: Some("2026-11-01".into()),
            unseen: true,
            ..Default::default()
        };
        assert_eq!(
            q.to_imap().unwrap(),
            "FROM \"ann@example.com\" SINCE 1-Nov-2026 UNSEEN"
        );
        assert_eq!(Query::default().to_imap().unwrap(), "ALL");
    }

    #[test]
    fn parses_a_message() {
        let raw = b"From: Ann <ann@example.com>\r\nTo: bob@example.com\r\nSubject: Hi\r\nMessage-ID: <1@x>\r\n\r\nHello!\r\n";
        let full = parse_full(7, vec![], raw).unwrap();
        assert_eq!(full.from, "Ann <ann@example.com>");
        assert_eq!(full.subject, "Hi");
        assert_eq!(full.message_id.as_deref(), Some("1@x"));
        assert!(full.text.starts_with("Hello!"));
    }

    #[test]
    fn builds_a_reply() {
        let c = Compose {
            from: "Ty <ty@example.com>".into(),
            to: vec!["ann@example.com".into()],
            subject: "Re: Hi".into(),
            body: "Thanks".into(),
            in_reply_to: Some("1@x".into()),
            references: vec!["1@x".into()],
            ..Default::default()
        };
        let text = String::from_utf8(build(&c, false).unwrap().formatted()).unwrap();
        assert!(text.contains("In-Reply-To: <1@x>"));
        assert_eq!(recipients(&c), vec!["ann@example.com"]);
    }
}
