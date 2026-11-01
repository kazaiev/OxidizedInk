//! Battery voltage. On V2, expander 1 pin 9 switches on a divider that
//! halves VBAT onto GPIO35 (ADC1).
//!
//! esp-hal has no ADC calibration for the original ESP32, so this uses the
//! ESP-IDF linear model for 11 dB attenuation (`characterize_using_vref` in
//! `esp_adc_cal_esp32.c`). It takes the chip's reference voltage from
//! eFuse. The model holds below raw 2880; a battery never gets near that.

use esp_hal::analog::adc::{Adc, AdcConfig, AdcPin, Attenuation};
use esp_hal::delay::Delay;
use esp_hal::efuse::{self, ADC_VREF};
use esp_hal::peripherals::{ADC1, GPIO35};
use esp_hal::Blocking;

use crate::expander::pin;
use crate::panel::Bus;

const SAMPLES: u32 = 16;

/// Gain correction for this board's divider and ADC, as (true, model) mV.
/// On 2026-10-22, on USB, a meter read 4.18 V at the cell and the model 4242.
/// One point only corrects gain; check again near 3.7 V for any offset.
const CAL: (u32, u32) = (4180, 4242);

pub struct Battery {
    adc: Adc<'static, ADC1<'static>, Blocking>,
    pin: AdcPin<GPIO35<'static>, ADC1<'static>>,
    pub vref: u32,
}

/// A reading: battery millivolts and the averaged raw ADC value behind it.
pub struct Reading {
    pub mv: u32,
    pub raw: u32,
}

impl Battery {
    pub fn new(adc1: ADC1<'static>, gpio35: GPIO35<'static>) -> Self {
        let mut cfg = AdcConfig::new();
        let pin = cfg.enable_pin(gpio35, Attenuation::_11dB);
        Battery { adc: Adc::new(adc1, cfg), pin, vref: vref() }
    }

    pub fn read(&mut self, bus: &mut Bus) -> Option<Reading> {
        bus.exp1.set_output(&mut bus.i2c, pin::BATTERY, true).ok()?;
        Delay::new().delay_millis(5);
        let mut sum = 0;
        for _ in 0..SAMPLES {
            sum += nb::block!(self.adc.read_oneshot(&mut self.pin)).unwrap_or(0) as u32;
        }
        let _ = bus.exp1.set_output(&mut bus.i2c, pin::BATTERY, false);
        let raw = sum / SAMPLES;
        // At 11 dB: mV = raw * vref * 196602 / 4096 / 65536 + 142, rounded.
        let coeff_a = self.vref * 196_602 / 4096;
        let pin_mv = (coeff_a * raw + 32_768) / 65_536 + 142;
        Some(Reading { mv: 2 * pin_mv * CAL.0 / CAL.1, raw })
    }
}

/// The ADC reference voltage in mV: 1100 plus a sign-magnitude offset in
/// 7 mV steps, burned in at the factory.
fn vref() -> u32 {
    let bits: u8 = efuse::read_field_le(ADC_VREF);
    let steps = (bits & 0x0f) as i32;
    let offset = if bits & 0x10 != 0 { -steps } else { steps };
    (1100 + offset * 7) as u32
}

/// Rough state of charge of a LiPo cell at rest. It reads high while
/// charging.
pub fn percent(mv: u32) -> u8 {
    const CURVE: [(u32, u8); 11] = [
        (3300, 0),
        (3500, 4),
        (3600, 10),
        (3650, 18),
        (3700, 28),
        (3750, 40),
        (3800, 49),
        (3900, 64),
        (4000, 78),
        (4100, 90),
        (4200, 100),
    ];
    if mv <= CURVE[0].0 {
        return 0;
    }
    for w in CURVE.windows(2) {
        let ((v0, p0), (v1, p1)) = (w[0], w[1]);
        if mv <= v1 {
            return p0 + ((mv - v0) * (p1 - p0) as u32 / (v1 - v0)) as u8;
        }
    }
    100
}
