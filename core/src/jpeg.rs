//! JPEG decoding to gray: baseline and progressive, Huffman coded, 8 bits
//! per sample. Only the first component is reconstructed: the luma, or the
//! only one of a gray picture. The inverse DCT can shrink each 8×8 block to
//! 4×4, 2×2 or 1×1 pixels by using only the block's lowest frequencies,
//! which is cheaper than decoding at full size and shrinking afterwards.
//!
//! A baseline picture streams through one row of blocks at a time. A
//! progressive one comes in several passes over the whole picture, so its
//! luma coefficients are kept until the end: only the ones the shrunken
//! blocks use, plus a bit per coefficient that says whether it is zero (the
//! refinement passes need that for all of them).

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::f32::consts::FRAC_1_SQRT_2;

use crate::image::{fit, Gray, Scaler};
use crate::Error;

/// The block position (row * 8 + column) of each coefficient in zigzag order.
const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21,
    28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61,
    54, 47, 55, 62, 63,
];

/// The most memory a progressive picture's kept coefficients may take. A
/// larger picture is decoded with smaller blocks.
const COEF_BUDGET: usize = 1536 * 1024;

/// Bits looked up at once when decoding Huffman codes.
const FAST: u32 = 9;

fn bad(what: &'static str) -> Error {
    Error::Image(what)
}

struct Huffman {
    /// `length << 8 | symbol` for each `FAST`-bit prefix of a code no longer
    /// than that, 0 for longer codes.
    fast: [u16; 1 << FAST],
    /// The largest code of each length, -1 for none.
    max: [i32; 17],
    /// Added to a code of each length to index `symbols`.
    delta: [i32; 17],
    symbols: [u8; 256],
}

impl Huffman {
    fn new(counts: &[u8], symbols: &[u8]) -> Result<Box<Self>, Error> {
        let mut h = Box::new(Huffman { fast: [0; 1 << FAST], max: [-1; 17], delta: [0; 17], symbols: [0; 256] });
        h.symbols[..symbols.len()].copy_from_slice(symbols);
        let mut code = 0i32;
        let mut k = 0;
        for len in 1..=16 {
            let n = counts[len - 1] as usize;
            h.delta[len] = k as i32 - code;
            for _ in 0..n {
                if len as u32 <= FAST {
                    let shift = FAST - len as u32;
                    let first = (code << shift) as usize;
                    h.fast[first..first + (1 << shift)].fill((len as u16) << 8 | symbols[k] as u16);
                }
                code += 1;
                k += 1;
            }
            if code > 1 << len {
                return Err(bad("bad Huffman table"));
            }
            if n > 0 {
                h.max[len] = code - 1;
            }
            code <<= 1;
        }
        Ok(h)
    }
}

