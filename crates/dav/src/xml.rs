//! A tiny namespace-blind XML tree: enough to read WebDAV multistatus bodies.

use quick_xml::Reader;
use quick_xml::events::Event;

#[derive(Debug, Clone, Default)]
pub struct Node {
    /// Local name, without prefix.
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
    pub text: String,
}

impl Node {
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// First descendant with that name, depth first.
    pub fn find(&self, name: &str) -> Option<&Node> {
        self.children.iter().find_map(|c| {
            if c.name == name {
                Some(c)
            } else {
                c.find(name)
            }
        })
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

fn local(raw: &str) -> String {
    raw.rsplit(':').next().unwrap_or_default().to_owned()
}

fn start(e: &quick_xml::events::BytesStart<'_>) -> Node {
    let mut node = Node {
        name: local(e.name().as_ref()),
        ..Default::default()
    };
    for a in e.attributes().flatten() {
        let value = a.value.to_string();
        node.attrs.push((
            local(a.key.as_ref()),
            quick_xml::escape::unescape(&value)
                .map(|v| v.into_owned())
                .unwrap_or(value),
        ));
    }
    node
}

pub fn parse(xml: &str) -> Result<Node, String> {
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<Node> = vec![Node::default()];
    loop {
        match reader.read_event().map_err(|e| format!("XML: {e}"))? {
            Event::Start(e) => stack.push(start(&e)),
            Event::Empty(e) => {
                let node = start(&e);
                stack.last_mut().expect("root").children.push(node);
            }
            Event::End(_) => {
                let done = stack.pop().ok_or("XML: unbalanced end tag")?;
                stack
                    .last_mut()
                    .ok_or("XML: unbalanced end tag")?
                    .children
                    .push(done);
            }
            Event::Text(t) => {
                stack.last_mut().expect("root").text.push_str(&t);
            }
            Event::CData(t) => {
                stack.last_mut().expect("root").text.push_str(&t);
            }
            Event::GeneralRef(r) => {
                let name: &str = &r;
                let ch = match name {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ => r.resolve_char_ref().ok().flatten(),
                };
                if let Some(c) = ch {
                    stack.last_mut().expect("root").text.push(c);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let mut root = stack.pop().ok_or("XML: empty")?;
    if !stack.is_empty() {
        return Err("XML: unclosed element".into());
    }
    root.children
        .pop()
        .ok_or_else(|| "XML: no root element".to_owned())
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_a_multistatus() {
        let root = super::parse("<?xml version=\"1.0\"?><d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>/a%20b/</d:href><d:propstat><d:prop><d:displayname>Tom &amp; Ann&apos;s</d:displayname><d:resourcetype><d:collection/><c:calendar xmlns:c=\"x\"/></d:resourcetype><c:set xmlns:c=\"x\"><c:comp name=\"VTODO\"/></c:set></d:prop></d:propstat></d:response></d:multistatus>").unwrap();
        assert_eq!(root.name, "multistatus");
        let prop = root.find("prop").unwrap();
        assert_eq!(prop.child("displayname").unwrap().text, "Tom & Ann's");
        assert!(
            prop.child("resourcetype")
                .unwrap()
                .child("calendar")
                .is_some()
        );
        assert_eq!(prop.find("comp").unwrap().attr("name"), Some("VTODO"));
    }
}
