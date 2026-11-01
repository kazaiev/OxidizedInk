//! OxidizedInk firmware for the Inkplate 6 V2.
//!
//! Boot puts the board in a safe state, shows the logo (after power-on, not
//! after a sleep), powers the SD card and starts the reader core, which
//! reopens the last book at its saved page. Each button press goes to the
//! core, and the panel gets a partial refresh whenever the screen changes,
//! and a full one every few changes (the "Full refresh" setting). After
//! the idle time set in Settings, the board leaves any menu, puts up the
//! sleep screen (the book's cover, a picture from the card, the logo, or
//! "Battery low"; or it leaves the page), cuts SD power and deep sleeps.
//! Any button wakes it; it boots again into the same page, and if the page
//! was left up, the press that woke it acts on it. On an empty battery it
//! shows "Battery empty" and sleeps again without opening the book.
//!
//! The page buttons on the side give Prev, Next and Menu, and holding Menu
//! gives Back. The board's own wake button is not used (see `buttons`).

#![no_std]
#![no_main]

extern crate alloc;

mod battery;
mod buttons;
mod expander;
mod panel;
mod sd;
mod storage;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use esp_alloc::{MemoryCapability, HEAP};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{self, AnyPin, InputConfig, Pin};
use esp_hal::i2c::master::{Config as I2cConfig, Error, I2c};
use esp_hal::main;
use esp_hal::rtc_cntl::wakeup_cause;
use esp_hal::time::{Duration, Instant, Rate};
use esp_hal::Blocking;
use inkreader_core::fonts::Fonts;
use inkreader_core::render::{Canvas, Renderer};
use inkreader_core::settings::Settings;
use inkreader_core::ui::{draw_logo, App, SleepScreen, Storage, HEIGHT, WIDTH};
use log::{info, warn};

use battery::Battery;
use buttons::Buttons;
use expander::{pin, Expander, EXP1, EXP2};
use panel::{pack_portrait, Bus, Panel, Refresh, FB_SIZE};
use sd::Sd;
use storage::CardStorage;

esp_bootloader_esp_idf::esp_app_desc!();

/// What the panel showed when the board went into deep sleep, kept in RTC
/// memory: `SHOWS_PAGE`, `SHOWS_EMPTY`, or anything else for something else
/// or unknown (memory that is not kept starts with random bytes).
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut PANEL_AT_SLEEP: u8 = 0;
/// The settled page, as `App::draw` draws it after `start`.
const SHOWS_PAGE: u8 = 0xa5;
/// The "Battery empty" screen.
const SHOWS_EMPTY: u8 = 0x5a;
/// With `SHOWS_PAGE`: the charge the page's status line showed, or 0xff for
/// none. The page drawn after the wake must match the panel.
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut STATUS_BATTERY_AT_SLEEP: u8 = 0xff;

/// Runs from the panic handler before the backtrace is printed.
#[no_mangle]
fn custom_pre_backtrace() {
    panel::emergency_off();
}

/// The panel with the buffers that feed it.
struct Screen {
    panel: Panel,
    canvas: Canvas,
    fb: Vec<u8>,
    /// The framebuffer the panel shows, if known, for partial refreshes.
    /// `diff` overwrites it in place, and a successful refresh restores it.
    shown: Vec<u8>,
    shown_known: bool,
    /// Partial refreshes since the last full one.
    partials: u32,
}

