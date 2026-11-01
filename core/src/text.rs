//! XHTML chapter -> styled text blocks and pictures. CSS is ignored;
//! semantics come from tags.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::xml::{self, local_name, Token, Tokenizer};

pub const BOLD: u8 = 1;
pub const ITALIC: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Para,
    /// h1..h6
    Heading(u8),
    /// Preformatted: whitespace and newlines are kept.
    Pre,
    /// Centered text (e.g. `<center>`).
    Center,
    /// Horizontal rule.
    Rule,
    /// A picture: its index in `Chapter::images`. The block has no text.
    Image(u16),
}

#[derive(Debug, Clone)]
pub struct Block {
    pub kind: BlockKind,
    /// Nesting depth for blockquotes and lists.
    pub indent: u8,
    /// Text with collapsed whitespace. `\n` is a forced line break (`<br>`),
    /// U+00AD a soft hyphen.
    pub text: String,
    /// `(byte offset, style)` runs, sorted, first one at offset 0.
    pub spans: Vec<(u32, u8)>,
}

impl Block {
    pub fn style_at(&self, byte: usize) -> u8 {
        let i = self.spans.partition_point(|&(o, _)| o as usize <= byte);
        if i == 0 {
            0
        } else {
            self.spans[i - 1].1
        }
    }
}

/// A picture in a chapter.
#[derive(Debug, Clone)]
pub struct Image {
    /// Its file inside the EPUB.
    pub path: String,
    /// Its size in pixels.
    pub width: u32,
    pub height: u32,
    /// How many blocks after the picture's own are its caption: the text
    /// after it inside the same element, such as a `<figure>` or a heading.
    pub caption: u8,
}

/// More blocks than this after a picture are not a caption.
const MAX_CAPTION: usize = 4;

#[derive(Debug, Default)]
pub struct Chapter {
    pub blocks: Vec<Block>,
    /// `id` attribute -> index of the block it starts in (for TOC fragments).
    pub anchors: Vec<(String, usize)>,
    /// Sorted indices of blocks that must start a new page, such as chapter
    /// starts listed in the table of contents. Set by the reader, not parsed.
    pub breaks: Vec<usize>,
    pub images: Vec<Image>,
}

impl Chapter {
    pub fn anchor(&self, id: &str) -> Option<usize> {
        self.anchors.iter().find(|(a, _)| a == id).map(|&(_, b)| b)
    }
}

fn is_block_tag(n: &str) -> bool {
    matches!(
        n,
        "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" | "blockquote" | "section"
            | "article" | "header" | "footer" | "aside" | "figure" | "figcaption" | "pre"
            | "table" | "tr" | "dt" | "dd" | "dl" | "ul" | "ol" | "body" | "main" | "center"
            | "address" | "hr" | "nav"
    )
}

struct Builder {
    chapter: Chapter,
    text: String,
    spans: Vec<(u32, u8)>,
    bold: u16,
    italic: u16,
    skip: u16,
    /// How many of the skipped elements are `<svg>`: an `<image>` in one
    /// is still shown.
    svg: u16,
    pre: u16,
    center: u16,
    quote: u8,
    heading: u8,
    /// Open lists: `None` = bullets, `Some(n)` = next ordinal.
    lists: Vec<Option<u32>>,
    /// Open block elements.
    depth: u16,
    /// The last picture while its element is open, for counting its
    /// caption: `(image, its block, depth)`.
    figure: Option<(usize, usize, u16)>,
}

impl Builder {
    fn style(&self) -> u8 {
        let mut s = 0;
        if self.bold > 0 || self.heading > 0 {
            s |= BOLD;
        }
        if self.italic > 0 {
            s |= ITALIC;
        }
        s
    }

    fn mark_style(&mut self) {
        let s = self.style();
        if self.spans.last().map(|&(_, st)| st) != Some(s) {
            let at = self.text.len() as u32;
            if let Some(last) = self.spans.last_mut() {
                if last.0 == at {
                    last.1 = s;
                    return;
                }
            }
            self.spans.push((at, s));
        }
    }

    fn push_text(&mut self, raw: &str, decode: bool) {
        if self.skip > 0 {
            return;
        }
        let mut decoded = String::new();
        let s = if decode {
            xml::decode_entities_into(raw, &mut decoded);
            decoded.as_str()
        } else {
            raw
        };
        for c in s.chars() {
            if self.pre > 0 {
                if c == '\r' {
                    continue;
                }
                self.mark_style();
                self.text.push(c);
            } else if c.is_whitespace() && c != '\u{a0}' {
                if !self.text.is_empty() && !self.text.ends_with([' ', '\n']) {
                    self.mark_style();
                    self.text.push(' ');
                }
            } else {
                self.mark_style();
                self.text.push(c);
            }
        }
    }

    fn line_break(&mut self) {
        if self.skip > 0 {
            return;
        }
        while self.text.ends_with(' ') {
            self.text.pop();
        }
        if !self.text.is_empty() {
            self.mark_style();
            self.text.push('\n');
        }
    }

