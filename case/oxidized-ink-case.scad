// OxidizedInk: a case for the Inkplate 6 V2 (Soldered, purple board) with a
// 606090 cell and three page buttons on a strip beside the screen.
//
// Frame: X and Y are the board's KiCad coordinates relative to its centre
// (x - 132.2, y - 103.1). X runs from the USB-C end (-72.2) to the wake
// button end (+72.2); Y from the expander header edge (-53.9) to the far edge
// (+53.9). Z is height above the back of the case, and the screen faces +Z.
// Held upright with USB-C at the bottom, +X points up and the button strip
// (-Y) is on the right.
//
// Measurements marked "check" come from the board's design files or from
// Soldered's own case, not from a real board. Measure them before printing
// the full case, or print the fit tests first.

part = "assembly"; // [assembly, exploded, shell, back, bar, caps, pins, fit_window, fit_ports]

$fn = 48;

/* [Board and screen] */
board = [144.4, 107.8];
board_r = 2.5;
pcb_t = 1.6;
panel = [138.4, 101.8];        // ED060SC7 outline, centred on the board
panel_t = 1.3;                 // check: screen plus adhesive (board+screen at the edge minus pcb_t)
window = [125.4, 93.6];        // opening at the screen, as in Soldered's case
window_c = [-4, 0];            // the active area sits 4 mm toward the USB end
window_chamfer = 1.0;
standoff_xy = [68.8, 50.5];    // the four M3 SMT standoffs on the component side
standoff_h = 7;                // check
notch = [[50.2, 39.2], [65.2, 53.9]];  // cut-out in the board's +Y edge
// Header rows on the component side; their solder joints get a groove in the ledge.
header_rows = [[-52.4, -60.2, 11.6], [52.4, -14.4, 44.4]];  // [y, x0, x1]

/* [Ports] */
usb_y = 38.9;
usb_hole = [13, 7.5];          // Soldered's case uses 12.7 x 7.5
usb_below = 1.2;               // check: centre of USB-C below the component side
sd_y = 9.9;
sd_hole = [13, 3];
sd_below = 1.4;                // check
side_sw_y = 27.9;              // SW1 (power toggle, USB end) and SW3 (wake, far end)
side_sw_below = 1.5;           // check: actuator centre below the component side
side_sw_tip = 72.4;            // check: how far the actuators reach in X (board edge is 72.2)

/* [Battery] */
cell = [60, 94, 6];            // 606090 with ~4 mm of protection board at the +Y end
cell_c = [14.8, 0];            // the free area right of the JST, between the header rows
cell_room = 1.0;               // swelling and air above the cell
parts_h = 2.0;                 // check: tallest part over the cell (reset button SW2)

/* [Case] */
gap = 0.4;                     // board to wall
wall = 2.4;
wall_far = 3.0;                // the +Y wall carries a screw boss
strip_w = 20;                  // button strip, inside
corner_r = 4;
t_back = 1.8;
t_front = 1.6;
front_gap = 0.1;               // screen to front plate

/* [Buttons] */
sw = 6.0;                      // check: switch body, square
sw_h = 5.0;                    // check: switch height to the plunger tip
leg_pitch = [6.5, 4.5];        // check
pocket_depth = 3.0;
bar_floor = 2.0;
btn_x = [12, -12, 45];         // Prev, Next, Menu
caps = [[15, 7.5], [15, 7.5], [8, 8]];  // [length along X, width]
cap_up = 0.8;                  // how far a cap stands proud of the front
cap_clear = 0.25;
lip = 1.0;
flange_t = 1.0;
bar_x = [-24, 54];
bar_w = 12;
bar_posts = [0, 30];
post_d = 5.5;

/* [Screws: M3 countersunk] */
m3_clear = 3.4;
m3_tap = 2.5;
m3_head = 6.2;

/* [Side button pins] */
pin_d = 2.6;
pin_hole = 3.0;
pin_flange = [5.0, 0.8];
pin_bore = [5.6, 1.0];
wake_out = 0.6;                // wake pin stands proud; the power pin stays flush

// Heights
zF = t_back + cell[2] + cell_room + parts_h;  // component side of the board
zB = zF + pcb_t;                               // screen side of the board
z_in = zB + panel_t + front_gap;               // inside of the front plate
H = z_in + t_front;
z_tip = z_in - flange_t - 0.1;                 // switch plunger tips
z_floor = z_tip - sw_h;
z_bar_front = z_floor + pocket_depth;
z_bar_back = z_floor - bar_floor;
assert(z_bar_back > t_back + 1.5, "switches too tall for the case: shorten sw_h or thicken the case");

