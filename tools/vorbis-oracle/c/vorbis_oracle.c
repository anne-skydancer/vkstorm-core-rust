/*
 * vorbis_oracle: the C reference for vkstorm-vorbis. Decodes Ogg Vorbis
 * files with the viewer's libogg 1.3.5 / libvorbis 1.3.7 / libvorbisfile
 * (the prebuilt secondlife 3p-ogg_vorbis package on Windows, the same
 * sources built with the package's Linux flags on Linux) and writes text
 * traces that the Rust harness (tools/vorbis-oracle/src/main.rs) reproduces
 * line for line.
 *
 * usage:
 *   vorbis_oracle file <in.ogg> <trace> [--keep-going] [--word W] [--be] [--unsigned] [--len L] [--pcm out.raw]
 *       llaudiodecodemgr.cpp's decode: ov_open_callbacks over the file in
 *       memory with the viewer's LLFileSystem seek/tell semantics,
 *       ov_streams, ov_pcm_total, ov_info, ov_comment, ov_serialnumber, then
 *       ov_read(vf, buf, 4096, 0, 2, 1, &section) until it returns 0 or < 0
 *       (--keep-going: continue after OV_HOLE). The trace has the return of
 *       every call (run-length coded) and the FNV-1a 64 hash of the PCM.
 *   vorbis_oracle packets <in.ogg> <trace> [--chunk N]
 *       the packet API as a stream player uses it: ogg_sync in N-byte chunks
 *       (default 4096), a new logical stream at every BOS page,
 *       vorbis_synthesis_headerin for three headers, vorbis_synthesis +
 *       vorbis_synthesis_blockin per packet, vorbis_synthesis_pcmout/_read;
 *       the trace hashes the float samples bit for bit.
 *   vorbis_oracle tables <out>
 *       the MDCT trig tables of mdct_init (sizes 64..8192) and the window
 *       tables of _vorbis_window_get, as float bit patterns.
 *   vorbis_oracle bench <listfile> <reps>
 *       decodes every listed file (the viewer's ov_read loop) reps times,
 *       interleaved, and prints the minimum time per file and in total.
 *
 * Build (Windows, VS 2022 x64 prompt, P = <viewer>\build-vc170-64\packages):
 *   cl /nologo /O2 /MD /I%P%\include vorbis_oracle.c /Fe:vorbis_oracle.exe
 *      /link /LIBPATH:%P%\lib\release libvorbisfile.lib libvorbis.lib libogg.lib
 * Build (Linux, L = prefix of the libogg/libvorbis build):
 *   gcc -O2 -I$L/include vorbis_oracle.c -o vorbis_oracle
 *      $L/lib/release/libvorbisfile.a $L/lib/release/libvorbis.a $L/lib/release/libogg.a -lm
 */
#include <vorbis/codec.h>
#include <vorbis/vorbisfile.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <windows.h>
static double now_s(void) {
    LARGE_INTEGER f, c;
    QueryPerformanceFrequency(&f);
    QueryPerformanceCounter(&c);
    return (double)c.QuadPart / (double)f.QuadPart;
}
#else
#include <time.h>
static double now_s(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec + ts.tv_nsec * 1e-9;
}
#endif

/* libvorbis internals that are exported from the static library */
typedef struct {
    int n;
    int log2n;
    float* trig;
    int* bitrev;
    float scale;
} mdct_lookup;
extern void mdct_init(mdct_lookup* lookup, int n);
extern void mdct_clear(mdct_lookup* l);
extern const float* _vorbis_window_get(int n);

/* ------------------------------------------------------------ utilities */

static unsigned char* load_file(const char* path, long* len) {
    FILE* f = fopen(path, "rb");
    unsigned char* buf;
    if (!f) return NULL;
    fseek(f, 0, SEEK_END);
    *len = ftell(f);
    fseek(f, 0, SEEK_SET);
    buf = (unsigned char*)malloc(*len > 0 ? *len : 1);
    if (*len > 0 && fread(buf, 1, *len, f) != (size_t)*len) {
        fclose(f);
        free(buf);
        return NULL;
    }
    fclose(f);
    return buf;
}

