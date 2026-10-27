# Inkplate 6 hardware notes

> **Disclaimer**: Gathered with the help from Claude Opus

These notes are for writing a bare-metal panel driver. They were checked against these sources:

- **Soldered Arduino library:** master (11.2.0), 10.2.2, and legacy 8.0.0.
  8.0.0 is the last version that bit-bangs the panel over GPIO; later versions
  use I2S DMA.
- **V2 KiCad design:** `SolderedElectronics/Soldered-Inkplate-6-hardware-design`
  CAD V1.2.1. The netlist was extracted and checked.
- **V1 schematic:** `e-radionicacom/Inkplate-6-hardware` v1.0. 
- **Datasheets:** TI TPS65186 (SLVSB04A) and NXP PCAL6416A (Rev 7.1).
- **Docs:** docs.soldered.com pages on touchpads, free GPIO, and jumpers.

> **License:** the Arduino library is LGPL-3.0 / GPL-3.0. A line-by-line port
> of its driver code would be a derivative work. Register values, pin numbers and
> timing *facts* below are hardware documentation. Re-implement from these
> facts.

## Which board is this?

My board has a **purple PCB with "Inkplate 6" on the back**. Per
Soldered, that makes it **Inkplate 6 V2** (the library calls it
`ARDUINO_INKPLATE6V2`).

| | V1 (e-radionica, usually blue) | **V2 (Soldered, purple)** |
|---|---|---|
| GPIO expanders | MCP23017 at 0x20 and 0x22 | **PCAL6416A at 0x20 and 0x21** |
| Touchpads | 3 (TTP223 → expander 1 pins 10/11/12, HIGH = touched) | **None** |
| RTC | None found | PCF85063A at 0x51, INT → GPIO39 (via JP5) |
| microSD power | Always on | **Switched**: expander 1 pin 10 LOW = on, INPUT = off (JP1 bypasses) |
| Battery divider enable (exp. pin 9) | LOW = on (PMOS) | **HIGH = on** (NMOS drives PMOS) |
| Expander INT on GPIO34 | Push-pull, active HIGH | **Open-drain, active LOW**, 10k pull-up |

**Runtime check:**

1. Probe I2C 0x21. An ACK means V2. Use a 1-byte read if your HAL can't do
   zero-length writes.
2. Probe 0x22. An ACK means V1.
3. An ACK at 0x51 also suggests V2.
4. The TPS65186 at 0x48 only answers while WAKEUP is HIGH.

## ESP32 GPIO (same on V1 and V2)

| Signal | GPIO | Notes |
|---|---|---|
| D0–D7 | 4, 5, 18, 19, 23, 25, 26, 27 | `DATA` mask = `0x0E8C0030` in `GPIO_OUT` |
| CL (pixel clock) | 0 | V2: 330R/100pF RC, then an SN74LVC1G34 buffer powered from 3V3-EINK |
| LE | 2 | |
| CKV | 32 | `GPIO_OUT1` bit 0 |
| SPH | 33 | `GPIO_OUT1` bit 1 |
| I2C SDA / SCL | 21 / 22 | The library uses the default speed; believed to be 100 kHz |
| microSD (SPI) | SCK 14, MISO 12, MOSI 13, CS 15 | Library clocks it at 25 MHz |
| Wake button | 36 | Active LOW, external pull-up |
| Battery ADC | 35 | Through a 100k/100k divider |
| Expander INT | 34 | Via JP2 (connected by default) |
| RTC INT (V2) | 39 | Via JP5 |

**Data byte to GPIO mapping:** byte bits 0–1 → GPIO 4–5, bits 2–3 →
GPIO 18–19, bit 4 → GPIO 23, bits 5–7 → GPIO 25–27. Build a 256-entry
`u32` LUT from that.

**GPIO registers** (base `0x3FF44000`):

| Register | Address |
|---|---|
| `OUT` | 0x3FF44004 |
| `OUT_W1TS` | 0x3FF44008 |
| `OUT_W1TC` | 0x3FF4400C |
| `OUT1` | 0x3FF44010 |
| `OUT1_W1TS` | 0x3FF44014 |
| `OUT1_W1TC` | 0x3FF44018 |
| `ENABLE_W1TS` | 0x3FF44024 |
| `ENABLE1_W1TS` | 0x3FF44030 |