/// Reads the entropy-coded data of a scan. Stuffed zero bytes after 0xFF are
/// dropped; at a marker it stops and reads zeros.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    /// Unused bits, from the most significant end, and how many there are.
    acc: u32,
    n: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Bits { data, pos, acc: 0, n: 0 }
    }

    fn fill(&mut self) {
        while self.n <= 24 {
            let mut b = 0;
            if let Some(&c) = self.data.get(self.pos) {
                if c != 0xff {
                    b = c;
                    self.pos += 1;
                } else if self.data.get(self.pos + 1) == Some(&0) {
                    b = 0xff;
                    self.pos += 2;
                }
            }
            self.acc |= (b as u32) << (24 - self.n);
            self.n += 8;
        }
    }

    /// The next `k` bits, 1 to 16.
    fn bits(&mut self, k: u32) -> u32 {
        self.fill();
        let v = self.acc >> (32 - k);
        self.acc <<= k;
        self.n -= k;
        v
    }

    fn bit(&mut self) -> bool {
        self.bits(1) != 0
    }

    /// A value of `s` bits, coded with the sign in its first bit.
    fn receive(&mut self, s: u32) -> i32 {
        if s == 0 {
            return 0;
        }
        let v = self.bits(s.min(16)) as i32;
        if v < 1 << (s - 1) {
            v - (1 << s) + 1
        } else {
            v
        }
    }

    fn decode(&mut self, h: &Huffman) -> Result<u8, Error> {
        self.fill();
        let e = h.fast[(self.acc >> (32 - FAST)) as usize];
        if e != 0 {
            let len = (e >> 8) as u32;
            self.acc <<= len;
            self.n -= len;
            return Ok(e as u8);
        }
        for len in FAST + 1..=16 {
            let code = (self.acc >> (32 - len)) as i32;
            if code <= h.max[len as usize] {
                self.acc <<= len;
                self.n -= len;
                return Ok(h.symbols[(code + h.delta[len as usize]) as usize]);
            }
        }
        Err(bad("bad Huffman code"))
    }

    /// Drops the bits left before a restart marker, and the marker.
    fn restart(&mut self) {
        self.acc = 0;
        self.n = 0;
        while let (Some(&a), Some(&b)) = (self.data.get(self.pos), self.data.get(self.pos + 1)) {
            if a == 0xff && (0xd0..=0xd7).contains(&b) {
                self.pos += 2;
                return;
            }
            if a == 0xff && b != 0 && b != 0xff {
                // Some other marker: the data is cut short.
                return;
            }
            self.pos += 1;
        }
    }
}

#[derive(Clone, Copy)]
struct Component {
    id: u8,
    /// Sampling factors.
    h: usize,
    v: usize,
    /// Quantization table.
    tq: usize,
    /// Huffman tables of the current scan, as indexes into `Decoder::huff`.
    dc: usize,
    ac: usize,
    /// The last DC value, which the next one is coded against.
    pred: i32,
}

struct Frame {
    progressive: bool,
    comps: Vec<Component>,
    /// MCUs across and down.
    mcux: usize,
    mcuy: usize,
    /// The luma's size in blocks.
    bw: usize,
    bh: usize,
    /// The shrunken luma's size in pixels.
    out_w: usize,
    out_h: usize,
}

struct Decoder<'a> {
    data: &'a [u8],
    /// Quantization tables, in zigzag order.
    qt: [[u16; 64]; 4],
    /// Huffman tables: DC 0–3, then AC 0–3.
    huff: [Option<Box<Huffman>>; 8],
    /// MCUs between restart markers, 0 for none.
    restart: usize,
    frame: Option<Frame>,
    /// A shrunken block's side: 8, 4, 2 or 1 pixels.
    n: usize,
    /// The inverse DCT's basis for `n`: `basis[i * n + u]` is frequency `u`
    /// at pixel `i`.
    basis: [f32; 64],
    /// For each coefficient in zigzag order, its index among the kept ones,
    /// or 0xff if the shrunken block doesn't use it.
    keep: [u8; 64],
    max_w: usize,
    max_h: usize,
    scaler: Option<Scaler>,
    /// Baseline: the pixels of the row of blocks being decoded.
    rows: Vec<u8>,
    /// Progressive: the luma blocks' kept coefficients, and a bit per
    /// coefficient (zigzag order) that is set once it is nonzero.
    coefs: Vec<i16>,
    nonzero: Vec<u64>,
    /// Progressive: blocks left that have no more coefficients in this band.
    eobrun: u32,
}

/// cos(m·π/16) for m = 0..=8.
const COS: [f32; 9] = [1.0, 0.980_785_3, 0.923_879_5, 0.831_469_6, FRAC_1_SQRT_2, 0.555_570_2, 0.382_683_4, 0.195_090_3, 0.0];

/// cos(m·π/16) for any m.
fn cos16(m: usize) -> f32 {
    let m = m % 32;
    let m = if m > 16 { 32 - m } else { m };
    if m <= 8 {
        COS[m]
    } else {
        -COS[16 - m]
    }
}

