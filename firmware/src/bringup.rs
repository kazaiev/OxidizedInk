//! Inkplate 6 bring-up program, kept for checking the board:
//! `cargo run --release --bin bringup`.
//!
//! 1. Logs memory, the chips on the I2C bus and which board this is, then
//!    puts expander 1 in a safe state: PMIC lines low, SD and battery
//!    divider off, pull-ups on the free pins.
//! 2. Powers the SD card and lists it. With `BENCHMARK`, also times the
//!    reader core on every EPUB in the root.
//! 3. Shows a test pattern on the panel, then the smallest book on the
//!    card: each press of the wake button shows the next page.

#![no_std]
#![no_main]
// Shares modules with the firmware; not all of them are used here.
#![allow(dead_code)]

extern crate alloc;

mod expander;
mod panel;
mod sd;

use alloc::vec;
use alloc::vec::Vec;

use esp_backtrace as _;
use esp_hal::analog::adc::{Adc, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Input, InputConfig, Pin};
use esp_hal::i2c::master::{Config as I2cConfig, Error, I2c};
use esp_hal::time::{Instant, Rate};
use esp_hal::main;
use inkreader_core::epub::Epub;
use inkreader_core::fonts::Fonts;
use inkreader_core::layout::{Layout, Pos};
use inkreader_core::render::{Canvas, Renderer, BLACK};
use inkreader_core::text::{Chapter, BOLD, ITALIC};
use log::{info, warn};

use expander::{pin, Expander, EXP1, EXP2};
use panel::{pack_portrait, Bus, Panel, FB_SIZE};
use sd::{Book, Entry, Sd};

esp_bootloader_esp_idf::esp_app_desc!();

/// Time the core on every book at startup (takes several seconds).
const BENCHMARK: bool = false;

const RTC: u8 = 0x51;

/// Runs from the panic handler before the backtrace is printed.
#[no_mangle]
fn custom_pre_backtrace() {
    panel::emergency_off();
}

fn name(addr: u8) -> &'static str {
    match addr {
        EXP1 => "I/O expander 1",
        EXP2 => "I/O expander 2 (V2)",
        0x22 => "I/O expander 2 (V1)",
        0x48 => "TPS65186 PMIC",
        RTC => "PCF85063A RTC (V2)",
        _ => "unknown",
    }
}

fn bcd(b: u8) -> u8 {
    (b >> 4) * 10 + (b & 0x0f)
}

fn ms(since: Instant) -> u64 {
    since.elapsed().as_millis()
}

fn kb(bytes: usize) -> usize {
    bytes / 1024
}

/// The reading view's geometry at the default settings.
fn layout(fonts: &Fonts) -> Layout<'_> {
    Layout {
        fonts,
        left: 36,
        top: 28,
        width: 528,
        height: 724,
        font_px: 24,
        line_pct: 140,
        justify: true,
        spaced: false,
    }
}

