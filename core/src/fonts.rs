//! A font family's four styles, indexed by style bits (`text::BOLD`,
//! `text::ITALIC`), with glyph ids and advance widths cached for common
//! characters: Literata, built in, or a family read from the card. Widths
//! are in font units; `Fonts::units_per_em` converts.

use ab_glyph::{Font, FontRef, FontVec, Glyph, GlyphId, OutlinedGlyph, PxScale};
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::cell::Cell;
use core::sync::atomic::{AtomicU32, Ordering};
use ttf_parser::gpos::{PairAdjustment, PositioningSubtable};

const DATA: [&[u8]; 4] = [
    include_bytes!("../../fonts/Literata-Regular.ttf"),
    include_bytes!("../../fonts/Literata-Bold.ttf"),
    include_bytes!("../../fonts/Literata-Italic.ttf"),
    include_bytes!("../../fonts/Literata-BoldItalic.ttf"),
];

/// Kerning lookups remembered, a power of two. Text uses the same pairs
/// over and over, and a GPOS lookup costs several binary searches.
const KERN_CACHE: usize = 4096;

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

/// Stand-ins for characters the font may lack.
fn substitute(c: char) -> char {
    match c {
        '\t' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' => ' ',
        '\u{2010}' => '-',
        '\u{2032}' => '\'',
        '\u{2033}' => '"',
        _ => c,
    }
}

/// Ids for fonts read from the card; the built-in family is 0.
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

/// One face: built in, or read from the card and owning its data.
enum Face {
    BuiltIn(Box<FontRef<'static>>, &'static [u8]),
    Loaded(FontVec),
}

/// Calls the same `ab_glyph::Font` method on either kind of face.
macro_rules! with_font {
    ($face:expr, $f:ident => $e:expr) => {
        match $face {
            Face::BuiltIn($f, _) => $e,
            Face::Loaded($f) => $e,
        }
    };
}

impl Face {
    fn data(&self) -> &[u8] {
        match self {
            Face::BuiltIn(_, data) => data,
            Face::Loaded(f) => f.as_slice(),
        }
    }
}

/// A face's pair kerning, from its GPOS `kern` feature. Fonts made today
/// keep kerning there rather than in the old `kern` table, the only one
/// ab_glyph reads. The tables are read again from the face's data for each
/// pair not in `Fonts::kern_cache`, so loaded faces can own their data.
struct Kerning {
    /// The `kern` feature's lookups, in order.
    lookups: Vec<u16>,
}

/// A face's GPOS table, parsed with only the tables it needs.
fn gpos(data: &[u8]) -> Option<ttf_parser::opentype_layout::LayoutTable<'_>> {
    let raw = ttf_parser::RawFace::parse(data, 0).ok()?;
    let table = |tag: &[u8; 4]| raw.table(ttf_parser::Tag::from_bytes(tag));
    let tables = ttf_parser::RawFaceTables {
        head: table(b"head")?,
        hhea: table(b"hhea")?,
        maxp: table(b"maxp")?,
        gpos: table(b"GPOS"),
        ..Default::default()
    };
    ttf_parser::Face::from_raw_tables(tables).ok()?.tables().gpos
}

impl Kerning {
    fn new(data: &[u8]) -> Kerning {
        let Some(gpos) = gpos(data) else { return Kerning { lookups: Vec::new() } };
        let kern = ttf_parser::Tag::from_bytes(b"kern");
        let mut lookups: Vec<u16> =
            gpos.features.into_iter().filter(|f| f.tag == kern).flat_map(|f| f.lookup_indices).collect();
        lookups.sort_unstable();
        lookups.dedup();
        Kerning { lookups }
    }

    /// The adjustment between glyphs `a` and `b`, in font units: each
    /// lookup's first pair-adjustment subtable that has the pair.
    fn pair(&self, data: &[u8], a: GlyphId, b: GlyphId) -> i32 {
        let Some(gpos) = gpos(data) else { return 0 };
        let (a, b) = (ttf_parser::GlyphId(a.0), ttf_parser::GlyphId(b.0));
        let mut total = 0;
        for lookup in self.lookups.iter().filter_map(|&i| gpos.lookups.get(i)) {
            let found = lookup.subtables.into_iter::<PositioningSubtable>().find_map(|s| match s {
                PositioningSubtable::Pair(PairAdjustment::Format1 { coverage, sets }) => {
                    sets.get(coverage.get(a)?)?.get(b)
                }
                PositioningSubtable::Pair(PairAdjustment::Format2 { coverage, classes, matrix }) => {
                    coverage.get(a)?;
                    matrix.get((classes.0.get(a), classes.1.get(b)))
                }
                _ => None,
            });
            if let Some((first, _)) = found {
                total += first.x_advance as i32;
            }
        }
        total
    }
}

