import math, random, struct, sys, os
random.seed(1138)
def write_ppm(path, w, h, px):
    with open(path, 'wb') as f:
        f.write(b'P6\n%d %d\n255\n' % (w, h)); f.write(bytes(px))
def photo_like(w, h):
    # smooth low-frequency field + mid-frequency sine detail + sharp edges + grain
    out = bytearray(w*h*3)
    cx, cy = random.random()*w, random.random()*h
    for y in range(h):
        fy = y / h
        for x in range(w):
            fx = x / w
            base = 0.5 + 0.35*math.sin(6.28*(fx*1.3+fy*0.7)) * math.cos(6.28*fy*0.9)
            detail = 0.12*math.sin(fx*w*0.11) * math.sin(fy*h*0.07) + 0.08*math.sin((fx*w+fy*h)*0.23)
            edge = 0.25 if ((int(fx*8)+int(fy*8)) % 2 == 0 and (x+y) % 97 < 40) else 0.0
            d = math.hypot(x-cx, y-cy) / max(w, h)
            vign = 1.0 - 0.6*d*d
            g = random.gauss(0, 0.02)
            r = (base + detail + edge + g) * vign
            gch = (base*0.8 + detail*1.2 - edge*0.5 + g) * vign
            b = (0.9 - base*0.6 + detail + g) * vign
            i = (y*w+x)*3
            out[i]   = max(0, min(255, int(r*255)))
            out[i+1] = max(0, min(255, int(gch*255)))
            out[i+2] = max(0, min(255, int(b*255)))
    return out
dst = sys.argv[1]
for n in (256, 512, 1024, 2048):
    write_ppm(os.path.join(dst, f'synth_{n}.ppm'), n, n, photo_like(n, n))
    print('wrote', n)
