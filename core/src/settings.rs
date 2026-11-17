//! Reader settings: a table of named options. They are stored in
//! `SETTINGS.TXT` as `key = value` lines, with the values shown on the
//! Settings screen, so the file can also be edited on the card by hand.

use alloc::string::{String, ToString};

pub struct Setting {
    /// Name in the settings file.
    pub key: &'static str,
    pub label: &'static str,
    /// Shown under the list while the setting is selected.
    pub help: &'static str,
    pub options: &'static [&'static str],
    pub default: u8,
}

pub const FONT: usize = 0;
pub const TEXT_SIZE: usize = 1;
pub const LINE_SPACING: usize = 2;
pub const MARGINS: usize = 3;
pub const ALIGNMENT: usize = 4;
pub const HYPHENATION: usize = 5;
pub const PARAGRAPHS: usize = 6;
pub const STATUS_LINE: usize = 7;
pub const PAGE_BUTTONS: usize = 8;
pub const SLEEP: usize = 9;
pub const SLEEP_SCREEN: usize = 10;
pub const FULL_REFRESH: usize = 11;
pub const LIBRARY_ORDER: usize = 12;
pub const BOOK_STYLES: usize = 13;

/// The built-in font family, the font setting's default.
pub const BUILT_IN_FONT: &str = "Literata";

/// Values behind the options, in the same order.
const FONT_SIZES: [i32; 8] = [18, 20, 22, 24, 26, 28, 32, 36];
const LINE_PCTS: [i32; 3] = [120, 140, 165];
/// `(side, top)` margins in pixels.
const MARGIN_PX: [(i32, i32); 3] = [(20, 20), (36, 28), (56, 40)];
const SLEEP_MINUTES: [u32; 6] = [1, 2, 5, 10, 15, 30];
const FULL_REFRESH_EVERY: [u32; 4] = [1, 5, 10, 20];

pub const SETTINGS: [Setting; 14] = [
    // Its options are the built-in family and the folders in /FONTS on the
    // card, kept in `Settings::font` rather than in the table.
    Setting {
        key: "font",
        label: "Font",
        help: "For books. More come from folders in /FONTS on the card.",
        options: &[BUILT_IN_FONT],
        default: 0,
    },
    Setting {
        key: "text_size",
        label: "Text size",
        help: "Body text size in pixels. Headings scale with it.",
        options: &["18", "20", "22", "24", "26", "28", "32", "36"],
        default: 3,
    },
    Setting {
        key: "line_spacing",
        label: "Line spacing",
        help: "Space between lines of text.",
        options: &["Tight", "Normal", "Wide"],
        default: 1,
    },
    Setting {
        key: "margins",
        label: "Margins",
        help: "Blank space around the text.",
        options: &["Narrow", "Normal", "Wide"],
        default: 1,
    },
    Setting {
        key: "alignment",
        label: "Alignment",
        help: "Justified lines all end at the right margin.",
        options: &["Justified", "Left"],
        default: 0,
    },
    Setting {
        key: "hyphenation",
        label: "Hyphenation",
        help: "Break long words at line ends: English and Ukrainian.",
        options: &["On", "Off"],
        default: 0,
    },
    Setting {
        key: "paragraphs",
        label: "Paragraphs",
        help: "Indent first lines, or leave a gap between paragraphs.",
        options: &["Indented", "Spaced"],
        default: 0,
    },
    Setting {
        key: "status_line",
        label: "Status line",
        help: "Chapter, page and progress under the text.",
        options: &["Full", "Progress", "Off"],
        default: 0,
    },
    Setting {
        key: "page_buttons",
        label: "Page buttons",
        help: "Swap Prev and Next while reading.",
        options: &["Normal", "Swapped"],
        default: 0,
    },
    Setting {
        key: "sleep",
        label: "Sleep after",
        help: "Idle time before sleep. Any button wakes the reader.",
        options: &["1 min", "2 min", "5 min", "10 min", "15 min", "30 min"],
        default: 2,
    },
    Setting {
        key: "sleep_screen",
        label: "Sleep screen",
        help: "Shown while asleep. Pictures come from /SLEEP.",
        options: &["Book cover", "Pictures", "Logo", "Page"],
        default: 0,
    },
    Setting {
        key: "full_refresh",
        label: "Full refresh",
        help: "How often the whole screen flashes to clear ghosting.",
        options: &["Every page", "Every 5", "Every 10", "Every 20"],
        default: 2,
    },
    Setting {
        key: "library_order",
        label: "Library order",
        help: "Recent: the books opened or added last come first.",
        options: &["Recent", "Title", "Author", "Added"],
        default: 0,
    },
    Setting {
        key: "book_styles",
        label: "Book styles",
        help: "The book's own centering, bold, italics, hidden text and page breaks.",
        options: &["On", "Off"],
        default: 0,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLine {
    /// Chapter title, page in chapter and percent of the book.
    Full,
    /// Page and percent only.
    Progress,
    Off,
}

/// What the screen shows while the device sleeps (unless the battery is
/// low).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepShows {
    /// The open book's cover.
    Cover,
    /// The pictures in `/SLEEP/` on the card, one after another.
    Pictures,
    /// The oxidized-ink logo.
    Logo,
    /// The page as it was.
    Page,
}

/// How the library lists books.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryOrder {
    /// The books opened or added last first.
    Recent,
    Title,
    /// By the author's surname.
    Author,
    /// The books added last first.
    Added,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Option index of each setting.
    values: [u8; SETTINGS.len()],
    /// The font family for books: a folder in /FONTS, or `BUILT_IN_FONT`.
    pub font: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { values: SETTINGS.map(|s| s.default), font: BUILT_IN_FONT.to_string() }
    }
}

