"""Make the codestreams in crates/vkstorm-openjp2/tests/fixtures that exercise
the OpenJPEG changes between v2.5.2 and the fork baseline.

usage: python -I baseline_fixtures.py <opj_compress from the fork at the baseline> <out dir>

Sources are integer patterns generated here; the fork's opj_compress encodes
them (tiles, tile-parts, TLM, SOP/EPH, Cinema 2K), and the special cases are
crafted from those by rewriting markers:
- tp_interleaved_64: tile-parts interleaved across tiles, TNsot only in each
  tile's last tile-part (upstream tests/test_tile_part_interleaved.c, #1558);
- tnsot_one_256: every SOT declares TNsot = 1 (the TPsot==TNsot probe is
  skipped for single tile-part tiles since 497d58c1);
- mco_zero_256: an MCO marker segment with zero stages in the main header
  (91966314);
- tlm_invalid_256: Stlm's ST field set to 3 (the TLM table is ignored with a
  warning, 954c6e3c);
- eph_missing_256: the first EPH marker overwritten (EPH required, c58bc128).
The reference `.opj.raw` outputs come from c/opj_bench built against the same
fork commit (see README.md)."""
import os, struct, subprocess, sys

opj_compress, d = sys.argv[1], sys.argv[2]
os.makedirs(d, exist_ok=True)
SOT, SOD, EOC, COD, TLM = 0xFF90, 0xFF93, 0xFFD9, 0xFF52, 0xFF55

def path(n): return os.path.join(d, n)
def rd(n): return bytearray(open(path(n), "rb").read())
def wr(n, b): open(path(n), "wb").write(bytes(b))
def be16(b, p): return (b[p] << 8) | b[p + 1]
def be32(b, p): return struct.unpack(">I", bytes(b[p:p + 4]))[0]

def compress(src, dst, *args):
    subprocess.run([opj_compress, "-i", path(src), "-o", path(dst), *args], check=True, stdout=subprocess.DEVNULL)

# ---- sources (integer patterns, so tests can rebuild the rasters exactly)
px = bytearray()
for y in range(256):
    for x in range(256):
        px += bytes(((x * 7 + y * 3) & 0xFF, ((x * x + y * y) >> 5) & 0xFF, ((x ^ y) * 5 + ((x * y) >> 6)) & 0xFF))
open(path("src_256.ppm"), "wb").write(b"P6\n256 256\n255\n" + px)
px = bytearray((x * 3 + y * 5) & 0xFF for y in range(64) for x in range(64))
open(path("src_64.pgm"), "wb").write(b"P5\n64 64\n255\n" + px)
px = bytearray()
for y in range(256):
    for x in range(256):
        px += struct.pack(">HHH", (x * 13 + y * 5) & 0xFFF, ((x * x + y * y) >> 3) & 0xFFF, ((x ^ y) * 9 + ((x * y) >> 4)) & 0xFFF)
open(path("src12_256.ppm"), "wb").write(b"P6\n256 256\n4095\n" + px)

# ---- encodes with the fork (viewer-like layers: RLCP, 9/7, rates 128,32,8)
viewer = ["-n", "4", "-r", "128,32,8", "-I", "-p", "RLCP"]
compress("src_256.ppm", "plain_256.j2c", *viewer)
compress("src_256.ppm", "tlm_tiles_256.j2c", *viewer, "-t", "64,64", "-TP", "R", "-TLM")
compress("src_256.ppm", "tp_r_256.j2k", *viewer, "-TP", "R")
compress("src_256.ppm", "eph_sop_256.j2c", *viewer, "-EPH", "-SOP")
compress("src_64.pgm", "tp_contig_64.j2k", "-t", "32,32", "-n", "6", "-TP", "R")
compress("src12_256.ppm", "cinema2k_256.j2c", "-cinema2K", "24")

# ---- crafting
def main_header_segments(buf):
    """(marker, offset, length including the marker) up to the first SOT."""
    segs, pos = [], 2
    while be16(buf, pos) != SOT:
        segs.append((be16(buf, pos), pos, be16(buf, pos + 2) + 2))
        pos += be16(buf, pos + 2) + 2
    return segs, pos

def tile_parts(buf, first_sot):
    """[(offset, psot, isot, tpsot, tnsot)] in stream order, and the EOC offset."""
    pos, tps = first_sot, []
    while pos + 12 <= len(buf) and be16(buf, pos) == SOT:
        psot = be32(buf, pos + 6)
        tps.append((pos, psot, be16(buf, pos + 4), buf[pos + 10], buf[pos + 11]))
        pos += psot
    assert be16(buf, pos) == EOC, "no EOC after the tile-parts"
    return tps, pos

def interleave(src, dst):
    buf = rd(src)
    _, first = main_header_segments(buf)
    tps, _ = tile_parts(buf, first)
    per_tile = {}
    for off, psot, isot, tpsot, tnsot in tps:
        per_tile[isot] = per_tile.get(isot, 0) + 1
    assert len(per_tile) >= 2 and min(per_tile.values()) >= 2
    for off, psot, isot, tpsot, tnsot in tps:
        buf[off + 11] = per_tile[isot] if tpsot + 1 == per_tile[isot] else 0
    out = bytearray(buf[:first])
    for rnd in range(max(per_tile.values())):
        for off, psot, isot, tpsot, tnsot in tps:
            if tpsot == rnd:
                out += buf[off:off + psot]
    out += b"\xff\xd9"
    assert len(out) == len(buf)
    wr(dst, out)

def tnsot_one(src, dst):
    buf = rd(src)
    _, first = main_header_segments(buf)
    tps, _ = tile_parts(buf, first)
    assert len(tps) >= 2
    for off, psot, isot, tpsot, tnsot in tps:
        buf[off + 11] = 1
    wr(dst, buf)

def mco_zero(src, dst):
    buf = rd(src)
    _, first = main_header_segments(buf)
    wr(dst, buf[:first] + bytes([0xFF, 0x77, 0x00, 0x03, 0x00]) + buf[first:])

def tlm_invalid(src, dst):
    buf = rd(src)
    segs, _ = main_header_segments(buf)
    tlms = [s for s in segs if s[0] == TLM]
    assert tlms, "no TLM marker"
    buf[tlms[0][1] + 5] |= 0x30
    wr(dst, buf)

def eph_missing(src, dst):
    buf = rd(src)
    segs, first = main_header_segments(buf)
    scod = buf[[s for s in segs if s[0] == COD][0][1] + 4]
    assert scod & 0x04, "COD does not announce EPH"
    tps, _ = tile_parts(buf, first)
    pos = tps[0][0] + 12
    while be16(buf, pos) != SOD:
        pos += be16(buf, pos + 2) + 2
    p = pos + 2
    if scod & 0x02:
        assert be16(buf, p) == 0xFF91
        p += 6
    eph = next(q for q in range(p, p + 64) if buf[q] == 0xFF and buf[q + 1] == 0x92)
    buf[eph] = 0x00
    wr(dst, buf)

interleave("tp_contig_64.j2k", "tp_interleaved_64.j2c")
tnsot_one("tp_r_256.j2k", "tnsot_one_256.j2c")
mco_zero("plain_256.j2c", "mco_zero_256.j2c")
tlm_invalid("tlm_tiles_256.j2c", "tlm_invalid_256.j2c")
eph_missing("eph_sop_256.j2c", "eph_missing_256.j2c")
for n in ("tp_contig_64.j2k", "tp_r_256.j2k", "src_256.ppm", "src_64.pgm", "src12_256.ppm"):
    os.remove(path(n))
print("fixtures in", d)