// Outline
cav_x = board[0] / 2 + gap;
cav_y0 = -(board[1] / 2 + gap + strip_w);
cav_y1 = board[1] / 2 + gap;
out_x = cav_x + wall;
out_y0 = cav_y0 - wall;
out_y1 = cav_y1 + wall_far;
btn_y = -(board[1] / 2 + gap + strip_w / 2);
boss_pts = [[62, btn_y], [-62, btn_y], [57.7, 54.0]];
standoffs = [for (sx = [-1, 1], sy = [-1, 1]) [sx * standoff_xy[0], sy * standoff_xy[1]]];

echo(str("case: ", 2 * out_x, " x ", out_y1 - out_y0, " x ", H, " mm"));

// 2D helpers

module rrect(p0, p1, r) {
    translate([p0[0] + r, p0[1] + r]) offset(r = r) square([p1[0] - p0[0] - 2 * r, p1[1] - p0[1] - 2 * r]);
}

module outer2d() { rrect([-out_x, out_y0], [out_x, out_y1], corner_r); }
module cavity2d() { rrect([-cav_x, cav_y0], [cav_x, cav_y1], board_r + gap); }
module board2d() { rrect(-board / 2, board / 2, board_r); }
module panel2d(d = 0) { translate(-panel / 2 - [d, d]) square(panel + [2 * d, 2 * d]); }
module window2d(d = 0) { translate(window_c - window / 2 - [d, d]) square(window + [2 * d, 2 * d]); }

module pill2d(c) {
    if (c[0] <= c[1]) circle(d = c[1]);
    else hull() for (s = [-1, 1]) translate([s * (c[0] - c[1]) / 2, 0]) circle(d = c[1]);
}

// The board-notch boss: a round boss cut back to clear the screen.
module notch_boss2d() {
    intersection() {
        outer2d();
        notch_boss_raw2d();
    }
}

module notch_boss_raw2d() {
    difference() {
        hull() {
            translate(boss_pts[2]) circle(d = 7);
            translate([boss_pts[2][0] - 3.5, cav_y1]) square([7, 0.1]);
        }
        panel2d(0.4);
    }
}

// A hole along X through an end wall, centred at (y, z).
module xcut(x0, x1, y, z, w, h, r = 0.5) {
    translate([x0, y, z]) rotate([0, 90, 0])
        linear_extrude(x1 - x0) offset(r = r) square([h - 2 * r, w - 2 * r], center = true);
}

module csk() {
    translate([0, 0, -0.01]) {
        cylinder(d = m3_clear, h = 30);
        cylinder(d1 = m3_head + 0.2, d2 = m3_clear, h = (m3_head + 0.2 - m3_clear) / 2);
    }
}

// Front shell: front plate, walls, ledge for the board, bosses, button posts.
module shell() {
    difference() {
        union() {
            translate([0, 0, t_back]) linear_extrude(H - t_back) difference() { outer2d(); cavity2d(); }
            translate([0, 0, z_in]) linear_extrude(t_front) outer2d();
            // The board's edge rests here, outside the screen, so the glass takes no load.
            translate([0, 0, zB]) linear_extrude(z_in - zB + 0.01)
                difference() { intersection() { board2d(); cavity2d(); } panel2d(0.4); }
            for (p = [boss_pts[0], boss_pts[1]]) translate([p[0], p[1], t_back]) cylinder(d = 7, h = z_in - t_back + 0.01);
            translate([0, 0, t_back]) linear_extrude(z_in - t_back + 0.01) notch_boss2d();
            for (x = bar_posts) translate([x, btn_y, z_bar_front]) cylinder(d = post_d, h = z_in - z_bar_front + 0.01);
        }
        translate([0, 0, z_in - 0.01]) linear_extrude(t_front + 0.02) window2d();
        translate([0, 0, H - window_chamfer]) hull() {
            linear_extrude(0.01) window2d();
            translate([0, 0, window_chamfer]) linear_extrude(0.01) window2d(window_chamfer);
        }
        for (r = header_rows) translate([r[1], r[0] - 1.3, zB - 0.01]) cube([r[2] - r[1], 2.6, 0.8]);
        for (i = [0 : 2]) translate([btn_x[i], btn_y, z_in - 0.01])
            linear_extrude(t_front + 0.02) offset(delta = cap_clear) pill2d(caps[i]);
        for (p = boss_pts) translate([p[0], p[1], t_back - 0.01]) cylinder(d = m3_tap, h = 8.5);
        for (x = bar_posts) translate([x, btn_y, z_bar_front - 0.01]) cylinder(d = m3_tap, h = z_in + t_front - 0.6 - z_bar_front);
        // USB end: USB-C, SD card with a finger scoop, power button
        xcut(-out_x - 1, -cav_x + 0.01, usb_y, zF - usb_below, usb_hole[0], usb_hole[1], 2);
        xcut(-out_x - 1, -cav_x + 0.01, sd_y, zF - sd_below, sd_hole[0], sd_hole[1], 0.8);
        xcut(-out_x - 1, -out_x + 1.2, sd_y, zF - sd_below, 16, 6, 2.5);
        xcut(-out_x - 1, -cav_x + 0.01, side_sw_y, zF - side_sw_below, pin_hole, pin_hole, pin_hole / 2 - 0.01);
        xcut(-cav_x - pin_bore[1], -cav_x + 0.01, side_sw_y, zF - side_sw_below, pin_bore[0], pin_bore[0], pin_bore[0] / 2 - 0.01);
        // Far end: wake button
        xcut(cav_x - 0.01, out_x + 1, side_sw_y, zF - side_sw_below, pin_hole, pin_hole, pin_hole / 2 - 0.01);
        xcut(cav_x - 0.01, cav_x + pin_bore[1], side_sw_y, zF - side_sw_below, pin_bore[0], pin_bore[0], pin_bore[0] / 2 - 0.01);
    }
}

