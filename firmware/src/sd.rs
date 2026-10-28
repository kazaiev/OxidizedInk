//! microSD card over SPI (SCK 14, MISO 12, MOSI 13, CS 15) with a FAT
//! filesystem. On V2 the slot is powered through expander 1 pin 10, which
//! must be on before `Sd::new` and off before deep sleep.
//!
//! GPIO12 (MISO) is also a boot strapping pin: if a powered card holds it
//! high during a reset, the ESP32 may pick the wrong flash voltage. Keep the
//! card off across resets.

use alloc::string::String;
use alloc::vec::Vec;
use core::ops::ControlFlow;

use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::{
    Directory, File, LfnBuffer, Mode, RawVolume, SdCard, ShortFileName, TimeSource, Timestamp, VolumeIdx, VolumeManager,
};
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::peripherals::{GPIO12, GPIO13, GPIO14, GPIO15, SPI2};
use esp_hal::spi::master::{Config, Spi};
use esp_hal::spi::Mode as SpiMode;
use esp_hal::time::Rate;
use esp_hal::Blocking;
use inkreader_core::{Error, ReadAt};

type Device = ExclusiveDevice<Spi<'static, Blocking>, Output<'static>, Delay>;
type Card = SdCard<Device, Delay>;
type Volumes = VolumeManager<Card, Clock, 4, 4, 1>;
pub type SdFile = File<'static, Card, Clock, 4, 4, 1>;

/// File times for anything written. The RTC isn't set yet, so a fixed date.
pub struct Clock;

impl TimeSource for Clock {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp::from_calendar(2026, 1, 1, 0, 0, 0).unwrap()
    }
}

/// A directory entry: the 8.3 name to open it by, the long name to show.
pub struct Entry {
    pub short: ShortFileName,
    pub long: Option<String>,
    pub size: u32,
    pub dir: bool,
}

impl Entry {
    pub fn name(&self) -> String {
        match &self.long {
            Some(l) => l.clone(),
            None => alloc::format!("{}", self.short),
        }
    }
}

pub struct Sd {
    volumes: &'static Volumes,
    /// The first partition, open for good.
    volume: RawVolume,
    /// Card size in bytes.
    pub size: u64,
}

/// The inner errors are only read through `Debug`, in the logs.
#[allow(dead_code)]
#[derive(Debug)]
pub enum SdError {
    Spi,
    Card(embedded_sdmmc::SdCardError),
    Fs(embedded_sdmmc::Error<embedded_sdmmc::SdCardError>),
}

impl From<embedded_sdmmc::Error<embedded_sdmmc::SdCardError>> for SdError {
    fn from(e: embedded_sdmmc::Error<embedded_sdmmc::SdCardError>) -> Self {
        SdError::Fs(e)
    }
}

impl Sd {
    /// Initializes the card at 400 kHz, then runs the bus at 20 MHz.
    pub fn new(spi2: SPI2<'static>, sck: GPIO14<'static>, miso: GPIO12<'static>, mosi: GPIO13<'static>, cs: GPIO15<'static>) -> Result<Self, SdError> {
        let slow = Config::default().with_frequency(Rate::from_khz(400)).with_mode(SpiMode::_0);
        let mut bus = Spi::new(spi2, slow).map_err(|_| SdError::Spi)?.with_sck(sck).with_miso(miso).with_mosi(mosi);
        let cs = Output::new(cs, Level::High, OutputConfig::default());
        // At least 74 clocks with the card deselected before the first command.
        bus.write(&[0xff; 10]).map_err(|_| SdError::Spi)?;
        let device = ExclusiveDevice::new(bus, cs, Delay::new()).map_err(|_| SdError::Spi)?;
        let card = SdCard::new(device, Delay::new());
        let size = card.num_bytes().map_err(SdError::Card)?;
        card.spi(|d| d.bus_mut().apply_config(&slow.with_frequency(Rate::from_mhz(20))))
            .map_err(|_| SdError::Spi)?;
        let volumes: &'static Volumes = alloc::boxed::Box::leak(alloc::boxed::Box::new(VolumeManager::new_with_limits(card, Clock, 5000)));
        let volume = volumes.open_volume(VolumeIdx(0))?.to_raw_volume();
        Ok(Sd { volumes, volume, size })
    }

    /// A directory (`""` for the root). Closed again when dropped.
    fn dir(&self, path: &str) -> Result<Directory<'static, Card, Clock, 4, 4, 1>, SdError> {
        let mut dir = self.volumes.open_root_dir(self.volume)?.to_directory(self.volumes);
        if !path.is_empty() {
            dir.change_dir(path)?;
        }
        Ok(dir)
    }

    /// Entries of a directory (`""` for the root), with long names.
    pub fn list(&self, path: &str) -> Result<Vec<Entry>, SdError> {
        let dir = self.dir(path)?;
        let mut storage = [0u8; 256];
        let mut lfn = LfnBuffer::new(&mut storage);
        let mut out = Vec::new();
        dir.iterate_dir_lfn(&mut lfn, |e, long| {
            if !e.attributes.is_volume() && !e.attributes.is_hidden() {
                out.push(Entry {
                    short: e.name.clone(),
                    long: long.map(String::from),
                    size: e.size,
                    dir: e.attributes.is_directory(),
                });
            }
            ControlFlow::Continue(())
        })?;
        Ok(out)
    }

    /// Reads a whole file, or `None` if it's missing or unreadable.
    pub fn read_file(&self, path: &str, name: &str) -> Option<Vec<u8>> {
        let file = self.dir(path).ok()?.open_file_in_dir(name, Mode::ReadOnly).ok()?;
        let mut out = alloc::vec![0u8; file.length() as usize];
        let mut done = 0;
        while done < out.len() {
            match file.read(&mut out[done..]) {
                Ok(0) | Err(_) => return None,
                Ok(n) => done += n,
            }
        }
        Some(out)
    }

    /// Replaces a file's contents. `path` is a directory in the root, made
    /// if missing, or `""` for the root itself.
    pub fn write_file(&self, path: &str, name: &str, data: &[u8]) -> Result<(), SdError> {
        let mut dir = self.dir("")?;
        if !path.is_empty() {
            if dir.change_dir(path).is_err() {
                dir.make_dir_in_dir(path)?;
                dir.change_dir(path)?;
            }
        }
        let file = dir.open_file_in_dir(name, Mode::ReadWriteCreateOrTruncate)?;
        file.write(data)?;
        file.flush()?;
        Ok(())
    }

    /// Opens a file in a directory (`""` for the root) by its 8.3 name.
    pub fn open(&self, path: &str, name: &ShortFileName) -> Result<SdFile, SdError> {
        Ok(self.dir(path)?.open_file_in_dir(name, Mode::ReadOnly)?)
    }
}

/// An open SD card file as the core's random-access reader.
pub struct Book(pub SdFile);

impl ReadAt for Book {
    fn size(&self) -> u64 {
        self.0.length() as u64
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let offset = u32::try_from(offset).map_err(|_| Error::Io)?;
        self.0.seek_from_start(offset).map_err(|_| Error::Io)?;
        let mut done = 0;
        while done < buf.len() {
            match self.0.read(&mut buf[done..]) {
                Ok(0) | Err(_) => return Err(Error::Io),
                Ok(n) => done += n,
            }
        }
        Ok(())
    }
}
