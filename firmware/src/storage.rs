//! The reader core's `Storage` on the SD card: books in the root directory,
//! state files in `/READER/`, sleep screen pictures in `/SLEEP/`, font
//! families in folders in `/FONTS/`.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use embedded_sdmmc::ShortFileName;
use esp_hal::time::{Duration, Instant};
use inkreader_core::ui::{Storage, Upload};
use inkreader_core::Error;
use log::{info, warn};

use crate::sd::{Book, Card, Sd, SdFile};

const STATE_DIR: &str = "READER";
const PICTURE_DIR: &str = "SLEEP";
const FONT_DIR: &str = "FONTS";
/// Larger pictures are skipped: the whole file is read into memory.
const MAX_PICTURE: u32 = 2 * 1024 * 1024;

/// An 8.3 name for an uploaded book or a new font folder, from the letters
/// and digits of `name` (without its extension), with extension `ext` if
/// it isn't empty: `PRIDEAND.EPU`, or `PRIDEA~2.EPU` and so on if that's
/// taken. embedded-sdmmc can't make long names.
fn short_name(name: &str, ext: &str, fallback: &str, taken: impl Fn(&str) -> bool) -> String {
    let stem = if ext.is_empty() { name } else { name.rsplit_once('.').map_or(name, |(s, _)| s) };
    let mut base: String = stem.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
    base.make_ascii_uppercase();
    if base.is_empty() {
        base = fallback.into();
    }
    let dot = if ext.is_empty() { String::new() } else { format!(".{ext}") };
    (1..)
        .map(|n| match n {
            1 => format!("{base}{dot}"),
            n => {
                let tail = format!("~{}", n);
                format!("{}{}{}", &base[..base.len().min(8 - tail.len())], tail, dot)
            }
        })
        .find(|n| !taken(n))
        .unwrap()
}

/// A family's display name inside its folder, for folders made by an
/// upload, whose own names are 8.3.
const FONT_NAME_FILE: &str = "NAME.TXT";

/// A font folder from the last listing.
struct FontFolder {
    /// Its display name: from `NAME.TXT`, or the folder's long name.
    name: String,
    /// Its 8.3 name.
    dir: String,
    /// Its font files: display name and 8.3 name.
    files: Vec<(String, ShortFileName)>,
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
    fonts: Vec<FontFolder>,
}

impl CardStorage {
    pub fn new(sd: Sd) -> Self {
        CardStorage { sd, books: Vec::new(), pictures: Vec::new(), fonts: Vec::new() }
    }

    pub fn sd(&self) -> &Sd {
        &self.sd
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
        let short = short_name(name, "EPU", "BOOK", |n| taken.iter().any(|t| t.eq_ignore_ascii_case(n)));
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

    fn list_fonts(&mut self) -> Vec<(String, Vec<(String, u64)>)> {
        let font = |ext: &[u8]| ext.eq_ignore_ascii_case(b"TTF") || ext.eq_ignore_ascii_case(b"OTF");
        let mut out = Vec::new();
        self.fonts.clear();
        for folder in self.sd.list(FONT_DIR).unwrap_or_default().into_iter().filter(|e| e.dir) {
            let dir = format!("{}", folder.short);
            let path = format!("{FONT_DIR}/{dir}");
            let files: Vec<_> = self.sd.list(&path).unwrap_or_default();
            let files: Vec<_> = files.into_iter().filter(|e| !e.dir && font(e.short.extension())).collect();
            let named = self.sd.read_file(&path, FONT_NAME_FILE).and_then(|d| String::from_utf8(d).ok());
            let name = named.map(|n| String::from(n.trim())).filter(|n| !n.is_empty()).unwrap_or_else(|| folder.name());
            // Empty folders aren't families, but an upload uses its own again.
            if !files.is_empty() {
                out.push((name.clone(), files.iter().map(|e| (e.name(), e.size as u64)).collect()));
            }
            self.fonts.push(FontFolder { name, dir, files: files.iter().map(|e| (e.name(), e.short)).collect() });
        }
        out.sort_by(|a: &(String, Vec<_>), b| a.0.cmp(&b.0));
        out
    }

    fn load_font(&mut self, family: &str, file: &str) -> Option<Vec<u8>> {
        let known = |fonts: &[FontFolder]| fonts.iter().any(|f| f.name == family && f.files.iter().any(|(n, _)| n == file));
        if !known(&self.fonts) {
            self.list_fonts();
        }
        let folder = self.fonts.iter().find(|f| f.name == family)?;
        let (_, short) = folder.files.iter().find(|(n, _)| n == file)?;
        let t = Instant::now();
        let data = self.sd.read_file(&format!("{FONT_DIR}/{}", folder.dir), short);
        let (kb, ms) = (data.as_ref().map_or(0, |d| d.len() / 1024), t.elapsed().as_millis().max(1));
        info!("font {family} {file}: {kb} KB read in {ms} ms ({} KB/s)", kb as u64 * 1000 / ms);
        data
    }

    fn create_firmware(&mut self) -> Result<CardUpload, Error> {
        let _ = self.sd.delete("", crate::update::FILE);
        let (file, card) = self.sd.create("", crate::update::FILE).map_err(|_| Error::Io)?;
        card.take_stats();
        Ok(CardUpload { file, card, bytes: 0, busy: Duration::ZERO })
    }

    fn read_firmware(&mut self, f: &mut dyn FnMut(&[u8])) -> Result<(), Error> {
        crate::update::read_file(&self.sd, &mut |_, piece| {
            f(piece);
            Ok(())
        })
        .map_err(|_| Error::Io)
    }

    fn delete_firmware(&mut self) {
        let _ = self.sd.delete("", crate::update::FILE);
    }

    fn create_font(&mut self, family: &str, file: &str) -> Result<CardUpload, Error> {
        self.list_fonts();
        let dir = match self.fonts.iter().find(|f| f.name == family) {
            Some(f) => f.dir.clone(),
            None => {
                let taken: Vec<String> = self.sd.list(FONT_DIR).unwrap_or_default().iter().map(|e| format!("{}", e.short)).collect();
                short_name(family, "", "FONT", |n| taken.iter().any(|t| t.eq_ignore_ascii_case(n)))
            }
        };
        let path = format!("{FONT_DIR}/{dir}");
        let failed = |what: &str, e| {
            warn!("can't {what} in {path}: {e:?}");
            Error::Io
        };
        self.sd.write_file(&path, FONT_NAME_FILE, family.as_bytes()).map_err(|e| failed("write the name", e))?;
        let _ = self.sd.delete(&path, file);
        let (file, card) = self.sd.create(&path, file).map_err(|e| failed("create the file", e))?;
        card.take_stats();
        Ok(CardUpload { file, card, bytes: 0, busy: Duration::ZERO })
    }

    fn delete_font(&mut self, family: &str, file: Option<&str>) -> Result<(), Error> {
        self.list_fonts();
        let folder = self.fonts.iter().find(|f| f.name == family).ok_or(Error::Io)?;
        let path = format!("{FONT_DIR}/{}", folder.dir);
        for (_, short) in folder.files.iter().filter(|(n, _)| file.is_none_or(|f| f == n)) {
            self.sd.delete(&path, *short).map_err(|_| Error::Io)?;
        }
        if file.is_none() {
            let _ = self.sd.delete(&path, FONT_NAME_FILE);
        }
        Ok(())
    }
}
