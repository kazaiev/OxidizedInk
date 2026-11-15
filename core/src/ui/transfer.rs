//! Wi-Fi transfer: a web page, served by the reader, for sending books to
//! the card and deleting them. The platform joins the network named in
//! `WIFI.TXT` and hands each browser connection to `App::serve`.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::*;
use crate::http::{percent_decode, push_json, respond, respond_head, Connection, Request};

/// The network to join, as `key = value` lines: `ssid = …`, `password = …`.
pub const WIFI_FILE: &str = "WIFI.TXT";
const PAGE: &str = include_str!("transfer.html");
/// Lines of the transfer log the screen shows.
const LOG_LINES: usize = 6;
/// The largest firmware image: an app slot.
const MAX_FIRMWARE: u64 = 0x1f_0000;

/// A Wi-Fi network to join, from `WIFI.TXT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wifi {
    pub ssid: String,
    pub password: String,
}

impl Wifi {
    /// Reads `WIFI.TXT`. None if it names no network.
    pub fn parse(data: &[u8]) -> Option<Wifi> {
        let text = core::str::from_utf8(data).ok()?;
        let mut wifi = Wifi { ssid: String::new(), password: String::new() };
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            match key.trim() {
                "ssid" => wifi.ssid = value.trim().to_string(),
                "password" => wifi.password = value.trim().to_string(),
                _ => {}
            }
        }
        (!wifi.ssid.is_empty()).then_some(wifi)
    }
}

/// Where the Wi-Fi transfer screen's connection stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Network {
    /// There's no `WIFI.TXT` naming a network.
    NotSetUp,
    /// The platform should join the network from `App::wifi_wanted`, then
    /// set `Ready` or `Failed`.
    Connecting,
    /// Serving the page at this address, such as "http://192.168.1.23/".
    Ready(String),
    /// Couldn't join the network, and why.
    Failed(String),
}

/// Upload names the page accepts: an `.epub` without directories.
fn upload_name_ok(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".epub") && lower.len() > 5 && !name.contains(['/', '\\']) && !name.starts_with('.')
}

/// A font family's name as the upload page sends it: what a folder can be
/// called.
fn family_name_ok(name: &str) -> bool {
    let bad = |c: char| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|');
    !name.trim().is_empty() && name.len() <= 64 && !name.starts_with('.') && !name.contains(bad)
}

/// The file names the upload page gives a family's styles.
fn font_file_ok(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    let Some((stem, ext)) = upper.split_once('.') else { return false };
    matches!(stem, "REGULAR" | "BOLD" | "ITALIC" | "BOLDITAL") && matches!(ext, "TTF" | "OTF")
}

impl<S: Storage> App<S> {
    /// The network to join while the Wi-Fi transfer screen is up and
    /// `network` is `Connecting`.
    pub fn wifi_wanted(&self) -> Option<&Wifi> {
        self.wifi.as_ref().filter(|_| self.screen == Screen::Transfer)
    }

    /// True while the Wi-Fi transfer screen is up: the platform keeps the
    /// network up and passes connections to `serve`.
    pub fn transfer_open(&self) -> bool {
        self.screen == Screen::Transfer
    }

    /// Starts the reader on the transfer screen, without opening a book:
    /// for a platform that brings Wi-Fi up in a boot of its own.
    pub fn start_transfer(&mut self) {
        self.load_settings();
        self.scan_library();
        self.open_transfer();
    }

    pub(super) fn open_transfer(&mut self) {
        self.wifi = self.storage.load(WIFI_FILE).and_then(|d| Wifi::parse(&d));
        self.network = if self.wifi.is_some() { Network::Connecting } else { Network::NotSetUp };
        self.screen = Screen::Transfer;
    }

    /// Leaves the transfer screen for the library, listing the books again.
    /// A platform that left Wi-Fi by restarting calls it after `start`.
    pub fn close_transfer(&mut self) {
        self.scan_library();
        self.transfer_log.clear();
        // A family sent or deleted may be the one books use.
        if core::mem::take(&mut self.fonts_changed) {
            self.book_family.clear();
            self.load_book_fonts();
            self.relayout();
        }
        self.screen = Screen::Library { sel: self.library.len() };
    }