/// Whether `data` is a font file this reader can use.
pub fn is_font(data: &[u8]) -> bool {
    FontRef::try_from_slice(data).is_ok()
}

/// A style of a family, for sorting font files out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl Style {
    /// The style a font file's name says, as font files are usually named
    /// ("Name-Regular.ttf", "Name-BoldItalic.otf", "Name Italic.ttf"), or
    /// None for another weight, such as "SemiBold" or "Light". The upload
    /// page names files `REGULAR.TTF`, `BOLD.TTF`, `ITALIC.TTF` and
    /// `BOLDITAL.TTF`.
    pub fn of_file(name: &str) -> Option<Style> {
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s).to_ascii_lowercase();
        if stem == "boldital" {
            return Some(Style::BoldItalic);
        }
        let style = stem.rsplit(['-', '_', ' ']).next().unwrap_or("");
        let has = |w: &str| stem.contains(w);
        let other = ["thin", "light", "medium", "semibold", "demibold", "extrabold", "black", "heavy", "book"];
        if other.iter().any(|w| has(w)) && !matches!(style, "regular" | "bold" | "italic" | "bolditalic") {
            return None;
        }
        match (has("bold") && !has("semibold") && !has("extrabold"), has("italic") || has("oblique")) {
            (true, true) => Some(Style::BoldItalic),
            (true, false) => Some(Style::Bold),
            (false, true) => Some(Style::Italic),
            (false, false) => Some(Style::Regular),
        }
    }

    /// The style bits (`text::BOLD`, `text::ITALIC`).
    fn bits(self) -> usize {
        match self {
            Style::Regular => 0,
            Style::Bold => 1,
            Style::Italic => 2,
            Style::BoldItalic => 3,
        }
    }
}

pub struct Fonts {
    /// Which cached glyphs belong to this family (0 for the built-in one).
    pub id: u32,
    faces: Vec<Face>,
    /// The face for each style: a missing style uses the regular face.
    styles: [usize; 4],
    kerning: Vec<Kerning>,
    /// Recent GPOS kerning lookups, by a hash of the face and glyphs: the
    /// key plus one (0 is empty) above the value's 16 bits.
    kern_cache: Vec<Cell<u64>>,
    /// `(glyph id, advance)` for each face and cached character.
    cache: Vec<(u16, u16)>,
    pub units_per_em: i32,
    /// Distance from the baseline to the top and bottom of the line box, in
    /// font units, both positive.
    pub ascent: i32,
    pub descent: i32,
}

impl Fonts {
    /// The built-in Literata.
    pub fn new() -> Self {
        let faces = DATA.map(|d| Face::BuiltIn(Box::new(FontRef::try_from_slice(d).expect("embedded font")), d));
        Fonts::with_faces(0, faces.into(), [0, 1, 2, 3])
    }

    /// A family read from the card: `files` are its font files with the
    /// style each is. None without a regular face that parses.
    pub fn from_files(files: Vec<(Style, Vec<u8>)>) -> Option<Fonts> {
        let mut faces = Vec::new();
        let mut styles = [usize::MAX; 4];
        for (style, data) in files {
            if styles[style.bits()] != usize::MAX {
                continue;
            }
            if let Ok(f) = FontVec::try_from_vec(data) {
                styles[style.bits()] = faces.len();
                faces.push(Face::Loaded(f));
            }
        }
        let regular = styles[0];
        if regular == usize::MAX {
            return None;
        }
        // Bold italic falls back to bold, then italic; the others to regular.
        let fallback = |s: usize, other: usize| if s == usize::MAX { other } else { s };
        let bold = fallback(styles[1], regular);
        let italic = fallback(styles[2], regular);
        let bold_italic = fallback(styles[3], fallback(styles[1], italic));
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        Some(Fonts::with_faces(id, faces, [regular, bold, italic, bold_italic]))
    }

