#!/usr/bin/env python3
"""Build the parity fixtures for crates/vkstorm-image with the C++ oracle.

usage: make_fixtures.py <oracle.exe> <viewer_indra_dir> <fixtures_dir>

1. Generates the synthetic sources (gen_sources.py) into <fixtures_dir>.
2. Copies a small real file of every PNG/TGA/BMP/JPEG variant found under the
   viewer's skins and character directories.
3. Runs the oracle: every encoded file gets its decoded raster (`.raw`) or the
   viewer's error message (`.err`); every raw raster is encoded with each
   encoder (`<raw>.<ext>`, JPEG at qualities 50, 75 and 95); the monochrome
   run-length TGAs are run through the alpha-gradient processing.
4. Writes manifest.txt, one line per expectation, read by tests/parity.rs.
"""
import collections
import os
import shutil
import struct
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def run(oracle, *args):
    p = subprocess.run([oracle, *args], capture_output=True, text=True)
    out = (p.stdout or "").strip()
    if p.returncode != 0 or out.startswith("FAIL"):
        return None, out[6:] if out.startswith("FAIL: ") else (out or "(no message)")
    return out, None


def classify(path):
    """Variant class of a bundled file, as scan_images did, or None."""
    e = path.lower().rsplit(".", 1)[-1]
    with open(path, "rb") as fh:
        d = fh.read()
    if e == "tga" and len(d) >= 18:
        cmt, it, cmd, ps, fl = d[1], d[2], d[7], d[16], d[17]
        if fl & 0x10:
            return None
        return ("tga", it, cmt, cmd if cmt else 0, ps, bool(fl & 0x20), fl & 0xF, bool(d[0]))
    if e == "png" and d[:8] == b"\x89PNG\r\n\x1a\n":
        w, h, bd, ct, cm, flt, il = struct.unpack(">IIBBBBB", d[16:29])
        chunks = set()
        pos = 8
        gama = None
        while pos + 8 <= len(d):
            ln, ty = struct.unpack(">I4s", d[pos:pos + 8])
            chunks.add(ty)
            if ty == b"gAMA":
                gama = struct.unpack(">I", d[pos + 8:pos + 12])[0]
            if ty in (b"IDAT", b"IEND"):
                break
            pos += 12 + ln
        return ("png", bd, ct, il, b"tRNS" in chunks, gama, b"sRGB" in chunks, b"sBIT" in chunks)
    if e == "bmp" and d[:2] == b"BM":
        hs, w, h, pl, bpp, comp = struct.unpack("<IiiHHI", d[14:34])
        return ("bmp", hs, bpp, comp, h < 0)
    if e in ("jpg", "jpeg") and d[:2] == b"\xff\xd8":
        return ("jpeg",)
    return None


def pick_real(indra, fixtures):
    """The smallest file of each variant class under newview/skins and
    newview/character, copied as real_<n>.<ext>."""
    best = {}
    for root in ("newview/skins", "newview/character"):
        for dp, dn, fn in os.walk(os.path.join(indra, root)):
            for f in fn:
                p = os.path.join(dp, f)
                try:
                    cls = classify(p)
                except (OSError, struct.error):
                    cls = None
                if cls is None:
                    continue
                size = os.path.getsize(p)
                if size > 48 * 1024:
                    continue  # keep the fixture set small
                if cls not in best or size < best[cls][0]:
                    best[cls] = (size, p)
    picked = []
    for i, (cls, (size, p)) in enumerate(sorted(best.items(), key=lambda kv: repr(kv[0]))):
        ext = p.lower().rsplit(".", 1)[-1].replace("jpeg", "jpg")
        name = "real_%02d_%s.%s" % (i, cls[0], ext)
        shutil.copyfile(p, os.path.join(fixtures, name))
        picked.append((name, os.path.relpath(p, indra).replace("\\", "/"), cls))
    # A few more monochrome RLE masks for the gradient processing.
    masks = []
    for dp, dn, fn in os.walk(os.path.join(indra, "newview/character")):
        for f in fn:
            p = os.path.join(dp, f)
            if f.lower().endswith(".tga") and classify(p) and classify(p)[1] == 11:
                masks.append((os.path.getsize(p), p))
    for j, (size, p) in enumerate(sorted(masks)[:3]):
        name = "real_mask_%d.tga" % j
        shutil.copyfile(p, os.path.join(fixtures, name))
        picked.append((name, os.path.relpath(p, indra).replace("\\", "/"), ("tga-mask",)))
    return picked


