/*
 * vorbis_corpus: deterministic Ogg Vorbis test-stream encoder for the
 * vkstorm-vorbis decoder oracle corpus (tools/vorbis-oracle/scripts/gen_corpus.py).
 *
 * Links the viewer's prebuilt libvorbisenc + libvorbis + libogg (secondlife
 * 3p-ogg_vorbis 1.3.5-1.3.7) or the Linux builds of the same sources. The
 * output depends only on the command line: signals come from a seeded
 * splitmix64 generator (never rand()/time()), and libvorbisenc itself is
 * deterministic for a given input and library build.
 *
 * usage: vorbis_corpus <out.ogg> [options]
 *   --channels C        1..255 (default 1)
 *   --rate R            sample rate in Hz (default 44100)
 *   --samples N         frames per channel (default 44100; 0 is allowed)
 *   --signal S          sine|sweep|noise|transient|silence|clip|square|mix|dc|impulse|gaps
 *   --seed S            64-bit seed for the noise generator and default serial (default 1)
 *   --amp A             peak amplitude (default 0.5; clip 1.5; impulse 0.9)
 *   --q Q               VBR quality -0.1..1.0 (default 0.4; libvorbis "quality -1..10" / 10)
 *   --managed MIN,NOM,MAX  managed bitrate in bits/s, -1 for unset (vorbis_encode_init)
 *   --vbr-bitrate NOM   VBR aimed at an average bitrate (setup_managed + RATEMANAGE2_SET NULL,
 *                       as oggenc -b without --managed does)
 *   --no-coupling       disable channel coupling (OV_ECTL_COUPLING_SET 0)
 *   --serial S          Ogg stream serial number (default derived from the seed)
 *   --comment TAG=value add a user comment (repeatable; ASCII via argv)
 *   --comment-hex HEX   add a user comment given as hex bytes (UTF-8 etc., no NUL)
 *   --vendor-hex HEX    replace the comment header's vendor string (hex bytes)
 *   --vendor-free       comment header with an empty vendor string
 *   --chunk N           frames submitted per vorbis_analysis_buffer call (default 1024)
 *   --flush-every K     ogg_stream_flush after every K audio packets (small pages)
 *   --fill N            use ogg_stream_pageout_fill(N) instead of ogg_stream_pageout
 *   --viewer            reproduce LLVorbisEncode (indra/llaudio/llvorbisencode.cpp) exactly:
 *                       the signal is synthesised as WAV PCM (--channels 1|2, --bits 16|8)
 *                       and fed through the viewer's conversion/downmix loop with
 *                       READ_BUFFER = 1024 frames, mono vorbis_encode_init_vbr(q=0.05),
 *                       empty vorbis_comment, ogg_stream_flush for the headers, pageout after.
 *                       Only --channels, --rate, --samples, --signal, --seed, --amp,
 *                       --serial and --bits apply.
 *   --bits B            WAV word size for --viewer: 16 (default) or 8
 * Prints one line: frames channels rate serial pages bytes.
 * Exit status: 0 ok, 2 usage, 3 encoder setup refused (e.g. unsupported rate), 4 I/O.
 */
#include <vorbis/vorbisenc.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif

/* ---------------------------------------------------------------- PRNG */

typedef struct { uint64_t s; } rng_t;

static uint64_t splitmix64(rng_t* r) {
    uint64_t z = (r->s += 0x9E3779B97F4A7C15ull);
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ull;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EBull;
    return z ^ (z >> 31);
}

/* uniform in [-1, 1) with 53-bit resolution */
static double rng_signed(rng_t* r) {
    return (double)(splitmix64(r) >> 11) * (1.0 / 4503599627370496.0) - 1.0;
}

/* ---------------------------------------------------------------- signals */

enum sig { SIG_SINE, SIG_SWEEP, SIG_NOISE, SIG_TRANSIENT, SIG_SILENCE, SIG_CLIP, SIG_SQUARE,
           SIG_MIX, SIG_DC, SIG_IMPULSE, SIG_GAPS };
static const char* const sig_names[] = { "sine", "sweep", "noise", "transient", "silence", "clip",
                                         "square", "mix", "dc", "impulse", "gaps" };

/* Per-channel generator state. Every channel gets its own variant of the signal
 * (different frequency/phase/noise stream) so that channels differ and the
 * decoder's channel mapping and coupling are actually exercised. */
typedef struct {
    enum sig kind;
    int ch;
    double rate, amp, total;
    rng_t noise;
} gen_t;

