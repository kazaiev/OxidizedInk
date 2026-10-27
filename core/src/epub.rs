//! EPUB 2/3 package parsing: container, OPF metadata, spine and table of contents.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::text::{self, Chapter};
use crate::xml::{self, local_name, Token, Tokenizer};
use crate::zip::Zip;
use crate::{Error, ReadAt};

#[derive(Debug, Clone)]
pub struct SpineItem {
    /// Full path inside the ZIP.
    pub path: String,
    /// Uncompressed size, used to estimate overall progress.
    pub size: u32,
}

#[derive(Debug, Clone)]
pub struct TocEntry {
    pub title: String,
    pub spine: usize,
    pub fragment: Option<String>,
    pub depth: u8,
}

pub struct Epub<F: ReadAt> {
    zip: Zip<F>,
    pub title: String,
    pub author: String,
    pub spine: Vec<SpineItem>,
    pub toc: Vec<TocEntry>,
}

fn utf8(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

/// Resolves `href` relative to the directory `base` (which ends with '/' or is
/// empty), handling `..`, `.` and percent-encoding. Drops any `#fragment`.
pub fn resolve(base: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or("");
    let href = percent_decode(href);
    let mut parts: Vec<&str> = if href.starts_with('/') {
        Vec::new()
    } else {
        base.split('/').filter(|p| !p.is_empty()).collect()
    };
    for p in href.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

fn dir_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..=i],
        None => "",
    }
}

fn fragment_of(href: &str) -> Option<String> {
    href.split_once('#').map(|(_, f)| percent_decode(f)).filter(|f| !f.is_empty())
}

fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    utf8(out)
}

/// Collapses whitespace and decodes entities in a short piece of metadata text.
fn clean_text(raw: &str) -> String {
    let decoded = xml::decode_entities(raw);
    let mut out = String::new();
    for w in decoded.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
}

struct ManifestItem {
    id: String,
    href: String,
    media_type: String,
    properties: String,
}

impl<F: ReadAt> Epub<F> {
    pub fn open(file: F) -> Result<Self, Error> {
        let mut zip = Zip::open(file)?;
        let container = utf8(zip.read("META-INF/container.xml")?);
        let opf_path = Tokenizer::new(&container)
            .find_map(|t| match t {
                Token::Start { name, attrs, .. } if local_name(name) == "rootfile" => {
                    xml::attr(attrs, "full-path").map(percent_decode)
                }
                _ => None,
            })
            .or_else(|| {
                zip.entries()
                    .iter()
                    .find(|e| e.name.to_ascii_lowercase().ends_with(".opf"))
                    .map(|e| e.name.clone())
            })
            .ok_or(Error::Epub("no package document"))?;
        let opf = utf8(zip.read(&opf_path)?);
        let base = dir_of(&opf_path).to_string();

        let mut title = String::new();
        let mut author = String::new();
        let mut manifest: Vec<ManifestItem> = Vec::new();
        let mut spine_ids: Vec<String> = Vec::new();
        let mut ncx_id: Option<String> = None;
        let mut capture: Option<&'static str> = None;
        let mut buf = String::new();

        for tok in Tokenizer::new(&opf) {
            match tok {
                Token::Start { name, attrs, self_closing } => match local_name(name) {
                    "title" if title.is_empty() && !self_closing => {
                        capture = Some("title");
                        buf.clear();
                    }
                    "creator" if author.is_empty() && !self_closing => {
                        capture = Some("creator");
                        buf.clear();
                    }
                    "item" => manifest.push(ManifestItem {
                        id: xml::attr(attrs, "id").unwrap_or("").to_string(),
                        href: xml::attr(attrs, "href").unwrap_or("").to_string(),
                        media_type: xml::attr(attrs, "media-type").unwrap_or("").to_string(),
                        properties: xml::attr(attrs, "properties").unwrap_or("").to_string(),
                    }),
                    "itemref" => {
                        if let Some(id) = xml::attr(attrs, "idref") {
                            spine_ids.push(id.to_string());
                        }
                    }
                    "spine" => ncx_id = xml::attr(attrs, "toc").map(|s| s.to_string()),
                    _ => {}
                },
                Token::Text(t) if capture.is_some() => buf.push_str(t),
                Token::CData(t) if capture.is_some() => buf.push_str(t),
                Token::End { name } => {
                    let n = local_name(name);
                    if capture == Some(n) {
                        match n {
                            "title" => title = clean_text(&buf),
                            _ => author = clean_text(&buf),
                        }
                        capture = None;
                    }
                }
                _ => {}
            }
        }

        let mut spine = Vec::new();
        for id in &spine_ids {
            if let Some(item) = manifest.iter().find(|m| &m.id == id) {
                let path = resolve(&base, &item.href);
                let size = zip.find(&path).map(|e| e.size).unwrap_or(0);
                spine.push(SpineItem { path, size });
            }
        }
        if spine.is_empty() {
            return Err(Error::Epub("empty spine"));
        }

        let mut epub = Epub { zip, title, author, spine, toc: Vec::new() };

        // Prefer the EPUB 3 navigation document, fall back to the NCX.
        let nav = manifest
            .iter()
            .find(|m| m.properties.split_whitespace().any(|p| p == "nav"))
            .map(|m| resolve(&base, &m.href));
        let ncx = manifest
            .iter()
            .find(|m| {
                Some(&m.id) == ncx_id.as_ref() || m.media_type == "application/x-dtbncx+xml"
            })
            .map(|m| resolve(&base, &m.href));
        if let Some(nav) = nav {
            epub.toc = epub.parse_nav(&nav).unwrap_or_default();
        }
        if epub.toc.is_empty() {
            if let Some(ncx) = ncx {
                epub.toc = epub.parse_ncx(&ncx).unwrap_or_default();
            }
        }
        if epub.title.is_empty() {
            epub.title = String::from("Untitled");
        }
        Ok(epub)
    }

