//! The reader's screens and navigation, driven by four abstract inputs.
//!
//! Screens: the library (books on the card), the reading view, a menu over
//! the page, the table of contents and the settings. All state that must
//! survive sleep or power-off goes through `Storage` as small text files.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::epub::Epub;
use crate::fonts::{Fonts, Meter};
use crate::layout::{page_of, Layout, Pos};
use crate::render::{Canvas, Renderer, BLACK, WHITE};
use crate::settings::{Settings, StatusLine, SETTINGS, TEXT_SIZE};
use crate::text::{Chapter, BOLD, ITALIC};
use crate::{Error, ReadAt};

/// Portrait canvas size in pixels.
pub const WIDTH: i32 = 600;
pub const HEIGHT: i32 = 800;

/// Side margin of the menus and lists. The reading view's margins are a
/// setting.
const MARGIN_X: i32 = 36;
/// Bottom of the body text when the status line is shown below it.
const TEXT_BOTTOM: i32 = 752;
const STATUS_PX: i32 = 16;
const STATUS_BASELINE: i32 = 784;

const LAST_FILE: &str = "LAST.TXT";
const SETTINGS_FILE: &str = "SETTINGS.TXT";
const LIBRARY_FILE: &str = "LIBRARY.TXT";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Prev,
    Next,
    /// Opens the menu; selects inside menus and lists.
    Menu,
    /// Leaves a menu or list.
    Back,
}

/// Book files and small state files. On the device both live on the SD card
/// (state in `/READER/`); the simulator uses directories.
pub trait Storage {
    type Book: ReadAt;
    /// `(file name, size in bytes)` of each book.
    fn list_books(&mut self) -> Vec<(String, u64)>;
    fn open_book(&mut self, name: &str) -> Result<Self::Book, Error>;
    /// Reads a state file. Names are 8.3 (`LAST.TXT`, `1A2B3C4D.POS`).
    fn load(&mut self, name: &str) -> Option<Vec<u8>>;
    fn save(&mut self, name: &str, data: &[u8]) -> Result<(), Error>;
}

#[derive(Debug, Clone)]
pub struct BookInfo {
    pub file: String,
    pub size: u64,
    pub title: String,
    pub author: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Screen {
    Library { sel: usize },
    Reading,
    Menu { sel: usize },
    Toc { sel: usize },
    /// `from_reading`: opened from the reading menu rather than the library.
    Settings { sel: usize, from_reading: bool },
    Message(String),
}

const MENU: [&str; 6] = ["Back to book", "Contents", "Larger text", "Smaller text", "Settings", "Library"];

/// Where to land when entering a chapter.
#[derive(Clone, Copy)]
enum Target {
    First,
    Last,
    At(Pos),
}

/// The open book and the current chapter, paginated.
struct Reader<F: ReadAt> {
    info: BookInfo,
    epub: Epub<F>,
    spine: usize,
    chapter: Chapter,
    /// Page start positions in the current chapter.
    starts: Vec<Pos>,
    page: usize,
    /// TOC entries inside the current chapter: `(toc index, block)`.
    marks: Vec<(usize, usize)>,
}

impl<F: ReadAt> Reader<F> {
    fn pos(&self) -> Pos {
        self.starts[self.page]
    }

    /// Loads spine item `spine`, skipping items without text (such as cover
    /// images) in the direction of travel. Returns false if there is nothing
    /// to move to, leaving the reader unchanged.
    fn goto(&mut self, layout: &Layout, spine: usize, target: Target) -> bool {
        let backward = matches!(target, Target::Last);
        let mut s = spine;
        let chapter = loop {
            if s >= self.epub.spine.len() {
                return false;
            }
            match self.epub.chapter(s) {
                Ok(ch) if !ch.blocks.is_empty() => break ch,
                _ => {}
            }
            if backward {
                match s.checked_sub(1) {
                    Some(p) => s = p,
                    None => return false,
                }
            } else {
                s += 1;
            }
        };
        self.chapter = chapter;
        self.spine = s;
        self.marks = self
            .epub
            .toc
            .iter()
            .enumerate()
            .filter(|(_, e)| e.spine == s)
            .map(|(i, e)| (i, e.fragment.as_deref().and_then(|f| self.chapter.anchor(f)).unwrap_or(0)))
            .collect();
        // Each TOC entry starts a page, so jumping to one shows it at the top.
        let mut breaks: Vec<usize> = self.marks.iter().map(|&(_, b)| b).filter(|&b| b > 0).collect();
        breaks.sort_unstable();
        breaks.dedup();
        self.chapter.breaks = breaks;
        self.starts = layout.paginate(&self.chapter);
        self.page = match target {
            Target::First => 0,
            Target::Last => self.starts.len() - 1,
            Target::At(p) if s == spine => page_of(&self.starts, p),
            Target::At(_) => 0,
        };
        true
    }

