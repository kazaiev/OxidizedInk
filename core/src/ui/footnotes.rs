//! Footnotes, and other links inside the book. "Footnotes" in the reading
//! menu lists the links on the page; following one shows where it leads
//! as an ordinary page, and holding Menu goes back to the text. Meanwhile
//! the place saved for the book stays the one in the text.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::*;
use crate::layout::Page;

/// Height of a row in the list: the link's text, then the words before it.
const ROW: i32 = 76;
/// How many characters before a link the list shows.
const CONTEXT: usize = 60;

impl<F: ReadAt> Reader<F> {
    /// Indices in `chapter.links` of the links that start on this page and
    /// lead into the text (not to a picture's file, say).
    pub(super) fn links_here(&self) -> Vec<usize> {
        let (start, end) = (self.pos(), self.starts.get(self.page + 1).copied());
        let on_page = |p: Pos| p >= start && end.is_none_or(|e| p < e);
        let links = self.chapter.links.iter().enumerate();
        links
            .filter(|(_, l)| on_page(Pos { block: l.block, byte: l.start }))
            .filter(|(_, l)| self.epub.link_target(self.spine, &l.href).is_some())
            .map(|(i, _)| i)
            .collect()
    }

    /// Goes where link `i` of the chapter leads, remembering the place to
    /// come back to unless already away from the text.
    fn follow(&mut self, layout: &Layout, i: usize) -> bool {
        let Some(link) = self.chapter.links.get(i) else { return false };
        let Some((spine, id)) = self.epub.link_target(self.spine, &link.href) else { return false };
        let here = Away { spine: self.spine, pos: self.pos(), percent: self.percent(), target: None };
        if spine != self.spine && !self.goto(layout, spine, Target::First) {
            return false;
        }
        let block = id.and_then(|id| self.chapter.anchor(&id)).filter(|_| self.spine == spine);
        match block {
            Some(block) => self.page = page_of(&self.starts, Pos::new(block, 0)),
            None if self.spine == here.spine => self.page = 0,
            None => {}
        }
        let away = self.away.get_or_insert(here);
        away.target = block.map(|b| (spine, b));
        true
    }

    /// Goes back to the text from a link's target.
    pub(super) fn go_back(&mut self, layout: &Layout) -> bool {
        let Some(Away { spine, pos, .. }) = self.away.take() else { return false };
        if spine == self.spine {
            self.page = page_of(&self.starts, pos);
            true
        } else {
            self.goto(layout, spine, Target::At(pos))
        }
    }

    /// Link `i`'s text and the words before it.
    fn link_text(&self, i: usize) -> (String, String) {
        let l = &self.chapter.links[i];
        let text = &self.chapter.blocks[l.block as usize].text;
        let label = text.get(l.start as usize..l.end as usize).unwrap_or("").trim().replace('\n', " ");
        let before = text.get(..l.start as usize).unwrap_or("").trim_end();
        let skip = before.chars().count().saturating_sub(CONTEXT);
        let words: String = before.chars().skip(skip).map(|c| if c == '\n' { ' ' } else { c }).collect();
        (label, if skip > 0 { format!("…{}", words.trim_start()) } else { words })
    }
}

/// Marks the block a link led to, if it's on `page`, with a bar in the
/// left margin, `side` pixels wide, beside its lines.
pub(super) fn mark_target<F: ReadAt>(canvas: &mut Canvas, r: &Reader<F>, page: &Page, side: i32) {
    let Some((spine, block)) = r.away.and_then(|a| a.target) else { return };
    if spine != r.spine {
        return;
    }
    let lines = page.items.iter().filter_map(|i| match i {
        Item::Text(l) if l.block as usize == block => Some((l.baseline - l.px * 4 / 5, l.baseline + l.px / 4)),
        _ => None,
    });
    if let Some((top, bottom)) = lines.reduce(|(t, _), (_, b)| (t, b)) {
        canvas.fill_rect(side / 2 - 2, top, 4, bottom - top, BLACK);
    }
}

impl<S: Storage> App<S> {
    /// Opens the page's links: the list, or the one link's target.
    pub(super) fn open_footnotes(&mut self) {
        let Some(r) = &mut self.reader else { return };
        let links = r.links_here();
        if let [only] = links[..] {
            r.follow(&layout(&self.fonts, &self.settings, r.lang), only);
            self.save_progress();
            self.screen = Screen::Reading;
        } else {
            self.screen = Screen::Footnotes { sel: 0 };
        }
    }

    pub(super) fn handle_footnotes(&mut self, input: Input, sel: usize) -> bool {
        let Some(r) = &mut self.reader else { return false };
        let links = r.links_here();
        let n = links.len();
        if n == 0 {
            self.screen = Screen::Reading;
            return true;
        }
        match input {
            Input::Prev => self.screen = Screen::Footnotes { sel: (sel + n - 1) % n },
            Input::Next => self.screen = Screen::Footnotes { sel: (sel + 1) % n },
            Input::SkipBack | Input::SkipForward => {
                let forward = input == Input::SkipForward;
                self.screen = Screen::Footnotes { sel: skip_rows(sel, n, rows_per_page(ROW), forward) };
            }
            Input::Back => self.screen = Screen::Reading,
            Input::Menu => {
                r.follow(&layout(&self.fonts, &self.settings, r.lang), links[sel.min(n - 1)]);
                self.save_progress();
                self.screen = Screen::Reading;
            }
        }
        true
    }

    /// The page's links, a row each: the link's text, then the words before
    /// it.
    pub(super) fn draw_footnotes(&mut self, canvas: &mut Canvas, sel: usize) {
        self.draw_header(canvas, "Footnotes");
        let Some(r) = &self.reader else { return };
        let rows: Vec<_> = r.links_here().into_iter().map(|i| r.link_text(i)).collect();
        let x = MARGIN_X + 12;
        let max = WIDTH - MARGIN_X - 12 - x;
        let per_page = rows_per_page(ROW);
        let first = sel / per_page * per_page;
        for (k, (label, before)) in rows.iter().enumerate().skip(first).take(per_page) {
            let y = LIST_TOP + (k - first) as i32 * ROW;
            let label = fit(&self.fonts, 0, 22, label, max);
            self.text(canvas, 0, 22, x, y + 30, &label);
            let before = fit(&self.fonts, ITALIC, 18, before, max);
            self.text(canvas, ITALIC, 18, x, y + 60, &before);
            if k == sel {
                canvas.invert_rect(MARGIN_X, y + 2, WIDTH - 2 * MARGIN_X, ROW - 4);
            }
        }
        self.draw_list_page(canvas, sel, per_page, rows.len());
    }
}
