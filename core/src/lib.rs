//! Platform-independent e-reader core. Runs both on the Inkplate (no_std +
//! alloc) and on a PC (simulator, tests).
#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod epub;
pub mod fonts;
pub mod layout;
pub mod render;
pub mod settings;
pub mod text;
pub mod ui;
pub mod xml;
pub mod zip;

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    Io,
    Zip(&'static str),
    Epub(&'static str),
}

/// Random-access read of a file (an SD card file on the device, a `std::fs::File` on a PC).
pub trait ReadAt {
    fn size(&self) -> u64;
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error>;
}

/// Lets tests and the simulator use an in-memory book.
impl ReadAt for alloc::vec::Vec<u8> {
    fn size(&self) -> u64 {
        self.len() as u64
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let start = usize::try_from(offset).map_err(|_| Error::Io)?;
        let src = self.get(start..start + buf.len()).ok_or(Error::Io)?;
        buf.copy_from_slice(src);
        Ok(())
    }
}