static void gen_init(gen_t* g, enum sig kind, int ch, double rate, double amp, double total, uint64_t seed) {
    g->kind = kind; g->ch = ch; g->rate = rate; g->amp = amp; g->total = total;
    g->noise.s = seed ^ (0xD1B54A32D192ED03ull * (uint64_t)(ch + 1));
    (void)splitmix64(&g->noise);
}

/* fractional part of i*f/rate computed without accumulating phase error */
static double frac_cycles(double i, double f, double rate) {
    double c = i * f / rate;
    return c - floor(c);
}

/* decaying noise bursts on silence: sharp attacks force short blocks and pre-echo handling */
static double burst(gen_t* g, double i, double period, double len) {
    double k = fmod(i, period);
    if (k >= len) return 0.0;
    return rng_signed(&g->noise) * exp(-k / (len / 6.0));
}

static double gen_sample(gen_t* g, int64_t idx) {
    const double i = (double)idx, R = g->rate, c = (double)g->ch, A = g->amp;
    const double t = i / R;
    switch (g->kind) {
    case SIG_SINE:
        return A * sin(2.0 * M_PI * frac_cycles(i, 440.0 * (1.0 + 0.5 * c), R));
    case SIG_SWEEP: {
        /* exponential sweep from ~20 Hz to 0.45*rate over the whole file */
        double f0 = 20.0 * (1.0 + 0.1 * c), f1 = 0.45 * R;
        double T = g->total > 1.0 ? g->total / R : 1.0;
        double L = log(f1 / f0);
        double ph = f0 * T / L * (exp(t / T * L) - 1.0);
        return A * sin(2.0 * M_PI * (ph - floor(ph)));
    }
    case SIG_NOISE:
        return A * rng_signed(&g->noise);
    case SIG_TRANSIENT: {
        double period = floor(R / 4.0) + floor(R / 50.0) * c;
        double v = burst(g, i, period, floor(R * 0.006));
        if (fmod(i, period) == 0.0) v = 1.0;  /* single-sample click at the attack */
        return A * v;
    }
    case SIG_SILENCE:
        return 0.0;
    case SIG_CLIP: {
        /* peaks well above +-1.0 so the decoder's float->int16 clipping is exercised */
        double s = sin(2.0 * M_PI * frac_cycles(i, 220.0 * (1.0 + 0.3 * c), R));
        double q = frac_cycles(i, 55.0 * (1.0 + 0.2 * c), R) < 0.5 ? 1.0 : -1.0;
        return A * (0.7 * s + 0.3 * q);
    }
    case SIG_SQUARE:
        return A * (frac_cycles(i, 220.0 * (1.0 + 0.3 * c), R) < 0.5 ? 1.0 : -1.0);
    case SIG_MIX: {
        double v = 0.45 * sin(2.0 * M_PI * frac_cycles(i, 330.0 * (1.0 + 0.2 * c), R))
                 + 0.25 * sin(2.0 * M_PI * frac_cycles(i, 1234.5, R) + c)
                 + 0.08 * rng_signed(&g->noise)
                 + 0.5 * burst(g, i, floor(R / 2.0) + 97.0 * c, floor(R * 0.004));
        /* a silent stretch from 40% to 55% of the file */
        if (g->total > 0 && i >= 0.40 * g->total && i < 0.55 * g->total) v = 0.0;
        return A * v;
    }
    case SIG_DC:
        return A * (1.0 - 0.15 * c) * ((g->ch & 1) ? -1.0 : 1.0);
    case SIG_IMPULSE: {
        double period = floor(R / 10.0) + 37.0 * c;
        double k = fmod(i, period);
        if (k != 0.0) return 0.0;
        return (fmod(floor(i / period), 2.0) == 0.0) ? A : -A;
    }
    case SIG_GAPS: {
        /* 0.5 s of tone, then 2 s of digital silence, repeating */
        double cyc = fmod(t, 2.5);
        if (cyc >= 0.5) return 0.0;
        return A * sin(2.0 * M_PI * frac_cycles(i, 523.25 * (1.0 + 0.25 * c), R));
    }
    }
    return 0.0;
}

/* ---------------------------------------------------------------- output */

typedef struct { FILE* f; long pages; long long bytes; int err; } sink_t;

static void write_page(sink_t* s, const ogg_page* og) {
    if (fwrite(og->header, 1, (size_t)og->header_len, s->f) != (size_t)og->header_len ||
        fwrite(og->body, 1, (size_t)og->body_len, s->f) != (size_t)og->body_len)
        s->err = 1;
    s->pages++;
    s->bytes += og->header_len + og->body_len;
}

