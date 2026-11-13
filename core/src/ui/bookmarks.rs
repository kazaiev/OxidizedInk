//! Bookmarks: pages marked from the reading menu. Each book's are kept in
//! its `.BMK` file beside the `.POS`, a line each, in reading order:
//! `spine block byte percent`, then a tab and the chapter's title, then a
//! tab and the page's first words, for the list.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::*;

/// How much of a page's text a bookmark keeps, in characters.
const WORDS: usize = 80;
/// Height of a row in the bookmark list: its place, then its words.
const ROW: i32 = 76;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Bookmark {
    pub spine: usize,
    pub pos: Pos,
    pub percent: u32,
    /// The title of the chapter it's in, from the TOC; may be empty.
    pub chapter: String,
    /// The first words of the page.
    pub words: String,
}

/// Reads a `.BMK` file. Lines that don't parse are skipped.
pub(super) fn parse(data: &[u8]) -> Vec<Bookmark> {
    let text = core::str::from_utf8(data).unwrap_or("");
    let mut marks: Vec<Bookmark> = text
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let mut n = parts.next()?.split_whitespace().map(|n| n.parse::<u32>().ok());
            let (spine, block, byte, percent) = (n.next()??, n.next()??, n.next()??, n.next()??);
            Some(Bookmark {
                spine: spine as usize,
                pos: Pos { block, byte },
                percent,
                chapter: parts.next().unwrap_or("").into(),
                words: parts.next().unwrap_or("").into(),
            })
        })
        .collect();
    marks.sort_by_key(|b| (b.spine, b.pos));
    marks
}

pub(super) fn to_text(marks: &[Bookmark]) -> String {
    let mut out = String::new();
    for b in marks {
        let line = format!("{} {} {} {}\t{}\t{}\n", b.spine, b.pos.block, b.pos.byte, b.percent, b.chapter, b.words);
        out.push_str(&line);
    }
    out
}

impl<F: ReadAt> Reader<F> {
    /// Index of the bookmark on the current page, if it has one.
    pub(super) fn bookmark_here(&self) -> Option<usize> {
        let (start, end) = (self.pos(), self.starts.get(self.page + 1).copied());
        self.bookmarks
            .iter()
            .position(|b| b.spine == self.spine && b.pos >= start && end.is_none_or(|e| b.pos < e))
    }

    /// A bookmark for the current page.
    fn bookmark(&self) -> Bookmark {
        let chapter = self.toc_index().map(|i| self.epub.toc[i].title.clone()).unwrap_or_default();
        Bookmark { spine: self.spine, pos: self.pos(), percent: self.percent(), chapter, words: self.first_words() }
    }

    /// The page's first words, on one line.
    fn first_words(&self) -> String {
        let pos = self.pos();
        let mut out = String::new();
        let mut count = 0;
        for (i, b) in self.chapter.blocks.iter().enumerate().skip(pos.block as usize) {
            let from = if i == pos.block as usize { pos.byte as usize } else { 0 };
            for c in b.text.get(from..).unwrap_or("").chars().chain([' ']) {
                if count >= WORDS {
                    return String::from(out.trim_end());
                }
                let c = if c.is_whitespace() { ' ' } else { c };
                if crate::fonts::is_invisible(c) || (c == ' ' && (out.is_empty() || out.ends_with(' '))) {
                    continue;
                }
                out.push(c);
                count += 1;
            }
        }
        String::from(out.trim_end())
    }
}

impl<S: Storage> App<S> {
    /// Adds a bookmark for the current page, or removes the one it has.
    pub(super) fn toggle_bookmark(&mut self) {
        let Some(r) = &mut self.reader else { return };
        match r.bookmark_here() {
            Some(i) => {
                r.bookmarks.remove(i);
            }
            None => {
                let b = r.bookmark();
                let at = r.bookmarks.partition_point(|m| (m.spine, m.pos) < (b.spine, b.pos));
                r.bookmarks.insert(at, b);
            }
        }
        let key = book_file(&r.info, "BMK");
        if r.bookmarks.is_empty() {
            self.storage.remove(&key);
        } else {
            let _ = self.storage.save(&key, to_text(&r.bookmarks).as_bytes());
        }
    }

