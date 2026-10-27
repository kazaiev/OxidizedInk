//! PC simulator: runs the reader core against a directory of books.
//!
//! ```text
//! inkreader-sim [--books DIR] [--state DIR] [--true-gray] [--keys KEYS --out DIR]
//! ```
//!
//! `--books` defaults to `testbooks/`; `--state` (the card's `/READER/`)
//! defaults to `READER/` inside it. The screen is shown in the panel's 8 gray
//! levels unless `--true-gray` is given.
//!
//! In a window: Right/Down/PageDown/Space = Next, Left/Up/PageUp = Prev,
//! Enter = Menu, Escape/Backspace = Back.
//!
//! Headless, with `--keys`: presses each key in turn (`n` Next, `p` Prev,
//! `m` Menu, `b` Back) and writes `000.png` for the first screen, then one
//! PNG after each key, into `--out`.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::exit;

use inkreader_core::render::{gray3, Canvas};
use inkreader_core::ui::{App, Input, Storage, HEIGHT, WIDTH};
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

struct DirStorage {
    books: PathBuf,
    state: PathBuf,
}

impl Storage for DirStorage {
    type Book = FileBook;

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

    fn load(&mut self, name: &str) -> Option<Vec<u8>> {
        fs::read(self.state.join(name)).ok()
    }

    fn save(&mut self, name: &str, data: &[u8]) -> Result<(), Error> {
        fs::create_dir_all(&self.state).map_err(|_| Error::Io)?;
        fs::write(self.state.join(name), data).map_err(|_| Error::Io)
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

fn usage() -> ! {
    eprintln!("usage: inkreader-sim [--books DIR] [--state DIR] [--true-gray] [--keys KEYS --out DIR]");
    exit(2);
}

fn main() {
    let mut books = PathBuf::from("testbooks");
    let mut state = None;
    let mut keys = None;
    let mut out = None;
    let mut true_gray = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| usage());
        match a.as_str() {
            "--books" => books = PathBuf::from(value()),
            "--state" => state = Some(PathBuf::from(value())),
            "--keys" => keys = Some(value()),
            "--out" => out = Some(PathBuf::from(value())),
            "--true-gray" => true_gray = true,
            _ => usage(),
        }
    }
    let state = state.unwrap_or_else(|| books.join("READER"));
    let mut app = App::new(DirStorage { books, state });
    app.start();
    let mut canvas = Canvas::new(WIDTH, HEIGHT);

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
                _ => usage(),
            };
            app.handle(input);
            app.draw(&mut canvas);
            write_png(&canvas, &out.join(format!("{:03}.png", i + 1)), true_gray);
        }
        return;
    }

    run_window(app, canvas, true_gray);
}

fn run_window(mut app: App<DirStorage>, mut canvas: Canvas, true_gray: bool) {
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