static int hexval(int c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}

/* decodes hex into a fresh NUL-terminated buffer; returns NULL on bad hex */
static char* unhex(const char* h, size_t* len_out) {
    size_t n = strlen(h);
    if (n % 2) return NULL;
    char* out = (char*)malloc(n / 2 + 1);
    if (!out) return NULL;
    for (size_t i = 0; i < n / 2; i++) {
        int a = hexval((unsigned char)h[2 * i]), b = hexval((unsigned char)h[2 * i + 1]);
        if (a < 0 || b < 0) { free(out); return NULL; }
        out[i] = (char)(a * 16 + b);
    }
    out[n / 2] = 0;
    *len_out = n / 2;
    return out;
}

static void put32(unsigned char* p, uint32_t v) {
    p[0] = (unsigned char)v; p[1] = (unsigned char)(v >> 8);
    p[2] = (unsigned char)(v >> 16); p[3] = (unsigned char)(v >> 24);
}

/* Comment header with a caller-chosen vendor string, laid out exactly like
 * libvorbis's _vorbis_pack_comment (type 3, "vorbis", vendor, comments, framing bit). */
static unsigned char* build_comment_packet(const vorbis_comment* vc, const char* vendor, size_t vlen, long* bytes) {
    size_t total = 7 + 4 + vlen + 4 + 1;
    for (int i = 0; i < vc->comments; i++) total += 4 + (size_t)vc->comment_lengths[i];
    unsigned char* p = (unsigned char*)malloc(total);
    if (!p) return NULL;
    size_t o = 0;
    p[o++] = 3; memcpy(p + o, "vorbis", 6); o += 6;
    put32(p + o, (uint32_t)vlen); o += 4;
    memcpy(p + o, vendor, vlen); o += vlen;
    put32(p + o, (uint32_t)vc->comments); o += 4;
    for (int i = 0; i < vc->comments; i++) {
        put32(p + o, (uint32_t)vc->comment_lengths[i]); o += 4;
        memcpy(p + o, vc->user_comments[i], (size_t)vc->comment_lengths[i]); o += (size_t)vc->comment_lengths[i];
    }
    p[o++] = 1;
    *bytes = (long)o;
    return p;
}

/* ---------------------------------------------------------------- main */

static void usage(void) {
    fputs("usage: vorbis_corpus <out.ogg> [--channels C] [--rate R] [--samples N] [--signal S] [--seed S]\n"
          "       [--amp A] [--q Q | --managed MIN,NOM,MAX | --vbr-bitrate NOM] [--no-coupling]\n"
          "       [--serial S] [--comment TAG=value]... [--comment-hex HEX]... [--vendor-hex HEX | --vendor-free]\n"
          "       [--chunk N] [--flush-every K] [--fill N] [--viewer [--bits 16|8]]\n", stderr);
}

#define MAX_COMMENTS 4096

