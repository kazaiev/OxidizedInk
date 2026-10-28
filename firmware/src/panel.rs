//! The ED060SC7 panel (800×600, landscape), driven directly over GPIO, and
//! the TPS65186 PMIC that powers it. Sequences, timing and the V2 waveform
//! follow docs/HARDWARE.md.
//!
//! The panel must never be left powered: `show` always powers down, also on
//! errors, and `emergency_off` does it from the panic handler.

use core::ptr::write_volatile;
use core::sync::atomic::Ordering;

use esp_hal::delay::Delay;
use esp_hal::gpio::{AnyPin, Flex, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, Error, I2c};
use esp_hal::peripherals::{GPIO21, GPIO22, I2C0};
use esp_hal::time::{Duration, Instant, Rate};
use esp_hal::Blocking;
use inkreader_core::render::{gray3, Canvas};

use crate::expander::{pin, Expander, CONFIG, EXP1, EXP1_CONFIG, EXP1_OUTPUT, OUTPUT};

pub const WIDTH: usize = 800;
pub const HEIGHT: usize = 600;
/// Framebuffer size: 4 bits per pixel, `fb[400 * y + x / 2]`, even x in the
/// high nibble; 0 = black ..= 7 = white.
pub const FB_SIZE: usize = WIDTH * HEIGHT / 2;
const ROW_BYTES: usize = WIDTH / 4;

// GPIO registers. CL, LE and the data lines are in OUT (GPIO 0-31); CKV and
// SPH are GPIO32 and 33, in OUT1.
const OUT_W1TS: *mut u32 = 0x3ff4_4008 as *mut u32;
const OUT_W1TC: *mut u32 = 0x3ff4_400c as *mut u32;
const OUT1_W1TS: *mut u32 = 0x3ff4_4014 as *mut u32;
const OUT1_W1TC: *mut u32 = 0x3ff4_4018 as *mut u32;
const DATA: u32 = 0x0e8c_0030;
const CL: u32 = 1 << 0;
const LE: u32 = 1 << 2;
const CKV: u32 = 1 << 0;
const SPH: u32 = 1 << 1;

const PMIC: u8 = 0x48;
const PMIC_ENABLE: u8 = 0x01;
const PMIC_UPSEQ0: u8 = 0x09;
const PMIC_DWNSEQ0: u8 = 0x0b;
const PMIC_PG: u8 = 0x0f;
const PG_ALL_GOOD: u8 = 0xfa;

/// Bytes sent for a whole frame by `clean`: 2 bits per pixel.
const LIGHTEN: u8 = 0xaa;
const DARKEN: u8 = 0x55;
const DISCHARGE: u8 = 0x00;
const SKIP: u8 = 0xff;

/// Before every full refresh: `(byte, frames)`.
const PRE_CLEAN: [(u8, u32); 9] = [
    (LIGHTEN, 1),
    (DARKEN, 18),
    (DISCHARGE, 1),
    (LIGHTEN, 18),
    (DISCHARGE, 1),
    (DARKEN, 18),
    (DISCHARGE, 1),
    (LIGHTEN, 18),
    (DISCHARGE, 1),
];

/// 3-bit grayscale waveform for V2: `[gray][frame]`, 0 = no-op, 1 = darken,
/// 2 = lighten.
const WAVEFORM: [[u8; 9]; 8] = [
    [0, 0, 0, 0, 1, 1, 1, 1, 0],
    [0, 0, 0, 1, 1, 1, 1, 0, 0],
    [1, 1, 1, 1, 0, 2, 1, 0, 0],
    [1, 1, 1, 2, 2, 1, 1, 0, 0],
    [1, 1, 1, 1, 2, 2, 1, 0, 0],
    [0, 1, 1, 1, 2, 2, 1, 0, 0],
    [0, 0, 0, 0, 1, 1, 2, 0, 0],
    [0, 0, 0, 0, 0, 0, 2, 0, 0],
];

#[inline(always)]
fn reg(r: *mut u32, v: u32) {
    unsafe { write_volatile(r, v) }
}

