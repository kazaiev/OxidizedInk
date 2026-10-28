//! The reader core's `Storage` on the SD card: books in the root directory,
//! state files in `/READER/`.

use alloc::string::String;
use alloc::vec::Vec;

use embedded_sdmmc::ShortFileName;
use inkreader_core::ui::Storage;
use inkreader_core::Error;

use crate::sd::{Book, Sd};

const STATE_DIR: &str = "READER";

pub struct CardStorage {
    sd: Sd,
    /// Each book's display name (its long file name) and 8.3 name, from the
    /// last listing.
    books: Vec<(String, ShortFileName)>,
}

impl CardStorage {
    pub fn new(sd: Sd) -> Self {
        CardStorage { sd, books: Vec::new() }
    }
}

impl Storage for CardStorage {
    type Book = Book;

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

    fn load(&mut self, name: &str) -> Option<Vec<u8>> {
        self.sd.read_file(STATE_DIR, name)
    }

    fn save(&mut self, name: &str, data: &[u8]) -> Result<(), Error> {
        self.sd.write_file(STATE_DIR, name, data).map_err(|_| Error::Io)
    }
}
