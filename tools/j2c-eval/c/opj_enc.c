/*
 * opj_enc: encode a raster the way the Second Life viewer does
 * (LLImageJ2COJ::encodeImpl / JPEG2KEncode): J2K codestream, RLCP, rate
 * allocation by distortion, 9/7 with layered rates 8*4^i (or reversible 5/3,
 * one layer), MCT for >= 3 components, fewer resolutions under 64 px, COM
 * comment "no comment" unless given.
 *
 * usage: opj_enc synth <w> <h> <c> <reversible 0|1> <out.j2c> [comment]
 *        opj_enc raw <in.raw bottom-up interleaved> <w> <h> <c> <reversible> <out.j2c> [comment]
 * The synthetic pattern matches tools/j2c-eval/examples/encode_probe.rs.
 * With OPJ_ENC_ITERS=<n> in the environment the encode (image fill through
 * opj_end_compress, the work of vkstorm_j2c::encode) runs n times and the
 * best time in ms is printed after the size.
 */
#include <openjpeg.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifdef _WIN32
#include <windows.h>
static double now_ms(void) { LARGE_INTEGER f, t; QueryPerformanceFrequency(&f); QueryPerformanceCounter(&t); return 1000.0 * (double)t.QuadPart / (double)f.QuadPart; }
#else
#include <time.h>
static double now_ms(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return t.tv_sec * 1000.0 + t.tv_nsec / 1e6; }
#endif

typedef struct { unsigned char* buf; size_t len; size_t cap; size_t pos; } mem_dst;

static OPJ_SIZE_T mem_write(void* data, OPJ_SIZE_T n, void* ud) {
    mem_dst* m = (mem_dst*)ud;
    size_t end = m->pos + n;
    if (end > m->cap) { while (m->cap < end) m->cap = m->cap ? m->cap * 2 : 65536; m->buf = (unsigned char*)realloc(m->buf, m->cap); }
    memcpy(m->buf + m->pos, data, n); m->pos = end; if (end > m->len) m->len = end; return n;
}
static OPJ_OFF_T mem_skip(OPJ_OFF_T n, void* ud) { mem_dst* m = (mem_dst*)ud; m->pos += (size_t)n; if (m->pos > m->len) m->len = m->pos; return n; }
static OPJ_BOOL mem_seek(OPJ_OFF_T n, void* ud) { mem_dst* m = (mem_dst*)ud; m->pos = (size_t)n; if (m->pos > m->len) m->len = m->pos; return OPJ_TRUE; }
static void quiet(const char* msg, void* d) { (void)msg; (void)d; }

static unsigned estimate_num_layers(unsigned surface) {
    if (surface <= 1024) return 2; if (surface <= 16384) return 3;
    if (surface <= 262144) return 4; if (surface <= 1048576) return 5; return 6;
}

int main(int argc, char** argv) {
    if (argc < 7) { fprintf(stderr, "usage: see source\n"); return 2; }
    int raw_mode = strcmp(argv[1], "raw") == 0; int ai = raw_mode ? 3 : 2;
    const char* in_path = raw_mode ? argv[2] : NULL;
    unsigned w = atoi(argv[ai]), h = atoi(argv[ai + 1]), c = atoi(argv[ai + 2]);
    int reversible = atoi(argv[ai + 3]); const char* out_path = argv[ai + 4];
    const char* comment = argc > ai + 5 ? argv[ai + 5] : "no comment";

    size_t n = (size_t)w * h * c; unsigned char* px = (unsigned char*)malloc(n);
    if (raw_mode) { FILE* f = fopen(in_path, "rb"); if (!f || fread(px, 1, n, f) != n) { fprintf(stderr, "read\n"); return 2; } fclose(f); }
    else { for (unsigned y = 0; y < h; y++) for (unsigned x = 0; x < w; x++) for (unsigned k = 0; k < c; k++) px[((size_t)y * w + x) * c + k] = (unsigned char)((x * 7 + y * 3 + k * 50) % 256); }

    opj_cparameters_t p; opj_set_default_encoder_parameters(&p);
    p.cod_format = OPJ_CODEC_J2K; p.prog_order = OPJ_RLCP; p.cp_disto_alloc = 1; p.max_cs_size = 0;
    if (reversible) { p.irreversible = 0; p.tcp_numlayers = 1; p.tcp_rates[0] = 0.0f; }
    else { p.irreversible = 1; unsigned L = estimate_num_layers(w * h); p.tcp_numlayers = (int)L;
           for (unsigned i = 0; i < L; i++) p.tcp_rates[L - 1 - i] = 8.0f * (float)(1u << (i << 1)); }
    p.cp_comment = (char*)comment;
    p.tcp_mct = (c >= 3) ? 1 : 0;
    { unsigned wt = w >> 6, ht = h >> 6; if (wt == 0 && w >= 4) wt = 1; if (ht == 0 && h >= 4) ht = 1;
      if (wt == 1 || ht == 1) { unsigned md = w < h ? w : h; p.numresolution = 1 + (int)floor(log2((double)md)); } }

    const char* iters_env = getenv("OPJ_ENC_ITERS");
    int iters = iters_env ? atoi(iters_env) : 1; if (iters < 1) iters = 1;
    double best = 1e30; mem_dst m = { 0 };
    for (int it = 0; it < iters; it++) {
    double t0 = now_ms();
    opj_image_cmptparm_t* cp = (opj_image_cmptparm_t*)calloc(c, sizeof(*cp));
    for (unsigned k = 0; k < c; k++) { cp[k].prec = 8; cp[k].sgnd = 0; cp[k].dx = p.subsampling_dx; cp[k].dy = p.subsampling_dy; cp[k].w = w; cp[k].h = h; }
    opj_image_t* img = opj_image_create(c, cp, OPJ_CLRSPC_SRGB); img->x1 = w; img->y1 = h;
    { size_t i = 0; for (int y = (int)h - 1; y >= 0; y--) for (unsigned x = 0; x < w; x++) { const unsigned char* pix = px + ((size_t)y * w + x) * c; for (unsigned k = 0; k < c; k++) img->comps[k].data[i] = pix[k]; i++; } }

    opj_codec_t* enc = opj_create_compress(OPJ_CODEC_J2K);
    opj_set_error_handler(enc, quiet, NULL); opj_set_warning_handler(enc, quiet, NULL); opj_set_info_handler(enc, quiet, NULL);
    if (!opj_setup_encoder(enc, &p, img)) { fprintf(stderr, "setup failed\n"); return 1; }
    if (it > 0) { free(m.buf); memset(&m, 0, sizeof m); }
    opj_stream_t* st = opj_stream_default_create(OPJ_FALSE);
    opj_stream_set_user_data(st, &m, NULL); opj_stream_set_write_function(st, mem_write);
    opj_stream_set_skip_function(st, mem_skip); opj_stream_set_seek_function(st, mem_seek);
    if (!opj_start_compress(enc, img, st) || !opj_encode(enc, st) || !opj_end_compress(enc, st)) { fprintf(stderr, "encode failed\n"); return 1; }
    opj_stream_destroy(st); opj_destroy_codec(enc); opj_image_destroy(img); free(cp);
    double dt = now_ms() - t0; if (dt < best) best = dt;
    }
    FILE* o = fopen(out_path, "wb"); fwrite(m.buf, 1, m.len, o); fclose(o);
    if (iters_env) printf("%zu %.3f\n", m.len, best); else printf("%zu\n", m.len);
    return 0;
}
