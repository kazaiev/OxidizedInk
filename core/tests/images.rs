//! Checks the JPEG and PNG decoders against the pictures in `tests/images/`
//! (made by `make.py` there): JPEGs against libjpeg-turbo's output, PNGs
//! against the `png` crate's.

use std::fs;
use std::io::Cursor;

use inkreader_core::image::{decode, Gray};

const W: usize = 203;
const H: usize = 157;

fn file(name: &str) -> Vec<u8> {
    fs::read(format!("{}/tests/images/{}", env!("CARGO_MANIFEST_DIR"), name)).unwrap()
}

/// A binary PGM's pixels.
fn pgm(name: &str) -> Vec<u8> {
    let data = file(name);
    let header = format!("P5\n{} {}\n255\n", W, H);
    assert!(data.starts_with(header.as_bytes()), "{name}: unexpected PGM header");
    data[header.len()..].to_vec()
}

/// Mean and largest absolute difference.
fn diff(a: &[u8], b: &[u8]) -> (f64, u8) {
    assert_eq!(a.len(), b.len());
    let sum: u64 = a.iter().zip(b).map(|(&x, &y)| x.abs_diff(y) as u64).sum();
    let max = a.iter().zip(b).map(|(&x, &y)| x.abs_diff(y)).max().unwrap_or(0);
    (sum as f64 / a.len() as f64, max)
}

fn jpeg(name: &str, max_w: i32, max_h: i32) -> Gray {
    decode(&file(name), max_w, max_h).unwrap_or_else(|e| panic!("{name}: {e:?}"))
}

/// `pixels` (`W`×`H`) averaged over `f`×`f` cells, partial ones at the edges.
fn box_average(pixels: &[u8], f: usize) -> Vec<u8> {
    let (w, h) = (W.div_ceil(f), H.div_ceil(f));
    let mut out = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let (mut sum, mut n) = (0u32, 0u32);
            for yy in y * f..((y + 1) * f).min(H) {
                for xx in x * f..((x + 1) * f).min(W) {
                    sum += pixels[yy * W + xx] as u32;
                    n += 1;
                }
            }
            out.push(((sum + n / 2) / n) as u8);
        }
    }
    out
}

#[test]
fn jpeg_matches_libjpeg() {
    for name in ["base420", "base422", "base444", "gray"] {
        let g = jpeg(&format!("{name}.jpg"), W as i32, H as i32);
        assert_eq!((g.width, g.height), (W as i32, H as i32));
        let (mean, max) = diff(&g.pixels, &pgm(&format!("{name}.pgm")));
        assert!(mean < 0.5 && max <= 3, "{name}: mean {mean:.2}, max {max}");
    }
}

#[test]
fn restart_markers() {
    for (name, reference) in [("restart", "base420"), ("progrestart", "base444")] {
        let g = jpeg(&format!("{name}.jpg"), W as i32, H as i32);
        let (mean, max) = diff(&g.pixels, &pgm(&format!("{reference}.pgm")));
        assert!(mean < 0.5 && max <= 3, "{name}: mean {mean:.2}, max {max}");
    }
}

#[test]
fn progressive_matches_baseline() {
    // jpegtran keeps the coefficients, so the pixels must be the same.
    for (prog, base) in [("prog420", "base420"), ("proggray", "gray")] {
        for (w, h) in [(W as i32, H as i32), (102, 79), (51, 40), (26, 20), (300, 300)] {
            let (p, b) = (jpeg(&format!("{prog}.jpg"), w, h), jpeg(&format!("{base}.jpg"), w, h));
            assert_eq!((p.width, p.height), (b.width, b.height));
            assert!(p.pixels == b.pixels, "{prog} at {w}x{h} differs from {base}");
        }
    }
}

#[test]
fn jpeg_shrinks_in_the_dct() {
    // At 1/2 and 1/8 the blocks shrink to 4×4 and 1×1, close to averaging
    // the full-size pixels. (At 1/4 the picture's shape gives a size that
    // the scaler changes again.)
    let full = pgm("base444.pgm");
    for (f, limit) in [(2, 2.0), (8, 1.0)] {
        let (w, h) = (W.div_ceil(f), H.div_ceil(f));
        let g = jpeg("base444.jpg", w as i32, h as i32);
        assert_eq!((g.width as usize, g.height as usize), (w, h));
        let (mean, _) = diff(&g.pixels, &box_average(&full, f));
        assert!(mean < limit, "1/{f}: mean {mean:.2}");
    }
}

#[test]
fn unsupported_jpeg_is_an_error() {
    assert!(decode(&file("arith.jpg"), 100, 100).is_err());
    assert!(decode(&file("base420.jpg")[..2000], 100, 100).is_ok_and(|g| g.width > 0));
    assert!(decode(b"not a picture", 100, 100).is_err());
}

/// What the PNG decoder should give: the `png` crate's 8-bit output, with
/// the same luma and laid over white.
fn png_reference(data: &[u8]) -> (u32, u32, Vec<u8>) {
    let mut d = png::Decoder::new(Cursor::new(data));
    d.set_transformations(png::Transformations::normalize_to_color8());
    let mut r = d.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    let over_white = |g: u32, a: u32| ((g * a + 255 * (255 - a) + 127) / 255) as u8;
    let luma = |p: &[u8]| (77 * p[0] as u32 + 150 * p[1] as u32 + 29 * p[2] as u32 + 128) >> 8;
    let px = &buf[..info.buffer_size()];
    let gray = match info.color_type {
        png::ColorType::Grayscale => px.to_vec(),
        png::ColorType::GrayscaleAlpha => px.chunks(2).map(|p| over_white(p[0] as u32, p[1] as u32)).collect(),
        png::ColorType::Rgb => px.chunks(3).map(|p| luma(p) as u8).collect(),
        png::ColorType::Rgba => px.chunks(4).map(|p| over_white(luma(p), p[3] as u32)).collect(),
        t => panic!("unexpected {t:?}"),
    };
    (info.width, info.height, gray)
}

#[test]
fn png_matches_reference() {
    let names = [
        "gray1", "gray2", "gray4", "gray8", "gray16", "gray8key", "rgb8", "rgb16", "graya8", "rgba8", "rgba16", "pal4",
        "rgb8i", "gray2i",
    ];
    for name in names {
        let data = file(&format!("{name}.png"));
        let (w, h, reference) = png_reference(&data);
        let g = decode(&data, w as i32, h as i32).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!((g.width as u32, g.height as u32), (w, h), "{name}");
        let (_, max) = diff(&g.pixels, &reference);
        assert!(max <= 1, "{name}: off by {max}");
    }
}

#[test]
fn png_scales() {
    let g = decode(&file("rgb8.png"), 600, 800).unwrap();
    assert_eq!((g.width, g.height), (600, 462));
    let g = decode(&file("rgb8i.png"), 20, 20).unwrap();
    assert_eq!((g.width, g.height), (20, 15));
}
