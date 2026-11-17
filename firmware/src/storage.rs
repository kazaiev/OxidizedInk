//! The reader core's `Storage` on the SD card: books in the root directory
//! and folders in it, state files in `/READER/`, sleep screen pictures in
//! `/SLEEP/`, font families in folders in `/FONTS/`.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use embedded_sdmmc::ShortFileName;
use esp_hal::time::{Duration, Instant};
use inkreader_core::ui::{Storage, Upload, MAX_FOLDERS, RESERVED_FOLDERS};
use inkreader_core::Error;
use log::{info, warn};

use crate::sd::{Book, Card, Entry, Sd, SdFile};

const STATE_DIR: &str = "READER";
const PICTURE_DIR: &str = "SLEEP";
const FONT_DIR: &str = "FONTS";
/// Larger pictures are skipped: the whole file is read into memory.
const MAX_PICTURE: u32 = 2 * 1024 * 1024;

/// A path in a folder: `name` alone in the root.
fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.into()
    } else {
        format!("{folder}/{name}")
    }
}

/// Whether a directory entry is a book's file, by its extension.
fn is_book(e: &Entry) -> bool {
    !e.dir && [&b"EPU"[..], b"TXT", b"MD"].iter().any(|x| e.short.extension().eq_ignore_ascii_case(x))
}

/// An 8.3 name for an uploaded book or a new folder, from the letters
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

/// A folder's display name inside it, for book folders and font families
/// made by an upload, whose own names are 8.3.
const NAME_FILE: &str = "NAME.TXT";

/// A book from the last listing.
struct CardBook {
    /// Its path as the library shows it: folder and file names joined by
    /// `/`.
    path: String,
    /// Its folder's 8.3 path, `""` for the root.
    dir: String,
    short: ShortFileName,
}

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
    books: Vec<CardBook>,
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

    /// A folder's display name: what its `NAME.TXT` says, for one made by
    /// an upload, or else its long name.
    fn folder_name(&self, path: &str, entry: &Entry) -> String {
        let named = self.sd.read_file(path, NAME_FILE).and_then(|d| String::from_utf8(d).ok());
        named.map(|n| String::from(n.trim())).filter(|n| !n.is_empty()).unwrap_or_else(|| entry.name())
    }

    /// Adds the books among `entries`, the listing of the folder at 8.3
    /// path `dir` that the library calls `shown`, and those in the folders
    /// in it, `depth` folders down.
    fn walk(&self, dir: &str, shown: &str, entries: Vec<Entry>, depth: usize, out: &mut Vec<(CardBook, u32)>) {
        for e in entries {
            let short = format!("{}", e.short);
            if e.dir {
                let reserved = depth == 0 && RESERVED_FOLDERS.iter().any(|r| short.eq_ignore_ascii_case(r));
                if depth >= MAX_FOLDERS || reserved {
                    continue;
                }
                let sub = join(dir, &short);
                let Ok(inside) = self.sd.list(&sub) else { continue };
                let named = inside.iter().any(|x| !x.dir && format!("{}", x.short) == NAME_FILE);
                let name = if named { self.folder_name(&sub, &e) } else { e.name() };
                self.walk(&sub, &join(shown, &name), inside, depth + 1, out);
            } else if is_book(&e) && !(depth > 0 && short == NAME_FILE) {
                out.push((CardBook { path: join(shown, &e.name()), dir: dir.into(), short: e.short }, e.size));
            }
        }
    }

    /// The 8.3 folder path and name of the book at `path` in the last
    /// listing, listing again if it isn't there.
    fn book(&mut self, path: &str) -> Result<(String, ShortFileName), Error> {
        if !self.books.iter().any(|b| b.path == path) {
            self.list_books();
        }
        let b = self.books.iter().find(|b| b.path == path).ok_or(Error::Io)?;
        Ok((b.dir.clone(), b.short.clone()))
    }
}

impl Storage for CardStorage {
    type Book = Book;
    type Upload = CardUpload;

