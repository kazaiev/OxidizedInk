//! microSD card over SPI (SCK 14, MISO 12, MOSI 13, CS 15) with a FAT
//! filesystem. On V2 the slot is powered through expander 1 pin 10, which
//! must be on before `Sd::new` and off before deep sleep.
//!
//! GPIO12 (MISO) is also a boot strapping pin: if a powered card holds it
//! high during a reset, the ESP32 may pick the wrong flash voltage. Keep the
//! card off across resets.

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::ops::ControlFlow;

use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::{
    Block, BlockCount, BlockDevice, BlockIdx, Directory, File, LfnBuffer, Mode, RawVolume, SdCard, SdCardError,
    ShortFileName, TimeSource, Timestamp, VolumeIdx, VolumeManager,
};
use embedded_sdmmc::filesystem::ToShortFileName;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::peripherals::{GPIO12, GPIO13, GPIO14, GPIO15, SPI2};
use esp_hal::spi::master::{Config, Spi};
use esp_hal::spi::Mode as SpiMode;
use esp_hal::time::Rate;
use esp_hal::Blocking;
use inkreader_core::{Error, ReadAt};

type Device = ExclusiveDevice<Spi<'static, Blocking>, Output<'static>, Delay>;
type Volumes = VolumeManager<Card, Clock, 4, 4, 1>;
pub type SdFile = File<'static, Card, Clock, 4, 4, 1>;

/// File times for anything written. The RTC isn't set yet, so a fixed date.
pub struct Clock;

impl TimeSource for Clock {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp::from_calendar(2026, 1, 1, 0, 0, 0).unwrap()
    }
}

/// Changed blocks held back at most: 64 KB.
const HELD: usize = 128;

/// The card, with a write-back cache. embedded-sdmmc writes a file one
/// 512-byte block at a time, and for each 2 KB cluster it reads and
/// rewrites a block of the FAT; each single-block write takes the card
/// about 6.5 ms, so uploads crawled at 80 KB/s. Changed blocks are held
/// here instead, reads see them, and a block written again is replaced in
/// place. When the cache is full, and on `Card::flush`, which every change
/// to the card ends with, they go to the card in order, each run of
/// neighbours as one multi-block write.
struct Cached {
    card: SdCard<Device, Delay>,
    held: RefCell<Held>,
    /// Reads, blocks read, writes, blocks written, µs reading, µs writing.
    stats: RefCell<[u64; 6]>,
}

struct Held {
    /// The block indices held, in the order first written.
    index: Vec<u32>,
    blocks: &'static mut [Block],
    /// For gathering a run of neighbours into one write.
    run: &'static mut [Block],
}

impl Cached {
    fn flush(&self) -> Result<(), SdCardError> {
        let mut held = self.held.borrow_mut();
        let Held { index, blocks, run } = &mut *held;
        if index.is_empty() {
            return Ok(());
        }
        let mut order: Vec<usize> = (0..index.len()).collect();
        order.sort_unstable_by_key(|&i| index[i]);
        let t = esp_hal::time::Instant::now();
        let mut result = Ok(());
        let mut writes = 0;
        let mut k = 0;
        while k < order.len() {
            let first = index[order[k]];
            let mut n = 0;
            while k + n < order.len() && index[order[k + n]] == first + n as u32 {
                run[n].clone_from(&blocks[order[k + n]]);
                n += 1;
            }
            writes += 1;
            if let Err(e) = self.card.write(&run[..n], BlockIdx(first)) {
                result = Err(e);
                break;
            }
            k += n;
        }
        let mut s = self.stats.borrow_mut();
        s[2] += writes;
        s[3] += index.len() as u64;
        s[5] += t.elapsed().as_micros();
        index.clear();
        result
    }
}

