//! A small CalDAV (RFC 4791) and CardDAV (RFC 6352) client: discovery, listing, queries and
//! guarded writes. Objects come back as text; [`ical`] reads and edits them.

pub mod ical;
pub mod xml;

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use xml::Node;

#[derive(Debug, Clone)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
pub struct Account {
    /// Any URL on the server: its root, a `.well-known` path or a home collection.
    pub url: String,
    pub user: String,
    pub password: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Calendar,
    AddressBook,
}

#[derive(Debug, Clone)]
pub struct Collection {
    /// Absolute path on the server, ending in `/`.
    pub href: String,
    pub name: String,
    pub kind: Kind,
    /// `VEVENT`, `VTODO`, ... for calendars; empty when the server doesn't say.
    pub components: Vec<String>,
}

impl Collection {
    pub fn holds(&self, component: &str) -> bool {
        self.components.is_empty() || self.components.iter().any(|c| c == component)
    }

    /// The last path segment, a stable short id for tools.
    pub fn slug(&self) -> String {
        let seg = self
            .href
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default();
        percent_decode(seg)
    }
}

#[derive(Debug, Clone)]
pub struct Item {
    pub href: String,
    pub etag: Option<String>,
    pub data: String,
}

pub struct Client {
    http: reqwest::Client,
    account: Account,
    origin: String,
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn tls_config() -> rustls::ClientConfig {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS versions")
        .with_root_certificates(roots)
        .with_no_client_auth()
}

const PROP_NS: &str = r#"xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:a="urn:ietf:params:xml:ns:carddav" xmlns:cs="http://calendarserver.org/ns/""#;

struct Response {
    href: String,
    prop: Node,
}

/// The successful properties of every response in a multistatus body.
fn responses(body: &str) -> Result<Vec<Response>> {
    let root = xml::parse(body).map_err(Error)?;
    let mut out = Vec::new();
    for r in root.children_named("response") {
        let Some(href) = r.child("href").map(|h| h.text.trim().to_owned()) else {
            continue;
        };
        let mut prop = Node {
            name: "prop".into(),
            ..Default::default()
        };
        for ps in r.children_named("propstat") {
            let ok = ps.child("status").is_none_or(|s| s.text.contains(" 200"));
            if ok && let Some(p) = ps.child("prop") {
                prop.children.extend(p.children.iter().cloned());
            }
        }
        out.push(Response { href, prop });
    }
    Ok(out)
}

impl Client {
    pub fn new(account: Account) -> Result<Self> {
        let url = account.url.trim();
        let scheme_end = url
            .find("://")
            .ok_or_else(|| Error(format!("not a URL: {url}")))?;
        let host_end = url[scheme_end + 3..]
            .find('/')
            .map_or(url.len(), |i| i + scheme_end + 3);
        let origin = url[..host_end].to_owned();
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls_config())
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("chance-of-pim/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error(format!("HTTP client: {e}")))?;
        Ok(Self {
            http,
            account,
            origin,
        })
    }

    fn absolute(&self, href: &str) -> String {
        if href.contains("://") {
            href.to_owned()
        } else {
            format!("{}{}", self.origin, href)
        }
    }

    /// The path part of an href, so hrefs compare whether or not the server sent a full URL.
    fn path(&self, href: &str) -> String {
        match href.find("://") {
            Some(i) => href[i + 3..]
                .find('/')
                .map_or("/".to_owned(), |j| href[i + 3 + j..].to_owned()),
            None => href.to_owned(),
        }
    }

    async fn send(
        &self,
        method: &str,
        href: &str,
        headers: &[(&str, &str)],
        body: Option<String>,
    ) -> Result<(u16, Option<String>, String)> {
        let method = reqwest::Method::from_bytes(method.as_bytes()).expect("valid method");
        let mut req = self
            .http
            .request(method.clone(), self.absolute(href))
            .basic_auth(&self.account.user, Some(&self.account.password));
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        if let Some(b) = body {
            req = req.body(b);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| Error(format!("{method} {href}: {e}")))?;
        let status = resp.status().as_u16();
        let etag = resp
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let text = resp
            .text()
            .await
            .map_err(|e| Error(format!("{method} {href}: {e}")))?;
        Ok((status, etag, text))
    }

    async fn propfind(&self, href: &str, depth: &str, props: &str) -> Result<Vec<Response>> {
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><d:propfind {PROP_NS}><d:prop>{props}</d:prop></d:propfind>"#
        );
        let (status, _, text) = self
            .send(
                "PROPFIND",
                href,
                &[
                    ("Depth", depth),
                    ("Content-Type", "application/xml; charset=utf-8"),
                ],
                Some(body),
            )
            .await?;
        match status {
            207 => responses(&text),
            401 | 403 => Err(Error(format!(
                "the server refused the sign-in (HTTP {status}) for {href}"
            ))),
            _ => Err(Error(format!("PROPFIND {href}: HTTP {status}"))),
        }
    }

