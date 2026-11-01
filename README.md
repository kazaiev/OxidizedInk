# OxidizedInk

An EPUB e-book reader for the Soldered **Inkplate 6**, written in Rust.
Books are parsed and laid out on the device itself. Copy `.epub` files to the
SD card and read.

**Status:** work in progress. The reader runs on the Inkplate 6 V2. It shows
the library from the SD card, opens books, turns pages, saves progress and
settings, light-sleeps between page turns and deep sleeps when idle. Three
buttons on the side of the case turn pages, open the menu and wake it. A PC
simulator runs the same code. A 3D-printed case is in `case/`.
See [OxidizedInk](
https://alex.k4zka.online/projects/oxidized-ink/) for the full plan and status.

## Layout

| Path | What |
|---|---|
| `core/` | `no_std` reader logic: ZIP, XML, EPUB, text extraction, fonts, layout, rendering, UI |
| `sim/` | PC simulator that runs `core` in a window, or headless writing PNGs |
| `firmware/` | ESP32 firmware for the Inkplate 6 V2 (esp-hal): panel driver, SD card, input, deep sleep. `bringup` is a separate test program |
| `case/` | OpenSCAD case with a button strip and room for a 606090 cell (see `case/README.md`) |
| `fonts/` | PT Serif, SIL Open Font License (see `fonts/OFL.txt`) |
| `docs/HARDWARE.md` | Inkplate 6 hardware notes, from Soldered's Arduino library |

## Firmware

Needs the `esp` Rust toolchain from [espup](https://github.com/esp-rs/espup)
and [espflash](https://github.com/esp-rs/espflash):

```bash
. ~/export-esp.sh
cd firmware
cargo build --release
espflash flash --monitor target/xtensa-esp32-none-elf/release/inkreader
```

Copy `.epub` files to the root of a FAT-formatted SD card. The three side
buttons (wired as in `case/README.md`) are Menu, Prev and Next from the top;
holding Menu for 0.6 s is Back. Any of them wakes the reader, and that press
also acts. The board's own wake button is not used.

Page turns and menu moves change only the pixels that differ, without the
black and white flash; one screen update in ten (the "Full refresh" setting)
is a full refresh that clears any ghosting.

The status line shows the battery; its percentage changes with full
refreshes. At 15% or below the reader sleeps behind a "Battery low" screen,
and at 2% or below it shows "Battery empty" and won't open the book until
it has been charged. To try those screens on a charged cell, build with
`TEST_BATTERY=12 cargo build --release`.

Progress and settings are saved in `/READER/` on the card.

## Testing

Put some `.epub` files in `testbooks/` (it's gitignored), then run:

```bash
cargo test --release -- --nocapture
```

## Simulator

```bash
cargo run --release -p inkreader-sim                # books from testbooks/
cargo run --release -p inkreader-sim -- --books ~/books
```

| Key | Button |
|---|---|
| Right, Down, PageDown, Space | Next |
| Left, Up, PageUp | Prev |
| Enter | Menu (also selects in lists) |
| Escape, Backspace | Back (holding Menu on the device) |

Settings and System info are the last two rows of the library, and items in
the reading menu. In Settings, Prev/Next pick a setting, Menu changes it, Back
leaves. System info shows the battery voltage and charge, the card and free
memory; any button reads them again, Back leaves. They are saved to
`SETTINGS.TXT`, which can also be edited by hand:

```text
text_size = 24
line_spacing = Normal
margins = Normal
alignment = Justified
paragraphs = Indented
status_line = Full
page_buttons = Normal
sleep = 5 min
full_refresh = Every 10
```

Reading state goes to `READER/` inside the books directory, like `/READER/`
on the SD card. Shades are shown as the panel's 8 gray levels; pass
`--true-gray` for the full 8-bit canvas.

For tests, `--keys` runs headless: `n` Next, `p` Prev, `m` Menu, `b` Back.
It writes a PNG of the screen before the first key and after each one:

```bash
cargo run --release -p inkreader-sim -- --state /tmp/st --keys nnmnnmb --out /tmp/shots
```
