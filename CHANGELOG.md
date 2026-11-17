# Changelog

## 0.1.0

The first release: an e-book reader for the Soldered Inkplate 6 V2, with a
3D-printed case and three side buttons.

### Reading

- EPUB 2 and 3: the book's own table of contents, pictures (JPEG and PNG,
  progressive JPEGs included) with their captions, and links inside the
  book.
- Plain-text and Markdown books (`.txt`, `.md`): headings become chapters,
  and Project Gutenberg's text files show without their header and
  license.
- Tables, in EPUBs and Markdown alike, set in columns sized to their
  content, with cells that wrap and a rule between rows.
- The book's own styles: centered and right-aligned text, bold and
  italics set by class, unindented or spaced paragraphs, set-in blocks,
  hidden text and page breaks. A setting turns them off.
- Large chapters open several times faster (12 s to under 1 s for the
  largest tried).
- Books set in Literata, built in, or in any font family sent to the card.
  Eight text sizes, three line spacings, three margins, justified or
  ragged-right text, indented or spaced paragraphs.
- Hyphenation in English and Ukrainian books, only where a line would
  otherwise be loose.
- Page turns change only the pixels that differ; a full refresh clears
  ghosting every so many pages (a setting).
- Holding Next or Prev skips to the next chapter or back to the start of
  this one.
- Go to a percentage of the book, bookmarks with the page's first words,
  and footnotes that open where they're kept and come back.
- The place in each book is saved on every page turn, and the last book
  opens again on start-up.

### Library

- Each book with its cover, title, author and how much of it is read.
- Books in folders, up to three deep: on the card, or made from the
  Wi-Fi page.
- In order by when they were last opened or added, by title, by author,
  or by when they were added.
- Delete book, in the reading menu, after asking.

### Power

- Light sleep between button presses, deep sleep after a set time.
- While asleep the screen shows the book's cover, a picture from
  `/SLEEP/`, the logo, or the page.
- Battery charge in the status line; the reader warns when it's low and
  stops before it's empty.

### Wi-Fi

- A page in the browser, at `http://oxidized.local/`, for sending books
  (about 300 KB/s), downloading and deleting them, sending and deleting
  font families, and changing the settings and the network. The network
  comes from `/READER/WIFI.TXT` on the card.
- Firmware updates from the same page, or as `FIRMWARE.BIN` on the card.
  An update is checked before and after it's written, and if it fails to
  start three times the firmware before comes back.

### Installing

- A first install, or one from firmware before 0.1.0, goes over USB with
  `oxidized-ink-0.1.0-full.bin` (see the README). Later updates can use
  `oxidized-ink-0.1.0.bin` on the Wi-Fi page.
