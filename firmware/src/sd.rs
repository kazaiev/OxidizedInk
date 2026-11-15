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
/// Blocks read ahead for a reader going through a file in order: 16 KB.
const AHEAD: usize = 32;
/// How long the card is left powered after its last write (see `settle`).
const SETTLE_MS: u64 = 1000;

/// When the card was last written to, in ms since boot.
static LAST_WRITE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn now_ms() -> u64 {
    esp_hal::time::Instant::now().duration_since_epoch().as_millis()
}

/// Waits, if need be, until the card has had `SETTLE_MS` since its last
/// write: call it before switching the card off. A card goes on storing
/// what it was sent after saying it's done, and switched off 100 ms after
/// an upload it lost the last 64 KB.
pub fn settle() {
    let since = now_ms().saturating_sub(LAST_WRITE.load(core::sync::atomic::Ordering::Relaxed) as u64);
    if since < SETTLE_MS {
        Delay::new().delay_millis((SETTLE_MS - since) as u32);
    }
}

/// The card, with a write-back cache. embedded-sdmmc writes a file one
/// 512-byte block at a time, and for each 2 KB cluster it reads and
/// rewrites a block of the FAT; each single-block write takes the card
/// about 6.5 ms, so uploads crawled at 80 KB/s. Changed blocks are held
/// here instead, reads see them, and a block written again is replaced in
/// place. When the cache is full, and on `Card::flush`, which every change
/// to the card ends with, they go to the card in order, each run of
/// neighbours as one multi-block write.
///
/// Reads come a block at a time too, at about 225 KB/s. When one asks for
/// the block after the last, the next `AHEAD` blocks are read in one go,
/// and the last block read singly, usually the FAT's, is kept: reading a
/// file runs at about 1 MB/s then.
struct Cached {
    card: SdCard<Device, Delay>,
    held: RefCell<Held>,
    ahead: RefCell<Ahead>,
    /// The card's size in blocks.
    count: u32,
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

/// Blocks read ahead, and the last block read on its own. Anything written
/// drops both, so they never hold a stale block.
struct Ahead {
    /// The first block in `blocks` and how many are read.
    start: u32,
    len: u32,
    blocks: &'static mut [Block],
    /// The block after the latest one asked for alone.
    next: u32,
    /// The latest block read on its own, with its index.
    last: Option<(u32, Block)>,
}

impl Ahead {
    fn forget(&mut self) {
        self.len = 0;
        self.last = None;
    }

    /// Fills `block` from what's here, if it is.
    fn get(&self, b: u32, block: &mut Block) -> bool {
        if (self.start..self.start + self.len).contains(&b) {
            block.clone_from(&self.blocks[(b - self.start) as usize]);
            true
        } else if let Some((_, kept)) = self.last.as_ref().filter(|(i, _)| *i == b) {
            block.clone_from(kept);
            true
        } else {
            false
        }
    }
}

impl Cached {
    /// Reads one block that isn't held back: from what was read ahead, or
    /// with the next `AHEAD` blocks if the reader is going in order.
    fn read_one(&self, block: &mut Block, b: u32) -> Result<(), SdCardError> {
        let mut ahead = self.ahead.borrow_mut();
        let in_order = b == ahead.next;
        ahead.next = b + 1;
        if ahead.get(b, block) {
            return Ok(());
        }
        let n = (AHEAD as u32).min(self.count.saturating_sub(b));
        if in_order && n > 1 {
            ahead.len = 0;
            self.card.read(&mut ahead.blocks[..n as usize], BlockIdx(b))?;
            ahead.start = b;
            ahead.len = n;
            block.clone_from(&ahead.blocks[0]);
        } else {
            self.card.read(core::slice::from_mut(block), BlockIdx(b))?;
            ahead.last = Some((b, block.clone()));
        }
        Ok(())
    }