impl Settings {
    /// Reads a settings file. Unknown keys and values are ignored, so missing
    /// or broken lines fall back to the defaults.
    pub fn parse(data: &[u8]) -> Self {
        let mut s = Settings::default();
        s.apply(data);
        s
    }

    /// Changes the settings that `key = value` lines name, as in the
    /// settings file, and leaves the others.
    pub fn apply(&mut self, data: &[u8]) {
        for line in core::str::from_utf8(data).unwrap_or("").lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            if key == SETTINGS[FONT].key {
                if !value.is_empty() {
                    self.font = value.to_string();
                }
                continue;
            }
            let Some(i) = SETTINGS.iter().position(|d| d.key == key) else { continue };
            if let Some(o) = SETTINGS[i].options.iter().position(|o| o.eq_ignore_ascii_case(value)) {
                self.values[i] = o as u8;
            }
        }
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for (d, &v) in SETTINGS.iter().zip(&self.values) {
            out.push_str(d.key);
            out.push_str(" = ");
            out.push_str(if d.key == SETTINGS[FONT].key { &self.font } else { d.options[v as usize] });
            out.push('\n');
        }
        out
    }

    /// The selected option's label.
    pub fn label(&self, i: usize) -> &str {
        if i == FONT {
            &self.font
        } else {
            SETTINGS[i].options[self.values[i] as usize]
        }
    }

    /// Selects the next option, wrapping around.
    pub fn cycle(&mut self, i: usize) {
        self.values[i] = ((self.values[i] as usize + 1) % SETTINGS[i].options.len()) as u8;
    }

    /// Moves `delta` options along without wrapping. Returns false if
    /// already at the end.
    pub fn step(&mut self, i: usize, delta: i32) -> bool {
        let last = SETTINGS[i].options.len() as i32 - 1;
        let v = (self.values[i] as i32 + delta).clamp(0, last) as u8;
        let moved = v != self.values[i];
        self.values[i] = v;
        moved
    }

    pub fn font_px(&self) -> i32 {
        FONT_SIZES[self.values[TEXT_SIZE] as usize]
    }

    /// Line height as a percentage of the font size.
    pub fn line_pct(&self) -> i32 {
        LINE_PCTS[self.values[LINE_SPACING] as usize]
    }

    /// `(side, top)` page margins in pixels.
    pub fn margins(&self) -> (i32, i32) {
        MARGIN_PX[self.values[MARGINS] as usize]
    }

    pub fn justify(&self) -> bool {
        self.values[ALIGNMENT] == 0
    }

    pub fn hyphenate(&self) -> bool {
        self.values[HYPHENATION] == 0
    }

    /// Paragraphs separated by space instead of a first-line indent.
    pub fn spaced(&self) -> bool {
        self.values[PARAGRAPHS] == 1
    }

    pub fn status_line(&self) -> StatusLine {
        match self.values[STATUS_LINE] {
            0 => StatusLine::Full,
            1 => StatusLine::Progress,
            _ => StatusLine::Off,
        }
    }

    pub fn swap_page_buttons(&self) -> bool {
        self.values[PAGE_BUTTONS] == 1
    }

    /// Idle minutes before the device sleeps.
    pub fn sleep_minutes(&self) -> u32 {
        SLEEP_MINUTES[self.values[SLEEP] as usize]
    }

    pub fn sleep_shows(&self) -> SleepShows {
        match self.values[SLEEP_SCREEN] {
            0 => SleepShows::Cover,
            1 => SleepShows::Pictures,
            2 => SleepShows::Logo,
            _ => SleepShows::Page,
        }
    }

    /// One screen update in this many is a full refresh; the others change
    /// only the pixels that differ.
    pub fn full_refresh_every(&self) -> u32 {
        FULL_REFRESH_EVERY[self.values[FULL_REFRESH] as usize]
    }

    /// Set books as their CSS says, where it can be read.
    pub fn book_styles(&self) -> bool {
        self.values[BOOK_STYLES] == 0
    }

    pub fn library_order(&self) -> LibraryOrder {
        match self.values[LIBRARY_ORDER] {
            0 => LibraryOrder::Recent,
            1 => LibraryOrder::Title,
            2 => LibraryOrder::Author,
            _ => LibraryOrder::Added,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    #[test]
    fn values_match_options() {
        let numbers = |i: usize| SETTINGS[i].options.len();
        assert_eq!(numbers(TEXT_SIZE), FONT_SIZES.len());
        assert_eq!(numbers(LINE_SPACING), LINE_PCTS.len());
        assert_eq!(numbers(MARGINS), MARGIN_PX.len());
        assert_eq!(numbers(SLEEP), SLEEP_MINUTES.len());
        assert_eq!(numbers(FULL_REFRESH), FULL_REFRESH_EVERY.len());
        for (o, px) in SETTINGS[TEXT_SIZE].options.iter().zip(FONT_SIZES) {
            assert_eq!(*o, format!("{px}"));
        }
        for (o, m) in SETTINGS[SLEEP].options.iter().zip(SLEEP_MINUTES) {
            assert_eq!(*o, format!("{m} min"));
        }
        for s in &SETTINGS {
            assert!((s.default as usize) < s.options.len(), "{}", s.key);
        }
    }

    #[test]
    fn parse_and_write() {
        let d = Settings::default();
        assert_eq!(d.font_px(), 24);
        assert_eq!(Settings::parse(d.to_text().as_bytes()), d);

        let s = Settings::parse(b"# comment\ntext_size = 32\r\nalignment=left\nmargins = Huge\nbogus = 1\n24\nfont = Source Serif 4\n");
        assert_eq!(s.font_px(), 32);
        assert_eq!(s.font, "Source Serif 4");
        assert_eq!(d.font, BUILT_IN_FONT);

        // Applying lines changes only what they name.
        let mut t = s.clone();
        t.apply(b"margins = Wide\nfont = Literata\n");
        assert_eq!(t.margins(), (56, 40));
        assert_eq!(t.font, BUILT_IN_FONT);
        assert_eq!(t.font_px(), 32);
        assert!(!s.justify());
        assert_eq!(s.margins(), d.margins());
        assert_eq!(Settings::parse(s.to_text().as_bytes()), s);
    }

    #[test]
    fn cycle_wraps_and_step_clamps() {
        let mut s = Settings::default();
        s.cycle(ALIGNMENT);
        assert!(!s.justify());
        s.cycle(ALIGNMENT);
        assert!(s.justify());
        assert!(s.step(TEXT_SIZE, 10));
        assert_eq!(s.font_px(), 36);
        assert!(!s.step(TEXT_SIZE, 1));
    }
}