    pub(super) fn handle_transfer(&mut self, input: Input) -> bool {
        match input {
            Input::Back => self.close_transfer(),
            // Try again, reading WIFI.TXT afresh.
            Input::Menu if matches!(self.network, Network::NotSetUp | Network::Failed(_)) => self.open_transfer(),
            _ => return false,
        }
        true
    }

    /// Answers one request on a browser connection. Returns true if the
    /// screen changed, as after a book arrives.
    pub fn serve(&mut self, conn: &mut impl Connection) -> bool {
        let Ok(mut req) = Request::read(conn) else { return false };
        let path = req.path.clone();
        if let Some(rest) = path.strip_prefix("/fonts/") {
            let changed = match req.method.as_str() {
                "PUT" => self.receive_font(conn, &mut req, rest),
                "DELETE" => self.delete_family(conn, rest),
                _ => {
                    let _ = respond(conn, "404 Not Found", "text/plain", b"Not found");
                    false
                }
            };
            return self.logged(changed);
        }
        let (method, name) = (req.method.as_str(), path.strip_prefix("/books/"));
        let changed = match (method, name) {
            ("GET", _) if path == "/" => {
                let _ = respond(conn, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes());
                false
            }
            ("GET", _) if path == "/books" => {
                let json = self.books_json();
                let _ = respond(conn, "200 OK", "application/json", json.as_bytes());
                false
            }
            ("GET", _) if path == "/firmware" => {
                let mut json = String::from("{\"version\":");
                push_json(&mut json, &self.version);
                json.push('}');
                let _ = respond(conn, "200 OK", "application/json", json.as_bytes());
                false
            }
            ("PUT", _) if path == "/firmware" => self.receive_firmware(conn, &mut req),
            ("GET", _) if path == "/fonts" => {
                let json = self.fonts_json();
                let _ = respond(conn, "200 OK", "application/json", json.as_bytes());
                false
            }
            ("GET", Some(name)) => {
                self.send_book(conn, name);
                false
            }
            ("PUT", Some(name)) => self.receive(conn, &mut req, name),
            ("DELETE", Some(name)) => self.delete(conn, name),
            _ => {
                let _ = respond(conn, "404 Not Found", "text/plain", b"Not found");
                false
            }
        };
        self.logged(changed)
    }

    /// Keeps the transfer log to its last lines after a change.
    fn logged(&mut self, changed: bool) -> bool {
        if changed {
            let excess = self.transfer_log.len().saturating_sub(LOG_LINES);
            self.transfer_log.drain(..excess);
        }
        changed
    }

