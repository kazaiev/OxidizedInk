//! Font families from the card for book text: a folder each in `/FONTS/`,
//! holding a family's regular, bold, italic and bold italic files. The
//! Font setting picks one; menus and lists keep the built-in Literata.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::*;
use crate::fonts::Style;
use crate::settings::BUILT_IN_FONT;

/// The most font data a family may load, in bytes: four faces of a text
/// font take well under a megabyte, and memory has to stay free for the
/// book's pictures.
pub(super) const MAX_FAMILY: u64 = 1200 * 1024;

impl<S: Storage> App<S> {
    /// Moves the Font setting on to the next family: the built-in one, then
    /// the folders in `/FONTS/`. Loads it, and saves the settings.
    pub(super) fn next_font(&mut self) {
        let mut names: Vec<String> = Vec::from([BUILT_IN_FONT.to_string()]);
        names.extend(self.storage.list_fonts().into_iter().map(|(f, _)| f).filter(|f| f != BUILT_IN_FONT));
        let at = names.iter().position(|n| *n == self.settings.font).map_or(0, |i| (i + 1) % names.len());
        self.settings.font = names.swap_remove(at);
        self.load_book_fonts();
        self.save_settings();
    }

    /// Loads the family the Font setting names for book text, unless it's
    /// loaded already. If it can't be used, books use the built-in family
    /// and `font_note` says why.
    pub(super) fn load_book_fonts(&mut self) {
        let family = self.settings.font.clone();
        if family == self.book_family {
            return;
        }
        self.font_note = None;
        self.book_fonts = None;
        self.book_family = BUILT_IN_FONT.to_string();
        if family == BUILT_IN_FONT {
            return;
        }
        match self.read_family(&family) {
            Ok(fonts) => {
                self.book_fonts = Some(fonts);
                self.book_family = family;
            }
            Err(why) => self.font_note = Some(format!("{why} Books use {BUILT_IN_FONT} meanwhile.")),
        }
    }

    /// Reads a family's files: for each style, the first file whose name
    /// says it's that style.
    fn read_family(&mut self, family: &str) -> Result<Fonts, String> {
        let fonts = self.storage.list_fonts();
        let Some((_, files)) = fonts.iter().find(|(f, _)| f == family) else {
            return Err(format!("/FONTS has no folder {family}."));
        };
        let mut chosen: Vec<(Style, &String, u64)> = Vec::new();
        for (name, size) in files {
            if let Some(style) = Style::of_file(name).filter(|s| !chosen.iter().any(|c| c.0 == *s)) {
                chosen.push((style, name, *size));
            }
        }
        if !chosen.iter().any(|c| c.0 == Style::Regular) {
            return Err(format!("{family} has no regular style, such as {family}-Regular.ttf."));
        }
        let total: u64 = chosen.iter().map(|c| c.2).sum();
        if total > MAX_FAMILY {
            return Err(format!("{family} is {} KB, more than {} KB.", total / 1024, MAX_FAMILY / 1024));
        }
        let data = chosen.into_iter().filter_map(|(style, name, _)| Some((style, self.storage.load_font(family, name)?)));
        Fonts::from_files(data.collect()).ok_or_else(|| format!("{family}'s regular file isn't a font this reader opens."))
    }
}
