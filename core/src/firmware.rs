//! Checking a firmware image for the reader before it's installed: the ESP32
//! app image format (as `espflash save-image` writes it), fed a piece at a
//! time, so a 2 MB image never sits in memory.
//!
//! An image is a 24-byte header (magic 0xE9, segment count, and the chip it
//! is for), its segments (load address, length, data), padding to a 16-byte
//! boundary less one, a checksum byte (0xEF XOR every segment data byte),
//! and, if the header says so, the SHA-256 of everything before it.

use alloc::vec::Vec;
use sha2::{Digest, Sha256};

const MAGIC: u8 = 0xe9;
const HEADER: usize = 24;
const HASH: usize = 32;
/// The chip id an image for the ESP32 has in its header.
const CHIP_ESP32: u16 = 0;

/// Feeds an image through; `finish` says whether it's whole and sound.
pub struct ImageCheck {
    sha: Sha256,
    /// Bytes not hashed yet: the last 32 may be the hash itself.
    tail: Vec<u8>,
    pos: u64,
    header: Vec<u8>,
    hash_appended: bool,
    segments_left: u8,
    /// A segment header being read, and the data left in the segment.
    segment: Vec<u8>,
    data_left: u32,
    xor: u8,
    /// Where the checksum byte is, once the segments are read, and what it says.
    checksum_at: Option<u64>,
    checksum: Option<u8>,
    error: Option<&'static str>,
}

impl Default for ImageCheck {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageCheck {
    pub fn new() -> Self {
        ImageCheck {
            sha: Sha256::new(),
            tail: Vec::new(),
            pos: 0,
            header: Vec::with_capacity(HEADER),
            hash_appended: false,
            segments_left: 0,
            segment: Vec::with_capacity(8),
            data_left: 0,
            xor: 0xef,
            checksum_at: None,
            checksum: None,
            error: None,
        }
    }

    /// Bytes fed so far.
    pub fn len(&self) -> u64 {
        self.pos
    }

    pub fn is_empty(&self) -> bool {
        self.pos == 0
    }

    pub fn feed(&mut self, data: &[u8]) {
        if self.error.is_some() {
            return;
        }
        for &b in data {
            self.byte(b);
        }
        // Hash all but the last 32 bytes, which may be the hash.
        self.tail.extend_from_slice(data);
        let keep = if self.hash_appended || self.header.len() < HEADER { HASH } else { 0 };
        if self.tail.len() > keep {
            let n = self.tail.len() - keep;
            self.sha.update(&self.tail[..n]);
            self.tail.drain(..n);
        }
    }

    fn byte(&mut self, b: u8) {
        let p = self.pos;
        self.pos += 1;
        if self.header.len() < HEADER {
            self.header.push(b);
            if self.header.len() == HEADER {
                self.read_header();
            }
            return;
        }
        match self.checksum_at {
            None if self.data_left > 0 => {
                self.xor ^= b;
                self.data_left -= 1;
                if self.data_left == 0 && self.segments_left == 0 {
                    self.segments_done(p + 1);
                }
            }
            None => {
                self.segment.push(b);
                if self.segment.len() == 8 {
                    self.data_left = u32::from_le_bytes([self.segment[4], self.segment[5], self.segment[6], self.segment[7]]);
                    self.segment.clear();
                    self.segments_left -= 1;
                    if self.data_left == 0 && self.segments_left == 0 {
                        self.segments_done(p + 1);
                    }
                }
            }
            Some(at) if p < at => {}
            Some(at) if p == at => self.checksum = Some(b),
            Some(at) if self.hash_appended && p < at + 1 + HASH as u64 => {}
            Some(_) => self.error = Some("there's more after the end of the image"),
        }
    }

    fn read_header(&mut self) {
        let h = &self.header;
        let chip = u16::from_le_bytes([h[12], h[13]]);
        self.error = if h[0] != MAGIC {
            Some("this isn't a firmware image")
        } else if chip != CHIP_ESP32 {
            Some("this firmware is for another chip")
        } else if h[1] == 0 || h[1] > 16 {
            Some("the image's header is damaged")
        } else {
            None
        };
        self.segments_left = h[1];
        self.hash_appended = h[23] == 1;
    }