impl Screen {
    fn new(pins: [AnyPin<'static>; 12]) -> Self {
        Screen {
            panel: Panel::new(pins),
            canvas: Canvas::new(WIDTH, HEIGHT),
            fb: vec![0u8; FB_SIZE],
            shown: vec![0u8; FB_SIZE],
            shown_known: false,
            partials: 0,
        }
    }

    /// Shows the canvas. One refresh in `full_every` is a full one; the
    /// others change only the pixels that differ from what the panel shows.
    fn refresh(&mut self, bus: &mut Bus, full_every: u32) {
        pack_portrait(&self.canvas, &mut self.fb);
        let mut refresh = Refresh::Full;
        let mut buf = &self.fb;
        if !self.next_is_full(full_every) {
            let changed = panel::diff(&mut self.shown, &self.fb);
            info!("partial refresh: {} pixels change", changed);
            if changed == 0 {
                self.shown.copy_from_slice(&self.fb);
                return;
            }
            refresh = Refresh::Partial;
            buf = &self.shown;
        }
        match self.panel.show(bus, buf, refresh) {
            Ok(t) => {
                info!(
                    "refresh: power on {} ms, clean {} ms, image {} ms, power off {} ms",
                    t.power_on, t.clean, t.frames, t.power_off
                );
                self.partials = if refresh == Refresh::Partial { self.partials + 1 } else { 0 };
                self.shown.copy_from_slice(&self.fb);
                self.shown_known = true;
            }
            Err(e) => {
                warn!("refresh failed, panel powered off: {:?}", e);
                self.shown_known = false;
            }
        }
    }

    /// Whether the next `refresh` will be a full one.
    fn next_is_full(&self, full_every: u32) -> bool {
        !self.shown_known || self.partials + 1 >= full_every
    }

    fn show_app<S: Storage>(&mut self, app: &mut App<S>, bus: &mut Bus) {
        let full_every = app.settings().full_refresh_every();
        // The status line's battery changes only with a full refresh, so a
        // partial one never redraws it for a reading 1% off the last.
        if self.next_is_full(full_every) {
            app.status_battery = app.battery;
        }
        let t = Instant::now();
        app.draw(&mut self.canvas);
        info!("drawn in {} ms, {} KB free", t.elapsed().as_millis(), HEAP.free() / 1024);
        self.refresh(bus, full_every);
    }

    /// A sleep screen, with a full refresh.
    fn show_sleep<S: Storage>(&mut self, app: &mut App<S>, sleep: SleepScreen, bus: &mut Bus) {
        let t = Instant::now();
        app.draw_sleep(&mut self.canvas, sleep);
        info!("{:?} sleep screen drawn in {} ms, {} KB free", sleep, t.elapsed().as_millis(), HEAP.free() / 1024);
        self.refresh(bus, 1);
    }

    /// The logo, while the reader starts. The screen after it gets a full
    /// refresh, so that no trace of the logo stays.
    fn show_logo(&mut self, bus: &mut Bus) {
        let t = Instant::now();
        draw_logo(&mut self.canvas);
        info!("logo drawn in {} ms", t.elapsed().as_millis());
        self.refresh(bus, 1);
        self.shown_known = false;
    }

    /// Takes the app's screen as what the panel already shows, without a
    /// refresh. After a wake, the panel still shows what `settle` drew
    /// before the sleep, so the first refresh can be a partial one.
    fn assume_shown<S: Storage>(&mut self, app: &mut App<S>) {
        app.draw(&mut self.canvas);
        pack_portrait(&self.canvas, &mut self.shown);
        self.shown_known = true;
    }

    /// Centered lines of text, for when there is no reader to draw.
    fn message(&mut self, bus: &mut Bus, lines: &[&str]) {
        let fonts = Fonts::new();
        let mut r = Renderer::new();
        self.canvas.clear();
        for (i, line) in lines.iter().enumerate() {
            let w = fonts.to_fx(fonts.measure(0, line), 24);
            r.draw_str(&fonts, &mut self.canvas, 0, 24, (WIDTH * 64 - w) / 2, HEIGHT / 2 + i as i32 * 40, line);
        }
        self.refresh(bus, 1);
    }
}

/// A charge to show instead of the real one, set when building with
/// `TEST_BATTERY=12 cargo build --release`, to try the low and empty battery
/// screens on a charged cell.
fn test_battery() -> Option<u8> {
    option_env!("TEST_BATTERY").and_then(|v| v.parse().ok())
}

/// The idle time before sleep, in seconds: `TEST_IDLE` at build time, to
/// try the sleep screens without waiting, or else the setting.
fn idle_time(settings: &Settings) -> Duration {
    let test = option_env!("TEST_IDLE").and_then(|v| v.parse().ok());
    Duration::from_secs(test.unwrap_or(settings.sleep_minutes() as u64 * 60))
}

/// Reads the battery, logs it, and refreshes what the app shows about the
/// device: the charge in the menu and the rows of System info.
fn update_status<S: Storage>(battery: &mut Battery, bus: &mut Bus, app: &mut App<S>, card_mb: u64) {
    let row = |label: &str, value: String| (String::from(label), value);
    app.system.clear();
    match battery.read(bus) {
        Some(r) => {
            let pct = battery::percent(r.mv);
            info!("battery: {} mV (raw {}), {}%", r.mv, r.raw, pct);
            app.battery = Some(test_battery().unwrap_or(pct));
            app.system.push(row("Battery", format!("{} mV, {}%", r.mv, pct)));
            app.system.push(row("Battery ADC", format!("raw {}, ref {} mV", r.raw, battery.vref)));
        }
        None => {
            app.battery = None;
            app.system.push(row("Battery", String::from("can't read")));
        }
    }
    app.system.push(row("SD card", format!("{} MB", card_mb)));
    let internal = HEAP.free_caps(MemoryCapability::Internal.into());
    let psram = HEAP.free() - internal;
    app.system.push(row("Free memory", format!("{} KB + {} KB PSRAM", internal / 1024, psram / 1024)));
    app.system.push(row("Firmware", String::from(env!("CARGO_PKG_VERSION"))));
}

/// Expander 1 into a safe state. VCOM and PWRUP go low before WAKEUP, as in
/// the PMIC's power-down order, in case the last firmware left it on.
fn safe_state(exp1: &mut Expander, i2c: &mut I2c<'_, Blocking>) -> Result<(), Error> {
    exp1.set_output(i2c, pin::VCOM_CTRL, false)?;
    exp1.set_output(i2c, pin::PWRUP, false)?;
    Delay::new().delay_millis(50);
    exp1.set_output(i2c, pin::WAKEUP, false)?;
    for p in [pin::OE, pin::GMOD, pin::SPV] {
        exp1.set_input(i2c, p, false)?;
    }
    exp1.set_output(i2c, pin::BATTERY, false)?;
    exp1.set_input(i2c, pin::SD_POWER, false)?;
    for p in pin::FREE {
        exp1.set_input(i2c, p, true)?;
    }
    Ok(())
}

#[main]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let boot = Instant::now();
    let delay = Delay::new();
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 98768);
    esp_alloc::psram_allocator!(p.PSRAM, esp_hal::psram);
    let woke = !wakeup_cause().is_empty();
    info!("oxidized-ink starting{}", if woke { " (woken from sleep)" } else { "" });

    let mut i2c = I2c::new(p.I2C0, I2cConfig::default().with_frequency(Rate::from_khz(100)))
        .expect("I2C config")
        .with_sda(p.GPIO21)
        .with_scl(p.GPIO22);
    let (Ok(mut exp1), Ok(mut exp2)) = (Expander::new(&mut i2c, EXP1), Expander::new(&mut i2c, EXP2)) else {
        warn!("no PCAL6416A expanders at 0x20 and 0x21: not an Inkplate 6 V2. Stopping.");
        loop {
            delay.delay_millis(1000);
        }
    };
    if let Err(e) = safe_state(&mut exp1, &mut i2c) {
        warn!("expander 1 setup failed: {:?}", e);
    }
    for p in 0..16 {
        let _ = exp2.set_input(&mut i2c, p, true);
    }
    let mut bus = Bus { i2c, exp1 };

    let mut screen = Screen::new([
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
    let expander_int = gpio::Input::new(p.GPIO34, InputConfig::default());
    let mut buttons = Buttons::new(expander_int, p.LPWR, &mut bus);
    let wake_press = buttons.wake_press(&mut bus);
    if woke {
        info!("woken by {:?}", wake_press);
    }
    let mut battery = Battery::new(p.ADC1, p.GPIO35);
    info!("ADC reference: {} mV", battery.vref);
    if !woke {
        screen.show_logo(&mut bus);
    }

    let card = match bus.exp1.set_output(&mut bus.i2c, pin::SD_POWER, false) {
        Ok(()) => {
            delay.delay_millis(50);
            Sd::new(p.SPI2, p.GPIO14, p.GPIO12, p.GPIO13, p.GPIO15).map_err(|e| warn!("SD card: {:?}", e)).ok()
        }
        Err(e) => {
            warn!("can't power the SD slot: {:?}", e);
            None
        }
    };

    match card {
        Some(sd) => {
            let card_mb = sd.size / 1_000_000;
            info!("SD card: {} MB", card_mb);
            let mut app = App::new(CardStorage::new(sd));
            update_status(&mut battery, &mut bus, &mut app, card_mb);
            let at_sleep = if woke { unsafe { PANEL_AT_SLEEP } } else { 0 };
            unsafe { PANEL_AT_SLEEP = 0 };
            let shows = if app.battery_empty() {
                // Too low to read. Say so, unless the panel already does.
                info!("battery empty");
                if at_sleep != SHOWS_EMPTY {
                    screen.show_sleep(&mut app, SleepScreen::BatteryEmpty, &mut bus);
                }
                SHOWS_EMPTY
            } else {
                app.start();
                info!("reader started in {} ms", boot.elapsed().as_millis());
                // If the panel still shows the page `settle` left there, the
                // first refresh can be a partial one, and the press that woke
                // the board acts on the page. After a sleep screen it just
                // brings the page back.
                let mut changed = false;
                if at_sleep == SHOWS_PAGE {
                    let b = unsafe { STATUS_BATTERY_AT_SLEEP };
                    app.status_battery = (b <= 100).then_some(b);
                    screen.assume_shown(&mut app);
                    changed = wake_press.is_some_and(|i| app.handle(i));
                }
                if at_sleep != SHOWS_PAGE || changed {
                    screen.show_app(&mut app, &mut bus);
                }
                while let Some(input) = buttons.wait(&mut bus, idle_time(app.settings())) {
                    update_status(&mut battery, &mut bus, &mut app, card_mb);
                    if app.battery_empty() {
                        break;
                    }
                    let t = Instant::now();
                    let changed = app.handle(input);
                    info!("button: {:?}, handled in {} ms", input, t.elapsed().as_millis());
                    if changed {
                        screen.show_app(&mut app, &mut bus);
                    }
                }
                let sleep = app.sleep_screen();
                if let Some(s) = sleep {
                    screen.show_sleep(&mut app, s, &mut bus);
                }
                match sleep {
                    Some(SleepScreen::BatteryEmpty) => SHOWS_EMPTY,
                    Some(_) => 0,
                    None => {
                        if app.settle() {
                            screen.show_app(&mut app, &mut bus);
                        }
                        unsafe { STATUS_BATTERY_AT_SLEEP = app.status_battery.unwrap_or(0xff) };
                        SHOWS_PAGE
                    }
                }
            };
            unsafe { PANEL_AT_SLEEP = shows };
        }
        None => {
            screen.message(&mut bus, &["No SD card found.", "Insert one with .epub books,", "then press any button."]);
        }
    }

    info!("going to sleep");
    let _ = bus.exp1.set_input(&mut bus.i2c, pin::SD_POWER, false);
    buttons.deep_sleep(&mut bus)
}
