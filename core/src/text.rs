//! XHTML chapter -> styled text blocks and pictures. Tags say what text is
//! (headings, lists, emphasis), and the book's CSS, where it's read, can
//! change some of that: see `css`.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub use crate::css::Align;
use crate::css::{self, Declarations, Display, Stylesheet};
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
    /// Horizontal rule.
    Rule,
    /// A picture: its index in `Chapter::images`. The block has no text.
    Image(u16),
    /// A table row: `Block::cells` are its cells. `table` tells the
    /// chapter's tables apart; a header row's cells are bold.
    Row { table: u16, header: bool },
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
    /// In a table row, each cell's text as a byte range in `text`; the
    /// cells are separated by a space. Empty for other blocks.
    pub cells: Vec<(u32, u32)>,
    pub style: BlockStyle,
}

/// How a block is set where its tags or the book's CSS say: `<center>`,
/// and otherwise only from CSS.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlockStyle {
    pub align: Option<Align>,
    /// Its first line isn't indented: `text-indent` is zero or less.
    pub flush: bool,
    /// Space above and below it: a margin of half a line or more, its own
    /// or that of an element it begins or ends.
    pub above: bool,
    pub below: bool,
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

/// A link inside the book, such as a footnote's number: its text in a
/// block, and where it leads, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub block: u32,
    /// The link's text, as byte offsets in the block's text.
    pub start: u32,
    pub end: u32,
    pub href: String,
}

#[derive(Debug, Default)]
pub struct Chapter {
    pub blocks: Vec<Block>,
    /// `id` attribute -> index of the block it starts in (for TOC fragments).
    pub anchors: Vec<(String, usize)>,
    /// Sorted indices of blocks that must start a new page: where the
    /// book's CSS breaks pages, and the reader adds the chapter starts
    /// listed in the table of contents.
    pub breaks: Vec<usize>,
    pub images: Vec<Image>,
    /// Links inside the book, in reading order.
    pub links: Vec<Link>,
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
            | "address" | "hr" | "nav" | "hgroup"
    )
}

/// An open element: what selectors see of it, and what it passes on to
/// the elements inside it.
struct Open {
    name: String,
    id: String,
    class: String,
    bold: bool,
    italic: bool,
    align: Option<Align>,
    /// `text-indent`, where the book's CSS sets it.
    indent: Option<bool>,
    /// Hidden by `display: none`, and counted in `skip`.
    hidden: bool,
    /// Sets its blocks in from the left, as a block quote: counted in
    /// `quote`.
    set_in: bool,
    /// Set as a block: a block element, or by `display: block`.
    block: bool,
    break_after: bool,
    /// A bottom margin of half a line or more, for its last block.
    below: bool,
    /// How many blocks the chapter had when it opened.
    first_block: usize,
}

impl css::Element for Open {
    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> &str {
        &self.id
    }

    fn class(&self) -> &str {
        &self.class
    }
}

impl Open {
    /// How the blocks of its text are set.
    fn block_style(&self) -> BlockStyle {
        BlockStyle { align: self.align, flush: self.indent == Some(false), above: false, below: false }
    }
}

struct Builder<'a> {
    chapter: Chapter,
    text: String,
    spans: Vec<(u32, u8)>,
    /// The book's stylesheets, in order, if its CSS is read.
    css: Option<&'a [&'a Stylesheet]>,
    /// The open elements, outermost first.
    stack: Vec<Open>,
    /// How the block being gathered is set.
    para: BlockStyle,
    /// The next block starts a page.
    page_break: bool,
    skip: u16,
    /// How many of the skipped elements are `<svg>`: an `<image>` in one
    /// is still shown.
    svg: u16,
    pre: u16,
    quote: u8,
    heading: u8,
    /// Open lists: `None` = bullets, `Some(n)` = next ordinal.
    lists: Vec<Option<u32>>,
    /// Open block elements.
    depth: u16,
    /// The last picture while its element is open, for counting its
    /// caption: `(image, its block, depth)`.
    figure: Option<(usize, usize, u16)>,
    /// The open link: `(href, blocks so far, start in text)`.
    link: Option<(String, usize, usize)>,
    /// Open tables, and the number of the outermost one.
    tables: u8,
    table: u16,
    /// The outermost table's open row: its cells so far, whether it's a
    /// header row, and where the open cell starts.
    row: Option<Row>,
    /// Inside a `<thead>`.
    thead: bool,
}

struct Row {
    cells: Vec<(u32, u32)>,
    /// In a `<thead>`, or every cell so far a `<th>`.
    header: bool,
    cell: Option<u32>,
}

