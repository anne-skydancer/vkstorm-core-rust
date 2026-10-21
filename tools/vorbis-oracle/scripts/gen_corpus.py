"""Builds the Ogg Vorbis decoder test corpus for the vkstorm-vorbis oracle.

usage: python -I tools/vorbis-oracle/scripts/gen_corpus.py <vorbis_corpus executable> <outdir> [--quick]
       python -I tools/vorbis-oracle/scripts/gen_corpus.py --inspect <file.ogg>...

The clean streams are encoded by tools/vorbis-oracle/c/vorbis_corpus.c (linked
against the viewer's libvorbisenc 1.3.7 / libogg 1.3.5); every damaged or
unusual stream is derived from them here by deterministic byte manipulation
(own splitmix64 generator, no `random`, no clock). Running the script twice with
the same encoder binary produces byte-identical output. Standard library only.

Layout of <outdir>:
  clean/<group>/...   valid streams as libvorbisenc writes them (and valid chains)
  odd/<group>/...     valid but unusual streams (re-paged layouts, multiplexed
                      streams, zero-length packets, start-trimmed granules)
  synthetic/<group>/… streams hand-built in Python to reach setup shapes libvorbisenc
                      never emits (floor 0, residue 0, many submaps/modes, odd block
                      sizes, codebook edge cases) and ~25 deliberately broken setups
  damaged/<group>/... truncated, corrupted, reordered and malformed inputs
  manifest.txt        one line per file: <relative path> <size> <description>

--quick writes a subset of about fifty files for fast iteration.
--inspect prints the page and header structure of existing files, including the
floor types and codebook count of each Vorbis setup header it finds.
"""
import os
import struct
import subprocess
import sys

MASK64 = (1 << 64) - 1


# --------------------------------------------------------------------------- PRNG

class Rng:
    """splitmix64; the same sequence as vorbis_corpus.c, independent of Python's `random`."""

    def __init__(self, seed):
        self.s = seed & MASK64

    def next(self):
        self.s = (self.s + 0x9E3779B97F4A7C15) & MASK64
        z = self.s
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & MASK64
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & MASK64
        return z ^ (z >> 31)

    def below(self, n):
        return self.next() % n

    def bytes(self, n):
        out = bytearray()
        while len(out) < n:
            out += self.next().to_bytes(8, "little")
        return bytes(out[:n])


# --------------------------------------------------------------------------- Ogg pages

def _crc_table():
    table = []
    for i in range(256):
        r = i << 24
        for _ in range(8):
            r = ((r << 1) ^ 0x04C11DB7) if r & 0x80000000 else (r << 1)
        table.append(r & 0xFFFFFFFF)
    return table


CRC_TABLE = _crc_table()


def ogg_crc(data):
    c = 0
    for b in data:
        c = ((c << 8) & 0xFFFFFFFF) ^ CRC_TABLE[((c >> 24) & 0xFF) ^ b]
    return c


class Page:
    __slots__ = ("off", "version", "htype", "granule", "serial", "seq", "crc", "lacing", "hlen", "blen")

    @property
    def end(self):
        return self.off + self.hlen + self.blen

    @property
    def body_off(self):
        return self.off + self.hlen


def parse_pages(data, strict=True):
    """Splits a byte string into Ogg pages. strict: the data must be exactly a page
    sequence (true for every encoder output); otherwise resynchronise on 'OggS'."""
    pages = []
    o = 0
    n = len(data)
    while o + 27 <= n:
        if data[o:o + 4] != b"OggS":
            if strict:
                raise ValueError("no capture pattern at %d" % o)
            o += 1
            continue
        nseg = data[o + 26]
        hlen = 27 + nseg
        if o + hlen > n:
            break
        lacing = list(data[o + 27:o + hlen])
        blen = sum(lacing)
        if o + hlen + blen > n:
            break
        p = Page()
        p.off, p.version, p.htype = o, data[o + 4], data[o + 5]
        p.granule, p.serial, p.seq, p.crc = struct.unpack_from("<qIII", data, o + 6)
        p.lacing, p.hlen, p.blen = lacing, hlen, blen
        pages.append(p)
        o += hlen + blen
    if strict and o != n:
        raise ValueError("trailing bytes after last page")
    return pages


def page_crc_ok(data, p):
    raw = bytearray(data[p.off:p.end])
    raw[22:26] = b"\0\0\0\0"
    return ogg_crc(raw) == p.crc


def packets_of(data, pages, serial=None):
    """Reassembles packets; each is a dict with data, spans (file offset, length),
    first_page and last_page (indices into pages)."""
    pkts = []
    cur = None
    for pi, p in enumerate(pages):
        if serial is not None and p.serial != serial:
            continue
        if not (p.htype & 1):
            cur = None
        pos = p.body_off
        for lv in p.lacing:
            if cur is None:
                cur = {"spans": [], "first_page": pi}
            if lv:
                cur["spans"].append((pos, lv))
            pos += lv
            if lv < 255:
                cur["last_page"] = pi
                cur["data"] = b"".join(data[a:a + k] for a, k in cur["spans"])
                pkts.append(cur)
                cur = None
    return pkts


def pkt_file_offset(pkt, k):
    for a, n in pkt["spans"]:
        if k < n:
            return a + k
        k -= n
    raise IndexError("offset beyond packet")


def make_page(htype, granule, serial, seq, lacing, body):
    raw = bytearray(b"OggS" + bytes([0, htype]) + struct.pack("<qIII", granule, serial, seq, 0)
                    + bytes([len(lacing)]) + bytes(lacing) + body)
    struct.pack_into("<I", raw, 22, ogg_crc(raw))
    return bytes(raw)


def refresh_crc(buf, pages, offsets):
    """Recomputes the CRC of every page containing one of the file offsets (in place)."""
    done = set()
    for o in offsets:
        for i, p in enumerate(pages):
            if p.off <= o < p.end and i not in done:
                done.add(i)
                raw = bytearray(buf[p.off:p.end])
                raw[22:26] = b"\0\0\0\0"
                struct.pack_into("<I", buf, p.off + 22, ogg_crc(raw))


def page_bytes(data, pages):
    return [data[p.off:p.end] for p in pages]


def repage(chunks, renumber_from=None, fix=True, serial=None):
    """Concatenates page byte strings, optionally renumbering sequence numbers / serials."""
    out = bytearray()
    for i, c in enumerate(chunks):
        c = bytearray(c)
        if renumber_from is not None:
            struct.pack_into("<I", c, 18, renumber_from + i)
        if serial is not None:
            struct.pack_into("<I", c, 14, serial)
        if fix and (renumber_from is not None or serial is not None):
            c[22:26] = b"\0\0\0\0"
            struct.pack_into("<I", c, 22, ogg_crc(c))
        out += c
    return bytes(out)


def set_page_field(data, pages, idx, fmt, at, value, fix=True):
    buf = bytearray(data)
    struct.pack_into(fmt, buf, pages[idx].off + at, value)
    if fix:
        refresh_crc(buf, pages, [pages[idx].off])
    return bytes(buf)


def mux(packets, granules, serial, layout="std", headers=3, eos=True, seq0=0):
    """Pages a packet list. The first packet sits alone on the first page (BOS);
    the remaining header packets are flushed together (as libogg's ogg_stream_flush
    does); audio pages follow the layout:
      std   close a page once >= 4096 body bytes end on a packet boundary (libogg-like)
      seg1  one lacing segment per page (maximal continuation)
      pkt1  every packet on its own page(s)
      max   always 255 segments per page
    granules[i] is the granule position after packet i (-1 = unknown)."""
    segs = []  # (packet index, lacing value, bytes)
    for i, pk in enumerate(packets):
        n = len(pk)
        o = 0
        while n - o >= 255:
            segs.append((i, 255, pk[o:o + 255]))
            o += 255
        segs.append((i, n - o, pk[o:]))
    groups = []
    cur = []
    cur_bytes = 0
    for s in segs:
        pi, lv, _ = s
        cur.append(s)
        cur_bytes += lv
        ends = lv < 255
        close = len(cur) == 255
        if pi < headers:
            close = close or (ends and (pi == 0 or pi == headers - 1))
        elif layout == "std":
            close = close or (ends and cur_bytes >= 4096)
        elif layout == "seg1":
            close = True
        elif layout == "pkt1":
            close = close or ends
        elif layout != "max":
            raise ValueError(layout)
        if close:
            groups.append(cur)
            cur, cur_bytes = [], 0
    if cur:
        groups.append(cur)
    out = bytearray()
    cont = False
    for gi, g in enumerate(groups):
        htype = (1 if cont else 0) | (2 if gi == 0 else 0) | (4 if (eos and gi == len(groups) - 1) else 0)
        done = [s[0] for s in g if s[1] < 255]
        if not done:
            gran = -1
        elif done[-1] < headers:
            gran = 0
        else:
            gran = granules[done[-1]]
        out += make_page(htype, gran, serial, seq0 + gi, [s[1] for s in g], b"".join(s[2] for s in g))
        cont = g[-1][1] == 255
    return bytes(out)


# --------------------------------------------------------------------------- Vorbis headers

def ilog(v):
    n = 0
    while v > 0:
        n += 1
        v >>= 1
    return n


class Bits:
    """LSb-first bit reader over a packet (Vorbis bit packing)."""

    def __init__(self, data):
        self.d = data
        self.p = 0

    def read(self, n):
        if self.p + n > len(self.d) * 8:
            raise EOFError("setup header ends at bit %d" % self.p)
        v = 0
        for i in range(n):
            b = self.p + i
            v |= ((self.d[b >> 3] >> (b & 7)) & 1) << i
        self.p += n
        return v

    def skip(self, n):
        if self.p + n > len(self.d) * 8:
            raise EOFError("setup header ends at bit %d" % self.p)
        self.p += n


def set_bits(buf, bitpos, nbits, value):
    for i in range(nbits):
        b = bitpos + i
        if (value >> i) & 1:
            buf[b >> 3] |= 1 << (b & 7)
        else:
            buf[b >> 3] &= ~(1 << (b & 7)) & 0xFF


def lookup1_values(entries, dims):
    r = int(round(entries ** (1.0 / dims))) if dims else 0
    while r > 0 and r ** dims > entries:
        r -= 1
    while (r + 1) ** dims <= entries:
        r += 1
    return r


# --------------------------------------------------------------------------- synthetic streams

class BitWriter:
    """LSb-first bit packer matching libvorbis oggpack_write: the low bit of a value
    goes out first, and bytes fill from their low bit upward."""

    def __init__(self):
        self.buf = bytearray()
        self.bit = 0  # bits already used in the last byte

    def write(self, value, nbits):
        if nbits < 0:
            raise ValueError("nbits<0")
        value &= (1 << nbits) - 1
        while nbits > 0:
            if self.bit == 0:
                self.buf.append(0)
            take = min(8 - self.bit, nbits)
            self.buf[-1] |= (value & ((1 << take) - 1)) << self.bit
            value >>= take
            self.bit = (self.bit + take) & 7
            nbits -= take

    def data(self):
        return bytes(self.buf)


def full_tree_lengths(entries):
    """A fully populated Huffman length list for `entries` leaves (Kraft sum == 1),
    built by splitting the shallowest leaf so codeword lengths stay near log2."""
    if entries == 1:
        return [1]  # single-entry book: libvorbis special-cases the length-1 '0' leaf
    import heapq
    depths = [0]
    for _ in range(entries - 1):
        d = heapq.heappop(depths)
        heapq.heappush(depths, d + 1)
        heapq.heappush(depths, d + 1)
    return sorted(depths)


def ladder_lengths(maxlen):
    """Lengths [1,2,...,maxlen-1,maxlen,maxlen]: a full tree whose longest codeword is
    exactly `maxlen` (used to force 31- and 32-bit codewords)."""
    return list(range(1, maxlen)) + [maxlen, maxlen]


def vorbis_float32_pack(val):
    """libvorbis _float32_pack (os.h/sharedbook.c): the non-IEEE 10-exponent/21-mantissa
    float used for codebook q_min/q_delta. Only needs to decode to something finite."""
    import math
    if val == 0:
        return 0
    sign = 0
    if val < 0:
        sign = 0x80000000
        val = -val
    exp = int(math.floor(math.log(val) / math.log(2.0) + 0.001))
    mant = int(round(math.ldexp(val, (21 - 1) - exp)))
    return sign | ((exp + 768) << 21) | (mant & 0x1FFFFF)


def blocksize_log2(bs):
    n = 0
    while bs > 1:
        bs >>= 1
        n += 1
    return n


class Codebook:
    """A static codebook as libvorbis packs it (codebook.c vorbis_staticbook_unpack)."""

    def __init__(self, dim, lengthlist, maptype=0, sparse=None, ordered=False,
                 q_min=None, q_delta=None, q_quant=8, q_sequencep=0, quant=None):
        self.dim = dim
        self.lengthlist = list(lengthlist)
        self.entries = len(self.lengthlist)
        self.maptype = maptype
        self.sparse = sparse
        self.ordered = ordered
        self.q_min = vorbis_float32_pack(1.0) if q_min is None else q_min
        self.q_delta = vorbis_float32_pack(0.5) if q_delta is None else q_delta
        self.q_quant = q_quant
        self.q_sequencep = q_sequencep
        self.quant = quant
        if ilog(dim) + ilog(self.entries) > 24:
            raise ValueError("ilog(dim)+ilog(entries)>24 (dim=%d entries=%d)" % (dim, self.entries))

    def quantvals(self):
        if self.maptype == 1:
            return lookup1_values(self.entries, self.dim) if self.dim else 0
        if self.maptype == 2:
            return self.entries * self.dim
        return 0

    def pack(self, bw):
        bw.write(0x564342, 24)
        bw.write(self.dim, 16)
        bw.write(self.entries, 24)
        if self.ordered:
            ll = self.lengthlist
            if ll != sorted(ll):
                raise ValueError("ordered book needs nondecreasing lengths")
            bw.write(1, 1)
            bw.write(ll[0] - 1, 5)
            i, cur = 0, ll[0]
            while i < self.entries:
                run = 0
                while i + run < self.entries and ll[i + run] == cur:
                    run += 1
                bw.write(run, ilog(self.entries - i))
                i += run
                cur += 1
        else:
            bw.write(0, 1)
            has_unused = any(x == 0 for x in self.lengthlist) if self.sparse is None else self.sparse
            bw.write(1 if has_unused else 0, 1)
            if has_unused:
                for x in self.lengthlist:
                    if x > 0:
                        bw.write(1, 1)
                        bw.write(x - 1, 5)
                    else:
                        bw.write(0, 1)
            else:
                for x in self.lengthlist:
                    bw.write(x - 1, 5)
        bw.write(self.maptype, 4)
        if self.maptype in (1, 2):
            bw.write(self.q_min, 32)
            bw.write(self.q_delta, 32)
            bw.write(self.q_quant - 1, 4)
            bw.write(self.q_sequencep, 1)
            qv = self.quantvals()
            quant = self.quant if self.quant is not None else [(i * 3 + 1) for i in range(qv)]
            if len(quant) < qv:
                quant = quant + [1] * (qv - len(quant))
            mask = (1 << self.q_quant) - 1
            for i in range(qv):
                bw.write(quant[i] & mask, self.q_quant)


