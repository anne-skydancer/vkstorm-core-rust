"""Tile a small P6 PPM into an n x n PPM (texture-like detail from a real photo).
usage: python -I mosaic.py <src.ppm> <dst.ppm> <n>"""
import sys
src, dst, n = sys.argv[1], sys.argv[2], int(sys.argv[3])
data = open(src, 'rb').read()
pos = 0; toks = []
while len(toks) < 4:
    if data[pos:pos+1] == b'#':
        pos = data.index(b'\n', pos) + 1; continue
    if data[pos:pos+1].isspace():
        pos += 1; continue
    e = pos
    while not data[e:e+1].isspace(): e += 1
    toks.append(data[pos:e]); pos = e
pos += 1
w, h = int(toks[1]), int(toks[2]); px = data[pos:]
out = bytearray(n * n * 3)
for y in range(n):
    row = px[(y % h) * w * 3:((y % h) + 1) * w * 3]
    for x0 in range(0, n, w):
        seg = row[:min(w, n - x0) * 3]
        out[(y * n + x0) * 3:(y * n + x0) * 3 + len(seg)] = seg
open(dst, 'wb').write(b'P6\n%d %d\n255\n' % (n, n) + bytes(out))