int main(int argc, char** argv) {
    if (argc < 2) { usage(); return 2; }
    const char* out = argv[1];
    int channels = 1, bits = 16, viewer = 0, coupling = 1, mode = 0; /* mode 0 q, 1 managed, 2 vbr-bitrate */
    long rate = 44100, chunk = 1024, flush_every = 0, fill = 0;
    long long samples = 44100;
    long mmin = -1, mnom = -1, mmax = -1;
    double q = 0.4, amp = -1.0;
    uint64_t seed = 1;
    int have_serial = 0;
    long serial = 0;
    enum sig kind = SIG_MIX;
    const char* vendor = NULL; size_t vendor_len = 0; char* vendor_buf = NULL;
    char* comments[MAX_COMMENTS]; size_t ncomments = 0;

    for (int a = 2; a < argc; a++) {
        const char* k = argv[a];
        const char* v = (a + 1 < argc) ? argv[a + 1] : NULL;
#define NEEDV if (!v) { usage(); return 2; } a++;
        if (!strcmp(k, "--channels")) { NEEDV channels = atoi(v); }
        else if (!strcmp(k, "--rate")) { NEEDV rate = atol(v); }
        else if (!strcmp(k, "--samples")) { NEEDV samples = atoll(v); }
        else if (!strcmp(k, "--seed")) { NEEDV seed = strtoull(v, NULL, 0); }
        else if (!strcmp(k, "--amp")) { NEEDV amp = atof(v); }
        else if (!strcmp(k, "--q")) { NEEDV q = atof(v); mode = 0; }
        else if (!strcmp(k, "--managed")) {
            NEEDV if (sscanf(v, "%ld,%ld,%ld", &mmin, &mnom, &mmax) != 3) { usage(); return 2; }
            mode = 1;
        }
        else if (!strcmp(k, "--vbr-bitrate")) { NEEDV mnom = atol(v); mode = 2; }
        else if (!strcmp(k, "--no-coupling")) { coupling = 0; }
        else if (!strcmp(k, "--serial")) { NEEDV serial = (long)strtol(v, NULL, 0); have_serial = 1; }
        else if (!strcmp(k, "--comment") || !strcmp(k, "--comment-hex")) {
            NEEDV
            if (ncomments >= MAX_COMMENTS) { fputs("too many comments\n", stderr); return 2; }
            if (!strcmp(k, "--comment")) {
                comments[ncomments] = (char*)malloc(strlen(v) + 1);
                if (!comments[ncomments]) return 4;
                strcpy(comments[ncomments], v);
            } else {
                size_t n;
                comments[ncomments] = unhex(v, &n);
                if (!comments[ncomments] || strlen(comments[ncomments]) != n) { fputs("bad --comment-hex\n", stderr); return 2; }
            }
            ncomments++;
        }
        else if (!strcmp(k, "--vendor-hex")) {
            NEEDV vendor_buf = unhex(v, &vendor_len);
            if (!vendor_buf) { fputs("bad --vendor-hex\n", stderr); return 2; }
            vendor = vendor_buf;
        }
        else if (!strcmp(k, "--vendor-free")) { vendor = ""; vendor_len = 0; }
        else if (!strcmp(k, "--chunk")) { NEEDV chunk = atol(v); }
        else if (!strcmp(k, "--flush-every")) { NEEDV flush_every = atol(v); }
        else if (!strcmp(k, "--fill")) { NEEDV fill = atol(v); }
        else if (!strcmp(k, "--viewer")) { viewer = 1; }
        else if (!strcmp(k, "--bits")) { NEEDV bits = atoi(v); }
        else if (!strcmp(k, "--signal")) {
            NEEDV
            int found = 0;
            for (size_t s = 0; s < sizeof(sig_names) / sizeof(sig_names[0]); s++)
                if (!strcmp(v, sig_names[s])) { kind = (enum sig)s; found = 1; }
            if (!found) { fprintf(stderr, "unknown signal %s\n", v); return 2; }
        }
        else { fprintf(stderr, "unknown option %s\n", k); usage(); return 2; }
#undef NEEDV
    }
    if (channels < 1 || channels > 255 || rate < 1 || samples < 0 || chunk < 1 || chunk > 65536 ||
        (viewer && ((channels != 1 && channels != 2) || (bits != 8 && bits != 16)))) {
        usage(); return 2;
    }
    if (amp < 0.0) amp = kind == SIG_CLIP ? 1.5 : kind == SIG_IMPULSE ? 0.9 : 0.5;
    if (!have_serial) {
        rng_t sr = { seed ^ 0x5E71A15E71A1ull };
        serial = (long)(splitmix64(&sr) & 0x7FFFFFFF);
    }

    gen_t* gens = (gen_t*)calloc((size_t)channels, sizeof(gen_t));
    if (!gens) return 4;
    for (int c = 0; c < channels; c++) gen_init(&gens[c], kind, c, (double)rate, amp, (double)samples, seed);

    FILE* f = fopen(out, "wb");
    if (!f) { perror(out); return 4; }
    sink_t sink = { f, 0, 0, 0 };

    ogg_stream_state os;
    ogg_page og;
    ogg_packet op;
    vorbis_info vi;
    vorbis_comment vc;
    vorbis_dsp_state vd;
    vorbis_block vb;
    int eos = 0;

    vorbis_info_init(&vi);
    int rc;
    const int enc_channels = viewer ? 1 : channels;  /* the viewer always encodes mono */
    if (viewer) {
        rc = vorbis_encode_init_vbr(&vi, 1, rate, 0.05f);
    } else if (mode == 0) {
        rc = vorbis_encode_setup_vbr(&vi, enc_channels, rate, (float)q);
        if (!rc && !coupling) { int zero = 0; rc = vorbis_encode_ctl(&vi, OV_ECTL_COUPLING_SET, &zero); }
        if (!rc) rc = vorbis_encode_setup_init(&vi);
    } else if (mode == 1) {
        rc = vorbis_encode_setup_managed(&vi, enc_channels, rate, mmax, mnom, mmin);
        if (!rc && !coupling) { int zero = 0; rc = vorbis_encode_ctl(&vi, OV_ECTL_COUPLING_SET, &zero); }
        if (!rc) rc = vorbis_encode_setup_init(&vi);
    } else {
        rc = vorbis_encode_setup_managed(&vi, enc_channels, rate, -1, mnom, -1);
        if (!rc) rc = vorbis_encode_ctl(&vi, OV_ECTL_RATEMANAGE2_SET, NULL);
        if (!rc && !coupling) { int zero = 0; rc = vorbis_encode_ctl(&vi, OV_ECTL_COUPLING_SET, &zero); }
        if (!rc) rc = vorbis_encode_setup_init(&vi);
    }
    if (rc) {
        fprintf(stderr, "encoder setup refused (%d) for %d ch %ld Hz\n", rc, enc_channels, rate);
        fclose(f); remove(out);
        return 3;
    }

    vorbis_comment_init(&vc);   /* the viewer writes an empty comment list */
    if (!viewer)
        for (size_t i = 0; i < ncomments; i++) vorbis_comment_add(&vc, comments[i]);

    vorbis_analysis_init(&vd, &vi);
    vorbis_block_init(&vd, &vb);
    ogg_stream_init(&os, (int)serial);

    {
        ogg_packet header, header_comm, header_code;
        unsigned char* custom = NULL;
        vorbis_analysis_headerout(&vd, &vc, &header, &header_comm, &header_code);
        if (vendor && !viewer) {
            long n = 0;
            custom = build_comment_packet(&vc, vendor, vendor_len, &n);
            if (!custom) return 4;
            header_comm.packet = custom;
            header_comm.bytes = n;
        }
        ogg_stream_packetin(&os, &header);
        ogg_stream_packetin(&os, &header_comm);
        ogg_stream_packetin(&os, &header_code);
        /* flush all header pages so audio starts on a fresh page (as the viewer and oggenc do) */
        while (ogg_stream_flush(&os, &og)) write_page(&sink, &og);
        free(custom);
    }

    long audio_packets = 0;
    /* emit any pages that are ready, honouring the page-layout options */
#define DRAIN_PAGES()                                                                              \
    do {                                                                                           \
        ogg_stream_packetin(&os, &op);                                                             \
        audio_packets++;                                                                           \
        if (flush_every > 0 && audio_packets % flush_every == 0) {                                 \
            while (ogg_stream_flush(&os, &og)) { write_page(&sink, &og); if (ogg_page_eos(&og)) eos = 1; } \
        }                                                                                          \
        for (;;) {                                                                                 \
            int r = fill > 0 ? ogg_stream_pageout_fill(&os, &og, (int)fill) : ogg_stream_pageout(&os, &og); \
            if (!r) break;                                                                         \
            write_page(&sink, &og);                                                                \
            if (ogg_page_eos(&og)) eos = 1;                                                        \
        }                                                                                          \
    } while (0)

    if (viewer) {
        /* ---- LLVorbisEncode path: synthesise the WAV data chunk, then run the
         * viewer's read/convert loop over it byte for byte. */
        const int bps = bits / 8;
        const long long data_len = samples * channels * bps;
        unsigned char* wav = (unsigned char*)malloc((size_t)(data_len > 0 ? data_len : 1));
        if (!wav) return 4;
        for (long long n = 0; n < samples; n++) {
            for (int c = 0; c < channels; c++) {
                double x = gen_sample(&gens[c], n);
                unsigned char* p = wav + (n * channels + c) * bps;
                if (bits == 16) {
                    double s = floor(x * 32767.0 + 0.5);
                    if (s > 32767.0) s = 32767.0;
                    if (s < -32768.0) s = -32768.0;
                    int v = (int)s;
                    p[0] = (unsigned char)(v & 0xFF);
                    p[1] = (unsigned char)((v >> 8) & 0xFF);
                } else {
                    double s = floor(x * 127.0 + 128.5);
                    if (s > 255.0) s = 255.0;
                    if (s < 0.0) s = 0.0;
                    p[0] = (unsigned char)s;
                }
            }
        }
#define READ_BUFFER 1024
        unsigned char readbuffer[READ_BUFFER * 4 + 44];
        long long pos = 0, data_left = data_len;
        const int num_channels = channels;
        while (!eos) {
            long bytes_per_sample = bits / 8;
            long want = READ_BUFFER * num_channels * bytes_per_sample;
            if (want > data_left) want = (long)data_left;   /* llclamp(.., 0, data_left) */
            if (want < 0) want = 0;
            long bytes = want;
            memcpy(readbuffer, wav + pos, (size_t)bytes);
            pos += bytes;
            if (bytes == 0) {
                vorbis_analysis_wrote(&vd, 0);
            } else {
                long i, nsamp;
                int temp;
                data_left -= bytes;
                float** buffer = vorbis_analysis_buffer(&vd, READ_BUFFER);
                i = 0;
                nsamp = bytes / (num_channels * bytes_per_sample);
                if (num_channels == 2) {
                    if (bytes_per_sample == 2) {
                        for (i = 0; i < nsamp; i++) {
                            temp = ((signed char*)readbuffer)[i * 4 + 1];
                            temp += ((signed char*)readbuffer)[i * 4 + 3];
                            temp <<= 8;
                            temp += readbuffer[i * 4];
                            temp += readbuffer[i * 4 + 2];
                            buffer[0][i] = ((float)temp) / 65536.f;
                        }
                    } else {
                        for (i = 0; i < nsamp; i++) {
                            temp = readbuffer[i * 2 + 0];
                            temp += readbuffer[i * 2 + 1];
                            temp -= 256;
                            buffer[0][i] = ((float)temp) / 256.f;
                        }
                    }
                } else {
                    if (bytes_per_sample == 2) {
                        for (i = 0; i < nsamp; i++) {
                            temp = ((signed char*)readbuffer)[i * 2 + 1];
                            temp <<= 8;
                            temp += readbuffer[i * 2];
                            buffer[0][i] = ((float)temp) / 32768.f;
                        }
                    } else {
                        for (i = 0; i < nsamp; i++) {
                            temp = readbuffer[i];
                            temp -= 128;
                            buffer[0][i] = ((float)temp) / 128.f;
                        }
                    }
                }
                vorbis_analysis_wrote(&vd, (int)i);
            }
            while (vorbis_analysis_blockout(&vd, &vb) == 1) {
                vorbis_analysis(&vb, NULL);
                vorbis_bitrate_addblock(&vb);
                while (vorbis_bitrate_flushpacket(&vd, &op)) {
                    ogg_stream_packetin(&os, &op);
                    while (!eos) {
                        if (!ogg_stream_pageout(&os, &og)) break;
                        write_page(&sink, &og);
                        if (ogg_page_eos(&og)) eos = 1;
                    }
                }
            }
        }
        free(wav);
    } else {
        long long n = 0;
        int ended = 0;
        while (!eos) {
            if (n >= samples) {
                if (!ended) { vorbis_analysis_wrote(&vd, 0); ended = 1; }
            } else {
                long take = (long)((samples - n) < chunk ? (samples - n) : chunk);
                float** buffer = vorbis_analysis_buffer(&vd, (int)take);
                for (int c = 0; c < channels; c++)
                    for (long i = 0; i < take; i++) buffer[c][i] = (float)gen_sample(&gens[c], n + i);
                vorbis_analysis_wrote(&vd, (int)take);
                n += take;
            }
            int progressed = 0;
            while (vorbis_analysis_blockout(&vd, &vb) == 1) {
                progressed = 1;
                vorbis_analysis(&vb, NULL);
                vorbis_bitrate_addblock(&vb);
                while (vorbis_bitrate_flushpacket(&vd, &op)) DRAIN_PAGES();
            }
            if (ended && !progressed && !eos) {
                /* everything submitted and drained: push out whatever is left */
                while (ogg_stream_flush(&os, &og)) { write_page(&sink, &og); if (ogg_page_eos(&og)) eos = 1; }
                if (!eos) { fputs("encoder finished without an EOS page\n", stderr); break; }
            }
        }
    }

    ogg_stream_clear(&os);
    vorbis_block_clear(&vb);
    vorbis_dsp_clear(&vd);
    vorbis_comment_clear(&vc);
    vorbis_info_clear(&vi);
    for (size_t i = 0; i < ncomments; i++) free(comments[i]);
    free(vendor_buf);
    free(gens);
    if (fclose(f) || sink.err) { perror(out); return 4; }
    printf("%lld %d %ld %ld %ld %lld\n", samples, channels, rate, serial, sink.pages, sink.bytes);
    return 0;
}
