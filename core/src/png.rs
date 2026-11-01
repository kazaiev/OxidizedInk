//! PNG decoding to gray: every color type and bit depth, interlaced or not.
//! Transparent pixels are laid over white. The image data is inflated a row
//! at a time, so only interlaced pictures are ever held whole.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use miniz_oxide::inflate::stream::{inflate, InflateState};
use miniz_oxide::{DataFormat, MZFlush, MZStatus};

use crate::image::{fit, Gray, Scaler};
use crate::Error;

/// The most pixels an interlaced picture may have: it is decoded whole
/// before scaling.
const MAX_INTERLACED: usize = 2 * 1024 * 1024;

/// Adam7's passes: first column and row, then the steps between them.
const ADAM7: [(usize, usize, usize, usize); 7] =
    [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];

fn bad(what: &'static str) -> Error {
    Error::Image(what)
}

fn u32_be(b: &[u8]) -> usize {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize
}

/// A gray value laid over white with opacity `a`.
fn over_white(g: u32, a: u32) -> u8 {
    ((g * a + 255 * (255 - a) + 127) / 255) as u8
}

fn luma(r: u32, g: u32, b: u32) -> u32 {
    (77 * r + 150 * g + 29 * b + 128) >> 8
}

/// How the pixels are stored, and how to turn them into gray.
struct Format {
    color: u8,
    depth: u8,
    channels: usize,
    /// Each palette entry as gray over white.
    palette: [u8; 256],
    /// The gray or RGB value that tRNS makes transparent.
    key: Option<[u16; 3]>,
}

impl Format {
    /// Bytes in a row of `w` pixels.
    fn row_bytes(&self, w: usize) -> usize {
        (w * self.channels * self.depth as usize).div_ceil(8)
    }

    /// Bytes per pixel, at least 1, for the filters.
    fn bpp(&self) -> usize {
        (self.channels * self.depth as usize / 8).max(1)
    }

    /// Converts an unfiltered row of `out.len()` pixels to gray.
    fn to_gray(&self, raw: &[u8], out: &mut [u8]) {
        let d = self.depth as usize;
        let sample = |i: usize| -> u16 {
            if d == 16 {
                u16::from_be_bytes([raw[2 * i], raw[2 * i + 1]])
            } else if d == 8 {
                raw[i] as u16
            } else {
                let bit = i * d;
                ((raw[bit / 8] >> (8 - d - bit % 8)) & ((1 << d) - 1) as u8) as u16
            }
        };
        // To 8 bits: the high byte of 16, or a small value stretched.
        let to8 = |v: u16| -> u32 {
            match d {
                16 => (v >> 8) as u32,
                8 => v as u32,
                _ => v as u32 * 255 / ((1 << d) - 1),
            }
        };
        let alpha = |v: u16| if d == 16 { (v >> 8) as u32 } else { v as u32 };
        for (x, o) in out.iter_mut().enumerate() {
            *o = match self.color {
                0 => {
                    let v = sample(x);
                    if self.key.is_some_and(|k| k[0] == v) {
                        255
                    } else {
                        to8(v) as u8
                    }
                }
                2 => {
                    let (r, g, b) = (sample(3 * x), sample(3 * x + 1), sample(3 * x + 2));
                    if self.key == Some([r, g, b]) {
                        255
                    } else {
                        luma(to8(r), to8(g), to8(b)) as u8
                    }
                }
                3 => self.palette[sample(x) as usize],
                4 => over_white(to8(sample(2 * x)), alpha(sample(2 * x + 1))),
                _ => {
                    let g = luma(to8(sample(4 * x)), to8(sample(4 * x + 1)), to8(sample(4 * x + 2)));
                    over_white(g, alpha(sample(4 * x + 3)))
                }
            };
        }
    }
}

/// Inflates the image data on demand.
struct Inflater<'a> {
    state: Box<InflateState>,
    input: &'a [u8],
}

impl Inflater<'_> {
    /// Fills `out`, or returns false if the data ends first.
    fn read(&mut self, out: &mut [u8]) -> bool {
        let mut filled = 0;
        while filled < out.len() {
            let r = inflate(&mut self.state, self.input, &mut out[filled..], MZFlush::None);
            self.input = &self.input[r.bytes_consumed..];
            filled += r.bytes_written;
            match r.status {
                Ok(MZStatus::StreamEnd) => return filled == out.len(),
                Ok(_) if r.bytes_consumed > 0 || r.bytes_written > 0 => {}
                _ => return false,
            }
        }
        true
    }
}

/// Reverses a row's filter, given the row above (zeros for the first).
fn unfilter(filter: u8, row: &mut [u8], prev: &[u8], bpp: usize) -> Result<(), Error> {
    match filter {
        0 => {}
        1 => {
            for i in bpp..row.len() {
                row[i] = row[i].wrapping_add(row[i - bpp]);
            }
        }
        2 => {
            for (r, &p) in row.iter_mut().zip(prev) {
                *r = r.wrapping_add(p);
            }
        }
        3 => {
            for i in 0..row.len() {
                let left = if i >= bpp { row[i - bpp] } else { 0 };
                row[i] = row[i].wrapping_add(((left as u16 + prev[i] as u16) / 2) as u8);
            }
        }
        4 => {
            for i in 0..row.len() {
                let (a, b) = (if i >= bpp { row[i - bpp] } else { 0 }, prev[i]);
                let c = if i >= bpp { prev[i - bpp] } else { 0 };
                let p = a as i16 + b as i16 - c as i16;
                let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
                let pred = if pa <= pb && pa <= pc {
                    a
                } else if pb <= pc {
                    b
                } else {
                    c
                };
                row[i] = row[i].wrapping_add(pred);
            }
        }
        _ => return Err(bad("bad PNG filter")),
    }
    Ok(())
}