    fn repaginate(&mut self, layout: &Layout) {
        let pos = self.pos();
        self.starts = layout.paginate(&self.chapter);
        self.page = page_of(&self.starts, pos);
    }

    fn next_page(&mut self, layout: &Layout) -> bool {
        if self.page + 1 < self.starts.len() {
            self.page += 1;
            true
        } else {
            self.goto(layout, self.spine + 1, Target::First)
        }
    }

    fn prev_page(&mut self, layout: &Layout) -> bool {
        if self.page > 0 {
            self.page -= 1;
            true
        } else if self.spine > 0 {
            self.goto(layout, self.spine - 1, Target::Last)
        } else {
            false
        }
    }

    /// Index of the TOC entry the current page is in.
    fn toc_index(&self) -> Option<usize> {
        let block = self.pos().block as usize;
        let here = self.marks.iter().rev().find(|&&(_, b)| b <= block).map(|&(i, _)| i);
        here.or_else(|| self.epub.toc.iter().rposition(|e| e.spine < self.spine))
    }

    /// Overall progress through the book in percent, by spine item sizes.
    fn percent(&self) -> u32 {
        let total = self.epub.total_size().max(1);
        let size = self.epub.spine[self.spine].size.max(1) as u64;
        let done = self.epub.size_before(self.spine) + size * self.page as u64 / self.starts.len() as u64;
        (done * 100 / total) as u32
    }
}

fn layout<'f>(fonts: &'f Fonts, s: &Settings) -> Layout<'f> {
    let (side, top) = s.margins();
    let bottom = if s.status_line() == StatusLine::Off { HEIGHT - top } else { TEXT_BOTTOM };
    Layout {
        fonts,
        left: side,
        top,
        width: WIDTH - 2 * side,
        height: bottom - top,
        font_px: s.font_px(),
        line_pct: s.line_pct(),
        justify: s.justify(),
        spaced: s.spaced(),
    }
}

/// FNV-1a over the file name and size, for naming a book's progress file.
fn book_key(info: &BookInfo) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for b in info.file.bytes().chain(info.size.to_le_bytes()) {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    format!("{:08X}.POS", h)
}

fn numbers(data: &[u8]) -> Vec<u64> {
    core::str::from_utf8(data)
        .unwrap_or("")
        .split_whitespace()
        .filter_map(|w| w.parse().ok())
        .collect()
}

/// Shortens `s` with an ellipsis to fit `max` pixels.
fn fit(fonts: &Fonts, face: u8, px: i32, s: &str, max: i32) -> String {
    let max = fonts.px_to_units(max, px);
    if fonts.measure(face, s) <= max {
        return s.to_string();
    }
    let ellipsis = fonts.measure(face, "…");
    let mut meter = Meter::new(fonts);
    let mut width = 0;
    let mut out = String::new();
    for c in s.chars() {
        width += meter.next(face, c).2;
        if width + ellipsis > max {
            break;
        }
        out.push(c);
    }
    let keep = out.trim_end().len();
    out.truncate(keep);
    out.push('…');
    out
}

pub struct App<S: Storage> {
    storage: S,
    fonts: Fonts,
    renderer: Renderer,
    settings: Settings,
    screen: Screen,
    library: Vec<BookInfo>,
    reader: Option<Reader<S::Book>>,
    /// Battery charge in percent, shown in the menu when known.
    pub battery: Option<u8>,
}

impl<S: Storage> App<S> {
    pub fn new(storage: S) -> Self {
        App {
            storage,
            fonts: Fonts::new(),
            renderer: Renderer::new(),
            settings: Settings::default(),
            screen: Screen::Library { sel: 0 },
            library: Vec::new(),
            reader: None,
            battery: None,
        }
    }