    fn flush(&mut self) {
        while self.text.ends_with([' ', '\n']) {
            self.text.pop();
        }
        if self.text.chars().any(|c| !c.is_whitespace()) {
            let kind = if self.heading > 0 {
                BlockKind::Heading(self.heading)
            } else if self.pre > 0 {
                BlockKind::Pre
            } else if self.center > 0 {
                BlockKind::Center
            } else {
                BlockKind::Para
            };
            let mut spans = core::mem::take(&mut self.spans);
            spans.retain(|&(o, _)| (o as usize) < self.text.len());
            if spans.first().map(|&(o, _)| o) != Some(0) {
                spans.insert(0, (0, 0));
            }
            let indent = self.indent();
            self.chapter.blocks.push(Block {
                kind,
                indent,
                text: core::mem::take(&mut self.text),
                spans,
            });
            if let Some((image, block, _)) = self.figure {
                let caption = self.chapter.blocks.len() - 1 - block;
                if caption <= MAX_CAPTION {
                    self.chapter.images[image].caption = caption as u8;
                } else {
                    self.chapter.images[image].caption = 0;
                    self.figure = None;
                }
            }
        }
        self.text.clear();
        self.spans.clear();
    }

    fn indent(&self) -> u8 {
        self.quote.saturating_add(self.lists.len().saturating_sub(1) as u8)
    }

    /// Ends the text so far, which goes on after the picture in a new block.
    fn picture(&mut self, image: Image) {
        let Ok(index) = u16::try_from(self.chapter.images.len()) else { return };
        self.flush();
        self.figure = Some((index as usize, self.chapter.blocks.len(), self.depth));
        self.chapter.images.push(image);
        self.chapter.blocks.push(Block {
            kind: BlockKind::Image(index),
            indent: self.indent(),
            text: String::new(),
            spans: Vec::new(),
        });
    }
}

