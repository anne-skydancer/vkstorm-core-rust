/*
 * opj_bench: decode a JPEG 2000 codestream the way the Second Life viewer does
 * (LLImageJ2COJ): J2K codec, strict mode off, decoded resolution factor =
 * discard level, memory stream truncated to max_bytes.
 *
 * usage: opj_bench <file.j2c> <reduce> <max_bytes|0> <iterations> [out.raw|-] [mode]
 * mode (optional, default whole image through opj_decode):
 *   tiles              every tile in turn through opj_get_decoded_tile on one
 *                      codec, as applications reading tiled images do; out.raw
 *                      is the concatenation of the tiles in index order
 *   area=x0,y0,x1,y1   opj_set_decode_area before opj_decode (image coordinates)
 * prints: w h comps best_ms mean_ms  (dimensions of the last image written)
 * out.raw: interleaved 8-bit samples, row-major, comps channels.
 */
#include <openjpeg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#ifdef _WIN32
#include <windows.h>
static double now_ms(void) {
    LARGE_INTEGER f, c; QueryPerformanceFrequency(&f); QueryPerformanceCounter(&c);
    return 1000.0 * (double)c.QuadPart / (double)f.QuadPart;
}
#else
static double now_ms(void) {
    struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1000.0 + t.tv_nsec / 1e6;
}
#endif

typedef struct { const unsigned char* data; OPJ_SIZE_T size; OPJ_SIZE_T pos; } mem_src;

static OPJ_SIZE_T mem_read(void* buf, OPJ_SIZE_T n, void* ud) {
    mem_src* s = (mem_src*)ud;
    if (s->pos >= s->size) return (OPJ_SIZE_T)-1;
    OPJ_SIZE_T left = s->size - s->pos; if (n > left) n = left;
    memcpy(buf, s->data + s->pos, n); s->pos += n; return n;
}
static OPJ_OFF_T mem_skip(OPJ_OFF_T n, void* ud) {
    mem_src* s = (mem_src*)ud;
    if (n < 0) { if ((OPJ_OFF_T)s->pos + n < 0) n = -(OPJ_OFF_T)s->pos; }
    else if ((OPJ_SIZE_T)n > s->size - s->pos) n = (OPJ_OFF_T)(s->size - s->pos);
    s->pos += (OPJ_SIZE_T)n; return n;
}
static OPJ_BOOL mem_seek(OPJ_OFF_T n, void* ud) {
    mem_src* s = (mem_src*)ud;
    if (n < 0 || (OPJ_SIZE_T)n > s->size) return OPJ_FALSE;
    s->pos = (OPJ_SIZE_T)n; return OPJ_TRUE;
}
static void quiet(const char* msg, void* d) { (void)msg; (void)d; }

typedef struct { int tiles; int area; int x0, y0, x1, y1; } bench_mode;
typedef struct { unsigned w, h, nc; } dims;

/* 8-bit interleaved rows of the image (component 0 dimensions), appended to o */
static dims write_image(const opj_image_t* img, FILE* o) {
    dims d; d.w = img->comps[0].w; d.h = img->comps[0].h; d.nc = img->numcomps;
    if (!o) return d;
    unsigned char* row = (unsigned char*)malloc((size_t)d.w * d.nc);
    for (unsigned y = 0; y < d.h; y++) {
        for (unsigned x = 0; x < d.w; x++)
            for (unsigned c = 0; c < d.nc; c++) {
                int prec = img->comps[c].prec; int v = img->comps[c].data[y * d.w + x];
                if (img->comps[c].sgnd) v += 1 << (prec - 1);
                if (prec > 8) v >>= (prec - 8); else if (prec < 8) v <<= (8 - prec);
                if (v < 0) v = 0; if (v > 255) v = 255;
                row[x * d.nc + c] = (unsigned char)v;
            }
        fwrite(row, 1, (size_t)d.w * d.nc, o);
    }
    free(row);
    return d;
}

