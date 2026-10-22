/* asset_oracle: the viewer's sound asset decode (LLVorbisDecodeState in
 * indra/llaudio/llaudiodecodemgr.cpp: initDecode, decodeSection, finishDecode)
 * reimplemented in C over the viewer's libvorbisfile, writing the decoded
 * cache file the viewer would write.
 *
 *   asset_oracle <in.ogg> <out>
 *   asset_oracle --bench <list.txt> <reps>      (prints min ms for the list)
 *
 * Exit 0: <out> holds the .dsf bytes. Exit 3: <out> holds the error name
 * (the same names as vkstorm_audio::asset::AssetDecodeError::code_name).
 * Exit 2: usage or I/O error.
 *
 * The file callbacks are LLFileSystem's read/seek/tell semantics over the
 * asset in memory, as in tools/vorbis-oracle/c/vorbis_oracle.c. The decode
 * steps are transcribed from llaudiodecodemgr.cpp with the viewer's types
 * (S32 sample count, size_t size guess, F32 fade arithmetic, llfloor).
 */
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include <vorbis/codec.h>
#include <vorbis/vorbisfile.h>

#ifdef _WIN32
#include <windows.h>
static double now_ms(void) {
    LARGE_INTEGER c, fq;
    QueryPerformanceCounter(&c);
    QueryPerformanceFrequency(&fq);
    return (double)c.QuadPart * 1000.0 / (double)fq.QuadPart;
}
#else
static double now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000.0 + ts.tv_nsec / 1e6;
}
#endif

/* llvorbisencode.h */
#define LLVORBIS_CLIP_MAX_TIME 30.0f
#define LLVORBIS_CLIP_MAX_CHANNELS 2
#define LLVORBIS_CLIP_SAMPLE_RATE 44100
#define LLVORBIS_CLIP_MAX_SAMPLES_PER_CHANNEL ((uint32_t)(LLVORBIS_CLIP_MAX_TIME * LLVORBIS_CLIP_SAMPLE_RATE))
#define LLVORBIS_CLIP_MAX_SAMPLES (LLVORBIS_CLIP_MAX_SAMPLES_PER_CHANNEL * LLVORBIS_CLIP_MAX_CHANNELS)
#define LLVORBIS_CLIP_MAX_SAMPLE_DATA ((size_t)LLVORBIS_CLIP_MAX_SAMPLES * 2)
#define LLVORBIS_CLIP_REJECT_SAMPLES ((size_t)LLVORBIS_CLIP_MAX_SAMPLES * 3)
#define LLVORBIS_CLIP_REJECT_SIZE (LLVORBIS_CLIP_MAX_SAMPLE_DATA * 3)

#define WAV_HEADER_SIZE 44

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

static int32_t llfloor(float f) { return (int32_t)floorf(f); }

typedef struct { unsigned char* p; size_t n, cap; } buf_t;

static void buf_push(buf_t* b, const void* src, size_t n) {
    if (b->n + n > b->cap) {
        size_t cap = b->cap ? b->cap * 2 : 65536;
        while (cap < b->n + n) cap *= 2;
        b->p = (unsigned char*)realloc(b->p, cap);
        if (!b->p) exit(2);
        b->cap = cap;
    }
    memcpy(b->p + b->n, src, n);
    b->n += n;
}