/// Opens one book and times the core's work on it: the first chapter with
/// text (what opening a book costs) and the largest one (the worst case for
/// a chapter change).
fn benchmark_book(sd: &Sd, e: &Entry, fonts: &Fonts, canvas: &mut Canvas, renderer: &mut Renderer) {
    info!("{} ({} KB)", e.name(), e.size / 1024);
    let t = Instant::now();
    let file = match sd.open("", &e.short) {
        Ok(f) => f,
        Err(err) => return warn!("  can't open: {:?}", err),
    };
    let mut epub = match Epub::open(Book(file)) {
        Ok(b) => b,
        Err(err) => return warn!("  not a readable EPUB: {:?}", err),
    };
    info!(
        "  \"{}\" by {}: {} spine items, {} TOC entries, opened in {} ms",
        epub.title,
        epub.author,
        epub.spine.len(),
        epub.toc.len(),
        ms(t)
    );
    let largest = (0..epub.spine.len()).max_by_key(|&i| epub.spine[i].size).unwrap_or(0);
    let first = (0..epub.spine.len()).find(|&i| epub.chapter(i).is_ok_and(|c| !c.blocks.is_empty()));
    let mut picks = Vec::new();
    picks.extend(first);
    if Some(largest) != first {
        picks.push(largest);
    }
    let layout = layout(fonts);
    for i in picks {
        let t = Instant::now();
        let ch = match epub.chapter(i) {
            Ok(c) => c,
            Err(err) => {
                warn!("  chapter {}: {:?}", i, err);
                continue;
            }
        };
        let parse = ms(t);
        let t = Instant::now();
        let starts = layout.paginate(&ch);
        let paginate = ms(t);
        let t = Instant::now();
        let page = layout.page(&ch, starts[0]);
        canvas.clear();
        renderer.draw_page(fonts, canvas, &ch, &page);
        let draw = ms(t);
        let chars: usize = ch.blocks.iter().map(|b| b.text.len()).sum();
        info!(
            "  chapter {} ({} KB of XHTML, {}k chars): parsed in {} ms, {} pages laid out in {} ms, page 1 drawn in {} ms; heap {} KB used",
            i,
            epub.spine[i].size / 1024,
            chars / 1000,
            parse,
            starts.len(),
            paginate,
            draw,
            kb(esp_alloc::HEAP.used())
        );
    }
}

fn text(r: &mut Renderer, fonts: &Fonts, canvas: &mut Canvas, face: u8, px: i32, x: i32, y: i32, s: &str) {
    r.draw_str(fonts, canvas, face, px, x * 64, y, s);
}

/// A frame around the edge, a title, and the panel's 8 gray levels as bars.
fn test_pattern(r: &mut Renderer, fonts: &Fonts, canvas: &mut Canvas) {
    canvas.clear();
    let (w, h) = (canvas.width, canvas.height);
    for (x, y, rw, rh) in [(0, 0, w, 3), (0, h - 3, w, 3), (0, 0, 3, h), (w - 3, 0, 3, h)] {
        canvas.fill_rect(x, y, rw, rh, BLACK);
    }
    text(r, fonts, canvas, BOLD, 44, 36, 84, "oxidized-ink");
    text(r, fonts, canvas, 0, 24, 36, 128, "Panel test: the 8 gray levels.");
    text(r, fonts, canvas, ITALIC, 20, 36, 162, "This edge is the top of the page.");
    for level in 0..8 {
        let y = 196 + level * 64;
        text(r, fonts, canvas, BOLD, 28, 40, y + 40, &alloc::format!("{level}"));
        canvas.fill_rect(84, y, 476, 52, (level * 255 / 7) as u8);
    }
    text(r, fonts, canvas, ITALIC, 20, 36, 754, "Press the wake button to open a book.");
}

/// The smallest EPUB on the card, paged with the wake button.
struct Reader {
    epub: Epub<Book>,
    spine: usize,
    chapter: Chapter,
    starts: Vec<Pos>,
    page: usize,
}

impl Reader {
    fn open(sd: &Sd, e: &Entry) -> Option<Self> {
        let file = sd.open("", &e.short).ok()?;
        let epub = Epub::open(Book(file)).ok()?;
        Some(Reader { epub, spine: 0, chapter: Chapter::default(), starts: Vec::new(), page: 0 })
    }

    /// Moves to the next spine item with text, from `from`. False at the end.
    fn load(&mut self, layout: &Layout, from: usize) -> bool {
        for i in from..self.epub.spine.len() {
            if let Ok(ch) = self.epub.chapter(i) {
                if !ch.blocks.is_empty() {
                    self.starts = layout.paginate(&ch);
                    self.chapter = ch;
                    self.spine = i;
                    self.page = 0;
                    return true;
                }
            }
        }
        false
    }

    fn next(&mut self, layout: &Layout) {
        if self.page + 1 < self.starts.len() {
            self.page += 1;
        } else if !self.load(layout, self.spine + 1) {
            self.load(layout, 0);
        }
    }