    pub(super) fn handle_bookmarks(&mut self, input: Input, sel: usize) -> bool {
        let Some(r) = &mut self.reader else { return false };
        let n = r.bookmarks.len();
        if n == 0 {
            self.screen = Screen::Reading;
            return true;
        }
        match input {
            Input::Prev => self.screen = Screen::Bookmarks { sel: (sel + n - 1) % n },
            Input::Next => self.screen = Screen::Bookmarks { sel: (sel + 1) % n },
            Input::SkipBack | Input::SkipForward => {
                let forward = input == Input::SkipForward;
                self.screen = Screen::Bookmarks { sel: skip_rows(sel, n, rows_per_page(ROW), forward) };
            }
            Input::Back => self.screen = Screen::Reading,
            Input::Menu => {
                let b = &r.bookmarks[sel];
                let (spine, pos) = (b.spine, b.pos);
                r.away = None;
                if spine == r.spine {
                    r.page = page_of(&r.starts, pos);
                } else {
                    r.goto(&layout(&self.fonts, &self.settings, r.lang), spine, Target::At(pos));
                }
                self.save_progress();
                self.screen = Screen::Reading;
            }
        }
        true
    }

    /// The bookmarks, a row each: percent and chapter, then the page's
    /// first words.
    pub(super) fn draw_bookmarks(&mut self, canvas: &mut Canvas, sel: usize) {
        self.draw_header(canvas, "Bookmarks");
        let Some(r) = &self.reader else { return };
        let marks = r.bookmarks.clone();
        let x = MARGIN_X + 12;
        let max = WIDTH - MARGIN_X - 12 - x;
        if marks.is_empty() {
            self.text(canvas, 0, 22, x, LIST_TOP + 30, "No bookmarks yet.");
            self.text(canvas, ITALIC, 18, x, LIST_TOP + 66, "\"Add bookmark\" in the menu marks a page.");
            return;
        }
        let per_page = rows_per_page(ROW);
        let first = sel / per_page * per_page;
        for (k, b) in marks.iter().enumerate().skip(first).take(per_page) {
            let y = LIST_TOP + (k - first) as i32 * ROW;
            let place = if b.chapter.is_empty() { format!("{}%", b.percent) } else { format!("{}%  ·  {}", b.percent, b.chapter) };
            let place = fit(&self.fonts, 0, 22, &place, max);
            self.text(canvas, 0, 22, x, y + 30, &place);
            let words = fit(&self.fonts, ITALIC, 18, &b.words, max);
            self.text(canvas, ITALIC, 18, x, y + 60, &words);
            if k == sel {
                canvas.invert_rect(MARGIN_X, y + 2, WIDTH - 2 * MARGIN_X, ROW - 4);
            }
        }
        self.draw_list_page(canvas, sel, per_page, marks.len());
    }
}

/// Draws the ribbon that marks a bookmarked page, at the top of the page's
/// right margin, `side` pixels wide.
pub(super) fn draw_ribbon(canvas: &mut Canvas, side: i32) {
    let (w, h, tail) = (14, 30, 7);
    let x = WIDTH - side + (side - w) / 2;
    canvas.fill_rect(x, 0, w, h - tail, BLACK);
    // A forked end.
    for i in 0..tail {
        let part = w / 2 - i;
        canvas.fill_rect(x, h - tail + i, part, 1, BLACK);
        canvas.fill_rect(x + w - part, h - tail + i, part, 1, BLACK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmark_files_round_trip() {
        let mark = |spine, block, chapter: &str| Bookmark {
            spine,
            pos: Pos { block, byte: 7 },
            percent: 40,
            chapter: chapter.into(),
            words: String::from("It is a truth universally acknowledged"),
        };
        let marks = [mark(1, 5, "CHAPTER I."), mark(3, 2, "")];
        let text = to_text(&marks);
        assert_eq!(parse(text.as_bytes()), marks);
        // Out of order, with a broken line: sorted, and the line skipped.
        let shuffled = format!("{}broken\n{}", to_text(&marks[1..]), to_text(&marks[..1]));
        assert_eq!(parse(shuffled.as_bytes()), marks);
    }
}
