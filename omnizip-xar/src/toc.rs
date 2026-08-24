//! XAR table of contents — quick-xml parsing (events → entries, with
//! nesting resolved through the open-element stack) and the
//! constrained writer (fixed element order → byte-deterministic
//! XML). Port of the Ruby `toc.rb` shape.
#![forbid(unsafe_code)]

use crate::ENCODING_NONE;
use quick_xml::events::Event;
use quick_xml::{Reader, Writer};

/// One TOC file entry.
#[derive(Clone, Debug, Default)]
pub struct TocEntry {
    pub id: u64,
    /// Full path (nested `<file>` names joined with `/`).
    pub name: String,
    /// "file" | "directory" | "symlink" | "hardlink" | "device" | "fifo".
    pub kind: String,
    pub mode: Option<u32>,
    pub uid: Option<u64>,
    pub gid: Option<u64>,
    pub size: Option<u64>,
    pub mtime: Option<f64>,
    /// Heap placement.
    pub data: Option<TocData>,
    /// (link type, target).
    pub link: Option<(String, String)>,
}

/// Heap data descriptor.
#[derive(Clone, Debug, Default)]
pub struct TocData {
    pub offset: u64,
    pub length: u64,
    pub size: u64,
    pub encoding: String,
    pub archived_checksum: Option<String>,
    pub extracted_checksum: Option<String>,
}

/// The whole TOC.
#[derive(Clone, Debug, Default)]
pub struct Toc {
    pub creation_time: f64,
    /// (style, offset, size) of the TOC checksum stored after the
    /// compressed TOC.
    pub checksum: (String, u64, u64),
    pub entries: Vec<TocEntry>,
}

#[derive(Default)]
struct Frame {
    element: String,
    /// Enclosing file-entry name (for path building).
    file_name: Option<String>,
    /// A `<file>` under construction in this frame.
    entry: Option<TocEntry>,
    /// Where this directory should insert (before its first child,
    /// which closes earlier in depth-first XML order).
    insert_at: Option<usize>,
}

