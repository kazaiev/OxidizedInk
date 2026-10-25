//! Parses every EPUB in `testbooks/` (not committed; see README) end to end.

use std::fs;
use std::io::{Read, Seek, SeekFrom};

use inkreader_core::epub::Epub;
use inkreader_core::{Error, ReadAt};

struct FileReader(fs::File, u64);

impl ReadAt for FileReader {
    fn size(&self) -> u64 {
        self.1
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.0.seek(SeekFrom::Start(offset)).map_err(|_| Error::Io)?;
        self.0.read_exact(buf).map_err(|_| Error::Io)
    }
}

#[test]
fn parse_test_books() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../testbooks");
    let Ok(entries) = fs::read_dir(dir) else {
        eprintln!("no testbooks/ directory, skipping");
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("epub") {
            continue;
        }
        let file = fs::File::open(&path).unwrap();
        let len = file.metadata().unwrap().len();
        let mut book = Epub::open(FileReader(file, len)).unwrap();
        let mut blocks = 0;
        let mut chars = 0;
        for i in 0..book.spine.len() {
            let ch = book.chapter(i).unwrap();
            blocks += ch.blocks.len();
            chars += ch.blocks.iter().map(|b| b.text.chars().count()).sum::<usize>();
        }
        println!(
            "{}: \"{}\" by {} — {} spine items, {} TOC entries, {} blocks, {} chars",
            path.file_name().unwrap().to_string_lossy(),
            book.title,
            book.author,
            book.spine.len(),
            book.toc.len(),
            blocks,
            chars
        );
        assert!(!book.toc.is_empty());
        assert!(chars > 10_000);
    }
}
