//! Minimal random-access ZIP reader: reads the central directory and inflates
//! individual entries on demand, so large EPUBs never have to fit in RAM.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::{Error, ReadAt};

const EOCD_SIG: u32 = 0x0605_4b50;
const CDIR_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;

/// Refuse to inflate anything bigger than this (protects the heap).
const MAX_ENTRY_SIZE: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    method: u16,
    compressed_size: u32,
    pub size: u32,
    local_offset: u32,
}

pub struct Zip<F: ReadAt> {
    file: F,
    entries: Vec<Entry>,
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

impl<F: ReadAt> Zip<F> {
    pub fn open(mut file: F) -> Result<Self, Error> {
        let size = file.size();
        if size < 22 {
            return Err(Error::Zip("file too small"));
        }
        // The end-of-central-directory record sits in the last 22..65557 bytes.
        let tail_len = size.min(22 + 0xFFFF) as usize;
        let tail_start = size - tail_len as u64;
        let mut tail = vec![0u8; tail_len];
        file.read_at(tail_start, &mut tail)?;
        let eocd = (0..=tail_len - 22)
            .rev()
            .find(|&i| u32_at(&tail, i) == EOCD_SIG)
            .ok_or(Error::Zip("no end of central directory"))?;
        let count = u16_at(&tail, eocd + 10) as usize;
        let cd_size = u32_at(&tail, eocd + 12) as usize;
        let cd_offset = u32_at(&tail, eocd + 16) as u64;
        drop(tail);

        let mut cd = vec![0u8; cd_size];
        file.read_at(cd_offset, &mut cd)?;
        let mut entries = Vec::with_capacity(count);
        let mut p = 0;
        while p + 46 <= cd.len() && u32_at(&cd, p) == CDIR_SIG {
            let name_len = u16_at(&cd, p + 28) as usize;
            let extra_len = u16_at(&cd, p + 30) as usize;
            let comment_len = u16_at(&cd, p + 32) as usize;
            let name_end = (p + 46 + name_len).min(cd.len());
            entries.push(Entry {
                name: String::from_utf8_lossy(&cd[p + 46..name_end]).into_owned(),
                method: u16_at(&cd, p + 10),
                compressed_size: u32_at(&cd, p + 20),
                size: u32_at(&cd, p + 24),
                local_offset: u32_at(&cd, p + 42),
            });
            p += 46 + name_len + extra_len + comment_len;
        }
        Ok(Zip { file, entries })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn find(&self, name: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|e| e.name == name)
            .or_else(|| self.entries.iter().find(|e| e.name.eq_ignore_ascii_case(name)))
    }

    pub fn read(&mut self, name: &str) -> Result<Vec<u8>, Error> {
        let entry = self.find(name).cloned().ok_or(Error::Zip("entry not found"))?;
        self.read_entry(&entry)
    }

    pub fn read_entry(&mut self, e: &Entry) -> Result<Vec<u8>, Error> {
        if e.size as usize > MAX_ENTRY_SIZE {
            return Err(Error::Zip("entry too large"));
        }
        let mut hdr = [0u8; 30];
        self.file.read_at(e.local_offset as u64, &mut hdr)?;
        if u32_at(&hdr, 0) != LOCAL_SIG {
            return Err(Error::Zip("bad local header"));
        }
        let data_off =
            e.local_offset as u64 + 30 + u16_at(&hdr, 26) as u64 + u16_at(&hdr, 28) as u64;
        let mut data = vec![0u8; e.compressed_size as usize];
        self.file.read_at(data_off, &mut data)?;
        match e.method {
            0 => Ok(data),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(&data, MAX_ENTRY_SIZE)
                .map_err(|_| Error::Zip("inflate failed")),
            _ => Err(Error::Zip("unsupported compression")),
        }
    }
}
