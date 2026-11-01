# Case

A 3D-printed case for the Inkplate 6 V2 (the purple Soldered board) with a
606090 LiPo cell and three page buttons. It measures about 150 × 123 ×
15.4 mm. Held upright with USB-C at the bottom, the buttons are on the right
of the screen: Menu at the top, then Prev and Next.

Everything is in `oxidized-ink-case.scad` (OpenSCAD 2021 or newer). The
values at the top are the ones to change. The printable parts are in `stl/`,
ready for a slicer. After changing the source, run `./build.sh` to export
them again.

## Before printing the whole case

Several sizes come from the board's design files or from Soldered's own case,
not from a real board. They are marked `check` in the source. Measure these
and update the source:

| Value | What to measure | Default |
|---|---|---|
| `panel_t` | Board plus screen at the edge, minus 1.6 | 1.3 |
| `standoff_h` | Height of the brass standoffs on the back | 7 |
| `cell` | The cell's thickness, and its length with the protection board | 6, 94 |
| `parts_h` | The reset button in the middle of the board (it ends up under the cell) | 2.0 |
| `sw`, `sw_h`, `leg_pitch` | Switch body, height to the plunger tip, leg spacing | 6, 5, 6.5 × 4.5 |
| `side_sw_tip` | How far the power and wake buttons reach past the board edge (edge = 72.2) | 72.4 |

Then print the two fit tests, which take far less time and plastic than the
case:

- `fit_window`: only the front plate and the ledge. Lay the board in screen
  down. The board's edge should rest on the ledge, and the screen's active
  area should sit evenly inside the window. If it's off, move `window_c`.
- `fit_ports`: both end walls. Hold each against its end of the board and
  check that USB-C plugs in, that the SD card goes in, and that the power and
  wake holes line up with the side buttons.

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

1. Press each switch into the bar with its legs through the notches. The legs
   point along the bar; the notches only fit that way round. Bend the legs
   flat on the back and solder the wires to them. Use two diagonal legs
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
