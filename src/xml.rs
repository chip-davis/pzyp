//! A tiny streaming reader for zypper's `--xmlout` stream.
//!
//! zypper emits `<stream> ... </stream>` where every direct child is an
//! independent event (a message, a progress tick, a prompt, a result list...).
//! We build a small DOM for each direct child and hand it out as soon as its
//! closing tag arrives, so interactive prompts can be answered while zypper
//! is still running.

use std::io::BufRead;

use anyhow::Result;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

#[derive(Debug, Clone, Default)]
pub struct Node {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
    pub text: String,
}

impl Node {
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn attr_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.attr(key).unwrap_or(default)
    }

    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// Depth-first search for every descendant with the given tag name.
    pub fn descendants<'a>(&'a self, name: &str, out: &mut Vec<&'a Node>) {
        for c in &self.children {
            if c.name == name {
                out.push(c);
            }
            c.descendants(name, out);
        }
    }

    pub fn child_text(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text.as_str())
    }
}

pub struct StreamReader<R: BufRead> {
    reader: Reader<R>,
    buf: Vec<u8>,
    stack: Vec<Node>,
    done: bool,
}

impl<R: BufRead> StreamReader<R> {
    pub fn new(inner: R) -> Self {
        let mut reader = Reader::from_reader(inner);
        reader.config_mut().trim_text(false);
        Self {
            reader,
            buf: Vec::new(),
            stack: Vec::new(),
            done: false,
        }
    }

    /// Returns the next complete top-level element, or `None` at end of stream.
    pub fn next_node(&mut self) -> Result<Option<Node>> {
        if self.done {
            return Ok(None);
        }
        loop {
            self.buf.clear();
            let ev = match self.reader.read_event_into(&mut self.buf) {
                Ok(ev) => ev,
                Err(e) => {
                    // zypper occasionally prints garbage outside the XML (e.g. from
                    // rpm scriptlets). Don't die on it; just stop reading.
                    self.done = true;
                    return Err(e.into());
                }
            };
            match ev {
                Event::Start(e) => {
                    let node = start_node(&e);
                    if node.name == "stream" && self.stack.is_empty() {
                        continue;
                    }
                    self.stack.push(node);
                }
                Event::Empty(e) => {
                    let node = start_node(&e);
                    if let Some(done) = self.attach(node) {
                        return Ok(Some(done));
                    }
                }
                Event::End(e) => {
                    let name = e.name().as_ref().to_string();
                    if name == "stream" && self.stack.is_empty() {
                        self.done = true;
                        return Ok(None);
                    }
                    if let Some(node) = self.stack.pop()
                        && let Some(done) = self.attach(node)
                    {
                        return Ok(Some(done));
                    }
                }
                Event::Text(t) => {
                    if let Some(top) = self.stack.last_mut() {
                        top.text.push_str(&t.xml10_content());
                    }
                }
                Event::CData(t) => {
                    if let Some(top) = self.stack.last_mut() {
                        top.text.push_str(&t.xml10_content());
                    }
                }
                Event::GeneralRef(r) => {
                    if let Some(top) = self.stack.last_mut() {
                        if let Ok(Some(c)) = r.resolve_char_ref() {
                            top.text.push(c);
                        } else {
                            let name = r.xml10_content();
                            match resolve_predefined_entity(&name) {
                                Some(s) => top.text.push_str(s),
                                None => {
                                    top.text.push('&');
                                    top.text.push_str(&name);
                                    top.text.push(';');
                                }
                            }
                        }
                    }
                }
                Event::Eof => {
                    self.done = true;
                    return Ok(None);
                }
                _ => {}
            }
        }
    }

    /// Attach a finished node to its parent; if it has no parent it's a
    /// complete top-level event and is returned.
    fn attach(&mut self, node: Node) -> Option<Node> {
        match self.stack.last_mut() {
            Some(parent) => {
                parent.children.push(node);
                None
            }
            None => Some(node),
        }
    }
}

fn start_node(e: &BytesStart) -> Node {
    let name = e.name().as_ref().to_string();
    let attrs = e
        .attributes()
        .flatten()
        .map(|a| {
            let key = a.key.as_ref().to_string();
            let val = a
                .normalized_value(XmlVersion::Implicit1_0)
                .map(|v| v.into_owned())
                .unwrap_or_else(|_| a.value.to_string());
            (key, val)
        })
        .collect();
    Node {
        name,
        attrs,
        ..Default::default()
    }
}

/// Parse a complete XML document (used for non-interactive queries).
pub fn parse_all(data: &[u8]) -> Vec<Node> {
    let mut r = StreamReader::new(data);
    let mut out = Vec::new();
    while let Ok(Some(n)) = r.next_node() {
        out.push(n);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stream_events() {
        let xml = br#"<?xml version='1.0'?>
<stream>
<message type="info">Loading &apos;repo&apos; data...</message>
<progress id="1" name="(1/2) Installing: foo" value="40"/>
<install-summary download-size="100" packages-to-change="2">
<to-install><solvable type="package" name="foo" edition="1.0-1" arch="x86_64"/></to-install>
<to-remove><solvable type="package" name="bar" edition="2.0-1" arch="noarch"/></to-remove>
</install-summary>
<prompt id="1"><description>desc</description><text>Continue?</text><option value="y" default="1"/><option value="n"/></prompt>
</stream>"#;
        let nodes = parse_all(xml);
        assert_eq!(nodes.len(), 4);
        assert_eq!(nodes[0].name, "message");
        assert_eq!(nodes[0].text, "Loading 'repo' data...");
        assert_eq!(nodes[1].attr("value"), Some("40"));
        let mut sols = vec![];
        nodes[2].descendants("solvable", &mut sols);
        assert_eq!(sols.len(), 2);
        assert_eq!(nodes[3].child_text("text"), Some("Continue?"));
        assert_eq!(nodes[3].children_named("option").count(), 2);
    }
}