    fn list_books(&mut self) -> Vec<(String, u64)> {
        let t = Instant::now();
        let mut found = Vec::new();
        self.walk("", "", self.sd.list("").unwrap_or_default(), 0, &mut found);
        info!("{} books listed in {} ms", found.len(), t.elapsed().as_millis());
        let listed = found.iter().map(|(b, size)| (b.path.clone(), *size as u64)).collect();
        self.books = found.into_iter().map(|(b, _)| b).collect();
        listed
    }

    fn open_book(&mut self, name: &str) -> Result<Book, Error> {
        let (dir, short) = self.book(name)?;
        self.sd.open(&dir, &short).map(Book).map_err(|_| Error::Io)
    }

    fn create_book(&mut self, name: &str) -> Result<(String, CardUpload), Error> {
        let (folders, name) = name.rsplit_once('/').unwrap_or(("", name));
        // Into each folder: one of that name if there is one, else a new one.
        let (mut dir, mut shown) = (String::new(), String::new());
        for folder in folders.split('/').filter(|f| !f.is_empty()) {
            let entries = self.sd.list(&dir).unwrap_or_default();
            let found = entries.iter().filter(|e| e.dir).find_map(|e| {
                let sub = join(&dir, &format!("{}", e.short));
                let name = self.folder_name(&sub, e);
                name.eq_ignore_ascii_case(folder.trim()).then_some((sub, name))
            });
            let (sub, name) = match found {
                Some(f) => f,
                None => {
                    let taken = |n: &str| {
                        entries.iter().any(|e| format!("{}", e.short).eq_ignore_ascii_case(n))
                            || (dir.is_empty() && RESERVED_FOLDERS.iter().any(|r| r.eq_ignore_ascii_case(n)))
                    };
                    let sub = join(&dir, &short_name(folder, "", "FOLDER", taken));
                    self.sd.write_file(&sub, NAME_FILE, folder.trim().as_bytes()).map_err(|_| Error::Io)?;
                    (sub, String::from(folder.trim()))
                }
            };
            dir = sub;
            shown = join(&shown, &name);
        }
        let taken: Vec<String> = self.sd.list(&dir).unwrap_or_default().iter().map(|e| format!("{}", e.short)).collect();
        // The card keeps the kind of book in the extension: EPU, TXT or MD.
        let lower = name.to_ascii_lowercase();
        let ext = if lower.ends_with(".txt") { "TXT" } else if lower.ends_with(".md") || lower.ends_with(".markdown") { "MD" } else { "EPU" };
        let short = short_name(name, ext, "BOOK", |n| taken.iter().any(|t| t.eq_ignore_ascii_case(n)) || n == NAME_FILE);
        let (file, card) = self.sd.create(&dir, &short).map_err(|_| Error::Io)?;
        card.take_stats();
        Ok((join(&shown, &short), CardUpload { file, card, bytes: 0, busy: Duration::ZERO }))
    }

    fn delete_book(&mut self, name: &str) -> Result<(), Error> {
        let (dir, short) = self.book(name)?;
        self.sd.delete(&dir, &short).map_err(|_| Error::Io)?;
        // Folders it leaves empty go too, but for the name an upload gave
        // them.
        let mut dir = dir.as_str();
        while !dir.is_empty() {
            let entries = self.sd.list(dir).unwrap_or_default();
            if !entries.iter().all(|e| !e.dir && format!("{}", e.short) == NAME_FILE) {
                break;
            }
            if !entries.is_empty() {
                let _ = self.sd.delete(dir, NAME_FILE);
            }
            let (parent, folder) = dir.rsplit_once('/').unwrap_or(("", dir));
            if self.sd.remove_dir(parent, folder).is_err() {
                warn!("can't remove the empty folder {dir}");
                break;
            }
            dir = parent;
        }
        Ok(())
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
            let name = self.folder_name(&path, &folder);
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
        self.sd.write_file(&path, NAME_FILE, family.as_bytes()).map_err(|e| failed("write the name", e))?;
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
            let _ = self.sd.delete(&path, NAME_FILE);
        }
        Ok(())
    }
}
