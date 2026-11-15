# OxidizedInk

An EPUB e-book reader for the Soldered **Inkplate 6**, written in Rust.
Books are parsed and laid out on the device itself. Copy `.epub` files to the
SD card and read.

**Status:** work in progress. The reader runs on the Inkplate 6 V2. It shows
the library from the SD card with each book's cover, opens books
(pictures included), turns pages, saves progress and settings,
light-sleeps between page turns and deep sleeps when idle, behind the
book's cover or a picture of your choice.
Three buttons on the side of the case turn pages, open the menu and wake
it. A PC simulator runs the same code. A 3D-printed case is in `case/`; the
fit prints fit.

See [OxidizedInk](
https://alex.k4zka.online/projects/oxidized-ink/) for the full plan and status.

## Layout

| Path | What |
|---|---|
| `core/` | `no_std` reader logic: ZIP, XML, EPUB, text extraction, fonts, layout, rendering, JPEG and PNG decoding, UI |
| `sim/` | PC simulator that runs `core` in a window, or headless writing PNGs |
| `firmware/` | ESP32 firmware for the Inkplate 6 V2 (esp-hal): panel driver, SD card, input, deep sleep. `bringup` is a separate test program |
| `case/` | OpenSCAD case with a button strip and room for a 606090 cell (see `case/README.md`) |
| `fonts/` | Literata, SIL Open Font License (see `fonts/OFL.txt`) |
| `docs/HARDWARE.md` | Inkplate 6 hardware notes, from Soldered's Arduino library |

## Firmware

Needs the `esp` Rust toolchain from [espup](https://github.com/esp-rs/espup)
and [espflash](https://github.com/esp-rs/espflash):

```bash
. ~/export-esp.sh
cd firmware
cargo run --release   # builds, flashes and shows the log
```

The flash holds two app slots (`firmware/partitions.csv`), so `cargo run`
flashes with `--partition-table partitions.csv --erase-parts otadata`:
by hand that's
`espflash flash --monitor --chip esp32 --partition-table partitions.csv --erase-parts otadata target/xtensa-esp32-none-elf/release/inkreader`.

**Updates without USB.** Make an update file with
`espflash save-image --chip esp32 target/xtensa-esp32-none-elf/release/inkreader inkreader.bin`,
then send it from the Wi-Fi page (Firmware, Update), or copy it to the
card's root as `FIRMWARE.BIN`. Either way the reader installs it the next
time it starts: it checks the image (header, checksum and SHA-256),
writes it to the other slot (about 40 s, with "Installing an update" on
the screen), checks it again there, and starts it. If the new firmware
fails to start the reader three times, the firmware before comes back.
System info and the Wi-Fi page show which build is running (its commit
and date, `+` if built with changes not committed).

Copy `.epub` files to the root of a FAT-formatted SD card. The three side
buttons (wired as in `case/README.md`) are Menu, Prev and Next from the top;
holding Menu for 0.6 s is Back. Holding Next while reading goes to the
next chapter; holding Prev goes back to the start of the chapter, or from
there to the previous one. In the library and the contents, holding them
moves a screen at a time. "Go to %" in the reading menu jumps to a place
in the book: Prev and Next change the percentage, holding them by 10.
"Add bookmark" marks the page (a ribbon shows in its corner) and
"Bookmarks" lists the marks; on a marked page the item removes it. On a
page with footnotes (or other links inside the book), "Footnotes" lists
them; picking one shows the note, and holding Menu goes back to the
text.
Any of them wakes the reader. The board's own
wake button is not used.

While it sleeps, the panel shows what the "Sleep screen" setting picks:

- **Book cover** (the default): the open book's cover. Without one (or
  without a book open), the logo.
- **Pictures**: the JPEG and PNG files in `/SLEEP/` on the card, a different
  one each time, in name order; the cover if there are none. Files over 2 MB
  are skipped. Any size works: pictures are scaled to fit 600×800 and
  dithered to the panel's 8 grays.
- **Logo**: the oxidized-ink logo (`assets/logo.png`), an ink drop made of
  the firmware's own code. It also shows while the reader starts after
  being switched on.
- **Page**: the page you were reading. The press that wakes the reader then
  also turns the page.

Drawing a cover takes about 2.5 s. To see the sleep screen without waiting
for "Sleep after", build with `TEST_IDLE=20 cargo build --release` (seconds).

Pictures in books show at their own size, or shrunk to fit the page, with
their captions. A page with a large picture takes about 1.5 s longer to draw.

Page turns and menu moves change only the pixels that differ, without the
black and white flash; one screen update in ten (the "Full refresh" setting)
is a full refresh that clears any ghosting.

The status line shows the battery; its percentage changes with full
refreshes. At 15% or below the reader sleeps behind a "Battery low" screen,
and at 2% or below it shows "Battery empty" and won't open the book until
it has been charged. To try those screens on a charged cell, build with
`TEST_BATTERY=12 cargo build --release`.

Progress and settings are saved in `/READER/` on the card. The library
shows each book's cover, title, author and how much of it you've read. The
first time a cover is shown it is shrunk from the book and saved there as a
`.THM` file, which takes a moment; after that the library opens quickly.

**Wi-Fi transfer** sends books to the reader from a browser. Once, put a
`WIFI.TXT` in `/READER/` on the card naming a 2.4 GHz network (WPA2 or
open):

```text
ssid = My network
password = its password
```

Then pick **Wi-Fi transfer** in the library. The reader restarts into Wi-Fi
mode, joins the network and shows an address such as
`http://192.168.1.23/`. Open it on a computer or phone on the same network:
drop `.epub` files on the page to send them (about 300 KB/s), click a title
to download a book, or delete one. Holding Menu, or the "Sleep after" time
without use, restarts it into the library. Books sent this way get short
names on the card (`PRIDEAND.EPU`), as the SD library can't make long ones;
the library shows their titles. `TEST_WIFI=1 cargo build --release` starts
Wi-Fi transfer on power-on, to try it without the buttons.

**Fonts.** Books are set in Literata, built in. Other font families can
be sent from the same page: drop a family's `.ttf` or `.otf` files (its
regular style, and bold, italic and bold italic if it has them, up to
1200 KB in all) and the page reads each file's family and style from the
font itself. Or copy them to the card in a folder per family in
`/FONTS/` (`/FONTS/Source Serif 4/SourceSerif4-Regular.ttf`, and so on;
the file names tell the styles apart). Settings, Font picks the family
for books; menus and lists stay in Literata. A family the reader can't
use shows why under Settings, and books keep Literata meanwhile.

## Testing

Put some `.epub` files in `testbooks/` (it's gitignored), then run:

```bash
cargo test --release -- --nocapture
```

The image decoders are checked against small pictures in
`core/tests/images/`: JPEGs against libjpeg-turbo's output, PNGs against the
`png` crate's. They're committed. `make.py` there makes them again, using
libjpeg-turbo's `cjpeg`, `jpegtran` and `djpeg`.

## Simulator

```bash
cargo run --release -p inkreader-sim                # books from testbooks/
cargo run --release -p inkreader-sim -- --books ~/books
```

| Key | Button |
|---|---|
| Right, Down, Space | Next |
| Left, Up | Prev |
| Enter | Menu (also selects in lists) |
| Escape, Backspace | Back (holding Menu on the device) |
| PageDown, PageUp | Holding Next, Prev on the device |

Wi-Fi transfer, Settings and System info are the last three rows of the
library; Settings and System info are also items in the reading menu. In Settings, Prev/Next pick a setting, Menu changes it, Back
leaves. System info shows the battery voltage and charge, the card and free
memory; any button reads them again, Back leaves. They are saved to
`SETTINGS.TXT`, which can also be edited by hand:

```text
font = Literata
text_size = 24
line_spacing = Normal
margins = Normal
alignment = Justified
hyphenation = On
paragraphs = Indented
status_line = Full
page_buttons = Normal
sleep = 5 min
sleep_screen = Book cover
full_refresh = Every 10
```

Reading state goes to `READER/` inside the books directory, like `/READER/`
on the SD card. Shades are shown as the panel's 8 gray levels; pass
`--true-gray` for the full 8-bit canvas.

Sleep screen pictures come from `SLEEP/` inside the books directory.

Wi-Fi transfer serves its page at `http://127.0.0.1:8080/` (`--port` for
another) once `WIFI.TXT` in the state directory names a network; any name
will do. Uploads and deletions change the books directory. Headless,
`--keys ... --out DIR --serve` keeps serving after the keys, writing
`serve.png` after each change, until stopped.

For tests, `--keys` runs headless: `n` Next, `p` Prev, `m` Menu, `b` Back,
`z` the sleep screen. It writes a PNG of the screen before the first key and
after each one:

```bash
cargo run --release -p inkreader-sim -- --state /tmp/st --keys nnmnnmb --out /tmp/shots
```
