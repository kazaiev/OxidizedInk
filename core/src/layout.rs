//! Line breaking and pagination.
//!
//! A position in a chapter is a `Pos`: a block index and a byte offset into
//! that block's text. Positions don't depend on the font size or the page
//! geometry, so saved reading progress survives a settings change.
//!
//! Lines are broken greedily at spaces, after dashes inside words, and at soft
//! hyphens. A word wider than the line is broken by character. Paragraph
//! lines are justified except the last one, unless set ragged right.

use alloc::vec::Vec;

use crate::fonts::{Fonts, Meter};
use crate::text::{Block, BlockKind, Chapter};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub block: u32,
    pub byte: u32,
}

impl Pos {
    pub fn new(block: usize, byte: usize) -> Self {
        Pos { block: block as u32, byte: byte as u32 }
    }
}

/// Text in one face, drawn from `x` (1/64 px) on its line's baseline.
#[derive(Debug, Clone)]
pub struct Run {
    pub x: i32,
    pub start: u32,
    pub end: u32,
    pub face: u8,
}

#[derive(Debug, Clone)]
pub struct Line {
    pub block: u32,
    /// Baseline y in pixels.
    pub baseline: i32,
    /// Font size in pixels per em.
    pub px: i32,
    pub runs: Vec<Run>,
    /// A hyphen added where the line broke at a soft hyphen: `(x, face)`.
    pub hyphen: Option<(i32, u8)>,
}

#[derive(Debug, Clone)]
pub enum Item {
    Text(Line),
    /// A horizontal rule, in pixels.
    Rule { x0: i32, x1: i32, y: i32 },
}

#[derive(Debug, Clone, Default)]
pub struct Page {
    pub start: Pos,
    /// Where the next page starts. `end.block == blocks.len()` at the end of
    /// the chapter.
    pub end: Pos,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Align {
    Justify,
    Left,
    Center,
}

/// How one block is set.
struct Style {
    px: i32,
    line_h: i32,
    /// Left edge and width of the block's text, in pixels.
    left: i32,
    width: i32,
    /// Extra indent of the first line, in pixels.
    first_indent: i32,
    /// Vertical space before the block, in pixels (dropped at a page top).
    gap: i32,
    align: Align,
}

/// Where a line ends.
#[derive(Debug, Clone, Copy)]
struct Break {
    /// End of the line's text (exclusive).
    end: usize,
    /// Start of the next line.
    next: usize,
    /// Natural width in font units, including an added hyphen.
    width: i32,
    /// Spaces inside the line, which justification stretches.
    spaces: i32,
    hyphen: bool,
    /// The last line of a paragraph or before a `<br>`: never justified.
    last: bool,
}

/// Finds the style in effect at a byte offset, for bytes visited in order.
struct Styles<'a> {
    spans: &'a [(u32, u8)],
    i: usize,
}

impl<'a> Styles<'a> {
    fn new(b: &'a Block, from: usize) -> Self {
        let i = b.spans.partition_point(|&(o, _)| o as usize <= from).saturating_sub(1);
        Styles { spans: &b.spans, i }
    }

    fn at(&mut self, byte: usize) -> u8 {
        while self.i + 1 < self.spans.len() && self.spans[self.i + 1].0 as usize <= byte {
            self.i += 1;
        }
        self.spans.get(self.i).map_or(0, |&(_, s)| s)
    }
}

fn is_dash(c: char) -> bool {
    matches!(c, '-' | '\u{2013}' | '\u{2014}')
}

fn is_plain(b: &Block) -> bool {
    b.kind == BlockKind::Para && b.indent == 0
}

/// Page geometry and type settings.
pub struct Layout<'f> {
    pub fonts: &'f Fonts,
    /// Content box on the canvas, in pixels.
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    /// Body text size in pixels per em.
    pub font_px: i32,
    /// Line height as a percentage of the font size.
    pub line_pct: i32,
    /// Justify paragraph lines; otherwise they are ragged right.
    pub justify: bool,
    /// Separate paragraphs with space instead of a first-line indent.
    pub spaced: bool,
}

impl<'f> Layout<'f> {
    fn line_height(&self, px: i32) -> i32 {
        px * self.line_pct / 100
    }

