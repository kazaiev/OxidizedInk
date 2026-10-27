//! Reader settings: a table of named options. They are stored in
//! `SETTINGS.TXT` as `key = value` lines, with the values shown on the
//! Settings screen, so the file can also be edited on the card by hand.

use alloc::string::String;

pub struct Setting {
    /// Name in the settings file.
    pub key: &'static str,
    pub label: &'static str,
    /// Shown under the list while the setting is selected.
    pub help: &'static str,
    pub options: &'static [&'static str],
    pub default: u8,
}

pub const TEXT_SIZE: usize = 0;
pub const LINE_SPACING: usize = 1;
pub const MARGINS: usize = 2;
pub const ALIGNMENT: usize = 3;
pub const PARAGRAPHS: usize = 4;
pub const STATUS_LINE: usize = 5;
pub const PAGE_BUTTONS: usize = 6;
pub const SLEEP: usize = 7;

/// Values behind the options, in the same order.
const FONT_SIZES: [i32; 8] = [18, 20, 22, 24, 26, 28, 32, 36];
const LINE_PCTS: [i32; 3] = [120, 140, 165];
/// `(side, top)` margins in pixels.
const MARGIN_PX: [(i32, i32); 3] = [(20, 20), (36, 28), (56, 40)];
const SLEEP_MINUTES: [u32; 6] = [1, 2, 5, 10, 15, 30];

pub const SETTINGS: [Setting; 8] = [
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
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLine {
    /// Chapter title, page in chapter and percent of the book.
    Full,
    /// Page and percent only.
    Progress,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// Option index of each setting.
    values: [u8; SETTINGS.len()],
}

impl Default for Settings {
    fn default() -> Self {
        Settings { values: SETTINGS.map(|s| s.default) }
    }
}

impl Settings {
    /// Reads a settings file. Unknown keys and values are ignored, so missing
    /// or broken lines fall back to the defaults.
    pub fn parse(data: &[u8]) -> Self {
        let mut s = Settings::default();
        for line in core::str::from_utf8(data).unwrap_or("").lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            let Some(i) = SETTINGS.iter().position(|d| d.key == key) else { continue };
            if let Some(o) = SETTINGS[i].options.iter().position(|o| o.eq_ignore_ascii_case(value)) {
                s.values[i] = o as u8;
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for (d, &v) in SETTINGS.iter().zip(&self.values) {
            out.push_str(d.key);
            out.push_str(" = ");
            out.push_str(d.options[v as usize]);
            out.push('\n');
        }
        out
    }

    /// The selected option's label.
    pub fn label(&self, i: usize) -> &'static str {
        SETTINGS[i].options[self.values[i] as usize]
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

        let s = Settings::parse(b"# comment\ntext_size = 32\r\nalignment=left\nmargins = Huge\nbogus = 1\n24\n");
        assert_eq!(s.font_px(), 32);
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
