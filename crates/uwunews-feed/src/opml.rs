//! OPML: the list of subscriptions every reader can export.
//!
//! Folders in OPML are `<outline>`s with children; UwUNews makes each one a
//! collection of the feeds inside it. Nested folders are flattened into the
//! outermost one, which is how most readers export them anyway.

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, Event};
use quick_xml::{Reader, Writer, XmlVersion};

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outline {
    pub title: String,
    pub xml_url: String,
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Opml {
    /// Feeds outside any folder.
    pub feeds: Vec<Outline>,
    /// Folders by name, with their feeds.
    pub folders: Vec<(String, Vec<Outline>)>,
}

impl Opml {
    /// Every feed once, in file order.
    pub fn all_feeds(&self) -> Vec<&Outline> {
        let mut seen = std::collections::HashSet::new();
        self.feeds
            .iter()
            .chain(self.folders.iter().flat_map(|(_, f)| f))
            .filter(|o| seen.insert(o.xml_url.clone()))
            .collect()
    }
}

pub fn read(bytes: &[u8]) -> Result<Opml> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut opml = Opml::default();
    // The stack of open outlines: Some(folder index) for a folder, None for a
    // feed or something we skip.
    let mut stack: Vec<Option<usize>> = Vec::new();
    let mut saw_root = false;
    let mut buf = Vec::new();
    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::Opml(e.to_string()))?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(event, Event::Empty(_));
                let name = e.local_name();
                if name.as_ref() == "opml" {
                    saw_root = true;
                }
                if name.as_ref() != "outline" {
                    continue;
                }
                let attr = |key: &str| {
                    e.attributes()
                        .flatten()
                        .find(|a| a.key.local_name().as_ref().eq_ignore_ascii_case(key))
                        .and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok())
                        .map(|v| v.trim().to_string())
                        .filter(|v| !v.is_empty())
                };
                let title = attr("title").or_else(|| attr("text"));
                let folder = stack.iter().flatten().next().copied();
                let opened = if let Some(xml_url) = attr("xmlUrl") {
                    let outline = Outline {
                        title: title.unwrap_or_else(|| xml_url.clone()),
                        xml_url,
                        html_url: attr("htmlUrl"),
                    };
                    match folder {
                        Some(i) => opml.folders[i].1.push(outline),
                        None => opml.feeds.push(outline),
                    }
                    None
                } else if folder.is_none() && !empty {
                    opml.folders
                        .push((title.unwrap_or_else(|| "Ordner".into()), Vec::new()));
                    Some(opml.folders.len() - 1)
                } else {
                    None
                };
                if !empty {
                    stack.push(opened);
                }
            }
            Event::End(ref e) if e.local_name().as_ref() == "outline" => {
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if !saw_root {
        return Err(Error::Opml("no <opml> element".into()));
    }
    opml.folders.retain(|(_, feeds)| !feeds.is_empty());
    Ok(opml)
}

pub fn write(opml: &Opml) -> String {
    let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);
    let outline = |w: &mut Writer<Vec<u8>>, o: &Outline| {
        let mut tag = BytesStart::new("outline");
        tag.push_attribute(("type", "rss"));
        tag.push_attribute(("text", o.title.as_str()));
        tag.push_attribute(("title", o.title.as_str()));
        tag.push_attribute(("xmlUrl", o.xml_url.as_str()));
        if let Some(html) = &o.html_url {
            tag.push_attribute(("htmlUrl", html.as_str()));
        }
        w.write_event(Event::Empty(tag)).ok();
    };
    writer
        .write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
        .ok();
    let mut root = BytesStart::new("opml");
    root.push_attribute(("version", "2.0"));
    writer.write_event(Event::Start(root)).ok();
    writer
        .write_event(Event::Start(BytesStart::new("head")))
        .ok();
    writer
        .create_element("title")
        .write_text_content(quick_xml::events::BytesText::new("UwUNews"))
        .ok();
    writer.write_event(Event::End(BytesEnd::new("head"))).ok();
    writer
        .write_event(Event::Start(BytesStart::new("body")))
        .ok();
    for o in &opml.feeds {
        outline(&mut writer, o);
    }
    for (name, feeds) in &opml.folders {
        let mut tag = BytesStart::new("outline");
        tag.push_attribute(("text", name.as_str()));
        tag.push_attribute(("title", name.as_str()));
        writer.write_event(Event::Start(tag)).ok();
        for o in feeds {
            outline(&mut writer, o);
        }
        writer
            .write_event(Event::End(BytesEnd::new("outline")))
            .ok();
    }
    writer.write_event(Event::End(BytesEnd::new("body"))).ok();
    writer.write_event(Event::End(BytesEnd::new("opml"))).ok();
    String::from_utf8(writer.into_inner()).unwrap_or_default()
}