impl Builder<'_> {
    fn style(&self) -> u8 {
        self.stack.last().map_or(0, |e| (e.bold as u8 * BOLD) | (e.italic as u8 * ITALIC))
    }

    /// Opens element `n` on top of the stack, with what it inherits, and
    /// returns what the book's CSS says of it.
    fn open(&mut self, n: &str, attrs: &str) -> Declarations {
        let parent = self.stack.last();
        let mut e = Open {
            name: n.to_string(),
            id: String::new(),
            class: String::new(),
            bold: parent.is_some_and(|p| p.bold) || matches!(n, "b" | "strong" | "th" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"),
            italic: parent.is_some_and(|p| p.italic) || matches!(n, "i" | "em" | "cite" | "var" | "dfn"),
            align: if n == "center" { Some(Align::Center) } else { parent.and_then(|p| p.align) },
            indent: parent.and_then(|p| p.indent),
            hidden: false,
            set_in: false,
            block: is_block_tag(n),
            break_after: false,
            below: false,
            first_block: 0,
        };
        let Some(sheets) = self.css.filter(|_| self.skip == 0) else {
            self.stack.push(e);
            return Declarations::default();
        };
        e.id = xml::attr(attrs, "id").unwrap_or("").to_string();
        e.class = xml::attr(attrs, "class").unwrap_or("").to_string();
        self.stack.push(e);
        let mut d = css::cascade(sheets, &self.stack);
        if let Some(style) = xml::attr(attrs, "style") {
            d.overlay(&Declarations::parse(&xml::decode_entities(style)));
        }
        if let Some(e) = self.stack.last_mut() {
            e.bold = d.bold.unwrap_or(e.bold);
            e.italic = d.italic.unwrap_or(e.italic);
            e.align = d.align.or(e.align);
            e.indent = d.indent.or(e.indent);
            e.hidden = d.display == Some(Display::None);
            e.block |= d.display == Some(Display::Block);
            e.break_after = d.break_after == Some(true);
            e.below = d.margin_bottom == Some(true);
        }
        d
    }

    /// Closes the innermost open `n`, with any elements left open inside
    /// it. None if no `n` is open.
    fn close(&mut self, n: &str) -> Option<Open> {
        let at = self.stack.iter().rposition(|e| e.name == n)?;
        let mut closed = self.stack.split_off(at);
        let hidden = closed[1..].iter().filter(|e| e.hidden).count() as u16;
        self.skip = self.skip.saturating_sub(hidden);
        Some(closed.swap_remove(0))
    }

    /// Adds a block to the chapter, starting a page if one is due.
    fn push_block(&mut self, block: Block) {
        if core::mem::take(&mut self.page_break) {
            self.chapter.breaks.push(self.chapter.blocks.len());
        }
        self.chapter.blocks.push(block);
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

    /// A new line in the open cell, between paragraphs in it: one, and not
    /// before its text.
    fn cell_break(&mut self) {
        let start = self.row.as_ref().and_then(|r| r.cell).unwrap_or(u32::MAX) as usize;
        if self.text.trim_end_matches(' ').len() > start && !self.text.trim_end_matches(' ').ends_with('\n') {
            self.line_break();
        }
    }

    /// Starts a cell of the open row, closing one left open.
    fn open_cell(&mut self, th: bool) {
        self.close_cell();
        let Some(row) = &mut self.row else { return };
        row.header &= th || self.thead;
        if !self.text.is_empty() && !self.text.ends_with(' ') {
            self.text.push(' ');
        }
        row.cell = Some(self.text.len() as u32);
    }

    fn close_cell(&mut self) {
        let Some(row) = &mut self.row else { return };
        if let Some(start) = row.cell.take() {
            let end = self.text.trim_end_matches([' ', '\n']).len().max(start as usize);
            row.cells.push((start, end as u32));
        }
    }

    /// Ends the open row as a block of its own.
    fn close_row(&mut self) {
        self.close_cell();
        let Some(row) = self.row.take() else { return };
        let header = row.header;
        if !row.cells.is_empty() && self.text.chars().any(|c| !c.is_whitespace()) {
            let mut spans = core::mem::take(&mut self.spans);
            spans.retain(|&(o, _)| (o as usize) < self.text.len());
            if spans.first().map(|&(o, _)| o) != Some(0) {
                spans.insert(0, (0, 0));
            }
            let block = Block {
                kind: BlockKind::Row { table: self.table, header },
                indent: self.indent(),
                text: core::mem::take(&mut self.text),
                spans,
                cells: row.cells,
                style: BlockStyle::default(),
            };
            self.push_block(block);
        }
        self.text.clear();
        self.spans.clear();
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
            } else {
                BlockKind::Para
            };
            let mut spans = core::mem::take(&mut self.spans);
            spans.retain(|&(o, _)| (o as usize) < self.text.len());
            if spans.first().map(|&(o, _)| o) != Some(0) {
                spans.insert(0, (0, 0));
            }
            let block = Block {
                kind,
                indent: self.indent(),
                text: core::mem::take(&mut self.text),
                spans,
                cells: Vec::new(),
                style: self.para,
            };
            self.push_block(block);
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
        // In a table row, the row so far ends, and goes on after it.
        let row = self.row.as_ref().map(|r| (r.header, r.cell.is_some(), r.cells.len()));
        if row.is_some() {
            self.close_row();
        } else {
            self.flush();
        }
        self.figure = Some((index as usize, self.chapter.blocks.len(), self.depth));
        self.chapter.images.push(image);
        let block = Block {
            kind: BlockKind::Image(index),
            indent: self.indent(),
            text: String::new(),
            spans: Vec::new(),
            cells: Vec::new(),
            style: BlockStyle::default(),
        };
        self.push_block(block);
        if let Some((header, in_cell, before)) = row {
            // Empty cells keep the rest in their columns.
            self.row = Some(Row { cells: alloc::vec![(0, 0); before], header, cell: None });
            if in_cell {
                self.open_cell(header);
            }
        }
    }
}