    fn spine_index(&self, path: &str) -> Option<usize> {
        self.spine.iter().position(|s| s.path == path)
    }

    fn toc_entry(&self, base: &str, href: &str, title: &str, depth: u8) -> Option<TocEntry> {
        let title = clean_text(title);
        let spine = self.spine_index(&resolve(base, href))?;
        if title.is_empty() {
            return None;
        }
        Some(TocEntry { title, spine, fragment: fragment_of(href), depth })
    }

    fn parse_nav(&mut self, path: &str) -> Result<Vec<TocEntry>, Error> {
        let doc = utf8(self.zip.read(path)?);
        let base = dir_of(path).to_string();
        let mut out = Vec::new();
        let mut in_toc = false;
        let mut nav_depth = 0;
        let mut list_depth: u8 = 0;
        let mut link: Option<(String, String)> = None;
        for tok in Tokenizer::new(&doc) {
            match tok {
                Token::Start { name, attrs, self_closing } => match local_name(name) {
                    "nav" if !self_closing => {
                        if in_toc {
                            nav_depth += 1;
                        } else if xml::attr(attrs, "type").is_some_and(|t| t.contains("toc")) {
                            in_toc = true;
                            nav_depth = 0;
                        }
                    }
                    "ol" | "ul" if in_toc && !self_closing => list_depth += 1,
                    "a" if in_toc => {
                        if let Some(href) = xml::attr(attrs, "href") {
                            link = Some((href.to_string(), String::new()));
                        }
                    }
                    _ => {}
                },
                Token::Text(t) => {
                    if let Some((_, text)) = link.as_mut() {
                        text.push_str(t);
                    }
                }
                Token::End { name } => match local_name(name) {
                    "nav" if in_toc => {
                        if nav_depth == 0 {
                            in_toc = false;
                        } else {
                            nav_depth -= 1;
                        }
                    }
                    "ol" | "ul" if in_toc => list_depth = list_depth.saturating_sub(1),
                    "a" => {
                        if let Some((href, text)) = link.take() {
                            let depth = list_depth.saturating_sub(1);
                            if let Some(e) = self.toc_entry(&base, &href, &text, depth) {
                                out.push(e);
                            }
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        Ok(out)
    }

    fn parse_ncx(&mut self, path: &str) -> Result<Vec<TocEntry>, Error> {
        let doc = utf8(self.zip.read(path)?);
        let base = dir_of(path).to_string();
        let mut out = Vec::new();
        let mut depth: u8 = 0;
        let mut in_label = false;
        let mut label = String::new();
        for tok in Tokenizer::new(&doc) {
            match tok {
                Token::Start { name, attrs, self_closing } => match local_name(name) {
                    "navPoint" if !self_closing => {
                        depth += 1;
                        label.clear();
                    }
                    "text" => in_label = !self_closing,
                    "content" => {
                        if let Some(src) = xml::attr(attrs, "src") {
                            if let Some(e) = self.toc_entry(&base, src, &label, depth.saturating_sub(1)) {
                                out.push(e);
                            }
                        }
                    }
                    _ => {}
                },
                Token::Text(t) if in_label => label.push_str(t),
                Token::End { name } => match local_name(name) {
                    "navPoint" => depth = depth.saturating_sub(1),
                    "text" => in_label = false,
                    _ => {}
                },
                _ => {}
            }
        }
        Ok(out)
    }

    /// Loads and parses one spine item into styled text blocks.
    pub fn chapter(&mut self, index: usize) -> Result<Chapter, Error> {
        let item = self.spine.get(index).ok_or(Error::Epub("chapter out of range"))?;
        let path = item.path.clone();
        let doc = utf8(self.zip.read(&path)?);
        Ok(text::parse_xhtml(&doc))
    }

    /// Title of the TOC entry that covers `spine`, if any.
    pub fn chapter_title(&self, spine: usize) -> Option<&str> {
        self.toc
            .iter()
            .filter(|t| t.spine <= spine)
            .max_by_key(|t| t.spine)
            .map(|t| t.title.as_str())
    }

    pub fn total_size(&self) -> u64 {
        self.spine.iter().map(|s| s.size.max(1) as u64).sum()
    }

    pub fn size_before(&self, spine: usize) -> u64 {
        self.spine[..spine].iter().map(|s| s.size.max(1) as u64).sum()
    }
}