/// The I2C bus together with expander 1, which the panel and most other
/// board functions go through.
pub struct Bus {
    pub i2c: I2c<'static, Blocking>,
    pub exp1: Expander,
}

impl Bus {
    fn pin(&mut self, p: u8, high: bool) -> Result<(), Error> {
        self.exp1.set_output(&mut self.i2c, p, high)
    }

    fn pmic_read(&mut self, reg: u8) -> Result<u8, Error> {
        let mut b = [0u8];
        self.i2c.write_read(PMIC, &[reg], &mut b)?;
        Ok(b[0])
    }
}

/// The inner values are only read through `Debug`, in the logs.
#[allow(dead_code)]
#[derive(Debug)]
pub enum PanelError {
    I2c(Error),
    /// The PMIC's rails didn't all come up; the last power-good reading.
    PowerGood(u8),
}

impl From<Error> for PanelError {
    fn from(e: Error) -> Self {
        PanelError::I2c(e)
    }
}

/// How long the parts of one refresh took, in milliseconds.
#[derive(Debug, Default)]
pub struct Timing {
    pub power_on: u64,
    pub clean: u64,
    pub frames: u64,
    pub power_off: u64,
}

pub struct Panel {
    /// D0-D7, CL, LE, CKV, SPH. Driven through the GPIO registers; kept
    /// here so they can be switched to high-Z.
    pins: [Flex<'static>; 12],
    /// Data byte -> GPIO OUT bits.
    lut: [u32; 256],
    delay: Delay,
}

impl Panel {
    /// `pins`: GPIO 4, 5, 18, 19, 23, 25, 26, 27 (D0-D7), 0 (CL), 2 (LE),
    /// 32 (CKV), 33 (SPH). They stay high-Z until the panel is powered.
    pub fn new(pins: [AnyPin<'static>; 12]) -> Self {
        let pins = pins.map(|p| {
            let mut f = Flex::new(p);
            f.apply_output_config(&OutputConfig::default());
            f.set_low();
            f.set_output_enable(false);
            f
        });
        let mut lut = [0u32; 256];
        for (b, v) in lut.iter_mut().enumerate() {
            let b = b as u32;
            *v = (b & 0x03) << 4 | (b >> 2 & 0x03) << 18 | (b >> 4 & 0x01) << 23 | (b >> 5 & 0x07) << 25;
        }
        Panel { pins, lut, delay: Delay::new() }
    }

    fn power_on(&mut self, bus: &mut Bus) -> Result<(), PanelError> {
        reg(OUT_W1TC, DATA | CL | LE);
        reg(OUT1_W1TC, CKV);
        reg(OUT1_W1TS, SPH);
        for p in &mut self.pins {
            p.set_output_enable(true);
        }
        bus.pin(pin::OE, false)?;
        bus.pin(pin::GMOD, true)?;
        bus.pin(pin::SPV, true)?;

        bus.pin(pin::WAKEUP, true)?;
        self.delay.delay_millis(5);
        // V3P3 on (panel logic), the default rail order, then start the
        // rails with a rising edge on PWRUP.
        bus.i2c.write(PMIC, &[PMIC_ENABLE, 0x20])?;
        bus.i2c.write(PMIC, &[PMIC_UPSEQ0, 0xe4])?;
        bus.i2c.write(PMIC, &[PMIC_DWNSEQ0, 0x1b])?;
        bus.pin(pin::PWRUP, true)?;

        let start = Instant::now();
        loop {
            self.delay.delay_millis(1);
            let pg = bus.pmic_read(PMIC_PG)?;
            if pg == PG_ALL_GOOD {
                break;
            }
            if start.elapsed() > Duration::from_millis(250) {
                return Err(PanelError::PowerGood(pg));
            }
        }
        bus.pin(pin::VCOM_CTRL, true)?;
        bus.pin(pin::OE, true)?;
        Ok(())
    }

    /// Never fails: every step is tried even if an earlier one did.
    fn power_off(&mut self, bus: &mut Bus) {
        reg(OUT_W1TC, DATA | CL | LE);
        reg(OUT1_W1TC, CKV | SPH);
        for p in [pin::OE, pin::GMOD, pin::SPV, pin::VCOM_CTRL, pin::PWRUP] {
            let _ = bus.pin(p, false);
        }
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(250) {
            self.delay.delay_millis(1);
            if bus.pmic_read(PMIC_PG).is_ok_and(|pg| pg == 0) {
                break;
            }
        }
        let _ = bus.pin(pin::WAKEUP, false);
        for p in &mut self.pins {
            p.set_output_enable(false);
        }
        for p in [pin::OE, pin::GMOD, pin::SPV] {
            let _ = bus.exp1.set_input(&mut bus.i2c, p, false);
        }
    }

    /// Starts a frame. SPV is an expander pin, so its two changes are I2C
    /// writes; they dominate the time spent here.
    fn vscan_start(&self, bus: &mut Bus) -> Result<(), Error> {
        let d = &self.delay;
        reg(OUT1_W1TS, CKV);
        d.delay_micros(7);
        bus.pin(pin::SPV, false)?;
        d.delay_micros(10);
        reg(OUT1_W1TC, CKV);
        d.delay_micros(1);
        reg(OUT1_W1TS, CKV);
        d.delay_micros(8);
        bus.pin(pin::SPV, true)?;
        d.delay_micros(10);
        reg(OUT1_W1TC, CKV);
        d.delay_micros(1);
        reg(OUT1_W1TS, CKV);
        d.delay_micros(18);
        reg(OUT1_W1TC, CKV);
        d.delay_micros(1);
        reg(OUT1_W1TS, CKV);
        d.delay_micros(18);
        reg(OUT1_W1TC, CKV);
        d.delay_micros(1);
        reg(OUT1_W1TS, CKV);
        Ok(())
    }

    /// Starts a row with its first data byte (as GPIO bits).
    #[inline(always)]
    fn hscan_start(&self, bits: u32) {
        reg(OUT1_W1TC, SPH);
        reg(OUT_W1TS, bits | CL);
        reg(OUT_W1TC, DATA | CL);
        reg(OUT1_W1TS, SPH);
        reg(OUT1_W1TS, CKV);
    }

    /// Latches the row.
    #[inline(always)]
    fn vscan_end(&self) {
        reg(OUT1_W1TC, CKV);
        reg(OUT_W1TS, LE);
        reg(OUT_W1TC, LE);
        self.delay.delay_micros(1);
    }

    /// `frames` frames with every pixel getting the same 2-bit code.
    fn clean(&self, bus: &mut Bus, byte: u8, frames: u32) -> Result<(), Error> {
        let bits = self.lut[byte as usize];
        for _ in 0..frames {
            self.vscan_start(bus)?;
            for _ in 0..HEIGHT {
                self.hscan_start(bits);
                for _ in 1..ROW_BYTES {
                    reg(OUT_W1TS, bits | CL);
                    reg(OUT_W1TC, DATA | CL);
                }
                self.vscan_end();
            }
            self.delay.delay_micros(230);
        }
        Ok(())
    }

    /// One full 3-bit grayscale refresh of a powered panel.
    fn draw(&self, bus: &mut Bus, fb: &[u8], timing: &mut Timing) -> Result<(), Error> {
        let t = Instant::now();
        for (byte, frames) in PRE_CLEAN {
            self.clean(bus, byte, frames)?;
        }
        timing.clean = t.elapsed().as_millis();

        let t = Instant::now();
        for frame in 0..9 {
            // Two framebuffer pixels -> their two 2-bit codes for this frame,
            // odd x in bits 3:2, even x in bits 1:0.
            let mut codes = [0u8; 256];
            for (b, c) in codes.iter_mut().enumerate() {
                *c = WAVEFORM[b & 7][frame] << 2 | WAVEFORM[b >> 4 & 7][frame];
            }
            self.vscan_start(bus)?;
            // Rows from y = 599 down, each from x = 799 down: walk the
            // framebuffer backwards, two bytes (four pixels) per panel byte.
            // The later byte holds the higher x, so it goes in the high bits.
            for row in fb.rchunks_exact(WIDTH / 2) {
                let mut pairs = row.rchunks_exact(2);
                let byte = |p: &[u8]| codes[p[1] as usize] << 4 | codes[p[0] as usize];
                self.hscan_start(self.lut[byte(pairs.next().unwrap()) as usize]);
                for p in pairs {
                    reg(OUT_W1TS, self.lut[byte(p) as usize] | CL);
                    reg(OUT_W1TC, DATA | CL);
                }
                self.vscan_end();
            }
            self.delay.delay_micros(230);
        }
        self.clean(bus, SKIP, 1)?;
        self.vscan_start(bus)?;
        timing.frames = t.elapsed().as_millis();
        Ok(())
    }

    /// Powers the panel up, shows `fb` with a full refresh, and powers it
    /// down again, whatever happened in between.
    pub fn show(&mut self, bus: &mut Bus, fb: &[u8]) -> Result<Timing, PanelError> {
        assert_eq!(fb.len(), FB_SIZE);
        let mut timing = Timing::default();
        let t = Instant::now();
        let result = self.power_on(bus).and_then(|()| {
            timing.power_on = t.elapsed().as_millis();
            self.draw(bus, fb, &mut timing).map_err(PanelError::from)
        });
        let t = Instant::now();
        self.power_off(bus);
        timing.power_off = t.elapsed().as_millis();
        result.map(|()| timing)
    }
}

/// Packs a portrait canvas (600×800) into the panel's landscape framebuffer:
/// canvas `(cx, cy)` lands on panel `(cy, 599 - cx)`.
pub fn pack_portrait(canvas: &Canvas, fb: &mut [u8]) {
    let (w, h) = (canvas.width as usize, canvas.height as usize);
    assert!(w == HEIGHT && h == WIDTH && fb.len() == FB_SIZE);
    for (py, row) in fb.chunks_exact_mut(WIDTH / 2).enumerate() {
        let cx = HEIGHT - 1 - py;
        for (i, b) in row.iter_mut().enumerate() {
            let px = 2 * i;
            let even = gray3(canvas.pixels[px * w + cx]);
            let odd = gray3(canvas.pixels[(px + 1) * w + cx]);
            *b = even << 4 | odd;
        }
    }
}

/// Switches the panel off from the panic handler, where the drivers are out
/// of reach: panel GPIOs low, then VCOM and PWRUP, then WAKEUP, through a
/// fresh I2C driver. Also cuts SD power.
pub fn emergency_off() {
    reg(OUT_W1TC, DATA | CL | LE);
    reg(OUT1_W1TC, CKV | SPH);
    let config = I2cConfig::default().with_frequency(Rate::from_khz(100));
    // SAFETY: we are panicking; nothing else will use these again.
    let Ok(i2c) = I2c::new(unsafe { I2C0::steal() }, config) else { return };
    let mut i2c = i2c.with_sda(unsafe { GPIO21::steal() }).with_scl(unsafe { GPIO22::steal() });
    let mut out = EXP1_OUTPUT.load(Ordering::Relaxed) as u16;
    for p in [pin::OE, pin::GMOD, pin::SPV, pin::VCOM_CTRL, pin::PWRUP] {
        out &= !(1 << p);
    }
    let [lo, hi] = out.to_le_bytes();
    let _ = i2c.write(EXP1, &[OUTPUT, lo, hi]);
    Delay::new().delay_millis(50);
    out &= !(1 << pin::WAKEUP);
    let [lo, hi] = out.to_le_bytes();
    let _ = i2c.write(EXP1, &[OUTPUT, lo, hi]);
    let config = EXP1_CONFIG.load(Ordering::Relaxed) as u16 | 1 << pin::SD_POWER;
    let [lo, hi] = config.to_le_bytes();
    let _ = i2c.write(EXP1, &[CONFIG, lo, hi]);
}