/// The width and height in the frame header, found by skipping the segments
/// before it. None for the kinds `decode` refuses, or if `data` ends first.
pub(crate) fn size(data: &[u8]) -> Option<(usize, usize)> {
    let mut pos = 2;
    loop {
        if *data.get(pos)? != 0xff {
            return None;
        }
        while *data.get(pos)? == 0xff {
            pos += 1;
        }
        let marker = data[pos];
        pos += 1;
        if matches!(marker, 0xd8 | 0xd0..=0xd7 | 0x01) {
            continue;
        }
        // Length, then for a frame: precision, height, width.
        let seg = data.get(pos..pos + 7)?;
        let be16 = |i: usize| u16::from_be_bytes([seg[i], seg[i + 1]]) as usize;
        match marker {
            0xc0..=0xc2 => return Some((be16(5), be16(3))),
            0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf | 0xd9 | 0xda => return None,
            _ => pos += be16(0),
        }
    }
}

pub(crate) fn decode(data: &[u8], max_w: usize, max_h: usize) -> Result<Gray, Error> {
    let mut d = Decoder {
        data,
        qt: [[1; 64]; 4],
        huff: Default::default(),
        restart: 0,
        frame: None,
        n: 8,
        basis: [0.0; 64],
        keep: [0xff; 64],
        max_w,
        max_h,
        scaler: None,
        rows: Vec::new(),
        coefs: Vec::new(),
        nonzero: Vec::new(),
        eobrun: 0,
    };
    d.run()?;
    let scaler = d.scaler.take().ok_or(bad("no picture in the file"))?;
    Ok(scaler.finish())
}

