//! PC simulator: runs the reader core against a directory of books.
//!
//! ```text
//! inkreader-sim [--books DIR] [--state DIR] [--true-gray] [--battery PERCENT] [--port PORT]
//!               [--keys KEYS --out DIR [--serve]]
//! ```
//!
//! `--books` defaults to `testbooks/`; `--state` (the card's `/READER/`)
//! defaults to `READER/` inside it, and the sleep screen's pictures (the
//! card's `/SLEEP/`) come from `SLEEP/` inside it. The screen is shown in the
//! panel's 8 gray levels unless `--true-gray` is given. `--battery` pretends
//! a charge level, as the device reports it.
//!
//! In a window: Right/Down/PageDown/Space = Next, Left/Up/PageUp = Prev,
//! Enter = Menu, Escape/Backspace = Back.
//!
//! The Wi-Fi transfer screen serves its page at `http://127.0.0.1:PORT/`
//! (8080 unless `--port` is given) once `WIFI.TXT` in the state directory
//! names a network; any name will do.
//!
//! Headless, with `--keys`: presses each key in turn (`n` Next, `p` Prev,
//! `m` Menu, `b` Back, `z` the screen the device would leave for its deep
//! sleep) and writes `000.png` for the first screen, then one PNG after each
//! key, into `--out`. With `--serve`, it then keeps serving the transfer
//! page, writing `serve.png` after each change, until stopped.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::exit;
use std::time::Duration;

use inkreader_core::http::Connection;
use inkreader_core::render::{gray3, Canvas};
use inkreader_core::ui::{App, Input, Network, Storage, Upload, HEIGHT, WIDTH};
use inkreader_core::{Error, ReadAt};

struct FileBook(File, u64);

impl ReadAt for FileBook {
    fn size(&self) -> u64 {
        self.1
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.0.seek(SeekFrom::Start(offset)).map_err(|_| Error::Io)?;
        self.0.read_exact(buf).map_err(|_| Error::Io)
    }
}

struct FileUpload(File);

impl Upload for FileUpload {
    fn write(&mut self, data: &[u8]) -> Result<(), Error> {
        self.0.write_all(data).map_err(|_| Error::Io)
    }

    fn finish(self) -> Result<(), Error> {
        self.0.sync_all().map_err(|_| Error::Io)
    }
}

struct DirStorage {
    books: PathBuf,
    state: PathBuf,
}

impl Storage for DirStorage {
    type Book = FileBook;
    type Upload = FileUpload;

    fn list_books(&mut self) -> Vec<(String, u64)> {
        let mut out: Vec<_> = fs::read_dir(&self.books)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("epub")))
            .filter_map(|e| Some((e.file_name().into_string().ok()?, e.metadata().ok()?.len())))
            .collect();
        out.sort();
        out
    }

    fn open_book(&mut self, name: &str) -> Result<FileBook, Error> {
        let f = File::open(self.books.join(name)).map_err(|_| Error::Io)?;
        let len = f.metadata().map_err(|_| Error::Io)?.len();
        Ok(FileBook(f, len))
    }

    fn create_book(&mut self, name: &str) -> Result<(String, FileUpload), Error> {
        // "Name (2).epub" if the name is taken.
        let (stem, ext) = name.rsplit_once('.').unwrap_or((name, "epub"));
        let mut file = name.to_string();
        for n in 2.. {
            if !self.books.join(&file).exists() {
                break;
            }
            file = format!("{stem} ({n}).{ext}");
        }
        let f = File::create(self.books.join(&file)).map_err(|_| Error::Io)?;
        Ok((file, FileUpload(f)))
    }

    fn delete_book(&mut self, name: &str) -> Result<(), Error> {
        fs::remove_file(self.books.join(name)).map_err(|_| Error::Io)
    }

    fn load(&mut self, name: &str) -> Option<Vec<u8>> {
        fs::read(self.state.join(name)).ok()
    }

    fn save(&mut self, name: &str, data: &[u8]) -> Result<(), Error> {
        fs::create_dir_all(&self.state).map_err(|_| Error::Io)?;
        fs::write(self.state.join(name), data).map_err(|_| Error::Io)
    }

    fn remove(&mut self, name: &str) {
        let _ = fs::remove_file(self.state.join(name));
    }

    fn list_pictures(&mut self) -> Vec<String> {
        let picture = |p: &Path| {
            p.extension().and_then(|x| x.to_str()).is_some_and(|x| ["jpg", "jpeg", "png"].contains(&x.to_ascii_lowercase().as_str()))
        };
        let mut out: Vec<_> = fs::read_dir(self.books.join("SLEEP"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| picture(&e.path()))
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        out.sort();
        out
    }

    fn load_picture(&mut self, name: &str) -> Option<Vec<u8>> {
        fs::read(self.books.join("SLEEP").join(name)).ok()
    }
}

/// What the screen shows: the panel's 8 gray levels, or the full 8-bit canvas.
fn shade(v: u8, true_gray: bool) -> u8 {
    if true_gray {
        v
    } else {
        (gray3(v) as u32 * 255 / 7) as u8
    }
}

fn write_png(canvas: &Canvas, path: &Path, true_gray: bool) {
    let file = File::create(path).unwrap_or_else(|e| {
        eprintln!("can't write {}: {e}", path.display());
        exit(1);
    });
    let mut enc = png::Encoder::new(BufWriter::new(file), canvas.width as u32, canvas.height as u32);
    enc.set_color(png::ColorType::Grayscale);
    enc.set_depth(png::BitDepth::Eight);
    let px: Vec<u8> = canvas.pixels.iter().map(|&v| shade(v, true_gray)).collect();
    enc.write_header().and_then(|mut w| w.write_image_data(&px)).expect("png encoding");
}