    pub fn storage(&mut self) -> &mut S {
        &mut self.storage
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Restores settings and reopens the last book at its saved page, or
    /// shows the library.
    pub fn start(&mut self) {
        if let Some(d) = self.storage.load(SETTINGS_FILE) {
            self.settings = Settings::parse(&d);
        }
        self.scan_library();
        let last = self.storage.load(LAST_FILE).and_then(|d| String::from_utf8(d).ok());
        if let Some(i) = last.and_then(|f| self.library.iter().position(|b| b.file == f.trim())) {
            self.open_book(i);
        } else {
            self.screen = Screen::Library { sel: 0 };
        }
    }

    /// Lists the books, reading titles from the cache file and opening only
    /// books that are new or changed.
    fn scan_library(&mut self) {
        let cached: Vec<BookInfo> = self
            .storage
            .load(LIBRARY_FILE)
            .and_then(|d| String::from_utf8(d).ok())
            .map(|s| {
                s.lines()
                    .filter_map(|l| {
                        let mut f = l.split('\t');
                        Some(BookInfo {
                            file: f.next()?.to_string(),
                            size: f.next()?.parse().ok()?,
                            title: f.next()?.to_string(),
                            author: f.next().unwrap_or("").to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut books = Vec::new();
        let mut changed = false;
        for (file, size) in self.storage.list_books() {
            if let Some(b) = cached.iter().find(|b| b.file == file && b.size == size) {
                books.push(b.clone());
                continue;
            }
            changed = true;
            let opened = self.storage.open_book(&file).and_then(Epub::open);
            let (title, author) = match opened {
                Ok(e) => (e.title, e.author),
                Err(_) => (file.clone(), String::from("(can't open this file)")),
            };
            books.push(BookInfo { file, size, title, author });
        }
        books.sort_by_cached_key(|b| b.title.to_lowercase());
        if changed || books.len() != cached.len() {
            let mut s = String::new();
            for b in &books {
                s.push_str(&format!("{}\t{}\t{}\t{}\n", b.file, b.size, b.title, b.author));
            }
            let _ = self.storage.save(LIBRARY_FILE, s.as_bytes());
        }
        self.library = books;
    }

    fn open_book(&mut self, i: usize) {
        let info = self.library[i].clone();
        if self.reader.as_ref().is_some_and(|r| r.info.file == info.file) {
            self.screen = Screen::Reading;
            return;
        }
        let epub = match self.storage.open_book(&info.file).and_then(Epub::open) {
            Ok(e) => e,
            Err(_) => {
                self.screen = Screen::Message(format!("Can't open {}", info.file));
                return;
            }
        };
        let saved = self.storage.load(&book_key(&info)).map(|d| numbers(&d)).unwrap_or_default();
        let (spine, pos) = match saved[..] {
            [s, b, o, ..] => (s as usize, Pos { block: b as u32, byte: o as u32 }),
            _ => (0, Pos::default()),
        };
        let mut reader = Reader {
            info,
            epub,
            spine: 0,
            chapter: Chapter::default(),
            starts: Vec::new(),
            page: 0,
            marks: Vec::new(),
        };
        let layout = layout(&self.fonts, &self.settings);
        let spine = if spine < reader.epub.spine.len() { spine } else { 0 };
        if !reader.goto(&layout, spine, Target::At(pos)) && !reader.goto(&layout, 0, Target::First) {
            self.screen = Screen::Message(format!("{} has no text", reader.info.title));
            return;
        }
        let _ = self.storage.save(LAST_FILE, reader.info.file.as_bytes());
        self.reader = Some(reader);
        self.screen = Screen::Reading;
        self.save_progress();
    }

    fn save_progress(&mut self) {
        if let Some(r) = &self.reader {
            let p = r.pos();
            let s = format!("{} {} {}\n", r.spine, p.block, p.byte);
            let _ = self.storage.save(&book_key(&r.info), s.as_bytes());
        }
    }

    fn save_settings(&mut self) {
        let _ = self.storage.save(SETTINGS_FILE, self.settings.to_text().as_bytes());
    }

    /// Repaginates the open book after a settings change, keeping the place.
    fn relayout(&mut self) {
        let layout = layout(&self.fonts, &self.settings);
        if let Some(r) = &mut self.reader {
            r.repaginate(&layout);
        }
        self.save_progress();
    }

    fn set_font(&mut self, step: i32) {
        if self.settings.step(TEXT_SIZE, step) {
            self.save_settings();
            self.relayout();
        }
    }

    /// Handles one input. Returns true if the screen changed.
    pub fn handle(&mut self, input: Input) -> bool {
        let layout = layout(&self.fonts, &self.settings);
        let screen = self.screen.clone();
        match screen {
            Screen::Reading => {
                let Some(r) = &mut self.reader else {
                    self.screen = Screen::Library { sel: 0 };
                    return true;
                };
                let input = match input {
                    Input::Prev if self.settings.swap_page_buttons() => Input::Next,
                    Input::Next if self.settings.swap_page_buttons() => Input::Prev,
                    i => i,
                };
                let moved = match input {
                    Input::Next => r.next_page(&layout),
                    Input::Prev => r.prev_page(&layout),
                    Input::Menu => {
                        self.screen = Screen::Menu { sel: 0 };
                        return true;
                    }
                    Input::Back => {
                        let sel = self.library.iter().position(|b| b.file == r.info.file).unwrap_or(0);
                        self.screen = Screen::Library { sel };
                        return true;
                    }
                };
                if moved {
                    self.save_progress();
                }
                moved
            }
            Screen::Menu { sel } => {
                match input {
                    Input::Prev => self.screen = Screen::Menu { sel: (sel + MENU.len() - 1) % MENU.len() },
                    Input::Next => self.screen = Screen::Menu { sel: (sel + 1) % MENU.len() },
                    Input::Back => self.screen = Screen::Reading,
                    Input::Menu => match sel {
                        1 => {
                            let sel = self.reader.as_ref().and_then(|r| r.toc_index()).unwrap_or(0);
                            self.screen = Screen::Toc { sel };
                        }
                        2 | 3 => {
                            self.set_font(if sel == 2 { 1 } else { -1 });
                            self.screen = Screen::Reading;
                        }
                        4 => self.screen = Screen::Settings { sel: 0, from_reading: true },
                        5 => return self.show_library(),
                        _ => self.screen = Screen::Reading,
                    },
                }
                true
            }
            Screen::Toc { sel } => {
                let Some(r) = &mut self.reader else { return false };
                let n = r.epub.toc.len();
                if n == 0 {
                    self.screen = Screen::Reading;
                    return true;
                }
                match input {
                    Input::Prev => self.screen = Screen::Toc { sel: (sel + n - 1) % n },
                    Input::Next => self.screen = Screen::Toc { sel: (sel + 1) % n },
                    Input::Back => self.screen = Screen::Reading,
                    Input::Menu => {
                        let e = r.epub.toc[sel].clone();
                        if e.spine != r.spine {
                            r.goto(&layout, e.spine, Target::First);
                        }
                        // goto may have skipped ahead past an item without text.
                        if r.spine == e.spine {
                            let block = e.fragment.as_deref().and_then(|f| r.chapter.anchor(f));
                            r.page = page_of(&r.starts, Pos::new(block.unwrap_or(0), 0));
                        }
                        self.save_progress();
                        self.screen = Screen::Reading;
                    }
                }
                true
            }
            Screen::Library { sel } => {
                // The books, then a row for the settings.
                let n = self.library.len() + 1;
                match input {
                    Input::Prev => self.screen = Screen::Library { sel: (sel + n - 1) % n },
                    Input::Next => self.screen = Screen::Library { sel: (sel + 1) % n },
                    Input::Menu if sel < self.library.len() => self.open_book(sel),
                    Input::Menu => self.screen = Screen::Settings { sel: 0, from_reading: false },
                    Input::Back if self.reader.is_some() => self.screen = Screen::Reading,
                    _ => return false,
                }
                true
            }
            Screen::Settings { sel, from_reading } => {
                let n = SETTINGS.len();
                match input {
                    Input::Prev => self.screen = Screen::Settings { sel: (sel + n - 1) % n, from_reading },
                    Input::Next => self.screen = Screen::Settings { sel: (sel + 1) % n, from_reading },
                    Input::Menu => {
                        self.settings.cycle(sel);
                        self.save_settings();
                    }
                    Input::Back => {
                        self.relayout();
                        self.screen = if from_reading && self.reader.is_some() {
                            Screen::Reading
                        } else {
                            Screen::Library { sel: self.library.len() }
                        };
                    }
                }
                true
            }
            Screen::Message(_) => {
                self.screen = Screen::Library { sel: 0 };
                true
            }
        }
    }

    /// Leaves any menu or list for the reading view (the library if no book
    /// is open), as `start` would show it, e.g. before the device sleeps.
    /// Returns true if the screen changed.
    pub fn settle(&mut self) -> bool {
        let target = if self.reader.is_some() { Screen::Reading } else { Screen::Library { sel: 0 } };
        if (self.reader.is_none() && matches!(self.screen, Screen::Library { .. })) || self.screen == target {
            return false;
        }
        if matches!(self.screen, Screen::Settings { .. }) {
            self.relayout();
        }
        self.screen = target;
        true
    }

    fn show_library(&mut self) -> bool {
        self.scan_library();
        let sel = self
            .reader
            .as_ref()
            .and_then(|r| self.library.iter().position(|b| b.file == r.info.file))
            .unwrap_or(0);
        self.screen = Screen::Library { sel };
        true
    }

    pub fn draw(&mut self, canvas: &mut Canvas) {
        canvas.clear();
        match self.screen.clone() {
            Screen::Reading => self.draw_reading(canvas),
            Screen::Menu { sel } => {
                self.draw_reading(canvas);
                self.draw_menu(canvas, sel);
            }
            Screen::Toc { sel } => self.draw_toc(canvas, sel),
            Screen::Library { sel } => self.draw_library(canvas, sel),
            Screen::Settings { sel, .. } => self.draw_settings(canvas, sel),
            Screen::Message(m) => {
                let x = (WIDTH - self.width(0, 24, &m)) / 2;
                self.text(canvas, 0, 24, x, HEIGHT / 2, &m);
            }
        }
    }

    fn text(&mut self, canvas: &mut Canvas, face: u8, px: i32, x: i32, y: i32, s: &str) -> i32 {
        self.renderer.draw_str(&self.fonts, canvas, face, px, x * 64, y, s)
    }

    /// Width of `s` in whole pixels.
    fn width(&self, face: u8, px: i32, s: &str) -> i32 {
        self.fonts.to_fx(self.fonts.measure(face, s), px) / 64
    }

    fn draw_reading(&mut self, canvas: &mut Canvas) {
        let Some(r) = &self.reader else { return };
        let layout = layout(&self.fonts, &self.settings);
        let page = layout.page(&r.chapter, r.pos());
        self.renderer.draw_page(&self.fonts, canvas, &r.chapter, &page);

        let status = self.settings.status_line();
        if status == StatusLine::Off {
            return;
        }
        let side = layout.left;
        let right = format!("{} / {}   {}%", r.page + 1, r.starts.len(), r.percent());
        let right_w = self.width(0, STATUS_PX, &right);
        let title = r.toc_index().map(|i| r.epub.toc[i].title.as_str()).unwrap_or(&r.info.title);
        let title = fit(&self.fonts, ITALIC, STATUS_PX, title, WIDTH - 2 * side - right_w - 24);
        if status == StatusLine::Full {
            self.text(canvas, ITALIC, STATUS_PX, side, STATUS_BASELINE, &title);
        }
        self.text(canvas, 0, STATUS_PX, WIDTH - side - right_w, STATUS_BASELINE, &right);
    }

    fn draw_menu(&mut self, canvas: &mut Canvas, sel: usize) {
        let row = 48;
        let (x, w) = (60, WIDTH - 120);
        let h = 56 + row * MENU.len() as i32 + 16;
        let y = HEIGHT - h - 60;
        canvas.fill_rect(x - 3, y - 3, w + 6, h + 6, BLACK);
        canvas.fill_rect(x, y, w, h, WHITE);
        let mut info = format!("Text size {}", self.settings.font_px());
        if let Some(b) = self.battery {
            info.push_str(&format!("  ·  Battery {}%", b));
        }
        self.text(canvas, ITALIC, 18, x + 24, y + 36, &info);
        for (i, item) in MENU.iter().enumerate() {
            let top = y + 56 + row * i as i32;
            self.text(canvas, 0, 24, x + 24, top + 32, item);
            if i == sel {
                canvas.invert_rect(x + 8, top + 2, w - 16, row - 4);
            }
        }
    }

    /// Draws a header and the page of rows that contains `sel`. Each row is
    /// `(text, detail, depth)`.
    fn draw_header(&mut self, canvas: &mut Canvas, title: &str) {
        self.text(canvas, BOLD, 28, MARGIN_X, 56, title);
        canvas.fill_rect(MARGIN_X, 72, WIDTH - 2 * MARGIN_X, 2, BLACK);
    }

    fn draw_list(&mut self, canvas: &mut Canvas, title: &str, rows: &[(String, String, u8)], sel: usize, detail: bool) {
        self.draw_header(canvas, title);
        let row_h = if detail { 72 } else { 44 };
        let top = 88;
        let per_page = ((STATUS_BASELINE - 30 - top) / row_h).max(1) as usize;
        let first = sel / per_page * per_page;
        for (k, (text, sub, depth)) in rows.iter().enumerate().skip(first).take(per_page) {
            let y = top + (k - first) as i32 * row_h;
            let x = MARGIN_X + 12 + (*depth).min(3) as i32 * 24;
            let max = WIDTH - MARGIN_X - 12 - x;
            let face = if detail { BOLD } else { 0 };
            let t = fit(&self.fonts, face, 22, text, max);
            self.text(canvas, face, 22, x, y + 30, &t);
            if detail {
                let s = fit(&self.fonts, ITALIC, 18, sub, max);
                self.text(canvas, ITALIC, 18, x, y + 58, &s);
            }
            if k == sel {
                canvas.invert_rect(MARGIN_X, y + 2, WIDTH - 2 * MARGIN_X, row_h - 4);
            }
        }
        if rows.len() > per_page {
            let pages = rows.len().div_ceil(per_page);
            let s = format!("{} / {}", first / per_page + 1, pages);
            let w = self.width(0, STATUS_PX, &s);
            self.text(canvas, 0, STATUS_PX, WIDTH - MARGIN_X - w, STATUS_BASELINE, &s);
        }
    }

    fn draw_toc(&mut self, canvas: &mut Canvas, sel: usize) {
        let Some(r) = &self.reader else { return };
        let rows: Vec<_> = r.epub.toc.iter().map(|e| (e.title.clone(), String::new(), e.depth)).collect();
        let title = r.info.title.clone();
        self.draw_list(canvas, &title, &rows, sel, false);
    }

    fn draw_library(&mut self, canvas: &mut Canvas, sel: usize) {
        let mut rows: Vec<_> = self.library.iter().map(|b| (b.title.clone(), b.author.clone(), 0)).collect();
        rows.push((String::from("Settings"), String::from("Text, layout, buttons and sleep"), 0));
        self.draw_list(canvas, "Library", &rows, sel, true);
        if self.library.is_empty() {
            self.text(canvas, 0, 22, MARGIN_X, 220, "No books found.");
            self.text(canvas, ITALIC, 18, MARGIN_X, 256, "Copy .epub files to the SD card.");
        }
    }

    fn draw_settings(&mut self, canvas: &mut Canvas, sel: usize) {
        self.draw_header(canvas, "Settings");
        let (top, row_h) = (88, 48);
        let (left, right) = (MARGIN_X + 12, WIDTH - MARGIN_X - 12);
        for (i, s) in SETTINGS.iter().enumerate() {
            let y = top + i as i32 * row_h;
            self.text(canvas, 0, 22, left, y + 32, s.label);
            let value = self.settings.label(i);
            let w = self.width(BOLD, 22, value);
            self.text(canvas, BOLD, 22, right - w, y + 32, value);
            if i == sel {
                canvas.invert_rect(MARGIN_X, y + 2, WIDTH - 2 * MARGIN_X, row_h - 4);
            }
        }
        let y = top + SETTINGS.len() as i32 * row_h + 40;
        self.text(canvas, ITALIC, 18, left, y, SETTINGS[sel].help);
        let hint = "Menu: change    Hold Menu: back";
        let w = self.width(0, STATUS_PX, hint);
        self.text(canvas, 0, STATUS_PX, (WIDTH - w) / 2, STATUS_BASELINE, hint);
    }
}