def mapbook(dim, entries, maptype=1, **kw):
    """A codebook usable as a floor/residue book: maptype != 0 and dim >= 1."""
    return Codebook(dim, full_tree_lengths(entries), maptype=maptype, **kw)


class Floor0:
    type = 0

    def __init__(self, order, rate, barkmap, ampbits, ampdB, books):
        self.order, self.rate, self.barkmap = order, rate, barkmap
        self.ampbits, self.ampdB, self.books = ampbits, ampdB, list(books)

    def pack(self, bw):
        bw.write(self.order, 8)
        bw.write(self.rate, 16)
        bw.write(self.barkmap, 16)
        bw.write(self.ampbits, 6)
        bw.write(self.ampdB, 8)
        bw.write(len(self.books) - 1, 4)
        for b in self.books:
            bw.write(b, 8)


class Floor1:
    type = 1

    def __init__(self, partitionclass, class_dim, class_subs, class_book, class_subbook,
                 mult, rangebits, posts):
        self.partitionclass = list(partitionclass)
        self.class_dim = list(class_dim)
        self.class_subs = list(class_subs)
        self.class_book = list(class_book)
        self.class_subbook = [list(x) for x in class_subbook]
        self.mult = mult
        self.rangebits = rangebits
        self.posts = list(posts)  # the explicitly coded posts (postlist[2:])

    def pack(self, bw):
        bw.write(len(self.partitionclass), 5)
        for pc in self.partitionclass:
            bw.write(pc, 4)
        maxclass = max(self.partitionclass) if self.partitionclass else -1
        for c in range(maxclass + 1):
            bw.write(self.class_dim[c] - 1, 3)
            bw.write(self.class_subs[c], 2)
            if self.class_subs[c]:
                bw.write(self.class_book[c], 8)
            for k in range(1 << self.class_subs[c]):
                bw.write(self.class_subbook[c][k] + 1, 8)
        bw.write(self.mult - 1, 2)
        bw.write(self.rangebits, 4)
        for p in self.posts:
            bw.write(p, self.rangebits)


class Residue:
    """Residue 0, 1 and 2 all unpack through res0_unpack, so one packer covers them."""

    def __init__(self, rtype, begin, end, grouping, partitions, groupbook, secondstages, booklist):
        self.rtype = rtype
        self.begin, self.end, self.grouping = begin, end, grouping
        self.partitions = partitions
        self.groupbook = groupbook
        self.secondstages = list(secondstages)
        self.booklist = list(booklist)

    def pack(self, bw):
        bw.write(self.begin, 24)
        bw.write(self.end, 24)
        bw.write(self.grouping - 1, 24)
        bw.write(self.partitions - 1, 6)
        bw.write(self.groupbook, 8)
        for cascade in self.secondstages:
            bw.write(cascade & 0x7, 3)
            hi = cascade >> 3
            if hi:
                bw.write(1, 1)
                bw.write(hi, 5)
            else:
                bw.write(0, 1)
        for b in self.booklist:
            bw.write(b, 8)


class Mapping:
    def __init__(self, submaps, coupling, chmux, floor_sub, residue_sub):
        self.submaps = submaps
        self.coupling = list(coupling)  # (magnitude, angle) pairs
        self.chmux = list(chmux)
        self.floor_sub = list(floor_sub)
        self.residue_sub = list(residue_sub)

    def pack(self, bw, channels):
        if self.submaps > 1:
            bw.write(1, 1)
            bw.write(self.submaps - 1, 4)
        else:
            bw.write(0, 1)
        if self.coupling:
            bw.write(1, 1)
            bw.write(len(self.coupling) - 1, 8)
            cb = ilog(channels - 1)
            for mag, ang in self.coupling:
                bw.write(mag, cb)
                bw.write(ang, cb)
        else:
            bw.write(0, 1)
        bw.write(0, 2)  # reserved, must be 0
        if self.submaps > 1:
            for c in range(channels):
                bw.write(self.chmux[c], 4)
        for s in range(self.submaps):
            bw.write(0, 8)  # time submap (unused)
            bw.write(self.floor_sub[s], 8)
            bw.write(self.residue_sub[s], 8)


class Mode:
    def __init__(self, blockflag, mapping):
        self.blockflag = blockflag
        self.mapping = mapping

    def pack(self, bw):
        bw.write(self.blockflag, 1)
        bw.write(0, 16)  # windowtype (VI_WINDOWB == 1, so only 0 is legal)
        bw.write(0, 16)  # transformtype
        bw.write(self.mapping, 8)


class SyntheticStream:
    """Builds a complete Ogg Vorbis stream from scratch: identification, comment and
    hand-packed setup headers, followed by audio packets of deterministic random bytes.
    The decoder walks the floor/residue/codebook paths over whatever bits it finds, so
    random payloads exercise the decode code without us encoding anything."""

    def __init__(self, channels, rate, bs0, bs1, books, floors, residues, mappings, modes,
                 serial, comments=None, vendor="vkstorm-synth"):
        self.channels, self.rate = channels, rate
        self.bs0, self.bs1 = bs0, bs1
        self.books, self.floors, self.residues = books, floors, residues
        self.mappings, self.modes = mappings, modes
        self.serial = serial
        self.comments = comments or []
        self.vendor = vendor

    def id_header(self):
        p = bytearray(b"\x01vorbis")
        p += struct.pack("<IBI", 0, self.channels & 0xFF, self.rate)
        p += struct.pack("<iii", 0, 0, 0)
        p += bytes([blocksize_log2(self.bs0) | (blocksize_log2(self.bs1) << 4)])
        p += bytes([1])  # framing
        return bytes(p)

    def comment_header(self):
        v = self.vendor.encode("utf-8")
        p = bytearray(b"\x03vorbis")
        p += struct.pack("<I", len(v)) + v
        p += struct.pack("<I", len(self.comments))
        for c in self.comments:
            cb = c.encode("utf-8")
            p += struct.pack("<I", len(cb)) + cb
        p += bytes([1])
        return bytes(p)

    def setup_header(self):
        bw = BitWriter()
        for b in b"\x05vorbis":
            bw.write(b, 8)
        bw.write(len(self.books) - 1, 8)
        for bk in self.books:
            bk.pack(bw)
        bw.write(0, 6)   # one time backend
        bw.write(0, 16)  # time type 0
        bw.write(len(self.floors) - 1, 6)
        for fl in self.floors:
            bw.write(fl.type, 16)
            fl.pack(bw)
        bw.write(len(self.residues) - 1, 6)
        for rs in self.residues:
            bw.write(rs.rtype, 16)
            rs.pack(bw)
        bw.write(len(self.mappings) - 1, 6)
        for mp in self.mappings:
            bw.write(0, 16)  # mapping type 0
            mp.pack(bw, self.channels)
        bw.write(len(self.modes) - 1, 6)
        for md in self.modes:
            md.pack(bw)
        bw.write(1, 1)  # framing
        return bw.data()

    def modebits(self):
        return ilog(len(self.modes) - 1)

    def audio_packet(self, rng, mode_idx, length):
        bw = BitWriter()
        bw.write(0, 1)  # audio packet marker
        bw.write(mode_idx, self.modebits())
        if self.modes[mode_idx].blockflag:
            bw.write(rng.below(2), 1)  # lW
            bw.write(rng.below(2), 1)  # nW
        data = bytearray(bw.data())
        while len(data) < length:
            data.append(rng.below(256))
        return bytes(data[:max(1, length)])

    def blocksize(self, mode_idx):
        return self.bs1 if self.modes[mode_idx].blockflag else self.bs0

    def build(self, rng, npackets, lengths=None, eos=True, headers=None):
        packets = [self.id_header(), self.comment_header(), self.setup_header()]
        if headers is not None:
            packets = list(headers)
        granules = [0, 0, 0]
        prev_bs = None
        g = 0
        for i in range(npackets):
            mode_idx = rng.below(len(self.modes))
            ln = lengths[i] if lengths else (2 + rng.below(300))
            packets.append(self.audio_packet(rng, mode_idx, ln))
            bs = self.blocksize(mode_idx)
            if prev_bs is not None:
                g += prev_bs // 4 + bs // 4
            prev_bs = bs
            granules.append(g)
        return mux(packets, granules, self.serial, "std", headers=3, eos=eos)


def parse_id(pk):
    if len(pk) < 30 or pk[0] != 1 or pk[1:7] != b"vorbis":
        raise ValueError("not a Vorbis identification header")
    version, channels, rate = struct.unpack_from("<IBI", pk, 7)
    bs = pk[28]
    return {"version": version, "channels": channels, "rate": rate,
            "bs0": 1 << (bs & 15), "bs1": 1 << (bs >> 4)}


def parse_setup(pk, channels):
    """Walks a setup header (Vorbis I spec 4.2.4) recording the bit position of
    every field the damaged-stream generator edits. Returns a dict with
    fields[(tag)] = (bitpos, nbits, value), floor_types, modes (blockflags)."""
    if pk[0] != 5 or pk[1:7] != b"vorbis":
        raise ValueError("not a Vorbis setup header")
    r = Bits(pk)
    r.p = 56
    f = {}

    def rd(tag, n):
        pos = r.p
        v = r.read(n)
        if tag is not None:
            f[tag] = (pos, n, v)
        return v

    nbooks = rd("codebook_count", 8) + 1
    book_lookup = []
    for b in range(nbooks):
        rd("cb%d.sync" % b, 24)
        dims = rd("cb%d.dims" % b, 16)
        entries = rd("cb%d.entries" % b, 24)
        ordered = rd("cb%d.ordered" % b, 1)
        if not ordered:
            sparse = rd("cb%d.sparse" % b, 1)
            for e in range(entries):
                if sparse:
                    if rd("cb%d.used%d" % (b, e) if e == 0 else None, 1):
                        rd("cb%d.len%d" % (b, e) if e == 0 else None, 5)
                else:
                    rd("cb%d.len%d" % (b, e) if e == 0 else None, 5)
        else:
            rd("cb%d.len0" % b, 5)
            cur = 0
            while cur < entries:
                cur += r.read(ilog(entries - cur))
            if cur > entries:
                raise ValueError("ordered codebook overflows")
        lt = rd("cb%d.lookup" % b, 4)
        book_lookup.append(lt)
        if lt in (1, 2):
            r.skip(64)
            vbits = rd("cb%d.value_bits" % b, 4) + 1
            r.skip(1)
            nvals = lookup1_values(entries, dims) if lt == 1 else entries * dims
            r.skip(vbits * nvals)
        elif lt != 0:
            raise ValueError("lookup type %d" % lt)
    ntime = rd("time_count", 6) + 1
    for t in range(ntime):
        rd("time%d" % t, 16)
    nfloors = rd("floor_count", 6) + 1
    floor_types = []
    for i in range(nfloors):
        ft = rd("floor%d.type" % i, 16)
        floor_types.append(ft)
        if ft == 0:
            rd("floor%d.order" % i, 8)
            r.skip(16 + 16 + 6 + 8)
            nb = rd("floor%d.books" % i, 4) + 1
            r.skip(8 * nb)
        elif ft == 1:
            parts = rd("floor%d.partitions" % i, 5)
            pclass = [r.read(4) for _ in range(parts)]
            maxc = max(pclass) + 1 if pclass else 0
            cdims = []
            for c in range(maxc):
                cdims.append(r.read(3) + 1)
                sub = rd("floor%d.class%d.subclasses" % (i, c), 2)
                if sub:
                    rd("floor%d.class%d.masterbook" % (i, c), 8)
                for j in range(1 << sub):
                    rd("floor%d.class%d.subbook%d" % (i, c, j), 8)
            rd("floor%d.multiplier" % i, 2)
            rb = rd("floor%d.rangebits" % i, 4)
            k = 0
            for pc in pclass:
                for _ in range(cdims[pc]):
                    rd("floor%d.x%d" % (i, k), rb)
                    k += 1
        else:
            raise ValueError("floor type %d" % ft)
    nres = rd("residue_count", 6) + 1
    for i in range(nres):
        rt = rd("residue%d.type" % i, 16)
        if rt > 2:
            raise ValueError("residue type %d" % rt)
        rd("residue%d.begin" % i, 24)
        rd("residue%d.end" % i, 24)
        rd("residue%d.partsize" % i, 24)
        ncls = rd("residue%d.classifications" % i, 6) + 1
        rd("residue%d.classbook" % i, 8)
        casc = []
        for c in range(ncls):
            lo = r.read(3)
            hi = r.read(5) if r.read(1) else 0
            casc.append(hi * 8 + lo)
        first = True
        for c in range(ncls):
            for j in range(8):
                if casc[c] & (1 << j):
                    rd("residue%d.book" % i if first else None, 8)
                    first = False
    nmaps = rd("mapping_count", 6) + 1
    for i in range(nmaps):
        rd("mapping%d.type" % i, 16)
        submaps = rd("mapping%d.submaps" % i, 4) + 1 if rd("mapping%d.submap_flag" % i, 1) else 1
        if rd("mapping%d.coupling_flag" % i, 1):
            steps = rd("mapping%d.coupling_steps" % i, 8) + 1
            cb = ilog(channels - 1)
            for s in range(steps):
                rd("mapping%d.magnitude%d" % (i, s), cb)
                rd("mapping%d.angle%d" % (i, s), cb)
        rd("mapping%d.reserved" % i, 2)
        if submaps > 1:
            for c in range(channels):
                rd("mapping%d.mux%d" % (i, c), 4)
        for s in range(submaps):
            rd(None, 8)
            rd("mapping%d.submap%d.floor" % (i, s), 8)
            rd("mapping%d.submap%d.residue" % (i, s), 8)
    nmodes = rd("mode_count", 6) + 1
    modes = []
    for i in range(nmodes):
        modes.append(rd("mode%d.blockflag" % i, 1))
        rd("mode%d.windowtype" % i, 16)
        rd("mode%d.transformtype" % i, 16)
        rd("mode%d.mapping" % i, 8)
    rd("framing", 1)
    return {"fields": f, "floor_types": floor_types, "modes": modes, "codebooks": nbooks,
            "lookup_types": book_lookup, "residues": nres, "mappings": nmaps}


