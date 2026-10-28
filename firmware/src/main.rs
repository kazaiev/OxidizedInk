//! OxidizedInk firmware for the Inkplate 6 V2.
//!
//! Boot puts the board in a safe state, powers the SD card and starts the
//! reader core, which reopens the last book at its saved page. Each button
//! press goes to the core, and the panel gets a full refresh whenever the
//! screen changes. After the idle time set in Settings, the board leaves any
//! menu, cuts SD power and deep sleeps. The wake button wakes it; it boots
//! again into the same page, and that press turns the page.
//!
//! Until the side buttons exist, the wake button does everything: a short
//! press is Next, a long one (0.6 s) is Menu, a very long one (2 s) Back.

#![no_std]
#![no_main]

extern crate alloc;

mod expander;
mod panel;
mod sd;
mod storage;

use alloc::vec;
use alloc::vec::Vec;

use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{self, AnyPin, Event, InputConfig, Pin, WakeupConfig};
use esp_hal::i2c::master::{Config as I2cConfig, Error, I2c};
use esp_hal::main;
use esp_hal::rtc_cntl::sleep::{LowPower, RtcSleepConfig};
use esp_hal::rtc_cntl::wakeup_cause;
use esp_hal::time::{Duration, Instant, Rate};
use esp_hal::Blocking;
use inkreader_core::fonts::Fonts;
use inkreader_core::render::{Canvas, Renderer};
use inkreader_core::ui::{App, Input, Storage, HEIGHT, WIDTH};
use log::{info, warn};

use expander::{pin, Expander, EXP1, EXP2};
use panel::{pack_portrait, Bus, Panel, FB_SIZE};
use sd::Sd;
use storage::CardStorage;

esp_bootloader_esp_idf::esp_app_desc!();

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
}

impl Screen {
    fn new(pins: [AnyPin<'static>; 12]) -> Self {
        Screen { panel: Panel::new(pins), canvas: Canvas::new(WIDTH, HEIGHT), fb: vec![0u8; FB_SIZE] }
    }

    /// Shows the canvas with a full refresh.
    fn refresh(&mut self, bus: &mut Bus) {
        pack_portrait(&self.canvas, &mut self.fb);
        match self.panel.show(bus, &self.fb) {
            Ok(t) => info!(
                "refresh: power on {} ms, clean {} ms, image {} ms, power off {} ms",
                t.power_on, t.clean, t.frames, t.power_off
            ),
            Err(e) => warn!("refresh failed, panel powered off: {:?}", e),
        }
    }

    fn show_app<S: Storage>(&mut self, app: &mut App<S>, bus: &mut Bus) {
        let t = Instant::now();
        app.draw(&mut self.canvas);
        info!("drawn in {} ms", t.elapsed().as_millis());
        self.refresh(bus);
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
        self.refresh(bus);
    }
}

/// Waits for a press of the wake button and turns its length into an
/// input, or returns `None` after `idle` without one.
fn wait_input(button: &gpio::Input, idle: Duration) -> Option<Input> {
    let delay = Delay::new();
    let start = Instant::now();
    // First let go of a press that is still held, such as the one that woke
    // the board.
    while button.is_low() {
        delay.delay_millis(10);
    }
    loop {
        if button.is_low() {
            let down = Instant::now();
            while button.is_low() {
                delay.delay_millis(10);
            }
            let held = down.elapsed().as_millis();
            if held >= 30 {
                return Some(match held {
                    0..600 => Input::Next,
                    600..2000 => Input::Menu,
                    _ => Input::Back,
                });
            }
        }
        if start.elapsed() > idle {
            return None;
        }
        delay.delay_millis(10);
    }
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
    info!("oxidized-ink starting{}", if woke { " (woken by the button)" } else { "" });

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
    let mut button = gpio::Input::new(p.GPIO36, InputConfig::default());

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
            info!("SD card: {} MB", sd.size / 1_000_000);
            let mut app = App::new(CardStorage::new(sd));
            app.start();
            info!("reader started in {} ms", boot.elapsed().as_millis());
            // After a wake the panel still shows what `settle` left there,
            // and the press that woke the board turns the page.
            if !woke || app.handle(Input::Next) {
                screen.show_app(&mut app, &mut bus);
            }
            loop {
                let idle = Duration::from_secs(app.settings().sleep_minutes() as u64 * 60);
                let Some(input) = wait_input(&button, idle) else { break };
                info!("button: {:?}", input);
                if app.handle(input) {
                    screen.show_app(&mut app, &mut bus);
                }
            }
            if app.settle() {
                screen.show_app(&mut app, &mut bus);
            }
        }
        None => {
            screen.message(&mut bus, &["No SD card found.", "Insert one with .epub books,", "then press the wake button."]);
        }
    }

    info!("going to sleep");
    let _ = bus.exp1.set_input(&mut bus.i2c, pin::SD_POWER, false);
    button.listen(Event::LowLevel);
    if let Err(e) = button.apply_wakeup_config(&WakeupConfig::default().with_low_power_path(true)) {
        warn!("can't arm the wake button: {:?}", e);
    }
    LowPower::new(p.LPWR).sleep_deep(RtcSleepConfig::deep())
}
