//! Firmware updates without USB. The flash has two app slots (see
//! `partitions.csv`). An update is `/FIRMWARE.BIN` on the card, copied
//! there or sent from the Wi-Fi page: at start-up it is checked, written to
//! the slot not running, checked again there, and started. The first starts
//! of new firmware are counted, and one that hasn't started the reader by
//! the third goes back to the other slot.

use alloc::vec;

use esp_bootloader_esp_idf::ota::OtaImageState;
use esp_bootloader_esp_idf::ota_updater::OtaUpdater;
use esp_bootloader_esp_idf::partitions::PARTITION_TABLE_MAX_LEN;
use esp_hal::time::Instant;
use esp_storage::FlashStorage;
use inkreader_core::firmware::ImageCheck;
use log::{info, warn};

use crate::sd::Sd;

/// The update's name on the card.
pub const FILE: &str = "FIRMWARE.BIN";

/// Starts of firmware not yet confirmed, in RTC memory beside a mark, as
/// the firmware before may have left anything there.
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut TRIES: [u32; 2] = [0; 2];
const TRIES_MARK: u32 = 0x5550_4454;
const MAX_TRIES: u32 = 3;

/// Takes a piece of the update and its offset.
pub type Piece<'a> = dyn FnMut(u32, &[u8]) -> Result<(), &'static str> + 'a;

/// Flash is erased and written a sector at a time.
const SECTOR: usize = 4096;

/// Whether the running firmware is new: installed, and not yet confirmed.
fn unconfirmed(state: OtaImageState) -> bool {
    matches!(state, OtaImageState::New | OtaImageState::PendingVerify)
}

/// Counts a start of new firmware, and goes back to the other slot after
/// `MAX_TRIES` starts that didn't get as far as `confirm`. Call it first
/// thing. Does nothing without two app slots (flashed with the old layout).
pub fn check_start(flash: &mut FlashStorage<'_>) {
    let mut buffer = [0u8; PARTITION_TABLE_MAX_LEN];
    let Ok(mut ota) = OtaUpdater::new(flash, &mut buffer) else { return };
    if !ota.current_ota_state().is_ok_and(unconfirmed) {
        return;
    }
    let tries = unsafe { if TRIES[0] == TRIES_MARK { TRIES[1] } else { 0 } } + 1;
    unsafe { TRIES = [TRIES_MARK, tries] };
    if tries <= MAX_TRIES {
        info!("new firmware: start {} of {}", tries, MAX_TRIES);
    } else {
        warn!("the new firmware didn't start the reader: going back to the one before");
        let _ = ota.set_current_ota_state(OtaImageState::Invalid);
        let _ = ota.activate_next_partition();
        unsafe { TRIES = [0, 0] };
        esp_hal::system::software_reset();
    }
}

/// Marks the running firmware good, once the reader has started.
pub fn confirm(flash: &mut FlashStorage<'_>) {
    unsafe { TRIES = [0, 0] };
    let mut buffer = [0u8; PARTITION_TABLE_MAX_LEN];
    let Ok(mut ota) = OtaUpdater::new(flash, &mut buffer) else { return };
    if ota.current_ota_state().is_ok_and(unconfirmed) && ota.set_current_ota_state(OtaImageState::Valid).is_ok() {
        info!("new firmware confirmed");
    }
}

/// The update's size, if there's one on the card.
pub fn pending(sd: &Sd) -> Option<u32> {
    let entries = sd.list("").ok()?;
    entries.into_iter().find(|e| !e.dir && alloc::format!("{}", e.short) == FILE).map(|e| e.size)
}

/// Reads the update from the card a sector at a time, handing each piece
/// and its offset to `f`.
pub fn read_file(sd: &Sd, f: &mut Piece<'_>) -> Result<(), &'static str> {
    let file = sd.open("", FILE).map_err(|_| "the card can't be read")?;
    let mut buf = vec![0u8; SECTOR];
    let mut at = 0u32;
    loop {
        let mut got = 0;
        while got < SECTOR {
            match file.read(&mut buf[got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(_) => return Err("the card can't be read"),
            }
        }
        if got == 0 {
            return Ok(());
        }
        f(at, &buf[..got])?;
        at += got as u32;
    }
}

/// Installs the update on the card into the slot not running and selects
/// it for the next start. The update is deleted, installed or not, so a bad
/// one isn't tried again.
pub fn install(sd: &Sd, flash: &mut FlashStorage<'_>) -> Result<(), &'static str> {
    let t = Instant::now();
    let result = write(sd, flash);
    let _ = sd.delete("", FILE);
    match result {
        Ok(size) => info!("update installed: {} KB in {} ms", size / 1024, t.elapsed().as_millis()),
        Err(why) => warn!("update not installed: {}", why),
    }
    result.map(|_| ())
}

fn write(sd: &Sd, flash: &mut FlashStorage<'_>) -> Result<u32, &'static str> {
    // First check the file, so a bad one never touches the flash.
    let mut check = ImageCheck::new();
    read_file(sd, &mut |_, piece| {
        check.feed(piece);
        Ok(())
    })?;
    let size = check.len() as u32;
    check.finish()?;

    let mut buffer = [0u8; PARTITION_TABLE_MAX_LEN];
    let mut ota = OtaUpdater::new(flash, &mut buffer).map_err(|_| "the flash has no second app slot; flash this firmware over USB once")?;
    {
        let (mut slot, _) = ota.next_partition().map_err(|_| "the flash's slots can't be read")?;
        if size as usize > slot.partition_size() {
            return Err("the update is larger than an app slot");
        }
        let end = (size as usize).div_ceil(SECTOR) * SECTOR;
        slot.erase(0, end as u32).map_err(|_| "the flash can't be erased")?;
        read_file(sd, &mut |at, piece| {
            // Writes go in whole words; the last piece is padded as erased
            // flash reads.
            let mut padded = vec![0xffu8; piece.len().div_ceil(4) * 4];
            padded[..piece.len()].copy_from_slice(piece);
            slot.write(at, &padded).map_err(|_| "the flash can't be written")
        })?;
        // Read it back from the flash and check it again.
        let mut check = ImageCheck::new();
        let mut buf = vec![0u8; SECTOR];
        let mut at = 0;
        while at < size {
            let n = (size - at).min(SECTOR as u32) as usize;
            slot.read(at, &mut buf[..n]).map_err(|_| "the flash can't be read")?;
            check.feed(&buf[..n]);
            at += n as u32;
        }
        check.finish().map_err(|_| "the update didn't read back from the flash as written")?;
    }
    ota.activate_next_partition().map_err(|_| "the new slot can't be selected")?;
    ota.set_current_ota_state(OtaImageState::New).map_err(|_| "the new slot can't be selected")?;
    Ok(size)
}