class Stream:
    """A parsed single-link Vorbis stream as written by vorbis_corpus."""

    def __init__(self, data):
        self.data = data
        self.pages = parse_pages(data)
        self.pkts = packets_of(data, self.pages)
        self.serial = self.pages[0].serial
        self.idh = parse_id(self.pkts[0]["data"])
        self.setup = parse_setup(self.pkts[2]["data"], self.idh["channels"])
        self.granules = self._granules()
        self._check_granules()

    def _granules(self):
        modes = self.setup["modes"]
        nb = ilog(len(modes) - 1)
        g, prev = 0, None
        out = [0, 0, 0]
        for pk in self.pkts[3:]:
            d = pk["data"]
            if not d or d[0] & 1:
                out.append(out[-1])
                continue
            mode = (d[0] >> 1) & ((1 << nb) - 1)
            bs = self.idh["bs1"] if mode < len(modes) and modes[mode] else self.idh["bs0"]
            if prev is not None:
                g += prev // 4 + bs // 4
            prev = bs
            out.append(g)
        # the stream's own end granule trims the final packet
        if len(self.pkts) > 3:
            out[-1] = self.pages[-1].granule
        return out

    def _check_granules(self):
        """Self-test of the setup parser: computed granules must equal the encoder's
        page granules on every page where a packet ends (except the trimmed last page)."""
        for i, pk in enumerate(self.pkts):
            last = pk["last_page"]
            ends_here = [j for j, q in enumerate(self.pkts) if q["last_page"] == last]
            if i == ends_here[-1] and last < len(self.pages) - 1 and i >= 3:
                if self.pages[last].granule != self.granules[i]:
                    raise AssertionError("granule mismatch on page %d: %d vs %d"
                                         % (last, self.pages[last].granule, self.granules[i]))

    def packet_list(self):
        return [p["data"] for p in self.pkts]

    def edit_packet(self, idx, edits, fix=True):
        """edits: {byte offset within packet: new byte value}; CRCs refreshed when fix."""
        buf = bytearray(self.data)
        offs = []
        for k, v in edits.items():
            o = pkt_file_offset(self.pkts[idx], k)
            buf[o] = v
            offs.append(o)
        if fix:
            refresh_crc(buf, self.pages, offs)
        return bytes(buf)

    def edit_packet_bits(self, idx, bitpos, nbits, value):
        pk = bytearray(self.pkts[idx]["data"])
        set_bits(pk, bitpos, nbits, value)
        orig = self.pkts[idx]["data"]
        return self.edit_packet(idx, {k: pk[k] for k in range(len(pk)) if pk[k] != orig[k]})

    def remux(self, packets=None, granules=None, layout="std", eos=True, serial=None, seq0=0):
        return mux(packets if packets is not None else self.packet_list(),
                   granules if granules is not None else self.granules,
                   self.serial if serial is None else serial, layout, eos=eos, seq0=seq0)


# --------------------------------------------------------------------------- corpus

