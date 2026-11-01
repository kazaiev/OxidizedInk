//! Pictures: JPEG and PNG decoded to gray, scaled to fit a box, and drawn
//! dithered to the panel's 8 gray levels.
//!
//! The decoders hand over rows of gray top to bottom, and a `Scaler` shrinks
//! them as they arrive, so a large picture never sits in memory at full
//! size. The JPEG decoder also shrinks each 8×8 block while decoding it.

use alloc::vec;
use alloc::vec::Vec;

use crate::render::Canvas;
use crate::{jpeg, png, Error};

/// An 8-bit gray picture, 255 = white.
pub struct Gray {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u8>,
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Decodes a JPEG or PNG file to gray, scaled to fit `max_w`×`max_h` and
/// keeping its shape. Pictures smaller than that are enlarged.
pub fn decode(data: &[u8], max_w: i32, max_h: i32) -> Result<Gray, Error> {
    if data.starts_with(&[0xff, 0xd8]) {
        jpeg::decode(data, max_w.max(1) as usize, max_h.max(1) as usize)
    } else if data.starts_with(PNG) {
        png::decode(data, max_w.max(1) as usize, max_h.max(1) as usize)
    } else {
        Err(Error::Image("not a JPEG or PNG file"))
    }
}

/// A JPEG or PNG file's width and height, read from its first bytes
/// without decoding it. None if it isn't a picture `decode` reads, or if
/// `data` ends before the size.
pub fn size(data: &[u8]) -> Option<(u32, u32)> {
    let (w, h) = if data.starts_with(&[0xff, 0xd8]) {
        jpeg::size(data)?
    } else if data.starts_with(PNG) {
        png::size(data)?
    } else {
        return None;
    };
    let ok = |v: usize| (1..=1 << 16).contains(&v);
    (ok(w) && ok(h)).then_some((w as u32, h as u32))
}

/// The size of a `w`×`h` picture scaled to fit `max_w`×`max_h`, keeping its
/// shape.
pub(crate) fn fit(w: usize, h: usize, max_w: usize, max_h: usize) -> (usize, usize) {
    if w * max_h <= h * max_w {
        ((w * max_h + h / 2) / h, max_h)
    } else {
        (max_w, (h * max_w + w / 2) / w)
    }
}

/// Takes a picture's rows top to bottom and scales them to the output size.
/// Shrinking averages the source pixels that land in each output pixel as
/// the rows arrive; enlarging keeps the whole source and interpolates.
pub(crate) struct Scaler {
    src_w: usize,
    src_h: usize,
    out: Gray,
    /// Source rows received.
    y: usize,
    shrink: bool,
    /// Shrinking: the output column of each source column, the number of
    /// source columns in each output column, and the sums for the output row
    /// being gathered.
    col: Vec<u16>,
    cols: Vec<u16>,
    sum: Vec<u32>,
    rows: u32,
    /// Enlarging: the whole source.
    src: Vec<u8>,
}

impl Scaler {
    pub(crate) fn new(src_w: usize, src_h: usize, w: usize, h: usize) -> Self {
        let (w, h) = (w.max(1), h.max(1));
        let shrink = w <= src_w && h <= src_h;
        let mut s = Scaler {
            src_w,
            src_h,
            out: Gray { width: w as i32, height: h as i32, pixels: vec![255; w * h] },
            y: 0,
            shrink,
            col: Vec::new(),
            cols: Vec::new(),
            sum: Vec::new(),
            rows: 0,
            src: Vec::new(),
        };
        if shrink {
            s.col = (0..src_w).map(|x| (x * w / src_w) as u16).collect();
            s.cols = vec![0; w];
            for &c in &s.col {
                s.cols[c as usize] += 1;
            }
            s.sum = vec![0; w];
        } else {
            s.src = vec![255; src_w * src_h];
        }
        s
    }

