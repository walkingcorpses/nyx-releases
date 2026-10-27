// feeds.rs - RSS/Atom/Torznab parsing, filter matching and the "already seen" list. Used by the
// RSS auto-download and by search. The XML is untrusted: it is size-limited and every link is
// checked (magnet or http(s) only). Parsing never panics.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use regex::RegexBuilder;

/// Largest feed document read, in bytes.
pub const MAX_FEED_BYTES: u64 = 10 * 1024 * 1024;
/// Most items taken from one document.
const MAX_ITEMS: usize = 2000;
/// Titles are cut to this many characters.
const MAX_TITLE: usize = 300;
/// How many ids the seen list remembers (oldest are forgotten first).
const MAX_SEEN: usize = 5000;

/// One entry of a feed or search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub title: String,
    /// A `magnet:` link, or an http(s) link to a .torrent file.
    pub link: String,
    /// Stable identity for the "seen" list (guid, else the link).
    pub id: String,
    pub size: Option<u64>,
    pub seeders: Option<u32>,
}

/// Is this link something Nyx may act on?
fn link_ok(l: &str) -> bool {
    let lower = l.trim().to_ascii_lowercase();
    lower.starts_with("magnet:?") || lower.starts_with("http://") || lower.starts_with("https://")
}

/// Value of the attribute `name` on a tag, if present.
fn attr(e: &BytesStart, name: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == name.as_bytes())
        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

/// Work-in-progress item while its element is open.
#[derive(Default)]
struct Draft {
    title: String,
    link_text: String,
    atom_href: String,
    enclosure: String,
    magnet_attr: String,
    guid: String,
    size: Option<u64>,
    seeders: Option<u32>,
}

impl Draft {
    /// Picks the best link (magnet attribute, enclosure, `<link>`, guid) and builds the item.
    fn finish(self) -> Option<Item> {
        let link = [&self.magnet_attr, &self.enclosure, &self.link_text, &self.atom_href, &self.guid]
            .into_iter()
            .map(|l| l.trim().to_string())
            .find(|l| link_ok(l))?;
        let title: String = self.title.trim().chars().take(MAX_TITLE).collect();
        let id = if self.guid.trim().is_empty() { link.clone() } else { self.guid.trim().to_string() };
        Some(Item { title: if title.is_empty() { link.clone() } else { title }, link, id, size: self.size, seeders: self.seeders })
    }
}

/// Appends text to the field of the item being read (title, link, guid, size, seeders).
fn push_text(d: &mut Draft, field: &str, text: &str) {
    match field {
        "title" => d.title.push_str(text),
        "link" => d.link_text.push_str(text),
        "guid" | "id" => d.guid.push_str(text),
        "size" if d.size.is_none() => d.size = text.trim().parse().ok(),
        "seeders" if d.seeders.is_none() => d.seeders = text.trim().parse().ok(),
        _ => {}
    }
}