    fn flush(&self) -> Result<(), SdCardError> {
        self.ahead.borrow_mut().forget();
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
        LAST_WRITE.store(now_ms() as u32, core::sync::atomic::Ordering::Relaxed);
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
        if blocks.len() > 1 && !span.clone().any(|b| find(b).is_some()) {
            self.0.card.read(blocks, start)?;
        } else {
            for (block, b) in blocks.iter_mut().zip(span) {
                match find(b) {
                    Some(i) => block.clone_from(&held.blocks[i]),
                    None => self.0.read_one(block, b)?,
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
        self.0.ahead.borrow_mut().forget();
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
            ahead: RefCell::new(Ahead { start: 0, len: 0, blocks: crate::psram::slice(AHEAD, Block::new), next: u32::MAX, last: None }),
            count: (size / 512) as u32,
            stats: RefCell::new([0; 6]),
        }));
        let card = Card(cached);
        let volumes: &'static Volumes = alloc::boxed::Box::leak(alloc::boxed::Box::new(VolumeManager::new_with_limits(card, Clock, 5000)));
        let volume = volumes.open_volume(VolumeIdx(0))?.to_raw_volume();
        Ok(Sd { volumes, volume, size, card })
    }

    /// A directory: `""` for the root, or 8.3 names joined by `/`, such as
    /// `FONTS/LITERATA`. With `make`, missing ones are made. Closed again
    /// when dropped.
    fn dir_at(&self, path: &str, make: bool) -> Result<Directory<'static, Card, Clock, 4, 4, 1>, SdError> {
        let mut dir = self.volumes.open_root_dir(self.volume)?.to_directory(self.volumes);
        for name in path.split('/').filter(|n| !n.is_empty()) {
            match dir.change_dir(name) {
                Ok(()) => {}
                Err(_) if make => {
                    dir.make_dir_in_dir(name)?;
                    dir.change_dir(name)?;
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(dir)
    }

    fn dir(&self, path: &str) -> Result<Directory<'static, Card, Clock, 4, 4, 1>, SdError> {
        self.dir_at(path, false)
    }

    /// Entries of a directory (see `dir_at`), with long names, without `.`
    /// and `..`.
    pub fn list(&self, path: &str) -> Result<Vec<Entry>, SdError> {
        entries(self.dir(path)?)
    }

    /// Reads a whole file, or `None` if it's missing or unreadable.
    pub fn read_file<N: ToShortFileName>(&self, path: &str, name: N) -> Option<Vec<u8>> {
        read_all(self.dir(path).ok()?.open_file_in_dir(name, Mode::ReadOnly).ok()?)
    }

    /// Replaces a file's contents. `path` is a directory (see `dir_at`),
    /// made if missing.
    pub fn write_file(&self, path: &str, name: &str, data: &[u8]) -> Result<(), SdError> {
        let file = self.dir_at(path, true)?.open_file_in_dir(name, Mode::ReadWriteCreateOrTruncate)?;
        file.write(data)?;
        file.close()?;
        self.card.flush()
    }

    /// Opens a file in a directory (`""` for the root) by its 8.3 name.
    pub fn open<N: ToShortFileName>(&self, path: &str, name: N) -> Result<SdFile, SdError> {
        Ok(self.dir(path)?.open_file_in_dir(name, Mode::ReadOnly)?)
    }

    /// Makes a new, empty file in a directory (see `dir_at`, made if
    /// missing), open for writing. Fails if the name is taken. What is
    /// written is held back until `Card::flush`, after closing it.
    pub fn create(&self, path: &str, name: &str) -> Result<(SdFile, Card), SdError> {
        Ok((self.dir_at(path, true)?.open_file_in_dir(name, Mode::ReadWriteCreate)?, self.card))
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

/// The entries of a directory, with long names.
fn entries(dir: Directory<'static, Card, Clock, 4, 4, 1>) -> Result<Vec<Entry>, SdError> {
    let mut storage = [0u8; 256];
    let mut lfn = LfnBuffer::new(&mut storage);
    let mut out = Vec::new();
    dir.iterate_dir_lfn(&mut lfn, |e, long| {
        if !e.attributes.is_volume() && !e.attributes.is_hidden() && !e.name.base_name().starts_with(b".") {
            out.push(Entry { short: e.name.clone(), long: long.map(String::from), size: e.size, dir: e.attributes.is_directory() });
        }
        ControlFlow::Continue(())
    })?;
    Ok(out)
}

/// Reads the whole of an open file.
fn read_all(file: SdFile) -> Option<Vec<u8>> {
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