typedef struct { uint64_t h; uint64_t n; } fnv_t;
static void fnv_init(fnv_t* f) { f->h = 0xcbf29ce484222325ull; f->n = 0; }
static void fnv_bytes(fnv_t* f, const unsigned char* p, size_t n) {
    size_t i;
    for (i = 0; i < n; i++) {
        f->h ^= p[i];
        f->h *= 0x100000001b3ull;
    }
    f->n += n;
}

static void put_hex(FILE* o, const unsigned char* p, size_t n) {
    size_t i;
    for (i = 0; i < n; i++) fprintf(o, "%02x", p[i]);
}

/* run-length coded "<tag> <a> <b>" lines */
typedef struct { FILE* o; const char* tag; long a, b; long count; } rle_t;
static void rle_flush(rle_t* r) {
    if (r->count) fprintf(r->o, "%s %ld %ld x%ld\n", r->tag, r->a, r->b, r->count);
    r->count = 0;
}
static void rle_add(rle_t* r, const char* tag, long a, long b) {
    if (r->count && r->tag == tag && r->a == a && r->b == b) {
        r->count++;
        return;
    }
    rle_flush(r);
    r->tag = tag;
    r->a = a;
    r->b = b;
    r->count = 1;
}

/* ------------------------------------------- the viewer's file callbacks */

/* LLFileSystem over a memory buffer: cache_read/cache_seek/cache_tell of
 * llaudiodecodemgr.cpp with LLFileSystem::read/seek/tell semantics. */
typedef struct { const unsigned char* data; int32_t size; int32_t pos; } memsrc_t;

static size_t mem_read(void* ptr, size_t size, size_t nmemb, void* ds) {
    memsrc_t* m = (memsrc_t*)ds;
    int32_t want = (int32_t)(size * nmemb);
    int32_t avail, n;
    if (m->pos >= m->size || m->pos < 0) return 0;
    avail = m->size - m->pos;
    n = want < avail ? want : avail;
    if (n <= 0) return 0;
    memcpy(ptr, m->data + m->pos, n);
    m->pos += n;
    return n / size;
}

static int mem_seek(void* ds, ogg_int64_t offset, int whence) {
    memsrc_t* m = (memsrc_t*)ds;
    int32_t origin, new_pos;
    if (offset > INT32_MAX) return -1;
    switch (whence) {
    case SEEK_SET: origin = 0; break;
    case SEEK_END: origin = m->size; break;
    case SEEK_CUR: origin = m->pos; break;
    default: return -1;
    }
    new_pos = (int32_t)((uint32_t)origin + (uint32_t)(int32_t)offset);
    if (new_pos > m->size) { m->pos = m->size; return -1; }
    if (new_pos < 0) { m->pos = 0; return -1; }
    m->pos = new_pos;
    return 0;
}

static long mem_tell(void* ds) { return ((memsrc_t*)ds)->pos; }
static int mem_close(void* ds) { (void)ds; return 0; }

static void trace_link(FILE* o, OggVorbis_File* vf, int link) {
    vorbis_info* vi = ov_info(vf, link);
    vorbis_comment* vc = ov_comment(vf, link);
    int i;
    fprintf(o, "pcm_total %d %lld\n", link, (long long)ov_pcm_total(vf, link));
    if (vi)
        fprintf(o, "info %d %d %d %ld %ld %ld %ld\n", link, vi->version, vi->channels, vi->rate, vi->bitrate_upper,
                vi->bitrate_nominal, vi->bitrate_lower);
    else
        fprintf(o, "info %d none\n", link);
    fprintf(o, "serial %d %ld\n", link, ov_serialnumber(vf, link));
    if (vc) {
        fprintf(o, "vendor %d ", link);
        if (vc->vendor) put_hex(o, (const unsigned char*)vc->vendor, strlen(vc->vendor));
        else fprintf(o, "none");
        fprintf(o, "\ncomments %d %d\n", link, vc->comments);
        for (i = 0; i < vc->comments; i++) {
            fprintf(o, "comment %d %d ", link, i);
            put_hex(o, (const unsigned char*)vc->user_comments[i], vc->comment_lengths[i]);
            fprintf(o, "\n");
        }
    } else {
        fprintf(o, "vendor %d none\ncomments %d 0\n", link, link);
    }
}

