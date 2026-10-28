//! PCAL6416A I/O expander (Inkplate 6 V2): 16 pins, 0–7 on port 0 and 8–15
//! on port 1. Registers come in port pairs, and a two-byte access to the
//! first one of a pair covers both ports.

use core::sync::atomic::{AtomicU32, Ordering};

use esp_hal::i2c::master::{Error, I2c};
use esp_hal::Blocking;

/// Expander 1 runs the panel and power; expander 2 is free.
pub const EXP1: u8 = 0x20;
pub const EXP2: u8 = 0x21;

#[allow(dead_code)] // For the side buttons.
const INPUT: u8 = 0x00;
pub const OUTPUT: u8 = 0x02;
pub const CONFIG: u8 = 0x06;
const PUPDEN: u8 = 0x46;
const PUPDSEL: u8 = 0x48;

/// Expander 1 pins.
pub mod pin {
    pub const OE: u8 = 0;
    pub const GMOD: u8 = 1;
    pub const SPV: u8 = 2;
    pub const WAKEUP: u8 = 3;
    pub const PWRUP: u8 = 4;
    pub const VCOM_CTRL: u8 = 5;
    /// V2: HIGH turns the battery voltage divider on.
    pub const BATTERY: u8 = 9;
    /// V2: LOW powers the microSD slot; an input turns it off.
    pub const SD_POWER: u8 = 10;
    /// Free pins, planned for the side buttons.
    pub const FREE: [u8; 5] = [11, 12, 13, 14, 15];
}

/// Expander 1's OUTPUT and CONFIG as last written, for the panic handler,
/// which has no access to the driver but must switch the panel off.
pub static EXP1_OUTPUT: AtomicU32 = AtomicU32::new(0xffff);
pub static EXP1_CONFIG: AtomicU32 = AtomicU32::new(0xffff);

pub struct Expander {
    addr: u8,
    output: u16,
    /// 1 = input.
    config: u16,
    pull_enable: u16,
    /// 1 = pull-up.
    pull_up: u16,
}

impl Expander {
    /// Reads the chip's current state rather than assuming the power-on
    /// defaults: the chip could keep the last firmware's setup across an
    /// ESP32 reset (on the test board it seems to reset with the ESP32).
    pub fn new(i2c: &mut I2c<'_, Blocking>, addr: u8) -> Result<Self, Error> {
        let mut e = Expander { addr, output: 0, config: 0, pull_enable: 0, pull_up: 0 };
        e.output = e.read(i2c, OUTPUT)?;
        e.config = e.read(i2c, CONFIG)?;
        e.mirror();
        e.pull_enable = e.read(i2c, PUPDEN)?;
        e.pull_up = e.read(i2c, PUPDSEL)?;
        Ok(e)
    }

    fn read(&self, i2c: &mut I2c<'_, Blocking>, reg: u8) -> Result<u16, Error> {
        let mut b = [0u8; 2];
        i2c.write_read(self.addr, &[reg], &mut b)?;
        Ok(u16::from_le_bytes(b))
    }

    fn write(&self, i2c: &mut I2c<'_, Blocking>, reg: u8, v: u16) -> Result<(), Error> {
        let [lo, hi] = v.to_le_bytes();
        i2c.write(self.addr, &[reg, lo, hi])
    }

    fn mirror(&self) {
        if self.addr == EXP1 {
            EXP1_OUTPUT.store(self.output as u32, Ordering::Relaxed);
            EXP1_CONFIG.store(self.config as u32, Ordering::Relaxed);
        }
    }

    /// Levels of all 16 pins. Reading also clears the interrupt line.
    #[allow(dead_code)] // For the side buttons.
    pub fn inputs(&self, i2c: &mut I2c<'_, Blocking>) -> Result<u16, Error> {
        self.read(i2c, INPUT)
    }

    /// Drives a pin. The level is latched before the pin becomes an output,
    /// so it never briefly drives the old level.
    pub fn set_output(&mut self, i2c: &mut I2c<'_, Blocking>, pin: u8, high: bool) -> Result<(), Error> {
        let bit = 1 << pin;
        let output = if high { self.output | bit } else { self.output & !bit };
        if output != self.output {
            self.write(i2c, OUTPUT, output)?;
            self.output = output;
        }
        if self.config & bit != 0 {
            self.config &= !bit;
            self.write(i2c, CONFIG, self.config)?;
        }
        self.mirror();
        Ok(())
    }

    /// Makes a pin an input (high-Z), with or without the internal pull-up.
    pub fn set_input(&mut self, i2c: &mut I2c<'_, Blocking>, pin: u8, pull_up: bool) -> Result<(), Error> {
        let bit = 1 << pin;
        if self.config & bit == 0 {
            self.config |= bit;
            self.write(i2c, CONFIG, self.config)?;
            self.mirror();
        }
        if pull_up && self.pull_up & bit == 0 {
            self.pull_up |= bit;
            self.write(i2c, PUPDSEL, self.pull_up)?;
        }
        let enable = if pull_up { self.pull_enable | bit } else { self.pull_enable & !bit };
        if enable != self.pull_enable {
            self.pull_enable = enable;
            self.write(i2c, PUPDEN, enable)?;
        }
        Ok(())
    }
}
