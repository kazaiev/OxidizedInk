//! The reader core's `Storage` on the SD card: books in the root directory,
//! state files in `/READER/`, sleep screen pictures in `/SLEEP/`.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use embedded_sdmmc::ShortFileName;
use esp_hal::time::{Duration, Instant};
use inkreader_core::ui::{Storage, Upload};
use inkreader_core::Error;
use log::info;

use crate::sd::{Book, Card, Sd, SdFile};

const STATE_DIR: &str = "READER";
const PICTURE_DIR: &str = "SLEEP";
/// Larger pictures are skipped: the whole file is read into memory.
const MAX_PICTURE: u32 = 2 * 1024 * 1024;

/// An 8.3 name for an uploaded book, from the letters and digits of its
/// name: `PRIDEAND.EPU`, or `PRIDEA~2.EPU` and so on if that's taken.
/// embedded-sdmmc can't make long names.
fn short_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let mut base: String = stem.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
    base.make_ascii_uppercase();
    if base.is_empty() {
        base = "BOOK".into();
    }
    (1..)
        .map(|n| match n {
            1 => format!("{}.EPU", base),
            n => {
                let tail = format!("~{}", n);
                format!("{}{}.EPU", &base[..base.len().min(8 - tail.len())], tail)
            }
        })
        .find(|n| !taken(n))
        .unwrap()
}

/// A book being uploaded to the card.
pub struct CardUpload {
    file: SdFile,
    card: Card,
    bytes: usize,
    /// Time spent writing to the card.
    busy: Duration,
}

impl Upload for CardUpload {
    fn write(&mut self, data: &[u8]) -> Result<(), Error> {
        let t = Instant::now();
        self.file.write(data).map_err(|_| Error::Io)?;
        self.busy += t.elapsed();
        self.bytes += data.len();
        Ok(())
    }

    fn finish(self) -> Result<(), Error> {
        let t = Instant::now();
        self.file.close().map_err(|_| Error::Io)?;
        self.card.flush().map_err(|_| Error::Io)?;
        let ms = (self.busy + t.elapsed()).as_millis();
        info!("upload: {} KB written to the card in {} ms: {}", self.bytes / 1024, ms, self.card.take_stats());
        Ok(())
    }
}

pub struct CardStorage {
    sd: Sd,
    /// Each book's display name (its long file name) and 8.3 name, from the
    /// last listing.
    books: Vec<(String, ShortFileName)>,
    /// The same for the pictures.
    pictures: Vec<(String, ShortFileName)>,
}

impl CardStorage {
    pub fn new(sd: Sd) -> Self {
        CardStorage { sd, books: Vec::new(), pictures: Vec::new() }
    }
}

impl Storage for CardStorage {
    type Book = Book;
    type Upload = CardUpload;

    fn list_books(&mut self) -> Vec<(String, u64)> {
        let entries = self.sd.list("").unwrap_or_default();
        let books: Vec<_> = entries
            .into_iter()
            .filter(|e| !e.dir && e.short.extension().eq_ignore_ascii_case(b"EPU"))
            .collect();
        self.books = books.iter().map(|e| (e.name(), e.short.clone())).collect();
        books.iter().map(|e| (e.name(), e.size as u64)).collect()
    }

    fn open_book(&mut self, name: &str) -> Result<Book, Error> {
        if !self.books.iter().any(|(n, _)| n == name) {
            self.list_books();
        }
        let (_, short) = self.books.iter().find(|(n, _)| n == name).ok_or(Error::Io)?;
        self.sd.open("", short).map(Book).map_err(|_| Error::Io)
    }

    fn create_book(&mut self, name: &str) -> Result<(String, CardUpload), Error> {
        let taken: Vec<String> = self.sd.list("").unwrap_or_default().iter().map(|e| format!("{}", e.short)).collect();
        let short = short_name(name, |n| taken.iter().any(|t| t.eq_ignore_ascii_case(n)));
        let (file, card) = self.sd.create("", &short).map_err(|_| Error::Io)?;
        card.take_stats();
        Ok((short, CardUpload { file, card, bytes: 0, busy: Duration::ZERO }))
    }

    fn delete_book(&mut self, name: &str) -> Result<(), Error> {
        if !self.books.iter().any(|(n, _)| n == name) {
            self.list_books();
        }
        let (_, short) = self.books.iter().find(|(n, _)| n == name).ok_or(Error::Io)?;
        self.sd.delete("", short).map_err(|_| Error::Io)
    }

    fn load(&mut self, name: &str) -> Option<Vec<u8>> {
        self.sd.read_file(STATE_DIR, name)
    }

    fn save(&mut self, name: &str, data: &[u8]) -> Result<(), Error> {
        self.sd.write_file(STATE_DIR, name, data).map_err(|_| Error::Io)
    }

    fn remove(&mut self, name: &str) {
        let _ = self.sd.delete(STATE_DIR, name);
    }

    fn list_pictures(&mut self) -> Vec<String> {
        let entries = self.sd.list(PICTURE_DIR).unwrap_or_default();
        let picture = |ext: &[u8]| [b"JPG", b"JPE", b"PNG"].iter().any(|x| ext.eq_ignore_ascii_case(*x));
        self.pictures = entries
            .into_iter()
            .filter(|e| !e.dir && e.size <= MAX_PICTURE && picture(e.short.extension()))
            .map(|e| (e.name(), e.short))
            .collect();
        self.pictures.sort_by(|a, b| a.0.cmp(&b.0));
        self.pictures.iter().map(|(n, _)| n.clone()).collect()
    }

    fn load_picture(&mut self, name: &str) -> Option<Vec<u8>> {
        let (_, short) = self.pictures.iter().find(|(n, _)| n == name)?;
        self.sd.read_file(PICTURE_DIR, short)
    }
}