    /// The next source row, `src_w` pixels (more are ignored).
    pub(crate) fn row(&mut self, row: &[u8]) {
        if self.y >= self.src_h {
            return;
        }
        let row = &row[..self.src_w];
        if self.shrink {
            let h = self.out.height as usize;
            let out_y = self.y * h / self.src_h;
            for (&c, &p) in self.col.iter().zip(row) {
                self.sum[c as usize] += p as u32;
            }
            self.rows += 1;
            // Flush when the next source row starts a new output row.
            if (self.y + 1) * h / self.src_h != out_y || self.y + 1 == self.src_h {
                let w = self.out.width as usize;
                let dst = &mut self.out.pixels[out_y * w..][..w];
                for ((d, s), &n) in dst.iter_mut().zip(&mut self.sum).zip(&self.cols) {
                    let n = n as u32 * self.rows;
                    *d = ((*s + n / 2) / n) as u8;
                    *s = 0;
                }
                self.rows = 0;
            }
        } else {
            self.src[self.y * self.src_w..][..self.src_w].copy_from_slice(row);
        }
        self.y += 1;
    }

    pub(crate) fn finish(mut self) -> Gray {
        if !self.shrink {
            self.enlarge();
        }
        self.out
    }

    /// Bilinear interpolation of the source, in 16.16 fixed point.
    fn enlarge(&mut self) {
        let (w, h) = (self.out.width as usize, self.out.height as usize);
        let (sw, sh) = (self.src_w as i64, self.src_h as i64);
        // The source coordinate of output pixel `i`'s centre.
        let at = |i: usize, n: usize, s: i64| -> (usize, u32) {
            let v = ((2 * i as i64 + 1) * s * 65536 / (2 * n as i64) - 32768).clamp(0, (s - 1) * 65536);
            ((v >> 16) as usize, (v & 0xffff) as u32)
        };
        let xs: Vec<_> = (0..w).map(|x| at(x, w, sw)).collect();
        for y in 0..h {
            let (y0, fy) = at(y, h, sh);
            let y1 = (y0 + 1).min(self.src_h - 1);
            let (r0, r1) = (&self.src[y0 * self.src_w..], &self.src[y1 * self.src_w..]);
            for (x, &(x0, fx)) in xs.iter().enumerate() {
                let x1 = (x0 + 1).min(self.src_w - 1);
                let lerp = |a: u8, b: u8, f: u32| (a as u32 * (65536 - f) + b as u32 * f) >> 8;
                let top = lerp(r0[x0], r0[x1], fx);
                let bottom = lerp(r1[x0], r1[x1], fx);
                let v = (top as u64 * (65536 - fy) as u64 + bottom as u64 * fy as u64 + (1 << 23)) >> 24;
                self.out.pixels[y * w + x] = v.min(255) as u8;
            }
        }
    }
}

/// The canvas values that the panel shows as its 8 gray levels.
const LEVELS: [u8; 8] = [0, 36, 72, 109, 145, 182, 218, 255];

/// The average gray `Gray::brighten` lifts a dark picture to.
const LIGHTEN_TO: f32 = 150.0;

impl Gray {
    /// Stretches the contrast so the darkest and lightest 1% of pixels
    /// become black and white, then, if the picture is still mostly dark,
    /// lightens its middle grays until its average is `LIGHTEN_TO`. Meant
    /// for small pictures such as the library's covers, where photographed
    /// cloth bindings otherwise come out as flat dark blocks.
    pub fn brighten(&mut self) {
        let mut hist = [0u32; 256];
        for &p in &self.pixels {
            hist[p as usize] += 1;
        }
        let n = self.pixels.len() as u32;
        // The first gray with more than `k` pixels at or below it.
        let rank = |k: u32| {
            let mut seen = 0;
            (0..256).find(|&v| {
                seen += hist[v];
                seen > k
            })
        };
        let (Some(lo), Some(hi)) = (rank(n / 100), rank(n * 99 / 100)) else { return };
        // A nearly flat picture has nothing to stretch.
        let (lo, span) = if hi > lo + 8 { (lo as f32, (hi - lo) as f32) } else { (0.0, 255.0) };
        let mut map = [0f32; 256];
        for (v, m) in map.iter_mut().enumerate() {
            *m = ((v as f32 - lo) / span).clamp(0.0, 1.0);
        }
        let mean = hist.iter().zip(&map).map(|(&c, &m)| c as f32 * m).sum::<f32>() / n.max(1) as f32;
        let gamma = if mean < 0.5 {
            (libm::logf(LIGHTEN_TO / 255.0) / libm::logf(mean.max(0.02))).clamp(0.35, 1.0)
        } else {
            1.0
        };
        let table = map.map(|m| (255.0 * libm::powf(m, gamma) + 0.5) as u8);
        for p in &mut self.pixels {
            *p = table[*p as usize];
        }
    }