// Back cover: screws into the board's standoffs and the shell's bosses, and
// carries the cell.
module back() {
    difference() {
        union() {
            linear_extrude(t_back) outer2d();
            // Locating lip just inside the walls.
            translate([0, 0, t_back]) linear_extrude(1.5) difference() {
                offset(delta = -0.3) cavity2d();
                offset(delta = -1.5) cavity2d();
                offset(delta = 0.3) notch_boss2d();
            }
            for (p = standoffs) translate([p[0], p[1], t_back]) cylinder(d = 7, h = zF - standoff_h - t_back);
            cell_corners();
        }
        for (p = concat(standoffs, boss_pts)) translate([p[0], p[1], 0]) csk();
    }
}

// L-shaped corners that locate the cell; stick it down with foam tape.
module cell_corners() {
    c0 = cell_c - [cell[0], cell[1]] / 2 - [0.5, 0.5];
    c1 = cell_c + [cell[0], cell[1]] / 2 + [0.5, 0.5];
    translate([0, 0, t_back]) linear_extrude(3) difference() {
        translate(c0 - [1.2, 1.2]) square(c1 - c0 + [2.4, 2.4]);
        translate(c0) square(c1 - c0);
        translate([c0[0] + 8, c0[1] - 2]) square([c1[0] - c0[0] - 16, c1[1] - c0[1] + 4]);
        translate([c0[0] - 2, c0[1] + 8]) square([c1[0] - c0[0] + 4, c1[1] - c0[1] - 16]);
    }
}

// Holds the three switches; screws onto two posts behind the strip. The
// switch legs go through the floor and get bent flat; solder wires to them.
module bar() {
    pocket = sw + 0.3;
    difference() {
        translate([bar_x[0], btn_y - bar_w / 2, z_bar_back]) cube([bar_x[1] - bar_x[0], bar_w, z_bar_front - z_bar_back]);
        for (x = btn_x) translate([x, btn_y, 0]) {
            translate([-pocket / 2, -pocket / 2, z_floor]) cube([pocket, pocket, 10]);
            // Notches for the legs, for either way round.
            for (p = [leg_pitch, [leg_pitch[1], leg_pitch[0]]], sx = [-1, 1], sy = [-1, 1])
                translate([sx * p[0] / 2 - 0.8, sy * p[1] / 2 - 0.8, z_bar_back - 1]) cube([1.6, 1.6, 20]);
        }
        for (x = bar_posts) translate([x, btn_y, z_bar_back]) csk();
    }
}

module cap(c) {
    linear_extrude(flange_t) offset(delta = lip) pill2d(c);
    linear_extrude(flange_t + t_front + cap_up - 0.6) pill2d(c);
    hull() {
        translate([0, 0, flange_t + t_front + cap_up - 0.6]) linear_extrude(0.01) pill2d(c);
        translate([0, 0, flange_t + t_front + cap_up - 0.01]) linear_extrude(0.01) offset(delta = -0.6) pill2d(c);
    }
}

// A pin through an end wall to press a side switch. Its flange sits in the
// counterbore inside the wall so it can't fall out.
module pin(out) {
    len = cav_x + wall + out - side_sw_tip - 0.2;
    cylinder(d = pin_flange[0], h = pin_flange[1]);
    cylinder(d = pin_d, h = len);
}

