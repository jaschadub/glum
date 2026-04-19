#!/usr/bin/env python3
"""Generate glum.ico from glum-solo-lg.png using only the Python stdlib.

Reads a non-interlaced 8-bit PNG (any color type), unfilters scanlines,
box-downsamples to standard ICO sizes, re-encodes each as PNG, and packs
them into an ICO file with PNG-compressed entries (Vista+ format).

Run from repo root:
    python3 scripts/build_ico.py
"""

from __future__ import annotations

import struct
import sys
import zlib
from pathlib import Path

ICO_SIZES = [256, 128, 64, 48, 32, 16]
SRC = Path(__file__).resolve().parent.parent / "glum-solo-lg.png"
DST = Path(__file__).resolve().parent.parent / "assets" / "glum.ico"


def read_png(path: Path) -> tuple[int, int, bytes]:
    """Return (width, height, RGBA bytes) for an 8-bit non-interlaced PNG."""
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("not a PNG")
    pos = 8
    width = height = 0
    bit_depth = color_type = 0
    palette: list[tuple[int, int, int]] = []
    trns: bytes = b""
    idat = bytearray()
    while pos < len(data):
        chunk_len = struct.unpack(">I", data[pos : pos + 4])[0]
        chunk_type = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + chunk_len]
        pos += 12 + chunk_len  # len + type + body + crc
        if chunk_type == b"IHDR":
            width, height = struct.unpack(">II", body[:8])
            bit_depth, color_type = body[8], body[9]
            interlace = body[12]
            if bit_depth != 8 or interlace != 0:
                raise ValueError(
                    f"unsupported PNG: bit_depth={bit_depth} interlace={interlace}"
                )
        elif chunk_type == b"PLTE":
            palette = [
                (body[i], body[i + 1], body[i + 2]) for i in range(0, len(body), 3)
            ]
        elif chunk_type == b"tRNS":
            trns = body
        elif chunk_type == b"IDAT":
            idat.extend(body)
        elif chunk_type == b"IEND":
            break

    raw = zlib.decompress(bytes(idat))
    bpp_map = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}  # bytes per pixel at 8-bit
    if color_type not in bpp_map:
        raise ValueError(f"unsupported color_type {color_type}")
    bpp = bpp_map[color_type]
    stride = width * bpp
    pixels = bytearray(width * height * bpp)
    prev = bytes(stride)
    rpos = 0
    for y in range(height):
        ftype = raw[rpos]
        rpos += 1
        line = bytearray(raw[rpos : rpos + stride])
        rpos += stride
        if ftype == 0:
            pass
        elif ftype == 1:  # Sub
            for x in range(bpp, stride):
                line[x] = (line[x] + line[x - bpp]) & 0xFF
        elif ftype == 2:  # Up
            for x in range(stride):
                line[x] = (line[x] + prev[x]) & 0xFF
        elif ftype == 3:  # Average
            for x in range(stride):
                left = line[x - bpp] if x >= bpp else 0
                line[x] = (line[x] + (left + prev[x]) // 2) & 0xFF
        elif ftype == 4:  # Paeth
            for x in range(stride):
                a = line[x - bpp] if x >= bpp else 0
                b = prev[x]
                c = prev[x - bpp] if x >= bpp else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                if pa <= pb and pa <= pc:
                    pr = a
                elif pb <= pc:
                    pr = b
                else:
                    pr = c
                line[x] = (line[x] + pr) & 0xFF
        else:
            raise ValueError(f"unknown filter type {ftype}")
        pixels[y * stride : (y + 1) * stride] = line
        prev = bytes(line)

    # Convert to RGBA
    if color_type == 6:  # RGBA already
        rgba = bytes(pixels)
    elif color_type == 2:  # RGB → RGBA (opaque)
        out = bytearray(width * height * 4)
        for i in range(width * height):
            out[i * 4 : i * 4 + 3] = pixels[i * 3 : i * 3 + 3]
            out[i * 4 + 3] = 255
        rgba = bytes(out)
    elif color_type == 4:  # Grayscale + alpha
        out = bytearray(width * height * 4)
        for i in range(width * height):
            g, a = pixels[i * 2], pixels[i * 2 + 1]
            out[i * 4 : i * 4 + 4] = bytes((g, g, g, a))
        rgba = bytes(out)
    elif color_type == 0:  # Grayscale, opaque
        out = bytearray(width * height * 4)
        for i in range(width * height):
            g = pixels[i]
            out[i * 4 : i * 4 + 4] = bytes((g, g, g, 255))
        rgba = bytes(out)
    elif color_type == 3:  # Palette
        if not palette:
            raise ValueError("PLTE missing for palette PNG")
        alphas = list(trns) + [255] * (len(palette) - len(trns))
        out = bytearray(width * height * 4)
        for i, idx in enumerate(pixels):
            r, g, b = palette[idx]
            out[i * 4 : i * 4 + 4] = bytes((r, g, b, alphas[idx]))
        rgba = bytes(out)
    else:
        raise ValueError("unreachable")

    return width, height, rgba


def box_resize(src: bytes, sw: int, sh: int, dw: int, dh: int) -> bytes:
    """Box-average downsample RGBA src (sw x sh) to (dw x dh)."""
    out = bytearray(dw * dh * 4)
    # Integer ratios computed as fractions — use floats here, simpler.
    x_ratio = sw / dw
    y_ratio = sh / dh
    for dy in range(dh):
        y0 = int(dy * y_ratio)
        y1 = max(int((dy + 1) * y_ratio), y0 + 1)
        for dx in range(dw):
            x0 = int(dx * x_ratio)
            x1 = max(int((dx + 1) * x_ratio), x0 + 1)
            r = g = b = a = count = 0
            for sy in range(y0, y1):
                row_off = (sy * sw + x0) * 4
                for sx in range(x1 - x0):
                    off = row_off + sx * 4
                    r += src[off]
                    g += src[off + 1]
                    b += src[off + 2]
                    a += src[off + 3]
                    count += 1
            o = (dy * dw + dx) * 4
            out[o] = r // count
            out[o + 1] = g // count
            out[o + 2] = b // count
            out[o + 3] = a // count
    return bytes(out)


def encode_png(rgba: bytes, width: int, height: int) -> bytes:
    """Encode RGBA bytes as a minimal PNG with filter type 0 (None)."""
    sig = b"\x89PNG\r\n\x1a\n"

    def chunk(tag: bytes, body: bytes) -> bytes:
        return (
            struct.pack(">I", len(body))
            + tag
            + body
            + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    stride = width * 4
    raw = bytearray()
    for y in range(height):
        raw.append(0)  # filter type None
        raw.extend(rgba[y * stride : (y + 1) * stride])
    idat = zlib.compress(bytes(raw), 9)
    return sig + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")


def build_ico(images: list[tuple[int, bytes]]) -> bytes:
    """Pack PNG-compressed icon entries into an ICO file."""
    # ICONDIR: reserved=0, type=1 (icon), count
    header = struct.pack("<HHH", 0, 1, len(images))
    entries = bytearray()
    payloads = bytearray()
    offset = 6 + 16 * len(images)
    for size, png in images:
        w = h = 0 if size >= 256 else size
        entry = struct.pack(
            "<BBBBHHII",
            w,  # width (0 = 256)
            h,  # height (0 = 256)
            0,  # palette count
            0,  # reserved
            1,  # color planes
            32,  # bits per pixel
            len(png),
            offset,
        )
        entries.extend(entry)
        payloads.extend(png)
        offset += len(png)
    return bytes(header) + bytes(entries) + bytes(payloads)


def main() -> int:
    sw, sh, rgba = read_png(SRC)
    print(f"loaded {SRC.name}: {sw}x{sh}")
    images = []
    for size in ICO_SIZES:
        print(f"  resizing to {size}x{size}…", flush=True)
        scaled = box_resize(rgba, sw, sh, size, size)
        png = encode_png(scaled, size, size)
        images.append((size, png))
    DST.parent.mkdir(parents=True, exist_ok=True)
    ico = build_ico(images)
    DST.write_bytes(ico)
    print(f"wrote {DST} ({len(ico):,} bytes, {len(images)} sizes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
