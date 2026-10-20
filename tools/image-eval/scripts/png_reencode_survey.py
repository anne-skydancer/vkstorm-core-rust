#!/usr/bin/env python3
"""Re-encode real PNG textures with the viewer's libpng + zlib-ng (oracle) and
with vkstorm-image, and report how often the files are byte-identical and the
size ratio otherwise. Decoded rasters are compared with the oracle decoder.

usage: png_reencode_survey.py <oracle.exe> <image-eval.exe> <dir> [max_files]
"""
import os
import subprocess
import sys
import tempfile


def run(exe, *args):
    p = subprocess.run([exe, *args], capture_output=True, text=True)
    return p.returncode == 0 and not (p.stdout or "").startswith("FAIL")


def main():
    oracle, rs, root = sys.argv[1:4]
    limit = int(sys.argv[4]) if len(sys.argv) > 4 else 400
    files = []
    for dp, dn, fn in os.walk(root):
        for f in fn:
            if f.lower().endswith(".png"):
                files.append(os.path.join(dp, f))
    files.sort(key=lambda p: (os.path.getsize(p), p))
    step = max(1, len(files) // limit)
    files = files[::step][:limit]
    tmp = tempfile.mkdtemp(prefix="pngsurvey")
    identical = 0
    differing = []
    bad = 0
    for i, f in enumerate(files):
        raw = os.path.join(tmp, "s.raw")
        a = os.path.join(tmp, "a.png")
        b = os.path.join(tmp, "b.png")
        if not run(oracle, "png-decode", f, raw):
            continue
        if not (run(oracle, "png-encode", raw, a) and run(rs, "png-encode", raw, b)):
            bad += 1
            continue
        da, db = open(a, "rb").read(), open(b, "rb").read()
        if da == db:
            identical += 1
            continue
        ra, rb = os.path.join(tmp, "a.raw"), os.path.join(tmp, "b.raw")
        same = run(oracle, "png-decode", a, ra) and run(oracle, "png-decode", b, rb) and open(ra, "rb").read() == open(rb, "rb").read()
        if not same:
            bad += 1
        differing.append((len(db) / len(da), len(da), len(db), os.path.relpath(f, root)))
    n = identical + len(differing)
    print("files: %d  byte-identical: %d (%.1f%%)  differing: %d  decode mismatches or failures: %d" % (n, identical, 100.0 * identical / max(1, n), len(differing), bad))
    if differing:
        ratios = sorted(r for r, _, _, _ in differing)
        total_a = sum(x for _, x, _, _ in differing)
        total_b = sum(x for _, _, x, _ in differing)
        print("differing size ratio rust/libpng: min %.4f median %.4f max %.4f; total bytes %d vs %d (%+.2f%%)" % (
            ratios[0], ratios[len(ratios) // 2], ratios[-1], total_b, total_a, (total_b / total_a - 1) * 100))
        for r, la, lb, name in sorted(differing, reverse=True)[:8]:
            print("  %.4f %7d %7d %s" % (r, la, lb, name))
    return 0


if __name__ == "__main__":
    sys.exit(main())