    fn style(&self, ch: &Chapter, i: usize) -> Style {
        let b = &ch.blocks[i];
        let prev = i.checked_sub(1).map(|j| &ch.blocks[j]);
        let body = self.font_px;
        let body_line = self.line_height(body);
        let (px, align) = match b.kind {
            BlockKind::Heading(1) => (body * 3 / 2, Align::Center),
            BlockKind::Heading(2) => (body * 13 / 10, Align::Center),
            BlockKind::Heading(3) => (body * 23 / 20, Align::Center),
            BlockKind::Heading(_) => (body, Align::Left),
            BlockKind::Pre => (body * 9 / 10, Align::Left),
            BlockKind::Center => (body, Align::Center),
            BlockKind::Para | BlockKind::Rule if self.justify => (body, Align::Justify),
            BlockKind::Para | BlockKind::Rule => (body, Align::Left),
        };
        let indent = (b.indent.min(4) as i32 * body * 3 / 2).min(self.width / 3);
        let follows_para = is_plain(b) && prev.is_some_and(is_plain);
        let first_indent = if follows_para && !self.spaced { body * 3 / 2 } else { 0 };
        let heading = |b: &Block| matches!(b.kind, BlockKind::Heading(_));
        let gap = match prev {
            None => 0,
            Some(p) if heading(b) && heading(p) => body_line / 4,
            Some(_) if heading(b) => body_line,
            Some(p) if heading(p) => body_line / 2,
            Some(_) if follows_para => if self.spaced { body_line / 2 } else { 0 },
            Some(p) if p.kind == b.kind && p.indent == b.indent => body_line / 3,
            Some(_) => body_line / 2,
        };
        Style {
            px,
            line_h: self.line_height(px),
            left: self.left + indent,
            width: self.width - indent,
            first_indent,
            gap,
            align,
        }
    }

    /// Finds the end of the line that starts at byte `start` of `b`, given
    /// `avail` font units of width.
    fn break_line(&self, b: &Block, start: usize, avail: i32) -> Break {
        let pre = b.kind == BlockKind::Pre;
        let mut meter = Meter::new(self.fonts);
        let mut styles = Styles::new(b, start);
        let mut cand: Option<Break> = None;
        let mut width = 0;
        let mut spaces = 0;
        let text = &b.text[start..];
        let mut chars = text.char_indices().peekable();
        let mut prev = ' ';
        while let Some((rel, c)) = chars.next() {
            let i = start + rel;
            let face = styles.at(i);
            match c {
                '\n' => {
                    return Break { end: i, next: i + 1, width, spaces, hyphen: false, last: true };
                }
                ' ' => {
                    if i > start {
                        cand = Some(Break { end: i, next: i + 1, width, spaces, hyphen: false, last: false });
                    }
                    meter.reset();
                    width += self.fonts.glyph(face, ' ').1;
                    spaces += 1;
                }
                '\u{ad}' | '\u{200b}' if !pre => {
                    let hyphen = c == '\u{ad}';
                    let w = width + if hyphen { self.fonts.glyph(face, '-').1 } else { 0 };
                    if w <= avail && i > start {
                        let next = i + c.len_utf8();
                        cand = Some(Break { end: i, next, width: w, spaces, hyphen, last: false });
                    }
                }
                _ => {
                    let w = width + meter.next(face, c).2;
                    if w > avail && i > start {
                        return cand.unwrap_or(Break {
                            end: i,
                            next: i,
                            width,
                            spaces,
                            hyphen: false,
                            last: false,
                        });
                    }
                    width = w;
                    let next_is_letter = chars.peek().is_some_and(|&(_, n)| !n.is_whitespace());
                    if is_dash(c) && prev != ' ' && next_is_letter && !pre {
                        let next = i + c.len_utf8();
                        cand = Some(Break { end: next, next, width, spaces, hyphen: false, last: false });
                    }
                }
            }
            prev = c;
        }
        Break { end: b.text.len(), next: b.text.len(), width, spaces, hyphen: false, last: true }
    }

    /// Positions the runs of one line.
    fn build_line(&self, bi: usize, b: &Block, start: usize, br: &Break, st: &Style, baseline: i32) -> Line {
        let fonts = self.fonts;
        let first = start == 0;
        let indent = if first { st.first_indent } else { 0 };
        let avail = fonts.px_to_units(st.width - indent, st.px);
        let slack = (avail - br.width).max(0);
        let (mut x, stretch) = match st.align {
            Align::Center => (slack / 2, 0),
            Align::Justify if !br.last && br.spaces > 0 && slack / br.spaces <= fonts.units_per_em => {
                (0, slack)
            }
            _ => (0, 0),
        };
        let origin = (st.left + indent) * 64;
        let mut runs = Vec::new();
        let mut meter = Meter::new(fonts);
        let mut styles = Styles::new(b, start);
        let mut run: Option<Run> = None;
        let mut seen_spaces = 0;
        let mut face = 0;
        for (rel, c) in b.text[start..br.end].char_indices() {
            let i = start + rel;
            face = styles.at(i);
            if c == ' ' {
                runs.extend(run.take());
                meter.reset();
                x += fonts.glyph(face, ' ').1;
                // Spread the stretch evenly; the first spaces take the remainder.
                seen_spaces += 1;
                let n = br.spaces.max(1);
                x += stretch * seen_spaces / n - stretch * (seen_spaces - 1) / n;
                continue;
            }
            if run.as_ref().is_some_and(|r| r.face != face) {
                runs.extend(run.take());
                meter.reset();
            }
            let r = run.get_or_insert(Run { x: origin + fonts.to_fx(x, st.px), start: i as u32, end: i as u32, face });
            r.end = (i + c.len_utf8()) as u32;
            x += meter.next(face, c).2;
        }
        runs.extend(run);
        let hyphen = br.hyphen.then(|| (origin + fonts.to_fx(x, st.px), face));
        Line { block: bi as u32, baseline, px: st.px, runs, hyphen }
    }

