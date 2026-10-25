//! XHTML chapter -> styled text blocks. CSS is ignored; semantics come from tags.

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

#[derive(Debug, Default)]
pub struct Chapter {
    pub blocks: Vec<Block>,
    /// `id` attribute -> index of the block it starts in (for TOC fragments).
    pub anchors: Vec<(String, usize)>,
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
    pre: u16,
    center: u16,
    quote: u8,
    heading: u8,
    /// Open lists: `None` = bullets, `Some(n)` = next ordinal.
    lists: Vec<Option<u32>>,
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
            let indent = self.quote.saturating_add(self.lists.len().saturating_sub(1) as u8);
            self.chapter.blocks.push(Block {
                kind,
                indent,
                text: core::mem::take(&mut self.text),
                spans,
            });
        }
        self.text.clear();
        self.spans.clear();
    }
}

pub fn parse_xhtml(doc: &str) -> Chapter {
    let mut b = Builder {
        chapter: Chapter::default(),
        text: String::new(),
        spans: Vec::new(),
        bold: 0,
        italic: 0,
        skip: 0,
        pre: 0,
        center: 0,
        quote: 0,
        heading: 0,
        lists: Vec::new(),
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
                if matches!(n, "head" | "script" | "style" | "svg" | "math" | "title") {
                    if !self_closing {
                        b.skip += 1;
                    }
                    continue;
                }
                if n == "br" {
                    b.line_break();
                    continue;
                }
                if matches!(n, "td" | "th") {
                    b.push_text(" ", false);
                    continue;
                }
                if is_block_tag(n) {
                    b.flush();
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
                    continue;
                }
                if is_block_tag(n) {
                    b.flush();
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
        );
        assert_eq!(c.blocks.len(), 5);
        assert_eq!(c.blocks[0].kind, BlockKind::Heading(1));
        assert_eq!(c.blocks[1].text, "Hello big world&co\nnext");
        assert_eq!(c.blocks[1].style_at(6), ITALIC);
        assert_eq!(c.blocks[1].style_at(10), 0);
        assert_eq!(c.blocks[2].kind, BlockKind::Rule);
        assert_eq!(c.blocks[3].text, "• a");
    }
}