    /// The font families on the card: each one's name, files (with the
    /// style each is) and whether books use it.
    fn fonts_json(&mut self) -> String {
        let mut out = String::from("[");
        for (i, (family, files)) in self.storage.list_fonts().iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str("{\"family\":");
            push_json(&mut out, family);
            out.push_str(&format!(",\"current\":{},\"files\":[", *family == self.settings.font));
            for (k, (name, size)) in files.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                out.push_str("{\"name\":");
                push_json(&mut out, name);
                let style = crate::fonts::Style::of_file(name).map_or(String::from("other"), |s| format!("{s:?}"));
                out.push_str(",\"style\":");
                push_json(&mut out, &style);
                out.push_str(&format!(",\"size\":{size}}}"));
            }
            out.push_str("]}");
        }
        out.push(']');
        out
    }

    /// Receives one style of a family, `family/STYLE.TTF`, as the upload
    /// page names it. It must open as a font, or it's deleted again.
    fn receive_font(&mut self, conn: &mut impl Connection, req: &mut Request, rest: &str) -> bool {
        let fail = |conn: &mut _, status, message: &str| {
            let _ = respond(conn, status, "text/plain; charset=utf-8", message.as_bytes());
            false
        };
        let parts = rest.split_once('/').and_then(|(f, n)| Some((percent_decode(f)?, percent_decode(n)?)));
        let Some((family, file)) = parts.filter(|(f, n)| family_name_ok(f) && font_file_ok(n)) else {
            return fail(conn, "400 Bad Request", "Send .ttf or .otf files from the upload page.");
        };
        if req.length > card_fonts::MAX_FAMILY {
            return fail(conn, "413 Content Too Large", "This font file is too large for the reader.");
        }
        let file = file.to_ascii_uppercase();
        let Ok(mut upload) = self.storage.create_font(&family, &file) else {
            return fail(conn, "500 Internal Server Error", "Couldn't make the file on the card.");
        };
        let written = match req.body(conn, |chunk| upload.write(chunk)) {
            Ok(()) => upload.finish(),
            Err(e) => {
                drop(upload);
                Err(e)
            }
        };
        let font = written.is_ok() && self.storage.load_font(&family, &file).is_some_and(|d| crate::fonts::is_font(&d));
        if !font {
            let _ = self.storage.delete_font(&family, Some(&file));
            return fail(conn, "422 Unprocessable Entity", "This isn't a font file the reader can use.");
        }
        let _ = respond(conn, "201 Created", "text/plain", b"");
        let style = crate::fonts::Style::of_file(&file).map_or(String::new(), |s| format!(" ({s:?})"));
        self.transfer_log.push(format!("Received font {family}{style}"));
        self.fonts_changed = true;
        true
    }

    /// A firmware update arrived, which the platform restarts to install.
    pub fn update_received(&self) -> bool {
        self.update_received
    }

    /// Receives a firmware image into `/FIRMWARE.BIN`, checking it as it
    /// comes; one that fails is deleted again.
    fn receive_firmware(&mut self, conn: &mut impl Connection, req: &mut Request) -> bool {
        let fail = |conn: &mut _, status, message: &str| {
            let _ = respond(conn, status, "text/plain; charset=utf-8", message.as_bytes());
            false
        };
        if req.length > MAX_FIRMWARE {
            return fail(conn, "413 Content Too Large", "This file is larger than the reader's firmware can be.");
        }
        let Ok(mut upload) = self.storage.create_firmware() else {
            return fail(conn, "500 Internal Server Error", "Couldn't make the file on the card.");
        };
        let mut check = crate::firmware::ImageCheck::new();
        let written = match req.body(conn, |chunk| {
            check.feed(chunk);
            upload.write(chunk)
        }) {
            Ok(()) => upload.finish(),
            Err(e) => {
                drop(upload);
                Err(e)
            }
        };
        if written.is_err() {
            self.storage.delete_firmware();
            return fail(conn, "500 Internal Server Error", "Writing to the card failed.");
        }
        if let Err(why) = check.finish() {
            self.storage.delete_firmware();
            let message = format!("This can't be installed: {why}.");
            return fail(conn, "422 Unprocessable Entity", &message);
        }
        // Check what the card kept, too.
        let mut kept = crate::firmware::ImageCheck::new();
        let read = self.storage.read_firmware(&mut |piece| kept.feed(piece));
        if read.is_err() || kept.finish().is_err() {
            self.storage.delete_firmware();
            return fail(conn, "500 Internal Server Error", "The card didn't keep the update as it was sent; try again.");
        }
        let _ = respond(conn, "201 Created", "text/plain", b"");
        self.transfer_log.push(String::from("Received a firmware update"));
        self.update_received = true;
        true
    }

    /// Deletes a font family's files.
    fn delete_family(&mut self, conn: &mut impl Connection, rest: &str) -> bool {
        let Some(family) = percent_decode(rest).filter(|f| family_name_ok(f)) else {
            let _ = respond(conn, "404 Not Found", "text/plain", b"No such font.");
            return false;
        };
        if self.storage.delete_font(&family, None).is_err() {
            let _ = respond(conn, "500 Internal Server Error", "text/plain", b"Couldn't delete the font.");
            return false;
        }
        let _ = respond(conn, "204 No Content", "text/plain", b"");
        self.transfer_log.push(format!("Deleted font {family}"));
        self.fonts_changed = true;
        true
    }

    fn books_json(&self) -> String {
        let mut out = String::from("[");
        for (i, b) in self.library.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str("{\"file\":");
            push_json(&mut out, &b.file);
            out.push_str(",\"title\":");
            push_json(&mut out, &b.title);
            out.push_str(",\"author\":");
            push_json(&mut out, &b.author);
            out.push_str(&format!(",\"size\":{}}}", b.size));
        }
        out.push(']');
        out
    }

    /// Sends a book's file, to save from the browser under its title.
    fn send_book(&mut self, conn: &mut impl Connection, name: &str) {
        let found = percent_decode(name).and_then(|n| self.library.iter().find(|b| b.file == n).cloned());
        let Some(info) = found else {
            let _ = respond(conn, "404 Not Found", "text/plain", b"No such book.");
            return;
        };
        let Ok(mut file) = self.storage.open_book(&info.file) else {
            let _ = respond(conn, "500 Internal Server Error", "text/plain", b"Couldn't open the file.");
            return;
        };
        // The title as a file name, in plain ASCII.
        let safe: String = info.title.chars().filter(|c| c.is_ascii_alphanumeric() || " -_,.'()".contains(*c)).collect();
        let name = if safe.trim().is_empty() { "book" } else { safe.trim() };
        let extra = format!("Content-Disposition: attachment; filename=\"{}.epub\"\r\n", name);
        let size = file.size();
        if respond_head(conn, "200 OK", "application/epub+zip", size, &extra).is_err() {
            return;
        }
        let mut buf = alloc::vec![0u8; 8192];
        let mut at = 0;
        while at < size {
            let n = (size - at).min(buf.len() as u64) as usize;
            if file.read_at(at, &mut buf[..n]).is_err() || conn.write(&buf[..n]).is_err() {
                return;
            }
            at += n as u64;
        }
    }

    /// Writes an uploaded book to the card, keeping it only if it opens.
    fn receive(&mut self, conn: &mut impl Connection, req: &mut Request, name: &str) -> bool {
        let fail = |conn: &mut _, status, message: &str| {
            let _ = respond(conn, status, "text/plain; charset=utf-8", message.as_bytes());
            false
        };
        let Some(name) = percent_decode(name).filter(|n| upload_name_ok(n)) else {
            return fail(conn, "400 Bad Request", "Only .epub files can be sent.");
        };
        let Ok((file, mut upload)) = self.storage.create_book(&name) else {
            return fail(conn, "500 Internal Server Error", "Couldn't make the file on the card.");
        };
        let written = match req.body(conn, |chunk| upload.write(chunk)) {
            Ok(()) => upload.finish(),
            Err(e) => {
                drop(upload);
                Err(e)
            }
        };
        if written.is_err() {
            let _ = self.storage.delete_book(&file);
            return fail(conn, "500 Internal Server Error", "Writing to the card failed.");
        }
        if self.storage.open_book(&file).and_then(Epub::open).is_err() {
            let _ = self.storage.delete_book(&file);
            return fail(conn, "422 Unprocessable Entity", "This isn't an EPUB the reader can open.");
        }
        self.scan_library();
        let title = self.library.iter().find(|b| b.file == file).map_or(name, |b| b.title.clone());
        let mut json = String::from("{\"file\":");
        push_json(&mut json, &file);
        json.push('}');
        let _ = respond(conn, "201 Created", "application/json", json.as_bytes());
        self.transfer_log.push(format!("Received {}", title));
        true
    }

    /// Deletes a book and its progress and cover thumbnail.
    fn delete(&mut self, conn: &mut impl Connection, name: &str) -> bool {
        let found = percent_decode(name).and_then(|n| self.library.iter().find(|b| b.file == n).cloned());
        let Some(info) = found else {
            let _ = respond(conn, "404 Not Found", "text/plain", b"No such book.");
            return false;
        };
        // The card can't delete an open file.
        if self.reader.as_ref().is_some_and(|r| r.info.file == info.file) {
            self.reader = None;
        }
        if self.storage.delete_book(&info.file).is_err() {
            let _ = respond(conn, "500 Internal Server Error", "text/plain", b"Couldn't delete the file.");
            return false;
        }
        for ext in ["POS", "THM", "BMK"] {
            self.storage.remove(&book_file(&info, ext));
        }
        self.scan_library();
        let _ = respond(conn, "204 No Content", "text/plain", b"");
        self.transfer_log.push(format!("Deleted {}", info.title));
        true
    }

    pub(super) fn draw_transfer(&mut self, canvas: &mut Canvas) {
        self.draw_header(canvas, "Wi-Fi transfer");
        let x = MARGIN_X + 12;
        let max = WIDTH - MARGIN_X - 12 - x;
        let ssid = self.wifi.as_ref().map_or(String::new(), |w| w.ssid.clone());
        let mut y = LIST_TOP + 44;
        let hint = match self.network.clone() {
            Network::NotSetUp => {
                y = self.paragraph(canvas, 0, x, y, max, "To send books over Wi-Fi, add a file named WIFI.TXT to the READER folder on the card, with your network's name and password:");
                y += 16;
                for line in ["ssid = My network", "password = its password"] {
                    y += 36;
                    self.text(canvas, BOLD, 22, x + 24, y, line);
                }
                "Menu: try again    Hold Menu: back"
            }
            Network::Connecting => {
                self.paragraph(canvas, 0, x, y, max, &format!("Joining {}…", ssid));
                "Hold Menu: back"
            }
            Network::Ready(address) => {
                y = self.paragraph(canvas, 0, x, y, max, &format!("On a computer or phone on {}, open:", ssid));
                y += 60;
                let w = self.width(BOLD, 30, &address);
                self.text(canvas, BOLD, 30, x + (max - w).max(0) / 2, y, &address);
                y += 60;
                y = self.paragraph(canvas, 0, x, y, max, "Drop .epub files on that page to send them here. You can delete books there too.");
                y += 16;
                for line in self.transfer_log.clone() {
                    y += 30;
                    let line = fit(&self.fonts, ITALIC, 20, &line, max);
                    self.text(canvas, ITALIC, 20, x, y, &line);
                }
                "Hold Menu: stop Wi-Fi"
            }
            Network::Failed(why) => {
                y = self.paragraph(canvas, 0, x, y, max, &format!("Couldn't join {}: {}.", ssid, why));
                self.paragraph(canvas, 0, x, y + 16, max, "Check the name and password in WIFI.TXT in the READER folder on the card.");
                "Menu: try again    Hold Menu: back"
            }
        };
        self.draw_hint(canvas, hint);
    }

    /// Draws `s` in 22-pixel lines from baseline `y`, wrapped to `max`
    /// pixels. Returns the last line's baseline.
    fn paragraph(&mut self, canvas: &mut Canvas, face: u8, x: i32, y: i32, max: i32, s: &str) -> i32 {
        let lines: Vec<String> = wrap(&self.fonts, face, 22, s, max, 12);
        let mut base = y;
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                base += 32;
            }
            self.text(canvas, face, 22, x, base, line);
        }
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wifi_file() {
        let w = Wifi::parse(b"# home\nssid = My Net \npassword=a=b c\n").unwrap();
        assert_eq!((w.ssid.as_str(), w.password.as_str()), ("My Net", "a=b c"));
        assert!(Wifi::parse(b"password = x\n").is_none());
    }

    #[test]
    fn upload_names() {
        assert!(upload_name_ok("Pride and Prejudice.EPUB"));
        assert!(!upload_name_ok("../x.epub"));
        assert!(!upload_name_ok("notes.txt"));
        assert!(!upload_name_ok(".epub"));
    }
}
