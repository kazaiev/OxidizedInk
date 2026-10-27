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

/// Paginates every chapter and checks that each character is laid out exactly
/// once: nothing dropped or repeated across line and page breaks.
#[test]
fn layout_covers_all_text() {
    use inkreader_core::fonts::Fonts;
    use inkreader_core::layout::{Item, Layout};
    use inkreader_core::text::BlockKind;
    use std::time::Instant;

    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../testbooks");
    let Ok(entries) = fs::read_dir(dir) else {
        eprintln!("no testbooks/ directory, skipping");
        return;
    };
    let fonts = Fonts::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("epub") {
            continue;
        }
        let file = fs::File::open(&path).unwrap();
        let len = file.metadata().unwrap().len();
        let mut book = Epub::open(FileReader(file, len)).unwrap();
        for font_px in [18, 26, 40] {
            // The large size also covers ragged-right, spaced paragraphs.
            let plain = font_px == 40;
            let layout = Layout {
                fonts: &fonts,
                left: 30,
                top: 20,
                width: 540,
                height: 730,
                font_px,
                line_pct: 140,
                justify: !plain,
                spaced: plain,
            };
            let mut pages = 0;
            let mut measure = 0.0;
            for i in 0..book.spine.len() {
                let mut ch = book.chapter(i).unwrap();
                if font_px == 26 {
                    // Also check forced page breaks, as before TOC entries.
                    ch.breaks = (0..ch.blocks.len())
                        .filter(|&b| matches!(ch.blocks[b].kind, BlockKind::Heading(_)))
                        .collect();
                }
                let t = Instant::now();
                let starts = layout.paginate(&ch);
                measure += t.elapsed().as_secs_f64();
                pages += starts.len();
                let mut seen: Vec<Vec<u8>> = ch.blocks.iter().map(|b| vec![0; b.text.len()]).collect();
                for (n, &start) in starts.iter().enumerate() {
                    let page = layout.page(&ch, start);
                    let want_end = starts.get(n + 1).copied();
                    if let Some(e) = want_end {
                        assert_eq!(page.end, e, "page end differs from pagination");
                    }
                    for item in &page.items {
                        if let Item::Text(line) = item {
                            for r in &line.runs {
                                for b in r.start..r.end {
                                    seen[line.block as usize][b as usize] += 1;
                                }
                            }
                        }
                    }
                }
                for (bi, b) in ch.blocks.iter().enumerate() {
                    for (j, c) in b.text.char_indices() {
                        let n = seen[bi][j];
                        let skippable = matches!(c, ' ' | '\n' | '\u{ad}' | '\u{200b}');
                        assert!(
                            n == 1 || (n == 0 && skippable),
                            "{}: chapter {i} block {bi} byte {j} {c:?} laid out {n} times",
                            path.display()
                        );
                    }
                }
            }
            println!(
                "{} at {font_px}px: {pages} pages, paginated in {:.0} ms",
                path.file_name().unwrap().to_string_lossy(),
                measure * 1000.0
            );
        }
    }
}
