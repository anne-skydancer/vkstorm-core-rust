"""Regenerates set 1's random task inventory files from the generator specified
in crates/vkstorm-inventory/tests/fixtures/task/README.md ("The sweep
generators"), independently of the C++, and compares the sampled ones with the
committed inputs (random/random_NNNN.inv).

usage: python tools/inventory-fixtures/task/check_set1_inputs.py [fixture directory]

Exits non-zero on any difference. (Derived from the V0 audit's re-implementation.)
"""
import os
import sys

M64 = (1 << 64) - 1
SEED = 0x52344154
COUNT = 3000
SAMPLE_EVERY = 97
TOKENS = [b"\tinv_item\t0\n", b"\tinv_object\t0\n", b"\t{\n", b"\t}\n", b"\n", b" \n", b"\tjunk\t1\n",
          b"\t\tname\tX|\n", b"\t\ttype\ttexture\n", b"\tpermissions 0\n", b"\tsale_info\t0\n",
          b"\t\tflags\t-1\n", b"\t\tsale_price\tx\n"]
PROTECTED = {b"creation_date", b"base_mask", b"owner_mask", b"group_mask", b"everyone_mask", b"next_owner_mask",
             b"creator_mask", b"creator_id", b"owner_id", b"last_owner_id", b"group_id", b"group_owned"}


class Rng:
    def __init__(self, seed):
        self.s = seed or 0x9E3779B97F4A7C15

    def next(self):
        s = self.s
        s ^= s >> 12
        s ^= (s << 25) & M64
        s ^= s >> 27
        self.s = s
        return (s * 0x2545F4914F6CDD1D) & M64

    def below(self, n):
        return self.next() % n if n else 0


def protected(line):
    words = line.split()  # C isspace on ASCII
    return bool(words) and words[0][:63] in PROTECTED


def mutate(rng, base):
    lines = []
    p = 0
    while p < len(base):
        e = base.find(b"\n", p)
        if e < 0:
            lines.append(base[p:])
            break
        lines.append(base[p:e + 1])
        p = e + 1
    ops = 1 + rng.below(4)
    k = 0
    while k < ops and lines:
        at = rng.below(len(lines))
        op = rng.below(7)
        if op >= 4 and protected(lines[at]):
            op = 1
        if op == 0:
            del lines[at]
        elif op == 1:
            lines.insert(at, lines[at])
        elif op == 2:
            lines.insert(at, TOKENS[rng.below(len(TOKENS))])
        elif op == 3:
            j = rng.below(len(lines))
            lines[at], lines[j] = lines[j], lines[at]
        elif op == 4:
            line = bytearray(lines[at])
            if line:
                value = 0x20 + rng.below(0x5f)
                pos = rng.below(len(line))
                line[pos] = value
            lines[at] = bytes(line)
        elif op == 5:
            line = lines[at]
            lines[at] = line[:rng.below(len(line) + 1)]
        else:
            line = lines[at]
            count = rng.below(300)
            pos = rng.below(len(line) + 1)
            lines[at] = line[:pos] + b"m" * count + line[pos:]
        k += 1
    return b"".join(lines)


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    fixtures = sys.argv[1] if len(sys.argv) > 1 else os.path.normpath(
        os.path.join(here, "..", "..", "..", "crates", "vkstorm-inventory", "tests", "fixtures", "task"))
    base = open(os.path.join(fixtures, "files", "normal.inv"), "rb").read()
    rng = Rng(SEED)
    ok = bad = 0
    for i in range(COUNT):
        data = mutate(rng, base)
        if i % SAMPLE_EVERY:
            continue
        want = open(os.path.join(fixtures, "random", "random_%04d.inv" % i), "rb").read()
        if want == data:
            ok += 1
        else:
            bad += 1
            print("differs: random_%04d.inv" % i)
    print("set 1 sampled inputs: %d identical, %d differ" % (ok, bad))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