    fn draw(&self, layout: &Layout, r: &mut Renderer, fonts: &Fonts, canvas: &mut Canvas) {
        canvas.clear();
        let page = layout.page(&self.chapter, self.starts[self.page]);
        r.draw_page(fonts, canvas, &self.chapter, &page);
        let status = alloc::format!("{}  ·  {} / {}", self.epub.title, self.page + 1, self.starts.len());
        text(r, fonts, canvas, ITALIC, 16, 36, 784, &status);
    }
}

#[main]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let delay = Delay::new();

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 98768);
    esp_alloc::psram_allocator!(p.PSRAM, esp_hal::psram);
    info!("oxidized-ink bring-up, steps 1-3");
    info!("heap: {} KB free", kb(esp_alloc::HEAP.free()));

    let mut i2c = I2c::new(p.I2C0, I2cConfig::default().with_frequency(Rate::from_khz(100)))
        .expect("I2C config")
        .with_sda(p.GPIO21)
        .with_scl(p.GPIO22);

    let mut found = [false; 128];
    for addr in 0x08u8..0x78 {
        let mut b = [0u8];
        if i2c.read(addr, &mut b).is_ok() {
            found[addr as usize] = true;
            info!("I2C {:#04x}: {}", addr, name(addr));
        }
    }
    if !found[EXP1 as usize] || !found[EXP2 as usize] {
        warn!("not an Inkplate 6 V2 (no expanders at 0x20 and 0x21); stopping here");
        loop {
            delay.delay_millis(1000);
        }
    }

    // Expander 1 into a safe state. VCOM and PWRUP go low before WAKEUP, as
    // in the PMIC's power-down order, in case the last firmware left it on.
    let mut exp1 = Expander::new(&mut i2c, EXP1).expect("expander 1");
    let safe = (|| {
        exp1.set_output(&mut i2c, pin::VCOM_CTRL, false)?;
        exp1.set_output(&mut i2c, pin::PWRUP, false)?;
        delay.delay_millis(50);
        exp1.set_output(&mut i2c, pin::WAKEUP, false)?;
        for p in [pin::OE, pin::GMOD, pin::SPV] {
            exp1.set_input(&mut i2c, p, false)?;
        }
        exp1.set_output(&mut i2c, pin::BATTERY, false)?;
        exp1.set_input(&mut i2c, pin::SD_POWER, false)?;
        for p in pin::FREE {
            exp1.set_input(&mut i2c, p, true)?;
        }
        Ok::<(), Error>(())
    })();
    if let Err(e) = safe {
        warn!("expander 1 setup failed: {:?}", e);
    }
    let mut exp2 = Expander::new(&mut i2c, EXP2).expect("expander 2");
    for p in 0..16 {
        let _ = exp2.set_input(&mut i2c, p, true);
    }

    let mut t = [0u8; 7];
    if i2c.write_read(RTC, &[0x04], &mut t).is_ok() {
        info!(
            "RTC: 20{:02}-{:02}-{:02} {:02}:{:02}:{:02}{}",
            bcd(t[6]),
            bcd(t[5] & 0x1f),
            bcd(t[3] & 0x3f),
            bcd(t[2] & 0x3f),
            bcd(t[1] & 0x7f),
            bcd(t[0] & 0x7f),
            if t[0] & 0x80 != 0 { " (not set)" } else { "" }
        );
    }

    let mut adc_cfg = AdcConfig::new();
    let mut bat_pin = adc_cfg.enable_pin(p.GPIO35, Attenuation::_11dB);
    let mut adc = Adc::new(p.ADC1, adc_cfg);
    if exp1.set_output(&mut i2c, pin::BATTERY, true).is_ok() {
        delay.delay_millis(5);
        let raw = nb::block!(adc.read_oneshot(&mut bat_pin)).unwrap_or(0);
        let _ = exp1.set_output(&mut i2c, pin::BATTERY, false);
        info!("battery ADC: {} (uncalibrated)", raw);
    }

    let mut bus = Bus { i2c, exp1 };
    let fonts = Fonts::new();
    let mut renderer = Renderer::new();
    let mut canvas = Canvas::new(600, 800);
    let mut fb = vec![0u8; FB_SIZE];

    // SD card.
    let mut sd = None;
    let mut entries = Vec::new();
    match bus.exp1.set_output(&mut bus.i2c, pin::SD_POWER, false) {
        Ok(()) => {
            delay.delay_millis(50);
            match Sd::new(p.SPI2, p.GPIO14, p.GPIO12, p.GPIO13, p.GPIO15) {
                Ok(card) => {
                    info!("SD card: {} MB", card.size / 1_000_000);
                    entries = card.list("").unwrap_or_default();
                    for e in &entries {
                        info!("  {:>9}  {}", e.size, e.name());
                    }
                    sd = Some(card);
                }
                Err(err) => warn!("SD card: {:?}", err),
            }
        }
        Err(err) => warn!("can't power the SD slot: {:?}", err),
    }
    let books: Vec<&Entry> =
        entries.iter().filter(|e| !e.dir && e.short.extension().eq_ignore_ascii_case(b"EPU")).collect();
    if BENCHMARK {
        if let Some(sd) = &sd {
            for e in &books {
                benchmark_book(sd, e, &fonts, &mut canvas, &mut renderer);
            }
        }
    }

    // Panel: the test pattern.
    let mut panel = Panel::new([
        p.GPIO4.degrade(),
        p.GPIO5.degrade(),
        p.GPIO18.degrade(),
        p.GPIO19.degrade(),
        p.GPIO23.degrade(),
        p.GPIO25.degrade(),
        p.GPIO26.degrade(),
        p.GPIO27.degrade(),
        p.GPIO0.degrade(),
        p.GPIO2.degrade(),
        p.GPIO32.degrade(),
        p.GPIO33.degrade(),
    ]);
    let mut show = |canvas: &Canvas, fb: &mut [u8], bus: &mut Bus| {
        let t = Instant::now();
        pack_portrait(canvas, fb);
        let pack = ms(t);
        match panel.show(bus, fb) {
            Ok(t) => info!(
                "refresh: pack {} ms, power on {} ms, clean {} ms (77 frames), image {} ms (11 frames), power off {} ms",
                pack, t.power_on, t.clean, t.frames, t.power_off
            ),
            Err(e) => warn!("refresh failed, panel powered off: {:?}", e),
        }
    };
    test_pattern(&mut renderer, &fonts, &mut canvas);
    show(&canvas, &mut fb, &mut bus);

    let wake = Input::new(p.GPIO36, InputConfig::default());
    let wait_press = || {
        while wake.is_low() {
            delay.delay_millis(20);
        }
        while wake.is_high() {
            delay.delay_millis(20);
        }
    };

    let smallest = books.iter().min_by_key(|e| e.size);
    let reader = match (&sd, smallest) {
        (Some(sd), Some(e)) => Reader::open(sd, e),
        _ => None,
    };
    let layout = layout(&fonts);
    let reader = reader.and_then(|mut r| r.load(&layout, 0).then_some(r));
    match reader {
        Some(mut r) => {
            info!("ready: \"{}\". Each wake button press shows the next page.", r.epub.title);
            loop {
                wait_press();
                let t = Instant::now();
                r.draw(&layout, &mut renderer, &fonts, &mut canvas);
                info!("page {} of {} in spine item {}, drawn in {} ms", r.page + 1, r.starts.len(), r.spine, ms(t));
                show(&canvas, &mut fb, &mut bus);
                r.next(&layout);
            }
        }
        _ => {
            warn!("no readable EPUB on the card");
            loop {
                delay.delay_millis(1000);
            }
        }
    }
}
