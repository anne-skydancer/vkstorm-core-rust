#!/usr/bin/env python3
"""Deterministic source images for the vkstorm-image parity fixtures and
benchmarks: raw rasters (VKSR), PNG variants, TGA variants and BMP variants,
written with hand-rolled encoders (stdlib only) so every byte is reproducible.

usage: gen_sources.py <out_dir> [--big]

Without --big the images are small (fixture size); with --big it also writes
the 2048x2048 benchmark rasters.
"""
import math
import os
import struct
import sys
import zlib


# ---------------------------------------------------------------------------
# Rasters: bottom-up rows, interleaved components, deterministic content.
# ---------------------------------------------------------------------------
def lcg(seed):
    state = seed & 0xFFFFFFFF
    while True:
        state = (1103515245 * state + 12345) & 0x7FFFFFFF
        yield state >> 16


def pattern(w, h, c, seed=1, opaque=False):
    """Photo-like content: gradients, two sinusoids and mild noise; alpha varies
    unless opaque."""
    rnd = lcg(seed)
    out = bytearray(w * h * c)
    i = 0
    for y in range(h):
        for x in range(w):
            fx, fy = x / max(1, w - 1), y / max(1, h - 1)
            base = [
                0.5 + 0.45 * math.sin(fx * 6.0 + seed) * math.cos(fy * 4.0),
                fy * 0.9 + 0.05 * math.sin(fx * 40.0),
                0.5 + 0.5 * math.sin((fx + fy) * 9.0 + 0.3 * seed),
                0.3 + 0.7 * fx,
            ]
            for k in range(c):
                v = base[k if c > 2 else (0 if k == 0 else 3)]
                if c == 1:
                    v = base[1]
                n = (next(rnd) & 15) - 8
                val = int(round(v * 255)) + n
                if opaque and ((c == 4 and k == 3) or (c == 2 and k == 1)):
                    val = 255
                out[i] = max(0, min(255, val))
                i += 1
    return bytes(out)


def write_raw(path, w, h, c, data):
    assert len(data) == w * h * c
    with open(path, "wb") as f:
        f.write(b"VKSR" + struct.pack("<III", w, h, c) + data)


def rows_top_down(w, h, c, data):
    """Scanlines of a bottom-up raster in top-down order."""
    row = w * c
    return [data[(h - 1 - y) * row:(h - y) * row] for y in range(h)]


# ---------------------------------------------------------------------------
# PNG writer: any bit depth / colour type, filter 0, optional chunks.
# ---------------------------------------------------------------------------
def png_chunk(kind, body):
    c = struct.pack(">I", len(body)) + kind + body
    return c + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)


def pack_samples(samples, bit_depth):
    """Pack a row of integer samples at the bit depth (big-endian, MSB first)."""
    if bit_depth == 8:
        return bytes(samples)
    if bit_depth == 16:
        return b"".join(struct.pack(">H", s) for s in samples)
    out = bytearray()
    per = 8 // bit_depth
    for i in range(0, len(samples), per):
        byte = 0
        chunk = samples[i:i + per]
        for j, s in enumerate(chunk):
            byte |= (s & ((1 << bit_depth) - 1)) << (8 - bit_depth * (j + 1))
        out.append(byte)
    return bytes(out)


def write_png(path, w, h, color_type, bit_depth, rows, palette=None, trns=None, gama=None, sbit=None):
    """rows: list (top-down) of lists of integer samples (channels interleaved)."""
    out = bytearray(b"\x89PNG\r\n\x1a\n")
    out += png_chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, bit_depth, color_type, 0, 0, 0))
    if gama is not None:
        out += png_chunk(b"gAMA", struct.pack(">I", gama))
    if sbit is not None:
        out += png_chunk(b"sBIT", bytes(sbit))
    if palette is not None:
        out += png_chunk(b"PLTE", bytes(palette))
    if trns is not None:
        out += png_chunk(b"tRNS", bytes(trns))
    raw = b"".join(b"\x00" + pack_samples(r, bit_depth) for r in rows)
    out += png_chunk(b"IDAT", zlib.compress(raw, 6))
    out += png_chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(out)


