//! Content lines shared by iCalendar (RFC 5545) and vCard (RFC 6350): a lossless tree, so an
//! edit changes the properties it names and writes every other line back as it was.

use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// vCard group prefix (`item1` in `item1.EMAIL`), if any.
    pub group: Option<String>,
    /// Upper-cased property name.
    pub name: String,
    /// Raw parameters including the leading `;`, or empty.
    pub params: String,
    /// Raw value, still escaped.
    pub value: String,
}

impl Line {
    pub fn new(name: &str, params: &str, value: impl Into<String>) -> Self {
        Self {
            group: None,
            name: name.to_ascii_uppercase(),
            params: params.to_owned(),
            value: value.into(),
        }
    }

    /// A parameter's value, matched case-insensitively, without quotes.
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params
            .split(';')
            .filter(|p| !p.is_empty())
            .find_map(|p| {
                let (k, v) = p.split_once('=')?;
                k.eq_ignore_ascii_case(key).then(|| v.trim_matches('"'))
            })
    }

    /// The value with text escapes undone.
    pub fn text(&self) -> String {
        unescape(&self.value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Component {
    pub name: String,
    pub lines: Vec<Line>,
    pub children: Vec<Component>,
}

pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') | Some('N') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Splits `NAME;PARAMS:VALUE`, honouring quoted parameter values.
fn split_line(raw: &str) -> Option<Line> {
    let mut quoted = false;
    let mut name_end = None;
    let mut colon = None;
    for (i, c) in raw.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted && name_end.is_none() => name_end = Some(i),
            ':' if !quoted => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let name_end = name_end.filter(|&n| n < colon).unwrap_or(colon);
    let full = &raw[..name_end];
    let (group, name) = match full.rsplit_once('.') {
        Some((g, n)) => (Some(g.to_owned()), n),
        None => (None, full),
    };
    Some(Line {
        group,
        name: name.to_ascii_uppercase(),
        params: raw[name_end..colon].to_owned(),
        value: raw[colon + 1..].to_owned(),
    })
}

/// Parses one object; the result is its outermost component (`VCALENDAR` or `VCARD`).
pub fn parse(text: &str) -> Result<Component, String> {
    // Unfold: a line starting with a space or tab continues the one before.
    let mut logical: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = raw.strip_prefix([' ', '\t'])
            && let Some(last) = logical.last_mut()
        {
            last.push_str(rest);
            continue;
        }
        if !raw.is_empty() {
            logical.push(raw.to_owned());
        }
    }
    let mut stack: Vec<Component> = Vec::new();
    let mut root = None;
    for raw in &logical {
        let Some(line) = split_line(raw) else {
            continue;
        };
        match line.name.as_str() {
            "BEGIN" => stack.push(Component {
                name: line.value.trim().to_ascii_uppercase(),
                ..Default::default()
            }),
            "END" => {
                let done = stack.pop().ok_or("END without BEGIN")?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(done),
                    None => {
                        root = Some(done);
                        break;
                    }
                }
            }
            _ => stack
                .last_mut()
                .ok_or("property outside a component")?
                .lines
                .push(line),
        }
    }
    root.ok_or_else(|| "no complete component found".to_owned())
}

fn fold(out: &mut String, line: &str) {
    // 75 octets per line; continuation lines start with one space.
    let mut width = 0;
    for c in line.chars() {
        let len = c.len_utf8();
        if width + len > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += len;
    }
    out.push_str("\r\n");
}

impl Component {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ..Default::default()
        }
    }

    pub fn get(&self, name: &str) -> Option<&Line> {
        self.lines.iter().find(|l| l.name == name)
    }

    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Line> {
        self.lines.iter().filter(move |l| l.name == name)
    }

    pub fn text(&self, name: &str) -> Option<String> {
        self.get(name).map(Line::text)
    }

    /// Replaces the first property of that name (keeping its place), or appends it.
    pub fn set(&mut self, name: &str, params: &str, value: impl Into<String>) {
        let line = Line::new(name, params, value);
        match self.lines.iter_mut().find(|l| l.name == line.name) {
            Some(existing) => {
                existing.params = line.params;
                existing.value = line.value;
            }
            None => self.lines.push(line),
        }
    }

    pub fn set_text(&mut self, name: &str, value: &str) {
        self.set(name, "", escape(value));
    }

    pub fn remove(&mut self, name: &str) {
        self.lines.retain(|l| l.name != name);
    }

    pub fn child(&self, name: &str) -> Option<&Component> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn child_mut(&mut self, name: &str) -> Option<&mut Component> {
        self.children.iter_mut().find(|c| c.name == name)
    }

    fn write(&self, out: &mut String) {
        fold(out, &format!("BEGIN:{}", self.name));
        for l in &self.lines {
            let mut s = String::new();
            if let Some(g) = &l.group {
                let _ = write!(s, "{g}.");
            }
            let _ = write!(s, "{}{}:{}", l.name, l.params, l.value);
            fold(out, &s);
        }
        for c in &self.children {
            c.write(out);
        }
        fold(out, &format!("END:{}", self.name));
    }

    pub fn serialize(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVENT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:abc\r\nSUMMARY:Lunch\\, with a very long title that goes on and on and on and on and on an\r\n d on\r\nDTSTART;TZID=America/Los_Angeles:20261112T120000\r\nX-CUSTOM;FOO=\"a:b\":kept\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn round_trips_and_edits() {
        let mut cal = parse(EVENT).unwrap();
        let ev = cal.child_mut("VEVENT").unwrap();
        assert!(ev.text("SUMMARY").unwrap().starts_with("Lunch, with"));
        assert_eq!(
            ev.get("DTSTART").unwrap().param("tzid"),
            Some("America/Los_Angeles")
        );
        assert_eq!(ev.get("X-CUSTOM").unwrap().value, "kept");
        ev.set_text("SUMMARY", "Dinner; late");
        let again = parse(&cal.serialize()).unwrap();
        let ev = again.child("VEVENT").unwrap();
        assert_eq!(ev.text("SUMMARY").unwrap(), "Dinner; late");
        assert_eq!(ev.get("X-CUSTOM").unwrap().params, ";FOO=\"a:b\"");
    }

    #[test]
    fn vcard_groups() {
        let card = parse("BEGIN:VCARD\nVERSION:3.0\nFN:Ann Lee\nitem1.EMAIL;TYPE=INTERNET:ann@example.com\nEND:VCARD\n").unwrap();
        let email = card.get("EMAIL").unwrap();
        assert_eq!(email.group.as_deref(), Some("item1"));
        assert!(
            card.serialize()
                .contains("item1.EMAIL;TYPE=INTERNET:ann@example.com")
        );
    }
}
