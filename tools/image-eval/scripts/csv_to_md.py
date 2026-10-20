#!/usr/bin/env python3
"""Render a run_bench.py CSV as the markdown table used in the viewer's
doc/rust_port.md: decode and encode rows, best-of-N milliseconds, Rust/C++.

usage: csv_to_md.py <results.csv>
"""
import csv
import sys


def main():
    rows = list(csv.DictReader(open(sys.argv[1], newline="")))
    print("| Image | Codec | Op | C++ ms | Rust ms | Rust/C++ |")
    print("|---|---|---|---:|---:|---:|")
    for r in rows:
        if r["op"] == "header":
            continue
        print("| %s | %s | %s | %.4f | %.4f | %.2f |" % (
            r["image"], r["codec"].upper(), r["op"], float(r["c_best_ms"]), float(r["rust_best_ms"]), float(r["rust_over_c_best"])))
    return 0


if __name__ == "__main__":
    sys.exit(main())