def png_variants(out, w, h):
    """Every colour type and bit depth libpng's transformations touch."""
    rgb = pattern(w, h, 3, seed=3)
    rgba = pattern(w, h, 4, seed=4)
    gray = pattern(w, h, 1, seed=5)
    ga = pattern(w, h, 2, seed=6)
    r3 = rows_top_down(w, h, 3, rgb)
    r4 = rows_top_down(w, h, 4, rgba)
    r1 = rows_top_down(w, h, 1, gray)
    r2 = rows_top_down(w, h, 2, ga)

    def samples(rows):
        return [list(r) for r in rows]

    def scaled(rows, bits):
        shift = 8 - bits
        return [[s >> shift for s in r] for r in rows]

    def to16(rows):
        # 16-bit samples with a varying low byte so the strip matters.
        return [[(s << 8) | ((s * 37 + i) & 0xFF) for i, s in enumerate(r)] for r in rows]

    names = {}
    names["png_rgb8"] = (2, 8, samples(r3), {})
    names["png_rgba8"] = (6, 8, samples(r4), {})
    names["png_gray8"] = (0, 8, samples(r1), {})
    names["png_ga8"] = (4, 8, samples(r2), {})
    names["png_rgb16"] = (2, 16, to16(samples(r3)), {})
    names["png_rgba16"] = (6, 16, to16(samples(r4)), {})
    names["png_gray16"] = (0, 16, to16(samples(r1)), {})
    names["png_ga16"] = (4, 16, to16(samples(r2)), {})
    for bits in (1, 2, 4):
        names["png_gray%d" % bits] = (0, bits, scaled(samples(r1), bits), {})
        trans = (1 << bits) - 1
        names["png_gray%d_trns" % bits] = (0, bits, scaled(samples(r1), bits), {"trns": struct.pack(">H", trans)})
    # Transparent colour: the first pixel's value so it is certainly present.
    names["png_gray8_trns"] = (0, 8, samples(r1), {"trns": struct.pack(">H", r1[0][0])})
    names["png_rgb8_trns"] = (2, 8, samples(r3), {"trns": struct.pack(">HHH", r3[0][0], r3[0][1], r3[0][2])})
    s16 = to16(samples(r3))
    names["png_rgb16_trns"] = (2, 16, s16, {"trns": struct.pack(">HHH", s16[0][0], s16[0][1], s16[0][2])})
    g16 = to16(samples(r1))
    names["png_gray16_trns"] = (0, 16, g16, {"trns": struct.pack(">H", g16[0][0])})
    # Palettes: 256 entries from the pattern, indexes from the grey image.
    palette = bytes(pattern(16, 16, 3, seed=9))
    for bits in (1, 2, 4, 8):
        idx = scaled(samples(r1), bits)
        names["png_pal%d" % bits] = (3, bits, idx, {"palette": palette})
        n = 1 << bits
        trns = bytes(((i * 97) & 0xFF) for i in range(n // 2))  # partial tRNS: the rest is opaque
        names["png_pal%d_trns" % bits] = (3, bits, idx, {"palette": palette, "trns": trns})
    # Gamma: insignificant values (no correction) and significant ones.
    for gama in (45455, 45000, 100000, 55531, 50000, 220000, 30000):
        names["png_rgb8_gama%d" % gama] = (2, 8, samples(r3), {"gama": gama})
        names["png_rgba8_gama%d" % gama] = (6, 8, samples(r4), {"gama": gama})
        names["png_rgb16_gama%d" % gama] = (2, 16, to16(samples(r3)), {"gama": gama})
        names["png_gray8_gama%d" % gama] = (0, 8, samples(r1), {"gama": gama})
    names["png_pal8_gama100000"] = (3, 8, scaled(samples(r1), 8), {"palette": palette, "gama": 100000})
    names["png_pal4_trns_gama100000"] = (3, 4, scaled(samples(r1), 4), {"palette": palette, "trns": bytes(range(0, 128, 8)), "gama": 100000})
    names["png_gray4_trns_gama100000"] = (0, 4, scaled(samples(r1), 4), {"trns": struct.pack(">H", 15), "gama": 100000})
    names["png_gray16_gama100000"] = (0, 16, to16(samples(r1)), {"gama": 100000})
    names["png_ga16_gama100000"] = (4, 16, to16(samples(r2)), {"gama": 100000})
    names["png_rgba16_gama100000"] = (6, 16, to16(samples(r4)), {"gama": 100000})
    # sBIT changes the 16-bit gamma shift.
    names["png_rgb16_gama100000_sbit8"] = (2, 16, to16(samples(r3)), {"gama": 100000, "sbit": [8, 8, 8]})
    names["png_rgb16_gama100000_sbit12"] = (2, 16, to16(samples(r3)), {"gama": 100000, "sbit": [12, 10, 12]})
    names["png_gray16_gama100000_sbit5"] = (0, 16, to16(samples(r1)), {"gama": 100000, "sbit": [5]})
    for name, (ct, bd, rows, extra) in names.items():
        write_png(os.path.join(out, name + ".png"), w, h, ct, bd, rows, **extra)
    # Not a PNG at all.
    with open(os.path.join(out, "png_junk.png"), "wb") as f:
        f.write(b"\x89PNX" + b"\x5a" * 60)
    with open(os.path.join(out, "png_truncated.png"), "wb") as f:
        with open(os.path.join(out, "png_rgb8.png"), "rb") as g:
            f.write(g.read()[:60])


# ---------------------------------------------------------------------------
# TGA writer.
# ---------------------------------------------------------------------------
def tga_rle(pixels, bpp):
    """Run-length packets within one row (never crossing rows)."""
    out = bytearray()
    i = 0
    n = len(pixels)
    while i < n:
        run = 1
        while i + run < n and run < 128 and pixels[i + run] == pixels[i]:
            run += 1
        if run >= 2:
            out.append(0x80 | (run - 1))
            out += pixels[i]
            i += run
        else:
            j = i + 1
            while j < n and j - i < 128 and not (j + 1 < n and pixels[j] == pixels[j + 1]):
                j += 1
            out.append((j - i) - 1)
            for p in pixels[i:j]:
                out += p
            i = j
    return bytes(out)


def write_tga(path, w, h, image_type, pixel_bytes_rows, pixel_size, origin_top=False, id_field=b"",
              cmap=None, cmap_depth=0, cmap_start=0, cmap_type=0, attr_bits=None, rle=False):
    """pixel_bytes_rows: list (bottom-up unless origin_top, in file order) of
    lists of per-pixel byte strings already in TGA byte order."""
    if attr_bits is None:
        attr_bits = 8 if pixel_size == 32 else 0
    flags = (attr_bits & 0xF) | (0x20 if origin_top else 0)
    cmap_len = 0 if cmap is None else len(cmap)
    hdr = struct.pack("<BBBHHBHHHHBB", len(id_field), cmap_type, image_type, cmap_start, cmap_len, cmap_depth,
                      0, 0, w, h, pixel_size, flags)
    body = bytearray(hdr + id_field)
    if cmap is not None:
        for entry in cmap:
            body += entry
    for row in pixel_bytes_rows:
        if rle:
            body += tga_rle(row, pixel_size // 8)
        else:
            for p in row:
                body += p
    with open(path, "wb") as f:
        f.write(body)


def to15(r, g, b):
    return struct.pack("<H", ((r >> 3) << 10) | ((g >> 3) << 5) | (b >> 3))


def tga_variants(out, w, h):
    rgb = pattern(w, h, 3, seed=13)
    rgba = pattern(w, h, 4, seed=14)
    rgba_opaque = pattern(w, h, 4, seed=14, opaque=True)
    gray = pattern(w, h, 1, seed=15)
    # Quantise so runs exist: every 3 pixels share a value in the RLE variants.
    def runs(data, c):
        d = bytearray(data)
        for i in range(0, len(d) - c * 3, c * 3):
            for k in range(c):
                d[i + c + k] = d[i + k]
                d[i + 2 * c + k] = d[i + k]
        return bytes(d)

    def px_rows(data, c, conv, top=False):
        rows = []
        row = w * c
        for y in range(h):
            r = data[y * row:(y + 1) * row]
            rows.append([conv(r[x * c:(x + 1) * c]) for x in range(w)])
        return rows[::-1] if top else rows  # file order: first row stored is bottom (or top)

    bgr = lambda p: bytes([p[2], p[1], p[0]])
    bgra = lambda p: bytes([p[2], p[1], p[0], p[3]])
    g1 = lambda p: bytes([p[0]])
    p15 = lambda p: to15(p[0], p[1], p[2])
    for top in (False, True):
        sfx = "_top" if top else ""
        for rle in (False, True):
            t = 10 if rle else 2
            tag = "_rle" if rle else ""
            src3 = runs(rgb, 3) if rle else rgb
            src4 = runs(rgba, 4) if rle else rgba
            src4o = runs(rgba_opaque, 4) if rle else rgba_opaque
            write_tga(os.path.join(out, "tga_24%s%s.tga" % (tag, sfx)), w, h, t, px_rows(src3, 3, bgr, top), 24, top, rle=rle)
            write_tga(os.path.join(out, "tga_32%s%s.tga" % (tag, sfx)), w, h, t, px_rows(src4, 4, bgra, top), 32, top, rle=rle)
            write_tga(os.path.join(out, "tga_32opaque%s%s.tga" % (tag, sfx)), w, h, t, px_rows(src4o, 4, bgra, top), 32, top, rle=rle)
            write_tga(os.path.join(out, "tga_16%s%s.tga" % (tag, sfx)), w, h, t, px_rows(src3, 3, p15, top), 16, top, rle=rle)
            write_tga(os.path.join(out, "tga_15%s%s.tga" % (tag, sfx)), w, h, t, px_rows(src3, 3, p15, top), 15, top, rle=rle)
            tm = 11 if rle else 3
            srcg = runs(gray, 1) if rle else gray
            write_tga(os.path.join(out, "tga_8%s%s.tga" % (tag, sfx)), w, h, tm, px_rows(srcg, 1, g1, top), 8, top, rle=rle)
    # ID field and attribute bits unset on 32-bit (the viewer forces them).
    write_tga(os.path.join(out, "tga_32_id.tga"), w, h, 2, px_rows(rgba, 4, bgra), 32, id_field=b"vkstorm fixture", attr_bits=0)
    write_tga(os.path.join(out, "tga_24_id_top.tga"), w, h, 2, px_rows(rgb, 3, bgr, True), 24, True, id_field=b"id")
    # Colour-mapped: 8-bit indexes into maps of 8, 15, 16, 24 and 32 bits.
    idx = bytes(pattern(w, h, 1, seed=21))
    idx_runs = runs(idx, 1)
    pal = pattern(16, 16, 4, seed=22)
    maps = {
        "cm8": (8, [bytes([pal[i * 4]]) for i in range(256)]),
        "cm15": (15, [to15(pal[i * 4], pal[i * 4 + 1], pal[i * 4 + 2]) for i in range(256)]),
        "cm16": (16, [to15(pal[i * 4], pal[i * 4 + 1], pal[i * 4 + 2]) for i in range(256)]),
        "cm24": (24, [bytes([pal[i * 4 + 2], pal[i * 4 + 1], pal[i * 4]]) for i in range(256)]),
        "cm32": (32, [bytes([pal[i * 4 + 2], pal[i * 4 + 1], pal[i * 4], pal[i * 4 + 3]]) for i in range(256)]),
    }
    for name, (depth, cmap) in maps.items():
        for top in (False, True):
            sfx = "_top" if top else ""
            write_tga(os.path.join(out, "tga_%s%s.tga" % (name, sfx)), w, h, 1, px_rows(idx, 1, g1, top), 8, top, cmap=cmap, cmap_depth=depth, cmap_type=1)
            write_tga(os.path.join(out, "tga_%s_rle%s.tga" % (name, sfx)), w, h, 9, px_rows(idx_runs, 1, g1, top), 8, top, cmap=cmap, cmap_depth=depth, cmap_type=1, rle=True)
    # A short map with a start offset: indexes clamp.
    short_map = maps["cm24"][1][:64]
    write_tga(os.path.join(out, "tga_cm24_short.tga"), w, h, 1, px_rows(idx, 1, g1), 8, cmap=short_map, cmap_depth=24, cmap_start=100, cmap_type=1)
    # A true-colour file carrying an unused colour map.
    write_tga(os.path.join(out, "tga_24_unusedmap.tga"), w, h, 2, px_rows(rgb, 3, bgr), 24, cmap=maps["cm24"][1][:16], cmap_depth=24, cmap_type=1)
    # Errors the viewer reports.
    write_tga(os.path.join(out, "tga_err_type0.tga"), w, h, 0, px_rows(gray, 1, g1), 8)
    write_tga(os.path.join(out, "tga_err_type4.tga"), w, h, 4, px_rows(gray, 1, g1), 8)
    write_tga(os.path.join(out, "tga_err_mono16.tga"), w, h, 3, px_rows(rgb, 3, p15), 16)
    write_tga(os.path.join(out, "tga_err_cmapped24.tga"), w, h, 1, px_rows(rgb, 3, bgr), 24, cmap=maps["cm24"][1], cmap_depth=24, cmap_type=1)
    write_tga(os.path.join(out, "tga_err_pixelsize12.tga"), w, h, 2, px_rows(rgb, 3, p15), 12)
    # Origin on the right: parses, refuses to decode.
    with open(os.path.join(out, "tga_err_right.tga"), "wb") as f:
        d = bytearray(open(os.path.join(out, "tga_24.tga"), "rb").read())
        d[17] |= 0x10
        f.write(d)
    with open(os.path.join(out, "tga_err_truncated.tga"), "wb") as f:
        f.write(open(os.path.join(out, "tga_24_rle.tga"), "rb").read()[:18 + 40])
    with open(os.path.join(out, "tga_err_truncated_raw.tga"), "wb") as f:
        f.write(open(os.path.join(out, "tga_24.tga"), "rb").read()[:18 + 40])


# ---------------------------------------------------------------------------
# BMP writer.
# ---------------------------------------------------------------------------
def write_bmp(path, w, h, bpp, rows_bottom_up, palette=None, compression=0, masks=None, header_size=40,
              origin_top=False, num_colors=None):
    """rows_bottom_up: list of bytes per row (unpadded pixel bytes, bottom row first)."""
    pal = b"" if palette is None else palette
    ext = b""
    if header_size == 40 and compression == 3:
        ext = struct.pack("<III", *masks[:3])
    if header_size >= 108:
        m = masks if masks else (0, 0, 0, 0)
        ext = struct.pack("<IIIII", m[0], m[1], m[2], m[3], 0x73524742) + b"\x00" * (header_size - 40 - 20)
    offset = 14 + header_size + len(pal)
    stride = (w * bpp // 8 + 3) & ~3
    body = bytearray()
    for r in rows_bottom_up:
        body += r + b"\x00" * (stride - len(r))
    size = offset + len(body)
    nc = (len(pal) // 4 if num_colors is None else num_colors)
    hdr = struct.pack("<IiiHHIIiiII", header_size if header_size >= 40 else 40, w, -h if origin_top else h, 1, bpp, compression,
                      len(body), 0, 0, nc, 0)
    if header_size == 12:
        hdr = struct.pack("<IHHHH", 12, w, h, 1, bpp)
        offset = 14 + 12 + len(pal)
        size = offset + len(body)
    with open(path, "wb") as f:
        f.write(b"BM" + struct.pack("<IHHI", size, 0, 0, offset) + hdr[:header_size if header_size < 40 else 40] + ext + pal + body)


def bmp_variants(out, w, h):
    rgb = pattern(w, h, 3, seed=31)
    rgba = pattern(w, h, 4, seed=32)
    gray = pattern(w, h, 1, seed=33)
    row3 = w * 3

    def rows_bgr(top=False):
        rows = [bytes(b for x in range(w) for b in (rgb[y * row3 + x * 3 + 2], rgb[y * row3 + x * 3 + 1], rgb[y * row3 + x * 3])) for y in range(h)]
        return rows[::-1] if top else rows

    def rows_bgra(order=(2, 1, 0, 3)):
        return [bytes(rgba[y * w * 4 + x * 4 + k] for x in range(w) for k in order) for y in range(h)]

    rows8 = [gray[y * w:(y + 1) * w] for y in range(h)]
    pal256 = b"".join(bytes([(i * 3) & 255, (i * 7) & 255, (i * 11) & 255, 0]) for i in range(256))
    write_bmp(os.path.join(out, "bmp_24.bmp"), w, h, 24, rows_bgr())
    write_bmp(os.path.join(out, "bmp_24_top.bmp"), w, h, 24, rows_bgr(True), origin_top=True)
    write_bmp(os.path.join(out, "bmp_8.bmp"), w, h, 8, rows8, palette=pal256)
    write_bmp(os.path.join(out, "bmp_8_nc0.bmp"), w, h, 8, rows8, palette=pal256, num_colors=0)
    write_bmp(os.path.join(out, "bmp_8_top.bmp"), w, h, 8, rows8[::-1], palette=pal256, origin_top=True)
    write_bmp(os.path.join(out, "bmp_32.bmp"), w, h, 32, rows_bgra())
    write_bmp(os.path.join(out, "bmp_32_bitfields.bmp"), w, h, 32, rows_bgra(), compression=3, masks=(0x00FF0000, 0x0000FF00, 0x000000FF))
    # Masks in an unusual order: file bytes are R,G,B,A per pixel.
    write_bmp(os.path.join(out, "bmp_32_bitfields_rgba.bmp"), w, h, 32, rows_bgra((0, 1, 2, 3)), compression=3, masks=(0x000000FF, 0x0000FF00, 0x00FF0000))
    write_bmp(os.path.join(out, "bmp_24_v4.bmp"), w, h, 24, rows_bgr(), header_size=108)
    write_bmp(os.path.join(out, "bmp_24_v5.bmp"), w, h, 24, rows_bgr(), header_size=124)
    write_bmp(os.path.join(out, "bmp_32_v4_bitfields.bmp"), w, h, 32, rows_bgra(), header_size=108, compression=3, masks=(0x00FF0000, 0x0000FF00, 0x000000FF, 0xFF000000))
    # The viewer reads a V4 file's colour table from the wrong offset (52
    # bytes into the 108-byte header): pinned as behaviour.
    write_bmp(os.path.join(out, "bmp_8_v4_quirk.bmp"), w, h, 8, rows8, palette=pal256, header_size=108)
    # Errors.
    write_bmp(os.path.join(out, "bmp_err_8_pal16.bmp"), w, h, 8, rows8, palette=pal256[:64])
    rows4 = [bytes(((gray[y * w + x] >> 4) << 4) | (gray[y * w + x + 1] >> 4 if x + 1 < w else 0) for x in range(0, w, 2)) for y in range(h)]
    write_bmp(os.path.join(out, "bmp_err_4.bmp"), w, h, 4, rows4, palette=pal256[:64])
    rows16 = [b"".join(to15(rgb[y * row3 + x * 3], rgb[y * row3 + x * 3 + 1], rgb[y * row3 + x * 3 + 2]) for x in range(w)) for y in range(h)]
    write_bmp(os.path.join(out, "bmp_err_16.bmp"), w, h, 16, rows16)
    write_bmp(os.path.join(out, "bmp_err_rle8.bmp"), w, h, 8, rows8, palette=pal256, compression=1)
    write_bmp(os.path.join(out, "bmp_err_os2.bmp"), w, h, 24, rows_bgr(), header_size=12)
    write_bmp(os.path.join(out, "bmp_err_os2v2.bmp"), w, h, 24, rows_bgr(), header_size=64)
    write_bmp(os.path.join(out, "bmp_err_bitfields24.bmp"), w, h, 24, rows_bgr(), compression=3, masks=(0xFF0000, 0xFF00, 0xFF))
    with open(os.path.join(out, "bmp_err_truncated.bmp"), "wb") as f:
        f.write(open(os.path.join(out, "bmp_24.bmp"), "rb").read()[:-7])
    with open(os.path.join(out, "bmp_err_notbmp.bmp"), "wb") as f:
        f.write(b"XY" + b"\x00" * 60)
    with open(os.path.join(out, "bmp_err_ba.bmp"), "wb") as f:
        f.write(b"BA" + b"\x00" * 60)


# ---------------------------------------------------------------------------
# Raw rasters for the encoders and for DXT.
# ---------------------------------------------------------------------------
def raw_variants(out, big):
    sizes = [(64, 64), (33, 17), (1, 1), (5, 3), (100, 37)]
    for w, h in sizes:
        for c in (1, 2, 3, 4):
            write_raw(os.path.join(out, "raw_%dx%d_c%d.raw" % (w, h, c)), w, h, c, pattern(w, h, c, seed=40 + c))
    write_raw(os.path.join(out, "raw_64x64_c4_opaque.raw"), 64, 64, 4, pattern(64, 64, 4, seed=44, opaque=True))
    write_raw(os.path.join(out, "raw_16x8_c3.raw"), 16, 8, 3, pattern(16, 8, 3, seed=45))
    write_raw(os.path.join(out, "raw_7x5_c4.raw"), 7, 5, 4, pattern(7, 5, 4, seed=46))
    write_raw(os.path.join(out, "raw_8x4_c1.raw"), 8, 4, 1, pattern(8, 4, 1, seed=47))
    # Flat and noisy extremes for the PNG filter heuristic.
    write_raw(os.path.join(out, "raw_64x64_c3_flat.raw"), 64, 64, 3, bytes([120, 30, 200]) * (64 * 64))
    rnd = lcg(99)
    write_raw(os.path.join(out, "raw_64x64_c3_noise.raw"), 64, 64, 3, bytes(next(rnd) & 255 for _ in range(64 * 64 * 3)))
    if big:
        for c in (3, 4):
            write_raw(os.path.join(out, "big_2048_c%d.raw" % c), 2048, 2048, c, pattern(2048, 2048, c, seed=50 + c))
        write_raw(os.path.join(out, "big_2048_c1.raw"), 2048, 2048, 1, pattern(2048, 2048, 1, seed=51))
        write_raw(os.path.join(out, "big_64_c3.raw"), 64, 64, 3, pattern(64, 64, 3, seed=53))
        write_raw(os.path.join(out, "big_64_c4.raw"), 64, 64, 4, pattern(64, 64, 4, seed=54))
        write_raw(os.path.join(out, "big_64_c1.raw"), 64, 64, 1, pattern(64, 64, 1, seed=55))


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    out = sys.argv[1]
    big = "--big" in sys.argv
    os.makedirs(out, exist_ok=True)
    if not big:
        png_variants(out, 37, 23)
        tga_variants(out, 37, 23)
        bmp_variants(out, 37, 23)
    raw_variants(out, big)
    return 0


if __name__ == "__main__":
    sys.exit(main())