class Corpus:
    def __init__(self, exe, outdir, quick):
        self.exe = exe
        self.out = outdir
        self.quick = quick
        self.entries = {}
        self.damaged = []  # (relpath, data, desc) collected, written at the end
        self.skipped = []

    def _write(self, rel, data, desc):
        path = os.path.join(self.out, *rel.split("/"))
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as fh:
            fh.write(data)
        self.entries[rel] = (len(data), desc)

    def encode(self, rel, args, desc, quick=False, optional=False):
        """Encodes one clean stream; returns its bytes (None if skipped)."""
        if self.quick and not quick:
            return None
        path = os.path.join(self.out, *rel.split("/"))
        os.makedirs(os.path.dirname(path), exist_ok=True)
        r = subprocess.run([self.exe, path] + [str(a) for a in args], capture_output=True, text=True)
        if r.returncode == 3 and optional:
            self.skipped.append((rel, r.stderr.strip()))
            return None
        if r.returncode != 0:
            raise RuntimeError("encoder failed (%d) for %s: %s" % (r.returncode, rel, r.stderr.strip()))
        with open(path, "rb") as fh:
            data = fh.read()
        self.entries[rel] = (len(data), desc + " [" + " ".join(str(a) for a in args) + "]")
        return data

    def add(self, rel, data, desc, quick=False):
        if self.quick and not quick:
            return
        self._write(rel, data, desc)

    def damage(self, rel, data, desc):
        self.damaged.append((rel, data, desc))

    def finish(self):
        keep = self.damaged
        if self.quick:
            step = max(1, len(self.damaged) // 25)
            keep = self.damaged[::step]
        seen = set()
        for rel, data, desc in keep:
            if rel in seen or rel in self.entries:
                raise ValueError("duplicate name " + rel)
            seen.add(rel)
            self._write(rel, data, desc)
        lines = []
        for rel in sorted(self.entries):
            size, desc = self.entries[rel]
            lines.append("%s %d %s" % (rel, size, " ".join(desc.split())))
        with open(os.path.join(self.out, "manifest.txt"), "w", encoding="utf-8", newline="\n") as fh:
            fh.write("\n".join(lines) + "\n")
        return len(lines)


VIEWER_LENGTHS = [1, 7, 63, 64, 65, 255, 256, 257, 1000, 4096, 44100, 441000, 1323000, 2646000]
SHORT_SIGNALS = ["sine", "noise", "impulse", "silence", "mix", "clip", "transient"]
ALL_SIGNALS = ["sine", "sweep", "noise", "transient", "silence", "clip", "square", "mix", "dc", "impulse", "gaps"]


def gen_clean(c):
    bases = {}
    # ---- bases for the damaged variants (always generated, also in --quick)
    bases["viewer"] = c.encode("clean/base/base_viewer_1s.ogg",
                               ["--viewer", "--samples", 44100, "--signal", "mix", "--seed", 101, "--serial", 0x1001],
                               "base: viewer path, 1 s mono mix", quick=True)
    bases["stereo"] = c.encode("clean/base/base_stereo_2s.ogg",
                               ["--channels", 2, "--samples", 88200, "--q", 0.5, "--signal", "mix", "--seed", 102,
                                "--serial", 0x1002], "base: 2 s stereo q0.5 mix", quick=True)
    bases["hq"] = c.encode("clean/base/base_hq_1s.ogg",
                           ["--channels", 2, "--samples", 44100, "--q", 1.0, "--signal", "noise", "--seed", 103,
                            "--serial", 0x1003], "base: 1 s stereo q1.0 noise (packets span pages)", quick=True)
    bases["short"] = c.encode("clean/base/base_short.ogg",
                              ["--viewer", "--samples", 4096, "--signal", "transient", "--seed", 104, "--serial", 0x1004],
                              "base: viewer path, 4096 samples transient", quick=True)
    links = {}
    links["a"] = c.encode("clean/base/link_a_44100_mono.ogg",
                          ["--viewer", "--samples", 22050, "--signal", "sine", "--seed", 111, "--serial", 0x2001],
                          "chain link: viewer 44100 mono 0.5 s", quick=True)
    links["a2"] = c.encode("clean/base/link_a2_44100_mono.ogg",
                           ["--viewer", "--samples", 22050, "--signal", "sine", "--seed", 111, "--serial", 0x2005],
                           "chain link: as link_a with another serial", quick=True)
    links["b"] = c.encode("clean/base/link_b_22050_stereo.ogg",
                          ["--channels", 2, "--rate", 22050, "--samples", 11025, "--q", 0.3, "--signal", "mix",
                           "--seed", 112, "--serial", 0x2002], "chain link: 22050 stereo 0.5 s", quick=True)
    links["c"] = c.encode("clean/base/link_c_8000_mono.ogg",
                          ["--rate", 8000, "--samples", 4000, "--q", 0.0, "--signal", "sweep", "--seed", 113,
                           "--serial", 0x2003], "chain link: 8000 mono 0.5 s", quick=True)
    links["d"] = c.encode("clean/base/link_d_48000_6ch.ogg",
                          ["--channels", 6, "--rate", 48000, "--samples", 12000, "--q", 0.2, "--signal", "transient",
                           "--seed", 114, "--serial", 0x2004], "chain link: 48000 6 ch 0.25 s", quick=True)
    links["e"] = c.encode("clean/base/link_e_empty.ogg",
                          ["--rate", 32000, "--samples", 0, "--seed", 115, "--serial", 0x2006],
                          "chain link: 0 samples", quick=True)

    # ---- viewer path (LLVorbisEncode): mono 44100 q0.05, lengths up to the 60 s OpenSim limit
    for n in VIEWER_LENGTHS:
        if n <= 44100:
            sigs = SHORT_SIGNALS
        elif n <= 441000:
            sigs = ["mix", "sweep", "noise", "gaps"]
        elif n <= 1323000:
            sigs = ["mix", "sweep", "gaps"]
        else:
            sigs = ["mix", "noise"]
        for s in sigs:
            c.encode("clean/viewer/v16m_%s_%07d.ogg" % (s, n),
                     ["--viewer", "--samples", n, "--signal", s, "--seed", 1000 + n],
                     "viewer path 16-bit mono WAV, %d samples (%.3f s) %s" % (n, n / 44100.0, s),
                     quick=(n in (1, 257, 44100) and s in ("mix", "impulse")))
    c.encode("clean/viewer/v16m_mix_0000000.ogg", ["--viewer", "--samples", 0, "--signal", "mix"],
             "viewer path, empty data chunk (headers + EOS only)", quick=True)
    for n in [1, 1000, 1023, 1024, 1025, 44100, 441000, 1323000]:
        for s in ["mix", "sine", "clip"]:
            c.encode("clean/viewer/v16s_%s_%07d.ogg" % (s, n),
                     ["--viewer", "--channels", 2, "--samples", n, "--signal", s, "--seed", 2000 + n],
                     "viewer path 16-bit stereo WAV downmixed by LLVorbisEncode, %d samples %s" % (n, s),
                     quick=(n == 44100 and s == "mix"))
    for ch in (1, 2):
        for n in [100, 44100]:
            c.encode("clean/viewer/v8%s_mix_%07d.ogg" % ("m" if ch == 1 else "s", n),
                     ["--viewer", "--channels", ch, "--bits", 8, "--samples", n, "--signal", "mix", "--seed", 3000 + n],
                     "viewer path 8-bit %s WAV, %d samples" % ("mono" if ch == 1 else "stereo", n))

    # ---- quality sweep: libvorbis quality -1..10
    for qi in range(-1, 11):
        q = qi / 10.0
        for ch in (1, 2):
            c.encode("clean/quality/q%+03d_%dch_mix.ogg" % (qi, ch),
                     ["--channels", ch, "--samples", 132300, "--q", "%.1f" % q, "--signal", "mix", "--seed", 4000 + qi],
                     "quality %d (q=%.1f), %d ch, 3 s mix" % (qi, q, ch), quick=(qi in (-1, 10) and ch == 2))
    for q in ("-0.1", "1.0"):
        c.encode("clean/quality/q%s_2ch_noise.ogg" % q, ["--channels", 2, "--samples", 88200, "--q", q,
                                                          "--signal", "noise", "--seed", 4100],
                 "extreme quality on white noise (largest packets)")
    for q in ("0.1", "0.5", "1.0"):
        c.encode("clean/quality/q%s_2ch_uncoupled.ogg" % q, ["--channels", 2, "--samples", 88200, "--q", q,
                                                              "--signal", "mix", "--no-coupling", "--seed", 4200],
                 "stereo without channel coupling")

    # ---- sample rates
    for rate in [8000, 11025, 16000, 22050, 32000, 44100, 48000, 96000, 192000, 7999, 12345, 50000]:
        for ch in (1, 2):
            for q in ("0.0", "0.6"):
                c.encode("clean/rates/r%06d_%dch_q%s.ogg" % (rate, ch, q),
                         ["--channels", ch, "--rate", rate, "--samples", rate * 2, "--q", q, "--signal", "mix",
                          "--seed", 5000 + rate],
                         "%d Hz %d ch q%s 2 s mix" % (rate, ch, q),
                         quick=(rate in (8000, 48000) and ch == 2 and q == "0.0"), optional=True)

    # ---- channel counts
    for ch in [1, 2, 3, 4, 5, 6, 7, 8, 12, 16, 32, 255]:
        n = 44100 if ch <= 8 else 8820
        c.encode("clean/channels/c%03d_q0.3_mix.ogg" % ch,
                 ["--channels", ch, "--samples", n, "--q", 0.3, "--signal", "mix", "--seed", 6000 + ch],
                 "%d channels q0.3 %d samples mix" % (ch, n), quick=(ch == 6), optional=True)
    for ch in [3, 6, 8]:
        c.encode("clean/channels/c%03d_managed_96k.ogg" % ch,
                 ["--channels", ch, "--samples", 44100, "--managed", "-1,%d" % (96000 * ch // 2) + ",-1",
                  "--signal", "transient", "--seed", 6100 + ch],
                 "%d channels managed average bitrate" % ch, optional=True)

    # ---- bitrate management
    managed = [("32k_mono", 1, "-1,32000,-1"), ("64k_abr", 2, "-1,64000,-1"), ("128k_abr", 2, "-1,128000,-1"),
               ("256k_abr", 2, "-1,256000,-1"), ("48-64-96k", 2, "48000,64000,96000"),
               ("max160k", 2, "-1,128000,160000"), ("min96k", 2, "96000,128000,-1"),
               ("cbr128k", 2, "128000,128000,128000"), ("cbr32k_mono", 1, "32000,32000,32000"),
               ("max48k_only", 2, "-1,-1,48000"), ("min200k_only", 2, "200000,-1,-1")]
    for name, ch, spec in managed:
        c.encode("clean/bitrate/managed_%s.ogg" % name,
                 ["--channels", ch, "--samples", 88200, "--managed", spec, "--signal", "mix", "--seed", 7000],
                 "managed bitrate min,nom,max=%s, %d ch, 2 s mix" % (spec, ch),
                 quick=(name == "cbr128k"), optional=True)
    for nom in [45000, 96000, 192000, 320000]:
        c.encode("clean/bitrate/vbr_%dk.ogg" % (nom // 1000),
                 ["--channels", 2, "--samples", 88200, "--vbr-bitrate", nom, "--signal", "mix", "--seed", 7100],
                 "VBR aimed at %d bit/s (oggenc -b without --managed)" % nom, optional=True)

    # ---- signals
    for s in ALL_SIGNALS:
        c.encode("clean/signals/%s_1ch_q0.05.ogg" % s, ["--samples", 88200, "--q", 0.05, "--signal", s, "--seed", 8000],
                 "signal %s, mono q0.05 (viewer quality) 2 s" % s, quick=(s == "clip"))
        c.encode("clean/signals/%s_2ch_q0.5.ogg" % s,
                 ["--channels", 2, "--samples", 88200, "--q", 0.5, "--signal", s, "--seed", 8001],
                 "signal %s, stereo q0.5 2 s" % s)
    c.encode("clean/signals/clip_2ch_q1.0_amp3.ogg", ["--channels", 2, "--samples", 44100, "--q", 1.0,
                                                       "--signal", "clip", "--amp", 3.0],
             "clipping-level signal at 3x full scale, q1.0")
    c.encode("clean/signals/transient_6ch_q0.4.ogg", ["--channels", 6, "--samples", 88200, "--q", 0.4,
                                                       "--signal", "transient", "--seed", 8002],
             "transients on 6 channels (short blocks)")
    c.encode("clean/signals/gaps_1ch_q0.05_20s.ogg", ["--samples", 882000, "--q", 0.05, "--signal", "gaps"],
             "long digital-silence stretches, 20 s")
    c.encode("clean/signals/silence_2ch_q0.5_10s.ogg", ["--channels", 2, "--samples", 441000, "--q", 0.5,
                                                         "--signal", "silence"], "entire file silent, 10 s")
    c.encode("clean/signals/dc_2ch_full.ogg", ["--channels", 2, "--samples", 44100, "--signal", "dc", "--amp", 1.0],
             "DC at full scale")

    # ---- lengths (non-viewer path, block-size boundaries)
    for n in [0, 1, 2, 3, 100, 127, 128, 129, 255, 256, 257, 1023, 1024, 1025, 2047, 2048, 2049, 4095, 4096,
              4097, 10000]:
        c.encode("clean/lengths/l%05d_2ch_48k.ogg" % n,
                 ["--channels", 2, "--rate", 48000, "--samples", n, "--q", 0.4, "--signal", "mix", "--seed", 9000 + n],
                 "%d samples, stereo 48 kHz q0.4" % n, quick=(n in (0, 3, 2049)))
    for n in [1, 64, 1000]:
        c.encode("clean/lengths/l%05d_1ch_chunk1.ogg" % n,
                 ["--samples", n, "--chunk", 1, "--signal", "sine", "--seed", 9100],
                 "%d samples submitted one sample per vorbis_analysis_buffer call" % n)
    c.encode("clean/lengths/l60s_2ch_44k_q0.4.ogg", ["--channels", 2, "--samples", 2646000, "--q", 0.4,
                                                      "--signal", "mix", "--seed", 9200], "60 s stereo q0.4 mix")
    c.encode("clean/lengths/l60s_6ch_48k_q0.0.ogg", ["--channels", 6, "--rate", 48000, "--samples", 2880000,
                                                      "--q", 0.0, "--signal", "mix", "--seed", 9201],
             "60 s 6 ch 48 kHz q0.0 mix")

    # ---- comments
    short = ["--samples", 22050, "--signal", "sine", "--seed", 9300]
    c.encode("clean/comments/none.ogg", short, "no user comments", quick=True)
    c.encode("clean/comments/several.ogg", short + ["--comment", "TITLE=Test title", "--comment", "ARTIST=Vulkanstorm",
                                                    "--comment", "ALBUM=Corpus", "--comment", "TRACKNUMBER=7",
                                                    "--comment", "DATE=2026"], "several standard tags")
    c.encode("clean/comments/mixed_case.ogg", short + ["--comment", "Title=Mixed case", "--comment", "artist=lower",
                                                       "--comment", "tItLe=second title"],
             "mixed-case tag names, repeated tag")
    c.encode("clean/comments/odd_fields.ogg", short + ["--comment", "EMPTY=", "--comment", "NOEQUALS",
                                                       "--comment", "=novalue", "--comment", "A=b=c",
                                                       "--comment", "SPACE IN NAME=x"],
             "empty value, no '=', empty name, '=' in value")
    utf8 = "TITLE=Grüße aus Köln – 日本語 🎵".encode("utf-8").hex()
    c.encode("clean/comments/utf8.ogg", short + ["--comment-hex", utf8,
                                                 "--comment-hex", "ARTIST=Zoë Ægir".encode("utf-8").hex()],
             "UTF-8 values")
    rng = Rng(0xC0)
    long_text = "".join(chr(0x61 + rng.below(26)) if rng.below(8) else " " for _ in range(10240))
    c.encode("clean/comments/long_10k.ogg", short + ["--comment", "DESCRIPTION=" + long_text], "one 10 kB comment")
    many = []
    for i in range(300):
        many += ["--comment", "TAG%03d=value %d" % (i, i * 7)]
    c.encode("clean/comments/many_300.ogg", short + many, "300 comments")
    c.encode("clean/comments/vendor_empty.ogg", short + ["--vendor-free", "--comment", "TITLE=x"],
             "empty vendor string", quick=True)
    c.encode("clean/comments/vendor_long.ogg", short + ["--vendor-hex", (b"V" * 5000).hex()], "5000-byte vendor string")
    c.encode("clean/comments/vendor_utf8.ogg", short + ["--vendor-hex", "Encodé par ☃".encode("utf-8").hex()],
             "UTF-8 vendor string")

    # ---- page layouts written by libogg
    pl = ["--channels", 2, "--samples", 88200, "--q", 0.3, "--signal", "mix", "--seed", 9400]
    for k in [1, 2, 7]:
        c.encode("clean/pages/flush_every_%d.ogg" % k, pl + ["--flush-every", k],
                 "ogg_stream_flush after every %d audio packets" % k, quick=(k == 1))
    for n in [1, 255, 1000, 16000, 65000]:
        c.encode("clean/pages/pageout_fill_%d.ogg" % n, pl + ["--fill", n], "ogg_stream_pageout_fill(%d)" % n)

    # ---- chains (concatenated complete streams)
    def chain(rel, parts, desc, quick=False):
        if any(p is None for p in parts):
            return
        c.add(rel, b"".join(parts), desc, quick=quick)
    chain("clean/chained/chain_44m_22s_8m.ogg", [links["a"], links["b"], links["c"]],
          "3 links: 44100 mono + 22050 stereo + 8000 mono", quick=True)
    chain("clean/chained/chain_same_params.ogg", [links["a"], links["a2"]], "2 links, same parameters, different serials")
    chain("clean/chained/chain_8m_6ch_44m.ogg", [links["c"], links["d"], links["a"]], "3 links: 8000 mono + 48000 6 ch + 44100 mono")
    chain("clean/chained/chain_with_empty_link.ogg", [links["a"], links["e"], links["b"]], "middle link has 0 samples")
    chain("clean/chained/chain_5links.ogg", [links["a"], links["b"], links["c"], links["d"], links["a2"]], "5 links")
    chain("clean/chained/chain_viewer_bases.ogg", [bases["viewer"], bases["short"]], "two viewer-path streams")
    return bases, links


# --------------------------------------------------------------------------- damaged

def gen_damaged(c, bases, links):
    D = c.damage
    V = Stream(bases["viewer"])
    S = Stream(bases["stereo"])
    H = Stream(bases["hq"])
    T = Stream(bases["short"])
    rng = Rng(0xDA4A6E)

    # -- truncation
    def trunc_set(st, tag, fine):
        n = len(st.data)
        pts = set()
        for p in st.pages:
            if fine:
                pts.update([p.off + 1, p.off + 4, p.off + 26, p.off + 27, p.body_off, p.body_off + p.blen // 2,
                            p.end - 1, p.end])
            else:
                pts.update([p.body_off + p.blen // 2, p.end - 1, p.end])
        for x in sorted(x for x in pts if 0 < x < n):
            D("damaged/truncate/%s_at_%06d.ogg" % (tag, x), st.data[:x], "%s truncated to %d of %d bytes" % (tag, x, n))
    trunc_set(V, "viewer", True)
    trunc_set(T, "short", True)
    trunc_set(S, "stereo", True)
    trunc_set(H, "hq", False)
    # inside each header packet (ids at fixed fractions)
    for i in range(3):
        pk = S.pkts[i]
        for frac in (1, 2, 3):
            o = pkt_file_offset(pk, len(pk["data"]) * frac // 4)
            D("damaged/truncate/stereo_hdr%d_%d4.ogg" % (i, frac), S.data[:o],
              "truncated %d/4 into header packet %d" % (frac, i))

    # -- ends mid-packet: last page of the kept prefix ends in a continued (255) segment
    cut = [p for p in H.pages[3:-1] if p.lacing[-1] == 255]
    for k, p in enumerate(cut[:4]):
        D("damaged/midpacket/hq_cut_after_page_seq%d.ogg" % p.seq, H.data[:p.end],
          "stream ends after a page whose last packet continues (no EOS)")
        buf = bytearray(H.data[:p.end])
        buf[p.off + 5] |= 4
        refresh_crc(buf, H.pages, [p.off])
        D("damaged/midpacket/hq_cut_eos_page_seq%d.ogg" % p.seq, bytes(buf),
          "EOS page ends in the middle of a packet")

    # -- single and multiple corrupted bytes
    def audio_range(st):
        """file range of the audio pages (everything after the setup header's page)"""
        return st.pages[st.pkts[2]["last_page"] + 1].off, len(st.data)

    for st, tag, count in ((V, "viewer", 30), (S, "stereo", 30), (T, "short", 15)):
        lo, hi = audio_range(st)
        for i in range(count):
            o = lo + rng.below(hi - lo)
            x = 1 + rng.below(255)
            buf = bytearray(st.data)
            buf[o] ^= x
            fixed = i % 3 != 2
            if fixed:
                refresh_crc(buf, st.pages, [o])
            D("damaged/corrupt/%s_byte%02d_%06d%s.ogg" % (tag, i, o, "" if fixed else "_badcrc"), bytes(buf),
              "byte %d xor 0x%02x in audio pages, CRC %s" % (o, x, "recomputed" if fixed else "left stale"))
    for st, tag in ((V, "viewer"), (S, "stereo")):
        lo, hi = 0, st.pages[st.pkts[2]["last_page"]].end
        for i in range(20):
            o = lo + rng.below(hi - lo)
            buf = bytearray(st.data)
            buf[o] ^= 1 + rng.below(255)
            refresh_crc(buf, st.pages, [o])
            D("damaged/corrupt/%s_hdrbyte%02d_%05d.ogg" % (tag, i, o), bytes(buf),
              "header-page byte %d corrupted, CRC recomputed" % o)
    for n in (5, 20, 100, 1000):
        lo, hi = audio_range(S)
        buf = bytearray(S.data)
        offs = [lo + rng.below(hi - lo) for _ in range(n)]
        for o in offs:
            buf[o] = rng.below(256)
        # keep page structure parsable: restore page headers, then fix CRCs
        for p in S.pages:
            buf[p.off:p.body_off] = S.data[p.off:p.body_off]
        refresh_crc(buf, S.pages, offs)
        D("damaged/corrupt/stereo_multi_%04d.ogg" % n, bytes(buf), "%d random audio bytes replaced, CRC recomputed" % n)
    for i in range(20):
        pi = 3 + rng.below(len(S.pkts) - 3)
        pk = S.pkts[pi]["data"]
        bit = rng.below(len(pk) * 8)
        D("damaged/corrupt/stereo_bitflip%02d_pkt%d_bit%d.ogg" % (i, pi, bit),
          S.edit_packet(pi, {bit >> 3: pk[bit >> 3] ^ (1 << (bit & 7))}), "single bit flip in audio packet %d" % pi)
    # audio packet header bit: packet type bit set (not an audio packet)
    for pi in (3, 4, len(S.pkts) // 2, len(S.pkts) - 1):
        pk = S.pkts[pi]["data"]
        D("damaged/corrupt/stereo_pkt%d_typebit.ogg" % pi, S.edit_packet(pi, {0: pk[0] | 1}),
          "audio packet %d with the packet-type bit set" % pi)
        D("damaged/corrupt/stereo_pkt%d_modeflip.ogg" % pi, S.edit_packet(pi, {0: pk[0] ^ 2}),
          "audio packet %d with its mode bit flipped (block size mismatch)" % pi)

    # -- page-header fields
    for st, tag in ((V, "viewer"), (S, "stereo")):
        for idx in sorted({0, 1, 2, 3, len(st.pages) - 1}):
            if idx >= len(st.pages):
                continue
            p = st.pages[idx]
            buf = bytearray(st.data)
            buf[p.off + 3] = ord("T")
            D("damaged/pagehdr/%s_p%d_capture.ogg" % (tag, idx), bytes(buf), "capture pattern OggT on page %d" % idx)
            D("damaged/pagehdr/%s_p%d_version1.ogg" % (tag, idx), set_page_field(st.data, st.pages, idx, "<B", 4, 1),
              "stream structure version 1 on page %d" % idx)
            D("damaged/pagehdr/%s_p%d_badcrc.ogg" % (tag, idx),
              set_page_field(st.data, st.pages, idx, "<I", 22, p.crc ^ 0x00010000, fix=False), "wrong CRC on page %d" % idx)
            D("damaged/pagehdr/%s_p%d_serial.ogg" % (tag, idx),
              set_page_field(st.data, st.pages, idx, "<I", 14, p.serial ^ 0x55), "foreign serial on page %d" % idx)
            D("damaged/pagehdr/%s_p%d_seqjump.ogg" % (tag, idx),
              set_page_field(st.data, st.pages, idx, "<I", 18, p.seq + 5), "sequence number +5 on page %d" % idx)
            for bit, nm in ((1, "cont"), (2, "bos"), (4, "eos")):
                D("damaged/pagehdr/%s_p%d_flag_%s.ogg" % (tag, idx, nm),
                  set_page_field(st.data, st.pages, idx, "<B", 5, p.htype ^ bit),
                  "header-type flag %s toggled on page %d" % (nm, idx))
            # lacing edits keep total length: move one byte between the first two segments
            if len(p.lacing) >= 2 and p.lacing[0] > 0 and p.lacing[1] < 255:
                buf = bytearray(st.data)
                buf[p.off + 27] -= 1
                buf[p.off + 28] += 1
                refresh_crc(buf, st.pages, [p.off])
                D("damaged/pagehdr/%s_p%d_lacing_shift.ogg" % (tag, idx), bytes(buf),
                  "lacing values shifted by one byte on page %d" % idx)
            for delta, nm in ((1, "plus1"), (-1, "minus1")):
                ns = len(p.lacing) + delta
                if 0 <= ns <= 255:
                    buf = bytearray(st.data)
                    buf[p.off + 26] = ns
                    refresh_crc(buf, st.pages, [p.off])
                    D("damaged/pagehdr/%s_p%d_nseg_%s.ogg" % (tag, idx, nm), bytes(buf),
                      "segment count %+d on page %d (CRC over the declared page)" % (delta, idx))
    # every page's sequence number duplicated from its predecessor
    for idx in range(1, len(S.pages)):
        D("damaged/pagehdr/stereo_p%d_seqdup.ogg" % idx,
          set_page_field(S.data, S.pages, idx, "<I", 18, S.pages[idx - 1].seq), "page %d repeats the previous sequence number" % idx)

    # -- granule positions
    for st, tag in ((V, "viewer"), (S, "stereo"), (T, "short")):
        last = len(st.pages) - 1
        g = st.pages[last].granule
        prev = st.pages[last - 1].granule if last >= 1 else 0
        for nm, val in (("minus1", -1), ("zero", 0), ("less1", g - 1), ("less1000", g - 1000), ("plus1", g + 1),
                        ("plus1000", g + 1000), ("plus100000", g + 100000), ("huge", (1 << 62)),
                        ("below_prev", max(0, prev - 500)), ("int32max", 0x7FFFFFFF), ("neg2", -2)):
            D("damaged/granule/%s_last_%s.ogg" % (tag, nm), set_page_field(st.data, st.pages, last, "<q", 6, val),
              "last-page granule %s (%d, real %d)" % (nm, val, g))
        for idx in range(3, last):
            for nm, val in (("minus1", -1), ("huge", 1 << 40), ("zero", 0)):
                D("damaged/granule/%s_p%d_%s.ogg" % (tag, idx, nm), set_page_field(st.data, st.pages, idx, "<q", 6, val),
                  "granule %s on middle page %d" % (nm, idx))
        D("damaged/granule/%s_hdr_granule.ogg" % tag, set_page_field(st.data, st.pages, 0, "<q", 6, 12345),
          "non-zero granule on the identification-header page")
        D("damaged/granule/%s_eosless.ogg" % tag, set_page_field(st.data, st.pages, last, "<B", 5,
                                                                  st.pages[last].htype & ~4),
          "EOS flag cleared on the last page")

    # -- removed / duplicated / swapped pages
    for st, tag in ((V, "viewer"), (S, "stereo"), (T, "short")):
        pb = page_bytes(st.data, st.pages)
        n = len(pb)
        for i in range(n):
            D("damaged/pages/%s_drop%02d.ogg" % (tag, i), b"".join(pb[:i] + pb[i + 1:]),
              "page %d removed (sequence gap)" % i)
            if i >= 3 and tag == "stereo":
                D("damaged/pages/%s_drop%02d_renum.ogg" % (tag, i), repage(pb[:i] + pb[i + 1:], renumber_from=0),
                  "page %d removed, later pages renumbered (no sequence gap)" % i)
        for i in range(n):
            D("damaged/pages/%s_dup%02d.ogg" % (tag, i), b"".join(pb[:i + 1] + pb[i:]), "page %d duplicated" % i)
        for i in range(n - 1):
            D("damaged/pages/%s_swap%02d.ogg" % (tag, i), b"".join(pb[:i] + [pb[i + 1], pb[i]] + pb[i + 2:]),
              "pages %d and %d swapped" % (i, i + 1))
        if n > 5:
            D("damaged/pages/%s_drop_3_4.ogg" % tag, b"".join(pb[:3] + pb[5:]), "two consecutive audio pages removed")
        hdr_end = st.pkts[2]["last_page"] + 1
        D("damaged/pages/%s_headers_only.ogg" % tag, b"".join(pb[:hdr_end]), "header pages only, no audio, no EOS")
        D("damaged/pages/%s_first_page_only.ogg" % tag, pb[0], "identification page only")
        D("damaged/pages/%s_no_bos_page.ogg" % tag, b"".join(pb[1:]), "first (BOS) page missing")
        D("damaged/pages/%s_reversed_audio.ogg" % tag, b"".join(pb[:hdr_end] + pb[hdr_end:][::-1]),
          "audio pages in reverse order")

    # -- garbage around and between pages
    pb = page_bytes(S.data, S.pages)
    for n in (1, 100, 5000, 70000):
        D("damaged/garbage/stereo_prepend_%05d.ogg" % n, rng.bytes(n) + S.data, "%d random bytes before the stream" % n)
    D("damaged/garbage/stereo_prepend_oggs_junk.ogg", b"OggS" + rng.bytes(200) + S.data, "'OggS' plus junk before the stream")
    D("damaged/garbage/stereo_prepend_zeros_70000.ogg", bytes(70000) + S.data, "70000 zero bytes before the stream")
    for n in (1, 27, 100, 5000):
        D("damaged/garbage/stereo_append_%05d.ogg" % n, S.data + rng.bytes(n), "%d random bytes after the stream" % n)
    D("damaged/garbage/stereo_append_partial_page.ogg", S.data + pb[4][:40], "first 40 bytes of a page appended")
    for i in range(1, len(pb)):
        n = 1 + rng.below(300)
        D("damaged/garbage/stereo_insert_before_p%02d.ogg" % i, b"".join(pb[:i]) + rng.bytes(n) + b"".join(pb[i:]),
          "%d random bytes inserted before page %d" % (n, i))
    for i in (1, 3, 5):
        D("damaged/garbage/stereo_insert_oggs_p%02d.ogg" % i, b"".join(pb[:i]) + b"OggS" + rng.bytes(60) + b"".join(pb[i:]),
          "'OggS' + junk inserted before page %d" % i)
    foreign = make_page(0, 1000, 0xABCDEF, 3, [200], rng.bytes(200))
    for i in (1, 3, len(pb) - 1):
        D("damaged/garbage/stereo_foreign_page_p%02d.ogg" % i, b"".join(pb[:i]) + foreign + b"".join(pb[i:]),
          "valid page of an unknown serial inserted before page %d" % i)
    D("damaged/garbage/viewer_with_stereo_page.ogg",
      b"".join(page_bytes(V.data, V.pages)[:3]) + pb[4] + b"".join(page_bytes(V.data, V.pages)[3:]),
      "audio page of another stream spliced into the middle")

    # -- whole-file junk
    D("damaged/files/zero_length.ogg", b"", "empty file")
    D("damaged/files/one_byte.ogg", b"O", "1-byte file")
    D("damaged/files/oggs_only.ogg", b"OggS", "capture pattern only")
    D("damaged/files/page_header_only.ogg", pb[0][:27], "27-byte page header without segment table")
    D("damaged/files/oggs_junk.ogg", b"OggS" + rng.bytes(100), "'OggS' followed by junk")
    D("damaged/files/random_10k.ogg", rng.bytes(10240), "10 kB random bytes")
    D("damaged/files/random_200k.ogg", rng.bytes(204800), "200 kB random bytes")
    D("damaged/files/zeros_64k.ogg", bytes(65536), "64 kB of zero bytes")
    D("damaged/files/repeated_oggs.ogg", b"OggS" * 1024, "'OggS' repeated 1024 times")
    wav = b"RIFF" + struct.pack("<I", 36 + 8820) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, 1, 1, 44100, 88200, 2, 16) \
        + b"data" + struct.pack("<I", 8820) + rng.bytes(8820)
    D("damaged/files/riff_wav.ogg", wav, "a RIFF/WAVE file with .ogg name")
    D("damaged/files/empty_bos_page.ogg", make_page(2 | 4, 0, 77, 0, [], b""), "single BOS+EOS page with no segments")
    D("damaged/files/opus_stream.ogg",
      make_page(2, 0, 9, 0, [19], b"OpusHead\x01\x02\x38\x01\x44\xac\x00\x00\x00\x00\x00")
      + make_page(4, 0, 9, 1, [16], b"OpusTags\x00\x00\x00\x00\x00\x00\x00\x00"),
      "an Ogg Opus header stream (non-Vorbis first packet)")
    sp = S.packet_list()
    D("damaged/files/theora_then_vorbis_same_stream.ogg",
      mux([b"\x80theora" + bytes(34)] + sp, [0] + S.granules, S.serial, headers=4),
      "non-Vorbis first packet followed by the Vorbis headers in the same logical stream")
    D("damaged/files/vorbis_id_twice.ogg", mux([sp[0]] + sp, [0] + S.granules, S.serial, headers=4),
      "identification header repeated as the second packet")

    # -- identification header edits (CRC recomputed)
    for st, tag in ((V, "viewer"), (S, "stereo")):
        idp = st.pkts[0]["data"]
        edits = [("type2", {0: 2}), ("magic", {6: ord("z")}), ("version1", {7: 1}), ("channels0", {11: 0}),
                 ("channels255", {11: 255}), ("rate0", {12: 0, 13: 0, 14: 0, 15: 0}),
                 ("rate_max", {12: 255, 13: 255, 14: 255, 15: 255}), ("blocksizes_00", {28: 0x00}),
                 ("blocksizes_equal", {28: 0x88}), ("blocksizes_swapped", {28: ((idp[28] & 15) << 4) | (idp[28] >> 4)}),
                 ("blocksizes_ff", {28: 0xFF}), ("blocksizes_small", {28: 0x55}), ("blocksizes_14", {28: 0xE8}),
                 ("framing0", {29: 0}), ("bitrates", {k: 0xFF for k in range(16, 28)})]
        for nm, ed in edits:
            D("damaged/idhdr/%s_%s.ogg" % (tag, nm), st.edit_packet(0, ed), "identification header: %s" % nm)
        pk = st.packet_list()
        D("damaged/idhdr/%s_short_packet.ogg" % tag, st.remux([pk[0][:20]] + pk[1:]), "identification header cut to 20 bytes")
        D("damaged/idhdr/%s_long_packet.ogg" % tag, st.remux([pk[0] + b"\0" * 10] + pk[1:]),
          "identification header with 10 trailing bytes")

    # -- comment header edits
    for st, tag in ((V, "viewer"), (S, "stereo")):
        cp = st.pkts[1]["data"]
        vlen = struct.unpack_from("<I", cp, 7)[0]
        cnt_at = 11 + vlen

        def le(at, v):
            return {at + k: (v >> (8 * k)) & 255 for k in range(4)}
        for nm, ed in (("type", {0: 4}), ("magic", {1: ord("V")}), ("vendor_len_huge", le(7, 0xFFFFFFFF)),
                       ("vendor_len_over", le(7, vlen + 40)), ("vendor_len_neg", le(7, 0x80000000)),
                       ("count_huge", le(cnt_at, 0x7FFFFFFF)), ("count_over", le(cnt_at, 3)),
                       ("framing0", {len(cp) - 1: 0})):
            D("damaged/commenthdr/%s_%s.ogg" % (tag, nm), st.edit_packet(1, ed), "comment header: %s" % nm)
        pk = st.packet_list()
        D("damaged/commenthdr/%s_short_packet.ogg" % tag, st.remux([pk[0], pk[1][:9]] + pk[2:]), "comment header cut to 9 bytes")
        D("damaged/commenthdr/%s_empty_packet.ogg" % tag, st.remux([pk[0], b""] + pk[2:]), "zero-length comment header")
    # comment header with a comment whose length overruns the packet
    pk = S.packet_list()
    bad = b"\x03vorbis" + struct.pack("<I", 4) + b"test" + struct.pack("<II", 1, 1000) + b"TITLE=x\x01"
    D("damaged/commenthdr/stereo_comment_len_over.ogg", S.remux([pk[0], bad] + pk[2:]), "comment length beyond packet end")

    # -- setup header edits (bit positions from parse_setup)
    for st, tag in ((S, "stereo"), (V, "viewer")):
        f = st.setup["fields"]
        nb = st.setup["codebooks"]
        edits = [("codebook_count_plus1", "codebook_count", f["codebook_count"][2] + 1),
                 ("codebook_count_1", "codebook_count", 0),
                 ("cb0_sync", "cb0.sync", 0x564343), ("cblast_sync", "cb%d.sync" % (nb - 1), 0),
                 ("cb0_dims0", "cb0.dims", 0), ("cb0_entries0", "cb0.entries", 0),
                 ("cb0_entries_huge", "cb0.entries", 0xFFFFFF), ("cb0_ordered_flip", "cb0.ordered", 1 - f["cb0.ordered"][2]),
                 ("cb0_lookup3", "cb0.lookup", 3), ("cb0_len0_32", "cb0.len0", 31),
                 ("time_nonzero", "time0", 1), ("floor0_type2", "floor0.type", 2), ("floor0_type0", "floor0.type", 0),
                 ("floor0_partitions31", "floor0.partitions", 31), ("floor0_rangebits15", "floor0.rangebits", 15),
                 ("floor0_x0_zero", "floor0.x0", 0), ("floor0_subbook255", "floor0.class0.subbook0", 255),
                 ("residue0_type3", "residue0.type", 3), ("residue0_begin_gt_end", "residue0.begin", 0xFFFFFF),
                 ("residue0_classbook255", "residue0.classbook", 255), ("residue0_book255", "residue0.book", 255),
                 ("residue0_partsize_huge", "residue0.partsize", 0xFFFFFF),
                 ("mapping0_type1", "mapping0.type", 1), ("mapping0_reserved", "mapping0.reserved", 3),
                 ("mapping0_floor63", "mapping0.submap0.floor", 63), ("mapping0_residue63", "mapping0.submap0.residue", 63),
                 ("mode_count_63", "mode_count", 63), ("mode_count_1", "mode_count", 0),
                 ("mode0_blockflag", "mode0.blockflag", 1 - st.setup["modes"][0]),
                 ("mode0_window1", "mode0.windowtype", 1), ("mode0_transform1", "mode0.transformtype", 1),
                 ("mode0_mapping63", "mode0.mapping", 63), ("framing0", "framing", 0)]
        if "mapping0.magnitude0" in f:
            edits += [("mapping0_couple_same", "mapping0.magnitude0", f["mapping0.angle0"][2]),
                      ("mapping0_couple_steps_max", "mapping0.coupling_steps", 255)]
        if "mapping0.submap_flag" in f:
            edits += [("mapping0_submap_flag", "mapping0.submap_flag", 1)]
        for nm, field, val in edits:
            if field not in f:
                continue
            pos, nbits, old = f[field]
            val &= (1 << nbits) - 1
            if val == old:
                continue
            D("damaged/setuphdr/%s_%s.ogg" % (tag, nm), st.edit_packet_bits(2, pos, nbits, val),
              "setup header: %s=%d (was %d)" % (field, val, old))
        pk = st.packet_list()
        for frac in (2, 4):
            D("damaged/setuphdr/%s_cut_1_%d.ogg" % (tag, frac), st.remux(pk[:2] + [pk[2][:len(pk[2]) // frac]] + pk[3:]),
              "setup header cut to 1/%d of its length" % frac)
        D("damaged/setuphdr/%s_empty.ogg" % tag, st.remux(pk[:2] + [b""] + pk[3:]), "zero-length setup header")
        D("damaged/setuphdr/%s_swapped_hdrs.ogg" % tag, st.remux([pk[0], pk[2], pk[1]] + pk[3:]),
          "setup header before comment header")
        D("damaged/setuphdr/%s_missing_comment.ogg" % tag,
          mux([pk[0], pk[2]] + pk[3:], [0, 0] + st.granules[3:], st.serial, headers=2),
          "comment header missing")
        D("damaged/setuphdr/%s_from_other.ogg" % tag,
          st.remux(pk[:2] + [(V if st is S else S).pkts[2]["data"]] + pk[3:]),
          "setup header of a different stream")

    # -- audio packet edits through re-paging (granules recomputed)
    pk = S.packet_list()
    mid = len(pk) // 2
    D("damaged/audiopkt/stereo_pkt_half.ogg", S.remux(pk[:mid] + [pk[mid][:len(pk[mid]) // 2]] + pk[mid + 1:]),
      "audio packet %d cut in half" % mid)
    D("damaged/audiopkt/stereo_pkt_1byte.ogg", S.remux(pk[:mid] + [pk[mid][:1]] + pk[mid + 1:]),
      "audio packet %d cut to 1 byte" % mid)
    D("damaged/audiopkt/stereo_pkt_extended.ogg", S.remux(pk[:mid] + [pk[mid] + rng.bytes(500)] + pk[mid + 1:]),
      "audio packet %d with 500 random bytes appended" % mid)
    D("damaged/audiopkt/stereo_pkt_random.ogg", S.remux(pk[:mid] + [bytes([pk[mid][0]]) + rng.bytes(len(pk[mid]) - 1)] + pk[mid + 1:]),
      "audio packet %d body replaced by random bytes" % mid)
    D("damaged/audiopkt/stereo_pkt_huge_random.ogg", S.remux(pk[:mid] + [b"\0" + rng.bytes(200000)] + pk[mid + 1:]),
      "200 kB random audio packet spanning several pages")
    D("damaged/audiopkt/stereo_first_audio_missing.ogg", S.remux(pk[:3] + pk[4:], S.granules[:3] + S.granules[4:]),
      "first audio packet removed")
    D("damaged/audiopkt/stereo_all_audio_random.ogg",
      S.remux(pk[:3] + [bytes([p[0]]) + rng.bytes(max(0, len(p) - 1)) for p in pk[3:]]),
      "every audio packet body random (mode bits kept)")
    D("damaged/audiopkt/stereo_audio_from_mono.ogg",
      mux(pk[:3] + V.packet_list()[3:], S.granules[:3] + V.granules[3:], S.serial),
      "stereo headers followed by mono audio packets")

    # -- chained streams with problems
    la, lb, lc = links["a"], links["b"], links["c"]
    A = Stream(la)
    D("damaged/chain/same_serial.ogg", la + repage(page_bytes(V.data, V.pages), serial=A.serial),
      "two different links sharing one serial number")
    D("damaged/chain/same_link_twice.ogg", la + la, "the same link concatenated twice (same serial)")
    D("damaged/chain/link1_truncated.ogg", la[:len(la) * 2 // 3] + lb, "first link truncated mid-stream, second link follows")
    D("damaged/chain/link1_mid_page_cut.ogg", la[:A.pages[-1].body_off + A.pages[-1].blen // 2] + lb + lc,
      "first link cut inside a page, then two complete links")
    D("damaged/chain/link1_eosless.ogg",
      set_page_field(la, A.pages, len(A.pages) - 1, "<B", 5, A.pages[-1].htype & ~4) + lb,
      "first link without EOS flag, second link follows")
    B2 = Stream(lb)
    D("damaged/chain/link2_bad_id.ogg", la + B2.edit_packet(0, {11: 0}) + lc, "second link's identification header has 0 channels")
    D("damaged/chain/link2_bad_setup.ogg", la + B2.edit_packet_bits(2, *B2.setup["fields"]["cb0.sync"][:2], 0) + lc,
      "second link's setup header has a broken codebook")
    D("damaged/chain/link2_no_headers.ogg", la + b"".join(page_bytes(lb, B2.pages)[2:]) + lc,
      "second link's header pages missing")
    D("damaged/chain/garbage_between_links.ogg", la + rng.bytes(3000) + lb, "3000 random bytes between links")
    D("damaged/chain/link2_granule_minus1.ogg",
      la + set_page_field(lb, B2.pages, len(B2.pages) - 1, "<q", 6, -1) + lc, "second link's last granule -1")
    D("damaged/chain/link2_interleaved_with_link1.ogg",
      b"".join(page_bytes(la, A.pages)[:3]) + b"".join(page_bytes(lb, B2.pages)[:2]) + b"".join(page_bytes(la, A.pages)[3:])
      + b"".join(page_bytes(lb, B2.pages)[2:]), "BOS of a second Vorbis stream inside the first link (multiplexed)")


def gen_odd(c, bases):
    """Valid-but-unusual streams: re-paged layouts, multiplexing, zero-length packets."""
    S = Stream(bases["stereo"])
    V = Stream(bases["viewer"])
    H = Stream(bases["hq"])
    for st, tag in ((S, "stereo"), (V, "viewer"), (H, "hq")):
        for layout in ("seg1", "pkt1", "max"):
            c.add("odd/pages/%s_%s.ogg" % (tag, layout), st.remux(layout=layout),
                  "re-paged: %s layout" % layout, quick=(tag == "stereo" and layout == "seg1"))
        c.add("odd/pages/%s_std_repaged.ogg" % tag, st.remux(), "re-paged with libogg-like 4096-byte pages")
    pk = S.packet_list()
    # zero-length packets between audio packets: no samples, granules unchanged
    zp, zg = pk[:4], S.granules[:4]
    for i in range(4, len(pk)):
        if i % 5 == 0:
            zp.append(b"")
            zg.append(zg[-1])
        zp.append(pk[i])
        zg.append(S.granules[i])
    c.add("odd/packets/stereo_zero_length_packets.ogg", mux(zp, zg, S.serial), "zero-length packets between audio packets")
    # start trimming: first audio page granule smaller than the samples it completes
    first_audio = S.pkts[2]["last_page"] + 1
    g = S.pages[first_audio].granule
    for nm, val in (("trim64", max(0, g - 64)), ("trim1", max(0, g - 1)), ("grow", g + 512)):
        c.add("odd/granule/stereo_first_audio_%s.ogg" % nm, set_page_field(S.data, S.pages, first_audio, "<q", 6, val),
              "first audio page granule %d (computed %d): start-trim / offset semantics" % (val, g))
    # grouped multiplex with a non-Vorbis stream (Skeleton-like BOS first)
    head = make_page(2, 0, 0x5EED, 0, [64], b"fishead\0" + bytes(56))
    tail = make_page(4, 0, 0x5EED, 1, [0], b"")
    sp = page_bytes(S.data, S.pages)
    c.add("odd/mux/skeleton_then_vorbis.ogg", head + sp[0] + tail + b"".join(sp[1:]),
          "non-Vorbis BOS page before the Vorbis BOS (grouped multiplex)", quick=True)
    c.add("odd/mux/vorbis_then_other.ogg", sp[0] + head + b"".join(sp[1:3]) + tail + b"".join(sp[3:]),
          "Vorbis BOS then another stream's BOS, other stream's pages interleaved")
    other = Stream(bases["short"])
    op = page_bytes(other.data, other.pages)
    inter = [sp[0], op[0]] + sp[1:3] + op[1:3]
    rest_s, rest_o = sp[3:], op[3:]
    while rest_s or rest_o:
        if rest_s:
            inter.append(rest_s.pop(0))
        if rest_o:
            inter.append(rest_o.pop(0))
    c.add("odd/mux/two_vorbis_streams.ogg", b"".join(inter), "two Vorbis streams multiplexed (vorbisfile decodes the first)")


def gen_synthetic(c):
    """Streams built from scratch in Python, covering setup shapes libvorbisenc never
    emits: floor 0, residue 0, multiple submaps/mappings/modes, odd block sizes and
    codebook edge cases, plus deliberately broken setups. All deterministic."""

    def std_books():
        # [0]=residue phrasebook (dim1, 16 entries), [1]=stage/floor book, [2]=floor1 class book
        return [mapbook(1, 16, maptype=1), mapbook(1, 8, maptype=1), mapbook(1, 4, maptype=1)]

    def simple_floor1():
        return Floor1([0], [1], [0], [0], [[-1]], 1, 4, [7])

    def simple_residue(rtype=1, begin=0, end=128, grouping=16, partitions=8, groupbook=0, stage=1):
        return Residue(rtype, begin, end, grouping, partitions, groupbook,
                       [stage and 1] * partitions, [stage] * partitions)

    def simple_mapping(channels):
        coup = [(0, 1)] if channels >= 2 else []
        return Mapping(1, coup, [], [0], [0])

    def two_modes():
        return [Mode(0, 0), Mode(1, 0)]

    def emit(rel, stream, seed, npackets=32, desc="", quick=False, lengths=None, headers=None):
        data = stream.build(Rng(seed), npackets, lengths=lengths, headers=headers)
        c.add("synthetic/" + rel, data, desc, quick=quick)

    def S(channels, rate, bs0, bs1, books, floors, residues, mappings, modes, serial, **kw):
        return SyntheticStream(channels, rate, bs0, bs1, books, floors, residues, mappings, modes,
                               serial, **kw)

    # ---- floor 0: orders, barkmaps, ampbits, book counts/dims, maptype 1 and 2
    serial = 0x5A000
    f0cfg = [
        ("order01_bark64_amp0", 1, 8000, 64, 0, 0, 1, 2),
        ("order02_bark128_amp6", 2, 16000, 128, 6, 30, 2, 2),
        ("order15_bark256_amp8", 15, 22050, 256, 8, 60, 4, 3),
        ("order16_bark256_amp8", 16, 44100, 256, 8, 64, 6, 4),
        ("order17_bark384_amp10", 17, 44100, 384, 10, 90, 8, 4),
        ("order31_bark512_amp12", 31, 48000, 512, 12, 120, 16, 8),
        ("order32_bark512_amp12", 32, 48000, 512, 12, 127, 12, 6),
        ("order08_bark100_rate8000", 8, 8000, 100, 6, 50, 3, 5),
    ]
    for name, order, frate, bark, ampbits, ampdB, nbooks, dim in f0cfg:
        f0books = [mapbook(dim, max(2, dim * 2), maptype=(2 if i % 2 else 1)) for i in range(nbooks)]
        books = [mapbook(1, 16, maptype=1), mapbook(1, 8, maptype=1)] + f0books
        fidx = list(range(2, 2 + nbooks))
        ch = 1 if nbooks % 2 else 2
        st = S(ch, 44100, 256, 2048, books, [Floor0(order, frate, bark, ampbits, ampdB, fidx)],
               [simple_residue()], [simple_mapping(ch)], two_modes(), serial)
        emit("floor0/%s.ogg" % name, st, serial, 30,
             "floor 0: %s, %d books dim %d, %d ch" % (name, nbooks, dim, ch),
             quick=(name == "order16_bark256_amp8"))
        serial += 1
    # floor 0 and floor 1 mixed across two submaps of a 2-channel stream
    books = [mapbook(1, 16, maptype=1), mapbook(1, 8, maptype=1), mapbook(1, 4, maptype=1),
             mapbook(4, 8, maptype=2)]
    f0 = Floor0(12, 44100, 256, 8, 60, [3])
    f1 = simple_floor1()
    st = S(2, 44100, 256, 2048, books, [f0, f1], [simple_residue(0), simple_residue(2)],
           [Mapping(2, [], [0, 1], [0, 1], [0, 1])], two_modes(), serial)
    emit("floor0/mixed_floor0_floor1_submaps.ogg", st, serial, 32,
         "two submaps: channel 0 floor 0, channel 1 floor 1", quick=True)
    serial += 1

    # ---- floor 1: partitions, classes with subclasses and books, mult, rangebits, posts
    serial = 0x5B000
    # 0 partitions
    st = S(2, 44100, 256, 2048, std_books(), [Floor1([], [], [], [], [], 2, 4, [])],
           [simple_residue()], [simple_mapping(2)], two_modes(), serial)
    emit("floor1/zero_partitions.ogg", st, serial, 30, "floor 1 with 0 partitions", quick=True)
    serial += 1
    # mult 1..4, rangebits 1..15
    for mult in (1, 2, 3, 4):
        rb = {1: 4, 2: 8, 3: 12, 4: 15}[mult]
        posts = [1 + (i * 7) % ((1 << rb) - 1) for i in range(4)]
        posts = sorted(set(posts))
        while len(posts) < 4:
            posts.append((posts[-1] + 3) % ((1 << rb) - 1) + 1)
        fl = Floor1([0, 0], [2, 2], [0, 0], [0, 0], [[-1], [-1]], mult, rb, posts)
        st = S(2, 44100, 256, 2048, std_books(), [fl], [simple_residue()], [simple_mapping(2)],
               two_modes(), serial)
        emit("floor1/mult%d_range%d.ogg" % (mult, rb), st, serial, 30,
             "floor 1 mult %d rangebits %d" % (mult, rb))
        serial += 1
    # subclasses 1..3 with class books and subbooks
    for subs in (1, 2, 3):
        nsub = 1 << subs
        # a class with subclasses needs a class book of 2^subs entries and subbooks
        cbooks = std_books() + [mapbook(1, nsub, maptype=1)]
        classbook = len(cbooks) - 1
        subbook = [1] * nsub  # all point at book 1 (maptype!=0)
        fl = Floor1([0], [2], [subs], [classbook], [subbook], 2, 6,
                    [3 + i * 5 for i in range(2)])
        st = S(2, 44100, 256, 2048, cbooks, [fl], [simple_residue()], [simple_mapping(2)],
               two_modes(), serial)
        emit("floor1/subclasses%d.ogg" % subs, st, serial, 30,
             "floor 1 class with %d subclass bits" % subs, quick=(subs == 2))
        serial += 1
    # many posts, up to VIF_POSIT = 63. The partitions field is 5 bits (<= 31
    # partitions), so large post counts use classes with dim up to 8.
    for count in (16, 32, 63):
        rb = 7  # 127 usable post values, enough for distinct posts up to 63
        d = min(8, count)
        p, r = count // d, count % d
        partitionclass = [0] * p + ([1] if r else [])
        class_dim = [d] + ([r] if r else [])
        nclasses = len(class_dim)
        uniq = [1 + 2 * i for i in range(count)]  # distinct odd values in [1, 2*count-1] < 128
        fl = Floor1(partitionclass, class_dim, [0] * nclasses, [0] * nclasses,
                    [[-1]] * nclasses, 2, rb, uniq)
        st = S(2, 44100, 256, 2048, std_books(), [fl], [simple_residue()], [simple_mapping(2)],
               two_modes(), serial)
        emit("floor1/posts%02d.ogg" % count, st, serial, 30,
             "floor 1 with %d posts (%d partitions)" % (count, len(partitionclass)),
             quick=(count == 63))
        serial += 1

    # ---- residue: types, begin/end, grouping, partitions, cascades, phrasebook dims
    serial = 0x5C000
    for rt in (0, 1, 2):
        st = S(2, 44100, 256, 2048, std_books(), [simple_floor1()], [simple_residue(rt)],
               [simple_mapping(2)], two_modes(), serial)
        emit("residue/type%d.ogg" % rt, st, serial, 32, "residue type %d" % rt,
             quick=(rt == 2))
        serial += 1
    rvar = [
        ("end_beyond_block", simple_residue(1, 0, 4096, 16, 8)),
        ("begin_gt_end", simple_residue(1, 200, 64, 16, 8)),
        ("grouping1_part64", simple_residue(2, 0, 64, 1, 64)),
        ("grouping64_part2", simple_residue(0, 0, 256, 64, 2)),
        ("part1", simple_residue(1, 0, 128, 16, 1)),
    ]
    for name, rs in rvar:
        # groupbook must have entries >= partitions^dim (dim 1 -> entries >= partitions)
        books = [mapbook(1, max(16, rs.partitions), maptype=1), mapbook(1, 8, maptype=1),
                 mapbook(1, 4, maptype=1)]
        st = S(2, 44100, 256, 2048, books, [simple_floor1()], [rs], [simple_mapping(2)],
               two_modes(), serial)
        emit("residue/%s.ogg" % name, st, serial, 30, "residue %s" % name)
        serial += 1
    # cascades 1..8 stages (secondstages bitmask), with the extra stage books present
    for nstages in (1, 2, 4, 8):
        mask = (1 << nstages) - 1
        nbooks_needed = bin(mask).count("1") * 4
        books = [mapbook(1, 16, maptype=1)] + [mapbook(1, 8, maptype=1) for _ in range(nbooks_needed + 1)]
        parts = 4
        rs = Residue(1, 0, 128, 16, parts, 0, [mask] * parts, [1] * (bin(mask).count("1") * parts))
        st = S(2, 44100, 256, 2048, books, [simple_floor1()], [rs], [simple_mapping(2)],
               two_modes(), serial)
        emit("residue/cascade%d.ogg" % nstages, st, serial, 30,
             "residue cascade with %d stages" % nstages, quick=(nstages == 8))
        serial += 1
    # phrasebook dims 1..4 (groupbook entries >= partitions^dim)
    for dim in (1, 2, 3, 4):
        parts = 3
        entries = parts ** dim
        books = [mapbook(dim, entries, maptype=1), mapbook(1, 8, maptype=1), mapbook(1, 4, maptype=1)]
        rs = Residue(2, 0, 128, 16, parts, 0, [1] * parts, [1] * parts)
        st = S(2, 44100, 256, 2048, books, [simple_floor1()], [rs], [simple_mapping(2)],
               two_modes(), serial)
        emit("residue/phrasebook_dim%d.ogg" % dim, st, serial, 30,
             "residue phrasebook dim %d, %d entries" % (dim, entries))
        serial += 1

    # ---- mappings: submaps, coupling steps, multiple mappings and modes
    serial = 0x5D000
    for nsub in (1, 2, 4, 8, 16):
        floors = [simple_floor1()]
        residues = [simple_residue()]
        chmux = [i % nsub for i in range(2)]
        mp = Mapping(nsub, [(0, 1)], chmux, [0] * nsub, [0] * nsub)
        st = S(2, 44100, 256, 2048, std_books(), floors, residues, [mp], two_modes(), serial)
        emit("mapping/submaps%02d.ogg" % nsub, st, serial, 32,
             "mapping with %d submaps" % nsub, quick=(nsub == 16))
        serial += 1
    # coupling steps for 2..8 channels
    for ch in (2, 3, 4, 6, 8):
        steps = [(2 * i, 2 * i + 1) for i in range(ch // 2)]
        mp = Mapping(1, steps, [], [0], [0])
        st = S(ch, 48000, 256, 2048, std_books(), [simple_floor1()], [simple_residue()], [mp],
               two_modes(), serial)
        emit("mapping/coupling_%dch.ogg" % ch, st, serial, 30,
             "%d channels, %d coupling steps" % (ch, len(steps)), quick=(ch == 6))
        serial += 1
    # multiple mappings and up to 64 modes (modebits 6)
    for nmodes in (3, 8, 64):
        maps = [simple_mapping(2), Mapping(1, [(0, 1)], [], [0], [0])]
        modes = [Mode(i % 2, i % len(maps)) for i in range(nmodes)]
        st = S(2, 44100, 256, 2048, std_books(), [simple_floor1()], [simple_residue()], maps,
               modes, serial)
        emit("mapping/modes%02d.ogg" % nmodes, st, serial, max(40, nmodes),
             "%d modes, %d mappings" % (nmodes, len(maps)), quick=(nmodes == 64))
        serial += 1

    # ---- block sizes and long/short transitions (build() already randomises modes)
    serial = 0x5E000
    for b0, b1 in [(64, 64), (64, 8192), (256, 2048), (8192, 8192), (512, 4096), (128, 1024)]:
        rs = simple_residue(2, 0, min(32, b0 // 2), 16, 8)
        st = S(2, 44100, b0, b1, std_books(), [simple_floor1()], [rs], [simple_mapping(2)],
               two_modes(), serial)
        emit("blocksizes/bs_%d_%d.ogg" % (b0, b1), st, serial, 40,
             "block sizes %d/%d, random long/short order" % (b0, b1),
             quick=((b0, b1) == (64, 8192)))
        serial += 1

    # ---- codebook edge cases
    serial = 0x5F000

    def book_stream(book, name, desc, quick=False, parts=None):
        parts = parts if parts is not None else min(book.entries, 8)
        # use `book` as the residue stage book (index 1); phrasebook/class books are simple
        books = [mapbook(1, max(16, parts), maptype=1), book, mapbook(1, 4, maptype=1)]
        rs = Residue(1, 0, 128, 16, parts, 0, [1] * parts, [1] * parts)
        st = S(2, 44100, 256, 2048, books, [simple_floor1()], [rs], [simple_mapping(2)],
               two_modes(), serial)
        emit("codebooks/%s.ogg" % name, st, serial, 32, desc, quick=quick)

    book_stream(Codebook(1, full_tree_lengths(32), maptype=1), "unordered_populated_32",
                "unordered codebook, 32 entries, fully populated", quick=True)
    book_stream(Codebook(1, [2, 0, 2, 0, 1, 0, 0, 0], maptype=1), "sparse_unused",
                "unordered codebook with unused (length-0) entries")
    serial += 1
    book_stream(Codebook(1, [4] * 16, maptype=1, ordered=True), "ordered_uniform16",
                "ordered codebook, 16 entries all length 4")
    serial += 1
    book_stream(Codebook(1, sorted(full_tree_lengths(10)), maptype=1, ordered=True), "ordered_nonuniform",
                "ordered codebook, nonuniform lengths")
    serial += 1
    book_stream(Codebook(1, [1], maptype=1), "single_entry", "single-entry book (one length-1 codeword)")
    serial += 1
    book_stream(Codebook(1, ladder_lengths(31), maptype=1), "maxlen31",
                "codebook with 31-bit codewords", quick=True)
    serial += 1
    book_stream(Codebook(1, ladder_lengths(32), maptype=1), "maxlen32",
                "codebook with 32-bit codewords (oggpack_look sign edge)", quick=True)
    serial += 1
    book_stream(Codebook(2, full_tree_lengths(16), maptype=1, q_sequencep=0, q_min=vorbis_float32_pack(-2.5),
                         q_delta=vorbis_float32_pack(0.25)), "maptype1_seq0_negmin",
                "maptype 1, q_sequencep 0, negative q_min", parts=4)
    serial += 1
    book_stream(Codebook(2, full_tree_lengths(16), maptype=1, q_sequencep=1,
                         q_min=vorbis_float32_pack(1e6), q_delta=vorbis_float32_pack(1e3), q_quant=12),
                "maptype1_seq1_largeexp", "maptype 1, q_sequencep 1, large exponents, 12-bit quant", parts=4)
    serial += 1
    book_stream(Codebook(4, full_tree_lengths(64), maptype=2, q_quant=4), "maptype2_dim4",
                "maptype 2 codebook, dim 4, 64 entries", parts=8)
    serial += 1
    book_stream(Codebook(8, full_tree_lengths(256), maptype=1, q_quant=6), "maptype1_dim8_256",
                "maptype 1 codebook, dim 8, 256 entries", parts=8)
    serial += 1
    book_stream(Codebook(16, full_tree_lengths(4096), maptype=1, q_quant=4), "dim16_4096",
                "codebook dim 16, 4096 entries", parts=8)
    serial += 1

    # ---- channel counts 1..8 and 32
    serial = 0x5FA00
    for ch in (1, 2, 3, 4, 5, 6, 7, 8, 32):
        steps = [(2 * i, 2 * i + 1) for i in range(ch // 2)] if ch >= 2 else []
        mp = Mapping(1, steps, [], [0], [0])
        rate = {1: 8000, 2: 44100, 3: 48000, 32: 96000}.get(ch, 44100)
        st = S(ch, rate, 256, 2048, std_books(), [simple_floor1()], [simple_residue()], [mp],
               two_modes(), serial)
        emit("channels/c%02d.ogg" % ch, st, serial, 24,
             "%d channels at %d Hz" % (ch, rate), quick=(ch in (1, 32)))
        serial += 1

    # ---- deliberately broken setups (each violates one rule; some crash the C decoder,
    #      which the oracle/harness tolerates). They must not decode as normal streams.
    serial = 0x5FB00
    broken = []

    def add_broken(name, stream, desc, npk=20, headers=None):
        broken.append((name, stream, desc, npk, headers))

    gb3 = std_books
    add_broken("codebook_overpopulated",
               S(2, 44100, 256, 2048, [mapbook(1, 16, maptype=1), Codebook(1, [1, 1, 1, 1], maptype=1),
                                       mapbook(1, 4, maptype=1)], [simple_floor1()], [simple_residue()],
                 [simple_mapping(2)], two_modes(), serial),
               "overpopulated Huffman tree")
    add_broken("codebook_underpopulated",
               S(2, 44100, 256, 2048, [mapbook(1, 16, maptype=1), Codebook(1, [2, 2], maptype=1),
                                       mapbook(1, 4, maptype=1)], [simple_floor1()], [simple_residue()],
                 [simple_mapping(2)], two_modes(), serial),
               "underpopulated Huffman tree")
    add_broken("codebook_bad_sync",
               S(2, 44100, 256, 2048, [mapbook(1, 16, maptype=1), _BadSyncBook(1, [1, 1], maptype=1),
                                       mapbook(1, 4, maptype=1)], [simple_floor1()], [simple_residue()],
                 [simple_mapping(2)], two_modes(), serial),
               "codebook sync pattern wrong")
    add_broken("codebook_dim0_groupbook",
               S(2, 44100, 256, 2048, [Codebook(0, [1, 1], maptype=1), mapbook(1, 8, maptype=1),
                                       mapbook(1, 4, maptype=1)], [simple_floor1()], [simple_residue()],
                 [simple_mapping(2)], two_modes(), serial),
               "dim-0 maptype-1 book used as residue phrasebook")
    add_broken("residue_partvals_over_entries",
               S(2, 44100, 256, 2048, [mapbook(2, 16, maptype=1), mapbook(1, 8, maptype=1),
                                       mapbook(1, 4, maptype=1)], [simple_floor1()],
                 [simple_residue(1, partitions=8, groupbook=0)], [simple_mapping(2)], two_modes(), serial),
               "residue partvals (partitions^dim) exceed phrasebook entries")
    add_broken("residue_type3",
               S(2, 44100, 256, 2048, std_books(), [simple_floor1()],
                 [Residue(3, 0, 128, 16, 8, 0, [1] * 8, [1] * 8)], [simple_mapping(2)], two_modes(), serial),
               "residue type 3 (out of range)")
    add_broken("floor1_duplicate_posts",
               S(2, 44100, 256, 2048, std_books(), [Floor1([0], [2], [0], [0], [[-1]], 1, 4, [7, 7])],
                 [simple_residue()], [simple_mapping(2)], two_modes(), serial),
               "floor 1 duplicate post values")
    add_broken("floor_type2",
               S(2, 44100, 256, 2048, std_books(), [_FloorType2()], [simple_residue()],
                 [simple_mapping(2)], two_modes(), serial),
               "floor type 2 (out of range)")
    add_broken("floor0_book_oob",
               S(1, 44100, 256, 2048, [mapbook(1, 16, maptype=1), mapbook(1, 8, maptype=1),
                                       mapbook(1, 4, maptype=1)], [Floor0(16, 8000, 256, 8, 60, [9])],
                 [simple_residue()], [Mapping(1, [], [], [0], [0])], two_modes(), serial),
               "floor 0 references a book index past the book count")
    add_broken("floor0_maptype0_book",
               S(1, 44100, 256, 2048, [Codebook(1, [1], maptype=0), mapbook(1, 8, maptype=1),
                                       mapbook(1, 4, maptype=1)], [Floor0(16, 8000, 256, 8, 60, [0])],
                 [simple_residue(groupbook=1)], [Mapping(1, [], [], [0], [0])], two_modes(), serial),
               "floor 0 references a maptype-0 book")
    add_broken("mapping_coupling_mag_eq_ang",
               S(2, 44100, 256, 2048, std_books(), [simple_floor1()], [simple_residue()],
                 [Mapping(1, [(1, 1)], [], [0], [0])], two_modes(), serial),
               "mapping coupling magnitude == angle")
    add_broken("mapping_floorsubmap_oob",
               S(2, 44100, 256, 2048, std_books(), [simple_floor1()], [simple_residue()],
                 [Mapping(1, [(0, 1)], [], [5], [0])], two_modes(), serial),
               "mapping floor submap index out of range")
    add_broken("mapping_residuesubmap_oob",
               S(2, 44100, 256, 2048, std_books(), [simple_floor1()], [simple_residue()],
                 [Mapping(1, [(0, 1)], [], [0], [7])], two_modes(), serial),
               "mapping residue submap index out of range")
    add_broken("mode_mapping_oob",
               S(2, 44100, 256, 2048, std_books(), [simple_floor1()], [simple_residue()],
                 [simple_mapping(2)], [Mode(0, 5), Mode(1, 0)], serial),
               "mode references a mapping index past the mapping count")
    add_broken("mode_windowtype_nonzero",
               S(2, 44100, 256, 2048, std_books(), [simple_floor1()], [simple_residue()],
                 [simple_mapping(2)], [_ModeBadWindow(0, 0), Mode(1, 0)], serial),
               "mode windowtype != 0")
    add_broken("info_channels0",
               S(0, 44100, 256, 2048, std_books(), [simple_floor1()], [simple_residue()],
                 [simple_mapping(2)], two_modes(), serial),
               "identification header channels == 0")
    add_broken("info_bs1_lt_bs0",
               S(2, 44100, 2048, 256, std_books(), [simple_floor1()], [simple_residue()],
                 [simple_mapping(2)], two_modes(), serial),
               "block size 1 smaller than block size 0")
    add_broken("info_bs_too_small",
               S(2, 44100, 32, 2048, std_books(), [simple_floor1()], [simple_residue()],
                 [simple_mapping(2)], two_modes(), serial),
               "block size 0 below 64")
    add_broken("info_bs_too_large",
               S(2, 44100, 256, 16384, std_books(), [simple_floor1()], [simple_residue()],
                 [simple_mapping(2)], two_modes(), serial),
               "block size 1 above 8192")
    add_broken("codebook_ilog_overflow",
               None, "codebook ilog(dim)+ilog(entries) > 24")  # built below by hand
    add_broken("floor1_classbook_oob",
               S(2, 44100, 256, 2048, std_books(), [Floor1([0], [1], [1], [9], [[1, 1]], 1, 4, [7])],
                 [simple_residue()], [simple_mapping(2)], two_modes(), serial),
               "floor 1 class book index out of range")
    add_broken("residue_groupbook_oob",
               S(2, 44100, 256, 2048, std_books(), [simple_floor1()],
                 [Residue(1, 0, 128, 16, 8, 99, [1] * 8, [1] * 8)], [simple_mapping(2)], two_modes(), serial),
               "residue groupbook index out of range")
    add_broken("residue_stagebook_maptype0",
               S(2, 44100, 256, 2048, [mapbook(1, 16, maptype=1), Codebook(1, [1], maptype=0),
                                       mapbook(1, 4, maptype=1)], [simple_floor1()],
                 [Residue(1, 0, 128, 16, 8, 0, [1] * 8, [1] * 8)], [simple_mapping(2)], two_modes(), serial),
               "residue stage book has maptype 0")
    add_broken("time_type_nonzero", None, "time backend type != 0")
    add_broken("floor_count_mismatch",
               S(2, 44100, 256, 2048, std_books(), [simple_floor1()], [simple_residue()],
                 [Mapping(1, [(0, 1)], [], [1], [0])], two_modes(), serial),
               "mapping floor submap references a nonexistent second floor")

    for i, (name, st, desc, npk, hdr) in enumerate(broken):
        if name == "codebook_ilog_overflow":
            # dim and entries each need few ilog bits individually but overflow together
            data = _ilog_overflow_stream(serial + i)
        elif name == "time_type_nonzero":
            data = _time_type_stream(serial + i)
        else:
            data = st.build(Rng(0xB00 + i), npk, headers=hdr)
        c.add("synthetic/broken/%s.ogg" % name, data, "BROKEN: " + desc,
              quick=(name in ("codebook_overpopulated", "residue_partvals_over_entries",
                              "mapping_coupling_mag_eq_ang", "info_channels0", "floor1_duplicate_posts")))
    return len(broken)


class _BadSyncBook(Codebook):
    def pack(self, bw):
        bw.write(0x000000, 24)  # wrong sync (should be 0x564342)
        bw.write(self.dim, 16)
        bw.write(self.entries, 24)
        bw.write(0, 1)
        bw.write(0, 1)
        for x in self.lengthlist:
            bw.write(x - 1, 5)
        bw.write(self.maptype, 4)
        if self.maptype in (1, 2):
            bw.write(self.q_min, 32)
            bw.write(self.q_delta, 32)
            bw.write(self.q_quant - 1, 4)
            bw.write(self.q_sequencep, 1)
            for _ in range(self.quantvals()):
                bw.write(1, self.q_quant)


class _FloorType2:
    type = 2

    def pack(self, bw):
        bw.write(0, 8)


class _ModeBadWindow(Mode):
    def pack(self, bw):
        bw.write(self.blockflag, 1)
        bw.write(1, 16)  # windowtype != 0
        bw.write(0, 16)
        bw.write(self.mapping, 8)


def _time_type_stream(serial):
    """A setup header whose time backend type is 1 (only 0 is legal: VI_TIMEB == 1)."""
    bw = BitWriter()
    for b in b"\x05vorbis":
        bw.write(b, 8)
    bw.write(0, 8)  # 1 codebook
    mapbook(1, 8, maptype=1).pack(bw)
    bw.write(0, 6)   # 1 time backend
    bw.write(1, 16)  # time type 1 -> rejected (>= VI_TIMEB)
    bw.write(0, 6)
    bw.write(1, 16)  # floor type 1 (never reached)
    bw.write(1, 1)
    setup = bw.data()
    idp = bytearray(b"\x01vorbis") + struct.pack("<IBI", 0, 2, 44100) + struct.pack("<iii", 0, 0, 0) \
        + bytes([blocksize_log2(256) | (blocksize_log2(2048) << 4), 1])
    com = bytearray(b"\x03vorbis") + struct.pack("<I", 0) + struct.pack("<I", 0) + bytes([1])
    packets = [bytes(idp), bytes(com), setup] + [bytes([0, 0, 0]) for _ in range(5)]
    granules = [0, 0, 0, 0, 512, 1024, 1536, 2048]
    return mux(packets, granules, serial, "std", headers=3)


def _ilog_overflow_stream(serial):
    """A setup header whose first codebook has ilog(dim)+ilog(entries) > 24, which
    vorbis_staticbook_unpack rejects. Built by hand so the Codebook guard is bypassed."""
    bw = BitWriter()
    for b in b"\x05vorbis":
        bw.write(b, 8)
    bw.write(0, 8)  # 1 codebook
    bw.write(0x564342, 24)
    bw.write(0xFFFF, 16)   # dim: ilog 16
    bw.write(0x1FF, 24)    # entries: ilog 9 -> 16+9 = 25 > 24
    bw.write(0, 1)
    bw.write(0, 1)
    for _ in range(0x1FF):
        bw.write(0, 5)
    bw.write(0, 4)
    bw.write(1, 1)  # framing (never reached)
    setup = bw.data()
    idp = bytearray(b"\x01vorbis") + struct.pack("<IBI", 0, 2, 44100) + struct.pack("<iii", 0, 0, 0) \
        + bytes([blocksize_log2(256) | (blocksize_log2(2048) << 4), 1])
    com = bytearray(b"\x03vorbis") + struct.pack("<I", 0) + struct.pack("<I", 0) + bytes([1])
    packets = [bytes(idp), bytes(com), setup] + [bytes([0, 0, 0]) for _ in range(5)]
    granules = [0, 0, 0, 0, 512, 1024, 1536, 2048]
    return mux(packets, granules, serial, "std", headers=3)


def inspect(path):
    with open(path, "rb") as fh:
        data = fh.read()
    pages = parse_pages(data, strict=False)
    print("%s: %d bytes, %d pages" % (path, len(data), len(pages)))
    bad = sum(1 for p in pages if not page_crc_ok(data, p))
    serials = []
    for p in pages:
        if p.serial not in serials:
            serials.append(p.serial)
    print("  serials %s, bad CRC pages %d" % (",".join("%08x" % s for s in serials), bad))
    for s in serials:
        pk = packets_of(data, pages, s)
        if len(pk) < 3 or pk[0]["data"][:7] != b"\x01vorbis":
            print("  serial %08x: not a Vorbis stream" % s)
            continue
        # a chain can reuse serials only illegally; walk each BOS of this serial
        try:
            idh = parse_id(pk[0]["data"])
            st = parse_setup(pk[2]["data"], idh["channels"])
            print("  serial %08x: %d ch %d Hz blocks %d/%d, %d codebooks, floors %s, residues %d, mappings %d, modes %s"
                  % (s, idh["channels"], idh["rate"], idh["bs0"], idh["bs1"], st["codebooks"], st["floor_types"],
                     st["residues"], st["mappings"], st["modes"]))
        except (ValueError, EOFError, IndexError) as e:
            print("  serial %08x: header parse failed: %s" % (s, e))


def main(argv):
    if len(argv) >= 2 and argv[1] == "--inspect":
        for p in argv[2:]:
            inspect(p)
        return 0
    args = [a for a in argv[1:] if not a.startswith("--")]
    quick = "--quick" in argv[1:]
    if len(args) != 2 or any(a not in ("--quick",) for a in argv[1:] if a.startswith("--")):
        print(__doc__)
        return 2
    exe, out = args
    os.makedirs(out, exist_ok=True)
    c = Corpus(os.path.abspath(exe), out, quick)
    bases, links = gen_clean(c)
    gen_odd(c, bases)
    gen_damaged(c, bases, links)
    gen_synthetic(c)
    n = c.finish()
    for rel, why in c.skipped:
        print("skipped (encoder refused): %s: %s" % (rel, why))
    total = sum(v[0] for v in c.entries.values())
    print("%d files, %d bytes, manifest at %s" % (n, total, os.path.join(out, "manifest.txt")))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