## IO expander 1 (0x20): pin map

Pin numbering: 0–7 = port 0, 8–15 = port 1.

| Pin | Function |
|---|---|
| 0 | OE |
| 1 | GMOD |
| 2 | SPV |
| 3 | WAKEUP (TPS65186) |
| 4 | PWRUP (TPS65186) |
| 5 | VCOM_CTRL (TPS65186) |
| 6 | TPS65186 nINT (input) |
| 7 | TPS65186 PWR_GOOD (input; the library reads the PG register instead) |
| 8 | GPIO0 MOSFET. On V2 it is not populated, so the pin does nothing |
| 9 | Battery divider enable (see the table above) |
| 10 | **V2: microSD power (LOW = on).** V1: touchpad 1 |
| 11, 12 | V1: touchpads 2, 3. V2: free |
| 13–15 | Free |

Expander 2 is entirely free on both boards. On V2 its INT only goes to a
header pad.

### PCAL6416A registers (V2)

| Register | Port 0 / 1 | Notes |
|---|---|---|
| INPUT | 0x00 / 0x01 | Reading clears INT |
| OUTPUT | 0x02 / 0x03 | Reset 0xFF |
| CONFIG | 0x06 / 0x07 | 1 = input; reset 0xFF |
| INLAT | 0x44 / 0x45 | |
| PUPDEN | 0x46 / 0x47 | Pull enable; reset 0x00 |
| PUPDSEL | 0x48 / 0x49 | 1 = pull-up; reset 0xFF |
| INTMSK | 0x4A / 0x4B | 0 = interrupt enabled; reset 0xFF |
| INTSTAT | 0x4C / 0x4D | |

How the library sets pins:

- **To make a pin an output:** write its OUTPUT bit first, then clear its
  CONFIG bit.
- **Input with pull-up:** set the CONFIG, PUPDEN and PUPDSEL bits.
- **Interrupt:** clear the INTMSK bit. INT fires on any change; the chip has
  no edge select.

### MCP23017 registers (V1, BANK = 0)

| Register | Port A / B |
|---|---|
| IODIR | 0x00 / 0x01 |
| GPINTEN | 0x04 / 0x05 |
| DEFVAL | 0x06 / 0x07 |
| INTCON | 0x08 / 0x09 |
| IOCON | 0x0A / 0x0B |
| GPPU | 0x0C / 0x0D |
| INTF | 0x0E / 0x0F |
| INTCAP | 0x10 / 0x11 |
| GPIO | 0x12 / 0x13 |
| OLAT | 0x14 / 0x15 |

### Init (V2)

1. Seed the shadow registers with the power-on defaults.
2. Make WAKEUP, PWRUP and VCOM outputs, driven LOW.
3. Make pin 9 (battery divider) an output, driven LOW. That is OFF on V2.
4. Set pins 11–15 as outputs driven LOW, or as inputs with pull-ups, so they
   don't float. Do the same for all of expander 2.
5. **SD power:** drive pin 10 LOW (card on) *before* initializing the SD card.
   Set it back to INPUT (card off) before deep sleep.
6. Panel pins OE, GMOD and SPV start as inputs (high-Z) until the panel is
   switched on.

## TPS65186 PMIC (I2C 0x48)

**Registers:**

| Register | Address | Notes |
|---|---|---|
| ENABLE | 0x01 | Bits 7→0: ACTIVE, STANDBY, V3P3_EN, VCOM_EN, VDDH_EN, VPOS_EN, VEE_EN, VNEG_EN |
| UPSEQ0 | 0x09 | |
| UPSEQ1 | 0x0A | |
| DWNSEQ0 | 0x0B | |
| DWNSEQ1 | 0x0C | |
| PG | 0x0F | 0xFA = all rails good |
| TMST1 / TMST_VALUE | 0x0D / 0x00 | Temperature: write 0x80 to 0x0D, wait 5 ms, read 0x00 as `i8` |

**SLEEP mode** (WAKEUP LOW) turns I2C off and resets every register, so
configure the chip after each wake.