    fn with_faces(id: u32, faces: Vec<Face>, styles: [usize; 4]) -> Fonts {
        let regular = &faces[styles[0]];
        let (units_per_em, ascent, descent) = with_font!(regular, f => (
            f.units_per_em().unwrap_or(1000.0) as i32,
            f.ascent_unscaled() as i32,
            -f.descent_unscaled() as i32,
        ));
        let mut fonts = Fonts {
            id,
            kerning: faces.iter().map(|f| Kerning::new(f.data())).collect(),
            faces,
            styles,
            kern_cache: (0..KERN_CACHE).map(|_| Cell::new(0)).collect(),
            units_per_em,
            ascent,
            descent,
            cache: Vec::new(),
        };
        let mut cache = Vec::with_capacity(4 * CACHED);
        for style in 0..4 {
            for c in (0..0x500).chain(0x2000..0x2070) {
                let (id, adv) = fonts.lookup(style, char::from_u32(c).unwrap_or(' '));
                cache.push((id.0, adv as u16));
            }
        }
        fonts.cache = cache;
        fonts
    }

    fn lookup(&self, style: usize, c: char) -> (GlyphId, i32) {
        if is_invisible(c) {
            return (GlyphId(0), 0);
        }
        with_font!(&self.faces[self.styles[style]], f => {
            let mut id = f.glyph_id(c);
            if id.0 == 0 {
                let s = substitute(c);
                if s != c {
                    id = f.glyph_id(s);
                }
            }
            (id, f.h_advance_unscaled(id) as i32)
        })
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

    /// Kerning between `a` and `b`, from GPOS, or else the old `kern` table.
    pub fn kern(&self, face: u8, a: GlyphId, b: GlyphId) -> i32 {
        let face = self.styles[(face & 3) as usize];
        if self.kerning[face].lookups.is_empty() {
            return with_font!(&self.faces[face], f => f.kern_unscaled(a, b)) as i32;
        }
        let key = ((face as u64) << 32 | (a.0 as u64) << 16 | b.0 as u64) + 1;
        let hash = key.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> (64 - KERN_CACHE.trailing_zeros());
        let slot = &self.kern_cache[hash as usize];
        if slot.get() >> 16 == key {
            return slot.get() as u16 as i16 as i32;
        }
        let kern = self.kerning[face].pair(self.faces[face].data(), a, b);
        slot.set(key << 16 | kern as i16 as u16 as u64);
        kern
    }

    /// The outline of glyph `glyph` in `face` (style bits).
    pub fn outline(&self, face: u8, glyph: Glyph) -> Option<OutlinedGlyph> {
        with_font!(&self.faces[self.styles[(face & 3) as usize]], f => f.outline_glyph(glyph))
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
#[derive(Clone)]
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

    #[test]
    fn loaded_fonts_measure_like_built_in() {
        let built_in = Fonts::new();
        let files = [Style::Regular, Style::Bold, Style::Italic, Style::BoldItalic].into_iter().zip(DATA);
        let loaded = Fonts::from_files(files.map(|(s, d)| (s, d.to_vec())).collect()).unwrap();
        assert_ne!(loaded.id, built_in.id);
        let text = "AVATAR, Ty Young; «Їжак» і м'ята";
        for face in 0..4 {
            assert_eq!(loaded.measure(face, text), built_in.measure(face, text));
        }
        // Only a regular face: the other styles use it.
        let regular = Fonts::from_files([(Style::Regular, DATA[0].to_vec())].into()).unwrap();
        assert_eq!(regular.measure(3, text), built_in.measure(0, text));
        assert!(Fonts::from_files([(Style::Bold, DATA[1].to_vec())].into()).is_none());
    }

    #[test]
    fn styles_from_file_names() {
        let s = Style::of_file;
        assert_eq!(s("Literata-Regular.ttf"), Some(Style::Regular));
        assert_eq!(s("SourceSerif4-BoldItalic.otf"), Some(Style::BoldItalic));
        assert_eq!(s("Charis SIL Italic.ttf"), Some(Style::Italic));
        assert_eq!(s("Merriweather_Bold.ttf"), Some(Style::Bold));
        assert_eq!(s("Gentium.ttf"), Some(Style::Regular));
        assert_eq!(s("Literata-SemiBold.ttf"), None);
        assert_eq!(s("Literata-LightItalic.ttf"), None);
        assert_eq!(s("Literata-ExtraBold.ttf"), None);
        assert_eq!(s("BOLDITAL.TTF"), Some(Style::BoldItalic));
        assert_eq!(s("REGULAR.OTF"), Some(Style::Regular));
    }
}