// Stand-ins for the board, screen and cell, for the assembly views.
module dummies() {
    color("indigo") translate([0, 0, zF]) linear_extrude(pcb_t) difference() {
        board2d();
        translate(notch[0]) square(notch[1] - notch[0] + [0, 1]);
    }
    color([0.9, 0.9, 0.86]) translate([0, 0, zB]) linear_extrude(panel_t) panel2d();
    color([0.75, 0.75, 0.72]) translate([0, 0, zB + panel_t]) linear_extrude(0.01) window2d(-0.5);
    color("silver") for (p = standoffs) translate([p[0], p[1], zF - standoff_h]) cylinder(d = 5.5, h = standoff_h);
    color("steelblue") translate([cell_c[0] - cell[0] / 2, cell_c[1] - cell[1] / 2, t_back]) cube(cell);
    color("black") for (x = btn_x) translate([x - sw / 2, btn_y - sw / 2, z_floor]) cube([sw, sw, 3.5]);
    // Rough connectors and tall parts on the component side, from the footprints.
    color("silver") {
        translate([-72.2, usb_y - 4.47, zF - usb_below - 1.6]) cube([7.3, 8.94, 3.2]);   // USB-C
        translate([-68.1, 1.1, zF - 1.9]) cube([12.2, 17.6, 1.9]);                      // microSD
        translate([-72.2, -34.2, zF - 3.2]) cube([31.1, 19, 3.2]);                      // ESP32 module
        translate([49.8, -47.8, zF - 5]) cube([20, 32, 5]);                             // coin cell holder
        translate([-24.2, 16.7, zF - 5]) cube([7.6, 8.8, 5]);                           // JST battery socket
    }
    color("black") for (s = [-1, 1]) {                                                  // SW1 power, SW3 wake
        translate([s > 0 ? 68.5 : -72.0, side_sw_y - 3.5, zF - 3.5]) cube([3.5, 7, 3.5]);
        translate([s > 0 ? 72.0 : -side_sw_tip, side_sw_y - 1, zF - side_sw_below - 0.75]) cube([side_sw_tip - 72.0, 2, 1.5]);
    }
}

module assembly(spread = 0) {
    color("whitesmoke", 0.55) translate([0, 0, 2 * spread]) shell();
    color("gainsboro") translate([0, 0, -spread]) back();
    color("dimgray") translate([0, 0, spread]) bar();
    color("darkslategray") for (i = [0 : 2]) translate([btn_x[i], btn_y, z_tip + 0.1 + 2.6 * spread]) cap(caps[i]);
    color("darkslategray") {
        translate([side_sw_tip + 0.2, side_sw_y, zF - side_sw_below]) rotate([0, 90, 0]) pin(wake_out);
        translate([-side_sw_tip - 0.2, side_sw_y, zF - side_sw_below]) rotate([0, -90, 0]) pin(0);
    }
    translate([0, 0, spread * 0.5]) dummies();
}

// Single parts where they sit in the assembly, for external viewers.
if (part == "asm_shell") shell();
else if (part == "asm_back") back();
else if (part == "asm_bar") bar();
else if (part == "asm_caps") for (i = [0 : 2]) translate([btn_x[i], btn_y, z_tip + 0.1]) cap(caps[i]);
else if (part == "asm_pins") {
    translate([side_sw_tip + 0.2, side_sw_y, zF - side_sw_below]) rotate([0, 90, 0]) pin(wake_out);
    translate([-side_sw_tip - 0.2, side_sw_y, zF - side_sw_below]) rotate([0, -90, 0]) pin(0);
}
else if (part == "asm_board") difference() { dummies(); translate([cell_c[0] - cell[0] / 2, cell_c[1] - cell[1] / 2, t_back] - [0.01, 0.01, 0.01]) cube(cell + [0.02, 0.02, 0.02]); }
else if (part == "asm_cell") translate([cell_c[0] - cell[0] / 2, cell_c[1] - cell[1] / 2, t_back]) cube(cell);
// Parts in their print orientation.
else if (part == "assembly") assembly();
else if (part == "exploded") assembly(18);
else if (part == "shell") translate([0, 0, H]) rotate([180, 0, 0]) shell();
else if (part == "back") back();
else if (part == "bar") translate([0, -btn_y, -z_bar_back]) bar();
else if (part == "caps") for (i = [0 : 2]) translate([i * 20, 0, 0]) cap(caps[i]);
else if (part == "pins") { pin(wake_out); translate([10, 0, 0]) pin(0); }
// The front plate and ledge only: lay the board in to check the window.
else if (part == "fit_window") translate([0, 0, H]) rotate([180, 0, 0]) intersection() {
    shell();
    translate([-200, -200, zB - 0.01]) cube([400, 400, 50]);
}
// Both end walls: hold them against the board to check the ports.
else if (part == "fit_ports") translate([0, 0, H]) rotate([180, 0, 0]) intersection() {
    shell();
    union() for (s = [-1, 1]) translate([s > 0 ? cav_x - 6 : -out_x - 1, out_y0 - 1, -1]) cube([out_x - cav_x + 7, out_y1 - out_y0 + 2, 50]);
}