/// The card as the filesystem sees it.
#[derive(Clone, Copy)]
pub struct Card(&'static Cached);

impl Card {
    /// Writes what is held back.
    pub fn flush(&self) -> Result<(), SdError> {
        self.0.flush().map_err(SdError::Card)
    }

    /// What the card did since the last call, for the log.
    pub fn take_stats(&self) -> alloc::string::String {
        let s = core::mem::take(&mut *self.0.stats.borrow_mut());
        alloc::format!("{} reads ({} blocks, {} ms), {} writes ({} blocks, {} ms)", s[0], s[1], s[4] / 1000, s[2], s[3], s[5] / 1000)
    }
}

impl BlockDevice for Card {
    type Error = SdCardError;

    fn read(&self, blocks: &mut [Block], start: BlockIdx) -> Result<(), SdCardError> {
        let t = esp_hal::time::Instant::now();
        let held = self.0.held.borrow();
        let find = |b: u32| held.index.iter().position(|&i| i == b);
        let span = start.0..start.0 + blocks.len() as u32;
        if !span.clone().any(|b| find(b).is_some()) {
            self.0.card.read(blocks, start)?;
        } else {
            for (block, b) in blocks.iter_mut().zip(span) {
                match find(b) {
                    Some(i) => block.clone_from(&held.blocks[i]),
                    None => self.0.card.read(core::slice::from_mut(block), BlockIdx(b))?,
                }
            }
        }
        let mut s = self.0.stats.borrow_mut();
        s[0] += 1;
        s[1] += blocks.len() as u64;
        s[4] += t.elapsed().as_micros();
        Ok(())
    }

    fn write(&self, blocks: &[Block], start: BlockIdx) -> Result<(), SdCardError> {
        for (block, b) in blocks.iter().zip(start.0..) {
            let full = {
                let held = self.0.held.borrow();
                !held.index.contains(&b) && held.index.len() == HELD
            };
            if full {
                self.0.flush()?;
            }
            let mut held = self.0.held.borrow_mut();
            let Held { index, blocks: kept, .. } = &mut *held;
            match index.iter().position(|&i| i == b) {
                Some(i) => kept[i].clone_from(block),
                None => {
                    kept[index.len()].clone_from(block);
                    index.push(b);
                }
            }
        }
        Ok(())
    }

    fn num_blocks(&self) -> Result<BlockCount, SdCardError> {
        self.0.card.num_blocks()
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
    card: Card,
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
        let cached = alloc::boxed::Box::leak(alloc::boxed::Box::new(Cached {
            card,
            held: RefCell::new(Held {
                index: Vec::with_capacity(HELD),
                blocks: crate::psram::slice(HELD, Block::new),
                run: crate::psram::slice(HELD, Block::new),
            }),
            stats: RefCell::new([0; 6]),
        }));
        let card = Card(cached);
        let volumes: &'static Volumes = alloc::boxed::Box::leak(alloc::boxed::Box::new(VolumeManager::new_with_limits(card, Clock, 5000)));
        let volume = volumes.open_volume(VolumeIdx(0))?.to_raw_volume();
        Ok(Sd { volumes, volume, size, card })
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
    pub fn read_file<N: ToShortFileName>(&self, path: &str, name: N) -> Option<Vec<u8>> {
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
        file.close()?;
        self.card.flush()
    }

    /// Opens a file in a directory (`""` for the root) by its 8.3 name.
    pub fn open(&self, path: &str, name: &ShortFileName) -> Result<SdFile, SdError> {
        Ok(self.dir(path)?.open_file_in_dir(name, Mode::ReadOnly)?)
    }

    /// Makes a new, empty file in a directory (`""` for the root), open for
    /// writing. Fails if the name is taken. What is written is held back
    /// until `Card::flush`, after closing it.
    pub fn create(&self, path: &str, name: &str) -> Result<(SdFile, Card), SdError> {
        Ok((self.dir(path)?.open_file_in_dir(name, Mode::ReadWriteCreate)?, self.card))
    }

    /// Deletes a file in a directory (`""` for the root). embedded-sdmmc
    /// only marks the entry deleted and leaves the file's clusters in use,
    /// so the file is emptied first, which frees all but its first cluster.
    pub fn delete<N: ToShortFileName + Copy>(&self, path: &str, name: N) -> Result<(), SdError> {
        let dir = self.dir(path)?;
        dir.open_file_in_dir(name, Mode::ReadWriteTruncate)?.close()?;
        dir.delete_entry_in_dir(name)?;
        self.card.flush()
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
