//! The reader's screens and navigation, driven by four abstract inputs.
//!
//! Screens: the library (books on the card), the reading view, a menu over
//! the page, the table of contents, the settings and system info, and the
//! screens left up while the device sleeps. All state that must survive
//! sleep or power-off goes through `Storage` as small text files.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::epub::Epub;
use crate::fonts::{Fonts, Meter};
use crate::image;
use crate::layout::{page_of, Item, Layout, Pos};
use crate::render::{Canvas, Renderer, BLACK, WHITE};
use crate::settings::{Settings, SleepShows, StatusLine, SETTINGS, TEXT_SIZE};
use crate::text::{Chapter, BOLD, ITALIC};
use crate::{Error, ReadAt};

/// Portrait canvas size in pixels.
pub const WIDTH: i32 = 600;
pub const HEIGHT: i32 = 800;

/// Side margin of the menus and lists. The reading view's margins are a
/// setting.
const MARGIN_X: i32 = 36;
/// Top of the rows below a list's header.
const LIST_TOP: i32 = 88;
/// Bottom of the body text when the status line is shown below it.
const TEXT_BOTTOM: i32 = 752;
const STATUS_PX: i32 = 16;
const STATUS_BASELINE: i32 = 784;

const LAST_FILE: &str = "LAST.TXT";
const SETTINGS_FILE: &str = "SETTINGS.TXT";
const LIBRARY_FILE: &str = "LIBRARY.TXT";
/// The sleep picture shown last.
const PICTURE_FILE: &str = "PICTURE.TXT";

/// At or below this charge, in percent, the device sleeps behind a "Battery
/// low" screen.
pub const LOW_BATTERY: u8 = 15;
/// At or below this charge (about 3.4 V) the reader stops: it shows
/// "Battery empty" and sleeps until a press finds it charged.
pub const EMPTY_BATTERY: u8 = 2;
/// The status line's battery icon.
const BATTERY_ICON: (i32, i32) = (24, 12);

/// The library's rows: a book's cover fits in `COVER`, beside its title,
/// author and progress.
const COVER: (i32, i32) = (80, 120);
const BOOK_ROW: i32 = 132;
/// Books whose covers the library keeps in memory, about two screens' worth.
const SHELF: usize = 12;
/// Starts a cover thumbnail file (`<book>.THM`), followed by the `COVER` box
/// it was made for and the picture's size, as little-endian u16s, then its
/// gray pixels. A book without a cover gets a file of size 0×0.
const THUMB_MAGIC: &[u8; 4] = b"THM2";

/// The oxidized-ink logo, 600×800: an ink drop of the firmware's code.
/// Shown while the reader starts from power-on, and while it sleeps when
/// there's no cover or picture to show.
const LOGO: &[u8] = include_bytes!("../../assets/logo.png");

/// Full-screen pictures left on the panel while the device sleeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepScreen {
    BatteryLow,
    BatteryEmpty,
    /// The open book's cover, or the logo if there is none.
    Cover,
    /// The next picture from the card, or the cover if there are none.
    Picture,
    Logo,
}

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
    /// The sleep screen's pictures (JPEG and PNG files in `/SLEEP/`), sorted.
    fn list_pictures(&mut self) -> Vec<String>;
    fn load_picture(&mut self, name: &str) -> Option<Vec<u8>>;
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
    System { from_reading: bool },
    Message(String),
}

const MENU: [&str; 7] = ["Back to book", "Contents", "Larger text", "Smaller text", "Settings", "System info", "Library"];

/// Where to land when entering a chapter.
#[derive(Clone, Copy)]
enum Target {
    First,
    Last,
    At(Pos),
}

/// Decoded pictures kept for drawing their pages again: at most this many
/// pixels in all, about one page's worth.
const PICTURE_CACHE: usize = 512 * 1024;

/// A book's picture, decoded at the size a page shows it.
struct Picture {
    path: String,
    size: (i32, i32),
    /// None if it doesn't decode.
    gray: Option<image::Gray>,
}

/// What the library shows of a book besides its title and author.
struct Shelved {
    /// The book's progress file name, from `book_file`.
    key: String,
    /// None if the book has no cover that decodes.
    cover: Option<image::Gray>,
    /// How far it has been read, if it has been opened since progress
    /// files began to record it.
    percent: Option<u32>,
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
    /// Pictures drawn lately, oldest first.
    pictures: Vec<Picture>,
}