/* returns 1 on success; mirrors LLImageJ2COJ::JPEG2KDecode::decode, plus the modes */
static int decode_once(const unsigned char* data, OPJ_SIZE_T size, int reduce, const bench_mode* mode, FILE* o, dims* out) {
    opj_dparameters_t params; opj_set_default_decoder_parameters(&params);
    params.cp_reduce = (OPJ_UINT32)reduce;
    opj_codec_t* codec = opj_create_decompress(OPJ_CODEC_J2K);
    opj_set_error_handler(codec, quiet, NULL);
    opj_set_warning_handler(codec, quiet, NULL);
    opj_set_info_handler(codec, quiet, NULL);
    if (!opj_setup_decoder(codec, &params)) { opj_destroy_codec(codec); return 0; }
    opj_decoder_set_strict_mode(codec, OPJ_FALSE);
    mem_src src = { data, size, 0 };
    opj_stream_t* stream = opj_stream_default_create(OPJ_TRUE);
    opj_stream_set_user_data(stream, &src, NULL);
    opj_stream_set_user_data_length(stream, (OPJ_UINT64)size);
    opj_stream_set_read_function(stream, mem_read);
    opj_stream_set_skip_function(stream, mem_skip);
    opj_stream_set_seek_function(stream, mem_seek);
    opj_image_t* image = NULL;
    if (!opj_read_header(stream, codec, &image)) { goto fail; }
    if (reduce > 0 && !opj_set_decoded_resolution_factor(codec, (OPJ_UINT32)reduce)) { goto fail; }
    if (mode->tiles) {
        opj_codestream_info_v2_t* info = opj_get_cstr_info(codec);
        if (!info) { goto fail; }
        OPJ_UINT32 ntiles = info->tw * info->th;
        opj_destroy_cstr_info(&info);
        for (OPJ_UINT32 t = 0; t < ntiles; t++) {
            if (!opj_get_decoded_tile(codec, stream, image, t)) { goto fail; }
            *out = write_image(image, o);
        }
    } else {
        if (mode->area && !opj_set_decode_area(codec, image, mode->x0, mode->y0, mode->x1, mode->y1)) { goto fail; }
        if (!opj_decode(codec, stream, image)) { goto fail; }
        *out = write_image(image, o);
    }
    opj_end_decompress(codec, stream);
    opj_image_destroy(image);
    opj_stream_destroy(stream); opj_destroy_codec(codec);
    return 1;
fail:
    if (image) opj_image_destroy(image);
    opj_stream_destroy(stream); opj_destroy_codec(codec);
    return 0;
}

int main(int argc, char** argv) {
    if (argc < 5) { fprintf(stderr, "usage: %s file reduce max_bytes iters [out.raw|-] [tiles|area=x0,y0,x1,y1]\n", argv[0]); return 2; }
    const char* path = argv[1]; int reduce = atoi(argv[2]);
    long max_bytes = atol(argv[3]); int iters = atoi(argv[4]);
    const char* out_path = (argc >= 6 && strcmp(argv[5], "-") != 0) ? argv[5] : NULL;
    bench_mode mode; memset(&mode, 0, sizeof mode);
    if (argc >= 7) {
        if (strcmp(argv[6], "tiles") == 0) mode.tiles = 1;
        else if (sscanf(argv[6], "area=%d,%d,%d,%d", &mode.x0, &mode.y0, &mode.x1, &mode.y1) == 4) mode.area = 1;
        else { fprintf(stderr, "unknown mode %s\n", argv[6]); return 2; }
    }
    FILE* f = fopen(path, "rb"); if (!f) { perror(path); return 2; }
    fseek(f, 0, SEEK_END); long size = ftell(f); fseek(f, 0, SEEK_SET);
    unsigned char* data = (unsigned char*)malloc((size_t)size);
    if (fread(data, 1, (size_t)size, f) != (size_t)size) { fprintf(stderr, "short read\n"); return 2; }
    fclose(f);
    OPJ_SIZE_T use = (max_bytes > 0 && max_bytes < size) ? (OPJ_SIZE_T)max_bytes : (OPJ_SIZE_T)size;

    double best = 1e30, total = 0; dims d = { 0, 0, 0 };
    for (int i = 0; i < iters; i++) {
        FILE* o = (i == iters - 1 && out_path) ? fopen(out_path, "wb") : NULL;
        double t0 = now_ms();
        int ok = decode_once(data, use, reduce, &mode, o, &d);
        double dt = now_ms() - t0;
        if (o) fclose(o);
        if (!ok) { printf("FAIL\n"); return 1; }
        if (dt < best) best = dt; total += dt;
    }
    printf("%u %u %u %.3f %.3f\n", d.w, d.h, d.nc, best, total / iters);
    free(data);
    return 0;
}
