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
        let mut pictures = 0;
        for i in 0..book.spine.len() {
            let ch = book.chapter(i).unwrap();
            blocks += ch.blocks.len();
            chars += ch.blocks.iter().map(|b| b.text.chars().count()).sum::<usize>();
            pictures += ch.images.len();
        }
        println!(
            "{}: \"{}\" by {} — {} spine items, {} TOC entries, {} blocks, {} chars, {} pictures",
            path.file_name().unwrap().to_string_lossy(),
            book.title,
            book.author,
            book.spine.len(),
            book.toc.len(),
            blocks,
            chars,
            pictures
        );
        assert!(!book.toc.is_empty());
        assert!(chars > 10_000);
    }
}

/// Decodes every picture in the books, at the size read from its header and
/// at the size a page shows it.
#[test]
fn pictures_decode() {
    use inkreader_core::image;
    use std::time::Instant;

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
        let (mut n, mut secs) = (0, 0.0);
        for i in 0..book.spine.len() {
            for img in book.chapter(i).unwrap().images {
                let data = book.read(&img.path).unwrap();
                assert_eq!(image::size(&data), Some((img.width, img.height)), "{}", img.path);
                let t = Instant::now();
                let g = image::decode(&data, 540, 730).unwrap_or_else(|e| panic!("{}: {e:?}", img.path));
                secs += t.elapsed().as_secs_f64();
                assert!(g.width == 540 || g.height == 730, "{}: {}x{}", img.path, g.width, g.height);
                n += 1;
            }
        }
        println!(
            "{}: {n} pictures decoded in {:.0} ms",
            path.file_name().unwrap().to_string_lossy(),
            secs * 1000.0
        );
    }
}

/// Paginates every chapter and checks that each character is laid out exactly
/// once: nothing dropped or repeated across line and page breaks. Each
/// picture must be shown once, inside the page.
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
                let mut shown = vec![0; ch.images.len()];
                for (n, &start) in starts.iter().enumerate() {
                    let page = layout.page(&ch, start);
                    let want_end = starts.get(n + 1).copied();
                    if let Some(e) = want_end {
                        assert_eq!(page.end, e, "page end differs from pagination");
                    }
                    for item in &page.items {
                        match *item {
                            Item::Text(ref line) => {
                                for r in &line.runs {
                                    for b in r.start..r.end {
                                        seen[line.block as usize][b as usize] += 1;
                                    }
                                }
                            }
                            Item::Image { index, x, y, w, h } => {
                                shown[index as usize] += 1;
                                let inside = x >= layout.left
                                    && x + w <= layout.left + layout.width
                                    && y >= layout.top
                                    && y + h <= layout.top + layout.height;
                                assert!(inside, "{}: picture {index} at {x},{y} {w}x{h}", path.display());
                            }
                            Item::Rule { .. } => {}
                        }
                    }
                }
                assert!(shown.iter().all(|&n| n == 1), "{}: pictures shown {shown:?}", path.display());
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