def main():
    if len(sys.argv) != 4:
        print(__doc__)
        return 2
    oracle, indra, fixtures = sys.argv[1:]
    os.makedirs(fixtures, exist_ok=True)
    for f in os.listdir(fixtures):
        os.remove(os.path.join(fixtures, f))
    subprocess.check_call([sys.executable, "-I", os.path.join(HERE, "gen_sources.py"), fixtures])
    picked = pick_real(indra, fixtures)

    manifest = []
    counts = collections.Counter()
    files = sorted(os.listdir(fixtures))
    raws = [f for f in files if f.endswith(".raw")]
    # Encoders first: their outputs join the decode set.
    for raw in raws:
        src = os.path.join(fixtures, raw)
        for ext, codec in (("png", "png"), ("tga", "tga"), ("bmp", "bmp"), ("dxt", "dxt")):
            out = raw + "." + ext
            ok, err = run(oracle, codec + "-encode", src, os.path.join(fixtures, out))
            if ok:
                manifest.append("encode %s %s %s" % (raw, codec, out))
                counts["encode-" + codec] += 1
            else:
                open(os.path.join(fixtures, out + ".err"), "w").write(err)
                manifest.append("encode-error %s %s %s.err" % (raw, codec, out))
                counts["encode-error-" + codec] += 1
        for q in (50, 75, 95):
            out = "%s.q%d.jpg" % (raw, q)
            ok, err = run(oracle, "jpeg-encode", src, os.path.join(fixtures, out), str(q))
            if ok:
                manifest.append("encode %s jpeg %s %d" % (raw, out, q))
                counts["encode-jpeg"] += 1
            else:
                open(os.path.join(fixtures, out + ".err"), "w").write(err)
                manifest.append("encode-error %s jpeg %s.err %d" % (raw, out, q))
                counts["encode-error-jpeg"] += 1
    # Hand-made DXT variants from the oracle's encodings.
    dds = os.path.join(fixtures, "raw_64x64_c3.raw.dxt")
    if os.path.exists(dds):
        d = open(dds, "rb").read()
        open(os.path.join(fixtures, "dxt_truncated.dxt"), "wb").write(d[:-5000])  # loses the top mip: discard 1
        open(os.path.join(fixtures, "dxt_truncated_all.dxt"), "wb").write(d[:130])  # not even the 1x1: discard -1
        old = struct.pack("<iiii", 3, 6, 64, 64) + d[128:]  # old 16-byte header, RGB8, max level 6
        open(os.path.join(fixtures, "dxt_oldheader.dxt"), "wb").write(old)
        bad = bytearray(d)
        bad[84:88] = struct.pack("<i", 0x31545844)  # DXT1 four-cc
        open(os.path.join(fixtures, "dxt_err_dxt1.dxt"), "wb").write(bad)
        bad[84:88] = b"ABCD"
        open(os.path.join(fixtures, "dxt_err_unknown.dxt"), "wb").write(bad)
        open(os.path.join(fixtures, "dxt_err_short.dxt"), "wb").write(d[:40])

    files = sorted(os.listdir(fixtures))
    for f in files:
        ext = f.lower().rsplit(".", 1)[-1]
        codec = {"png": "png", "tga": "tga", "bmp": "bmp", "jpg": "jpeg", "dxt": "dxt"}.get(ext)
        if codec is None:
            continue
        src = os.path.join(fixtures, f)
        ok, err = run(oracle, codec + "-decode", src, src + ".raw")
        if ok:
            manifest.append("decode %s %s %s.raw" % (f, codec, f))
            counts["decode-" + codec] += 1
        else:
            if os.path.exists(src + ".raw"):
                os.remove(src + ".raw")
            open(src + ".err", "w").write(err)
            manifest.append("decode-error %s %s %s.err" % (f, codec, f))
            counts["decode-error-" + codec] += 1
        if codec == "jpeg":
            ok, err = run(oracle, "jpeg-dims", src)
            manifest.append("jpeg-dims %s %s" % (f, ok.replace(" ", "x") if ok else "error"))
        if codec == "tga" and ("_8_rle" in f or "mask" in f or "real" in f):
            pairs = ((0.0, 0.5), (0.1, 0.3), (1.0, 0.0), (0.05, 0.98), (0.3, 1.2), (0.0, 0.0))
            if "real" in f:
                pairs = pairs[:2]  # the real masks are larger: two settings each
            for domain, weight in pairs:
                out = "%s.proc_%s_%s.raw" % (f, domain, weight)
                ok, err = run(oracle, "tga-process", src, os.path.join(fixtures, out), str(domain), str(weight))
                if ok:
                    manifest.append("process %s %s %s %s" % (f, domain, weight, out))
                    counts["process"] += 1
                else:
                    open(os.path.join(fixtures, out + ".err"), "w").write(err)
                    manifest.append("process-error %s %s %s %s.err" % (f, domain, weight, out))
                    counts["process-error"] += 1

    # The real PNG textures' decoded rasters re-encoded by libpng: the encode
    # gate (decoded-identical, size within 20%, no corpus growth) over real
    # content, not only the synthetic rasters.
    for f in sorted(os.listdir(fixtures)):
        if f.startswith("real_") and f.endswith(".png.raw"):
            out = f + ".png"
            ok, err = run(oracle, "png-encode", os.path.join(fixtures, f), os.path.join(fixtures, out))
            if ok:
                manifest.append("encode %s png %s" % (f, out))
                counts["encode-png-real"] += 1

    with open(os.path.join(fixtures, "manifest.txt"), "w") as m:
        m.write("# generated by tools/image-eval/scripts/make_fixtures.py with the C++ oracle; do not edit\n")
        for name, rel, cls in picked:
            m.write("# %s <- %s %r\n" % (name, rel, cls))
        m.write("\n".join(manifest) + "\n")
    total = sum(os.path.getsize(os.path.join(fixtures, f)) for f in os.listdir(fixtures))
    print("fixtures: %d files, %.1f KB" % (len(os.listdir(fixtures)), total / 1024))
    for k, v in sorted(counts.items()):
        print("  %-22s %d" % (k, v))
    return 0


if __name__ == "__main__":
    sys.exit(main())
