# Case

A 3D-printed case for the Inkplate 6 V2 (the purple Soldered board) with a
606090 LiPo cell and three page buttons. It measures about 150 × 134 ×
15.4 mm. Held upright with USB-C at the bottom, the buttons are on the right
of the screen: Menu at the top, then Prev and Next.

Everything is in `oxidized-ink-case.scad` (OpenSCAD 2021 or newer). The
values at the top are the ones to change. `./build.sh` exports the printable
parts to `stl/`.

## Before printing the whole case

This was fitted to my purple board, check yours!

## Parts

| Part | Print orientation | Notes |
|---|---|---|
| `shell` | Front face down | Front plate, walls, button strip. No supports needed |
| `back` | Outside face down | Screws to the board and the shell; carries the cell |
| `bar` | Pockets up | Holds the three switches |
| `caps` | Flange down | Prev, Next (long) and Menu (round) |
| `pins` | Flange down | Wake (stands out 0.6 mm) and power (flush, so it isn't pressed by accident) |

PETG or PLA at 0.2 mm layers works. The walls are 2.4 mm thick: use at least
4 perimeters.

You also need nine M3×8 countersunk screws:
- 4 go into the board's standoffs.
- 3 go into the shell's bosses.
- 2 hold the button bar.

The last five cut their own thread in 2.5 mm holes.

## Assembly

1. Press each switch into the bar with its legs through the notches. Bend the
   legs flat on the back and solder the wires to them. Use two diagonal legs
   of each switch, or check with a meter, because the legs are joined in
   pairs.
2. Wire the switches to the "GPIO EXPANDER 1" row. Hole 1 is GND, shared by
   all three. Hole 5 is Menu, hole 6 is Next, hole 7 is Prev. Solder from the
   component side and trim the wires flush on the screen side; the ledge has a
   shallow groove there.
3. Drop the caps into their holes from the inside. Screw the bar onto its two
   posts.
4. Put the wake pin in the hole at the top end and the power pin in the hole
   at the bottom, both flange inside.
5. Lay the board into the shell screen down.
6. Stick the cell to the back cover between its corner guides with foam tape,
   protection board toward the battery socket. Plug it in.
7. Close the case: 4 screws into the standoffs, 3 into the shell.

The SD card slot is at the bottom, next to USB-C. With the case closed, the
card sits a few millimetres inside the wall. Push it in and out with a
fingernail or a toothpick.

## On-board buttons

- **Power** (SW1, bottom end): toggles power to the whole board.
- **Wake** (SW3, top end): wakes the reader from sleep. It is also the only
  button that works until the side buttons are wired.
- **Reset** (SW2, middle of the board): sits under the cell and is reachable
  only with the back off. Power-cycling does the same job.