/* Decodes `data`; returns 0 with *out filled, or 3 with err set. */
static int decode(const unsigned char* data, long len, buf_t* wav, char* err, size_t errlen) {
    memsrc_t src;
    ov_callbacks cb;
    OggVorbis_File vf;
    int32_t sample_count;
    size_t size_guess;
    vorbis_info* vi;
    int abort_decode = 0;
    static const unsigned char header[WAV_HEADER_SIZE] = {
        0x52, 0x49, 0x46, 0x46, 0, 0, 0, 0, 0x57, 0x41, 0x56, 0x45, 0x66, 0x6D, 0x74, 0x20,
        0x10, 0, 0, 0, 0x01, 0x00, 0x01, 0x00, 0x44, 0xAC, 0x00, 0x00, 0x88, 0x58, 0x01, 0x00,
        0x02, 0x00, 0x10, 0x00, 0x64, 0x61, 0x74, 0x61, 0, 0, 0, 0};
    int current_section = 0;
    int32_t data_length, fade_length, i, near_end;
    char pcmout[4096];

    err[0] = 0;
    wav->n = 0;
    /* initDecode */
    if (len == 0) { snprintf(err, errlen, "EmptyAsset"); return 3; }
    src.data = data;
    src.size = (int32_t)len;
    src.pos = 0;
    cb.read_func = mem_read;
    cb.seek_func = mem_seek;
    cb.close_func = mem_close;
    cb.tell_func = mem_tell;
    {
        int r = ov_open_callbacks(&src, &vf, NULL, 0, cb);
        if (r < 0) {
            snprintf(err, errlen, "NotOgg(%d)", r);
            return 3;
        }
    }
    sample_count = (int32_t)ov_pcm_total(&vf, -1);
    size_guess = (size_t)sample_count;
    vi = ov_info(&vf, -1);
    size_guess *= (vi ? vi->channels : 1);
    size_guess *= 2;
    size_guess += 2048;
    if (vi) {
        if (vi->channels < 1 || vi->channels > LLVORBIS_CLIP_MAX_CHANNELS) {
            abort_decode = 1;
            if (!*err) snprintf(err, errlen, "BadChannelCount(%d)", vi->channels);
        }
    } else {
        abort_decode = 1;
        if (!*err) snprintf(err, errlen, "NoDefaultBitstream");
    }
    if ((size_t)sample_count > LLVORBIS_CLIP_REJECT_SAMPLES || (size_t)sample_count <= 0) {
        abort_decode = 1;
        if (!*err) snprintf(err, errlen, "IllegalSampleCount(%d)", sample_count);
    }
    if (size_guess > LLVORBIS_CLIP_REJECT_SIZE) {
        abort_decode = 1;
        if (!*err) snprintf(err, errlen, "IllegalSampleSize(%llu)", (unsigned long long)size_guess);
    }
    if (abort_decode) {
        ov_clear(&vf);
        return 3;
    }
    buf_push(wav, header, WAV_HEADER_SIZE);

    /* decodeSection, until done */
    for (;;) {
        long ret = ov_read(&vf, pcmout, sizeof(pcmout), 0, 2, 1, &current_section);
        if (ret == 0) break;
        if (ret < 0) {
            ov_clear(&vf);
            snprintf(err, errlen, "DecodeError(%ld)", ret);
            return 3;
        }
        buf_push(wav, pcmout, (size_t)ret);
    }

    /* finishDecode */
    ov_clear(&vf);
    data_length = (int32_t)wav->n - WAV_HEADER_SIZE;
    wav->p[40] = (data_length) & 0x000000FF;
    wav->p[41] = (data_length >> 8) & 0x000000FF;
    wav->p[42] = (data_length >> 16) & 0x000000FF;
    wav->p[43] = (data_length >> 24) & 0x000000FF;
    data_length += 36;
    wav->p[4] = (data_length) & 0x000000FF;
    wav->p[5] = (data_length >> 8) & 0x000000FF;
    wav->p[6] = (data_length >> 16) & 0x000000FF;
    wav->p[7] = (data_length >> 24) & 0x000000FF;
    {
        int16_t* samplep;
        fade_length = (int32_t)128 < (int32_t)(data_length - 36) / 8 ? (int32_t)128 : (int32_t)(data_length - 36) / 8;
        if ((int32_t)wav->n >= (WAV_HEADER_SIZE + 2 * fade_length))
            memcpy(pcmout, &wav->p[WAV_HEADER_SIZE], (2 * fade_length));
        samplep = (int16_t*)pcmout;
        for (i = 0; i < fade_length; i++) {
            *samplep = (int16_t)llfloor((float)*samplep * ((float)i / (float)fade_length));
            samplep++;
        }
        if ((WAV_HEADER_SIZE + (2 * fade_length)) < (int32_t)wav->n)
            memcpy(&wav->p[WAV_HEADER_SIZE], pcmout, (2 * fade_length));
        near_end = (int32_t)wav->n - (2 * fade_length);
        if ((int32_t)wav->n >= (near_end + 2 * fade_length))
            memcpy(pcmout, &wav->p[near_end], (2 * fade_length));
        samplep = (int16_t*)pcmout;
        for (i = fade_length - 1; i >= 0; i--) {
            *samplep = (int16_t)llfloor((float)*samplep * ((float)i / (float)fade_length));
            samplep++;
        }
        if (near_end + (2 * fade_length) < (int32_t)wav->n)
            memcpy(&wav->p[near_end], pcmout, (2 * fade_length));
    }
    if (36 == data_length) { snprintf(err, errlen, "EmptyDecode"); return 3; }
    return 0;
}

static unsigned char* read_file(const char* path, long* len) {
    FILE* f = fopen(path, "rb");
    unsigned char* data;
    if (!f) return NULL;
    fseek(f, 0, SEEK_END);
    *len = ftell(f);
    fseek(f, 0, SEEK_SET);
    data = (unsigned char*)malloc(*len > 0 ? (size_t)*len : 1);
    if (data && *len > 0 && fread(data, 1, (size_t)*len, f) != (size_t)*len) {
        free(data);
        data = NULL;
    }
    fclose(f);
    return data;
}

int main(int argc, char** argv) {
    buf_t wav = {0, 0, 0};
    char err[128];
    long len = 0;
    unsigned char* data;
    int r;
    FILE* f;

    if (argc >= 4 && strcmp(argv[1], "--bench") == 0) {
        char line[4096];
        unsigned char* files[4096];
        long lens[4096];
        int n = 0, reps = atoi(argv[3]), k, j;
        double best = 1e300;
        f = fopen(argv[2], "r");
        if (!f) return 2;
        while (n < 4096 && fgets(line, sizeof line, f)) {
            line[strcspn(line, "\r\n")] = 0;
            if (!*line) continue;
            files[n] = read_file(line, &lens[n]);
            if (files[n]) n++;
        }
        fclose(f);
        for (k = 0; k < reps; k++) {
            double t0 = now_ms(), dt;
            size_t total = 0;
            for (j = 0; j < n; j++) {
                if (decode(files[j], lens[j], &wav, err, sizeof err) == 0) total += wav.n;
            }
            dt = now_ms() - t0;
            if (total == (size_t)-1) puts("");
            if (dt < best) best = dt;
        }
        printf("%.3f\n", best);
        return 0;
    }
    if (argc < 3) return 2;
    data = read_file(argv[1], &len);
    if (!data) return 2;
    r = decode(data, len, &wav, err, sizeof err);
    f = fopen(argv[2], "wb");
    if (!f) return 2;
    if (r == 0) fwrite(wav.p, 1, wav.n, f);
    else fputs(err, f);
    fclose(f);
    return r;
}