    async fn report(&self, href: &str, body: String) -> Result<Vec<Item>> {
        let (status, _, text) = self
            .send(
                "REPORT",
                href,
                &[
                    ("Depth", "1"),
                    ("Content-Type", "application/xml; charset=utf-8"),
                ],
                Some(body),
            )
            .await?;
        if status != 207 {
            return Err(Error(format!("REPORT {href}: HTTP {status}")));
        }
        Ok(responses(&text)?
            .into_iter()
            .filter_map(|r| {
                let data = r
                    .prop
                    .child("calendar-data")
                    .or_else(|| r.prop.child("address-data"))?;
                Some(Item {
                    href: self.path(&r.href),
                    etag: r.prop.child("getetag").map(|e| e.text.trim().to_owned()),
                    data: data.text.clone(),
                })
            })
            .collect())
    }

    /// The home collection for calendars or address books, found from the account URL.
    pub async fn home(&self, kind: Kind) -> Result<String> {
        let home_prop = match kind {
            Kind::Calendar => "calendar-home-set",
            Kind::AddressBook => "addressbook-home-set",
        };
        let props = "<d:current-user-principal/><c:calendar-home-set/><a:addressbook-home-set/>";
        let start = self.path(&self.account.url);
        let first = self.propfind(&start, "0", props).await?;
        let href_of = |rs: &[Response], name: &str| {
            rs.iter().find_map(|r| {
                r.prop
                    .child(name)?
                    .child("href")
                    .map(|h| h.text.trim().to_owned())
            })
        };
        if let Some(h) = href_of(&first, home_prop) {
            return Ok(self.path(&h));
        }
        let principal = href_of(&first, "current-user-principal").ok_or_else(|| {
            Error(format!(
                "{}: the server names no principal for this account",
                self.account.url
            ))
        })?;
        let second = self.propfind(&self.path(&principal), "0", props).await?;
        href_of(&second, home_prop)
            .map(|h| self.path(&h))
            .ok_or_else(|| {
                Error(format!(
                    "{}: no {home_prop} for this account",
                    self.account.url
                ))
            })
    }