/// Parses a chapter without its CSS. `image` looks up a picture by its
/// `src` as written in the document; pictures it doesn't find are left out.
pub fn parse_xhtml(doc: &str, image: impl FnMut(&str) -> Option<Image>) -> Chapter {
    parse_styled(doc, None, image)
}

/// Parses a chapter, reading the book's CSS if `css` has its stylesheets,
/// in the order the document names them; `style` attributes are read too.
pub fn parse_styled(doc: &str, css: Option<&[&Stylesheet]>, mut image: impl FnMut(&str) -> Option<Image>) -> Chapter {
    let mut b = Builder {
        chapter: Chapter::default(),
        text: String::new(),
        spans: Vec::new(),
        css,
        stack: Vec::new(),
        para: BlockStyle::default(),
        page_break: false,
        skip: 0,
        svg: 0,
        pre: 0,
        quote: 0,
        heading: 0,
        lists: Vec::new(),
        depth: 0,
        figure: None,
        link: None,
        tables: 0,
        table: 0,
        row: None,
        thead: false,
    };
    for tok in Tokenizer::new(doc) {
        match tok {
            Token::Text(t) => b.push_text(t, true),
            Token::CData(t) => b.push_text(t, false),
            Token::Start { name, attrs, self_closing } => {
                let lname = local_name(name).to_ascii_lowercase();
                let n = lname.as_str();
                let decl = b.open(n, attrs);
                let block = b.stack.last().is_some_and(|e| e.block);
                if self_closing {
                    b.stack.pop();
                }
                if decl.display == Some(Display::None) {
                    b.skip += !self_closing as u16;
                    continue;
                }
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
                // Tables: the outermost one's rows become blocks; a nested
                // one's cells are just spaced out in its cell.
                if b.skip == 0 && matches!(n, "table" | "tr" | "td" | "th" | "thead") {
                    match n {
                        "table" if !self_closing => {
                            b.tables += 1;
                            if b.tables == 1 {
                                b.flush();
                                b.table += 1;
                            }
                        }
                        "thead" => b.thead = !self_closing && b.tables == 1,
                        "tr" if b.tables == 1 => {
                            b.close_row();
                            b.flush();
                            b.row = Some(Row { cells: Vec::new(), header: true, cell: None });
                        }
                        "td" | "th" if b.tables == 1 && b.row.is_some() => b.open_cell(n == "th"),
                        _ => b.push_text(" ", false),
                    }
                    continue;
                }
                if block && b.row.is_some() {
                    // A paragraph or the like inside a cell: a new line in it.
                    b.cell_break();
                } else if block {
                    // A margin above an element with no text before its
                    // first block is that block's.
                    let carried = b.para.above && b.text.trim().is_empty();
                    b.flush();
                    b.depth += !self_closing as u16;
                    let first_block = b.chapter.blocks.len();
                    if let Some(e) = b.stack.last_mut().filter(|_| !self_closing) {
                        e.first_block = first_block;
                        let above = carried || decl.margin_top == Some(true);
                        b.para = BlockStyle { above, ..e.block_style() };
                    }
                    if decl.break_before == Some(true) {
                        b.page_break = true;
                    }
                }
                // A block quote is set in, unless the book's CSS says
                // otherwise; so is another block it sets in.
                if block && !self_closing && !matches!(n, "html" | "body") && decl.margin_left.unwrap_or(n == "blockquote") {
                    b.quote = b.quote.saturating_add(1);
                    if let Some(e) = b.stack.last_mut() {
                        e.set_in = true;
                    }
                }
                if self_closing {
                    if n == "hr" && b.skip == 0 {
                        b.push_block(Block {
                            kind: BlockKind::Rule,
                            indent: 0,
                            text: String::new(),
                            spans: Vec::new(),
                            cells: Vec::new(),
                            style: BlockStyle::default(),
                        });
                    }
                    continue;
                }
                match n {
                    "pre" => b.pre += 1,
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
                    // Only links inside the book: no scheme such as http:.
                    "a" if b.skip == 0 => {
                        let href = xml::attr(attrs, "href").map(xml::decode_entities);
                        b.link = href
                            .filter(|h| !h.is_empty() && !h.split('#').next().unwrap_or("").contains(':'))
                            .map(|h| (h, b.chapter.blocks.len(), b.text.len()));
                    }
                    _ => {}
                }
            }
            Token::End { name } => {
                let lname = local_name(name).to_ascii_lowercase();
                let n = lname.as_str();
                let open = b.close(n);
                if open.as_ref().is_some_and(|e| e.hidden) {
                    b.skip = b.skip.saturating_sub(1);
                    continue;
                }
                let block = open.as_ref().map_or(is_block_tag(n), |e| e.block);
                if matches!(n, "head" | "script" | "style" | "svg" | "math" | "title") {
                    b.skip = b.skip.saturating_sub(1);
                    if n == "svg" {
                        b.svg = b.svg.saturating_sub(1);
                    }
                    continue;
                }
                if b.skip == 0 && matches!(n, "table" | "tr" | "td" | "th" | "thead") {
                    match n {
                        "table" => {
                            if b.tables == 1 {
                                b.close_row();
                            }
                            b.tables = b.tables.saturating_sub(1);
                        }
                        "thead" => b.thead = false,
                        "tr" if b.tables == 1 => b.close_row(),
                        "td" | "th" if b.tables == 1 => b.close_cell(),
                        _ => {}
                    }
                    continue;
                }
                if block && b.row.is_some() {
                    b.cell_break();
                } else if block {
                    b.flush();
                    b.depth = b.depth.saturating_sub(1);
                    if b.figure.is_some_and(|(_, _, d)| b.depth < d) {
                        b.figure = None;
                    }
                    // Its margin below is its last block's.
                    if open.as_ref().is_some_and(|e| e.below && b.chapter.blocks.len() > e.first_block) {
                        if let Some(last) = b.chapter.blocks.last_mut() {
                            last.style.below = true;
                        }
                    }
                    // Text after it is set as the element around it says.
                    b.para = b.stack.last().map_or(BlockStyle::default(), Open::block_style);
                    if open.as_ref().is_some_and(|e| e.break_after) {
                        b.page_break = true;
                    }
                }
                if open.is_some_and(|e| e.set_in) {
                    b.quote = b.quote.saturating_sub(1);
                }
                match n {
                    "pre" => b.pre = b.pre.saturating_sub(1),
                    "ul" | "ol" => {
                        b.lists.pop();
                    }
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => b.heading = 0,
                    // Kept if its text is still in the block it began in.
                    "a" => {
                        if let Some((href, block, start)) = b.link.take() {
                            let text = b.text.get(start..).unwrap_or("");
                            if block == b.chapter.blocks.len() && text.chars().any(|c| !c.is_whitespace()) {
                                let (start, end) = (start as u32, b.text.trim_end().len() as u32);
                                b.chapter.links.push(Link { block: block as u32, start, end, href });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    b.close_row();
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
    fn book_css() {
        let sheet = Stylesheet::parse(
            ".c { text-align: center } .r { text-align: right } .nind { text-indent: 0 }\n\
             .note { display: none } span.line { display: block } h2 { font-weight: normal; page-break-before: always }\n\
             .epigraph { font-style: italic } .epigraph i { font-style: normal } .bold { font-weight: bold }\n\
             .letter { margin-top: 1em } .end { page-break-after: always }",
        );
        let doc = "<p>One</p><p class=\"nind\">Two <span class=\"note\">hidden</span>words</p>\
                   <div class=\"c\"><p>Centered</p></div><p class=\"r\">Right</p>\
                   <h2>Chapter</h2><div class=\"epigraph\"><p>Said <i>here</i></p></div>\
                   <p><span class=\"line\">Line one</span><span class=\"line\">Line two</span></p>\
                   <div class=\"letter\"><p class=\"end\">Dear <span class=\"bold\">Sir</span></p></div>\
                   <p style=\"text-align: center; font-style: italic; margin-bottom: 1em\">Last</p><p>After</p>";
        let c = parse_styled(doc, Some(&[&sheet]), |_| None);
        let texts: Vec<&str> = c.blocks.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(texts, ["One", "Two words", "Centered", "Right", "Chapter", "Said here", "Line one", "Line two", "Dear Sir", "Last", "After"]);
        let style = |i: usize| c.blocks[i].style;
        assert!(!style(0).flush && style(1).flush);
        assert_eq!((style(2).align, style(3).align, style(0).align), (Some(Align::Center), Some(Align::Right), None));
        // Headings are bold unless the book says not; italics can be undone.
        assert_eq!(c.blocks[4].style_at(0), 0);
        assert_eq!((c.blocks[5].style_at(0), c.blocks[5].style_at(5)), (ITALIC, 0));
        // The letter's margin is its first paragraph's; the last
        // paragraph's own margin is below it.
        assert!(style(8).above && !style(8).below && !style(9).above && style(9).below && !style(10).above);
        assert_eq!(c.blocks[8].style_at(5), BOLD);
        assert_eq!((style(9).align, c.blocks[9].style_at(0)), (Some(Align::Center), ITALIC));
        // Before the heading, and after the paragraph that ends a page.
        assert_eq!(c.breaks, [4, 9]);
        // Without CSS, the same blocks as tags alone make.
        let plain = parse_xhtml(doc, |_| None);
        assert_eq!(plain.blocks.len(), 10);
        assert!(plain.breaks.is_empty() && plain.blocks.iter().all(|b| b.style == BlockStyle::default()));
        assert_eq!(plain.blocks[4].style_at(0), BOLD);

        // Set in by a block quote, or by a margin; not where CSS takes it away.
        let sheet = Stylesheet::parse("blockquote.letter { margin: 1em 0 } .in { margin-left: 2em }");
        let c = parse_styled(
            "<blockquote><p>Quote</p></blockquote><blockquote class=\"letter\"><p>Letter</p></blockquote>\
             <div class=\"in\"><p>In</p></div><p>Out</p>",
            Some(&[&sheet]),
            |_| None,
        );
        let indents: Vec<u8> = c.blocks.iter().map(|b| b.indent).collect();
        assert_eq!(indents, [1, 0, 1, 0]);
    }

    #[test]
    fn tables() {
        let c = parse_xhtml(
            "<p>Before</p><table><thead><tr><th>Name</th><th>Note</th></tr></thead>\
             <tr><td> <a href=\"#x\">One</a></td><td>first <i>row</i><br/>two lines</td></tr>\
             <tr><td><p>Two</p><p>paras</p></td><td></td></tr></table><p>After</p>",
            |_| None,
        );
        let kinds: Vec<_> = c.blocks.iter().map(|b| b.kind).collect();
        assert_eq!(
            kinds,
            [
                BlockKind::Para,
                BlockKind::Row { table: 1, header: true },
                BlockKind::Row { table: 1, header: false },
                BlockKind::Row { table: 1, header: false },
                BlockKind::Para
            ]
        );
        let cells = |i: usize| -> Vec<&str> {
            let b = &c.blocks[i];
            b.cells.iter().map(|&(s, e)| &b.text[s as usize..e as usize]).collect()
        };
        assert_eq!(cells(1), ["Name", "Note"]);
        assert_eq!(c.blocks[1].style_at(0), BOLD);
        assert_eq!(cells(2), ["One", "first row\ntwo lines"]);
        assert_eq!(cells(3), ["Two\nparas", ""]);
        assert_eq!(c.links.len(), 1);
    }

    #[test]
    fn links_inside_the_book() {
        let c = parse_xhtml(
            "<p>Pemberley<a href=\"notes.xhtml#n1\"><sup>1</sup></a> and \
             <a href=\"https://example.com/\">a site</a>, see <a href=\"#ch2\">Chapter 2 </a>.</p>\
             <p><a href=\"#empty\"> </a><a href=\"#x\">across<br/>lines</a></p>",
            |_| None,
        );
        let links: Vec<_> = c
            .links
            .iter()
            .map(|l| (l.block, &c.blocks[l.block as usize].text[l.start as usize..l.end as usize], l.href.as_str()))
            .collect();
        assert_eq!(
            links,
            [(0, "1", "notes.xhtml#n1"), (0, "Chapter 2", "#ch2"), (1, "across\nlines", "#x")]
        );
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
