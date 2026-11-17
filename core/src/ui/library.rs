//! The library: the books on the card, in their folders, in the order the
//! Library order setting picks; and deleting a book.
//!
//! A book's `file` is its path: folder and file names joined by `/`, as
//! `Storage::list_books` gives them. The screen shows one folder at a time:
//! the folders in it, then its books, and at the top level the `TOOLS`.
//!
//! `LIBRARY.TXT` caches each book's title and author, so only new or
//! changed books are opened when the library is listed. It also keeps when
//! each book was added and last opened, as steps of a counter, since the
//! reader has no clock. A line is `file`, `size`, `title`, `author`,
//! `added` and `opened`, tab-separated.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Reverse;

use super::*;
use crate::settings::LibraryOrder;

/// Folders the library looks into, below the top level.
pub const MAX_FOLDERS: usize = 3;
/// Top-level folders that hold the reader's own files, not books.
pub const RESERVED_FOLDERS: [&str; 3] = ["READER", "SLEEP", "FONTS"];

/// A row of the library screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Row {
    /// A folder in the one shown, by name, with how many books it holds,
    /// counting those in folders inside it.
    Folder(String, usize),
    /// A book, by its index in `App::library`.
    Book(usize),
    /// One of `TOOLS`, at the top level only.
    Tool(usize),
}