struct Tcp(TcpStream);

impl Connection for Tcp {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
        self.0.read(buf).map_err(|_| Error::Io)
    }

    fn write(&mut self, data: &[u8]) -> Result<(), Error> {
        self.0.write_all(data).map_err(|_| Error::Io)
    }
}

/// The transfer page's server: "joins Wi-Fi" by listening on localhost.
struct Server {
    port: u16,
    listener: Option<TcpListener>,
}

impl Server {
    /// Starts or stops listening as the transfer screen opens or closes, and
    /// answers a waiting browser. Returns true if the screen changed.
    fn poll(&mut self, app: &mut App<DirStorage>) -> bool {
        if !app.transfer_open() {
            self.listener = None;
            return false;
        }
        if app.wifi_wanted().is_some() && app.network == Network::Connecting {
            app.network = match TcpListener::bind(("127.0.0.1", self.port)) {
                Ok(l) => {
                    l.set_nonblocking(true).expect("non-blocking listener");
                    self.listener = Some(l);
                    Network::Ready(format!("http://127.0.0.1:{}/", self.port))
                }
                Err(e) => Network::Failed(e.to_string()),
            };
            return true;
        }
        let Some(stream) = self.listener.as_ref().and_then(|l| l.accept().ok()) else { return false };
        let _ = stream.0.set_nonblocking(false);
        let _ = stream.0.set_read_timeout(Some(Duration::from_secs(10)));
        app.serve(&mut Tcp(stream.0))
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: inkreader-sim [--books DIR] [--state DIR] [--true-gray] [--battery PERCENT] [--port PORT] [--keys KEYS --out DIR [--serve]]"
    );
    exit(2);
}

fn main() {
    let mut books = PathBuf::from("testbooks");
    let mut state = None;
    let mut keys = None;
    let mut out = None;
    let mut true_gray = false;
    let mut battery = None;
    let mut port = 8080;
    let mut serve = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| usage());
        match a.as_str() {
            "--books" => books = PathBuf::from(value()),
            "--state" => state = Some(PathBuf::from(value())),
            "--keys" => keys = Some(value()),
            "--out" => out = Some(PathBuf::from(value())),
            "--true-gray" => true_gray = true,
            "--battery" => battery = Some(value().parse().unwrap_or_else(|_| usage())),
            "--port" => port = value().parse().unwrap_or_else(|_| usage()),
            "--serve" => serve = true,
            _ => usage(),
        }
    }
    let state = state.unwrap_or_else(|| books.join("READER"));
    let mut app = App::new(DirStorage { books, state });
    app.system = vec![("Device".into(), "Simulator".into())];
    app.battery = battery;
    app.status_battery = battery;
    app.start();
    let mut canvas = Canvas::new(WIDTH, HEIGHT);
    let mut server = Server { port, listener: None };

    if let Some(keys) = keys {
        let out = out.unwrap_or_else(|| usage());
        fs::create_dir_all(&out).expect("create --out directory");
        app.draw(&mut canvas);
        write_png(&canvas, &out.join("000.png"), true_gray);
        for (i, k) in keys.chars().enumerate() {
            let input = match k {
                'n' => Input::Next,
                'p' => Input::Prev,
                'm' => Input::Menu,
                'b' => Input::Back,
                'z' => {
                    let t = std::time::Instant::now();
                    match app.sleep_screen() {
                        Some(screen) => app.draw_sleep(&mut canvas, screen),
                        None => {
                            app.settle();
                            app.draw(&mut canvas);
                        }
                    }
                    eprintln!("sleep screen drawn in {} ms", t.elapsed().as_millis());
                    write_png(&canvas, &out.join(format!("{:03}.png", i + 1)), true_gray);
                    continue;
                }
                _ => usage(),
            };
            app.handle(input);
            app.draw(&mut canvas);
            write_png(&canvas, &out.join(format!("{:03}.png", i + 1)), true_gray);
        }
        if serve {
            loop {
                if server.poll(&mut app) {
                    app.draw(&mut canvas);
                    write_png(&canvas, &out.join("serve.png"), true_gray);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        return;
    }

    run_window(app, canvas, server, true_gray);
}

fn run_window(mut app: App<DirStorage>, mut canvas: Canvas, mut server: Server, true_gray: bool) {
    use minifb::{Key, KeyRepeat, Window, WindowOptions};

    let mut window = Window::new("oxidized-ink", WIDTH as usize, HEIGHT as usize, WindowOptions::default())
        .unwrap_or_else(|e| {
            eprintln!("can't open a window: {e}");
            exit(1);
        });
    window.set_target_fps(30);
    let mut buffer = vec![0u32; (WIDTH * HEIGHT) as usize];
    let mut dirty = true;
    while window.is_open() {
        for key in window.get_keys_pressed(KeyRepeat::Yes) {
            let input = match key {
                Key::Right | Key::Down | Key::PageDown | Key::Space => Input::Next,
                Key::Left | Key::Up | Key::PageUp => Input::Prev,
                Key::Enter => Input::Menu,
                Key::Escape | Key::Backspace => Input::Back,
                _ => continue,
            };
            dirty |= app.handle(input);
        }
        dirty |= server.poll(&mut app);
        if dirty {
            app.draw(&mut canvas);
            for (d, &v) in buffer.iter_mut().zip(&canvas.pixels) {
                let g = shade(v, true_gray) as u32;
                *d = g << 16 | g << 8 | g;
            }
            dirty = false;
        }
        window
            .update_with_buffer(&buffer, WIDTH as usize, HEIGHT as usize)
            .expect("window update");
    }
}
