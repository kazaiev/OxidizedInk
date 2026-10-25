# oxidized-ink

An EPUB e-book reader for the Soldered **Inkplate 6**, written in Rust.
Books are parsed and laid out on the device itself. Copy `.epub` files to the
SD card and read.

**Status:** early work in progress. EPUB parsing works and is tested; layout,
rendering, the PC simulator and the firmware are next. See [OxidizedInk](
https://alex.k4zka.online/projects/oxidized-ink/) for the full plan and status.

## Layout

| Path | What |
|---|---|
| `core/` | `no_std` reader logic: ZIP, XML, EPUB, text extraction (next: layout, rendering, UI) |
| `sim/` | *(planned)* PC simulator that runs `core` in a window |
| `firmware/` | *(planned)* ESP32 firmware for the Inkplate 6 (esp-hal) |
| `fonts/` | PT Serif, SIL Open Font License (see `fonts/OFL.txt`) |
| `docs/HARDWARE.md` | Inkplate 6 hardware notes, from Soldered's Arduino library |

## Testing

Put some `.epub` files in `testbooks/` (it's gitignored), then run:

```bash
cargo test --release -- --nocapture
```
