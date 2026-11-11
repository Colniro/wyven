"""Draws Wyven's icon (a gold "W" stroke on the dark editor background) without any libraries.

Writes assets/wyven.ico (16-256 px, PNG entries) and assets/icon-64.rgba (raw RGBA for the
window icon). Run: python assets/make_icon.py
"""
import math
import os
import struct
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
BG = (0x16, 0x18, 0x1D)
EDGE = (0x2A, 0x2F, 0x38)
GOLD = (0xE0, 0xB4, 0x6C)
# The "W": a wing-like zigzag, in 0..1 coordinates.
POINTS = [(0.17, 0.27), (0.35, 0.75), (0.5, 0.43), (0.65, 0.75), (0.83, 0.27)]
STROKE = 0.115
RADIUS = 0.22


def seg_dist(px, py, a, b):
    ax, ay = a
    bx, by = b
    dx, dy = bx - ax, by - ay
    t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    return math.hypot(px - (ax + t * dx), py - (ay + t * dy))


def inside_rounded(px, py, inset=0.0):
    r = RADIUS - inset
    lo, hi = inset, 1.0 - inset
    if px < lo or px > hi or py < lo or py > hi:
        return False
    cx = min(max(px, lo + r), hi - r)
    cy = min(max(py, lo + r), hi - r)
    return math.hypot(px - cx, py - cy) <= r


def render(size, ss=4):
    pixels = bytearray()
    for y in range(size):
        for x in range(size):
            acc = [0.0, 0.0, 0.0, 0.0]
            for sy in range(ss):
                for sx in range(ss):
                    px = (x + (sx + 0.5) / ss) / size
                    py = (y + (sy + 0.5) / ss) / size
                    if not inside_rounded(px, py):
                        continue
                    if min(seg_dist(px, py, POINTS[i], POINTS[i + 1]) for i in range(len(POINTS) - 1)) <= STROKE / 2:
                        c = GOLD
                    elif not inside_rounded(px, py, 0.035):
                        c = EDGE
                    else:
                        c = BG
                    acc[0] += c[0]
                    acc[1] += c[1]
                    acc[2] += c[2]
                    acc[3] += 255
            n = ss * ss
            a = acc[3] / n
            if a == 0:
                pixels += bytes(4)
            else:
                cov = acc[3] / 255
                pixels += bytes([round(acc[0] / cov), round(acc[1] / cov), round(acc[2] / cov), round(a)])
    return bytes(pixels)


def png(size, rgba):
    raw = b"".join(b"\0" + rgba[y * size * 4:(y + 1) * size * 4] for y in range(size))

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def main():
    sizes = [16, 24, 32, 48, 64, 128, 256]
    images = {s: render(s) for s in sizes}
    pngs = [png(s, images[s]) for s in sizes]
    header = struct.pack("<HHH", 0, 1, len(sizes))
    offset = 6 + 16 * len(sizes)
    entries = b""
    for s, data in zip(sizes, pngs):
        entries += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    with open(os.path.join(HERE, "wyven.ico"), "wb") as f:
        f.write(header + entries + b"".join(pngs))
    with open(os.path.join(HERE, "icon-64.rgba"), "wb") as f:
        f.write(images[64])
    with open(os.path.join(HERE, "icon-256.png"), "wb") as f:
        f.write(pngs[-1])
    print("wrote wyven.ico, icon-64.rgba, icon-256.png")


if __name__ == "__main__":
    main()