typedef struct {
    int keep_going;
    int word;
    int bigendian;
    int sgned;
    int len;
    const char* pcm_path;
} file_opts;

/* decodes like LLVorbisDecodeState::initDecode + decodeSection */
static int run_file(const unsigned char* data, long len, FILE* o, const file_opts* opt, FILE* pcmf) {
    OggVorbis_File vf;
    ov_callbacks cb;
    memsrc_t m;
    int r, link, section = 0;
    char buf[65536];
    fnv_t h;
    rle_t rl = { o, NULL, 0, 0, 0 };
    long calls = 0;

    m.data = data;
    m.size = (int32_t)len;
    m.pos = 0;
    cb.read_func = mem_read;
    cb.seek_func = mem_seek;
    cb.close_func = mem_close;
    cb.tell_func = mem_tell;
    memset(&vf, 0, sizeof(vf));
    r = ov_open_callbacks(&m, &vf, NULL, 0, cb);
    fprintf(o, "open %d\n", r);
    if (r < 0) return 0;

    fprintf(o, "streams %ld\n", ov_streams(&vf));
    trace_link(o, &vf, -1);
    for (link = 0; link < ov_streams(&vf); link++) trace_link(o, &vf, link);

    fnv_init(&h);
    for (;;) {
        long ret = ov_read(&vf, buf, opt->len, opt->bigendian, opt->word, opt->sgned, &section);
        calls++;
        rle_add(&rl, "read", ret, section);
        if (ret > 0) {
            fnv_bytes(&h, (const unsigned char*)buf, (size_t)ret);
            if (pcmf) fwrite(buf, 1, ret, pcmf);
            continue;
        }
        if (ret == 0) break;
        if (!opt->keep_going || ret != OV_HOLE) break;
        if (calls > 10000000) break;
    }
    rle_flush(&rl);
    fprintf(o, "pcm_tell %lld\n", (long long)ov_pcm_tell(&vf));
    fprintf(o, "pcm %llu %016llx\n", (unsigned long long)h.n, (unsigned long long)h.h);
    ov_clear(&vf);
    return 0;
}

/* --------------------------------------------------------- packet API */