/// Parse TOC XML (already decompressed) into entries with full paths.
///
/// # Errors
///
/// XML structure errors via [`quick_xml::Error`].
pub fn parse_toc(xml: &[u8]) -> Result<Toc, quick_xml::Error> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);

    let mut toc = Toc::default();
    let mut stack: Vec<Frame> = Vec::new();
    let mut in_file = false;

    loop {
        let event = reader.read_event()?;
        match event {
            Event::Start(e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                match name.as_str() {
                    "toc" => {}
                    "file" => {
                        let frame = Frame {
                            element: name,
                            entry: Some(TocEntry {
                                id: attr_u64(&e, "id").unwrap_or(0),
                                ..TocEntry::default()
                            }),
                            ..Frame::default()
                        };
                        stack.push(frame);
                        in_file = true;
                    }
                    "checksum" if !in_file => {
                        toc.checksum.0 = attr(&e, "style").unwrap_or_else(|| "sha1".into());
                        stack.push(Frame {
                            element: name,
                            ..Frame::default()
                        });
                    }
                    _ => stack.push(Frame {
                        element: name,
                        ..Frame::default()
                    }),
                }
            }
            Event::Empty(e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if name == "encoding" {
                    let style = attr(&e, "style").unwrap_or_else(|| ENCODING_NONE.into());
                    if let Some(entry) = stack.iter_mut().rev().find_map(|x| x.entry.as_mut()) {
                        let data = entry.data.get_or_insert_with(TocData::default);
                        data.encoding = style;
                    }
                }
            }
            Event::Text(t) => {
                let text: String = t.xml_content()?.into_owned();
                let element = match stack.last() {
                    Some(f) => f.element.clone(),
                    None => continue,
                };
                let in_data = stack.iter().any(|f| f.element == "data");
                let in_file = stack.iter().any(|f| f.entry.is_some());
                match element.as_str() {
                    "creation-time" => toc.creation_time = text.parse().unwrap_or(0.0),
                    "name" if in_file => {
                        assign(&mut stack, |entry| entry.name.clone_from(&text));
                        if let Some(f) = stack.iter_mut().rev().find(|x| x.entry.is_some()) {
                            f.file_name = Some(text.clone());
                        }
                    }
                    "type" => assign(&mut stack, |entry| entry.kind = text.clone()),
                    "mode" => assign(&mut stack, |entry| {
                        entry.mode = parse_mode(&text);
                    }),
                    "uid" => assign(&mut stack, |entry| entry.uid = text.parse().ok()),
                    "gid" => assign(&mut stack, |entry| entry.gid = text.parse().ok()),
                    "mtime" => assign(&mut stack, |entry| entry.mtime = text.parse().ok()),
                    "size" if in_data => {
                        assign_data(&mut stack, |d| d.size = text.parse().unwrap_or(0))
                    }
                    "size" if in_file => assign(&mut stack, |entry| entry.size = text.parse().ok()),
                    "offset" if in_data => {
                        assign_data(&mut stack, |d| d.offset = text.parse().unwrap_or(0))
                    }
                    "length" if in_data => {
                        assign_data(&mut stack, |d| d.length = text.parse().unwrap_or(0))
                    }
                    "offset" => toc.checksum.1 = text.parse().unwrap_or(0),
                    "size" => toc.checksum.2 = text.parse().unwrap_or(0),
                    "archived-checksum" => {
                        assign_data(&mut stack, |d| d.archived_checksum = Some(text.clone()))
                    }
                    "extracted-checksum" => {
                        assign_data(&mut stack, |d| d.extracted_checksum = Some(text.clone()))
                    }
                    "link" => {
                        assign(&mut stack, |entry| {
                            entry.link = Some(("symbolic".into(), text.clone()));
                        });
                    }
                    _ => {}
                }
            }
            Event::End(_) => {
                if let Some(frame) = stack.pop() {
                    if frame.element == "file" {
                        if let Some(mut entry) = frame.entry {
                            // Full path = enclosing file names + own.
                            let prefix: Vec<&str> = stack
                                .iter()
                                .filter_map(|f| f.file_name.as_deref())
                                .collect();
                            let mut full = prefix.join("/");
                            if !full.is_empty() {
                                full.push('/');
                            }
                            full.push_str(&entry.name);
                            entry.name = full;
                            let at = frame.insert_at.unwrap_or(toc.entries.len());
                            toc.entries.insert(at, entry);
                            // Tell the nearest enclosing file frame
                            // where it must insert (before this entry).
                            if let Some(parent) = stack.iter_mut().rev().find(|f| f.entry.is_some())
                            {
                                parent.insert_at.get_or_insert(at);
                            }
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(toc)
}

/// Apply to the nearest enclosing frame's file entry (inner element
/// frames like `<name>` do not hold the entry themselves).
fn assign<F: FnOnce(&mut TocEntry)>(stack: &mut [Frame], f: F) {
    if let Some(entry) = stack.iter_mut().rev().find_map(|x| x.entry.as_mut()) {
        f(entry);
    }
}

fn assign_data<F: FnOnce(&mut TocData)>(stack: &mut [Frame], f: F) {
    if let Some(entry) = stack.iter_mut().rev().find_map(|x| x.entry.as_mut()) {
        let data = entry.data.get_or_insert_with(TocData::default);
        f(data);
    }
}

fn parse_mode(text: &str) -> Option<u32> {
    // Modes are written as decimal-looking octal ("0755" / "493").
    if let Ok(v) = u32::from_str_radix(text.trim(), 8) {
        return Some(v);
    }
    text.parse().ok()
}

fn attr(e: &quick_xml::events::BytesStart, key: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key.as_bytes())
        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

fn attr_u64(e: &quick_xml::events::BytesStart, key: &str) -> Option<u64> {
    attr(e, key).and_then(|v| v.parse().ok())
}

/// Serialize the TOC deterministically: XML decl, `<xar><toc>`,
/// creation-time, checksum, then files in insertion order with a
/// fixed child-element order. `creation_time` and per-entry mtimes
/// come from the caller's fixed options.
///
/// # Errors
///
/// IO errors from the underlying writer (in-memory: none in
/// practice).
pub fn write_toc(toc: &Toc) -> Result<Vec<u8>, quick_xml::Error> {
    let mut buf = Vec::new();
    {
        let mut w = Writer::new_with_indent(&mut buf, b' ', 2);
        w.write_event(Event::Text(quick_xml::events::BytesText::from_escaped(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        )))?;
        w.write_event(Event::Text(quick_xml::events::BytesText::from_escaped(
            "<xar>",
        )))?;
        w.write_event(Event::Text(quick_xml::events::BytesText::from_escaped(
            "<toc>",
        )))?;

        type W<'a> = Writer<&'a mut Vec<u8>>;
        let field = |w: &mut W<'_>, tag: &str, value: &str| -> Result<(), quick_xml::Error> {
            use quick_xml::events::{BytesEnd, BytesStart, BytesText};
            w.write_event(Event::Start(BytesStart::new(tag)))?;
            w.write_event(Event::Text(BytesText::new(value)))?;
            w.write_event(Event::End(BytesEnd::new(tag)))?;
            Ok(())
        };

        field(&mut w, "creation-time", &format!("{}", toc.creation_time))?;
        use quick_xml::events::{BytesEnd, BytesStart};
        let mut ck = BytesStart::new("checksum");
        ck.push_attribute(("style", toc.checksum.0.as_str()));
        w.write_event(Event::Start(ck))?;
        field(&mut w, "offset", &toc.checksum.1.to_string())?;
        field(&mut w, "size", &toc.checksum.2.to_string())?;
        w.write_event(Event::End(BytesEnd::new("checksum")))?;

        // Nested emission: entries with full paths become a tree so
        // parent directories wrap their children (real XAR shape).
        let tree = build_tree(&toc.entries);
        for entry in &tree {
            write_entry(&mut w, entry)?;
        }
        w.write_event(Event::End(BytesEnd::new("toc")))?;
        w.write_event(Event::End(BytesEnd::new("xar")))?;
    }
    buf.push(b'\n');
    Ok(buf)
}

/// Tree node: an entry plus nested children (full-path → tree).
struct TreeNode<'a> {
    entry: &'a TocEntry,
    children: Vec<TreeNode<'a>>,
}

fn build_tree<'a>(entries: &'a [TocEntry]) -> Vec<TreeNode<'a>> {
    // Sort by path so parents precede children (BTreeMap effect).
    let mut sorted: Vec<&'a TocEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));

    let mut roots: Vec<TreeNode<'a>> = Vec::new();
    // Stack of (depth-path, node index chain) — walk parents by prefix.
    fn insert<'a>(roots: &mut Vec<TreeNode<'a>>, entry: &'a TocEntry) {
        let parent_path = match entry.name.rfind('/') {
            Some(i) => &entry.name[..i],
            None => "",
        };
        for root in roots.iter_mut() {
            if root.entry.name == parent_path && !parent_path.is_empty() {
                insert(&mut root.children, entry);
                return;
            }
            if !parent_path.is_empty()
                && root.entry.kind == "directory"
                && parent_path.starts_with(&root.entry.name)
                && parent_path.len() > root.entry.name.len()
                && parent_path[root.entry.name.len()..].starts_with('/')
            {
                insert(&mut root.children, entry);
                return;
            }
        }
        roots.push(TreeNode {
            entry,
            children: Vec::new(),
        });
    }
    for e in sorted {
        insert(&mut roots, e);
    }
    roots
}

