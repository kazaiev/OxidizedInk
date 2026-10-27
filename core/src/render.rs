//! Rasterizing: an 8-bit gray canvas (255 = white) and a glyph cache.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use ab_glyph::{point, Font, GlyphId};

use crate::fonts::{is_invisible, Fonts, Meter};
use crate::layout::{Item, Page};
use crate::text::Chapter;

pub const WHITE: u8 = 255;
pub const BLACK: u8 = 0;

/// The panel's gray level (0 = black ..= 7 = white) for a canvas value.
pub fn gray3(v: u8) -> u8 {
    v >> 5
}

pub struct Canvas {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u8>,
}

impl Canvas {
    pub fn new(width: i32, height: i32) -> Self {
        Canvas { width, height, pixels: vec![WHITE; (width * height) as usize] }
    }

    pub fn clear(&mut self) {
        self.pixels.fill(WHITE);
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, v: u8) {
        let (x0, x1) = (x.max(0), (x + w).min(self.width));
        let (y0, y1) = (y.max(0), (y + h).min(self.height));
        if x0 >= x1 {
            return;
        }
        for yy in y0..y1 {
            let row = (yy * self.width) as usize;
            self.pixels[row + x0 as usize..row + x1 as usize].fill(v);
        }
    }

    pub fn invert_rect(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let (x0, x1) = (x.max(0), (x + w).min(self.width));
        let (y0, y1) = (y.max(0), (y + h).min(self.height));
        for yy in y0..y1 {
            let row = (yy * self.width) as usize;
            for p in &mut self.pixels[row + x0.max(0) as usize..row + x1.max(x0) as usize] {
                *p = 255 - *p;
            }
        }
    }

    /// Darkens the canvas by a coverage bitmap.
    fn blit(&mut self, g: &GlyphBitmap, x: i32, y: i32) {
        let (gw, gh) = (g.w as i32, g.h as i32);
        let (x0, y0) = (x + g.left as i32, y + g.top as i32);
        for gy in 0..gh {
            let py = y0 + gy;
            if py < 0 || py >= self.height {
                continue;
            }
            for gx in 0..gw {
                let px = x0 + gx;
                if px < 0 || px >= self.width {
                    continue;
                }
                let cov = g.cov[(gy * gw + gx) as usize] as u32;
                if cov == 0 {
                    continue;
                }
                let p = &mut self.pixels[(py * self.width + px) as usize];
                *p = ((*p as u32 * (255 - cov) + 127) / 255) as u8;
            }
        }
    }
}

struct GlyphBitmap {
    left: i16,
    top: i16,
    w: u16,
    h: u16,
    cov: Box<[u8]>,
}

/// Rasterizes glyphs on demand and keeps them for reuse.
pub struct Renderer {
    cache: BTreeMap<(u8, u16, u16), GlyphBitmap>,
    bytes: usize,
    /// Drop the whole cache when it grows past this many bytes.
    pub budget: usize,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        Renderer { cache: BTreeMap::new(), bytes: 0, budget: 512 * 1024 }
    }

    fn glyph(&mut self, fonts: &Fonts, face: u8, px: i32, id: GlyphId) -> &GlyphBitmap {
        let key = (face, px as u16, id.0);
        if !self.cache.contains_key(&key) {
            if self.bytes > self.budget {
                self.cache.clear();
                self.bytes = 0;
            }
            let g = rasterize(fonts, face, px, id);
            self.bytes += g.cov.len() + 32;
            self.cache.insert(key, g);
        }
        &self.cache[&key]
    }

    /// Draws `s` in black with its left edge at `x` (1/64 px) on baseline
    /// `y`; returns the end x in 1/64 px.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_str(&mut self, fonts: &Fonts, canvas: &mut Canvas, face: u8, px: i32, x: i32, y: i32, s: &str) -> i32 {
        let mut meter = Meter::new(fonts);
        let mut pen = 0;
        for c in s.chars() {
            let (id, kern, adv) = meter.next(face, c);
            if c == ' ' || is_invisible(c) {
                pen += adv;
                continue;
            }
            let gx = (x + fonts.to_fx(pen + kern, px) + 32) >> 6;
            let g = self.glyph(fonts, face, px, id);
            canvas.blit(g, gx, y);
            pen += adv;
        }
        x + fonts.to_fx(pen, px)
    }

    pub fn draw_page(&mut self, fonts: &Fonts, canvas: &mut Canvas, ch: &Chapter, page: &Page) {
        for item in &page.items {
            match item {
                Item::Text(line) => {
                    let text = &ch.blocks[line.block as usize].text;
                    for r in &line.runs {
                        let s = &text[r.start as usize..r.end as usize];
                        self.draw_str(fonts, canvas, r.face, line.px, r.x, line.baseline, s);
                    }
                    if let Some((x, face)) = line.hyphen {
                        self.draw_str(fonts, canvas, face, line.px, x, line.baseline, "-");
                    }
                }
                &Item::Rule { x0, x1, y } => canvas.fill_rect(x0, y, x1 - x0, 1, BLACK),
            }
        }
    }
}

fn rasterize(fonts: &Fonts, face: u8, px: i32, id: GlyphId) -> GlyphBitmap {
    let glyph = id.with_scale_and_position(fonts.scale(px), point(0.0, 0.0));
    let Some(outline) = fonts.face(face).outline_glyph(glyph) else {
        return GlyphBitmap { left: 0, top: 0, w: 0, h: 0, cov: Box::new([]) };
    };
    let b = outline.px_bounds();
    let (w, h) = (b.width() as usize, b.height() as usize);
    let mut cov = vec![0u8; w * h].into_boxed_slice();
    outline.draw(|x, y, c| {
        let i = y as usize * w + x as usize;
        if i < cov.len() {
            cov[i] = (c * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        }
    });
    GlyphBitmap { left: b.min.x as i16, top: b.min.y as i16, w: w as u16, h: h as u16, cov }
}