static void run_packets(const unsigned char* data, long len, FILE* o, long chunk) {
    /* debugging aid: VORBIS_ORACLE_DUMP=<file> receives the raw samples */
    FILE* dump = getenv("VORBIS_ORACLE_DUMP") ? fopen(getenv("VORBIS_ORACLE_DUMP"), "wb") : NULL;
    ogg_sync_state oy;
    ogg_stream_state os;
    ogg_page og;
    ogg_packet op;
    vorbis_info vi;
    vorbis_comment vc;
    vorbis_dsp_state vd;
    vorbis_block vb;
    int have = 0, state = 0, nheaders = 0;
    long pos = 0;
    fnv_t h;
    rle_t rl = { o, NULL, 0, 0, 0 };
    uint64_t samples = 0;

    fnv_init(&h);
    ogg_sync_init(&oy);
    for (;;) {
        int r;
        if (pos < len) {
            long n = len - pos < chunk ? len - pos : chunk;
            char* b = ogg_sync_buffer(&oy, n);
            memcpy(b, data + pos, n);
            ogg_sync_wrote(&oy, n);
            pos += n;
        }
        while ((r = ogg_sync_pageout(&oy, &og)) != 0) {
            if (r < 0) {
                rle_add(&rl, "sync", r, 0);
                continue;
            }
            if (ogg_page_bos(&og)) {
                rle_flush(&rl);
                if (have) {
                    if (state == 2) {
                        vorbis_block_clear(&vb);
                        vorbis_dsp_clear(&vd);
                    }
                    vorbis_comment_clear(&vc);
                    vorbis_info_clear(&vi);
                    ogg_stream_clear(&os);
                }
                fprintf(o, "bos %d\n", ogg_page_serialno(&og));
                ogg_stream_init(&os, ogg_page_serialno(&og));
                vorbis_info_init(&vi);
                vorbis_comment_init(&vc);
                have = 1;
                state = 1;
                nheaders = 0;
            }
            if (!have) continue;
            if (ogg_stream_pagein(&os, &og) < 0) continue;
            while ((r = ogg_stream_packetout(&os, &op)) != 0) {
                if (r < 0) {
                    rle_add(&rl, "hole", 0, 0);
                    continue;
                }
                if (state == 1) {
                    int hr = vorbis_synthesis_headerin(&vi, &vc, &op);
                    rle_add(&rl, "hdr", hr, 0);
                    if (hr < 0) {
                        state = 3;
                        continue;
                    }
                    if (++nheaders == 3) {
                        rle_flush(&rl);
                        if (vorbis_synthesis_init(&vd, &vi)) {
                            fprintf(o, "init fail\n");
                            state = 3;
                        } else {
                            vorbis_block_init(&vd, &vb);
                            fprintf(o, "init %d %ld\n", vi.channels, vi.rate);
                            state = 2;
                        }
                    }
                    continue;
                }
                if (state == 2) {
                    int s = vorbis_synthesis(&vb, &op);
                    float** pcm;
                    int n, total = 0, i, c;
                    if (s == 0) s = vorbis_synthesis_blockin(&vd, &vb);
                    while ((n = vorbis_synthesis_pcmout(&vd, &pcm)) > 0) {
                        for (i = 0; i < n; i++)
                            for (c = 0; c < vi.channels; c++) {
                                /* NaN samples hash as one canonical NaN: their sign
                                   bit is not reproducible (see the crate docs) */
                                float v = pcm[c][i];
                                if (v != v) {
                                    uint32_t q = 0x7fc00000u;
                                    memcpy(&v, &q, 4);
                                }
                                fnv_bytes(&h, (const unsigned char*)&v, 4);
                                if (dump) fwrite(&pcm[c][i], 4, 1, dump);
                            }
                        vorbis_synthesis_read(&vd, n);
                        total += n;
                    }
                    samples += total;
                    rle_add(&rl, "pkt", s, total);
                }
            }
        }
        if (pos >= len) break;
    }
    rle_flush(&rl);
    if (have) {
        if (state == 2) {
            vorbis_block_clear(&vb);
            vorbis_dsp_clear(&vd);
        }
        vorbis_comment_clear(&vc);
        vorbis_info_clear(&vi);
        ogg_stream_clear(&os);
    }
    ogg_sync_clear(&oy);
    fprintf(o, "samples %llu %016llx\n", (unsigned long long)samples, (unsigned long long)h.h);
}

/* ------------------------------------------------------------- tables */

static void run_tables(FILE* o) {
    int n, i;
    for (n = 64; n <= 8192; n *= 2) {
        mdct_lookup l;
        memset(&l, 0, sizeof(l));
        mdct_init(&l, n);
        fprintf(o, "mdct %d", n);
        for (i = 0; i < n + n / 4; i++) {
            uint32_t b;
            memcpy(&b, &l.trig[i], 4);
            fprintf(o, " %08x", b);
        }
        fprintf(o, "\n");
        mdct_clear(&l);
    }
    for (n = 0; n < 8; n++) {
        const float* w = _vorbis_window_get(n);
        fprintf(o, "window %d", n);
        for (i = 0; i < (32 << n); i++) {
            uint32_t b;
            memcpy(&b, &w[i], 4);
            fprintf(o, " %08x", b);
        }
        fprintf(o, "\n");
    }
}

/* -------------------------------------------------------------- bench */

static long decode_all(const unsigned char* data, long len) {
    OggVorbis_File vf;
    ov_callbacks cb;
    memsrc_t m;
    char buf[4096];
    int section = 0;
    long total = 0;
    m.data = data;
    m.size = (int32_t)len;
    m.pos = 0;
    cb.read_func = mem_read;
    cb.seek_func = mem_seek;
    cb.close_func = mem_close;
    cb.tell_func = mem_tell;
    if (ov_open_callbacks(&m, &vf, NULL, 0, cb) < 0) return -1;
    for (;;) {
        long r = ov_read(&vf, buf, sizeof(buf), 0, 2, 1, &section);
        if (r <= 0) break;
        total += r;
    }
    ov_clear(&vf);
    return total;
}