    /// Draws the picture with its top left at `(x, y)`, dithered to the
    /// panel's 8 gray levels by error diffusion (Floyd–Steinberg, every other
    /// row right to left).
    pub fn draw(&self, canvas: &mut Canvas, x: i32, y: i32) {
        let w = self.width as usize;
        // Error carried to this row and the next, with a pixel of padding on
        // each side.
        let mut here = vec![0i16; w + 2];
        let mut next = vec![0i16; w + 2];
        for row in 0..self.height as usize {
            let cy = y + row as i32;
            let ltr = row % 2 == 0;
            for i in 0..w {
                let col = if ltr { i } else { w - 1 - i };
                let v = (self.pixels[row * w + col] as i16 + here[col + 1]).clamp(0, 255);
                let level = LEVELS[((v as u32 * 7 + 127) / 255) as usize];
                let e = v - level as i16;
                let (ahead, behind) = if ltr { (col + 2, col) } else { (col, col + 2) };
                here[ahead] += e * 7 / 16;
                next[behind] += e * 3 / 16;
                next[col + 1] += e * 5 / 16;
                next[ahead] += e / 16;
                let cx = x + col as i32;
                if (0..canvas.width).contains(&cx) && (0..canvas.height).contains(&cy) {
                    canvas.pixels[(cy * canvas.width + cx) as usize] = level;
                }
            }
            core::mem::swap(&mut here, &mut next);
            next.fill(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::gray3;

    #[test]
    fn levels_map_to_panel_grays() {
        for (k, &v) in LEVELS.iter().enumerate() {
            assert_eq!(gray3(v) as usize, k);
        }
    }

    #[test]
    fn fit_keeps_shape() {
        assert_eq!(fit(1824, 2726, 600, 800), (535, 800));
        assert_eq!(fit(1600, 900, 600, 800), (600, 338));
        assert_eq!(fit(300, 400, 600, 800), (600, 800));
    }

    #[test]
    fn shrink_averages() {
        let mut s = Scaler::new(4, 2, 2, 1);
        s.row(&[0, 100, 200, 200]);
        s.row(&[100, 0, 0, 0]);
        let g = s.finish();
        assert_eq!(g.pixels, vec![50, 100]);
    }

    #[test]
    fn enlarge_interpolates() {
        let mut s = Scaler::new(2, 1, 4, 1);
        s.row(&[0, 255]);
        let g = s.finish();
        assert_eq!(g.pixels, vec![0, 64, 191, 255]);
    }

    #[test]
    fn brighten_stretches_and_lightens_dark_pictures() {
        let mean = |g: &Gray| g.pixels.iter().map(|&v| v as u32).sum::<u32>() / g.pixels.len() as u32;
        // Dark and flat, like a photographed cloth binding: grays 20 to 79.
        let mut dark = Gray { width: 60, height: 10, pixels: (0..600).map(|i| 20 + (i % 60) as u8).collect() };
        dark.brighten();
        assert_eq!(dark.pixels.iter().min(), Some(&0));
        assert_eq!(dark.pixels.iter().max(), Some(&255));
        assert!((140..=160).contains(&mean(&dark)), "mean {}", mean(&dark));
        // A light picture is only stretched.
        let mut light = Gray { width: 2, height: 1, pixels: vec![100, 250] };
        light.brighten();
        assert_eq!(light.pixels, vec![0, 255]);
        // Black stays black.
        let mut black = Gray { width: 2, height: 1, pixels: vec![0, 0] };
        black.brighten();
        assert_eq!(black.pixels, vec![0, 0]);
    }

    #[test]
    fn dither_keeps_mean() {
        let g = Gray { width: 64, height: 64, pixels: vec![100; 64 * 64] };
        let mut c = Canvas::new(64, 64);
        g.draw(&mut c, 0, 0);
        let mean = c.pixels.iter().map(|&v| v as u32).sum::<u32>() / (64 * 64);
        assert!((97..=103).contains(&mean), "mean {mean}");
        assert!(c.pixels.iter().all(|v| LEVELS.contains(v)));
    }
}
