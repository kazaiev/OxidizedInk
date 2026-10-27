//! The four embedded PT Serif faces, indexed by style bits (`text::BOLD`,
//! `text::ITALIC`), with glyph ids and advance widths cached for common
//! characters. Widths are in font units; `Fonts::units_per_em` converts.

use ab_glyph::{Font, FontRef, GlyphId, PxScale};
use alloc::vec::Vec;

const DATA: [&[u8]; 4] = [
    include_bytes!("../../fonts/PT_Serif-Web-Regular.ttf"),
    include_bytes!("../../fonts/PT_Serif-Web-Bold.ttf"),
    include_bytes!("../../fonts/PT_Serif-Web-Italic.ttf"),
    include_bytes!("../../fonts/PT_Serif-Web-BoldItalic.ttf"),
];

/// Cached characters: U+0000..U+04FF (Latin, Greek, Cyrillic) and the
/// General Punctuation block (quotes, dashes, ellipsis).
const CACHED: usize = 0x500 + 0x70;

fn slot(c: char) -> Option<usize> {
    let c = c as usize;
    match c {
        0..=0x4ff => Some(c),
        0x2000..=0x206f => Some(0x500 + c - 0x2000),
        _ => None,
    }
}

/// Characters that take no space and draw nothing. A soft hyphen only shows
/// when a line breaks at it.
pub fn is_invisible(c: char) -> bool {
    matches!(c, '\u{ad}' | '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}')
}

/// Stand-ins for characters PT Serif lacks.
fn substitute(c: char) -> char {
    match c {
        '\t' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' => ' ',
        '\u{2010}' => '-',
        '\u{2032}' => '\'',
        '\u{2033}' => '"',
        _ => c,
    }
}

pub struct Fonts {
    faces: [FontRef<'static>; 4],
    /// `(glyph id, advance)` for each face and cached character.
    cache: Vec<(u16, u16)>,
    pub units_per_em: i32,
    /// Distance from the baseline to the top and bottom of the line box, in
    /// font units, both positive.
    pub ascent: i32,
    pub descent: i32,
}

impl Fonts {
    pub fn new() -> Self {
        let faces = DATA.map(|d| FontRef::try_from_slice(d).expect("embedded font"));
        let f = &faces[0];
        let mut fonts = Fonts {
            units_per_em: f.units_per_em().unwrap_or(1000.0) as i32,
            ascent: f.ascent_unscaled() as i32,
            descent: -f.descent_unscaled() as i32,
            faces,
            cache: Vec::new(),
        };
        let mut cache = Vec::with_capacity(4 * CACHED);
        for face in 0..4 {
            for c in (0..0x500).chain(0x2000..0x2070) {
                let (id, adv) = fonts.lookup(face, char::from_u32(c).unwrap_or(' '));
                cache.push((id.0, adv as u16));
            }
        }
        fonts.cache = cache;
        fonts
    }

    fn lookup(&self, face: usize, c: char) -> (GlyphId, i32) {
        if is_invisible(c) {
            return (GlyphId(0), 0);
        }
        let f = &self.faces[face];
        let mut id = f.glyph_id(c);
        if id.0 == 0 {
            let s = substitute(c);
            if s != c {
                id = f.glyph_id(s);
            }
        }
        (id, f.h_advance_unscaled(id) as i32)
    }

    /// Glyph id and advance width of `c` in `face` (style bits).
    pub fn glyph(&self, face: u8, c: char) -> (GlyphId, i32) {
        let face = (face & 3) as usize;
        match slot(c) {
            Some(i) => {
                let (id, adv) = self.cache[face * CACHED + i];
                (GlyphId(id), adv as i32)
            }
            None => self.lookup(face, c),
        }
    }

    pub fn kern(&self, face: u8, a: GlyphId, b: GlyphId) -> i32 {
        self.faces[(face & 3) as usize].kern_unscaled(a, b) as i32
    }

    pub fn face(&self, face: u8) -> &FontRef<'static> {
        &self.faces[(face & 3) as usize]
    }

    /// ab_glyph scale for a font size in pixels per em.
    pub fn scale(&self, px: i32) -> PxScale {
        let height = (self.ascent + self.descent) as f32;
        PxScale::from(px as f32 * height / self.units_per_em as f32)
    }

    /// Converts font units at `px` pixels per em to 1/64 pixels.
    pub fn to_fx(&self, units: i32, px: i32) -> i32 {
        (units as i64 * px as i64 * 64 / self.units_per_em as i64) as i32
    }

    /// Converts pixels at `px` pixels per em to font units.
    pub fn px_to_units(&self, pixels: i32, px: i32) -> i32 {
        (pixels as i64 * self.units_per_em as i64 / px.max(1) as i64) as i32
    }

    /// Width of a string in font units, kerned.
    pub fn measure(&self, face: u8, s: &str) -> i32 {
        let mut m = Meter::new(self);
        s.chars().map(|c| m.next(face, c).2).sum()
    }
}

impl Default for Fonts {
    fn default() -> Self {
        Self::new()
    }
}

/// Walks a run of text, applying kerning between neighbors in the same face.
/// Layout and rendering both measure with it, so they always agree.
pub struct Meter<'a> {
    fonts: &'a Fonts,
    prev: Option<(u8, GlyphId)>,
}

impl<'a> Meter<'a> {
    pub fn new(fonts: &'a Fonts) -> Self {
        Meter { fonts, prev: None }
    }

    /// Forgets the previous glyph, so no kerning applies across a space.
    pub fn reset(&mut self) {
        self.prev = None;
    }

    /// Returns the glyph for `c`, the kerning to apply before it, and the
    /// total advance (kerning included), all in font units. Spaces are never
    /// kerned.
    pub fn next(&mut self, face: u8, c: char) -> (GlyphId, i32, i32) {
        if is_invisible(c) {
            return (GlyphId(0), 0, 0);
        }
        let (id, adv) = self.fonts.glyph(face, c);
        if c == ' ' {
            self.prev = None;
            return (id, 0, adv);
        }
        let kern = match self.prev {
            Some((f, p)) if f == face => self.fonts.kern(face, p, id),
            _ => 0,
        };
        self.prev = Some((face, id));
        (id, kern, kern + adv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_adds_advances_and_kerning() {
        let f = Fonts::new();
        let (a, adv_a) = f.glyph(0, 'A');
        let (v, adv_v) = f.glyph(0, 'V');
        let kern = f.kern(0, a, v);
        assert!(kern < 0);
        assert_eq!(f.measure(0, "AV"), adv_a + adv_v + kern);
        // No kerning across a space; soft hyphens take no room.
        let space = f.glyph(0, ' ').1;
        assert_eq!(f.measure(0, "A V"), adv_a + space + adv_v);
        assert_eq!(f.measure(0, "A\u{ad}V"), f.measure(0, "AV"));
    }
}
