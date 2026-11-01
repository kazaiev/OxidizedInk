//! The three page buttons on the side of the case.
//!
//! The buttons pull expander-1 pins 11–13 to ground. The expander's
//! open-drain INT, on GPIO34, goes low when one of them changes, so the loop
//! reads the expander only then, or while a button is down.
//!
//! Between presses the chip light-sleeps until INT goes low or the idle time
//! runs out; RAM and PSRAM keep their contents. After the idle time it
//! deep-sleeps, and INT wakes it with a reset. During that sleep the expander
//! latches its inputs, so a short press is still in the input register once
//! the board has booted, and tells which button woke it.
//!
//! The board's own wake button (GPIO36) is not used: on the user's board it
//! broke off, probably with its pull-up R31, and the pin floats.

use esp_hal::delay::Delay;
use esp_hal::gpio::{self, Event, WakeupConfig};
use esp_hal::peripherals::LPWR;
use esp_hal::rtc_cntl::sleep::{LowPower, RtcSleepConfig};
use esp_hal::time::{Duration, Instant};
use inkreader_core::ui::Input;
use log::{info, warn};

use crate::expander::pin;
use crate::panel::Bus;

// Buttons as bits.
const PREV: u8 = 1;
const NEXT: u8 = 2;
const MENU: u8 = 4;

/// The buttons' expander pins.
const PINS: u16 = 1 << pin::PREV | 1 << pin::NEXT | 1 << pin::MENU;

/// A shorter press is a bounce.
const MIN_MS: u64 = 30;
/// Holding Menu this long gives Back.
const LONG_MS: u64 = 600;

/// Light sleep between presses (see `nap`). Turn it off to poll at full speed
/// instead.
const LIGHT_SLEEP: bool = true;

pub struct Buttons {
    int: gpio::Input<'static>,
    sleep: LowPower<'static>,
    /// Buttons down at the last read of the expander.
    down: u8,
}

impl Buttons {
    pub fn new(int: gpio::Input<'static>, lpwr: LPWR<'static>, bus: &mut Bus) -> Self {
        if let Err(e) = bus.exp1.set_interrupts(&mut bus.i2c, PINS) {
            warn!("can't enable the button interrupt: {:?}", e);
        }
        Buttons { int, sleep: LowPower::new(lpwr), down: 0 }
    }

    /// The input for the press that woke the board from deep sleep, if one
    /// did. Call it once at boot: it reads, then turns off, the latch. INT is
    /// the only wake source, so a latched press is the one that woke the
    /// board. (esp-hal's `caused_wakeup` says false for it, though the wake
    /// cause is ext1.)
    pub fn wake_press(&mut self, bus: &mut Bus) -> Option<Input> {
        let latched = bus.exp1.inputs(&mut bus.i2c).ok();
        let _ = bus.exp1.set_latch(&mut bus.i2c, 0);
        match buttons_down(latched?) {
            d if d & NEXT != 0 => Some(Input::Next),
            d if d & PREV != 0 => Some(Input::Prev),
            d if d & MENU != 0 => Some(Input::Menu),
            _ => None,
        }
    }