impl<'a> Decoder<'a> {
    fn run(&mut self) -> Result<(), Error> {
        let data = self.data;
        let mut pos = 2;
        loop {
            // Find the next marker, skipping anything else and fill bytes.
            while pos < data.len() && data[pos] != 0xff {
                pos += 1;
            }
            while pos < data.len() && data[pos] == 0xff {
                pos += 1;
            }
            let Some(&marker) = data.get(pos) else { break };
            pos += 1;
            match marker {
                0xd8 | 0xd0..=0xd7 | 0x01 => continue,
                0xd9 => break,
                _ => {}
            }
            let len = match data.get(pos..pos + 2) {
                Some(b) => u16::from_be_bytes([b[0], b[1]]) as usize,
                None => break,
            };
            let seg = data.get(pos + 2..pos + len.max(2)).ok_or(bad("truncated JPEG"))?;
            pos += len.max(2);
            match marker {
                0xdb => self.quant_tables(seg)?,
                0xc4 => self.huffman_tables(seg)?,
                0xdd if seg.len() >= 2 => self.restart = u16::from_be_bytes([seg[0], seg[1]]) as usize,
                0xc0..=0xc2 => self.frame(seg, marker == 0xc2)?,
                0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                    return Err(bad("unsupported JPEG (lossless, hierarchical or arithmetic coded)"))
                }
                0xda => {
                    let (end, luma_done) = self.scan(seg, pos)?;
                    pos = end;
                    if luma_done {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
        // The end of a progressive picture: all its passes are in.
        if self.frame.as_ref().is_some_and(|f| f.progressive) {
            self.finish_progressive();
        }
        Ok(())
    }

    fn quant_tables(&mut self, mut seg: &[u8]) -> Result<(), Error> {
        while let Some(&pq_tq) = seg.first() {
            let wide = pq_tq >> 4 != 0;
            let size = if wide { 128 } else { 64 };
            let t = seg.get(1..1 + size).ok_or(bad("bad DQT"))?;
            let q = &mut self.qt[(pq_tq & 3) as usize];
            for (k, q) in q.iter_mut().enumerate() {
                *q = if wide { u16::from_be_bytes([t[2 * k], t[2 * k + 1]]) } else { t[k] as u16 };
            }
            seg = &seg[1 + size..];
        }
        Ok(())
    }

    fn huffman_tables(&mut self, mut seg: &[u8]) -> Result<(), Error> {
        while let Some(&tc_th) = seg.first() {
            let counts = seg.get(1..17).ok_or(bad("bad DHT"))?;
            let total: usize = counts.iter().map(|&c| c as usize).sum();
            let symbols = seg.get(17..17 + total).filter(|_| total <= 256).ok_or(bad("bad DHT"))?;
            let index = (tc_th >> 4 & 1) as usize * 4 + (tc_th & 3) as usize;
            self.huff[index] = Some(Huffman::new(counts, symbols)?);
            seg = &seg[17 + total..];
        }
        Ok(())
    }

    fn frame(&mut self, seg: &[u8], progressive: bool) -> Result<(), Error> {
        if seg.len() < 6 || seg[0] != 8 {
            return Err(bad("unsupported JPEG (not 8 bits per sample)"));
        }
        let height = u16::from_be_bytes([seg[1], seg[2]]) as usize;
        let width = u16::from_be_bytes([seg[3], seg[4]]) as usize;
        let count = seg[5] as usize;
        if width == 0 || height == 0 {
            return Err(bad("unsupported JPEG (no height)"));
        }
        if count != 1 && count != 3 {
            return Err(bad("unsupported JPEG (CMYK)"));
        }
        let mut comps = Vec::new();
        for c in seg.get(6..6 + 3 * count).ok_or(bad("bad SOF"))?.chunks(3) {
            let (h, v) = ((c[1] >> 4) as usize, (c[1] & 15) as usize);
            if !(1..=4).contains(&h) || !(1..=4).contains(&v) {
                return Err(bad("bad sampling factors"));
            }
            comps.push(Component { id: c[0], h, v, tq: (c[2] & 3) as usize, dc: 0, ac: 4, pred: 0 });
        }
        // A gray picture's one component is a single block per MCU, whatever
        // its sampling factors say.
        if count == 1 {
            comps[0].h = 1;
            comps[0].v = 1;
        }
        let hmax = comps.iter().map(|c| c.h).max().unwrap_or(1);
        let vmax = comps.iter().map(|c| c.v).max().unwrap_or(1);
        let mcux = width.div_ceil(8 * hmax);
        let mcuy = height.div_ceil(8 * vmax);
        // The luma's size in pixels and blocks.
        let lw = (width * comps[0].h).div_ceil(hmax);
        let lh = (height * comps[0].v).div_ceil(vmax);
        let (bw, bh) = (lw.div_ceil(8), lh.div_ceil(8));

        // The smallest blocks that still give at least the output size.
        let (w, h) = fit(width, height, self.max_w, self.max_h);
        let (need_w, need_h) = ((w * lw).div_ceil(width), (h * lh).div_ceil(height));
        let mut n = [1, 2, 4, 8]
            .into_iter()
            .find(|&n| (lw * n).div_ceil(8) >= need_w && (lh * n).div_ceil(8) >= need_h)
            .unwrap_or(8);
        let blocks = mcux * comps[0].h * mcuy * comps[0].v;
        if progressive {
            while n > 1 && blocks * (2 * n * n + 8) > COEF_BUDGET {
                n /= 2;
            }
            self.coefs = vec![0; blocks * n * n];
            self.nonzero = vec![0; blocks];
        }
        self.n = n;
        for i in 0..n {
            for u in 0..n {
                let c = if u == 0 { COS[4] } else { 1.0 };
                self.basis[i * n + u] = c * cos16((2 * i + 1) * u * (8 / n)) / 2.0;
            }
        }
        for (k, &p) in ZIGZAG.iter().enumerate() {
            let (row, col) = (p as usize / 8, p as usize % 8);
            self.keep[k] = if row < n && col < n { (row * n + col) as u8 } else { 0xff };
        }
        let (out_w, out_h) = ((lw * n).div_ceil(8), (lh * n).div_ceil(8));
        self.scaler = Some(Scaler::new(out_w, out_h, w, h));
        self.frame = Some(Frame { progressive, comps, mcux, mcuy, bw, bh, out_w, out_h });
        Ok(())
    }

    /// Decodes a scan whose header is `seg` and whose data starts at `pos`.
    /// Returns where the data ends, and whether the luma is complete.
    fn scan(&mut self, seg: &[u8], pos: usize) -> Result<(usize, bool), Error> {
        let frame = self.frame.as_mut().ok_or(bad("scan before frame"))?;
        let count = *seg.first().ok_or(bad("bad SOS"))? as usize;
        let header = seg.get(1..1 + 2 * count + 3).ok_or(bad("bad SOS"))?;
        let mut members = Vec::new();
        for c in header[..2 * count].chunks(2) {
            let i = frame.comps.iter().position(|f| f.id == c[0]).ok_or(bad("bad SOS"))?;
            frame.comps[i].dc = (c[1] >> 4 & 3) as usize;
            frame.comps[i].ac = 4 + (c[1] & 3) as usize;
            frame.comps[i].pred = 0;
            members.push(i);
        }
        let (ss, se) = (header[2 * count] as usize, header[2 * count + 1] as usize);
        let (ah, al) = ((header[2 * count + 2] >> 4) as u32, (header[2 * count + 2] & 15) as u32);
        let progressive = frame.progressive;
        let mut bits = Bits::new(self.data, pos);
        self.eobrun = 0;
        // Scans without the luma, and in a progressive picture the AC passes
        // of other components, are skipped over.
        let wanted = members.contains(&0) && (!progressive || ss == 0 || members == [0]);
        if wanted {
            if !progressive {
                self.sequential(&mut bits, &members)?;
            } else if ss == 0 {
                self.progressive_dc(&mut bits, &members, ah, al)?;
            } else {
                if se > 63 || ss > se {
                    return Err(bad("bad progressive scan"));
                }
                self.progressive_ac(&mut bits, ss, se, ah, al)?;
            }
        }
        Ok((next_marker(self.data, bits.pos), wanted && !progressive))
    }

    fn huffman(&self, i: usize) -> Result<&Huffman, Error> {
        self.huff[i].as_deref().ok_or(bad("missing Huffman table"))
    }

    /// Goes through the blocks of a scan in order, calling `block` with the
    /// component and the luma block's position (`None` for other
    /// components), and handling restart markers. In a scan of one
    /// component, the blocks cover just the component; in one of several,
    /// whole MCUs.
    fn blocks(
        &mut self,
        bits: &mut Bits<'a>,
        members: &[usize],
        mut block: impl FnMut(&mut Self, &mut Bits<'a>, usize, Option<(usize, usize)>) -> Result<(), Error>,
        mut row_done: impl FnMut(&mut Self, usize),
    ) -> Result<(), Error> {
        let f = self.frame.as_ref().ok_or(bad("scan before frame"))?;
        let comps = f.comps.clone();
        // A scan of one component goes through its blocks; only the luma's
        // single scans are decoded. One of several goes through whole MCUs.
        let single = members.len() == 1;
        let (units_x, units_y) = if single { (f.bw, f.bh) } else { (f.mcux, f.mcuy) };
        let mut left = self.restart;
        for uy in 0..units_y {
            for ux in 0..units_x {
                if self.restart > 0 {
                    if left == 0 {
                        bits.restart();
                        if let Some(f) = self.frame.as_mut() {
                            f.comps.iter_mut().for_each(|c| c.pred = 0);
                        }
                        self.eobrun = 0;
                        left = self.restart;
                    }
                    left -= 1;
                }
                if single {
                    block(self, bits, members[0], (members[0] == 0).then_some((ux, uy)))?;
                    continue;
                }
                for &ci in members {
                    let c = comps[ci];
                    for v in 0..c.v {
                        for h in 0..c.h {
                            let at = (ci == 0).then_some((ux * c.h + h, uy * c.v + v));
                            block(self, bits, ci, at)?;
                        }
                    }
                }
            }
            row_done(self, uy);
        }
        Ok(())
    }

    /// A baseline scan: each luma block goes through the inverse DCT into a
    /// row of blocks, and each finished row goes to the scaler.
    fn sequential(&mut self, bits: &mut Bits<'a>, members: &[usize]) -> Result<(), Error> {
        let f = self.frame.as_ref().ok_or(bad("scan before frame"))?;
        let n = self.n;
        // Block rows per MCU.
        let v0 = if members.len() == 1 { 1 } else { f.comps[0].v };
        let stride = f.mcux * f.comps[0].h * n;
        let (out_w, out_h) = (f.out_w, f.out_h);
        self.rows = vec![255; stride * v0 * n];
        let mut y = 0;
        self.blocks(
            bits,
            members,
            |d, bits, ci, at| {
                let mut blk = [0i32; 64];
                d.sequential_block(bits, ci, &mut blk)?;
                if let Some((bx, by)) = at {
                    idct(&blk, n, &d.basis, &mut d.rows[(by % v0) * n * stride + bx * n..], stride);
                }
                Ok(())
            },
            |d, _| {
                if let Some(s) = d.scaler.as_mut() {
                    for r in d.rows.chunks(stride).take(out_h.saturating_sub(y)) {
                        s.row(&r[..out_w]);
                        y += 1;
                    }
                }
            },
        )
    }

    /// Decodes a baseline block into `blk` (dequantized, in block order),
    /// or just past it for components other than the luma.
    fn sequential_block(&mut self, bits: &mut Bits<'a>, ci: usize, blk: &mut [i32; 64]) -> Result<(), Error> {
        let f = self.frame.as_mut().ok_or(bad("scan before frame"))?;
        let c = &mut f.comps[ci];
        let (dc, ac, tq) = (c.dc, c.ac, c.tq);
        let s = bits.decode(self.huff[dc].as_deref().ok_or(bad("missing Huffman table"))?)?;
        c.pred += bits.receive(s as u32);
        let pred = c.pred;
        let q = &self.qt[tq];
        let luma = ci == 0;
        if luma {
            blk[0] = pred * q[0] as i32;
        }
        let ac = self.huff[ac].as_deref().ok_or(bad("missing Huffman table"))?;
        let mut k = 1;
        while k < 64 {
            let rs = bits.decode(ac)?;
            let (r, s) = ((rs >> 4) as usize, (rs & 15) as u32);
            if s == 0 {
                if r != 15 {
                    break;
                }
                k += 16;
                continue;
            }
            k += r;
            let v = bits.receive(s);
            if luma && k < 64 && self.keep[k] != 0xff {
                blk[ZIGZAG[k] as usize] = v * q[k] as i32;
            }
            k += 1;
        }
        Ok(())
    }

    /// A progressive DC pass: the first bits of each block's DC, or one more
    /// bit of it.
    fn progressive_dc(&mut self, bits: &mut Bits<'a>, members: &[usize], ah: u32, al: u32) -> Result<(), Error> {
        let kk = self.n * self.n;
        let gw = self.frame.as_ref().map_or(0, |f| f.mcux * f.comps[0].h);
        self.blocks(
            bits,
            members,
            |d, bits, ci, at| {
                let i = at.map(|(x, y)| (y * gw + x) * kk);
                if ah == 0 {
                    let f = d.frame.as_mut().ok_or(bad("scan before frame"))?;
                    let dc = f.comps[ci].dc;
                    let s = bits.decode(d.huff[dc].as_deref().ok_or(bad("missing Huffman table"))?)?;
                    let f = d.frame.as_mut().ok_or(bad("scan before frame"))?;
                    f.comps[ci].pred += bits.receive(s as u32);
                    if let Some(i) = i {
                        d.coefs[i] = (f.comps[ci].pred << al) as i16;
                    }
                } else if bits.bit() {
                    if let Some(i) = i {
                        d.coefs[i] |= 1 << al;
                    }
                }
                Ok(())
            },
            |_, _| {},
        )
    }

    /// A progressive AC pass over the luma: the first bits of coefficients
    /// `ss..=se`, or one more bit of each.
    fn progressive_ac(&mut self, bits: &mut Bits<'a>, ss: usize, se: usize, ah: u32, al: u32) -> Result<(), Error> {
        let kk = self.n * self.n;
        let gw = self.frame.as_ref().map_or(0, |f| f.mcux * f.comps[0].h);
        let ac = self.frame.as_ref().map_or(4, |f| f.comps[0].ac);
        self.huffman(ac)?;
        self.blocks(
            bits,
            &[0],
            |d, bits, _, at| {
                let Some((x, y)) = at else { return Ok(()) };
                let b = y * gw + x;
                if ah == 0 {
                    d.ac_first(bits, ac, b * kk, b, ss, se, al)
                } else {
                    d.ac_refine(bits, ac, b * kk, b, ss, se, al)
                }
            },
            |_, _| {},
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn ac_first(&mut self, bits: &mut Bits<'a>, ac: usize, at: usize, b: usize, ss: usize, se: usize, al: u32) -> Result<(), Error> {
        if self.eobrun > 0 {
            self.eobrun -= 1;
            return Ok(());
        }
        let h = self.huff[ac].as_deref().ok_or(bad("missing Huffman table"))?;
        let mut k = ss;
        while k <= se {
            let rs = bits.decode(h)?;
            let (r, s) = ((rs >> 4) as u32, (rs & 15) as u32);
            if s == 0 {
                if r < 15 {
                    self.eobrun = (1 << r) - 1;
                    if r > 0 {
                        self.eobrun += bits.bits(r);
                    }
                    break;
                }
                k += 16;
                continue;
            }
            k += r as usize;
            if k > 63 {
                break;
            }
            let v = bits.receive(s) << al;
            self.nonzero[b] |= 1 << k;
            if self.keep[k] != 0xff {
                self.coefs[at + self.keep[k] as usize] = v as i16;
            }
            k += 1;
        }
        Ok(())
    }

    /// One more bit of the coefficients `ss..=se` of a block: a correction
    /// bit for each that is already nonzero, and the positions and signs of
    /// those that become nonzero now. (As in libjpeg's
    /// `decode_mcu_AC_refine`.)
    #[allow(clippy::too_many_arguments)]
    fn ac_refine(&mut self, bits: &mut Bits<'a>, ac: usize, at: usize, b: usize, ss: usize, se: usize, al: u32) -> Result<(), Error> {
        let p1 = 1i16 << al;
        let h = self.huff[ac].as_deref().ok_or(bad("missing Huffman table"))?;
        let coefs = &mut self.coefs;
        let nonzero = &mut self.nonzero[b];
        let keep = &self.keep;
        // Adds a correction bit to the nonzero coefficient `k`.
        let refine = |bits: &mut Bits<'a>, coefs: &mut Vec<i16>, k: usize| {
            if bits.bit() && keep[k] != 0xff {
                let c = &mut coefs[at + keep[k] as usize];
                if *c & p1 == 0 {
                    *c += if *c >= 0 { p1 } else { -p1 };
                }
            }
        };
        let mut k = ss;
        if self.eobrun == 0 {
            while k <= se {
                let rs = bits.decode(h)?;
                let (mut r, s) = ((rs >> 4) as i32, rs & 15);
                let mut v = 0;
                if s != 0 {
                    v = if bits.bit() { p1 } else { -p1 };
                } else if r != 15 {
                    self.eobrun = 1 << r;
                    if r > 0 {
                        self.eobrun += bits.bits(r as u32);
                    }
                    break;
                }
                // Past the nonzero coefficients, refining them, and `r` zero
                // ones, to the one that becomes nonzero.
                while k <= se {
                    if *nonzero & 1 << k != 0 {
                        refine(bits, coefs, k);
                    } else {
                        r -= 1;
                        if r < 0 {
                            break;
                        }
                    }
                    k += 1;
                }
                if v != 0 && k <= se {
                    *nonzero |= 1 << k;
                    if keep[k] != 0xff {
                        coefs[at + keep[k] as usize] = v;
                    }
                }
                k += 1;
            }
        }
        if self.eobrun > 0 {
            while k <= se {
                if *nonzero & 1 << k != 0 {
                    refine(bits, coefs, k);
                }
                k += 1;
            }
            self.eobrun -= 1;
        }
        Ok(())
    }

    /// Runs the kept coefficients of a progressive picture through the
    /// inverse DCT, a row of blocks at a time.
    fn finish_progressive(&mut self) {
        let Some(f) = self.frame.as_ref() else { return };
        let n = self.n;
        let kk = n * n;
        let gw = f.mcux * f.comps[0].h;
        let q = self.qt[f.comps[0].tq];
        let (bw, bh, out_w, out_h) = (f.bw, f.bh, f.out_w, f.out_h);
        let stride = bw * n;
        let mut rows = vec![255u8; stride * n];
        let mut y = 0;
        for by in 0..bh {
            for bx in 0..bw {
                let at = (by * gw + bx) * kk;
                let mut blk = [0i32; 64];
                for (k, &slot) in self.keep.iter().enumerate() {
                    if slot != 0xff {
                        blk[ZIGZAG[k] as usize] = self.coefs[at + slot as usize] as i32 * q[k] as i32;
                    }
                }
                idct(&blk, n, &self.basis, &mut rows[bx * n..], stride);
            }
            if let Some(s) = self.scaler.as_mut() {
                for r in rows.chunks(stride).take(out_h.saturating_sub(y)) {
                    s.row(&r[..out_w]);
                    y += 1;
                }
            }
        }
    }
}

/// Where the next marker after a scan's data starts: an 0xFF that isn't a
/// stuffed byte, a restart marker or a fill byte.
fn next_marker(data: &[u8], mut pos: usize) -> usize {
    while pos + 1 < data.len() {
        let b = data[pos + 1];
        if data[pos] == 0xff && b != 0 && b != 0xff && !(0xd0..=0xd7).contains(&b) {
            return pos;
        }
        pos += 1;
    }
    data.len()
}

/// The inverse DCT of the top left `n`×`n` coefficients of `blk` (in block
/// order), written as `n`×`n` pixels into `out` with rows `stride` apart.
/// Using only the low frequencies samples the block's picture at `n`×`n`
/// points, so it shrinks the block without aliasing.
fn idct(blk: &[i32; 64], n: usize, basis: &[f32; 64], out: &mut [u8], stride: usize) {
    if blk.iter().skip(1).all(|&c| c == 0) || n == 1 {
        // Flat: the DC alone.
        let v = (blk[0] as f32 / 8.0 + 128.5).clamp(0.0, 255.0) as u8;
        for j in 0..n {
            out[j * stride..j * stride + n].fill(v);
        }
        return;
    }
    // Along each row of frequencies, then down each column.
    let mut tmp = [0f32; 64];
    for v in 0..n {
        let row = &blk[v * 8..v * 8 + n];
        if row.iter().all(|&c| c == 0) {
            continue;
        }
        for i in 0..n {
            let b = &basis[i * n..i * n + n];
            tmp[v * n + i] = row.iter().zip(b).map(|(&c, &b)| c as f32 * b).sum();
        }
    }
    for j in 0..n {
        let b = &basis[j * n..j * n + n];
        for i in 0..n {
            let s: f32 = (0..n).map(|v| b[v] * tmp[v * n + i]).sum();
            out[j * stride + i] = (s + 128.5).clamp(0.0, 255.0) as u8;
        }
    }
}