/// Parses an RSS 2.0, Atom or Torznab document into items. Malformed XML yields whatever parsed
/// before the error; items without a usable link are skipped.
pub fn parse_feed(xml: &[u8]) -> Vec<Item> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut items = Vec::new();
    let mut cur: Option<Draft> = None;
    let mut field: Option<String> = None;
    let mut buf = Vec::new();
    loop {
        let ev = reader.read_event_into(&mut buf);
        let is_empty = matches!(&ev, Ok(Event::Empty(_)));
        match ev {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if name == "item" || name == "entry" {
                    cur = Some(Draft::default());
                } else if let Some(d) = cur.as_mut() {
                    match name.as_str() {
                        "enclosure" => {
                            if let Some(u) = attr(&e, "url") {
                                d.enclosure = u;
                            }
                            if d.size.is_none() {
                                d.size = attr(&e, "length").and_then(|v| v.parse().ok());
                            }
                        }
                        "link" => {
                            if let Some(h) = attr(&e, "href") {
                                if d.atom_href.is_empty() {
                                    d.atom_href = h;
                                }
                            }
                        }
                        "attr" => {
                            let (n, v) = (attr(&e, "name").unwrap_or_default(), attr(&e, "value").unwrap_or_default());
                            match n.as_str() {
                                "magneturl" => d.magnet_attr = v,
                                "size" => d.size = v.parse().ok(),
                                "seeders" => d.seeders = v.parse().ok(),
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                    if !is_empty {
                        field = Some(name);
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if let (Some(d), Some(f)) = (cur.as_mut(), field.as_deref()) {
                    push_text(d, f, &t.decode().map(|c| c.into_owned()).unwrap_or_default());
                }
            }
            Ok(Event::CData(t)) => {
                if let (Some(d), Some(f)) = (cur.as_mut(), field.as_deref()) {
                    push_text(d, f, &String::from_utf8_lossy(&t));
                }
            }
            // `&amp;` and friends arrive as their own events; put the character back in the text.
            Ok(Event::GeneralRef(r)) => {
                if let (Some(d), Some(f)) = (cur.as_mut(), field.as_deref()) {
                    let ch = match r.resolve_char_ref() {
                        Ok(Some(c)) => Some(c),
                        _ => match r.decode().as_deref() {
                            Ok("amp") => Some('&'),
                            Ok("lt") => Some('<'),
                            Ok("gt") => Some('>'),
                            Ok("quot") => Some('"'),
                            Ok("apos") => Some('\''),
                            _ => None,
                        },
                    };
                    if let Some(c) = ch {
                        push_text(d, f, &c.to_string());
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                if name == "item" || name == "entry" {
                    if let Some(item) = cur.take().and_then(Draft::finish) {
                        items.push(item);
                        if items.len() >= MAX_ITEMS {
                            break;
                        }
                    }
                }
                field = None;
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    items
}

/// Does `title` pass a feed filter? The filter is a case-insensitive regular expression; empty
/// matches everything; an invalid or absurdly big expression matches nothing (never auto-download
/// on a broken filter).
pub fn matches(filter: &str, title: &str) -> bool {
    if filter.trim().is_empty() {
        return true;
    }
    RegexBuilder::new(filter)
        .case_insensitive(true)
        .size_limit(1 << 20)
        .build()
        .map(|r| r.is_match(title))
        .unwrap_or(false)
}

/// Is this a usable filter (empty, or a regex that compiles)?
pub fn filter_ok(filter: &str) -> bool {
    filter.trim().is_empty() || RegexBuilder::new(filter).size_limit(1 << 20).build().is_ok()
}

/// The list of item ids already handled, saved to a text file (one id per line) so a restart does
/// not re-add everything. Also remembers which feeds have had their first, baseline-only poll.
pub struct Seen {
    path: PathBuf,
    ids: HashSet<String>,
    order: VecDeque<String>,
}

impl Seen {
    /// Loads the list from `path` (missing file = empty).
    pub fn load(path: &Path) -> Seen {
        let mut s = Seen { path: path.to_path_buf(), ids: HashSet::new(), order: VecDeque::new() };
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines().filter(|l| !l.is_empty()) {
                s.insert(line);
            }
        }
        s
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    /// Adds an id (forgetting the oldest beyond the cap). Newlines are stripped so one id stays one line.
    pub fn insert(&mut self, id: &str) {
        let id = id.replace(['\n', '\r'], " ");
        if self.ids.insert(id.clone()) {
            self.order.push_back(id);
            while self.order.len() > MAX_SEEN {
                if let Some(old) = self.order.pop_front() {
                    self.ids.remove(&old);
                }
            }
        }
    }

    /// Writes the list back to disk.
    pub fn save(&self) -> std::io::Result<()> {
        let text: String = self.order.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(&self.path, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:torznab="http://torznab.com/schemas/2015/feed"><channel><title>Feed</title>
 <item><title>Ubuntu 26.04 Desktop</title><guid>g1</guid><link>https://example.org/ubuntu.torrent</link>
   <enclosure url="https://example.org/ubuntu.torrent" length="12345" type="application/x-bittorrent"/></item>
 <item><title><![CDATA[Debian 13 netinst]]></title><link>magnet:?xt=urn:btih:abababababababababababababababababababab&amp;dn=Debian</link></item>
 <item><title>Torznab one</title><guid>t1</guid><link>https://x.example/page</link>
   <torznab:attr name="magneturl" value="magnet:?xt=urn:btih:cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"/>
   <torznab:attr name="seeders" value="42"/><torznab:attr name="size" value="999"/></item>
 <item><title>Bad scheme</title><link>javascript:alert(1)</link></item>
 <item><title>No link</title></item>
</channel></rss>"#;

    #[test]
    fn parses_rss_with_enclosure_magnet_cdata_and_torznab() {
        let v = parse_feed(RSS.as_bytes());
        assert_eq!(v.len(), 3, "{v:?}");
        assert_eq!(v[0].title, "Ubuntu 26.04 Desktop");
        assert_eq!(v[0].link, "https://example.org/ubuntu.torrent");
        assert_eq!(v[0].id, "g1");
        assert_eq!(v[0].size, Some(12345));
        assert_eq!(v[1].title, "Debian 13 netinst");
        assert!(v[1].link.starts_with("magnet:?xt=urn:btih:abab"));
        assert!(v[1].link.contains("&dn=Debian"), "entities unescaped: {}", v[1].link);
        assert_eq!(v[2].link, "magnet:?xt=urn:btih:cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd");
        assert_eq!((v[2].seeders, v[2].size), (Some(42), Some(999)));
    }

    #[test]
    fn parses_atom_links() {
        let atom = r#"<feed xmlns="http://www.w3.org/2005/Atom"><entry><title>A</title><id>urn:1</id>
            <link rel="alternate" href="https://e.org/a.torrent"/></entry></feed>"#;
        let v = parse_feed(atom.as_bytes());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].link, "https://e.org/a.torrent");
        assert_eq!(v[0].id, "urn:1");
    }

    #[test]
    fn garbage_and_truncated_xml_never_panic() {
        for s in ["", "<", "<item><title>x</item>", "\u{0}\u{1}", "<rss><item><link>magnet:?x</link>", "<a><b></a></b>"] {
            let _ = parse_feed(s.as_bytes());
        }
    }

    #[test]
    fn filter_matching() {
        assert!(matches("", "anything"));
        assert!(matches("ubuntu.*desktop", "Ubuntu 26.04 DESKTOP amd64"));
        assert!(!matches("^debian", "Ubuntu"));
        assert!(!matches("(", "x"), "a broken filter must match nothing");
        assert!(filter_ok("") && filter_ok("a+b") && !filter_ok("("));
    }

    #[test]
    fn seen_list_persists_and_caps() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("seen.txt");
        let mut s = Seen::load(&p);
        s.insert("a");
        s.insert("b\nc");
        s.save().unwrap();
        let s2 = Seen::load(&p);
        assert!(s2.contains("a") && s2.contains("b c") && !s2.contains("zzz"));
        let mut big = Seen::load(&p);
        for i in 0..(MAX_SEEN + 10) {
            big.insert(&format!("id{i}"));
        }
        assert!(!big.contains("id0") && big.contains(&format!("id{}", MAX_SEEN + 9)));
    }
}