static int run_bench(const char* listfile, int reps) {
    FILE* lf = fopen(listfile, "r");
    char line[4096];
    int nfiles = 0, cap = 64, i, r;
    char** names = malloc(cap * sizeof(char*));
    unsigned char** datas = malloc(cap * sizeof(unsigned char*));
    long* lens = malloc(cap * sizeof(long));
    double* best;
    double best_total = 1e30;
    if (!lf) return 1;
    while (fgets(line, sizeof(line), lf)) {
        size_t l = strlen(line);
        while (l && (line[l - 1] == '\n' || line[l - 1] == '\r')) line[--l] = 0;
        if (!l) continue;
        if (nfiles == cap) {
            cap *= 2;
            names = realloc(names, cap * sizeof(char*));
            datas = realloc(datas, cap * sizeof(unsigned char*));
            lens = realloc(lens, cap * sizeof(long));
        }
        names[nfiles] = strdup(line);
        datas[nfiles] = load_file(line, &lens[nfiles]);
        if (!datas[nfiles]) {
            fprintf(stderr, "cannot read %s\n", line);
            return 1;
        }
        nfiles++;
    }
    fclose(lf);
    best = malloc(nfiles * sizeof(double));
    for (i = 0; i < nfiles; i++) best[i] = 1e30;
    for (r = 0; r < reps; r++) {
        double total = 0;
        for (i = 0; i < nfiles; i++) {
            double t0 = now_s(), dt;
            long n = decode_all(datas[i], lens[i]);
            dt = now_s() - t0;
            if (n < -1) return 1;
            if (dt < best[i]) best[i] = dt;
            total += dt;
        }
        if (total < best_total) best_total = total;
    }
    for (i = 0; i < nfiles; i++) printf("file %.9f %s\n", best[i], names[i]);
    printf("total %.9f\n", best_total);
    return 0;
}

/* --------------------------------------------------------------- main */

int main(int argc, char** argv) {
    if (argc >= 4 && !strcmp(argv[1], "file")) {
        file_opts opt = { 0, 2, 0, 1, 4096, NULL };
        long len;
        unsigned char* data;
        FILE *o, *pcmf = NULL;
        int i;
        for (i = 4; i < argc; i++) {
            if (!strcmp(argv[i], "--keep-going")) opt.keep_going = 1;
            else if (!strcmp(argv[i], "--word") && i + 1 < argc) opt.word = atoi(argv[++i]);
            else if (!strcmp(argv[i], "--be")) opt.bigendian = 1;
            else if (!strcmp(argv[i], "--unsigned")) opt.sgned = 0;
            else if (!strcmp(argv[i], "--len") && i + 1 < argc) opt.len = atoi(argv[++i]);
            else if (!strcmp(argv[i], "--pcm") && i + 1 < argc) opt.pcm_path = argv[++i];
            else { fprintf(stderr, "unknown option %s\n", argv[i]); return 2; }
        }
        if (opt.len > 65536) opt.len = 65536;
        data = load_file(argv[2], &len);
        if (!data) { fprintf(stderr, "cannot read %s\n", argv[2]); return 1; }
        o = fopen(argv[3], "wb");
        if (!o) return 1;
        if (opt.pcm_path) pcmf = fopen(opt.pcm_path, "wb");
        run_file(data, len, o, &opt, pcmf);
        fclose(o);
        if (pcmf) fclose(pcmf);
        free(data);
        return 0;
    }
    if (argc >= 4 && !strcmp(argv[1], "packets")) {
        long len, chunk = 4096;
        unsigned char* data;
        FILE* o;
        if (argc >= 6 && !strcmp(argv[4], "--chunk")) chunk = atol(argv[5]);
        if (chunk < 1) chunk = 1;
        data = load_file(argv[2], &len);
        if (!data) { fprintf(stderr, "cannot read %s\n", argv[2]); return 1; }
        o = fopen(argv[3], "wb");
        if (!o) return 1;
        run_packets(data, len, o, chunk);
        fclose(o);
        free(data);
        return 0;
    }
    if (argc >= 3 && !strcmp(argv[1], "tables")) {
        FILE* o = fopen(argv[2], "wb");
        if (!o) return 1;
        run_tables(o);
        fclose(o);
        return 0;
    }
    if (argc >= 4 && !strcmp(argv[1], "bench")) return run_bench(argv[2], atoi(argv[3]));
    fprintf(stderr, "usage: vorbis_oracle file|packets|tables|bench ...\n");
    return 2;
}
