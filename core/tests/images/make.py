#!/usr/bin/env python3
"""Makes the test pictures in this directory: a synthetic picture as 203x157
JPEGs (with libjpeg-turbo's cjpeg, jpegtran and djpeg), and as 61x47 PNGs of
every color type, bit depth and filter, interlaced or not (written here).

The *.pgm files are djpeg's gray output, which the tests compare against.
Run it from anywhere; it writes next to itself.
"""

import math
import os
import struct
import subprocess
import zlib

W, H = 203, 157
# The PNGs are smaller, to keep the files small.
PW, PH = 61, 47
HERE = os.path.dirname(os.path.abspath(__file__))


def pixel(x, y):
    """Smooth gradients, rings, hard-edged shapes and some texture."""
    r = 128 + 127 * math.sin(x / 9.0) * math.cos(y / 13.0)
    g = 255 * x / (W - 1)
    b = 128 + 127 * math.sin(math.hypot(x - 120, y - 70) / 5.0)
    if 20 <= x < 70 and 30 <= y < 60:
        r, g, b = 10, 10, 10
    if (x - 160) ** 2 + (y - 120) ** 2 < 25 ** 2:
        r, g, b = 250, 240, 20
    if y % 20 < 2 and x > 100:
        r, g, b = 0, 0, 0
    return tuple(max(0, min(255, int(v))) for v in (r, g, b))


def alpha(x, y):
    return max(0, min(255, int(255 * (y / (PH - 1)) * (0.5 + 0.5 * math.cos(x / 5.0)))))


def path(name):
    return os.path.join(HERE, name)


def run(*args):
    subprocess.run(args, check=True)


def make_jpegs():
    rgb = bytearray()
    for y in range(H):
        for x in range(W):
            rgb += bytes(pixel(x, y))
    src = path("source.ppm")
    with open(src, "wb") as f:
        f.write(b"P6 %d %d 255\n" % (W, H) + bytes(rgb))
    for name, opts in [
        ("base420", ["-sample", "2x2"]),
        ("base422", ["-sample", "2x1"]),
        ("base444", ["-sample", "1x1"]),
        ("gray", ["-grayscale"]),
        ("restart", ["-sample", "2x2", "-restart", "3B"]),
    ]:
        run("cjpeg", "-quality", "85", *opts, "-outfile", path(name + ".jpg"), src)
    run("jpegtran", "-progressive", "-outfile", path("prog420.jpg"), path("base420.jpg"))
    run("jpegtran", "-progressive", "-outfile", path("proggray.jpg"), path("gray.jpg"))
    run("jpegtran", "-progressive", "-restart", "2B", "-outfile", path("progrestart.jpg"), path("base444.jpg"))
    run("cjpeg", "-arithmetic", "-outfile", path("arith.jpg"), src)
    for name in ["base420", "base422", "base444", "gray"]:
        run("djpeg", "-grayscale", "-pnm", "-outfile", path(name + ".pgm"), path(name + ".jpg"))
    os.remove(src)


def png_chunk(kind, body):
    return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))


def pack(samples, depth):
    """Packs one row of samples at `depth` bits."""
    if depth == 16:
        return b"".join(struct.pack(">H", s) for s in samples)
    if depth == 8:
        return bytes(samples)
    out, acc, n = bytearray(), 0, 0
    for s in samples:
        acc = acc << depth | s
        n += depth
        if n == 8:
            out.append(acc)
            acc, n = 0, 0
    if n:
        out.append(acc << (8 - n))
    return bytes(out)


def paeth(a, b, c):
    p = a + b - c
    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
    return a if pa <= pb and pa <= pc else b if pb <= pc else c


def filtered(rows, bpp):
    """Each row with a filter byte, cycling through the five filters."""
    out = bytearray()
    prev = bytes(len(rows[0])) if rows else b""
    for i, row in enumerate(rows):
        f = i % 5
        out.append(f)
        for j, v in enumerate(row):
            a = row[j - bpp] if j >= bpp else 0
            b = prev[j]
            c = prev[j - bpp] if j >= bpp else 0
            pred = [0, a, b, (a + b) // 2, paeth(a, b, c)][f]
            out.append((v - pred) & 255)
        prev = row
    return bytes(out)


def write_png(name, color, depth, samples_at, channels, interlaced=False, extra=b""):
    """`samples_at(x, y)` gives the samples of a pixel."""
    bpp = max(1, channels * depth // 8)
    passes = [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)]
    if not interlaced:
        passes = [(0, 0, 1, 1)]
    data = bytearray()
    for x0, y0, dx, dy in passes:
        xs, ys = range(x0, PW, dx), range(y0, PH, dy)
        if not xs or not ys:
            continue
        rows = [pack([s for x in xs for s in samples_at(x, y)], depth) for y in ys]
        data += filtered(rows, bpp)
    ihdr = struct.pack(">IIBBBBB", PW, PH, depth, color, 0, 0, 1 if interlaced else 0)
    png = b"\x89PNG\r\n\x1a\n" + png_chunk(b"IHDR", ihdr) + extra
    # Split the data over several IDAT chunks, as encoders do.
    z = zlib.compress(bytes(data), 9)
    for i in range(0, len(z), 1000):
        png += png_chunk(b"IDAT", z[i : i + 1000])
    png += png_chunk(b"IEND", b"")
    with open(path(name + ".png"), "wb") as f:
        f.write(png)


def make_pngs():
    def gray(x, y):
        r, g, b = pixel(x, y)
        return (r * 3 + g * 5 + b * 2) // 10

    write_png("gray1", 0, 1, lambda x, y: [gray(x, y) >> 7], 1)
    write_png("gray2", 0, 2, lambda x, y: [gray(x, y) >> 6], 1)
    write_png("gray4", 0, 4, lambda x, y: [gray(x, y) >> 4], 1)
    write_png("gray8", 0, 8, lambda x, y: [gray(x, y)], 1)
    write_png("gray16", 0, 16, lambda x, y: [gray(x, y) * 257 + x % 7], 1)
    write_png("gray8key", 0, 8, lambda x, y: [gray(x, y)], 1, extra=png_chunk(b"tRNS", struct.pack(">H", 10)))
    write_png("rgb8", 2, 8, lambda x, y: list(pixel(x, y)), 3)
    write_png("rgb16", 2, 16, lambda x, y: [v * 257 for v in pixel(x, y)], 3)
    write_png("graya8", 4, 8, lambda x, y: [gray(x, y), alpha(x, y)], 2)
    write_png("rgba8", 6, 8, lambda x, y: list(pixel(x, y)) + [alpha(x, y)], 4)
    write_png("rgba16", 6, 16, lambda x, y: [v * 257 for v in pixel(x, y)] + [alpha(x, y) * 257], 4)
    # A palette of 16 grays and colors, half of them see-through.
    palette = [((i * 17) % 256, (i * 53) % 256, (i * 91) % 256) for i in range(16)]
    plte = png_chunk(b"PLTE", b"".join(bytes(p) for p in palette))
    trns = png_chunk(b"tRNS", bytes(255 if i % 2 else i * 16 for i in range(16)))
    write_png("pal4", 3, 4, lambda x, y: [(x // 13 + y // 11) % 16], 1, extra=plte + trns)
    write_png("rgb8i", 2, 8, lambda x, y: list(pixel(x, y)), 3, interlaced=True)
    write_png("gray2i", 0, 2, lambda x, y: [gray(x, y) >> 6], 1, interlaced=True)


make_jpegs()
make_pngs()