**Power up** (library master; panel GPIOs are already outputs):

1. Drive LE LOW, SPH HIGH, GMOD HIGH, SPV HIGH, CKV LOW, OE LOW.
2. Set WAKEUP HIGH, then wait 5 ms.
3. Write ENABLE (0x01) = 0x20. That turns on the V3P3 switch, which powers
   the panel logic and the CL buffer on V2.
4. Write UPSEQ0 (0x09) = 0xE4. This is the default: VNEG, VEE, VPOS, VDDH.
5. Write DWNSEQ0 (0x0B) = 0x1B.
6. Set PWRUP HIGH. The rails start on this rising edge.
7. Poll PG (0x0F) every 1 ms until it reads 0xFA, for up to 250 ms. **On
   timeout, run power down and fail.**
8. Set VCOM_CTRL HIGH, then OE HIGH.

**Power down:**

1. Drive OE, GMOD, LE, CKV, SPH and SPV LOW.
2. Set VCOM_CTRL LOW, then PWRUP LOW.
3. Poll PG every 1 ms until it reads 0, for up to 250 ms.
4. Set WAKEUP LOW.
5. Put all panel GPIOs and OE/GMOD/SPV into high-Z (inputs).

**VCOM:** the library never sets it in normal use. The panel's VCOM is stored
in the TPS65186's EEPROM at the factory, so leave it alone.

## Panel timing

**Shorthand:** SET = drive HIGH, CLR = drive LOW, `µs(n)` = busy-wait n µs.
SPV is an *expander* pin, so each SPV change is an I2C write (about 0.3 ms at
100 kHz). That dominates `vscan_start`.

```
vscan_start:
  CKV SET; µs(7); SPV CLR; µs(10); CKV CLR; µs(0)
  CKV SET; µs(8); SPV SET; µs(10); CKV CLR; µs(0)
  CKV SET; µs(18); CKV CLR; µs(0)
  CKV SET; µs(18); CKV CLR; µs(0)
  CKV SET

hscan_start(first_byte_gpio_bits):        (GPIO version, 8.0.0)
  SPH CLR
  OUT_W1TS = bits | CL; OUT_W1TC = DATA | CL
  SPH SET; CKV SET

send byte:  OUT_W1TS = lut[byte] | CL; OUT_W1TC = DATA | CL

vscan_end:
  CKV CLR; LE SET; LE CLR; µs(0)

after each frame:  µs(230)
```

**Per row** (GPIO version): `hscan_start(byte 0)`, then the remaining 199
bytes, then `vscan_end`. That is 200 bytes per row, 4 pixels per byte. V1 adds
one extra CL pulse before `vscan_end`; **V2 doesn't** ("new panel doesn't need
last clock").

The suggestion to put the data lines on the bus *before* raising CL comes from
the research, not from the library. On V2 the RC filter delays CL anyway.

**Frame:** `vscan_start`, then 600 rows.

**Pixel codes** (2 bits per pixel on the bus): `01` = darken, `10` = lighten,
`00` = discharge or no drive, `11` = skip.

**`clean(c, rep)`:** `rep` frames with every byte set to one value:

| c | Byte | Meaning |
|---|---|---|
| 0 | 0xAA | Lighten |
| 1 | 0x55 | Darken |
| 2 | 0x00 | Discharge |
| 3 | 0xFF | Skip |

**Pre-clean** before every full refresh (master / 10.2.2):

```
clean(0,1) clean(1,18) clean(2,1) clean(0,18) clean(2,1)
clean(1,18) clean(2,1) clean(0,18) clean(2,1)
```

### Send order

These are panel coordinates: 800×600 landscape. Rows go from **y = 599 down
to y = 0**, and within a row from **x = 799 down to x = 0**. The first byte
of each row holds:

| Bits | Pixel |
|---|---|
| D7:D6 | x 799 |
| D5:D4 | x 798 |
| D3:D2 | x 797 |
| D1:D0 | x 796 |

### 3-bit grayscale (`display3b`)

- **Framebuffer:** 4 bits per pixel, `buf[400*y + x/2]`. Even x goes in the
  high nibble, odd x in the low nibble. Values run from 0 = black to
  7 = white.