/// The width and height in the header, which comes first.
pub(crate) fn size(data: &[u8]) -> Option<(usize, usize)> {
    let h = data.get(8..24).filter(|h| &h[4..8] == b"IHDR")?;
    Some((u32_be(&h[8..12]), u32_be(&h[12..16])))
}

pub(crate) fn decode(data: &[u8], max_w: usize, max_h: usize) -> Result<Gray, Error> {
    let mut header: Option<&[u8]> = None;
    let mut plte: &[u8] = &[];
    let mut trns: &[u8] = &[];
    let mut idat = Vec::new();
    let mut pos = 8;
    while let Some(b) = data.get(pos..pos + 8) {
        let len = u32_be(b);
        let body = data.get(pos + 8..pos + 8 + len).ok_or(bad("truncated PNG"))?;
        match &b[4..8] {
            b"IHDR" => header = Some(body),
            b"PLTE" => plte = body,
            b"tRNS" => trns = body,
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + len;
    }
    let h = header.filter(|h| h.len() >= 13).ok_or(bad("no PNG header"))?;
    let (width, height) = (u32_be(&h[0..4]), u32_be(&h[4..8]));
    let (depth, color, interlaced) = (h[8], h[9], h[12] != 0);
    let channels = match (color, depth) {
        (0, 1 | 2 | 4 | 8 | 16) | (3, 1 | 2 | 4 | 8) => 1,
        (2, 8 | 16) => 3,
        (4, 8 | 16) => 2,
        (6, 8 | 16) => 4,
        _ => return Err(bad("bad PNG color type or depth")),
    };
    if width == 0 || height == 0 || width > 1 << 16 || height > 1 << 16 {
        return Err(bad("bad PNG size"));
    }
    let mut format = Format { color, depth, channels, palette: [255; 256], key: None };
    for (i, rgb) in plte.as_chunks::<3>().0.iter().take(256).enumerate() {
        let g = luma(rgb[0] as u32, rgb[1] as u32, rgb[2] as u32);
        let a = trns.get(i).copied().unwrap_or(255) as u32;
        format.palette[i] = over_white(g, a);
    }
    let word = |i: usize| trns.get(2 * i..2 * i + 2).map(|b| u16::from_be_bytes([b[0], b[1]]));
    format.key = match color {
        0 => word(0).map(|g| [g, 0, 0]),
        2 => word(0).zip(word(1)).zip(word(2)).map(|((r, g), b)| [r, g, b]),
        _ => None,
    };

    let mut inflater = Inflater { state: InflateState::new_boxed(DataFormat::ZLibIgnoreChecksum), input: &idat };
    let (w, h) = fit(width, height, max_w, max_h);
    let mut scaler = Scaler::new(width, height, w, h);
    let bpp = format.bpp();
    let mut gray = vec![0u8; width];
    if !interlaced {
        let rb = format.row_bytes(width);
        let mut row = vec![0u8; rb + 1];
        let mut prev = vec![0u8; rb];
        for _ in 0..height {
            if !inflater.read(&mut row) {
                return Err(bad("truncated PNG"));
            }
            unfilter(row[0], &mut row[1..], &prev, bpp)?;
            format.to_gray(&row[1..], &mut gray);
            scaler.row(&gray);
            prev.copy_from_slice(&row[1..]);
        }
        return Ok(scaler.finish());
    }

    // Interlaced: seven passes over smaller pictures, gathered whole.
    if width * height > MAX_INTERLACED {
        return Err(bad("interlaced PNG too large"));
    }
    let mut whole = vec![255u8; width * height];
    for (x0, y0, dx, dy) in ADAM7 {
        let (pw, ph) = (width.saturating_sub(x0).div_ceil(dx), height.saturating_sub(y0).div_ceil(dy));
        if pw == 0 || ph == 0 {
            continue;
        }
        let rb = format.row_bytes(pw);
        let mut row = vec![0u8; rb + 1];
        let mut prev = vec![0u8; rb];
        for r in 0..ph {
            if !inflater.read(&mut row) {
                return Err(bad("truncated PNG"));
            }
            unfilter(row[0], &mut row[1..], &prev, bpp)?;
            format.to_gray(&row[1..], &mut gray[..pw]);
            let y = y0 + r * dy;
            for (i, &g) in gray[..pw].iter().enumerate() {
                whole[y * width + x0 + i * dx] = g;
            }
            prev.copy_from_slice(&row[1..]);
        }
    }
    for row in whole.chunks(width) {
        scaler.row(row);
    }
    Ok(scaler.finish())
}
