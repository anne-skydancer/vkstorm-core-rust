/*
 * opj_bench: decode a JPEG 2000 codestream the way the Second Life viewer does
 * (LLImageJ2COJ): J2K codec, strict mode off, decoded resolution factor =
 * discard level, memory stream truncated to max_bytes.
 *
 * usage: opj_bench <file.j2c> <reduce> <max_bytes|0> <iterations> [out.raw]
 * prints: w h comps best_ms mean_ms  (dimensions are of the reduced output)
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

/* returns image or NULL; mirrors LLImageJ2COJ::JPEG2KDecode::decode */
static opj_image_t* decode_once(const unsigned char* data, OPJ_SIZE_T size, int reduce) {
    opj_dparameters_t params; opj_set_default_decoder_parameters(&params);
    params.cp_reduce = (OPJ_UINT32)reduce;
    opj_codec_t* codec = opj_create_decompress(OPJ_CODEC_J2K);
    opj_set_error_handler(codec, quiet, NULL);
    opj_set_warning_handler(codec, quiet, NULL);
    opj_set_info_handler(codec, quiet, NULL);
    if (!opj_setup_decoder(codec, &params)) { opj_destroy_codec(codec); return NULL; }
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
    if (!opj_decode(codec, stream, image)) { goto fail; }
    opj_end_decompress(codec, stream);
    opj_stream_destroy(stream); opj_destroy_codec(codec);
    return image;
fail:
    if (image) opj_image_destroy(image);
    opj_stream_destroy(stream); opj_destroy_codec(codec);
    return NULL;
}

int main(int argc, char** argv) {
    if (argc < 5) { fprintf(stderr, "usage: %s file reduce max_bytes iters [out.raw]\n", argv[0]); return 2; }
    const char* path = argv[1]; int reduce = atoi(argv[2]);
    long max_bytes = atol(argv[3]); int iters = atoi(argv[4]);
    FILE* f = fopen(path, "rb"); if (!f) { perror(path); return 2; }
    fseek(f, 0, SEEK_END); long size = ftell(f); fseek(f, 0, SEEK_SET);
    unsigned char* data = (unsigned char*)malloc((size_t)size);
    if (fread(data, 1, (size_t)size, f) != (size_t)size) { fprintf(stderr, "short read\n"); return 2; }
    fclose(f);
    OPJ_SIZE_T use = (max_bytes > 0 && max_bytes < size) ? (OPJ_SIZE_T)max_bytes : (OPJ_SIZE_T)size;

    double best = 1e30, total = 0; opj_image_t* last = NULL;
    for (int i = 0; i < iters; i++) {
        double t0 = now_ms();
        opj_image_t* img = decode_once(data, use, reduce);
        double dt = now_ms() - t0;
        if (!img) { printf("FAIL\n"); return 1; }
        if (dt < best) best = dt; total += dt;
        if (last) opj_image_destroy(last); last = img;
    }
    /* reduced dimensions: component 0 */
    unsigned w = last->comps[0].w, h = last->comps[0].h, nc = last->numcomps;
    printf("%u %u %u %.3f %.3f\n", w, h, nc, best, total / iters);
    if (argc >= 6) {
        FILE* o = fopen(argv[5], "wb");
        unsigned char* row = (unsigned char*)malloc((size_t)w * nc);
        for (unsigned y = 0; y < h; y++) {
            for (unsigned x = 0; x < w; x++)
                for (unsigned c = 0; c < nc; c++) {
                    int prec = last->comps[c].prec; int v = last->comps[c].data[y * w + x];
                    if (last->comps[c].sgnd) v += 1 << (prec - 1);
                    if (prec > 8) v >>= (prec - 8); else if (prec < 8) v <<= (8 - prec);
                    if (v < 0) v = 0; if (v > 255) v = 255;
                    row[x * nc + c] = (unsigned char)v;
                }
            fwrite(row, 1, (size_t)w * nc, o);
        }
        free(row); fclose(o);
    }
    opj_image_destroy(last); free(data);
    return 0;
}