- **Waveform** `wf[value 0..7][frame 0..8]`: 0 = no-op, 1 = darken,
  2 = lighten. All 9 frames are used. **V2 (library master):**

  ```
  {0,0,0,0,1,1,1,1,0}, {0,0,0,1,1,1,1,0,0}, {1,1,1,1,0,2,1,0,0}, {1,1,1,2,2,1,1,0,0},
  {1,1,1,1,2,2,1,0,0}, {0,1,1,1,2,2,1,0,0}, {0,0,0,0,1,1,2,0,0}, {0,0,0,0,0,0,2,0,0}
  ```

  For reference, the V1 table from 8.0.0–10.2.2:

  ```
  {0,1,1,0,0,1,1,0,0}, {0,1,2,1,1,2,1,0,0}, {1,1,1,2,2,1,0,0,0}, {0,0,0,1,1,1,2,0,0},
  {2,1,1,1,2,1,2,0,0}, {2,2,1,1,2,1,2,0,0}, {1,1,1,2,1,2,2,0,0}, {0,0,0,0,0,0,2,0,0}
  ```

- **Per-frame lookup tables.** For frame k and framebuffer byte b (high
  nibble h = even x, low nibble l = odd x):
  - `lo[k][b] = (wf[l][k] << 2) | wf[h][k]`
  - `hi[k][b] = lo[k][b] << 4`

  Each panel byte combines two framebuffer bytes:
  `hi[k][*--p] | lo[k][*--p]`. The **first** byte read (the later one in
  memory) goes through `hi`. Make that order explicit: the C code relies on
  unspecified evaluation order.
- **Sequence:** pre-clean, then 9 frames (k = 0..8), then `clean(3,1)`, then
  `vscan_start`, then power down.

### 1-bit (`display1b`) and partial update

- **Framebuffer:** `buf[100*y + x/8]`, with pixel x in bit `x & 7`.
  1 = black.
- **Lookup tables** (per nibble):
  - `LUTB = {FF,FD,F7,F5,DF,DD,D7,D5,7F,7D,77,75,5F,5D,57,55}`
  - `LUTW = {FF,FE,FB,FA,EF,EE,EB,EA,BF,BE,BB,BA,AF,AE,AB,AA}`
  - `LUT2 = {AA,A9,A6,A5,9A,99,96,95,6A,69,66,65,5A,59,56,55}`
- **Full refresh:** pre-clean, then 5 frames (V2; 4 on V1) of
  `LUTB[nibble]`, high nibble first, bytes from the end. Then 1 frame of
  `LUT2`, 1 frame of 0x00, and `vscan_start`.
- **Partial update:** for each byte, `diffw = old & !new` and
  `diffb = !old & new`. Send `LUTW[diffw] & LUTB[diffb]` per nibble for 6
  frames (V2; 5 on V1), then `clean(2,2)`, `clean(3,1)`, `vscan_start`. Do a
  full refresh every ~10 partials.

## Wake sources and deep sleep

| Source | Setup |
|---|---|
| Wake button | ext0 on GPIO36, level LOW |
| V2 expander INT (e.g. external buttons on free expander pins) | ext0/ext1 on GPIO34, **level LOW** (open-drain). The 10.2.2 example uses HIGH, which is likely a bug |
| V2 RTC alarm | ext0 on GPIO39, level LOW |
| V1 touchpads | MCP: IOCONB active HIGH, INTCON/DEFVAL/GPINTEN bits for pins 10–12; ext0 on GPIO34, level HIGH |

Before deep sleep: power the panel down, cut SD power (V2: pin 10 → INPUT),
and turn the battery divider off.

## Battery

1. Enable the divider: pin 9 HIGH on V2, LOW on V1.
2. Wait 5 ms.
3. Read GPIO35 in millivolts at 11 dB attenuation (believed; not verified).
4. Battery voltage in V = mV × 2 / 1000.
5. Disable the divider again.

## Not verified

- I2C bus speed (believed to be 100 kHz) and the ADC attenuation default.
- V1 jumper defaults and the V1 battery MOSFET circuit
- The I2S wire byte order, which only matters for a later I2S/DMA driver.