    /// The account's calendars or address books.
    pub async fn collections(&self, kind: Kind) -> Result<Vec<Collection>> {
        let home = self.home(kind).await?;
        let rs = self
            .propfind(
                &home,
                "1",
                "<d:displayname/><d:resourcetype/><c:supported-calendar-component-set/>",
            )
            .await?;
        let want = match kind {
            Kind::Calendar => "calendar",
            Kind::AddressBook => "addressbook",
        };
        let mut out: Vec<Collection> = rs
            .into_iter()
            .filter(|r| {
                r.prop
                    .child("resourcetype")
                    .is_some_and(|t| t.child(want).is_some())
            })
            .map(|r| {
                let mut href = self.path(&r.href);
                if !href.ends_with('/') {
                    href.push('/');
                }
                let components = r
                    .prop
                    .child("supported-calendar-component-set")
                    .map(|s| {
                        s.children_named("comp")
                            .filter_map(|c| c.attr("name").map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let mut c = Collection {
                    href,
                    name: String::new(),
                    kind,
                    components,
                };
                c.name = r
                    .prop
                    .child("displayname")
                    .map(|d| d.text.trim().to_owned())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| c.slug());
                c
            })
            .collect();
        out.sort_by_key(|a| a.name.to_lowercase());
        Ok(out)
    }

    /// Events (`VEVENT`) or tasks (`VTODO`) in a calendar. `range` is a pair of UTC stamps
    /// (`20261110T000000Z`); with `expand`, recurring events come back as single occurrences.
    pub async fn calendar_query(
        &self,
        calendar: &str,
        component: &str,
        range: Option<(&str, &str)>,
        expand: bool,
    ) -> Result<Vec<Item>> {
        let time = range.map(|(s, e)| {
            format!(
                r#"<c:time-range start="{}" end="{}"/>"#,
                xml::escape(s),
                xml::escape(e)
            )
        });
        let data = match (expand, range) {
            (true, Some((s, e))) => format!(
                r#"<c:calendar-data><c:expand start="{}" end="{}"/></c:calendar-data>"#,
                xml::escape(s),
                xml::escape(e)
            ),
            _ => "<c:calendar-data/>".to_owned(),
        };
        let filter = match time {
            Some(t) => format!(r#"<c:comp-filter name="{component}">{t}</c:comp-filter>"#),
            None => format!(r#"<c:comp-filter name="{component}"/>"#),
        };
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><c:calendar-query {PROP_NS}><d:prop><d:getetag/>{data}</d:prop><c:filter><c:comp-filter name="VCALENDAR">{filter}</c:comp-filter></c:filter></c:calendar-query>"#
        );
        self.report(calendar, body).await
    }

    /// Every card in an address book.
    pub async fn cards(&self, book: &str) -> Result<Vec<Item>> {
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><a:addressbook-query {PROP_NS}><d:prop><d:getetag/><a:address-data/></d:prop><a:filter/></a:addressbook-query>"#
        );
        self.report(book, body).await
    }

    /// Creates a calendar under the account's calendar home and returns its path.
    /// `components` are the kinds it accepts (`VEVENT`, `VTODO`).
    pub async fn create_calendar(
        &self,
        slug: &str,
        name: &str,
        components: &[&str],
    ) -> Result<String> {
        let mut home = self.home(Kind::Calendar).await?;
        if !home.ends_with('/') {
            home.push('/');
        }
        let href = format!("{home}{slug}/");
        let comps: String = components
            .iter()
            .map(|c| format!(r#"<c:comp name="{c}"/>"#))
            .collect();
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><c:mkcalendar {PROP_NS}><d:set><d:prop><d:displayname>{}</d:displayname><c:supported-calendar-component-set>{comps}</c:supported-calendar-component-set></d:prop></d:set></c:mkcalendar>"#,
            xml::escape(name)
        );
        let (status, _, text) = self
            .send(
                "MKCALENDAR",
                &href,
                &[("Content-Type", "application/xml; charset=utf-8")],
                Some(body),
            )
            .await?;
        match status {
            200 | 201 => Ok(href),
            405 | 409 => Err(Error(format!(
                "a calendar already exists at {href}; choose another name"
            ))),
            _ => Err(Error(format!(
                "MKCALENDAR {href}: HTTP {status} {}",
                text.chars().take(200).collect::<String>()
            ))),
        }
    }

    /// Deletes a whole collection and everything in it.
    pub async fn delete_collection(&self, href: &str) -> Result<()> {
        if !href.ends_with('/') {
            return Err(Error(format!("{href} is not a collection")));
        }
        let (status, _, _) = self.send("DELETE", href, &[], None).await?;
        match status {
            200 | 204 => Ok(()),
            404 => Err(Error(format!("{href} was not found"))),
            _ => Err(Error(format!("DELETE {href}: HTTP {status}"))),
        }
    }

    /// Moves one object to another collection on the same server; never overwrites.
    pub async fn move_item(&self, from: &str, to: &str) -> Result<()> {
        let dest = self.absolute(to);
        let (status, _, text) = self
            .send(
                "MOVE",
                from,
                &[("Destination", &dest), ("Overwrite", "F")],
                None,
            )
            .await?;
        match status {
            201 | 204 => Ok(()),
            404 => Err(Error(format!("{from} was not found"))),
            412 => Err(Error(format!("something already exists at {to}"))),
            _ => Err(Error(format!(
                "MOVE {from}: HTTP {status} {}",
                text.chars().take(200).collect::<String>()
            ))),
        }
    }

    pub async fn get(&self, href: &str) -> Result<Item> {
        let (status, etag, data) = self.send("GET", href, &[], None).await?;
        match status {
            200 => Ok(Item {
                href: href.to_owned(),
                etag,
                data,
            }),
            404 => Err(Error(format!("{href} was not found"))),
            _ => Err(Error(format!("GET {href}: HTTP {status}"))),
        }
    }

    /// Creates (`etag` None: refuses to overwrite) or replaces (`etag` Some: only that version).
    pub async fn put(
        &self,
        href: &str,
        content_type: &str,
        data: String,
        etag: Option<&str>,
    ) -> Result<Option<String>> {
        let guard = match etag {
            Some(e) => ("If-Match", e),
            None => ("If-None-Match", "*"),
        };
        let (status, new_etag, text) = self
            .send(
                "PUT",
                href,
                &[("Content-Type", content_type), guard],
                Some(data),
            )
            .await?;
        match status {
            200 | 201 | 204 => Ok(new_etag),
            412 => Err(Error("it changed on the server since it was read (or already exists); read it again and retry".into())),
            _ => Err(Error(format!("PUT {href}: HTTP {status} {}", text.chars().take(200).collect::<String>()))),
        }
    }

    pub async fn delete(&self, href: &str, etag: Option<&str>) -> Result<()> {
        let headers: Vec<(&str, &str)> = etag.map(|e| ("If-Match", e)).into_iter().collect();
        let (status, _, _) = self.send("DELETE", href, &headers, None).await?;
        match status {
            200 | 204 => Ok(()),
            404 => Err(Error(format!("{href} was not found"))),
            412 => Err(Error(
                "it changed on the server since it was read; read it again and retry".into(),
            )),
            _ => Err(Error(format!("DELETE {href}: HTTP {status}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_slugs() {
        let c = Collection {
            href: "/dav/cal/a%40b.c/my%20cal/".into(),
            name: String::new(),
            kind: Kind::Calendar,
            components: vec!["VTODO".into()],
        };
        assert_eq!(c.slug(), "my cal");
        assert!(c.holds("VTODO") && !c.holds("VEVENT"));
    }
}