    /// The segments end at `end`: the checksum byte is where a 16-byte
    /// boundary is one byte away.
    fn segments_done(&mut self, end: u64) {
        self.checksum_at = Some(end + (15 - end % 16));
    }

    /// Whether the image fed in is a whole, sound image for this chip.
    pub fn finish(mut self) -> Result<(), &'static str> {
        if let Some(e) = self.error {
            return Err(e);
        }
        let (Some(at), Some(checksum)) = (self.checksum_at, self.checksum) else {
            return Err("the image is cut short");
        };
        if checksum != self.xor {
            return Err("the image's checksum is wrong");
        }
        if self.hash_appended {
            if self.pos != at + 1 + HASH as u64 {
                return Err("the image is cut short");
            }
            if self.sha.finalize()[..] != self.tail[..] {
                return Err("the image's hash is wrong");
            }
        } else {
            self.sha.update(&self.tail);
            self.tail.clear();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// An image as `espflash save-image` lays it out.
    fn image(segments: &[&[u8]], hash: bool) -> Vec<u8> {
        let mut out = vec![MAGIC, segments.len() as u8, 2, 0x20, 0x00, 0x00, 0x08, 0x40];
        out.extend_from_slice(&[0xee, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, hash as u8]);
        let mut xor = 0xef;
        for (i, s) in segments.iter().enumerate() {
            out.extend_from_slice(&(0x3f40_0000u32 + i as u32 * 0x1000).to_le_bytes());
            out.extend_from_slice(&(s.len() as u32).to_le_bytes());
            out.extend_from_slice(s);
            xor = s.iter().fold(xor, |x, b| x ^ b);
        }
        while out.len() % 16 != 15 {
            out.push(0);
        }
        out.push(xor);
        if hash {
            let digest = Sha256::digest(&out);
            out.extend_from_slice(&digest);
        }
        out
    }

    fn check(data: &[u8], piece: usize) -> Result<(), &'static str> {
        let mut c = ImageCheck::new();
        for p in data.chunks(piece) {
            c.feed(p);
        }
        c.finish()
    }

    #[test]
    fn sound_images_pass_in_any_pieces() {
        let data: Vec<u8> = (0..5000u32).map(|i| (i * 7) as u8).collect();
        for hash in [true, false] {
            let img = image(&[&data[..1000], &data[1000..1003], &data[1003..]], hash);
            for piece in [1, 7, 512, 4096, img.len()] {
                assert_eq!(check(&img, piece), Ok(()), "hash {hash}, pieces of {piece}");
            }
        }
    }

    #[test]
    fn damage_is_caught() {
        let data: Vec<u8> = (0..3000u32).map(|i| (i * 13) as u8).collect();
        let img = image(&[&data[..2000], &data[2000..]], true);
        let mut flipped = img.clone();
        flipped[500] ^= 1;
        assert!(check(&flipped, 512).is_err());
        assert_eq!(check(&img[..img.len() - 10], 512), Err("the image is cut short"));
        let mut longer = img.clone();
        longer.push(0);
        assert!(check(&longer, 512).is_err());
        let mut other_chip = img.clone();
        other_chip[12] = 5;
        assert_eq!(check(&other_chip, 512), Err("this firmware is for another chip"));
        assert_eq!(check(b"PK\x03\x04 not firmware at all, but long enough", 512), Err("this isn't a firmware image"));
    }

    /// Checks a real image, when `FIRMWARE_IMAGE` names one (made with
    /// `espflash save-image --chip esp32`).
    #[test]
    fn real_image() {
        let Ok(path) = std::env::var("FIRMWARE_IMAGE") else { return };
        let data = std::fs::read(path).unwrap();
        assert_eq!(check(&data, 8192), Ok(()));
        let mut damaged = data.clone();
        damaged[data.len() / 2] ^= 0x10;
        assert!(check(&damaged, 8192).is_err());
    }
}