/// The folder holding `file`: `""` at the top level.
fn folder_of(file: &str) -> &str {
    file.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// A title as the library sorts it: in lower case, without a leading
/// English article.
fn title_key(title: &str) -> String {
    let lower = title.trim().to_lowercase();
    for article in ["the ", "a ", "an "] {
        if let Some(rest) = lower.strip_prefix(article) {
            return rest.trim_start().to_string();
        }
    }
    lower
}

/// An author as the library sorts them: surname first. The surname is the
/// first author's last word, before any "Jr." or the like, unless the name
/// is written "Surname, Given" already.
fn author_key(author: &str) -> String {
    const SUFFIXES: [&str; 6] = ["jr", "jr.", "sr", "sr.", "ii", "iii"];
    let lower = author.to_lowercase();
    let first = lower.split([';', '&']).next().unwrap_or("");
    let first = first.split(" and ").next().unwrap_or(first).trim();
    if let Some((surname, given)) = first.split_once(',') {
        if !SUFFIXES.contains(&given.trim()) {
            return format!("{}, {}", surname.trim(), given.trim());
        }
    }
    let mut words: Vec<&str> = first.split([' ', ',']).filter(|w| !w.is_empty()).collect();
    if words.len() > 1 && words.last().is_some_and(|w| SUFFIXES.contains(w)) {
        words.pop();
    }
    match words.split_last() {
        Some((surname, given)) if !given.is_empty() => format!("{}, {}", surname, given.join(" ")),
        _ => words.join(" "),
    }
}

/// When a book was last opened or added, for the Recent order.
fn touched(b: &BookInfo) -> u32 {
    b.added.max(b.opened)
}

impl<S: Storage> App<S> {
    /// Lists the books, reading titles from the cache file and opening only
    /// books that are new or changed.
    pub(super) fn scan_library(&mut self) {
        let cached: Vec<BookInfo> = self
            .storage
            .load(LIBRARY_FILE)
            .and_then(|d| String::from_utf8(d).ok())
            .map(|s| {
                s.lines()
                    .filter_map(|l| {
                        let mut f = l.split('\t');
                        Some(BookInfo {
                            file: f.next()?.to_string(),
                            size: f.next()?.parse().ok()?,
                            title: f.next()?.to_string(),
                            author: f.next().unwrap_or("").to_string(),
                            added: f.next().and_then(|n| n.parse().ok()).unwrap_or(0),
                            opened: f.next().and_then(|n| n.parse().ok()).unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Books new to the library are added together, at the counter's
        // next step.
        let now = cached.iter().map(touched).max().unwrap_or(0) + 1;
        let mut books = Vec::new();
        let mut changed = false;
        for (file, size) in self.storage.list_books() {
            if let Some(b) = cached.iter().find(|b| b.file == file && b.size == size) {
                books.push(b.clone());
                continue;
            }
            changed = true;
            let opened = self.storage.open_book(&file).and_then(|f| epub::open(f, &file));
            let (title, author) = match opened {
                Ok(e) => (e.title, e.author),
                Err(_) => (file.rsplit('/').next().unwrap_or(&file).to_string(), String::from("(can't open this file)")),
            };
            books.push(BookInfo { file, size, title, author, added: now, opened: 0 });
        }
        self.library = books;
        if changed || self.library.len() != cached.len() {
            self.save_library();
        }
        self.sort_library();
        // A folder left empty, as by deleting its last book, gives way to
        // the nearest one that still has books.
        while !self.folder.is_empty() && !self.library.iter().any(|b| folder_of(&b.file).starts_with(self.folder.as_str()) && b.file.as_bytes().get(self.folder.len()) == Some(&b'/')) {
            self.folder = folder_of(&self.folder).to_string();
        }
    }

    fn save_library(&mut self) {
        let mut s = String::new();
        for b in &self.library {
            s.push_str(&format!("{}\t{}\t{}\t{}\t{}\t{}\n", b.file, b.size, b.title, b.author, b.added, b.opened));
        }
        let _ = self.storage.save(LIBRARY_FILE, s.as_bytes());
    }

    /// Puts the books in the order the Library order setting picks.
    pub(super) fn sort_library(&mut self) {
        // One kind of key for every order, so the firmware has one sort.
        let order = self.settings.library_order();
        self.library.sort_by_cached_key(|b| match order {
            LibraryOrder::Recent => (Reverse(touched(b)), title_key(&b.title), String::new()),
            LibraryOrder::Title => (Reverse(0), title_key(&b.title), String::new()),
            // Books without an author last.
            LibraryOrder::Author => (Reverse(!b.author.trim().is_empty() as u32), author_key(&b.author), title_key(&b.title)),
            LibraryOrder::Added => (Reverse(b.added), title_key(&b.title), String::new()),
        });
    }

    /// Notes that the book `file` was opened from the library, for the
    /// Recent order.
    pub(super) fn mark_opened(&mut self, file: &str) {
        let latest = self.library.iter().map(touched).max().unwrap_or(0);
        let Some(i) = self.library.iter().position(|b| b.file == file) else { return };
        // Already the one book opened or added last, as when the reader
        // starts again: the card isn't written.
        let tied = self.library.iter().filter(|b| touched(b) == latest).count();
        if touched(&self.library[i]) == latest && tied == 1 {
            return;
        }
        self.library[i].opened = latest + 1;
        self.save_library();
        self.sort_library();
    }

    /// The rows of the folder shown: the folders in it, its books, then at
    /// the top level the tools.
    pub(super) fn rows(&self) -> Vec<Row> {
        let mut folders: Vec<(String, usize)> = Vec::new();
        let mut rows = Vec::new();
        for (i, b) in self.library.iter().enumerate() {
            let rest = if self.folder.is_empty() {
                b.file.as_str()
            } else {
                match b.file.strip_prefix(self.folder.as_str()).and_then(|r| r.strip_prefix('/')) {
                    Some(rest) => rest,
                    None => continue,
                }
            };
            match rest.split_once('/') {
                Some((name, _)) => match folders.iter_mut().find(|(f, _)| f == name) {
                    Some((_, n)) => *n += 1,
                    None => folders.push((name.to_string(), 1)),
                },
                None => rows.push(Row::Book(i)),
            }
        }
        folders.sort_by_cached_key(|(f, _)| f.to_lowercase());
        let mut out: Vec<Row> = folders.into_iter().map(|(f, n)| Row::Folder(f, n)).collect();
        out.append(&mut rows);
        if self.folder.is_empty() {
            out.extend((0..TOOLS.len()).map(Row::Tool));
        }
        out
    }

    /// Lists the books again and shows the open book's folder with the book
    /// selected, or the top level.
    pub(super) fn show_library(&mut self) -> bool {
        self.scan_library();
        let file = self.reader.as_ref().map(|r| r.info.file.clone());
        self.library_at(file.as_deref());
        true
    }

    /// Shows the folder holding the book `file`, with it selected, or the
    /// top level if there's no such book.
    pub(super) fn library_at(&mut self, file: Option<&str>) {
        let i = file.and_then(|f| self.library.iter().position(|b| b.file == f));
        self.folder = i.map_or(String::new(), |i| folder_of(&self.library[i].file).to_string());
        let sel = i.and_then(|i| self.rows().iter().position(|r| *r == Row::Book(i)));
        self.screen = Screen::Library { sel: sel.unwrap_or(0) };
    }

    /// The library's top level, with tool `k` selected.
    pub(super) fn library_tool(&mut self, k: usize) -> Screen {
        self.folder.clear();
        let sel = self.rows().iter().position(|r| *r == Row::Tool(k));
        Screen::Library { sel: sel.unwrap_or(0) }
    }

    pub(super) fn handle_library(&mut self, input: Input, sel: usize) -> bool {
        let rows = self.rows();
        let n = rows.len();
        if n == 0 {
            self.folder.clear();
            self.screen = Screen::Library { sel: 0 };
            return true;
        }
        let sel = sel.min(n - 1);
        match input {
            Input::Prev => self.screen = Screen::Library { sel: (sel + n - 1) % n },
            Input::Next => self.screen = Screen::Library { sel: (sel + 1) % n },
            Input::SkipBack | Input::SkipForward => {
                let forward = input == Input::SkipForward;
                self.screen = Screen::Library { sel: skip_rows(sel, n, rows_per_page(BOOK_ROW), forward) };
            }
            Input::Menu => match &rows[sel] {
                Row::Folder(name, _) => {
                    self.folder = if self.folder.is_empty() { name.clone() } else { format!("{}/{}", self.folder, name) };
                    self.screen = Screen::Library { sel: 0 };
                }
                Row::Book(i) => {
                    let file = self.library[*i].file.clone();
                    self.open_book(*i);
                    if self.screen == Screen::Reading {
                        self.mark_opened(&file);
                    }
                }
                Row::Tool(0) => self.open_transfer(),
                Row::Tool(1) => self.screen = Screen::Settings { sel: 0, from_reading: false },
                Row::Tool(_) => self.screen = Screen::System { from_reading: false },
            },
            // Up a folder, with the one left selected.
            Input::Back if !self.folder.is_empty() => {
                let left = self.folder.rsplit('/').next().unwrap_or("").to_string();
                self.folder = folder_of(&self.folder).to_string();
                let sel = self.rows().iter().position(|r| matches!(r, Row::Folder(f, _) if *f == left));
                self.screen = Screen::Library { sel: sel.unwrap_or(0) };
            }
            Input::Back if self.reader.is_some() => self.screen = Screen::Reading,
            _ => return false,
        }
        true
    }

    /// Deletes a book's file, with its place, cover thumbnail and
    /// bookmarks, and lists the library again.
    pub(super) fn remove_book(&mut self, info: &BookInfo) -> Result<(), Error> {
        // The card can't delete an open file.
        if self.reader.as_ref().is_some_and(|r| r.info.file == info.file) {
            self.reader = None;
        }
        self.storage.delete_book(&info.file)?;
        // The state files go by the file's name, so a copy in another
        // folder keeps them.
        let key = book_file(info, "POS");
        if !self.library.iter().any(|b| b.file != info.file && book_file(b, "POS") == key) {
            for ext in ["POS", "THM", "BMK"] {
                self.storage.remove(&book_file(info, ext));
            }
        }
        if self.storage.load(LAST_FILE).is_some_and(|d| d.trim_ascii() == info.file.as_bytes()) {
            self.storage.remove(LAST_FILE);
        }
        self.shelf.retain(|b| b.key != key);
        self.scan_library();
        Ok(())
    }

    /// Asking whether to delete the open book: `yes` when Delete is
    /// selected rather than Keep.
    pub(super) fn handle_delete(&mut self, input: Input, yes: bool) -> bool {
        match input {
            Input::Prev | Input::Next | Input::SkipBack | Input::SkipForward => self.screen = Screen::Delete { yes: !yes },
            Input::Menu if yes => {
                let Some(info) = self.reader.as_ref().map(|r| r.info.clone()) else { return false };
                // Where the book was, to select the row that takes its place.
                self.library_at(Some(&info.file));
                let Screen::Library { sel } = self.screen else { return false };
                match self.remove_book(&info) {
                    Ok(()) => {
                        let n = self.rows().len();
                        self.screen = Screen::Library { sel: sel.min(n.saturating_sub(1)) };
                    }
                    Err(_) => self.screen = Screen::Message(format!("Couldn't delete {}", info.title)),
                }
            }
            Input::Menu | Input::Back => self.screen = Screen::Reading,
        }
        true
    }

    /// The rows of the folder shown, with their covers.
    pub(super) fn draw_library(&mut self, canvas: &mut Canvas, sel: usize) {
        let title = match self.folder.rsplit('/').next() {
            Some(name) if !name.is_empty() => fit(&self.fonts, BOLD, 28, name, WIDTH - 2 * MARGIN_X),
            _ => String::from("Library"),
        };
        self.draw_header(canvas, &title);
        let rows = self.rows();
        let per_page = rows_per_page(BOOK_ROW);
        let first = sel / per_page * per_page;
        for (k, row) in rows.iter().enumerate().skip(first).take(per_page) {
            let y = LIST_TOP + (k - first) as i32 * BOOK_ROW;
            // Where the row's text starts; selection inverts it from there.
            let x = match row {
                Row::Folder(name, books) => self.draw_folder(canvas, name, *books, y),
                Row::Book(i) => self.draw_book(canvas, *i, y),
                Row::Tool(t) => {
                    let (label, help) = TOOLS[*t];
                    let x = MARGIN_X + 12;
                    self.text(canvas, BOLD, 22, x, y + 58, label);
                    self.text(canvas, ITALIC, 18, x, y + 88, help);
                    x
                }
            };
            if k == sel {
                canvas.invert_rect(x - 12, y + 2, WIDTH - MARGIN_X - x + 12, BOOK_ROW - 4);
            }
        }
        self.draw_list_page(canvas, sel, per_page, rows.len());
        if self.library.is_empty() {
            let y = LIST_TOP + TOOLS.len() as i32 * BOOK_ROW + 48;
            self.text(canvas, 0, 22, MARGIN_X + 12, y, "No books found.");
            self.text(canvas, ITALIC, 18, MARGIN_X + 12, y + 36, "Copy books (.epub, .txt, .md) to the card, or use Wi-Fi.");
        }
        if !self.folder.is_empty() {
            self.draw_hint(canvas, "Hold Menu: back");
        }
    }

    /// A folder's row at `y`: a folder where a book's cover goes, its name
    /// and how many books it holds. Returns where the text starts.
    fn draw_folder(&mut self, canvas: &mut Canvas, name: &str, books: usize, y: i32) -> i32 {
        let (cx, cy) = (MARGIN_X + 6, y + (BOOK_ROW - COVER.1) / 2);
        let (w, h, tab) = (COVER.0, 60, 34);
        let top = cy + (COVER.1 - h) / 2 + 4;
        canvas.fill_rect(cx, top - 10, tab, 12, BLACK);
        canvas.fill_rect(cx, top, w, h, BLACK);
        canvas.fill_rect(cx + 2, top - 8, tab - 4, 12, 218);
        canvas.fill_rect(cx + 2, top + 2, w - 4, h - 4, 218);
        let x = cx + COVER.0 + 20;
        let max = WIDTH - MARGIN_X - 12 - x;
        let mut base = y + 34;
        for line in wrap(&self.fonts, BOLD, 22, name, max, 2) {
            self.text(canvas, BOLD, 22, x, base, &line);
            base += 28;
        }
        let count = if books == 1 { String::from("1 book") } else { format!("{books} books") };
        self.text(canvas, ITALIC, 18, x, base + 2, &count);
        x
    }

    /// Book `i`'s library row at `y`: its cover, then title, author and
    /// progress. Returns where the text starts.
    pub(super) fn draw_book(&mut self, canvas: &mut Canvas, i: usize, y: i32) -> i32 {
        let at = self.shelved(i);
        let (cx, cy) = (MARGIN_X + 6, y + (BOOK_ROW - COVER.1) / 2);
        let percent = self.shelf[at].percent;
        match &self.shelf[at].cover {
            Some(g) => {
                let (x, y) = (cx + (COVER.0 - g.width) / 2, cy + (COVER.1 - g.height) / 2);
                // A hairline frame keeps white covers apart from the page.
                canvas.fill_rect(x - 1, y - 1, g.width + 2, g.height + 2, BLACK);
                g.draw(canvas, x, y);
            }
            None => {
                // A gray card with the title's first letter.
                canvas.fill_rect(cx, cy, COVER.0, COVER.1, BLACK);
                canvas.fill_rect(cx + 2, cy + 2, COVER.0 - 4, COVER.1 - 4, 218);
                let initial: String = self.library[i].title.chars().take(1).collect();
                let w = self.width(BOLD, 48, &initial);
                self.text(canvas, BOLD, 48, cx + (COVER.0 - w) / 2, cy + COVER.1 / 2 + 17, &initial);
            }
        }
        let x = cx + COVER.0 + 20;
        let max = WIDTH - MARGIN_X - 12 - x;
        let title = wrap(&self.fonts, BOLD, 22, &self.library[i].title, max, 2);
        let author = fit(&self.fonts, ITALIC, 18, &self.library[i].author, max);
        let mut base = y + 34;
        for line in &title {
            self.text(canvas, BOLD, 22, x, base, line);
            base += 28;
        }
        self.text(canvas, ITALIC, 18, x, base + 2, &author);
        if let Some(p) = percent {
            self.text(canvas, 0, STATUS_PX, x, y + BOOK_ROW - 20, &format!("{}% read", p));
        }
        x
    }

    /// The open book, and whether to delete it: Keep or Delete.
    pub(super) fn draw_delete(&mut self, canvas: &mut Canvas, yes: bool) {
        let Some(file) = self.reader.as_ref().map(|r| r.info.file.clone()) else { return };
        self.draw_header(canvas, "Delete book");
        if let Some(i) = self.library.iter().position(|b| b.file == file) {
            self.draw_book(canvas, i, LIST_TOP);
        }
        let x = MARGIN_X + 12;
        let y = LIST_TOP + BOOK_ROW + 44;
        let y = self.paragraph(canvas, 0, x, y, WIDTH - 2 * x, "This deletes the book from the card, with its place and bookmarks.");
        let top = y + 32;
        for (k, label) in ["Keep it", "Delete"].into_iter().enumerate() {
            let row = top + k as i32 * LIST_ROW;
            self.text(canvas, if k == 1 { BOLD } else { 0 }, 22, x, row + 30, label);
            if (k == 1) == yes {
                canvas.invert_rect(MARGIN_X, row + 2, WIDTH - 2 * MARGIN_X, LIST_ROW - 4);
            }
        }
        self.draw_hint(canvas, "Menu: choose    Hold Menu: back");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;

    #[test]
    fn sort_keys() {
        assert_eq!(title_key("The Time Machine"), "time machine");
        assert_eq!(title_key("An Old Story"), "old story");
        assert_eq!(title_key("Anna Karenina"), "anna karenina");
        assert_eq!(title_key("Кобзар"), "кобзар");
        assert_eq!(author_key("Lewis Carroll"), "carroll, lewis");
        assert_eq!(author_key("Austen, Jane"), "austen, jane");
        assert_eq!(author_key("Martin Luther King, Jr."), "king, martin luther");
        assert_eq!(author_key("H. G. Wells & Someone Else"), "wells, h. g.");
        assert_eq!(author_key("Тарас Шевченко"), "шевченко, тарас");
        assert_eq!(author_key("Homer"), "homer");
        assert_eq!(author_key(""), "");
    }

    /// Books as text files in memory, and state files beside them.
    #[derive(Default)]
    struct Memory {
        books: BTreeMap<String, Vec<u8>>,
        state: BTreeMap<String, Vec<u8>>,
    }

    struct Nothing;

    impl Upload for Nothing {
        fn write(&mut self, _: &[u8]) -> Result<(), Error> {
            Err(Error::Io)
        }

        fn finish(self) -> Result<(), Error> {
            Err(Error::Io)
        }
    }

    impl Storage for Memory {
        type Book = Vec<u8>;
        type Upload = Nothing;

        fn list_books(&mut self) -> Vec<(String, u64)> {
            self.books.iter().map(|(n, d)| (n.clone(), d.len() as u64)).collect()
        }

        fn open_book(&mut self, name: &str) -> Result<Vec<u8>, Error> {
            self.books.get(name).cloned().ok_or(Error::Io)
        }

        fn create_book(&mut self, _: &str) -> Result<(String, Nothing), Error> {
            Err(Error::Io)
        }

        fn delete_book(&mut self, name: &str) -> Result<(), Error> {
            self.books.remove(name).map(|_| ()).ok_or(Error::Io)
        }

        fn load(&mut self, name: &str) -> Option<Vec<u8>> {
            self.state.get(name).cloned()
        }

        fn save(&mut self, name: &str, data: &[u8]) -> Result<(), Error> {
            self.state.insert(name.to_string(), data.to_vec());
            Ok(())
        }

        fn remove(&mut self, name: &str) {
            self.state.remove(name);
        }

        fn list_pictures(&mut self) -> Vec<String> {
            Vec::new()
        }

        fn load_picture(&mut self, _: &str) -> Option<Vec<u8>> {
            None
        }

        fn list_fonts(&mut self) -> Vec<(String, Vec<(String, u64)>)> {
            Vec::new()
        }

        fn load_font(&mut self, _: &str, _: &str) -> Option<Vec<u8>> {
            None
        }

        fn create_font(&mut self, _: &str, _: &str) -> Result<Nothing, Error> {
            Err(Error::Io)
        }

        fn delete_font(&mut self, _: &str, _: Option<&str>) -> Result<(), Error> {
            Ok(())
        }

        fn create_firmware(&mut self) -> Result<Nothing, Error> {
            Err(Error::Io)
        }

        fn read_firmware(&mut self, _: &mut dyn FnMut(&[u8])) -> Result<(), Error> {
            Err(Error::Io)
        }

        fn delete_firmware(&mut self) {}
    }

    /// A library of Markdown books, each `(path, title, author)`.
    fn app(books: &[(&str, &str, &str)]) -> App<Memory> {
        let mut storage = Memory::default();
        for (path, title, author) in books {
            let text = format!("---\ntitle: {title}\nauthor: {author}\n---\n\n# One\n\nSome text.\n");
            storage.books.insert(path.to_string(), text.into_bytes());
        }
        let mut app = App::new(storage);
        app.start();
        app
    }

    /// What the library screen lists: folder names, book titles and tools.
    fn shown(app: &App<Memory>) -> Vec<String> {
        app.rows()
            .iter()
            .map(|r| match r {
                Row::Folder(f, n) => format!("{f}/ {n}"),
                Row::Book(i) => app.library[*i].title.clone(),
                Row::Tool(t) => TOOLS[*t].0.to_string(),
            })
            .collect()
    }

    fn press(app: &mut App<Memory>, keys: &str) {
        for k in keys.chars() {
            app.handle(match k {
                'n' => Input::Next,
                'p' => Input::Prev,
                'm' => Input::Menu,
                _ => Input::Back,
            });
        }
    }

    const SHELF: [(&str, &str, &str); 5] = [
        ("time.md", "The Time Machine", "H. G. Wells"),
        ("alice.md", "Alice in Wonderland", "Lewis Carroll"),
        ("Fiction/emma.md", "Emma", "Jane Austen"),
        ("Fiction/Old/kobzar.md", "Кобзар", "Тарас Шевченко"),
        ("Fiction/Old/odyssey.md", "The Odyssey", "Homer"),
    ];

    #[test]
    fn folders_open_and_close() {
        let mut app = app(&SHELF);
        app.settings.apply(b"library_order = Title\n");
        app.sort_library();
        assert_eq!(shown(&app), ["Fiction/ 3", "Alice in Wonderland", "The Time Machine", "Wi-Fi transfer", "Settings", "System info"]);
        press(&mut app, "m");
        assert_eq!(app.folder, "Fiction");
        assert_eq!(shown(&app), ["Old/ 2", "Emma"]);
        press(&mut app, "m");
        assert_eq!(shown(&app), ["The Odyssey", "Кобзар"]);
        // Back goes up, to the folder left.
        press(&mut app, "b");
        assert_eq!((app.folder.as_str(), app.screen.clone()), ("Fiction", Screen::Library { sel: 0 }));
        press(&mut app, "b");
        assert_eq!((app.folder.as_str(), app.screen.clone()), ("", Screen::Library { sel: 0 }));
        // A book in a folder opens, and leaving it shows its folder.
        press(&mut app, "mnm");
        assert_eq!(app.reader.as_ref().unwrap().info.title, "Emma");
        press(&mut app, "b");
        assert_eq!((app.folder.as_str(), app.screen.clone()), ("Fiction", Screen::Library { sel: 1 }));
    }

    #[test]
    fn orders() {
        let mut app = app(&SHELF);
        let titles = |app: &App<Memory>| app.library.iter().map(|b| b.title.as_str()).collect::<Vec<_>>().join(", ");
        let order = |app: &mut App<Memory>, order: &str| {
            app.settings.apply(format!("library_order = {order}\n").as_bytes());
            app.sort_library();
        };
        order(&mut app, "Title");
        assert_eq!(titles(&app), "Alice in Wonderland, Emma, The Odyssey, The Time Machine, Кобзар");
        order(&mut app, "Author");
        assert_eq!(titles(&app), "Emma, Alice in Wonderland, The Odyssey, The Time Machine, Кобзар");
        // Books found together were added together: by title among them.
        order(&mut app, "Recent");
        assert_eq!(titles(&app), "Alice in Wonderland, Emma, The Odyssey, The Time Machine, Кобзар");
        // Opening a book from the library puts it first, and a book added
        // later goes before it.
        assert_eq!(shown(&app)[2], "The Time Machine");
        press(&mut app, "nnm");
        assert_eq!(app.library[0].title, "The Time Machine");
        app.storage.books.insert("new.md".to_string(), b"# New\n\nText.\n".to_vec());
        app.scan_library();
        assert_eq!(app.library[0].file, "new.md");
        assert_eq!(app.library[1].title, "The Time Machine");
        // Kept in LIBRARY.TXT. Reopening the last book at start-up doesn't
        // count as opening it.
        let mut again = App::new(core::mem::take(&mut app.storage));
        again.settings = app.settings.clone();
        again.start();
        assert_eq!(again.reader.as_ref().unwrap().info.title, "The Time Machine");
        assert_eq!(again.library[0].file, "new.md");
        order(&mut again, "Added");
        assert_eq!(again.library[0].file, "new.md");
        assert_eq!(again.library[1].title, "Alice in Wonderland");
    }

    #[test]
    fn deleting_the_open_book() {
        let mut app = app(&SHELF);
        app.settings.apply(b"library_order = Title\n");
        app.sort_library();
        // Open Emma, the only book in Fiction outside Old.
        press(&mut app, "mnm");
        let info = app.reader.as_ref().unwrap().info.clone();
        assert_eq!(info.title, "Emma");
        app.toggle_bookmark();
        assert!(app.storage.state.contains_key(&book_file(&info, "BMK")));
        // The menu's last item asks first; Keep it, the first choice, keeps it.
        let menu = app.menu();
        assert!(menu.last() == Some(&MenuItem::Delete));
        app.screen = Screen::Menu { sel: menu.len() - 1 };
        press(&mut app, "m");
        assert_eq!(app.screen, Screen::Delete { yes: false });
        press(&mut app, "m");
        assert_eq!(app.screen, Screen::Reading);
        app.screen = Screen::Menu { sel: menu.len() - 1 };
        press(&mut app, "mnm");
        assert!(app.reader.is_none());
        assert!(!app.storage.books.contains_key("Fiction/emma.md"));
        for ext in ["POS", "BMK"] {
            assert!(!app.storage.state.contains_key(&book_file(&info, ext)));
        }
        assert!(!app.storage.state.contains_key(LAST_FILE));
        // Fiction still has Old, where Emma's row was.
        assert_eq!(app.folder, "Fiction");
        assert_eq!(shown(&app), ["Old/ 2"]);
        assert_eq!(app.screen, Screen::Library { sel: 0 });
        // Back reaches the top; nothing is open to go back to after that.
        press(&mut app, "b");
        assert_eq!(app.folder, "");
        assert!(!app.handle(Input::Back));
    }

    #[test]
    fn an_emptied_folder_gives_way() {
        let mut app = app(&SHELF[..3]);
        app.folder = String::from("Fiction");
        app.storage.books.remove("Fiction/emma.md");
        app.scan_library();
        assert_eq!(app.folder, "");
        assert!(!shown(&app).iter().any(|r| r.starts_with("Fiction")));
    }
}