/// Parses a chapter. `image` looks up a picture by its `src` as written in
/// the document; pictures it doesn't find are left out.
pub fn parse_xhtml(doc: &str, mut image: impl FnMut(&str) -> Option<Image>) -> Chapter {
    let mut b = Builder {
        chapter: Chapter::default(),
        text: String::new(),
        spans: Vec::new(),
        bold: 0,
        italic: 0,
        skip: 0,
        svg: 0,
        pre: 0,
        center: 0,
        quote: 0,
        heading: 0,
        lists: Vec::new(),
        depth: 0,
        figure: None,
    };
    for tok in Tokenizer::new(doc) {
        match tok {
            Token::Text(t) => b.push_text(t, true),
            Token::CData(t) => b.push_text(t, false),
            Token::Start { name, attrs, self_closing } => {
                let lname = local_name(name).to_ascii_lowercase();
                let n = lname.as_str();
                if b.skip == 0 {
                    if let Some(id) = xml::attr(attrs, "id") {
                        b.chapter.anchors.push((id.to_string(), b.chapter.blocks.len()));
                    }
                }
                if n == "image" && b.skip == b.svg {
                    // Cover pages often wrap the picture in an SVG.
                    let src = xml::attr(attrs, "href").map(xml::decode_entities);
                    if let Some(img) = src.and_then(|s| image(&s)) {
                        b.picture(img);
                    }
                    continue;
                }
                if matches!(n, "head" | "script" | "style" | "svg" | "math" | "title") {
                    if !self_closing {
                        b.skip += 1;
                        b.svg += (n == "svg") as u16;
                    }
                    continue;
                }
                if n == "br" {
                    b.line_break();
                    continue;
                }
                if n == "img" {
                    if b.skip > 0 {
                        continue;
                    }
                    // Illustrated editions set a chapter's first letter as an
                    // image (a drop cap) whose alt text is the letter.
                    let alt = xml::attr(attrs, "alt").map(xml::decode_entities).unwrap_or_default();
                    let alt = alt.trim();
                    if !alt.is_empty() && alt.chars().count() <= 3 && !alt.contains(char::is_whitespace) {
                        b.push_text(alt, false);
                    } else if let Some(img) = xml::attr(attrs, "src").and_then(|s| image(&xml::decode_entities(s))) {
                        b.picture(img);
                    }
                    continue;
                }
                if matches!(n, "td" | "th") {
                    b.push_text(" ", false);
                    continue;
                }
                if is_block_tag(n) {
                    b.flush();
                    b.depth += !self_closing as u16;
                }
                if self_closing {
                    if n == "hr" && b.skip == 0 {
                        b.chapter.blocks.push(Block {
                            kind: BlockKind::Rule,
                            indent: 0,
                            text: String::new(),
                            spans: Vec::new(),
                        });
                    }
                    continue;
                }
                match n {
                    "b" | "strong" => b.bold += 1,
                    "i" | "em" | "cite" | "var" | "dfn" => b.italic += 1,
                    "pre" => b.pre += 1,
                    "center" => b.center += 1,
                    "blockquote" => b.quote = b.quote.saturating_add(1),
                    "ul" => b.lists.push(None),
                    "ol" => {
                        let start = xml::attr(attrs, "start").and_then(|s| s.parse().ok());
                        b.lists.push(Some(start.unwrap_or(1)));
                    }
                    "li" => {
                        let marker = match b.lists.last_mut() {
                            Some(Some(n)) => {
                                let m = alloc::format!("{}. ", n);
                                *n += 1;
                                m
                            }
                            _ => String::from("• "),
                        };
                        b.push_text(&marker, false);
                    }
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        b.heading = n.as_bytes()[1] - b'0';
                    }
                    _ => {}
                }
            }
            Token::End { name } => {
                let lname = local_name(name).to_ascii_lowercase();
                let n = lname.as_str();
                if matches!(n, "head" | "script" | "style" | "svg" | "math" | "title") {
                    b.skip = b.skip.saturating_sub(1);
                    if n == "svg" {
                        b.svg = b.svg.saturating_sub(1);
                    }
                    continue;
                }
                if is_block_tag(n) {
                    b.flush();
                    b.depth = b.depth.saturating_sub(1);
                    if b.figure.is_some_and(|(_, _, d)| b.depth < d) {
                        b.figure = None;
                    }
                }
                match n {
                    "b" | "strong" => b.bold = b.bold.saturating_sub(1),
                    "i" | "em" | "cite" | "var" | "dfn" => b.italic = b.italic.saturating_sub(1),
                    "pre" => b.pre = b.pre.saturating_sub(1),
                    "center" => b.center = b.center.saturating_sub(1),
                    "blockquote" => b.quote = b.quote.saturating_sub(1),
                    "ul" | "ol" => {
                        b.lists.pop();
                    }
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => b.heading = 0,
                    _ => {}
                }
            }
        }
    }
    b.flush();
    b.chapter
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_and_blocks() {
        let c = parse_xhtml(
            "<html><head><title>x</title></head><body><h1>Title</h1>\
             <p>Hello <em>big</em>   world&amp;co<br/>next</p><hr/><ul><li>a</li><li>b</li></ul></body></html>",
            |_| None,
        );
        assert_eq!(c.blocks.len(), 5);
        assert_eq!(c.blocks[0].kind, BlockKind::Heading(1));
        assert_eq!(c.blocks[1].text, "Hello big world&co\nnext");
        assert_eq!(c.blocks[1].style_at(6), ITALIC);
        assert_eq!(c.blocks[1].style_at(10), 0);
        assert_eq!(c.blocks[2].kind, BlockKind::Rule);
        assert_eq!(c.blocks[3].text, "• a");
    }

    #[test]
    fn drop_cap_alt_text() {
        let c = parse_xhtml(
            "<p><span><img alt=\"W\" src=\"w.png\"/></span>HEN the ladies</p>\
             <p><img alt=\"\" src=\"a.jpg\"/><img alt=\"A long description\" src=\"b.jpg\"/>Text</p>",
            |_| None,
        );
        assert_eq!(c.blocks[0].text, "WHEN the ladies");
        assert_eq!(c.blocks[1].text, "Text");
    }

    #[test]
    fn pictures() {
        let found = |src: &str| {
            let path = alloc::format!("OEBPS/{src}");
            (src != "missing.png").then_some(Image { path, width: 10, height: 20, caption: 0 })
        };
        let c = parse_xhtml(
            "<h2 id=\"c1\"><img alt=\"\" src=\"head.jpg\"/><br/>CHAPTER I.</h2>\
             <p>Before<img src=\"missing.png\"/> <img src=\"a&amp;b.png\"/>after</p>\
             <svg><title>Cover</title><image xlink:href=\"cover.jpg\"/></svg>",
            found,
        );
        let blocks: Vec<_> = c.blocks.iter().map(|b| (b.kind, b.text.as_str())).collect();
        assert_eq!(
            blocks,
            [
                (BlockKind::Image(0), ""),
                (BlockKind::Heading(2), "CHAPTER I."),
                (BlockKind::Para, "Before"),
                (BlockKind::Image(1), ""),
                (BlockKind::Para, "after"),
                (BlockKind::Image(2), ""),
            ]
        );
        let paths: Vec<_> = c.images.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, ["OEBPS/head.jpg", "OEBPS/a&b.png", "OEBPS/cover.jpg"]);
        assert_eq!(c.anchor("c1"), Some(0));
        let captions: Vec<_> = c.images.iter().map(|i| i.caption).collect();
        assert_eq!(captions, [1, 1, 0]);
    }

    #[test]
    fn captions() {
        let found = |src: &str| Some(Image { path: src.to_string(), width: 10, height: 20, caption: 0 });
        let c = parse_xhtml(
            "<body><div class=\"figcenter\"><img src=\"a.jpg\"/>\
             <div class=\"caption\"><p>One</p><p>Two</p></div></div><p>Text</p>\
             <p><img src=\"b.jpg\"/></p><p>Not a caption</p>\
             <div><img src=\"c.jpg\"/><p>1</p><p>2</p><p>3</p><p>4</p><p>5</p></div></body>",
            found,
        );
        let captions: Vec<_> = c.images.iter().map(|i| i.caption).collect();
        assert_eq!(captions, [2, 0, 0]);
    }
}