    /// Waits for a press and turns it into an input, or returns `None` after
    /// `idle` without one: Prev, Next and Menu, and Back for a long press on
    /// Menu.
    pub fn wait(&mut self, bus: &mut Bus, idle: Duration) -> Option<Input> {
        let delay = Delay::new();
        let start = Instant::now();
        let deadline = start + idle;
        let mut asleep = 0;
        // First let go of a press that is still held, such as the one that
        // woke the board.
        while self.read(bus) != 0 {
            delay.delay_millis(10);
        }
        loop {
            let down = self.read(bus);
            if down != 0 {
                // The first button down decides; the others wait until it is
                // let go.
                let button = down & down.wrapping_neg();
                let t = Instant::now();
                let mut held = 0;
                let mut quiet = 0;
                while quiet < 3 {
                    delay.delay_millis(10);
                    if self.read(bus) & button != 0 {
                        held = t.elapsed().as_millis();
                        quiet = 0;
                    } else {
                        quiet += 1;
                    }
                }
                if held >= MIN_MS {
                    if LIGHT_SLEEP {
                        info!("light sleep: {} of {} ms", asleep, start.elapsed().as_millis());
                    }
                    return Some(input(button, held));
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
            asleep += self.nap(deadline);
        }
    }

    /// Light-sleeps until a button goes down or `deadline`, and returns the
    /// milliseconds asleep. Listening makes INT a wake source; the default
    /// interrupt handler then disables its interrupt, so it is armed again
    /// for each sleep. With `LIGHT_SLEEP` off it just waits 10 ms.
    fn nap(&mut self, deadline: Instant) -> u64 {
        if !LIGHT_SLEEP {
            Delay::new().delay_millis(10);
            return 0;
        }
        // The CPU runs from the crystal around the sleep, and the UART's baud
        // rate is set for the PLL, so send all log output first.
        flush_log();
        let t = Instant::now();
        self.int.listen(Event::LowLevel);
        self.sleep.set_wakeup_deadline(deadline);
        // esp-hal's default config sets the core voltage for the time after
        // the sleep to 1.10 V, and writes it before the sleep starts. At
        // 240 MHz the ESP32 needs what the clock setup chose (1.25 V less an
        // eFuse trim), and freezes on less, so keep that.
        let mut config = RtcSleepConfig::default();
        config.set_dig_dbias_wak(LPWR::regs().reg().read().dig_dbias_wak().bits());
        // The PLL comes back from the sleep at its default 320 MHz instead of
        // 480, which leaves every clock, the UART's included, at 2/3. As
        // ESP-IDF does, run the CPU from the crystal across the sleep, then
        // set the PLL up again and go back to it. esp-hal 1.2.2 does neither.
        let clk = LPWR::regs().clk_conf();
        clk.modify(|_, w| w.soc_clk_sel().xtal());
        self.sleep.sleep_light(config);
        restore_pll();
        clk.modify(|_, w| w.soc_clk_sel().pll());
        // A deadline left armed would make the deep sleep panic.
        self.sleep.clear_wakeup_deadline();
        self.int.unlisten();
        t.elapsed().as_millis()
    }

    /// Deep-sleeps until a button is pressed; the wake resets the chip. The
    /// expander latches the buttons, so that `wake_press` finds the press
    /// after the boot.
    pub fn deep_sleep(&mut self, bus: &mut Bus) -> ! {
        let _ = bus.exp1.set_latch(&mut bus.i2c, PINS);
        // Reading clears INT, which would otherwise end the sleep at once.
        let _ = bus.exp1.inputs(&mut bus.i2c);
        self.int.listen(Event::LowLevel);
        if let Err(e) = self.int.apply_wakeup_config(&WakeupConfig::default().with_low_power_path(true)) {
            warn!("can't arm the wake line: {:?}", e);
        }
        flush_log();
        // esp-hal's deep sleep powers RTC fast memory down, which would lose
        // the statics the firmware keeps there across the sleep.
        let mut config = RtcSleepConfig::deep();
        config.set_rtc_fastmem_pd_en(false);
        self.sleep.sleep_deep(config)
    }

    /// The buttons that are down, as bits.
    fn read(&mut self, bus: &mut Bus) -> u8 {
        if self.int.is_low() || self.down != 0 {
            if let Ok(inputs) = bus.exp1.inputs(&mut bus.i2c) {
                self.down = buttons_down(inputs);
            }
        }
        self.down
    }
}

unsafe extern "C" {
    /// ROM: writes a register of an analog block on the chip's internal I2C.
    fn rom_i2c_writeReg(block: u8, host_id: u8, reg: u8, data: u8);
    /// ROM: busy-waits, counting CPU cycles at the frequency the ROM was
    /// last told, so it waits longer while the CPU runs from the crystal.
    fn ets_delay_us(us: u32);
}

/// Sets the BBPLL up for 480 MHz from the 40 MHz crystal, with the values
/// esp-hal uses at boot (`enable_pll_clk_impl`). The CPU must not run from
/// the PLL meanwhile.
fn restore_pll() {
    const BBPLL: u8 = 0x66;
    const HOST: u8 = 4;
    let write = |reg: u8, data: u8| unsafe { rom_i2c_writeReg(BBPLL, HOST, reg, data) };
    // Calibration: IR_CAL_DELAY, IR_CAL_EXT_CAP, OC_ENB_FCAL, OC_ENB_VCON,
    // BBADC_CAL.
    write(0, 0x18);
    write(1, 0x20);
    write(4, 0x9a);
    write(10, 0x00);
    write(12, 0x00);
    // 480 MHz: ENDIV5 and BBADC_DSMP. The core voltage for it is already set.
    write(11, 0xc3);
    write(9, 0x74);
    unsafe { ets_delay_us(3) };
    // OC_LREF (div_ref 0), OC_DIV 28, OC_DCUR (bw 3, dcur 6).
    write(2, 0);
    write(3, 28);
    write(5, 3 << 6 | 6);
    // Lock time.
    unsafe { ets_delay_us(160) };
}

/// Waits until UART0 has sent all log output. esp-println leaves bytes in the
/// UART's FIFO, and on the ESP32 its flush does nothing.
fn flush_log() {
    const UART0_STATUS: *const u32 = 0x3ff4_001c as *const u32;
    loop {
        let status = unsafe { UART0_STATUS.read_volatile() };
        // TXFIFO_CNT (bits 23:16) empty and the transmitter (27:24) idle.
        if status >> 16 & 0xff == 0 && status >> 24 & 0xf == 0 {
            break;
        }
    }
}

/// The buttons that are down in a reading of the expander's inputs.
fn buttons_down(inputs: u16) -> u8 {
    let low = |p: u8| inputs & 1 << p == 0;
    (if low(pin::PREV) { PREV } else { 0 })
        | (if low(pin::NEXT) { NEXT } else { 0 })
        | (if low(pin::MENU) { MENU } else { 0 })
}

fn input(button: u8, held: u64) -> Input {
    match button {
        PREV => Input::Prev,
        NEXT => Input::Next,
        MENU if held < LONG_MS => Input::Menu,
        _ => Input::Back,
    }
}
