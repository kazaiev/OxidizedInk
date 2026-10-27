# OxidizedInk

An EPUB e-book reader for the Soldered **Inkplate 6**, written in Rust.
Books are parsed and laid out on the device itself. Copy `.epub` files to the
SD card and read.

**Status:** work in progress. The reader runs in a PC simulator: EPUB
parsing, layout, rendering and the library, reading, contents and settings
screens all work.

See [OxidizedInk](
https://alex.k4zka.online/projects/oxidized-ink/) for the full plan and status.

## Layout

| Path | What |
|---|---|
| `core/` | `no_std` reader logic: ZIP, XML, EPUB, text extraction, fonts, layout, rendering, UI |
| `sim/` | PC simulator that runs `core` in a window, or headless writing PNGs |
| `firmware/` | *(planned)* ESP32 firmware for the Inkplate 6 (esp-hal) |
| `fonts/` | PT Serif, SIL Open Font License (see `fonts/OFL.txt`) |
| `docs/HARDWARE.md` | Inkplate 6 hardware notes, from Soldered's Arduino library |

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

Settings are the last row of the library, and an item in the reading menu.
`SETTINGS.TXT`:

```text
text_size = 24
line_spacing = Normal
margins = Normal
alignment = Justified
paragraphs = Indented
status_line = Full
page_buttons = Normal
sleep = 5 min
```

Reading state goes to `READER/` inside the books directory, like `/READER/`
on the SD card. Shades are shown as the panel's 8 gray levels; pass
`--true-gray` for the full 8-bit canvas.

For tests, `--keys` runs headless: `n` Next, `p` Prev, `m` Menu, `b` Back.
It writes a PNG of the screen before the first key and after each one:

```bash
cargo run --release -p inkreader-sim -- --state /tmp/st --keys nnmnnmb --out /tmp/shots
```