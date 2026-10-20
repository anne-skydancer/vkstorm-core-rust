#!/usr/bin/env python3
"""Like-for-like timing of the viewer's C++ codecs (c/image_oracle.exe, MSVC
/O2 /arch:AVX2 on the viewer's prebuilt libpng + zlib-ng and libjpeg-turbo)
against vkstorm-image (target/release/image-eval.exe, cargo release profile,
target-cpu=x86-64-v3), single thread, same inputs, best and mean of N runs.

usage: run_bench.py <oracle.exe> <image-eval.exe> <src_dir> <results.csv> [iters]

<src_dir> holds the gen_sources.py --big rasters (big_64_c*.raw,
big_2048_c*.raw). Encoded inputs for the decode rows are produced here by
the C++ encoders (the same bytes are decoded by both sides).
"""
import os
import subprocess
import sys


def run(exe, *args):
    p = subprocess.run([exe, *[str(a) for a in args]], capture_output=True, text=True)
    out = (p.stdout or "").strip()
    if p.returncode != 0 or out.startswith("FAIL"):
        return None
    return out


def main():
    if len(sys.argv) < 5:
        print(__doc__)
        return 2
    oracle, rs, src, csv = sys.argv[1:5]
    iters = int(sys.argv[5]) if len(sys.argv) > 5 else 20
    rows = []
    # (label, raw file, codecs that take this component count)
    inputs = [
        ("64x64 RGB", "big_64_c3.raw", ["png", "jpeg", "tga", "bmp", "dxt"]),
        ("64x64 RGBA", "big_64_c4.raw", ["png", "tga", "bmp", "dxt"]),
        ("64x64 gray", "big_64_c1.raw", ["png", "jpeg", "tga", "bmp", "dxt"]),
        ("2048x2048 RGB", "big_2048_c3.raw", ["png", "jpeg", "tga", "bmp", "dxt"]),
        ("2048x2048 RGBA", "big_2048_c4.raw", ["png", "tga", "bmp", "dxt"]),
        ("2048x2048 gray", "big_2048_c1.raw", ["png", "jpeg", "tga", "bmp", "dxt"]),
    ]
    ext = {"png": "png", "jpeg": "jpg", "tga": "tga", "bmp": "bmp", "dxt": "dxt"}
    with open(csv, "w") as f:
        f.write("image,codec,op,c_best_ms,c_mean_ms,rust_best_ms,rust_mean_ms,rust_over_c_best,c_bytes,rust_bytes\n")
        for label, raw, codecs in inputs:
            rawp = os.path.join(src, raw)
            if not os.path.exists(rawp):
                continue
            n = iters if "64x64" in label else max(5, iters // 4)
            if "64x64" in label:
                n = iters * 10
            for codec in codecs:
                enc = os.path.join(src, "%s.%s" % (raw, ext[codec]))
                if not run(oracle, "%s-encode" % codec, rawp, enc, 75):
                    print("C++ encode failed", label, codec)
                    continue
                for op in ("decode", "encode", "header"):
                    target = rawp if op == "encode" else enc
                    c = run(oracle, "bench", codec, op, target, n, 75)
                    r = run(rs, "bench", codec, op, target, n, 75)
                    if not c or not r:
                        print("bench failed", label, codec, op, c, r)
                        continue
                    cb, cm, cs = c.split()
                    rb, rm, rsz = r.split()
                    ratio = float(rb) / float(cb) if float(cb) > 0 else 0
                    row = (label, codec, op, cb, cm, rb, rm, "%.3f" % ratio, cs, rsz)
                    rows.append(row)
                    f.write(",".join(row) + "\n")
                    print("%-16s %-5s %-7s C++ %9s ms  Rust %9s ms  Rust/C++ %s" % (label, codec, op, cb, rb, "%.2f" % ratio))
    return 0


if __name__ == "__main__":
    sys.exit(main())