fn write_entry(w: &mut Writer<&mut Vec<u8>>, entry: &TreeNode<'_>) -> Result<(), quick_xml::Error> {
    use quick_xml::events::{BytesEnd, BytesStart, BytesText};
    let e = entry.entry;
    let mut fs = BytesStart::new("file");
    fs.push_attribute(("id", e.id.to_string().as_str()));
    w.write_event(Event::Start(fs))?;
    let field =
        |w: &mut Writer<&mut Vec<u8>>, tag: &str, value: &str| -> Result<(), quick_xml::Error> {
            w.write_event(Event::Start(BytesStart::new(tag)))?;
            w.write_event(Event::Text(BytesText::new(value)))?;
            w.write_event(Event::End(BytesEnd::new(tag)))?;
            Ok(())
        };
    field(w, "name", file_display_name(&e.name))?;
    field(w, "type", &e.kind)?;
    if let Some(mode) = e.mode {
        field(w, "mode", &format!("{mode:o}"))?;
    }
    if let Some(uid) = e.uid {
        field(w, "uid", &uid.to_string())?;
    }
    if let Some(gid) = e.gid {
        field(w, "gid", &gid.to_string())?;
    }
    if let Some(mtime) = e.mtime {
        field(w, "mtime", &mtime.to_string())?;
    }
    if let Some(link) = &e.link {
        let mut ls = BytesStart::new("link");
        ls.push_attribute(("type", link.0.as_str()));
        w.write_event(Event::Start(ls))?;
        w.write_event(Event::Text(BytesText::new(&link.1)))?;
        w.write_event(Event::End(BytesEnd::new("link")))?;
    }
    if let Some(data) = &e.data {
        w.write_event(Event::Start(BytesStart::new("data")))?;
        field(w, "offset", &data.offset.to_string())?;
        field(w, "length", &data.length.to_string())?;
        field(w, "size", &data.size.to_string())?;
        let mut es = BytesStart::new("encoding");
        let style = if data.encoding.is_empty() {
            ENCODING_NONE
        } else {
            data.encoding.as_str()
        };
        es.push_attribute(("style", style));
        w.write_event(Event::Empty(es))?;
        if let Some(c) = &data.extracted_checksum {
            let mut cs = BytesStart::new("extracted-checksum");
            cs.push_attribute(("style", "sha1"));
            w.write_event(Event::Start(cs))?;
            w.write_event(Event::Text(BytesText::new(c)))?;
            w.write_event(Event::End(BytesEnd::new("extracted-checksum")))?;
        }
        if let Some(c) = &data.archived_checksum {
            let mut cs = BytesStart::new("archived-checksum");
            cs.push_attribute(("style", "sha1"));
            w.write_event(Event::Start(cs))?;
            w.write_event(Event::Text(BytesText::new(c)))?;
            w.write_event(Event::End(BytesEnd::new("archived-checksum")))?;
        }
        w.write_event(Event::End(BytesEnd::new("data")))?;
    }
    for child in &entry.children {
        write_entry(w, child)?;
    }
    w.write_event(Event::End(BytesEnd::new("file")))?;
    Ok(())
}

/// The `<name>` element holds the basename; nesting carries the path.
fn file_display_name(full: &str) -> &str {
    full.rsplit('/').next().unwrap_or(full)
}