    /// Height of a whole block, for keeping a heading with the text after it.
    fn block_height(&self, b: &Block, st: &Style) -> i32 {
        let mut byte = 0;
        let mut lines = 0;
        while byte < b.text.len() {
            let indent = if byte == 0 { st.first_indent } else { 0 };
            let avail = self.fonts.px_to_units(st.width - indent, st.px);
            byte = self.break_line(b, byte, avail).next;
            lines += 1;
        }
        lines * st.line_h
    }

    /// Lays out the page that starts at `start`. Only measures unless `out`
    /// is given; returns where the next page starts.
    fn fill(&self, ch: &Chapter, start: Pos, mut out: Option<&mut Vec<Item>>) -> Pos {
        let fonts = self.fonts;
        let bottom = self.top + self.height;
        let mut y = self.top;
        let mut empty = true;
        let mut bi = start.block as usize;
        let mut byte = start.byte as usize;
        while bi < ch.blocks.len() {
            let b = &ch.blocks[bi];
            let st = self.style(ch, bi);
            if byte == 0 && !empty && ch.breaks.binary_search(&bi).is_ok() {
                return Pos::new(bi, 0);
            }
            if byte == 0 && !empty {
                let mut need = st.gap + st.line_h;
                if matches!(b.kind, BlockKind::Heading(_)) && bi + 1 < ch.blocks.len() {
                    let next = self.style(ch, bi + 1);
                    need = st.gap + self.block_height(b, &st) + next.gap + next.line_h;
                }
                if y + need > bottom {
                    return Pos::new(bi, 0);
                }
                y += st.gap;
            }
            if b.kind == BlockKind::Rule {
                if let Some(out) = out.as_deref_mut() {
                    let w = self.width * 2 / 5;
                    let x0 = self.left + (self.width - w) / 2;
                    out.push(Item::Rule { x0, x1: x0 + w, y: y + st.line_h / 2 });
                }
                y += st.line_h;
                empty = false;
            }
            // Center the font's ascent + descent in the line box.
            let font_h = fonts.to_fx(fonts.ascent + fonts.descent, st.px) / 64;
            let ascent = fonts.to_fx(fonts.ascent, st.px) / 64;
            let baseline_off = (st.line_h - font_h) / 2 + ascent;
            while byte < b.text.len() {
                if !empty && y + st.line_h > bottom {
                    return Pos::new(bi, byte);
                }
                let indent = if byte == 0 { st.first_indent } else { 0 };
                let avail = fonts.px_to_units(st.width - indent, st.px);
                let br = self.break_line(b, byte, avail);
                if let Some(out) = out.as_deref_mut() {
                    out.push(Item::Text(self.build_line(bi, b, byte, &br, &st, y + baseline_off)));
                }
                y += st.line_h;
                empty = false;
                byte = br.next;
            }
            bi += 1;
            byte = 0;
        }
        Pos::new(ch.blocks.len(), 0)
    }

    /// Lays out the page that starts at `start`.
    pub fn page(&self, ch: &Chapter, start: Pos) -> Page {
        let mut items = Vec::new();
        let end = self.fill(ch, start, Some(&mut items));
        Page { start, end, items }
    }

    /// Start positions of every page in the chapter (at least one).
    pub fn paginate(&self, ch: &Chapter) -> Vec<Pos> {
        let end = Pos::new(ch.blocks.len(), 0);
        let mut starts = Vec::new();
        let mut pos = Pos::default();
        loop {
            starts.push(pos);
            let next = self.fill(ch, pos, None);
            if next >= end || next <= pos {
                return starts;
            }
            pos = next;
        }
    }
}

/// Index of the page in `starts` that contains `pos`.
pub fn page_of(starts: &[Pos], pos: Pos) -> usize {
    starts.partition_point(|&s| s <= pos).saturating_sub(1)
}