impl<F: ReadAt> Reader<F> {
    fn pos(&self) -> Pos {
        self.starts[self.page]
    }

    /// Loads spine item `spine`, skipping items with nothing to show in the
    /// direction of travel. Returns false if there is nothing
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

    /// Picture `index` of the chapter, decoded at `w`×`h`.
    fn picture(&mut self, index: u16, w: i32, h: i32) -> Option<&image::Gray> {
        let path = &self.chapter.images[index as usize].path;
        let at = match self.pictures.iter().position(|p| p.path == *path && p.size == (w, h)) {
            Some(i) => i,
            None => {
                let gray = self.epub.read(path).ok().and_then(|data| image::decode(&data, w, h).ok());
                let pixels = |p: &Picture| (p.size.0 * p.size.1) as usize;
                let mut used: usize = self.pictures.iter().map(pixels).sum();
                while used + (w * h) as usize > PICTURE_CACHE && !self.pictures.is_empty() {
                    used -= pixels(&self.pictures.remove(0));
                }
                self.pictures.push(Picture { path: path.clone(), size: (w, h), gray });
                self.pictures.len() - 1
            }
        };
        self.pictures[at].gray.as_ref()
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

/// A book's state file with extension `ext`: `POS` for the place reached
/// (spine item, block, byte and percent), `THM` for the cover thumbnail.
/// Named by FNV-1a over the file name and size, so a changed book starts
/// afresh.
fn book_file(info: &BookInfo, ext: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for b in info.file.bytes().chain(info.size.to_le_bytes()) {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    format!("{:08X}.{}", h, ext)
}

fn thumb_bytes(cover: Option<&image::Gray>) -> Vec<u8> {
    let (w, h) = cover.map_or((0, 0), |g| (g.width, g.height));
    let mut out = THUMB_MAGIC.to_vec();
    for n in [COVER.0, COVER.1, w, h] {
        out.extend_from_slice(&(n as u16).to_le_bytes());
    }
    if let Some(g) = cover {
        out.extend_from_slice(&g.pixels);
    }
    out
}

/// Reads a thumbnail file: `Some(None)` for a book without a cover. None if
/// it isn't a thumbnail made for today's `COVER`.
fn parse_thumb(data: &[u8]) -> Option<Option<image::Gray>> {
    if data.len() < 12 || !data.starts_with(THUMB_MAGIC) {
        return None;
    }
    let n = |i: usize| u16::from_le_bytes([data[4 + 2 * i], data[5 + 2 * i]]) as i32;
    if (n(0), n(1)) != COVER {
        return None;
    }
    let (width, height) = (n(2), n(3));
    let pixels = &data[12..];
    if width == 0 || height == 0 {
        return Some(None);
    }
    (pixels.len() == (width * height) as usize).then(|| Some(image::Gray { width, height, pixels: pixels.to_vec() }))
}

fn numbers(data: &[u8]) -> Vec<u64> {
    core::str::from_utf8(data)
        .unwrap_or("")
        .split_whitespace()
        .filter_map(|w| w.parse().ok())
        .collect()
}

/// A battery `w`×`h` with a nub on the right and walls `t` thick, filled to
/// `percent`.
fn draw_battery(canvas: &mut Canvas, x: i32, y: i32, (w, h): (i32, i32), t: i32, percent: u8) {
    let nub = 2 * t;
    let body = w - nub;
    canvas.fill_rect(x, y, body, h, BLACK);
    canvas.fill_rect(x + t, y + t, body - 2 * t, h - 2 * t, WHITE);
    canvas.fill_rect(x + body, y + h / 4, nub, h - h / 2, BLACK);
    // The charge, a wall's width inside the walls.
    let fill = (body - 4 * t) * percent.min(100) as i32 / 100;
    if fill > 0 {
        canvas.fill_rect(x + 2 * t, y + 2 * t, fill.max(t), h - 4 * t, BLACK);
    }
}

/// Draws a JPEG or PNG file as big as fits on the screen, centred on white.
/// Returns false if it doesn't decode.
fn draw_picture(canvas: &mut Canvas, data: &[u8]) -> bool {
    let Ok(picture) = image::decode(data, WIDTH, HEIGHT) else { return false };
    canvas.clear();
    picture.draw(canvas, (WIDTH - picture.width) / 2, (HEIGHT - picture.height) / 2);
    true
}

/// Draws the logo, full screen.
pub fn draw_logo(canvas: &mut Canvas) {
    draw_picture(canvas, LOGO);
}

/// How many rows `row_h` high fit between a list's header and its page
/// number.
fn rows_per_page(row_h: i32) -> usize {
    ((STATUS_BASELINE - 30 - LIST_TOP) / row_h).max(1) as usize
}

/// Breaks `s` at spaces into at most `lines` lines of `max` pixels, the
/// last shortened with an ellipsis if the rest doesn't fit.
fn wrap(fonts: &Fonts, face: u8, px: i32, s: &str, max: i32, lines: usize) -> Vec<String> {
    let max_units = fonts.px_to_units(max, px);
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        // The longest run of words that fits.
        let end = rest
            .match_indices(' ')
            .map(|(i, _)| i)
            .take_while(|&i| fonts.measure(face, &rest[..i]) <= max_units)
            .last();
        let last = out.len() + 1 == lines || fonts.measure(face, rest) <= max_units;
        match end {
            Some(end) if !last => {
                out.push(rest[..end].to_string());
                rest = rest[end..].trim_start();
            }
            _ => {
                out.push(fit(fonts, face, px, rest, max));
                break;
            }
        }
    }
    out
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
    /// Covers and progress of the books shown lately, oldest first.
    shelf: Vec<Shelved>,
    reader: Option<Reader<S::Book>>,
    /// Battery charge in percent, when known: the latest reading. The menu
    /// shows it, and the low and empty battery checks use it.
    pub battery: Option<u8>,
    /// The charge the status line shows. The device copies `battery` here
    /// only before a full refresh, so partial refreshes never change it.
    pub status_battery: Option<u8>,
    /// `(label, value)` rows for the System info screen, from the platform.
    pub system: Vec<(String, String)>,
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
            shelf: Vec::new(),
            reader: None,
            battery: None,
            status_battery: None,
            system: Vec::new(),
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
        let saved = self.storage.load(&book_file(&info, "POS")).map(|d| numbers(&d)).unwrap_or_default();
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
            pictures: Vec::new(),
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
            let percent = r.percent();
            let s = format!("{} {} {} {}\n", r.spine, p.block, p.byte, percent);
            let key = book_file(&r.info, "POS");
            let _ = self.storage.save(&key, s.as_bytes());
            if let Some(b) = self.shelf.iter_mut().find(|b| b.key == key) {
                b.percent = Some(percent);
            }
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
                        5 => self.screen = Screen::System { from_reading: true },
                        6 => return self.show_library(),
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
                // The books, then rows for the settings and system info.
                let n = self.library.len() + 2;
                match input {
                    Input::Prev => self.screen = Screen::Library { sel: (sel + n - 1) % n },
                    Input::Next => self.screen = Screen::Library { sel: (sel + 1) % n },
                    Input::Menu if sel < self.library.len() => self.open_book(sel),
                    Input::Menu if sel == self.library.len() => {
                        self.screen = Screen::Settings { sel: 0, from_reading: false }
                    }
                    Input::Menu => self.screen = Screen::System { from_reading: false },
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
            Screen::System { from_reading } => {
                // Other inputs just redraw: the platform updates the rows
                // before each one.
                if input == Input::Back {
                    self.screen = if from_reading && self.reader.is_some() {
                        Screen::Reading
                    } else {
                        Screen::Library { sel: self.library.len() + 1 }
                    };
                }
                true
            }
            Screen::Message(_) => {
                self.screen = Screen::Library { sel: 0 };
                true
            }
        }
    }

    /// True when the battery is too low to keep reading.
    pub fn battery_empty(&self) -> bool {
        self.battery.is_some_and(|b| b <= EMPTY_BATTERY)
    }

    /// What to leave on the panel for a deep sleep: a sleep screen, or `None`
    /// for the page (after `settle`). The battery screens come first.
    pub fn sleep_screen(&self) -> Option<SleepScreen> {
        match self.battery {
            Some(b) if b <= EMPTY_BATTERY => return Some(SleepScreen::BatteryEmpty),
            Some(b) if b <= LOW_BATTERY => return Some(SleepScreen::BatteryLow),
            _ => {}
        }
        match self.settings.sleep_shows() {
            SleepShows::Cover => Some(SleepScreen::Cover),
            SleepShows::Pictures => Some(SleepScreen::Picture),
            SleepShows::Logo => Some(SleepScreen::Logo),
            SleepShows::Page => None,
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
            Screen::System { .. } => self.draw_system(canvas),
            Screen::Message(m) => self.centered(canvas, 0, 24, HEIGHT / 2, &m),
        }
    }

    /// Draws a sleep screen. Without a cover or picture that decodes (or a
    /// book open, for the cover), it draws the logo.
    pub fn draw_sleep(&mut self, canvas: &mut Canvas, screen: SleepScreen) {
        let (title, line, hint, level) = match screen {
            SleepScreen::Cover => {
                let cover = self.reader.as_mut().and_then(|r| r.epub.read_cover());
                if !cover.is_some_and(|data| draw_picture(canvas, &data)) {
                    draw_logo(canvas);
                }
                return;
            }
            SleepScreen::Picture => {
                if !self.draw_next_picture(canvas) {
                    self.draw_sleep(canvas, SleepScreen::Cover);
                }
                return;
            }
            SleepScreen::Logo => return draw_logo(canvas),
            SleepScreen::BatteryLow => (
                "Battery low",
                "Charge the reader soon.",
                "Press a button to keep reading.",
                self.battery.unwrap_or(LOW_BATTERY),
            ),
            SleepScreen::BatteryEmpty => (
                "Battery empty",
                "Charge the reader to keep reading.",
                "Once it has charged, press a button.",
                0,
            ),
        };
        canvas.clear();
        let size = (200, 96);
        draw_battery(canvas, (WIDTH - size.0) / 2, 260, size, 8, level);
        self.centered(canvas, BOLD, 32, 440, title);
        self.centered(canvas, 0, 24, 488, line);
        self.centered(canvas, ITALIC, 18, STATUS_BASELINE, hint);
    }

    /// Draws the picture after the one shown last, skipping any that don't
    /// decode, and remembers it. Returns false if none does.
    fn draw_next_picture(&mut self, canvas: &mut Canvas) -> bool {
        let names = self.storage.list_pictures();
        let last = self.storage.load(PICTURE_FILE).and_then(|d| String::from_utf8(d).ok());
        let next = last.and_then(|l| names.iter().position(|n| *n == l.trim())).map_or(0, |i| i + 1);
        for i in 0..names.len() {
            let name = &names[(next + i) % names.len()];
            if self.storage.load_picture(name).is_some_and(|data| draw_picture(canvas, &data)) {
                let _ = self.storage.save(PICTURE_FILE, name.as_bytes());
                return true;
            }
        }
        false
    }

    fn centered(&mut self, canvas: &mut Canvas, face: u8, px: i32, y: i32, s: &str) {
        let x = (WIDTH - self.width(face, px, s)) / 2;
        self.text(canvas, face, px, x, y, s);
    }

    fn text(&mut self, canvas: &mut Canvas, face: u8, px: i32, x: i32, y: i32, s: &str) -> i32 {
        self.renderer.draw_str(&self.fonts, canvas, face, px, x * 64, y, s)
    }

    /// Width of `s` in whole pixels.
    fn width(&self, face: u8, px: i32, s: &str) -> i32 {
        self.fonts.to_fx(self.fonts.measure(face, s), px) / 64
    }

    fn draw_reading(&mut self, canvas: &mut Canvas) {
        let Some(r) = &mut self.reader else { return };
        let layout = layout(&self.fonts, &self.settings);
        let page = layout.page(&r.chapter, r.pos());
        self.renderer.draw_page(&self.fonts, canvas, &r.chapter, &page);
        for item in &page.items {
            if let &Item::Image { index, x, y, w, h } = item {
                if let Some(picture) = r.picture(index, w, h) {
                    picture.draw(canvas, x, y);
                }
            }
        }
        let side = layout.left;
        self.draw_status(canvas, side);
    }

    /// The status line below the page: chapter, battery, page and percent.
    fn draw_status(&mut self, canvas: &mut Canvas, side: i32) {
        let Some(r) = &self.reader else { return };
        let status = self.settings.status_line();
        if status == StatusLine::Off {
            return;
        }
        let right = format!("{} / {}   {}%", r.page + 1, r.starts.len(), r.percent());
        let right_w = self.width(0, STATUS_PX, &right);
        // The battery shows left of the page numbers, as an icon and percent.
        let battery = self.status_battery;
        let battery_text = battery.map(|b| format!("{}%", b));
        let battery_w = battery_text.as_ref().map_or(0, |t| BATTERY_ICON.0 + 6 + self.width(0, STATUS_PX, t) + 20);
        let title = r.toc_index().map(|i| r.epub.toc[i].title.as_str()).unwrap_or(&r.info.title);
        let title = fit(&self.fonts, ITALIC, STATUS_PX, title, WIDTH - 2 * side - right_w - battery_w - 24);
        if status == StatusLine::Full {
            self.text(canvas, ITALIC, STATUS_PX, side, STATUS_BASELINE, &title);
        }
        self.text(canvas, 0, STATUS_PX, WIDTH - side - right_w, STATUS_BASELINE, &right);
        if let (Some(b), Some(t)) = (battery, battery_text) {
            let x = WIDTH - side - right_w - battery_w;
            draw_battery(canvas, x, STATUS_BASELINE - 11, BATTERY_ICON, 1, b);
            self.text(canvas, 0, STATUS_PX, x + BATTERY_ICON.0 + 6, STATUS_BASELINE, &t);
        }
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

    fn draw_header(&mut self, canvas: &mut Canvas, title: &str) {
        self.text(canvas, BOLD, 28, MARGIN_X, 56, title);
        canvas.fill_rect(MARGIN_X, 72, WIDTH - 2 * MARGIN_X, 2, BLACK);
    }

    /// Draws a header and the page of rows that contains `sel`. Each row is
    /// `(text, depth)`.
    fn draw_list(&mut self, canvas: &mut Canvas, title: &str, rows: &[(String, u8)], sel: usize) {
        self.draw_header(canvas, title);
        let row_h = 44;
        let per_page = rows_per_page(row_h);
        let first = sel / per_page * per_page;
        for (k, (text, depth)) in rows.iter().enumerate().skip(first).take(per_page) {
            let y = LIST_TOP + (k - first) as i32 * row_h;
            let x = MARGIN_X + 12 + (*depth).min(3) as i32 * 24;
            let t = fit(&self.fonts, 0, 22, text, WIDTH - MARGIN_X - 12 - x);
            self.text(canvas, 0, 22, x, y + 30, &t);
            if k == sel {
                canvas.invert_rect(MARGIN_X, y + 2, WIDTH - 2 * MARGIN_X, row_h - 4);
            }
        }
        self.draw_list_page(canvas, sel, per_page, rows.len());
    }

    /// "page / pages" at the bottom right, if a list has more than one page.
    fn draw_list_page(&mut self, canvas: &mut Canvas, sel: usize, per_page: usize, rows: usize) {
        if rows > per_page {
            let s = format!("{} / {}", sel / per_page + 1, rows.div_ceil(per_page));
            let w = self.width(0, STATUS_PX, &s);
            self.text(canvas, 0, STATUS_PX, WIDTH - MARGIN_X - w, STATUS_BASELINE, &s);
        }
    }

    fn draw_toc(&mut self, canvas: &mut Canvas, sel: usize) {
        let Some(r) = &self.reader else { return };
        let rows: Vec<_> = r.epub.toc.iter().map(|e| (e.title.clone(), e.depth)).collect();
        let title = r.info.title.clone();
        self.draw_list(canvas, &title, &rows, sel);
    }

    /// The books with their covers, then rows for the settings and system
    /// info.
    fn draw_library(&mut self, canvas: &mut Canvas, sel: usize) {
        self.draw_header(canvas, "Library");
        let books = self.library.len();
        let per_page = rows_per_page(BOOK_ROW);
        let first = sel / per_page * per_page;
        for k in first..(books + 2).min(first + per_page) {
            let y = LIST_TOP + (k - first) as i32 * BOOK_ROW;
            // Where the row's text starts; selection inverts it from there.
            let x = if k < books {
                self.draw_book(canvas, k, y)
            } else {
                let (label, help) = if k == books {
                    ("Settings", "Text, layout, buttons and sleep")
                } else {
                    ("System info", "Battery, card and memory")
                };
                let x = MARGIN_X + 12;
                self.text(canvas, BOLD, 22, x, y + 58, label);
                self.text(canvas, ITALIC, 18, x, y + 88, help);
                x
            };
            if k == sel {
                canvas.invert_rect(x - 12, y + 2, WIDTH - MARGIN_X - x + 12, BOOK_ROW - 4);
            }
        }
        self.draw_list_page(canvas, sel, per_page, books + 2);
        if books == 0 {
            let y = LIST_TOP + 2 * BOOK_ROW + 48;
            self.text(canvas, 0, 22, MARGIN_X + 12, y, "No books found.");
            self.text(canvas, ITALIC, 18, MARGIN_X + 12, y + 36, "Copy .epub files to the SD card.");
        }
    }

    /// Book `i`'s library row at `y`: its cover, then title, author and
    /// progress. Returns where the text starts.
    fn draw_book(&mut self, canvas: &mut Canvas, i: usize, y: i32) -> i32 {
        let at = self.shelved(i);
        let (cx, cy) = (MARGIN_X + 6, y + (BOOK_ROW - COVER.1) / 2);
        let percent = self.shelf[at].percent;
        match &self.shelf[at].cover {
            Some(g) => {
                let (x, y) = (cx + (COVER.0 - g.width) / 2, cy + (COVER.1 - g.height) / 2);
                // A hairline frame keeps white covers apart from the page.
                canvas.fill_rect(x - 1, y - 1, g.width + 2, g.height + 2, BLACK);
                g.draw(canvas, x, y);
            }
            None => {
                // A gray card with the title's first letter.
                canvas.fill_rect(cx, cy, COVER.0, COVER.1, BLACK);
                canvas.fill_rect(cx + 2, cy + 2, COVER.0 - 4, COVER.1 - 4, 218);
                let initial: String = self.library[i].title.chars().take(1).collect();
                let w = self.width(BOLD, 48, &initial);
                self.text(canvas, BOLD, 48, cx + (COVER.0 - w) / 2, cy + COVER.1 / 2 + 17, &initial);
            }
        }
        let x = cx + COVER.0 + 20;
        let max = WIDTH - MARGIN_X - 12 - x;
        let title = wrap(&self.fonts, BOLD, 22, &self.library[i].title, max, 2);
        let author = fit(&self.fonts, ITALIC, 18, &self.library[i].author, max);
        let mut base = y + 34;
        for line in &title {
            self.text(canvas, BOLD, 22, x, base, line);
            base += 28;
        }
        self.text(canvas, ITALIC, 18, x, base + 2, &author);
        if let Some(p) = percent {
            self.text(canvas, 0, STATUS_PX, x, y + BOOK_ROW - 20, &format!("{}% read", p));
        }
        x
    }

    /// Book `i`'s cover and progress, from the shelf or else from the card.
    /// Returns its index in the shelf.
    fn shelved(&mut self, i: usize) -> usize {
        let key = book_file(&self.library[i], "POS");
        if let Some(at) = self.shelf.iter().position(|b| b.key == key) {
            // Most recently used last.
            let b = self.shelf.remove(at);
            self.shelf.push(b);
            return self.shelf.len() - 1;
        }
        let info = self.library[i].clone();
        let percent = self.storage.load(&key).and_then(|d| numbers(&d).get(3).map(|&p| p as u32));
        let cover = self.load_cover(&info);
        if self.shelf.len() >= SHELF {
            self.shelf.remove(0);
        }
        self.shelf.push(Shelved { key, cover, percent });
        self.shelf.len() - 1
    }

    /// A book's cover shrunk to fit `COVER`: from its thumbnail file, or else
    /// decoded from the book and saved as one, which is slower.
    fn load_cover(&mut self, info: &BookInfo) -> Option<image::Gray> {
        let name = book_file(info, "THM");
        if let Some(thumb) = self.storage.load(&name).and_then(|d| parse_thumb(&d)) {
            return thumb;
        }
        let data = match &mut self.reader {
            // The card can't open a file twice.
            Some(r) if r.info.file == info.file => r.epub.read_cover(),
            // If the book doesn't open, try again another time.
            _ => self.storage.open_book(&info.file).and_then(Epub::open).ok()?.read_cover(),
        };
        let mut cover = data.and_then(|d| image::decode(&d, COVER.0, COVER.1).ok());
        if let Some(g) = &mut cover {
            g.brighten();
        }
        let _ = self.storage.save(&name, &thumb_bytes(cover.as_ref()));
        cover
    }

    fn draw_settings(&mut self, canvas: &mut Canvas, sel: usize) {
        self.draw_header(canvas, "Settings");
        let (top, row_h) = (LIST_TOP, 48);
        let left = MARGIN_X + 12;
        for (i, s) in SETTINGS.iter().enumerate() {
            let y = top + i as i32 * row_h;
            self.draw_pair(canvas, y, s.label, self.settings.label(i));
            if i == sel {
                canvas.invert_rect(MARGIN_X, y + 2, WIDTH - 2 * MARGIN_X, row_h - 4);
            }
        }
        let y = top + SETTINGS.len() as i32 * row_h + 40;
        self.text(canvas, ITALIC, 18, left, y, SETTINGS[sel].help);
        self.draw_hint(canvas, "Menu: change    Hold Menu: back");
    }

    fn draw_system(&mut self, canvas: &mut Canvas) {
        self.draw_header(canvas, "System info");
        let mut rows = self.system.clone();
        rows.push((String::from("Books"), self.library.len().to_string()));
        for (i, (label, value)) in rows.iter().enumerate() {
            self.draw_pair(canvas, LIST_TOP + i as i32 * 48, label, value);
        }
        self.draw_hint(canvas, "Any button: update    Hold Menu: back");
    }

    /// A row of a 48-pixel list starting at `y`: label left, value right.
    fn draw_pair(&mut self, canvas: &mut Canvas, y: i32, label: &str, value: &str) {
        let (left, right) = (MARGIN_X + 12, WIDTH - MARGIN_X - 12);
        self.text(canvas, 0, 22, left, y + 32, label);
        let w = self.width(BOLD, 22, value);
        self.text(canvas, BOLD, 22, right - w, y + 32, value);
    }

    /// A centred line of help at the bottom of the screen.
    fn draw_hint(&mut self, canvas: &mut Canvas, hint: &str) {
        self.centered(canvas, 0, STATUS_PX, STATUS_BASELINE, hint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnails_round_trip() {
        let g = image::Gray { width: 3, height: 2, pixels: (0..6).collect() };
        let back = parse_thumb(&thumb_bytes(Some(&g))).unwrap().unwrap();
        assert_eq!((back.width, back.height, back.pixels), (3, 2, g.pixels.clone()));
        assert!(parse_thumb(&thumb_bytes(None)).unwrap().is_none());
        // Made for another cover size, or cut short: to be made again.
        let mut other = thumb_bytes(Some(&g));
        other[4] += 1;
        assert!(parse_thumb(&other).is_none());
        assert!(parse_thumb(&thumb_bytes(Some(&g))[..15]).is_none());
    }

    #[test]
    fn logo_fills_the_screen() {
        let logo = image::decode(LOGO, WIDTH, HEIGHT).unwrap();
        assert_eq!((logo.width, logo.height), (WIDTH, HEIGHT));
    }

    #[test]
    fn titles_wrap_at_spaces() {
        let fonts = Fonts::new();
        let wrap = |s, lines| wrap(&fonts, BOLD, 22, s, 300, lines);
        let fits = |l: &String| fonts.to_fx(fonts.measure(BOLD, l), 22) / 64 <= 300;
        assert_eq!(wrap("Dracula", 2), ["Dracula"]);
        let title = "The Adventures of Tom Sawyer, Complete";
        let two = wrap(title, 2);
        assert_eq!(two.len(), 2);
        assert!(two.iter().all(fits));
        assert_eq!(two.join(" "), title);
        let one = wrap(title, 1);
        assert!(one.len() == 1 && one[0].ends_with('…') && fits(&one[0]));
        let three = wrap("A Very Long Title That Will Not Fit In Two Short Lines At All", 2);
        assert!(three.len() == 2 && three[1].ends_with('…') && three.iter().all(fits));
    }
}
