/**
 * @file vkstorm_ffi.h
 * @brief C ABI exported by the Rust crate vkstorm-ffi (vkstorm-core-rust,
 * crates/vkstorm-ffi), shipped to the viewer as the vkstorm-ffi package.
 *
 * Keep in step with crates/vkstorm-ffi/src: j2c.rs (JPEG 2000), llsd.rs
 * (LLSD), volume.rs (the prim mesher), image.rs (PNG, TGA, BMP, DXT),
 * messages.rs (message templates, data packers), http.rs (the HTTP layer),
 * assets.rs (the asset pipeline), audio.rs (the audio engine) and
 * inventory.rs (the inventory data model).
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Vulkanstorm Viewer Source Code
 * Copyright (C) 2026, Vulkanstorm Viewer Project
 * Licensed under the GNU Lesser General Public License, version 2.1.
 * $/LicenseInfo$
 */
#ifndef VKSTORM_FFI_H
#define VKSTORM_FFI_H

#include <stddef.h>
#include <stdint.h>
#ifndef __cplusplus
#include <stdbool.h>
#endif

#ifdef __cplusplus
extern "C" {
#endif

/* ---- JPEG 2000 ---------------------------------------------------------- */

typedef struct VksJ2cMetadata
{
    uint32_t width;
    uint32_t height;
    uint32_t components;
} VksJ2cMetadata;

typedef struct VksJ2cDecoded VksJ2cDecoded;
typedef struct VksJ2cEncoded VksJ2cEncoded;

/* Static, NUL-terminated codec description for the engine-info display. */
const char* vks_j2c_engine_info(void);

/* Parse the main header. Returns 1 on success and fills out, 0 on failure. */
int32_t vks_j2c_metadata(const uint8_t* data, size_t len, VksJ2cMetadata* out);

/* Decode at discard_level using at most max_bytes of data (0 = all), keeping at
 * most max_channels channels from first_channel. Null on failure. */
VksJ2cDecoded* vks_j2c_decode(const uint8_t* data, size_t len, size_t max_bytes,
                              uint32_t discard_level, uint32_t first_channel, uint32_t max_channels);
void vks_j2c_decoded_size(const VksJ2cDecoded* d, uint32_t* width, uint32_t* height,
                          uint32_t* channels, uint32_t* components);
/* Copy width*height*channels interleaved, bottom-up 8-bit samples. 1 on success. */
int32_t vks_j2c_decoded_copy(const VksJ2cDecoded* d, uint8_t* dst, size_t dst_len);
/* COM marker comment, NUL-terminated, valid until freed; null when absent. */
const char* vks_j2c_decoded_comment(const VksJ2cDecoded* d);
void vks_j2c_decoded_free(VksJ2cDecoded* d);

/* Encode width*height*channels interleaved, bottom-up 8-bit samples into a raw
 * J2K codestream with the viewer's upload rules. comment may be null (then
 * "no comment"); reversible selects lossless coding. Null on failure. */
VksJ2cEncoded* vks_j2c_encode(const uint8_t* data, size_t len, uint32_t width, uint32_t height,
                              uint32_t channels, const char* comment, int32_t reversible);
const uint8_t* vks_j2c_encoded_data(const VksJ2cEncoded* e);
size_t vks_j2c_encoded_len(const VksJ2cEncoded* e);
void vks_j2c_encoded_free(VksJ2cEncoded* e);

/* ---- LLSD serialization ------------------------------------------------- */

/* Format ids. */
#define VKS_LLSD_NOTATION 0
#define VKS_LLSD_BINARY 1
#define VKS_LLSD_XML 2

/* A byte source for the parsers. read pulls up to cap bytes and returns the
 * count (0 = end of input). peek returns the next byte or -1 and is only used
 * when exact is set; then the parser never reads ahead of what it consumes,
 * so a stream is left positioned right after the parsed value. */
typedef size_t (*VksLlsdRead)(void* ctx, uint8_t* buf, size_t cap);
typedef int32_t (*VksLlsdPeek)(void* ctx);

typedef struct VksLlsdSource
{
    void* ctx;
    VksLlsdRead read;
    VksLlsdPeek peek;
    int32_t exact;
} VksLlsdSource;

/* The outcome of a parse: the LLSDParser::parse() return value, the bytes the
 * parser consumed and the bytes it pulled but left unconsumed (to seek back). */
typedef struct VksLlsdParseResult
{
    int32_t count;
    size_t consumed;
    size_t unconsumed;
} VksLlsdParseResult;

/* One node of a parsed document. len is the entry count of a map or array
 * and the byte length of a string, URI, binary, key or UUID (16). payload is
 * the integer or boolean (as int64), the real or date (double bits), or the
 * byte offset into the document's arena for the length-carrying kinds. A map's
 * entries follow it as key node, value subtree pairs; an array's as subtrees. */
typedef struct VksLlsdNode
{
    uint8_t kind;
    uint8_t pad[3];
    uint32_t len;
    uint64_t payload;
} VksLlsdNode;

#define VKS_LLSD_NODE_UNDEF 0
#define VKS_LLSD_NODE_BOOLEAN 1
#define VKS_LLSD_NODE_INTEGER 2
#define VKS_LLSD_NODE_REAL 3
#define VKS_LLSD_NODE_STRING 4
#define VKS_LLSD_NODE_UUID 5
#define VKS_LLSD_NODE_DATE 6
#define VKS_LLSD_NODE_URI 7
#define VKS_LLSD_NODE_BINARY 8
#define VKS_LLSD_NODE_MAP 9
#define VKS_LLSD_NODE_ARRAY 10
#define VKS_LLSD_NODE_KEY 11

typedef struct VksLlsdDoc VksLlsdDoc;

/* Parse one value of format from src, after the optional prefix bytes.
 * max_bytes < 0 and max_depth < 0 are unlimited; xml_lines selects the
 * parseLines() end-of-input rule. Fills out; returns the document when
 * out->count is positive, else null. */
VksLlsdDoc* vks_llsd_parse(int32_t format, const VksLlsdSource* src, const uint8_t* prefix, size_t prefix_len,
                           int64_t max_bytes, int32_t max_depth, int32_t xml_lines, VksLlsdParseResult* out);
const VksLlsdNode* vks_llsd_doc_nodes(const VksLlsdDoc* d, size_t* count);
const uint8_t* vks_llsd_doc_bytes(const VksLlsdDoc* d, size_t* len);
void vks_llsd_doc_free(VksLlsdDoc* d);

/* Formatting: one formatter per document, driven in document order. */
typedef void (*VksLlsdWrite)(void* ctx, const uint8_t* data, size_t len);
typedef struct VksLlsdFormatter VksLlsdFormatter;

#define VKS_LLSD_OPT_PRETTY 1u
#define VKS_LLSD_OPT_PRETTY_BINARY 2u
#define VKS_LLSD_OPT_BOOLALPHA 4u

/* precision and floatfield (0 general, 1 fixed, 2 scientific) are the
 * destination stream's settings for reals. Null for an unknown format. */
VksLlsdFormatter* vks_llsd_formatter_new(int32_t format, uint32_t options, int32_t precision, int32_t floatfield,
                                         void* ctx, VksLlsdWrite write);
void vks_llsd_fmt_undef(VksLlsdFormatter* f);
void vks_llsd_fmt_boolean(VksLlsdFormatter* f, int32_t v);
void vks_llsd_fmt_integer(VksLlsdFormatter* f, int32_t v);
void vks_llsd_fmt_real(VksLlsdFormatter* f, double v);
/* A real with its text already produced by the formatter's realFormat. */
void vks_llsd_fmt_real_text(VksLlsdFormatter* f, double v, const uint8_t* text, size_t len);
void vks_llsd_fmt_uuid(VksLlsdFormatter* f, const uint8_t* bytes16);
void vks_llsd_fmt_string(VksLlsdFormatter* f, const uint8_t* p, size_t len);
void vks_llsd_fmt_date(VksLlsdFormatter* f, double seconds);
void vks_llsd_fmt_uri(VksLlsdFormatter* f, const uint8_t* p, size_t len);
void vks_llsd_fmt_binary(VksLlsdFormatter* f, const uint8_t* p, size_t len);
void vks_llsd_fmt_map_begin(VksLlsdFormatter* f, size_t count);
void vks_llsd_fmt_key(VksLlsdFormatter* f, const uint8_t* p, size_t len);
void vks_llsd_fmt_map_end(VksLlsdFormatter* f);
void vks_llsd_fmt_array_begin(VksLlsdFormatter* f, size_t count);
void vks_llsd_fmt_array_end(VksLlsdFormatter* f);
/* Flushes, frees and returns the number of values written (-1 on failure). */
int32_t vks_llsd_formatter_finish(VksLlsdFormatter* f);

/* The notation (kind 0) or XML (kind 1) escape of len bytes, to write. */
void vks_llsd_escape(int32_t kind, const uint8_t* p, size_t len, void* ctx, VksLlsdWrite write);

/* ---- Parametric prim mesher ---------------------------------------------
 *
 * Vectors ("float*" below) are LLVector4a: four floats, 16-byte aligned.
 * Profile and path generation return pointers into per-thread scratch
 * storage, valid until the next generate call on the same thread; the
 * sweep, the face builders and the tangents write into the caller's
 * buffers. Every function returns -1 on bad arguments or an internal
 * failure (panics are caught at the boundary).
 */

/* LLProfileParams. */
typedef struct VksProfileParams
{
    uint8_t curve;
    float begin;
    float end;
    float hollow;
} VksProfileParams;

/* LLPathParams. */
typedef struct VksPathParams
{
    uint8_t curve;
    float begin;
    float end;
    float scale_x;
    float scale_y;
    float shear_x;
    float shear_y;
    float twist_begin;
    float twist_end;
    float radius_offset;
    float taper_x;
    float taper_y;
    float revolutions;
    float skew;
} VksPathParams;

/* LLProfile::Face, same layout (16 bytes). */
typedef struct VksProfileFace
{
    int32_t index;
    int32_t count;
    float scale_u;
    uint8_t cap;
    uint8_t flat;
    uint16_t face_id;
} VksProfileFace;

/* LLPath::PathPt, same layout (112 bytes; the buffers are 16-byte aligned). */
typedef struct VksPathPt
{
    float rot[4][4];
    float pos[4];
    float scale[4];
    float tex_t;
    float pad[3];
} VksPathPt;

typedef struct VksProfileOut
{
    const float* points;          /* point_count vectors */
    uint32_t point_count;
    const VksProfileFace* faces;
    uint32_t face_count;
    int32_t total;
    int32_t total_out;
    uint8_t open;
    uint8_t concave;
} VksProfileOut;

typedef struct VksPathOut
{
    const VksPathPt* points;
    uint32_t count;
    uint8_t open;
    float step;
} VksPathOut;

/* LLProfile::generate: 1 generated, 0 invalid cut (begin > end - 0.01; empty
 * profile), -1 failure. */
int32_t vks_volume_profile_generate(const VksProfileParams* params, int32_t path_open, float detail,
                                    int32_t split, int32_t is_sculpted, int32_t sculpt_size,
                                    VksProfileOut* out);
/* LLPath::generate: 1 generated, -1 failure. */
int32_t vks_volume_path_generate(const VksPathParams* params, float detail, int32_t split,
                                 int32_t is_sculpted, int32_t sculpt_size, VksPathOut* out);
/* LLProfile::getNumPoints and LLPath::getNumPoints (LOD triangle estimates). */
int32_t vks_volume_profile_num_points(const VksProfileParams* params, int32_t path_open, float detail,
                                      int32_t split, int32_t is_sculpted, int32_t sculpt_size);
int32_t vks_volume_path_num_points(const VksPathParams* params, float detail);

/* The mesh sweep of LLVolume::generate: path_count * profile_count vectors
 * into dst (path-major). 1 on success. */
int32_t vks_volume_sweep(const VksPathPt* path, uint32_t path_count, const float* profile,
                         uint32_t profile_count, float* dst);

/* What a face build reads: the volume's mesh, profile and path, the face
 * fields LLVolume::createVolumeFaces set, and the parameters consulted. */
typedef struct VksFaceParams
{
    const float* mesh;
    uint32_t mesh_count;
    const float* profile;
    uint32_t profile_count;
    int32_t profile_total;
    uint8_t profile_open;
    const VksPathPt* path;
    uint32_t path_count;
    uint8_t path_open;
    int32_t id;
    uint32_t type_mask;
    int32_t begin_s;
    int32_t num_s;
    int32_t begin_t;
    int32_t num_t;
    uint8_t profile_curve;
    uint8_t path_curve;
    uint8_t sculpt_type;
    float path_begin;
    float path_end;
} VksFaceParams;

/* The LLVolumeFace buffers; num_vertices/num_indices are the allocated
 * counts (mNumVertices, mNumIndices). The texture coordinate buffer must
 * have the even-count padding slot resizeVertices allocates. */
typedef struct VksFaceBuffers
{
    float* positions;
    float* normals;
    float* texcoords;   /* num_vertices pairs (+ padding) */
    uint16_t* indices;
    int32_t num_vertices;
    int32_t num_indices;
    uint8_t write_indices; /* !partial_build */
} VksFaceBuffers;

typedef struct VksFaceResult
{
    float extents_min[4];
    float extents_max[4];
    float center[4];
    float tc_min[2];
    float tc_max[2];
    uint8_t tc_extents_set; /* sides set the texture coordinate extents, caps do not */
} VksFaceResult;

enum
{
    VKS_FACE_SIDE = 0,
    VKS_FACE_CAP = 1,
    VKS_FACE_UNCUT_CUBE_CAP = 2
};

/* Which builder the face takes (VKS_FACE_*) and the vertex and index counts
 * it produces; -1 on bad arguments. */
int32_t vks_volume_face_sizes(const VksFaceParams* p, int32_t* num_vertices, int32_t* num_indices);
/* LLVolumeFace::create into the buffers: 1 built, 0 degenerate inputs
 * (nothing written), -1 failure. */
int32_t vks_volume_face_build(const VksFaceParams* p, const VksFaceBuffers* b, VksFaceResult* out);

/* LLCalculateTangentArray into tangents (handedness in w); with
 * normalize_normals the normals are normalised in place afterwards as
 * LLVolumeFace::createTangents does (otherwise they are only read). */
int32_t vks_volume_tangents(int32_t num_vertices, const float* positions, float* normals,
                            const float* texcoords, int32_t num_indices, const uint16_t* indices,
                            float* tangents, int32_t normalize_normals);

/* ---- Raster codecs: PNG, TGA, BMP, DXT (rung two) ---------------------- */

typedef struct VksImageHeader
{
    uint32_t width;
    uint32_t height;
    uint32_t components;
} VksImageHeader;

typedef struct VksRaster VksRaster; /* a decoded image, rows bottom-up */
typedef struct VksBytes VksBytes;   /* an encoded file */

/* The message of the last failed vks_png_*, vks_tga_*, vks_bmp_* or vks_dxt_*
 * call on this thread (the text the C++ codec passed to setLastError),
 * NUL-terminated, valid until the next failing call on the thread. */
const char* vks_image_last_error(void);

uint32_t vks_raster_width(const VksRaster* r);
uint32_t vks_raster_height(const VksRaster* r);
uint32_t vks_raster_components(const VksRaster* r);
/* width*height*components bytes, bottom-up, valid until freed. */
const uint8_t* vks_raster_data(const VksRaster* r);
size_t vks_raster_len(const VksRaster* r);
void vks_raster_free(VksRaster* r);

const uint8_t* vks_bytes_data(const VksBytes* b);
size_t vks_bytes_len(const VksBytes* b);
void vks_bytes_free(VksBytes* b);

/* PNG: header read (1 on success), decode (3 or 4 components, as libpng with
 * the viewer's transformations produced), encode of 1 to 4 components. */
int32_t vks_png_header(const uint8_t* data, size_t len, VksImageHeader* out);
VksRaster* vks_png_decode(const uint8_t* data, size_t len);
VksBytes* vks_png_encode(const uint8_t* data, size_t len, uint32_t width, uint32_t height, uint32_t components);

/* TGA */
typedef struct VksTgaHeader
{
    uint32_t width;
    uint32_t height;
    uint32_t components;
    uint32_t image_type;
    uint32_t origin_top;
    uint32_t origin_right;
} VksTgaHeader;

int32_t vks_tga_header(const uint8_t* data, size_t len, VksTgaHeader* out);
VksRaster* vks_tga_decode(const uint8_t* data, size_t len);
/* LLImageTGA::decodeAndProcess: monochrome run-length through the avatar
 * alpha-gradient function. */
VksRaster* vks_tga_decode_and_process(const uint8_t* data, size_t len, float domain, float weight);
VksBytes* vks_tga_encode(const uint8_t* data, size_t len, uint32_t width, uint32_t height, uint32_t components);

/* BMP: the header's components are what LLImageBMP reports (1 for 8-bit
 * files); decodes always have 3. */
int32_t vks_bmp_header(const uint8_t* data, size_t len, VksImageHeader* out);
VksRaster* vks_bmp_decode(const uint8_t* data, size_t len);
VksBytes* vks_bmp_encode(const uint8_t* data, size_t len, uint32_t width, uint32_t height, uint32_t components,
                         int32_t pels_per_meter);

/* DXT: the viewer's mipped container. format is LLImageDXT::EFileFormat. */
typedef struct VksDxtHeader
{
    uint32_t width;
    uint32_t height;
    uint32_t components;
    int32_t format;
    int32_t header_size;
    int32_t discard_level;
} VksDxtHeader;

int32_t vks_dxt_header(const uint8_t* data, size_t len, VksDxtHeader* out);
VksRaster* vks_dxt_decode(const uint8_t* data, size_t len, const VksDxtHeader* header);
VksRaster* vks_dxt_mip_data(const uint8_t* data, size_t len, const VksDxtHeader* header, int32_t discard);
VksBytes* vks_dxt_encode(const uint8_t* data, size_t len, uint32_t width, uint32_t height, uint32_t components);
/* Null when there is nothing to convert or on failure. */
VksBytes* vks_dxt_convert_to_dxr(const uint8_t* data, size_t len, const VksDxtHeader* header);
int32_t vks_dxt_data_size(const VksDxtHeader* header, int32_t discard);   /* -1 on failure */
int32_t vks_dxt_mip_offset(int32_t format, int32_t header_size, int32_t width, int32_t height, int32_t discard); /* -1 on failure */
int32_t vks_dxt_format_bits(int32_t format);                                /* 0 if unknown */
int32_t vks_dxt_format_bytes(int32_t format, int32_t width, int32_t height); /* 0 if unknown */
int32_t vks_dxt_format_components(int32_t format);                          /* 0 if unknown */
int32_t vks_dxt_format_from_fourcc(int32_t fourcc);
int32_t vks_dxt_fourcc(int32_t format);
int32_t vks_dxt_calc_num_mips(int32_t width, int32_t height);
void vks_dxt_calc_discard_width_height(int32_t discard, int32_t format, int32_t* width, int32_t* height);
void vks_dxt_check_min_width_height(int32_t format, int32_t* width, int32_t* height);

/* ---- Message templates and data packers (rung three) ------------------- */
/* See indra/rust/vkstorm-ffi/src/messages.rs. Blocks and variables are
 * identified by 64-bit keys: the viewer passes its canonical string
 * pointers. Errors the C++ aborted on come back as codes; the text is in
 * vks_msg_last_error() (thread-local, until the next failing call). */

#define VKS_MSG_OK 0
#define VKS_MSG_ERR_INTERNAL (-1)
#define VKS_MSG_ERR_NO_MESSAGE (-2)
#define VKS_MSG_ERR_NOT_A_BLOCK (-3)
#define VKS_MSG_ERR_SINGLE_REPEATED (-4)
#define VKS_MSG_ERR_MULTIPLE_EXCEEDED (-5)
#define VKS_MSG_ERR_TOO_MANY_BLOCKS (-6)
#define VKS_MSG_ERR_NO_BLOCK (-7)
#define VKS_MSG_ERR_NOT_A_VARIABLE (-8)
#define VKS_MSG_ERR_SIZE_MISMATCH (-9)
#define VKS_MSG_ERR_BUILD (-10)

typedef struct VksMsgTokenizer VksMsgTokenizer;
typedef struct VksMsgDesc VksMsgDesc;
typedef struct VksMsgTemplate VksMsgTemplate;
typedef struct VksMsgBuilder VksMsgBuilder;
typedef struct VksMsgReader VksMsgReader;

const char* vks_msg_last_error(void);

/* LLTemplateTokenizer */
VksMsgTokenizer* vks_msg_tokenizer_new(const uint8_t* text, size_t len);
VksMsgTokenizer* vks_msg_tokenizer_clone(const VksMsgTokenizer* t);
void vks_msg_tokenizer_free(VksMsgTokenizer* t);
int32_t vks_msg_tokenizer_at_eof(const VksMsgTokenizer* t);
uint32_t vks_msg_tokenizer_line(const VksMsgTokenizer* t);
/* 1 with the token (valid until the next call on t), 0 on error. */
int32_t vks_msg_tokenizer_next(VksMsgTokenizer* t, const uint8_t** out, size_t* len);
/* 1 consumed, 0 not the next token, -1 error. */
int32_t vks_msg_tokenizer_want(VksMsgTokenizer* t, const uint8_t* tok, size_t len);
int32_t vks_msg_tokenizer_want_eof(VksMsgTokenizer* t);

/* LLTemplateParser: descriptions the viewer turns into its own objects. */
typedef struct VksMsgInfo
{
    const uint8_t* name;
    size_t name_len;
    uint32_t number;
    int32_t frequency;
    int32_t trust;
    int32_t encoding;
    int32_t deprecation;
    int32_t total_size;
    uint32_t block_count;
} VksMsgInfo;

typedef struct VksMsgBlockInfo
{
    const uint8_t* name;
    size_t name_len;
    int32_t type;
    int32_t number;
    int32_t total_size;
    uint32_t var_count;
} VksMsgBlockInfo;

typedef struct VksMsgVarInfo
{
    const uint8_t* name;
    size_t name_len;
    int32_t type;
    int32_t size;
} VksMsgVarInfo;

int32_t vks_msg_parse_version(VksMsgTokenizer* t, float* out);
/* A description with status 1; null with status 0 when the next token is
 * not "{", null with status -1 on error. */
VksMsgDesc* vks_msg_parse_message(VksMsgTokenizer* t, int32_t* status);
VksMsgDesc* vks_msg_parse_block(VksMsgTokenizer* t, int32_t* status);
VksMsgDesc* vks_msg_parse_variable(VksMsgTokenizer* t, int32_t* status);
void vks_msg_desc_free(VksMsgDesc* d);
int32_t vks_msg_desc_message(const VksMsgDesc* d, VksMsgInfo* out);
int32_t vks_msg_desc_block(const VksMsgDesc* d, uint32_t block, VksMsgBlockInfo* out);
int32_t vks_msg_desc_variable(const VksMsgDesc* d, uint32_t block, uint32_t var, VksMsgVarInfo* out);

/* A template compiled from an LLMessageTemplate; builders and readers that
 * hold it keep it alive after vks_msg_template_free. */
VksMsgTemplate* vks_msg_template_new(const uint8_t* name, size_t name_len, uint64_t key, uint32_t number,
                                     int32_t frequency, int32_t trust, int32_t encoding, int32_t deprecation);
int32_t vks_msg_template_add_block(VksMsgTemplate* t, const uint8_t* name, size_t name_len, uint64_t key, int32_t type, int32_t number);
int32_t vks_msg_template_add_variable(VksMsgTemplate* t, uint32_t block, const uint8_t* name, size_t name_len, uint64_t key, int32_t type, int32_t size);
int32_t vks_msg_template_total_size(const VksMsgTemplate* t, int32_t block);
void vks_msg_template_free(VksMsgTemplate* t);

/* LLTemplateMessageBuilder */
VksMsgBuilder* vks_msg_builder_new(void);
void vks_msg_builder_free(VksMsgBuilder* b);
/* 1 when the message is deprecated (warn), else 0. */
int32_t vks_msg_builder_new_message(VksMsgBuilder* b, const VksMsgTemplate* t);
void vks_msg_builder_clear(VksMsgBuilder* b);
int32_t vks_msg_builder_next_block(VksMsgBuilder* b, uint64_t key);
int32_t vks_msg_builder_remove_last_block(VksMsgBuilder* b);
/* >= 0: flags (1 clamped to 255 bytes, 2 type mismatch); < 0 error. */
int32_t vks_msg_builder_add(VksMsgBuilder* b, uint64_t key, const uint8_t* data, size_t len, int32_t type);
int32_t vks_msg_builder_is_full(const VksMsgBuilder* b, uint64_t key, int32_t has_key);
int32_t vks_msg_builder_message_size(const VksMsgBuilder* b);
int32_t vks_msg_builder_is_built(const VksMsgBuilder* b);
void vks_msg_builder_set_built(VksMsgBuilder* b, int32_t built);
int32_t vks_msg_builder_is_clear(const VksMsgBuilder* b);
/* The packed size, or an error code. */
int64_t vks_msg_builder_build(VksMsgBuilder* b, uint8_t* buffer, size_t size, uint8_t offset);
int32_t vks_msg_builder_block_repeats(const VksMsgBuilder* b, uint32_t block);
const uint8_t* vks_msg_builder_slot(const VksMsgBuilder* b, uint32_t block, uint32_t repeat, uint32_t var, size_t* len, int32_t* data_size);

/* LLTemplateMessageReader */
VksMsgReader* vks_msg_reader_new(void);
void vks_msg_reader_free(VksMsgReader* r);
/* The message number (>= 0), -1 empty, -2 too short. */
int64_t vks_msg_decode_number(const uint8_t* buffer, size_t size);
void vks_msg_reader_set_message(VksMsgReader* r, const VksMsgTemplate* t, int32_t receive_size);
/* 1 when the message had blocks, 0 when none; events counts the reads
 * past the end (details through vks_msg_reader_event). */
int32_t vks_msg_reader_decode(VksMsgReader* r, const uint8_t* buffer, size_t size, int32_t* events);
int32_t vks_msg_reader_event(const VksMsgReader* r, uint32_t i, int32_t* position, int32_t* wanted);
void vks_msg_reader_clear(VksMsgReader* r);
int32_t vks_msg_reader_message_size(const VksMsgReader* r);
/* 0 ok, 1 cut to max_size, or an error code; actual gets the size. */
int32_t vks_msg_reader_get(const VksMsgReader* r, uint64_t block, int32_t blocknum, uint64_t var, uint8_t* dst, int32_t size, int32_t max_size, int32_t* actual);
int32_t vks_msg_reader_number_of_blocks(const VksMsgReader* r, uint64_t block);
int32_t vks_msg_reader_size(const VksMsgReader* r, uint64_t block, int32_t blocknum, uint64_t var);
int32_t vks_msg_reader_block_repeats(const VksMsgReader* r, uint32_t block);
const uint8_t* vks_msg_reader_slot(const VksMsgReader* r, uint32_t block, uint32_t repeat, uint32_t var, size_t* len);

/* Zero-coding */
/* The coded length in out (flag set), 0 when coding would not shrink the
 * packet, -1 on bad arguments. out needs len + 1 bytes. */
int32_t vks_msg_zero_code(const uint8_t* packet, size_t len, uint8_t* out, size_t out_len);
int32_t vks_msg_zero_code_gain(const uint8_t* packet, size_t len);
/* The expanded length in out (flag cleared); -1/-2 when the packet would
 * pass the end of out. overruns counts runs written from the start. */
int32_t vks_msg_zero_code_expand(const uint8_t* packet, size_t len, uint8_t* out, size_t out_len, int32_t* overruns);

/* LLDataPacker: the buffer state the C++ object keeps, per call. */
typedef struct VksDpBuffer
{
    uint8_t* data;
    int32_t size;
    int32_t cur;
    int32_t write_enabled;
    int32_t include_names;
} VksDpBuffer;

#define VKS_DP_STRING 0
#define VKS_DP_BINARY 1
#define VKS_DP_BINARY_FIXED 2
#define VKS_DP_INT 3
#define VKS_DP_UINT 4
#define VKS_DP_FLOATS 5
#define VKS_DP_BYTES 6
#define VKS_DP_UUID 7
#define VKS_DP_FLOATS_SHORT 8 /* %f cut to 19 characters, the stream file form */

int32_t vks_dp_bin_pack(VksDpBuffer* s, const uint8_t* v, size_t len);
int32_t vks_dp_bin_unpack(VksDpBuffer* s, uint8_t* dst, size_t len);
int32_t vks_dp_bin_pack_s16(VksDpBuffer* s, int16_t v);
int32_t vks_dp_bin_unpack_s16(VksDpBuffer* s, int16_t* out);
int32_t vks_dp_bin_pack_string(VksDpBuffer* s, const uint8_t* v, size_t len);
int32_t vks_dp_bin_unpack_string(VksDpBuffer* s, const uint8_t** out, size_t* len);
int32_t vks_dp_bin_pack_binary(VksDpBuffer* s, const uint8_t* v, size_t len);
int32_t vks_dp_bin_unpack_binary(VksDpBuffer* s, const uint8_t** out, size_t* len);
int32_t vks_dp_ascii_pack(VksDpBuffer* s, int32_t kind, const uint8_t* name, size_t name_len,
                          const int64_t* ints, size_t nints, const float* floats, size_t nfloats,
                          const uint8_t* data, size_t ndata, int32_t* truncated);
int32_t vks_dp_ascii_next_value(VksDpBuffer* s, const uint8_t* name, size_t name_len, uint8_t* out, size_t cap, size_t* len);
int32_t vks_dp_ascii_parse(int32_t kind, const uint8_t* text, size_t len, size_t count,
                           int64_t* ints, float* floats, uint8_t* data, size_t data_cap, int32_t* size, size_t* ndata);
size_t vks_dp_ascii_format(int32_t kind, const int64_t* ints, size_t nints, const float* floats, size_t nfloats,
                           const uint8_t* data, size_t ndata, uint8_t* out, size_t cap);
void vks_dp_ascii_split(const uint8_t* line, size_t len, const uint8_t** key, size_t* key_len, const uint8_t** value, size_t* value_len);
uint32_t vks_dp_fixed_pack(float value, int32_t is_signed, uint32_t int_bits, uint32_t frac_bits, int32_t* total_bits);
float vks_dp_fixed_unpack(uint32_t fixed, int32_t is_signed, uint32_t int_bits, uint32_t frac_bits);
int32_t vks_dp_fixed_total_bits(int32_t is_signed, uint32_t int_bits, uint32_t frac_bits);

/* ---- HTTP layer (rung four) --------------------------------------------- */

/* The service of LLCore::HttpRequest: request queue, service thread, policy
 * classes, retry queue and the libcurl transport live in the crate
 * (vkstorm-http). One service per process. Handles are a 64-bit counter,
 * never recycled; the C++ casts them to HttpHandle. */

/* What the SSL context hook receives for a request whose connection is
 * being set up, and where its verification callback writes its decision
 * (a status type and code; code 0 is success). Lives as long as the
 * request is active. */
typedef struct VksHttpSslInfo
{
    uint64_t    handle;
    void*       user;
    const char* url;         /* NUL-terminated */
    int32_t     verify_peer;
    uint16_t    result_type;
    int16_t     result_code;
} VksHttpSslInfo;

/* The host's facilities. Any function may be null. Levels: 0 debug, 1 info,
 * 2 warning. Stat kinds: 0 bytes received, 1 bytes sent, 2 result code. */
typedef struct VksHttpHooks
{
    void* ctx;
    void (*log)(void* ctx, int32_t level, const char* text, size_t len);
    void (*stat)(void* ctx, int32_t kind, uint64_t value);
    void (*apply_proxy)(void* ctx, void* curl);                       /* LLProxy::applyProxySettings */
    void (*ssl_ctx)(void* ctx, void* ssl_ctx, VksHttpSslInfo* info);  /* CURLOPT_SSL_CTX_FUNCTION body */
    void (*thread_start)(void* ctx);
    void (*thread_stop)(void* ctx);
    void (*release_user)(void* ctx, void* user);                      /* user data never delivered */
} VksHttpHooks;

/* LLCore::HttpStatus by value. */
typedef struct VksHttpStatus
{
    uint16_t type;
    int16_t  code;
} VksHttpStatus;

/* LLCore::HttpOptions by value. */
typedef struct VksHttpOptions
{
    int32_t  want_headers;
    int32_t  trace;
    uint32_t timeout;
    uint32_t transfer_timeout;
    uint32_t retries;
    uint64_t min_backoff;
    uint64_t max_backoff;
    int32_t  use_retry_after;
    int32_t  follow_redirects;
    int32_t  verify_peer;
    int32_t  verify_host;
    int32_t  dns_cache_timeout;
    int32_t  headers_only;
    int64_t  last_modified;
} VksHttpOptions;

typedef struct VksHttpHeader
{
    const char* name;
    size_t      name_len;
    const char* value;
    size_t      value_len;
} VksHttpHeader;

/* method: 0 GET, 1 POST, 2 PUT, 3 DELETE, 4 PATCH, 5 COPY, 6 MOVE. */
typedef struct VksHttpRequestDesc
{
    uint32_t              policy;
    int32_t               method;
    const char*           url;
    size_t                url_len;
    const uint8_t*        body;
    size_t                body_len;
    int32_t               has_body;
    uint64_t              offset;
    uint64_t              length;
    const VksHttpHeader*  headers;
    size_t                header_count;
    const VksHttpOptions* options;   /* may be null */
} VksHttpRequestDesc;

typedef struct VksHttpReplyQueue VksHttpReplyQueue;
typedef struct VksHttpReply VksHttpReply;

/* Service life cycle. States: -1 not initialised, 0 initialised, 1 running,
 * 2 stopped. */
int32_t  vks_http_service_create(const VksHttpHooks* hooks, const char* user_agent);
void     vks_http_service_destroy(void);
int32_t  vks_http_service_state(void);
void     vks_http_service_start_thread(void);

/* Policy classes and the static option interface (before the thread runs).
 * opt is HttpRequest::EPolicyOption; pclass a class id or GLOBAL_POLICY_ID. */
uint32_t      vks_http_policy_class_create(void);
VksHttpStatus vks_http_policy_set_long(int32_t opt, uint32_t pclass, int64_t value, int64_t* ret);
VksHttpStatus vks_http_policy_set_string(int32_t opt, uint32_t pclass, const char* value, size_t len);
VksHttpStatus vks_http_policy_set_callback(int32_t opt, uint32_t pclass, int32_t set);
VksHttpStatus vks_http_policy_get_long(int32_t opt, uint32_t pclass, int64_t* ret);
VksHttpStatus vks_http_policy_get_string(int32_t opt, uint32_t pclass, char* buf, size_t cap, size_t* len);

/* Reply queues: one per HttpRequest instance. */
VksHttpReplyQueue* vks_http_reply_queue_new(void);
void               vks_http_reply_queue_free(VksHttpReplyQueue* q);

/* Operations. Each returns the handle, or 0 with *status set. `user` is
 * handed back with the reply (or through release_user). */
uint64_t vks_http_request_submit(VksHttpReplyQueue* q, const VksHttpRequestDesc* desc, void* user, VksHttpStatus* status);
uint64_t vks_http_request_cancel(VksHttpReplyQueue* q, uint64_t target, void* user, VksHttpStatus* status);
uint64_t vks_http_request_noop(VksHttpReplyQueue* q, void* user, VksHttpStatus* status);
uint64_t vks_http_request_stop(VksHttpReplyQueue* q, void* user, VksHttpStatus* status);
uint64_t vks_http_request_spin(VksHttpReplyQueue* q, int32_t mode, VksHttpStatus* status);
uint64_t vks_http_request_set_long(VksHttpReplyQueue* q, int32_t opt, uint32_t pclass, int64_t value, void* user, VksHttpStatus* status);
uint64_t vks_http_request_set_string(VksHttpReplyQueue* q, int32_t opt, uint32_t pclass, const char* value, size_t len, void* user, VksHttpStatus* status);

/* Replies. Kinds: 0 request, 1 cancel, 2 option set/get, 3 stop, 4 no-op.
 * Pointers returned by the accessors are valid until the reply is freed;
 * strings are not NUL-terminated. */
VksHttpReply* vks_http_reply_fetch(VksHttpReplyQueue* q);
void          vks_http_reply_free(VksHttpReply* r);
uint64_t      vks_http_reply_handle(const VksHttpReply* r);
void*         vks_http_reply_user(const VksHttpReply* r);
int32_t       vks_http_reply_kind(const VksHttpReply* r);
VksHttpStatus vks_http_reply_status(const VksHttpReply* r);
const uint8_t* vks_http_reply_body(const VksHttpReply* r, size_t* len);
int32_t       vks_http_reply_has_headers(const VksHttpReply* r);
size_t        vks_http_reply_header_count(const VksHttpReply* r);
int32_t       vks_http_reply_header(const VksHttpReply* r, size_t i, VksHttpHeader* out);
const char*   vks_http_reply_content_type(const VksHttpReply* r, size_t* len);
int32_t       vks_http_reply_range(const VksHttpReply* r, uint64_t* offset, uint64_t* length, uint64_t* full);
uint32_t      vks_http_reply_retries(const VksHttpReply* r);
uint32_t      vks_http_reply_retries_503(const VksHttpReply* r);
const char*   vks_http_reply_url(const VksHttpReply* r, size_t* len);
const char*   vks_http_reply_method(const VksHttpReply* r, size_t* len);
void          vks_http_reply_stats(const VksHttpReply* r, double* size_download, double* total_time, double* speed_download);
int64_t       vks_http_reply_long(const VksHttpReply* r);
const char*   vks_http_reply_string(const VksHttpReply* r, size_t* len);

/* HttpStatus::isRetryable, and curl_version() of the linked libcurl. */
int32_t     vks_http_status_is_retryable(uint16_t type, int16_t code);
const char* vks_http_curl_version(void);

/* ---- The asset pipeline (rung five) ------------------------------------ */

/* The log sink the asset crate reports through: level 0 debug, 1 info,
 * 2 warn; the text is not NUL-terminated. */
void vks_assets_set_log(void (*log)(int32_t level, const char* msg, size_t len));

/* The texture cache: LLTextureCache's index, entries file, header-data
 * file, body files and fast cache, byte-compatible with the C++ files. */
typedef struct VksTexCache VksTexCache;

typedef struct VksTexCachePaths
{
    const char* location;       /* the cache location directory */
    size_t      location_len;
    const char* delim;          /* the directory delimiter */
    size_t      delim_len;
} VksTexCachePaths;

typedef struct VksFastEntry
{
    int32_t        width;
    int32_t        height;
    int32_t        components;
    int32_t        discard_level;
    const uint8_t* data;
    size_t         data_len;
} VksFastEntry;

VksTexCache* vks_texcache_new(const char* encoder, size_t encoder_len, uint32_t address_size, uint32_t (*clock)(void));
void         vks_texcache_free(VksTexCache* c);
int32_t      vks_texcache_set_read_only(VksTexCache* c, int32_t read_only);
int32_t      vks_texcache_is_read_only(VksTexCache* c);
int64_t      vks_texcache_usage(VksTexCache* c);
int64_t      vks_texcache_max_usage(VksTexCache* c);
uint32_t     vks_texcache_entries(VksTexCache* c);
uint32_t     vks_texcache_max_entries(VksTexCache* c);
int32_t      vks_texcache_is_in_cache(VksTexCache* c, const uint8_t* id);
int32_t      vks_texcache_purge_pending(VksTexCache* c);
int32_t      vks_texcache_purge_cache(VksTexCache* c, const VksTexCachePaths* paths, int32_t remove_dir);
int64_t      vks_texcache_init(VksTexCache* c, const VksTexCachePaths* paths, int64_t max_size, int32_t mismatch, int32_t validate_idx);
int32_t      vks_texcache_lazy_purge_tick(VksTexCache* c, float time_limit_sec);
int32_t      vks_texcache_write_updated_entries(VksTexCache* c);
int32_t      vks_texcache_shutdown(VksTexCache* c);
int32_t      vks_texcache_remove(VksTexCache* c, const uint8_t* id);
size_t       vks_texcache_texture_file_name(VksTexCache* c, const uint8_t* id, char* buf, size_t cap);
/* doRead: up to `size` bytes at `offset` into `buf` (at least `size`
 * bytes); returns the data size (0 not cached, -1 failed). */
int32_t      vks_texcache_read(VksTexCache* c, const uint8_t* id, int32_t offset, int32_t size, uint8_t* buf, int32_t* image_size);
int32_t      vks_texcache_fast_level(int32_t width, int32_t height, int32_t components);
/* doWrite: 1 on success. */
int32_t      vks_texcache_write(VksTexCache* c, const uint8_t* id, const uint8_t* data, size_t len, int32_t image_size, int32_t discard_level, const VksFastEntry* fast);
/* readFromFastCache: `data` receives up to 1024 bytes. */
int32_t      vks_texcache_read_fast(VksTexCache* c, const uint8_t* id, int32_t* width, int32_t* height, int32_t* components, int32_t* discard_level, uint8_t* data);
/* readFromFastCache without waiting for the fast cache (upstream #5356):
 * 1 read, 0 not cached or unreadable, -1 busy (another thread is writing the
 * fast cache; skip and retry next frame). */
int32_t      vks_texcache_try_read_fast(VksTexCache* c, const uint8_t* id, int32_t* width, int32_t* height, int32_t* components, int32_t* discard_level, uint8_t* data);

/* Plain file operations: LLAPRFile by name, LLFileSystem, LLLFSThread. */
int32_t vks_file_size(const char* path, size_t len);
int32_t vks_file_read_at(const char* path, size_t len, int64_t offset, uint8_t* buf, size_t cap);
int32_t vks_asset_file_exists(const char* path, size_t len);
int32_t vks_asset_file_size(const char* path, size_t len);
int32_t vks_asset_file_read(const char* path, size_t len, int32_t position, uint8_t* buf, size_t cap);
int32_t vks_asset_file_write(const char* path, size_t len, int32_t mode, int32_t position, const uint8_t* data, size_t data_len);
int32_t vks_asset_file_seek(int32_t position, int32_t offset, int32_t origin, int32_t size, int32_t* new_position);
void    vks_asset_file_remove(const char* path, size_t len);
int32_t vks_asset_file_rename(const char* from, size_t from_len, const char* to, size_t to_len);
void    vks_asset_file_touch(const char* path, size_t len);
int32_t vks_lfs_read(const char* path, size_t len, int32_t offset, uint8_t* buf, size_t cap);
int32_t vks_lfs_write(const char* path, size_t len, int32_t offset, const uint8_t* data, size_t data_len);

/* The texture fetcher: LLTextureFetch's HTTP resource accounting and
 * LLTextureFetchWorker's state machine. */
typedef struct VksTexFetcher VksTexFetcher;
typedef struct VksTexFetchWorker VksTexFetchWorker;

typedef struct VksTexFetchEnv
{
    int32_t quitting;
    int32_t delete_requested;
    int32_t debug_pause;
    int32_t non_interactive;
    int32_t blacklisted;
    int32_t formatted_present;
    int32_t formatted_size;
    int32_t formatted_discard;
    int32_t formatted_is_j2c;
    int32_t formatted_width;
    int32_t formatted_height;
    int32_t can_load_from_cache;
    int32_t use_http_textures;
    int32_t disconnected;
    int32_t range_requests_disabled;
    int32_t decode_disabled;
    int32_t raw_width;
    int32_t raw_height;
    float   texture_bandwidth;
    float   max_bandwidth;
} VksTexFetchEnv;

typedef struct VksTexFetchRegion
{
    int32_t     present;
    uint8_t     id[16];
    const char* viewer_asset_url;
    size_t      viewer_asset_url_len;
    const char* http_url;
    size_t      http_url_len;
} VksTexFetchRegion;

typedef struct VksTexFetchHttpGet
{
    const char* url;
    size_t      url_len;
    int32_t     offset;
    int32_t     size;        /* 0: the rest of the file */
    int32_t     no_range;    /* HttpRangeRequestsDisable */
    int32_t     with_headers;
} VksTexFetchHttpGet;

/* The host's side of the state machine: the C++ worker's objects. None of
 * the functions may throw. */
typedef struct VksTexFetchHost
{
    void* ctx;
    void     (*env)(void* ctx, VksTexFetchEnv* out);
    void     (*region)(void* ctx, VksTexFetchRegion* out);
    void     (*read_cache)(void* ctx, const char* local_file, size_t local_file_len, int32_t offset, int32_t size);
    int32_t  (*cache_read_complete)(void* ctx);
    int32_t  (*send_http)(void* ctx, const VksTexFetchHttpGet* get);
    void     (*drop_raw_images)(void* ctx);
    uint32_t (*decode)(void* ctx, int32_t discard, int32_t needs_aux);
    void     (*write_cache)(void* ctx, int32_t file_size, int32_t decoded_discard);
    int32_t  (*cache_write_complete)(void* ctx);
    void     (*prioritize_write)(void* ctx);
    void     (*remove_from_cache)(void* ctx);
    void     (*reset_formatted_data)(void* ctx);
    void     (*drop_formatted_image)(void* ctx);
    void     (*release_http_buffer)(void* ctx);
    int32_t  (*assemble_http_reply)(void* ctx, const char* url, size_t url_len, int32_t src_offset, int32_t cur_size, int32_t append_size, int32_t total_size);
    void     (*add_to_http_queue)(void* ctx);
    void     (*record_texture_start)(void* ctx, int32_t is_http);
    void     (*record_texture_done)(void* ctx, int32_t is_http, double byte_count);
    void     (*stat_cache_attempt)(void* ctx);
    void     (*stat_cache_hit)(void* ctx);
    void     (*stat_cache_hit_rate)(void* ctx, int32_t hit);
    void     (*log)(void* ctx, int32_t level, int32_t once, const char* msg, size_t len);
    void     (*add_to_network_queue)(void* ctx);
    void     (*remove_from_network_queue)(void* ctx, int32_t cancel);
    int32_t  (*assemble_packets)(void* ctx, int32_t first_packet, int32_t last_packet);
} VksTexFetchHost;

typedef struct VksTexFetchWorkerInfo
{
    int32_t  state;
    int32_t  write_to_cache_state;
    float    image_priority;
    float    requested_priority;
    int32_t  desired_discard;
    int32_t  requested_discard;
    int32_t  loaded_discard;
    int32_t  decoded_discard;
    int32_t  desired_size;
    int32_t  requested_size;
    int32_t  requested_offset;
    int32_t  file_size;
    int32_t  cached_size;
    int32_t  loaded;
    int32_t  decoded;
    int32_t  written;
    int32_t  needs_aux;
    int32_t  have_all_data;
    int32_t  in_local_cache;
    int32_t  in_cache;
    int32_t  can_use_http;
    int32_t  can_use_net;
    int32_t  retry_attempt;
    int32_t  active_count;
    int32_t  get_status_ok;
    int32_t  get_status_type;
    int32_t  http_active;
    int32_t  http_has_resource;
    int32_t  http_buffer_present;
    int32_t  cache_read_pending;
    int32_t  cache_write_pending;
    uint32_t decode_handle;
    int32_t  image_codec;
    uint32_t cache_read_count;
    uint32_t cache_write_count;
    uint32_t resource_wait_count;
    float    cache_read_time;
    float    decode_time;
    float    cache_write_time;
    float    fetch_time;
    float    skipped_states_time;
    float    state_timers[15];
    int32_t  sent_request;
    int32_t  sim_requested_discard;
    int32_t  total_packets;
    int32_t  last_packet;
} VksTexFetchWorkerInfo;

VksTexFetcher* vks_tfetch_fetcher_new(void);
void           vks_tfetch_fetcher_free(VksTexFetcher* f);
int32_t        vks_tfetch_fetcher_set_pipelined(VksTexFetcher* f, int32_t pipelined);
int32_t        vks_tfetch_fetcher_add_waiter(VksTexFetcher* f, const uint8_t* id);
int32_t        vks_tfetch_fetcher_remove_waiter(VksTexFetcher* f, const uint8_t* id);
int32_t        vks_tfetch_fetcher_is_waiter(VksTexFetcher* f, const uint8_t* id);
int32_t        vks_tfetch_fetcher_waiters_count(VksTexFetcher* f);
int32_t        vks_tfetch_fetcher_cancel_waiters(VksTexFetcher* f);
int32_t        vks_tfetch_fetcher_semaphore(VksTexFetcher* f);
uint32_t       vks_tfetch_fetcher_total_http_requests(VksTexFetcher* f);
int32_t        vks_tfetch_fetcher_update_state_stats(VksTexFetcher* f, uint32_t reads, uint32_t writes, uint32_t waits);
int32_t        vks_tfetch_fetcher_get_state_stats(VksTexFetcher* f, uint32_t* reads, uint32_t* writes, uint32_t* waits);
int32_t        vks_tfetch_fetcher_set_debug(VksTexFetcher* f, int32_t pause, const uint8_t* id);
int32_t        vks_tfetch_fetcher_debug_count(VksTexFetcher* f);
/* releaseHttpWaiters: the waiters to look up (ids, 16 bytes each) and the
 * number to release; then the release order of the found ones by
 * priority. */
size_t         vks_tfetch_fetcher_waiters_to_release(VksTexFetcher* f, int32_t* needed, uint8_t* ids, size_t cap);
void           vks_tfetch_release_order(const float* priorities, size_t n, int32_t needed, size_t* order);

VksTexFetchWorker* vks_tfetch_worker_new(int32_t ftype, const char* url, size_t url_len, const uint8_t* id, int32_t host_is_ok, float priority, int32_t in_second_life);
void     vks_tfetch_worker_free(VksTexFetchWorker* w);
int32_t  vks_tfetch_worker_do_work(VksTexFetchWorker* w, VksTexFetcher* f, const VksTexFetchHost* host);
int32_t  vks_tfetch_worker_state(VksTexFetchWorker* w);
int32_t  vks_tfetch_worker_set_state(VksTexFetchWorker* w, int32_t state);
int32_t  vks_tfetch_worker_set_desired_discard(VksTexFetchWorker* w, int32_t discard, int32_t size, int32_t have_work);
int32_t  vks_tfetch_worker_set_image_priority(VksTexFetchWorker* w, float priority);
int32_t  vks_tfetch_worker_set_can_use_http(VksTexFetchWorker* w, int32_t can);
int32_t  vks_tfetch_worker_set_needs_aux(VksTexFetchWorker* w, int32_t needs);
int32_t  vks_tfetch_worker_set_url(VksTexFetchWorker* w, const char* url, size_t url_len);
size_t   vks_tfetch_worker_url(VksTexFetchWorker* w, char* buf, size_t cap);
int32_t  vks_tfetch_worker_inc_active(VksTexFetchWorker* w);
int32_t  vks_tfetch_worker_end_work(VksTexFetchWorker* w);
int32_t  vks_tfetch_worker_reset_timers(VksTexFetchWorker* w);
float    vks_tfetch_worker_requested_delta_secs(VksTexFetchWorker* w);
float    vks_tfetch_worker_fetch_delta_secs(VksTexFetchWorker* w);
float    vks_tfetch_worker_data_progress(VksTexFetchWorker* w, int32_t formatted_present, int32_t formatted_size);
float    vks_tfetch_worker_requested_priority_for_state(VksTexFetchWorker* w);
int32_t  vks_tfetch_worker_cache_read_done(VksTexFetchWorker* w, int32_t success, int32_t imagesize, int32_t data_size, int32_t codec, int32_t is_local);
int32_t  vks_tfetch_worker_cache_write_done(VksTexFetchWorker* w, int32_t success);
/* The UDP (simulator) path, live with the opensim feature: insertPacket's
 * bookkeeping (the host keeps the bytes), setupPacketData (1 ok, 0 bad
 * cached size: remove from the cache and reset the formatted data, -1
 * cached through HTTP: reset the formatted data), receiveImageHeader's
 * fields and the "sent to the simulator" mark. */
int32_t  vks_tfetch_worker_insert_packet(VksTexFetchWorker* w, int32_t index, uint32_t size);
int32_t  vks_tfetch_worker_setup_packet_data(VksTexFetchWorker* w, int32_t data_size);
int32_t  vks_tfetch_worker_receive_header(VksTexFetchWorker* w, int32_t codec, uint16_t total_packets, int32_t file_size);
int32_t  vks_tfetch_worker_mark_sent_sim(VksTexFetchWorker* w);
int32_t  vks_tfetch_worker_info(VksTexFetchWorker* w, VksTexFetchWorkerInfo* out);
size_t   vks_tfetch_worker_get_reason(VksTexFetchWorker* w, char* buf, size_t cap);
int32_t  vks_tfetch_worker_decode_done(VksTexFetchWorker* w, const VksTexFetchHost* host, int32_t success, int32_t formatted_discard, uint32_t decode_id);
/* onCompleted: the data size, or -1 when the request is being retried. */
int64_t  vks_tfetch_worker_http_completed(VksTexFetchWorker* w, VksTexFetcher* f, const VksTexFetchHost* host, int32_t status_ok, int32_t status_type, const char* reason, size_t reason_len, int32_t partial, int32_t body_size, int32_t has_range, uint32_t range_offset, uint32_t range_length, uint32_t range_full, int32_t has_retry_after, float retry_after);
/* 0 released, 1 not waiting, 2 no slot. */
int32_t  vks_tfetch_worker_try_release(VksTexFetchWorker* w, VksTexFetcher* f);
int32_t  vks_tfetch_worker_on_destroy(VksTexFetchWorker* w, VksTexFetcher* f);
int32_t  vks_tfetch_worker_delete_ok_state(VksTexFetchWorker* w, VksTexFetcher* f, int32_t have_work);
void     vks_tfetch_desired_size(int32_t ftype, int32_t url_non_empty, int32_t url_is_non_j2c, int32_t w, int32_t h, int32_t c, int32_t desired_discard, int32_t* size_out, int32_t* discard_out);
int32_t  vks_tfetch_already_done(int32_t worker_done, int32_t worker_desired_size, int32_t worker_desired_discard, int32_t desired_size, int32_t desired_discard);
size_t   vks_tfetch_state_string(int32_t state, char* buf, size_t cap);
int32_t  vks_j2c_calc_data_size(int32_t w, int32_t h, int32_t comp, int32_t discard_level, float rate);
int32_t  vks_j2c_calc_discard_level_bytes(int32_t w, int32_t h, int32_t comp, int32_t bytes, int32_t reverse_percent);

/* LLAdaptiveRetryPolicy. */
typedef struct VksRetryPolicy VksRetryPolicy;
VksRetryPolicy* vks_retry_policy_new(float min_delay, float max_delay, float backoff_factor, uint32_t max_retries, int32_t retry_on_4xx);
void     vks_retry_policy_free(VksRetryPolicy* p);
int32_t  vks_retry_policy_reset(VksRetryPolicy* p);
int32_t  vks_retry_policy_on_success(VksRetryPolicy* p);
int32_t  vks_retry_policy_on_failure(VksRetryPolicy* p, int32_t status, int32_t has_retry_after, float retry_after);
int32_t  vks_retry_policy_should_retry(VksRetryPolicy* p, float* seconds);
uint32_t vks_retry_policy_retry_count(VksRetryPolicy* p);

/* LLDiskCache. */
typedef struct VksDiskCache VksDiskCache;
VksDiskCache* vks_diskcache_new(const char* dir, size_t dir_len, const char* delim, size_t delim_len, uint64_t max_size, int32_t debug, float high, float low);
void     vks_diskcache_free(VksDiskCache* c);
int32_t  vks_diskcache_purge(VksDiskCache* c);
/* purge with upstream's shutdown early-out: is_running is LLApp::isRunning()
 * (non-zero while running; NULL = always). 1 done, 2 stopped for shutdown
 * (scan not recorded), 0 failed. */
int32_t      vks_diskcache_purge_while(VksDiskCache* c, int32_t (*is_running)(void));
int32_t  vks_diskcache_clear(VksDiskCache* c);
int32_t  vks_diskcache_set_max_size(VksDiskCache* c, uint64_t size);
int32_t  vks_diskcache_set_high_water(VksDiskCache* c, float pct);
int32_t  vks_diskcache_set_low_water(VksDiskCache* c, float pct);
uint64_t vks_diskcache_dir_file_size(VksDiskCache* c, int32_t force);
size_t   vks_diskcache_info(VksDiskCache* c, char* buf, size_t cap);
int32_t  vks_diskcache_skip_list_clear(VksDiskCache* c);
int32_t  vks_diskcache_add_static(VksDiskCache* c, const char* from_folder, size_t from_folder_len, const char* from_file, size_t from_file_len);
size_t   vks_diskcache_filepath(VksDiskCache* c, const uint8_t* id, char* buf, size_t cap);
size_t   vks_diskcache_metadata_to_filepath(const char* dir, size_t dir_len, const char* delim, size_t delim_len, const uint8_t* id, char* buf, size_t cap);
void     vks_diskcache_remove_old_vfs_files(const char* path, size_t len);

/* Mesh assets. */
typedef struct VksMeshHeader
{
    int32_t version;
    int32_t skin_offset;
    int32_t skin_size;
    int32_t skin_in_cache;
    int32_t physics_convex_offset;
    int32_t physics_convex_size;
    int32_t physics_convex_in_cache;
    int32_t physics_mesh_offset;
    int32_t physics_mesh_size;
    int32_t physics_mesh_in_cache;
    int32_t lod_offset[4];
    int32_t lod_size[4];
    int32_t lod_in_cache[4];
    int32_t header_size;
    int32_t not_found;
    uint8_t creator_id[16];
} VksMeshHeader;

typedef struct VksMeshHeaderReceipt
{
    VksMeshHeader header;
    int64_t combined_header_size;
    int32_t data_size;
    int32_t skin_in_data;
    size_t  skin_offset;
    size_t  skin_size;
    int32_t lod_in_data[4];
    size_t  lod_offset[4];
    size_t  lod_size[4];
} VksMeshHeaderReceipt;

typedef struct VksMeshFace
{
    int32_t         num_vertices;
    int32_t         num_indices;
    int32_t         has_weights;
    int32_t         incomplete;
    const float*    positions;      /* num_vertices x 4 */
    const float*    normals;        /* num_vertices x 4 */
    const float*    texcoords;      /* texcoord_count x 2 */
    size_t          texcoord_count;
    const uint16_t* indices;        /* num_indices */
    const float*    weights;        /* num_vertices x 4, when has_weights */
    float           extents[8];
    float           tc_extents[4];
    float           normalized_scale[3];
} VksMeshFace;

typedef struct VksMeshSkinInfo
{
    size_t  joint_count;
    size_t  inv_bind_count;
    size_t  alt_bind_count;
    size_t  bind_pose_count;
    float   pelvis_offset;
    int32_t lock_scale_if_joint_position;
    float   bind_shape_matrix[16];
} VksMeshSkinInfo;

typedef struct VksMeshFaces VksMeshFaces;
typedef struct VksMeshSkin VksMeshSkin;
typedef struct VksMeshDecomp VksMeshDecomp;

int32_t  vks_mesh_header_received(const uint8_t* data, size_t len, uint32_t flags, VksMeshHeaderReceipt* out);
uint32_t vks_mesh_header_flags(const VksMeshHeader* h);
void     vks_mesh_header_set_from_flags(VksMeshHeader* h, uint32_t flags);
int32_t  vks_mesh_header_actual_lod(VksMeshHeader* h, int32_t lod);
int32_t  vks_mesh_header_cache_file_size(const VksMeshHeader* h);
void     vks_mesh_preamble_encode(int32_t header_bytes, uint32_t flags, uint8_t* out);
int32_t  vks_mesh_preamble_decode(const uint8_t* buf, size_t len, uint32_t* version, int32_t* header_size, uint32_t* flags);
int32_t  vks_mesh_looks_unwritten(const uint8_t* buf, size_t len);
size_t   vks_mesh_construct_url(const char* mesh_cap, size_t mesh_cap_len, const char* legacy1, size_t legacy1_len, const char* legacy2, size_t legacy2_len, int32_t legacy_pref_version, const uint8_t* id, char* buf, size_t cap, int32_t* legacy_version);
int32_t  vks_mesh_policy_for(uint32_t len, int32_t legacy_cap_version);
int32_t  vks_mesh_part_request(const VksMeshHeader* h, int32_t kind, int32_t* offset, int32_t* size, int32_t* in_cache, int32_t* disk_offset);
int32_t  vks_mesh_response_body_offset(uint32_t request_offset, uint32_t response_offset, size_t data_size, size_t* out);
VksMeshFaces* vks_mesh_unpack(const uint8_t* data, size_t len, uint8_t sculpt_type, int32_t* err);
void     vks_mesh_faces_free(VksMeshFaces* f);
size_t   vks_mesh_faces_count(const VksMeshFaces* f);
int32_t  vks_mesh_face(const VksMeshFaces* f, size_t i, VksMeshFace* out);
VksMeshSkin* vks_mesh_skin_parse(const uint8_t* data, size_t len);
void     vks_mesh_skin_free(VksMeshSkin* s);
int32_t  vks_mesh_skin_info(const VksMeshSkin* s, VksMeshSkinInfo* out);
size_t   vks_mesh_skin_joint_name(const VksMeshSkin* s, size_t i, const char** out);
int32_t  vks_mesh_skin_matrix(const VksMeshSkin* s, int32_t kind, size_t i, float* out);
VksMeshDecomp* vks_mesh_decomp_parse(const uint8_t* data, size_t len);
void     vks_mesh_decomp_free(VksMeshDecomp* d);
void     vks_mesh_decomp_info(const VksMeshDecomp* d, size_t* hull_count, size_t* base_hull_count, int32_t* base_mesh_cleared);
size_t   vks_mesh_decomp_hull(const VksMeshDecomp* d, size_t i, const float** points);

/* The generic asset fetch. */
typedef struct VksToxicMap VksToxicMap;
typedef struct VksAssetRequestKey
{
    uint8_t  id[16];
    int32_t  asset_type;
    uint64_t callback;
    uint64_t user_data;
    double   time;
} VksAssetRequestKey;

size_t       vks_asset_url(const char* cap_url, size_t cap_len, const char* type_name, size_t type_len, const uint8_t* id, char* buf, size_t cap);
const char*  vks_asset_error_string(int32_t status);
VksToxicMap* vks_toxic_new(void);
void         vks_toxic_free(VksToxicMap* m);
int32_t      vks_toxic_is_toxic(VksToxicMap* m, const uint8_t* id, uint64_t now_us);
void         vks_toxic_flush_old(VksToxicMap* m, uint64_t now_us, int32_t force);
void         vks_toxic_mark(VksToxicMap* m, const uint8_t* id, uint64_t now_us);
int32_t      vks_asset_duplicate_verdict(const VksAssetRequestKey* keys, size_t n, const uint8_t* id, int32_t asset_type, uint64_t callback, uint64_t user_data);
size_t       vks_asset_completion_order(const VksAssetRequestKey* keys, size_t n, const uint8_t* id, int32_t asset_type, size_t* out, size_t cap);
size_t       vks_asset_timed_out_order(const VksAssetRequestKey* keys, size_t n, int32_t is_download_list, int32_t all, double now_secs, double timeout_secs, size_t* out, size_t cap);

/* ---- Audio engine (rung six) -------------------------------------------- */
/* indra/rust/vkstorm-ffi/src/audio.rs over vkstorm-audio: the mixer and
 * output device behind LLAudioEngine_VKS, sound asset decoding for
 * LLAudioDecodeMgr, parcel music for LLStreamingAudio_VKS. Voice handles are
 * the mixer's (0 = none). Strings come back through callbacks that must not
 * throw. */

typedef struct VksAudio VksAudio;
typedef struct VksAudioWav VksAudioWav;

typedef void (*VksAudioStrFn)(void* user, const char* s, size_t len);
typedef void (*VksAudioDeviceFn)(void* user, const uint8_t* uuid, const char* name, size_t name_len);
/* kind 1: artist and title; 0: cleared (empty map). */
typedef void (*VksMusicMetaFn)(void* user, int32_t kind, const char* artist, size_t artist_len,
                               const char* title, size_t title_len);

typedef struct VksAudioFormat
{
    uint32_t rate;
    uint32_t channels;
    uint32_t channel_mask;
    int32_t  headphones;
    uint32_t period_frames;
    uint32_t buffer_frames;
} VksAudioFormat;

typedef struct VksMusicStats
{
    uint64_t pushed_frames;
    uint64_t pulled_frames;
    uint64_t underruns;
    uint32_t reconnects;
    uint64_t buffered_frames;
    uint64_t bus_input_frames;
    uint64_t bus_output_frames;
    uint64_t bus_underrun_frames;
    uint64_t source_frames;
    uint64_t device_frames;
    uint64_t expected_device_frames;
} VksMusicStats;

const char* vks_audio_last_error(void);
/* "SoLoud 202002 (Rust)" and "202002": the engine's name and the SoLoud release its
 * mixer is a port of. Static. */
const char* vks_audio_engine_info(void);
const char* vks_audio_engine_version(void);

/* uuid: 16 bytes (LLUUID::mData) or null / the null UUID for the default endpoint. */
VksAudio* vks_audio_create(const uint8_t* uuid);
VksAudio* vks_audio_create_null(uint32_t rate, uint32_t channels, uint32_t mask, int32_t headphones);
void      vks_audio_destroy(VksAudio* a);
int32_t   vks_audio_restart(VksAudio* a);
/* 0 unchanged, 1 not found (untouched), 2 switched, 3 restored previous, 4 unavailable. */
int32_t   vks_audio_set_device(VksAudio* a, const uint8_t* uuid);
int32_t   vks_audio_devices(VksAudioDeviceFn cb, void* user);
/* name_cb receives backend, a NUL byte, then the device name. */
int32_t   vks_audio_format(VksAudio* a, VksAudioFormat* out, VksAudioStrFn name_cb, void* user);
/* -1 no output, 0 running, 1 detached, 2 lost, 3 failed. */
int32_t   vks_audio_poll(VksAudio* a);
void      vks_audio_set_global_volume(VksAudio* a, float volume);
float     vks_audio_global_volume(VksAudio* a);
int32_t   vks_audio_wind_init(VksAudio* a);
void      vks_audio_wind_cleanup(VksAudio* a);
void      vks_audio_wind_targets(VksAudio* a, float freq, float gain, float pan_gain_r);
uint32_t  vks_audio_render_null(VksAudio* a, float* out, uint32_t frames);
void      vks_audio_set_null_clock(VksAudio* a, uint64_t frames_played);
uint64_t  vks_audio_frames_played(VksAudio* a);

VksAudioWav* vks_audio_wav_load(const char* path, int32_t* missing);
void         vks_audio_wav_free(VksAudio* a, VksAudioWav* w);
uint32_t     vks_audio_wav_sample_count(const VksAudioWav* w);
double       vks_audio_wav_length(const VksAudioWav* w);

uint32_t vks_audio_play(VksAudio* a, const VksAudioWav* w, float volume, int32_t paused);
uint32_t vks_audio_play_3d(VksAudio* a, const VksAudioWav* w, const float* pos, const float* vel,
                           float volume, int32_t paused);
void     vks_audio_voice_set_volume(VksAudio* a, uint32_t h, float volume);
void     vks_audio_voice_set_looping(VksAudio* a, uint32_t h, int32_t looping);
void     vks_audio_voice_set_inaudible_behavior(VksAudio* a, uint32_t h, int32_t must_tick, int32_t kill);
void     vks_audio_voice_set_pause(VksAudio* a, uint32_t h, int32_t pause);
int32_t  vks_audio_voice_seek(VksAudio* a, uint32_t h, double seconds);
void     vks_audio_voice_fade_volume(VksAudio* a, uint32_t h, float to, double seconds);
void     vks_audio_voice_schedule_stop(VksAudio* a, uint32_t h, double seconds);
void     vks_audio_voice_stop(VksAudio* a, uint32_t h);
void     vks_audio_voice_set_protect(VksAudio* a, uint32_t h, int32_t protect);
int32_t  vks_audio_voice_is_valid(VksAudio* a, uint32_t h);
uint32_t vks_audio_voice_loop_count(VksAudio* a, uint32_t h);
double   vks_audio_voice_stream_position(VksAudio* a, uint32_t h);
void     vks_audio_voice_set_doppler_factor(VksAudio* a, uint32_t h, float factor);
void     vks_audio_voice_set_inverse_distance_rolloff(VksAudio* a, uint32_t h, float rolloff);
void     vks_audio_voice_set_3d(VksAudio* a, uint32_t h, const float* pos, const float* vel);
void     vks_audio_set_listener(VksAudio* a, const float* pos, const float* at, const float* up, const float* vel);
void     vks_audio_update_3d(VksAudio* a);
uint32_t vks_audio_voice_count(VksAudio* a);

/* 0 ok (free *out with vks_audio_bytes_free); 1 empty asset, 2 not Ogg, 3 bad channel
 * count, 4 no default bitstream, 5 illegal sample count, 6 illegal sample size,
 * 7 decode error, 8 no samples; *detail = the libvorbis code or offending value. */
int32_t vks_audio_decode_asset(const uint8_t* data, size_t len, uint8_t** out, size_t* out_len, int64_t* detail);
void    vks_audio_bytes_free(uint8_t* p, size_t len);
int32_t vks_audio_asset_vendor(const uint8_t* data, size_t len, VksAudioStrFn cb, void* user);

int32_t  vks_music_create(VksAudio* a, const char* ca_bundle, const char* user_agent, const char* ffmpeg_dir);
void     vks_music_start(VksAudio* a, const char* url);
void     vks_music_stop(VksAudio* a);
void     vks_music_pause(VksAudio* a, int32_t pause);
int32_t  vks_music_is_playing(VksAudio* a);
/* 0 idle, 1 connecting, 2 buffering, 3 playing, 4 paused, 5 ended, 6 failed (reason to cb), 7 stopped. */
int32_t  vks_music_state(VksAudio* a, VksAudioStrFn cb, void* user);
void     vks_music_url(VksAudio* a, VksAudioStrFn cb, void* user);
int32_t  vks_music_poll_metadata(VksAudio* a, VksMusicMetaFn cb, void* user);
int32_t  vks_music_current_metadata(VksAudio* a, VksMusicMetaFn cb, void* user);
void     vks_music_set_volume(VksAudio* a, float volume);
void     vks_music_set_muted(VksAudio* a, int32_t muted);
int32_t  vks_music_set_envelope(VksAudio* a, float value);
/* Returns the transition's serial (non-zero) or 0 if refused. */
uint32_t vks_music_transition(VksAudio* a, float target, float seconds);
/* 0 none (replaced or cancelled), 1 pending, 2 complete. */
int32_t  vks_music_transition_state(VksAudio* a, uint32_t serial);
int32_t  vks_music_set_spatial_layout(VksAudio* a, int32_t layout);
int32_t  vks_music_stats(VksAudio* a, VksMusicStats* out, VksAudioStrFn cb, void* user);

/* ---- The inventory data model (rung seven, sub-rung R1) ---------------- */
/* src/inventory.rs over vkstorm-inventory: LLPermissions,
 * LLAggregatePermissions, LLSaleInfo, the LLInventoryObject, LLInventoryItem
 * and LLInventoryCategory records (CRCs, name and description rules, legacy
 * text and message blocks) and LLNotecard's text. Their LLSD forms (asLLSD,
 * fromLLSD, the category cache import and export, the permission and sale
 * LLSD messages) stay C++ until R2 and R3a stop building C++ LLSD trees:
 * flattening a C++ tree across this ABI cost more than the rules saved.
 *
 * Records.
 * - VksInvPerms, VksInvSale and VksInvAggregate have the exact layout of
 *   LLPermissions, LLSaleInfo and LLAggregatePermissions (no virtual
 *   functions, no base class), so the C++ passes reinterpret_cast<const
 *   VksInvPerms*>(this) with no copy; the calls read and write the object in
 *   place. The C++ classes static_assert the VKS_INV_*_SIZE and
 *   VKS_INV_PERMS_OFFSET_* constants below (offsetof from inside the class),
 *   the Rust side asserts the same numbers.
 * - Items, categories and objects are virtual classes: the C++ fills a
 *   VksInvItem / VksInvCategory / VksInvObject view whose VksInvBytes borrow
 *   its own std::string data for the length of the call.
 * - Results that allocate come back as a VksInvRecords block: read it with
 *   vks_inv_records_view (one struct with every array, the text and the
 *   scalar results; the strings of its records point into the block), free
 *   it with vks_inv_records_free. Null means failure (vks_inv_last_error).
 * - UUID arguments are 16 bytes (LLUUID::mData), copied before a record is
 *   written, so they may point into it; a null pointer is the null UUID.
 *
 * Legacy text. A reader takes the whole REST of the C++ stream as one window
 * (data, len): its end is the stream's end-of-file. The caller has read the
 * block's header line (e.g. "inv_item\t0"). VksInvStream.state holds the
 * stream's rdstate() bits on entry; on return it holds the state afterwards
 * and consumed the bytes read from the window, so the caller clears the
 * stream, seeks to its start position + consumed (or, for a stream it has
 * read whole, continues from window + consumed), then setstate(state). The
 * window is the bytes as the C++ stream delivers them: for a file opened in
 * Windows text mode (llifstream), after CRLF -> LF translation and with the
 * stream ending at a Ctrl-Z; a binary-mode read of the same file is not the
 * same input. The import calls never write the C++ object: the record in the
 * result block is the object afterwards.
 *
 * Message blocks. The C++ keeps the add*Fast/get*Fast field I/O; Rust makes
 * the decisions: VksInvItemBlock is the ItemData wire record (CRC included),
 * VksInvFolderBlock the FolderData one. LLPermissions pack/unpackMessage and
 * LLSaleInfo::packMessage are field I/O on the in-place records and stay C++;
 * vks_inv_sale_unpack_wire and vks_inv_aggregate_from_u8/to_u8 hold the rest.
 *
 * Every call catches panics (null / false / 0 / no change, text in
 * vks_inv_last_error). The in-place permission, sale and aggregate calls do
 * not allocate. */

typedef struct VksInvPerms
{
    uint8_t creator[16];
    uint8_t owner[16];
    uint8_t last_owner[16];
    uint8_t group[16];
    uint32_t base_mask;
    uint32_t owner_mask;
    uint32_t everyone_mask; /* LLPermissions order: everyone before group */
    uint32_t group_mask;
    uint32_t next_owner_mask;
    bool is_group_owned;
} VksInvPerms;

#define VKS_INV_PERMS_SIZE 88
#define VKS_INV_PERMS_OFFSET_CREATOR 0
#define VKS_INV_PERMS_OFFSET_OWNER 16
#define VKS_INV_PERMS_OFFSET_LAST_OWNER 32
#define VKS_INV_PERMS_OFFSET_GROUP 48
#define VKS_INV_PERMS_OFFSET_BASE 64
#define VKS_INV_PERMS_OFFSET_OWNER_MASK 68
#define VKS_INV_PERMS_OFFSET_EVERYONE 72
#define VKS_INV_PERMS_OFFSET_GROUP_MASK 76
#define VKS_INV_PERMS_OFFSET_NEXT_OWNER 80
#define VKS_INV_PERMS_OFFSET_GROUP_OWNED 84

typedef struct VksInvSale
{
    int32_t sale_type; /* LLSaleInfo::EForSale, any value */
    int32_t sale_price;
} VksInvSale;

#define VKS_INV_SALE_SIZE 8

typedef struct VksInvAggregate
{
    uint8_t bits[3]; /* copy, modify, transfer: AP_EMPTY..AP_ALL */
} VksInvAggregate;

#define VKS_INV_AGGREGATE_SIZE 3

typedef struct VksInvBytes
{
    const uint8_t* ptr; /* anything when len is 0 */
    size_t len;
} VksInvBytes;

typedef struct VksInvObject
{
    uint8_t id[16];
    uint8_t parent[16];
    uint8_t thumbnail[16];
    int32_t asset_type; /* LLAssetType::EType, any value */
    bool favorite;
    int64_t creation_date; /* time_t */
    VksInvBytes name;
    VksInvBytes runtime; /* mRuntime, empty for none */
} VksInvObject;

typedef struct VksInvItem
{
    VksInvObject object;
    VksInvPerms perms;
    uint8_t asset[16];
    VksInvSale sale;
    int32_t inv_type; /* LLInventoryType::EType, any value */
    uint32_t flags;
    VksInvBytes description;
} VksInvItem;

/* owner and version: LLViewerInventoryCategory's; no call reads them (the
 * cache LLSD forms stayed C++), the calls carry them through unchanged. */
typedef struct VksInvCategory
{
    VksInvObject object;
    int32_t preferred_type; /* LLFolderType::EType, any value */
    uint8_t owner[16];
    int32_t version;
} VksInvCategory;

#define VKS_INV_STREAM_EOF 1u
#define VKS_INV_STREAM_FAIL 2u
#define VKS_INV_STREAM_BAD 4u

typedef struct VksInvStream
{
    size_t consumed; /* out */
    uint32_t state;  /* in: rdstate(); out: the state afterwards */
} VksInvStream;

/* asset: the AssetID slot, or TransactionID (LLViewerInventoryItem). */
typedef struct VksInvItemBlock
{
    uint8_t item_id[16];
    uint8_t folder_id[16];
    uint8_t creator_id[16];
    uint8_t owner_id[16];
    uint8_t group_id[16];
    uint32_t base_mask;
    uint32_t owner_mask;
    uint32_t group_mask;
    uint32_t everyone_mask;
    uint32_t next_owner_mask;
    bool group_owned;
    int8_t asset_type;
    int8_t inv_type;
    uint8_t sale_type;
    uint8_t asset[16];
    uint32_t flags;
    int32_t sale_price;
    int32_t creation_date;
    uint32_t crc;
    VksInvBytes name;        /* as getStringFast read it */
    VksInvBytes description;
} VksInvItemBlock;

typedef struct VksInvFolderBlock
{
    uint8_t folder_id[16];
    uint8_t parent_id[16];
    int8_t preferred_type;
    VksInvBytes name; /* as getStringFast read it */
} VksInvFolderBlock;

/* 64-bit sizes, asserted on both sides. */
#define VKS_INV_OBJECT_SIZE 96
#define VKS_INV_ITEM_SIZE 232
#define VKS_INV_CATEGORY_SIZE 120
#define VKS_INV_ITEM_BLOCK_SIZE 168
#define VKS_INV_FOLDER_BLOCK_SIZE 56

typedef struct VksInvRecords VksInvRecords;

typedef struct VksInvRecordsView
{
    const VksInvItem* items;
    size_t item_count;
    const VksInvCategory* categories;
    size_t category_count;
    const VksInvObject* objects;
    size_t object_count;
    VksInvBytes text;     /* a legacy export, a notecard's text */
    bool ok;              /* the C++ function's bool result (success, CRC verdict) */
    VksInvStream stream;  /* the legacy readers' stream */
    int32_t version;      /* LLNotecard */
    int32_t embedded_version;
} VksInvRecordsView;

const char* vks_inv_last_error(void);
const VksInvRecordsView* vks_inv_records_view(const VksInvRecords* r);
void vks_inv_records_free(VksInvRecords* r);

/* LLPermissions, in place. allowModifyBy/allowCopyBy/allowMoveBy are
 * allow_operation_by with PERM_MODIFY/PERM_COPY/PERM_MOVE (null group for the
 * one-argument forms). get_ownership leaves its outputs alone when it returns
 * false; safe_owner returns false for an unowned record, where the C++ logs
 * and returns a random UUID itself. */
void vks_inv_perms_init(VksInvPerms* p, const uint8_t* creator, const uint8_t* owner,
                        const uint8_t* last_owner, const uint8_t* group);
void vks_inv_perms_init_masks(VksInvPerms* p, uint32_t base, uint32_t owner, uint32_t everyone,
                              uint32_t group, uint32_t next);
void vks_inv_perms_init_masks_for_type(VksInvPerms* p, int32_t inv_type);
bool vks_inv_perms_get_ownership(const VksInvPerms* p, uint8_t* owner_out, bool* is_group_owned);
bool vks_inv_perms_safe_owner(const VksInvPerms* p, uint8_t* out);
uint32_t vks_inv_perms_crc32(const VksInvPerms* p);
void vks_inv_perms_fix(VksInvPerms* p);
void vks_inv_perms_fix_fair_use(VksInvPerms* p);
void vks_inv_perms_fix_ownership(VksInvPerms* p);
void vks_inv_perms_accumulate(VksInvPerms* p, const VksInvPerms* perm);
bool vks_inv_perms_set_owner_and_group(VksInvPerms* p, const uint8_t* agent, const uint8_t* owner,
                                       const uint8_t* group, bool is_atomic);
void vks_inv_perms_set_last_owner(VksInvPerms* p, const uint8_t* last_owner);
void vks_inv_perms_yes_really_set_owner(VksInvPerms* p, const uint8_t* owner, bool group_owned);
bool vks_inv_perms_deed_to_group(VksInvPerms* p, const uint8_t* agent, const uint8_t* group);
bool vks_inv_perms_set_base_bits(VksInvPerms* p, const uint8_t* agent, bool set, uint32_t bits);
bool vks_inv_perms_set_owner_bits(VksInvPerms* p, const uint8_t* agent, bool set, uint32_t bits);
bool vks_inv_perms_set_group_bits(VksInvPerms* p, const uint8_t* agent, const uint8_t* group, bool set,
                                  uint32_t bits);
bool vks_inv_perms_set_everyone_bits(VksInvPerms* p, const uint8_t* agent, const uint8_t* group, bool set,
                                     uint32_t bits);
bool vks_inv_perms_set_next_owner_bits(VksInvPerms* p, const uint8_t* agent, const uint8_t* group, bool set,
                                       uint32_t bits);
bool vks_inv_perms_allow_operation_by(const VksInvPerms* p, uint32_t op, const uint8_t* requester,
                                      const uint8_t* group);
bool vks_inv_perms_allow_transfer_to(const VksInvPerms* p, const uint8_t* agent);
bool vks_inv_perms_allow_export_by(const VksInvPerms* p, const uint8_t* agent);
/* Defined only in an opensim build. */
bool vks_inv_perms_allow_open_sim_export_by(const VksInvPerms* p, const uint8_t* agent);
bool vks_inv_perms_equal(const VksInvPerms* a, const VksInvPerms* b);
/* mask_to_string into (buf, cap) (see vks_ffi_string); 6 bytes suffice. The
 * export column needs an opensim build and is_open_sim. */
size_t vks_inv_mask_to_string(uint32_t mask, bool is_open_sim, char* buf, size_t cap);

/* LLPermissions: legacy text. import_legacy writes p and stream and returns
 * importLegacyStream's result (false only on a failure, nothing written). */
bool vks_inv_perms_import_legacy(VksInvPerms* p, const uint8_t* data, size_t len, VksInvStream* stream);
VksInvRecords* vks_inv_perms_export_legacy(const VksInvPerms* p);

/* LLAggregatePermissions, in place. */
uint8_t vks_inv_aggregate_value(const VksInvAggregate* a, uint32_t bit);
uint8_t vks_inv_aggregate_to_u8(const VksInvAggregate* a);
void vks_inv_aggregate_from_u8(VksInvAggregate* a, uint8_t byte);
bool vks_inv_aggregate_is_empty(const VksInvAggregate* a);
void vks_inv_aggregate_mask(VksInvAggregate* a, uint32_t mask);
void vks_inv_aggregate_merge(VksInvAggregate* a, const VksInvAggregate* other);

/* LLSaleInfo, in place. unpack_wire: unpackMessage/unpackMultiMessage from
 * the read SaleType (U8) and SalePrice (S32). import_legacy also writes
 * has_perm_mask and perm_mask as the C++ does. */
uint32_t vks_inv_sale_crc32(const VksInvSale* s);
void vks_inv_sale_set_price(VksInvSale* s, int32_t price);
void vks_inv_sale_accumulate(VksInvSale* s, const VksInvSale* other);
void vks_inv_sale_unpack_wire(VksInvSale* s, uint8_t sale_type, int32_t sale_price);
bool vks_inv_sale_import_legacy(VksInvSale* s, const uint8_t* data, size_t len, bool* has_perm_mask,
                                uint32_t* perm_mask, VksInvStream* stream);
VksInvRecords* vks_inv_sale_export_legacy(const VksInvSale* s);

/* Names and descriptions. correct_* write into (buf, cap) as vks_ffi_string
 * expects (a name is at most 63 bytes; a description keeps its length).
 * rename / set_description: true when the field changes, the new value in
 * (buf, cap) and its length in *out_len; VKS_INV_NAME_BUF bytes hold any
 * name, desc_len + 1 any description. */
#define VKS_INV_NAME_BUF 64
size_t vks_inv_correct_name(const uint8_t* name, size_t len, char* buf, size_t cap);
size_t vks_inv_correct_description(const uint8_t* desc, size_t len, char* buf, size_t cap);
bool vks_inv_rename(const uint8_t* current, size_t current_len, const uint8_t* name, size_t name_len, char* buf,
                    size_t cap, size_t* out_len);
bool vks_inv_set_description(const uint8_t* current, size_t current_len, const uint8_t* desc, size_t desc_len,
                             char* buf, size_t cap, size_t* out_len);

/* LLInventoryObject. A null stream is a good one. */
VksInvRecords* vks_inv_object_import_legacy(const VksInvObject* obj, const uint8_t* data, size_t len,
                                            const VksInvStream* stream);
VksInvRecords* vks_inv_object_export_legacy(const VksInvObject* obj);

/* LLInventoryItem. crc32, slam_bits (the flags after
 * accumulatePermissionSlamBits) and to_block read no string and do not
 * allocate. to_block: the block packMessage sends, slot null for the asset
 * id (LLInventoryItem) or the transaction id (LLViewerInventoryItem); its
 * strings borrow the item's. apply_block: unpackMessage of a read block,
 * ok = the CRC verdict. */
uint32_t vks_inv_item_crc32(const VksInvItem* item);
uint32_t vks_inv_item_slam_bits(const VksInvItem* item, const VksInvItem* old_item);
bool vks_inv_item_to_block(const VksInvItem* item, const uint8_t* slot, VksInvItemBlock* out);
VksInvRecords* vks_inv_item_apply_block(const VksInvItem* item, const VksInvItemBlock* block);
VksInvRecords* vks_inv_item_import_legacy(const VksInvItem* item, const uint8_t* data, size_t len,
                                          const VksInvStream* stream);
VksInvRecords* vks_inv_item_export_legacy(const VksInvItem* item, bool include_asset_key);

/* LLInventoryCategory. apply_block: unpackMessage of a read folder block. */
VksInvRecords* vks_inv_category_import_legacy(const VksInvCategory* cat, const uint8_t* data, size_t len,
                                              const VksInvStream* stream);
VksInvRecords* vks_inv_category_export_legacy(const VksInvCategory* cat);
VksInvRecords* vks_inv_category_apply_block(const VksInvCategory* cat, const VksInvFolderBlock* block);

/* LLNotecard. import: importStream of a notecard with mMaxText max_text and
 * the given versions; the result's items are to be appended to mItems (also
 * on failure), version/embedded_version replace the notecard's, and on ok
 * the text replaces mText. export: exportStream of the items and text. */
VksInvRecords* vks_inv_notecard_import(const uint8_t* data, size_t len, int32_t max_text, int32_t version,
                                       int32_t embedded_version, const VksInvStream* stream);
VksInvRecords* vks_inv_notecard_export(const VksInvItem* items, size_t count, const uint8_t* text,
                                       size_t text_len);


/* ---- Inventory R4b: offers and giving (rung seven, sub-rung R4b) ------- */
/* src/inventory_r4b.rs over vkstorm-inventory's exchange module: received
 * inventory offers (LLIMProcessing::processNewMessage for IM_INVENTORY_OFFERED,
 * IM_TASK_INVENTORY_OFFERED, IM_GROUP_NOTICE and IM_GROUP_NOTICE_REQUESTED,
 * inventory_offer_handler, the ReadOfflineMsgs message loop, LLOfferInfo's
 * responses, callbacks and LLSD form, the offer throttle) and giving
 * (LLGiveInventory, the give and drop collect functors and predicates,
 * willObjectAcceptInventory, move_inv_category_world_to_agent,
 * handleGiveDragAndDrop, the dad3dGiveInventory* handlers, the share
 * dialog's callback).
 *
 * Rust decides; the viewer keeps its state and does the work. A call reads
 * the viewer through VksInvR4bHost.query and tells it what to do through
 * VksInvR4bHost.act, one action at a time, in the order the C++ did it. The
 * executor carries each action out before act returns, so the queries after
 * it see its effect (a mute removed, an item deleted). An action may run
 * C++ that calls back into these functions (a name lookup answering at once
 * that shows the share confirmation): every call is re-entrant.
 *
 * The offer a call works on is a C++ LLOfferInfo: the caller passes its
 * fields (VksInvOffer); VKS_INV_R4B_A_OFFER_NEW makes a new one the current
 * offer, VKS_INV_R4B_A_OFFER_UPDATE writes the current offer's fields,
 * VKS_INV_R4B_A_OFFER_DELETE deletes it.
 *
 * Strings are bytes (VksInvBytes, borrowed for the length of the call or of
 * the action). LLSD crosses as binary LLSD without a header (LLSDSerialize::
 * toBinary / fromBinary); an empty VksInvBytes is undefined. UUIDs are 16
 * bytes (LLUUID::mData); a null pointer is the null UUID.
 *
 * Queries. query(ctx, q, out) answers q->code (VKS_INV_R4B_Q_*) with its
 * arguments in q. A scalar answer (bool 0/1, U32, S32, a found flag) is the
 * return value; bytes, records and LLSD go to `out` through
 * vks_inv_r4b_out_* during the query (Rust copies them before the query
 * returns).
 *
 * Every call catches panics (false / ACCEPT_NO / nothing, text in
 * vks_inv_last_error). */

typedef struct VksInvNetHost
{
    uint8_t ip[4]; /* LLHost::mIP as it lies in memory (network order) */
    uint32_t port;
} VksInvNetHost;

/* LLOfferInfo's fields. */
typedef struct VksInvOffer
{
    int32_t im; /* EInstantMessage, any value */
    uint8_t from_id[16];
    bool from_group;
    bool from_object;
    uint8_t transaction_id[16];
    uint8_t folder_id[16];
    uint8_t object_id[16];
    int32_t type; /* LLAssetType::EType, any value */
    VksInvBytes from_name;
    VksInvBytes desc;
    VksInvNetHost host;
    bool persist;
} VksInvOffer;

/* An inventory item as the model holds it (LLViewerInventoryItem): the
 * stored type (getActualType), the stored asset id (a link's target), the
 * stored name and the item's own permissions (the LLInventoryItem members,
 * not the link-following viewer getters), isFinished(). */
typedef struct VksInvR4bItem
{
    uint8_t id[16];
    uint8_t parent[16];
    int32_t type;
    uint8_t asset[16];
    VksInvBytes name;
    VksInvPerms perms;
    bool finished;
} VksInvR4bItem;

/* A category: the preferred type and the stored name. */
typedef struct VksInvR4bCat
{
    uint8_t id[16];
    uint8_t parent[16];
    int32_t preferred;
    VksInvBytes name;
} VksInvR4bCat;

/* LLViewerObject as the drop tests see it; root: getRootEdit()'s id. */
typedef struct VksInvR4bViewerObject
{
    uint8_t id[16];
    bool perm_modify;
    bool flag_allow_inventory_add;
    bool perm_you_owner;
    uint8_t pcode;
    bool is_attachment;
    uint8_t root[16];
} VksInvR4bViewerObject;

/* One entry of an object's inventory; has_perms false for an object that is
 * not an LLInventoryItem. */
typedef struct VksInvR4bTaskObject
{
    uint8_t id[16];
    int32_t type;
    bool has_perms;
    VksInvPerms perms;
} VksInvR4bTaskObject;

/* An instant message.
 * - VKS_INV_R4B_A_SEND_RELIABLE: the ImprovedInstantMessage fields as they
 *   are added; from_agent_name and message hold the exact field data
 *   addStringFast adds (the bytes and a NUL, nothing for an empty
 *   std::string), bucket the BinaryBucket.
 * - VKS_INV_R4B_A_PACK_IM: pack_instant_message's arguments: agent_id is
 *   from_id, the strings as passed, bucket before its truncation, has_bucket
 *   false for the default EMPTY_BINARY_BUCKET.
 * - VKS_INV_R4B_A_POST_TO_MAIN_CORO: processNewMessage's arguments of an
 *   offline message: agent_id is from_id, to_agent_id to_id, the strings and
 *   the bucket as received. */
typedef struct VksInvR4bIm
{
    uint8_t agent_id[16];
    uint8_t session_id[16];
    bool from_group;
    uint8_t to_agent_id[16];
    uint32_t parent_estate_id;
    uint8_t region_id[16];
    float position[3];
    uint8_t offline;
    int32_t dialog;
    uint8_t id[16];
    uint32_t timestamp;
    VksInvBytes from_agent_name;
    VksInvBytes message;
    VksInvBytes bucket;
    bool has_bucket;
} VksInvR4bIm;

/* LLMoveInv: its object, category and (category, item) move list. */
typedef struct VksInvR4bMove
{
    uint8_t object_id[16];
    uint8_t category_id[16];
    const uint8_t* pairs; /* count pairs of two 16-byte ids */
    size_t count;
} VksInvR4bMove;

/* Query codes. Arguments in brackets; "-> ..." answers through out. */
enum
{
    VKS_INV_R4B_Q_AGENT_ID = 1,           /* -> 16 bytes */
    VKS_INV_R4B_Q_AGENT_SESSION_ID,       /* -> 16 bytes */
    VKS_INV_R4B_Q_AGENT_GROUP_ID,         /* gAgent.getGroupID() -> 16 bytes */
    VKS_INV_R4B_Q_AGENT_POSITION,         /* getPositionAgent() -> 3 floats (12 bytes) */
    VKS_INV_R4B_Q_AGENT_REGION_HOST,      /* -> a VksInvNetHost (8 bytes) */
    VKS_INV_R4B_Q_IS_DO_NOT_DISTURB,
    VKS_INV_R4B_Q_HAVE_AVATAR,            /* gAgentAvatarp != NULL */
    VKS_INV_R4B_Q_AVATAR_VALID,           /* isAgentAvatarValid() */
    VKS_INV_R4B_Q_AVATAR_FULLNAME,        /* -> bytes */
    VKS_INV_R4B_Q_IS_WEARING_ATTACHMENT,  /* [id] gAgentAvatarp->isWearingAttachment */
    VKS_INV_R4B_Q_IS_WEARING_ITEM,        /* [id] gAgentWearables.isWearingItem */
    VKS_INV_R4B_Q_IS_GESTURE_ACTIVE,      /* [id] */
    VKS_INV_R4B_Q_AVATAR_SITTING,
    VKS_INV_R4B_Q_AVATAR_ROOT,            /* found -> 16 bytes (gAgentAvatarp->getRoot()'s id) */
    VKS_INV_R4B_Q_SETTING_BOOL,           /* [text name] */
    VKS_INV_R4B_Q_SETTING_U32,            /* [text name] */
    VKS_INV_R4B_Q_AUTORESPONSE_TEXT,      /* [id from] -> bytes */
    VKS_INV_R4B_Q_TRANS,                  /* [text key, list] LLTrans::getString -> bytes */
    VKS_INV_R4B_Q_SLURL_LOCATION,         /* [text url] LLSLURL(url).getLocationString() -> bytes */
    VKS_INV_R4B_Q_RLV_ANONYM,             /* [text name] RlvStrings::getAnonym(name) -> bytes */
    VKS_INV_R4B_Q_RLV_ANONYM_AVATAR,      /* [id] found -> bytes (the cached avatar name's anonym) */
    VKS_INV_R4B_Q_SLURL,                  /* [text command, id, text2 verb] getSLURLString() -> bytes */
    VKS_INV_R4B_Q_APP_TITLE,              /* getSecondLifeTitle() -> bytes */
    VKS_INV_R4B_Q_TRIM_RESIDENT_SURNAME,  /* LLAvatarName::trimResidentSurname() */
    VKS_INV_R4B_Q_RLV_ENABLED,
    VKS_INV_R4B_Q_RLV_CAN_SHOW_NAME,      /* [id] RlvActions::canShowName(SNC_DEFAULT, id) */
    VKS_INV_R4B_Q_RLV_CAN_GIVE_INVENTORY, /* [id] */
    VKS_INV_R4B_Q_RLV_IS_NEARBY,          /* [id] RlvUtil::isNearbyAgent */
    VKS_INV_R4B_Q_RLV_HAS_OPEN_IM,        /* [id] RlvUIEnabler::hasOpenIM */
    VKS_INV_R4B_Q_RLV_HAS_OPEN_PROFILE,   /* [id] RlvUIEnabler::hasOpenProfile */
    VKS_INV_R4B_Q_RLV_FORBID_GIVE_TO_RLV, /* RlvSettings::getForbidGiveToRLV() */
    VKS_INV_R4B_Q_RLV_HAS_SHARED_ROOT,    /* RlvInventory::getSharedRoot() != NULL */
    VKS_INV_R4B_Q_RLV_SHARED_ROOT_ID,     /* -> 16 bytes */
    VKS_INV_R4B_Q_RLV_LOCKED_ATTACHMENT,  /* [id root object] gRlvAttachmentLocks.isLockedAttachment */
    VKS_INV_R4B_Q_RLV_HAS_BEHAVIOUR,      /* [number: 0 unsit, 1 sittp] */
    VKS_INV_R4B_Q_IS_MUTED,               /* [id, text name, number flags] LLMuteList::isMuted */
    VKS_INV_R4B_Q_AVATAR_USER_NAME,       /* [id] found -> bytes (LLAvatarNameCache::get, getUserName) */
    VKS_INV_R4B_Q_AVATAR_COMPLETE_NAME,   /* [id] found -> bytes (getCompleteName) */
    VKS_INV_R4B_Q_GROUP_NAME,             /* [id] found -> bytes (gCacheName->getGroupName) */
    VKS_INV_R4B_Q_FIND_ID_BY_NAME,        /* [text legacy name] -> 16 bytes */
    VKS_INV_R4B_Q_NOTIFICATION_LIVE,      /* [id] LLNotifications::find(id) != NULL */
    VKS_INV_R4B_Q_NOTIFICATION_FORM,      /* [text template] found -> binary LLSD (the form's asLLSD) */
    VKS_INV_R4B_Q_ITEM,                   /* [id] found -> one VksInvR4bItem */
    VKS_INV_R4B_Q_CATEGORY,               /* [id] found -> one VksInvR4bCat */
    VKS_INV_R4B_Q_CHILDREN,               /* [id parent] -> its categories, then its items, in model order */
    VKS_INV_R4B_Q_IS_CATEGORY_COMPLETE,   /* [id] */
    VKS_INV_R4B_Q_FIND_CATEGORY_FOR_TYPE, /* [number LLFolderType] -> 16 bytes */
    VKS_INV_R4B_Q_GENERATE_UUID,          /* LLUUID::generate() -> 16 bytes */
    VKS_INV_R4B_Q_NOW,                    /* LLDate::now() -> one double (8 bytes) */
    VKS_INV_R4B_Q_FRAME_TIME,             /* the frame timer's clock (seconds) -> one double */
    VKS_INV_R4B_Q_STARTUP_STARTED,        /* getStartupState() >= STATE_STARTED */
    VKS_INV_R4B_Q_IM_SESSION_NAME,        /* [id] found -> bytes (LLIMModel::findIMSession's mName) */
    VKS_INV_R4B_Q_MAX_ITEMS_TRANSFER,     /* LLWorld::getMaxInventoryItemsTransfer() */
    VKS_INV_R4B_Q_OBJECT,                 /* [id] found -> one VksInvR4bViewerObject */
    VKS_INV_R4B_Q_OBJECT_INVENTORY,       /* [id] -> every VksInvR4bTaskObject of its inventory, in order */
    VKS_INV_R4B_Q_HAVE_FLOATER_TOOLS,     /* gFloaterTools != NULL */
    VKS_INV_R4B_Q_OBSERVER_NEW,           /* [number kind (VKS_INV_R4B_OBS_*), id, id2, text from_name,
                                             flag manual] new <observer> -> its handle (>= 0) */
    VKS_INV_R4B_Q_OBSERVER_START,         /* [number handle] startFetch(); isFinished() */
    VKS_INV_R4B_Q_FILTER_CHECK,           /* [number index into VKS_INV_R4B_Q_OBJECT_INVENTORY's list]
                                             world_to_agent's filter: check(item) */
};

/* Observer kinds (VKS_INV_R4B_Q_OBSERVER_NEW). */
enum
{
    VKS_INV_R4B_OBS_FETCH_ITEMS = 0,       /* LLInventoryFetchItemsObserver(id) */
    VKS_INV_R4B_OBS_OPEN_AGENT_OFFER,      /* LLOpenAgentOffer(id, from_name, manual) */
    VKS_INV_R4B_OBS_DISCARD_AGENT_OFFER,   /* LLDiscardAgentOffer(folder id, object id2) */
    VKS_INV_R4B_OBS_RLV_AGENT_OFFER,       /* RlvGiveToRLVAgentOffer(folder id) */
    VKS_INV_R4B_OBS_RLV_TASK_OFFER,        /* RlvGiveToRLVTaskOffer(transaction id) */
    VKS_INV_R4B_OBS_OPEN_TASK_GROUP_OFFER, /* LLOpenTaskGroupOffer */
};

typedef struct VksInvR4bQuery
{
    uint32_t code;
    const uint8_t* id;
    const uint8_t* id2;
    VksInvBytes text;
    VksInvBytes text2;
    int32_t number;
    bool flag;
    const VksInvBytes* list; /* VKS_INV_R4B_Q_TRANS: key, value, key, value... (keys sorted) */
    size_t list_len;         /* entries: twice the pairs */
} VksInvR4bQuery;

typedef struct VksInvR4bOut VksInvR4bOut;
void vks_inv_r4b_out_bytes(VksInvR4bOut* out, const uint8_t* data, size_t len);
void vks_inv_r4b_out_item(VksInvR4bOut* out, const VksInvR4bItem* item);
void vks_inv_r4b_out_cat(VksInvR4bOut* out, const VksInvR4bCat* cat);
void vks_inv_r4b_out_object(VksInvR4bOut* out, const VksInvR4bViewerObject* obj);
void vks_inv_r4b_out_task_object(VksInvR4bOut* out, const VksInvR4bTaskObject* obj);

/* Action kinds, with the slots each uses. */
enum
{
    VKS_INV_R4B_A_OFFER_NEW = 1,           /* offer: new LLOfferInfo, now the current offer */
    VKS_INV_R4B_A_OFFER_UPDATE,            /* offer: the current offer's fields */
    VKS_INV_R4B_A_OFFER_DELETE,            /* delete the current offer */
    VKS_INV_R4B_A_SEND_RELIABLE,           /* im; number[0] 0: msg->sendReliable(host),
                                              1: gAgent.sendReliableMessage() */
    VKS_INV_R4B_A_PACK_IM,                 /* im: pack_instant_message(gMessageSystem, ...) */
    VKS_INV_R4B_A_SEND_PACKED_TO_REGION,   /* gAgent.sendReliableMessage() */
    VKS_INV_R4B_A_NOTIFY,                  /* text[0] name, sd[0] substitutions, sd[1] payload,
                                              number[0] handler (VKS_INV_R4B_H_*):
                                              LLNotificationsUtil::add */
    VKS_INV_R4B_A_NOTIFY_POSTPONED,        /* text[0] name, sd[0], sd[1], flag[0] offer_from_agent,
                                              id[0] from, flag[1] is_group; the current offer is
                                              the responder (LLPostponedOfferNotification) */
    VKS_INV_R4B_A_FORCE_RESPOND,           /* text[0] name, sd[0] response: the temporary
                                              notification of LLNotifications::forceResponse */
    VKS_INV_R4B_A_AVATAR_NAME_LOOKUP,      /* id[0], then: LLAvatarNameCache::get(id, callback) */
    VKS_INV_R4B_A_GROUP_NAME_LOOKUP,       /* id[0], then: gCacheName->getGroup(id, callback) */
    VKS_INV_R4B_A_CACHE_NAME_GET,          /* id[0], flag[0] is_group, then: gCacheName->get */
    VKS_INV_R4B_A_OBSERVER_DONE,           /* number[0] handle: observer->done() */
    VKS_INV_R4B_A_OBSERVER_ADD,            /* number[0] handle: gInventory.addObserver(observer) */
    VKS_INV_R4B_A_OBSERVER_DROP,           /* number[0] handle: delete observer */
    VKS_INV_R4B_A_RECENT_PEOPLE_ADD,       /* id[0] */
    VKS_INV_R4B_A_FIRST_USE_NEW_INVENTORY,
    VKS_INV_R4B_A_NEARBY_CHAT,             /* text[0]: FSCommon::report_to_nearby_chat */
    VKS_INV_R4B_A_UI_SOUND,                /* text[0]: make_ui_sound */
    VKS_INV_R4B_A_INVENTORY_PANEL_SELECT,  /* flag[0] auto_open, id[0] */
    VKS_INV_R4B_A_GROUP_PANEL_SHOW_NOTICE, /* text[0] subject, text[1] message, id[0] group,
                                              flag[0] has_inventory, text[2] inventory name,
                                              flag[1]: the current offer goes to the panel */
    VKS_INV_R4B_A_RLV_SEND_NOTIFICATION,   /* text[0] */
    VKS_INV_R4B_A_RLV_NOTIFY_BLOCKED_SHARE,/* sd[0] args */
    VKS_INV_R4B_A_POST_TO_MAIN_CORO,       /* im, host sender, sd[0] metadata, id[0] aux id:
                                              postToMainCoro(processNewMessage(...)) */
    VKS_INV_R4B_A_HUD_BEAM,                /* id[0] target */
    VKS_INV_R4B_A_FLOATER_TOOLS_DIRTY,
    VKS_INV_R4B_A_MUTE_AUTO_REMOVE,        /* id[0], number[0] reason: LLMuteList::autoRemove */
    VKS_INV_R4B_A_IM_SYSTEM_MESSAGE,       /* id[0] session, text[0] name, sd[0] args */
    VKS_INV_R4B_A_IM_LOG_TO_FILE,          /* text[0] file, text[1] from, id[0], text[2] template,
                                              sd[0] args */
    VKS_INV_R4B_A_DELETE_OBJECT,           /* id[0]: gInventory.deleteObject */
    VKS_INV_R4B_A_NOTIFY_OBSERVERS,
    VKS_INV_R4B_A_FLOATER_HIDE,            /* text[0] */
    VKS_INV_R4B_A_WARN_MOVE_INVENTORY,     /* move */
    VKS_INV_R4B_A_MOVE_TASK_INVENTORY,     /* sd[0] notification, sd[1] response, move */
    VKS_INV_R4B_A_LOG,                     /* number[0] level (0 LL_DEBUGS, 1 LL_INFOS, 2 LL_WARNS),
                                              text[0] tag (empty: untagged), text[1] message:
                                              the C++ log line at the point it logged */
};

/* What runs when a name lookup answers (number[1] of the lookups). */
enum
{
    VKS_INV_R4B_THEN_OFFER_MUTE_AVATAR = 0, /* inventory_offer_mute_avatar_callback */
    VKS_INV_R4B_THEN_OFFER_MUTE_GROUP,      /* inventory_offer_mute_callback */
    VKS_INV_R4B_THEN_SYSTEM_MESSAGE_TIP,    /* text[1] format, text[2] message: a SystemMessageTip
                                               of llformat(format, name) + message */
    VKS_INV_R4B_THEN_GROUP_NOTICE,          /* text[1] sender, text[2] subject, text[3] message,
                                               sd[1] payload, number[2] timestamp:
                                               notification_group_name_cb */
    VKS_INV_R4B_THEN_SHARE_CONFIRMATION,    /* id[1] item, text[1] item name, text[2] dest:
                                               vks_inv_r4b_share_confirmation with the name */
    VKS_INV_R4B_THEN_AUTO_UNMUTE_NOTICE,    /* number[2] reason: notify_automute_callback */
};

/* Notification response handlers (number[0] of VKS_INV_R4B_A_NOTIFY). */
enum
{
    VKS_INV_R4B_H_NONE = 0,
    VKS_INV_R4B_H_COPY_PROTECTED_ITEM,     /* LLGiveInventory::handleCopyProtectedItem */
    VKS_INV_R4B_H_COPY_PROTECTED_CATEGORY, /* LLGiveInventory::handleCopyProtectedCategory */
    VKS_INV_R4B_H_DRAG_GIVE_INVENTORY,     /* give_inventory_cb (lltooldraganddrop.cpp) */
};

typedef struct VksInvR4bAction
{
    uint32_t kind;
    int32_t number[3];
    bool flag[2];
    uint8_t id[2][16];
    VksInvBytes text[4];
    VksInvBytes sd[2];
    const VksInvOffer* offer;
    const VksInvR4bIm* im;
    VksInvNetHost host;
    const VksInvR4bMove* move;
} VksInvR4bAction;

/* The offer throttle's state (gThrottleTimer's expiry on the frame clock and
 * check_offer_throttle's statics), owned by the caller. */
typedef struct VksInvR4bThrottle
{
    double expiry;
    uint32_t count;
    bool logged;
} VksInvR4bThrottle;

typedef struct VksInvR4bHost
{
    void* ctx;
    int64_t (*query)(void* ctx, const VksInvR4bQuery* q, VksInvR4bOut* out);
    void (*act)(void* ctx, const VksInvR4bAction* a);
    VksInvR4bThrottle* throttle; /* read and written by check_offer_throttle */
} VksInvR4bHost;

/* Offers (W6). receive_im: processNewMessage's case for dialogs 4, 9, 32
 * and 37 (other dialogs do nothing), from its raw arguments: name as
 * received; buffer the bytes the bucket was copied into (process_improved_im's
 * U8[MTUBYTES], or the offline message's vector; the legacy C-string reads
 * stop at its end); bucket_size the declared size. Returns whether the case
 * ends with its break (true) or returns from processNewMessage (false: a
 * malformed agent offer or group notice bucket, a notice from a muted
 * sender), skipping what follows the dialog switch. */
bool vks_inv_r4b_receive_im(const VksInvR4bHost* host, const uint8_t* from_id, bool from_group, int32_t dialog,
                            const uint8_t* session_id, uint32_t timestamp, const uint8_t* name, size_t name_len,
                            const uint8_t* message, size_t message_len, const uint8_t* buffer, size_t buffer_len,
                            int32_t bucket_size, const VksInvNetHost* sender, const uint8_t* aux_id);
/* The ReadOfflineMsgs loop over its validated messages array (binary LLSD). */
void vks_inv_r4b_receive_offline(const VksInvR4bHost* host, const uint8_t* messages, size_t len);
/* LLOfferInfo: `offer` is the call's current offer. */
void vks_inv_r4b_respond(const VksInvR4bHost* host, const VksInvOffer* offer, const uint8_t* notification,
                         size_t notification_len, const uint8_t* response, size_t response_len);
void vks_inv_r4b_force_response(const VksInvR4bHost* host, const VksInvOffer* offer, int32_t response);
void vks_inv_r4b_send_receive_response(const VksInvR4bHost* host, const VksInvOffer* offer, bool accept,
                                       const uint8_t* folder);
void vks_inv_r4b_send_auto_receive_response(const VksInvR4bHost* host, const VksInvOffer* offer);
void vks_inv_r4b_send_decline_response(const VksInvR4bHost* host, const VksInvOffer* offer);
/* LLOfferInfo's LLSD form (asLLSD, LLOfferInfo(LLSD), fromLLSD) stays C++
 * until the inventory storage moves to Rust (owner's decision: crossing the
 * LLSD cost 3x the C++); the crate keeps its logic (exchange::info). */
bool vks_inv_r4b_check_offer_throttle(const VksInvR4bHost* host, const uint8_t* from_name, size_t len,
                                      bool check_only);

/* Giving (W7). A null record pointer is a null C++ pointer. */
bool vks_inv_r4b_give_item(const VksInvR4bHost* host, const uint8_t* to, const VksInvR4bItem* item,
                           const uint8_t* im_session);
bool vks_inv_r4b_give_category(const VksInvR4bHost* host, const uint8_t* to, const VksInvR4bCat* cat,
                               const uint8_t* im_session, const uint8_t* success_notification, size_t len);
/* The answer to a notification posted with a handler (VKS_INV_R4B_H_*):
 * the handler's result (false for give_inventory_cb, which has none). */
bool vks_inv_r4b_answer(const VksInvR4bHost* host, int32_t handler, const uint8_t* notification,
                        size_t notification_len, const uint8_t* response, size_t response_len);
/* The share dialog's callback; avatars and selected are arrays of 16-byte ids. */
void vks_inv_r4b_share(const VksInvR4bHost* host, const uint8_t* avatars, size_t avatar_count,
                       const uint8_t* notification, size_t notification_len, const uint8_t* response,
                       size_t response_len, const uint8_t* selected, size_t selected_count);
bool vks_inv_r4b_is_give_acceptable(const VksInvR4bHost* host, const VksInvR4bItem* item);
bool vks_inv_r4b_is_group_give_acceptable(const VksInvR4bHost* host, const VksInvR4bItem* item);
bool vks_inv_r4b_transfer_commonly_allowed(const VksInvR4bHost* host, const VksInvR4bItem* item);
/* The give and drop collect functors; *count: countNoCopy() for the
 * counting ones, -1 for the others. */
enum
{
    VKS_INV_R4B_F_GIVEABLE = 0,
    VKS_INV_R4B_F_UNCOPYABLE,
    VKS_INV_R4B_F_DROPPABLE_TRANSFER,
    VKS_INV_R4B_F_DROPPABLE_NO_TRANSFER,
    VKS_INV_R4B_F_DROP_COPYABLE,
};
bool vks_inv_r4b_collect_functor(const VksInvR4bHost* host, int32_t functor, const VksInvR4bCat* cat,
                                 const VksInvR4bItem* item, int32_t* count);
/* EAcceptance values. */
int32_t vks_inv_r4b_will_object_accept(const VksInvR4bHost* host, const VksInvR4bViewerObject* obj,
                                       const VksInvR4bItem* item, int32_t dad_type);
/* filter_types: the filter's getFilterTypes(), or -1 for no filter (then
 * VKS_INV_R4B_Q_FILTER_CHECK is never asked). */
bool vks_inv_r4b_world_to_agent(const VksInvR4bHost* host, const uint8_t* object_id, const uint8_t* category_id,
                                bool drop, int64_t filter_types);
/* handleGiveDragAndDrop: the cargo is the item or the category (one of them
 * non-null) the cargo type names; dest is binary LLSD; *accept gets the
 * EAcceptance. */
bool vks_inv_r4b_give_drag_and_drop(const VksInvR4bHost* host, const uint8_t* dest_agent, const uint8_t* session,
                                    bool drop, int32_t cargo_type, const VksInvR4bItem* item,
                                    const VksInvR4bCat* cat, const uint8_t* dest, size_t dest_len,
                                    int32_t* accept);
/* dad3dGiveInventoryObject (kind 0), dad3dGiveInventory (1),
 * dad3dGiveInventoryCategory (2): source the tool's ESource, item and cat
 * locateInventory's answer, obj the avatar's object id (null for none). */
int32_t vks_inv_r4b_dad3d_give(const VksInvR4bHost* host, int32_t kind, int32_t source, const VksInvR4bItem* item,
                               const VksInvR4bCat* cat, const uint8_t* obj, bool drop);
/* show_object_sharing_confirmation (get_name_cb's body), for
 * VKS_INV_R4B_THEN_SHARE_CONFIRMATION once the name is known. */
void vks_inv_r4b_share_confirmation(const VksInvR4bHost* host, const uint8_t* name, size_t name_len,
                                    const uint8_t* item_id, const uint8_t* item_name, size_t item_name_len,
                                    const uint8_t* dest, size_t dest_len, const uint8_t* dest_agent,
                                    const uint8_t* session);

/* 64-bit sizes, asserted on both sides. */
#define VKS_INV_NET_HOST_SIZE 8
#define VKS_INV_OFFER_SIZE 128
#define VKS_INV_R4B_ITEM_SIZE 168
#define VKS_INV_R4B_CAT_SIZE 56
#define VKS_INV_R4B_VIEWER_OBJECT_SIZE 37
#define VKS_INV_R4B_TASK_OBJECT_SIZE 112
#define VKS_INV_R4B_IM_SIZE 168
#define VKS_INV_R4B_MOVE_SIZE 48
#define VKS_INV_R4B_QUERY_SIZE 80
#define VKS_INV_R4B_ACTION_SIZE 184
#define VKS_INV_R4B_THROTTLE_SIZE 16
#define VKS_INV_R4B_HOST_SIZE 32

#ifdef __cplusplus
}

#include <string>
#include <vector>

/* The inventory records' layout (the Rust side asserts the same numbers). */
static_assert(sizeof(VksInvPerms) == VKS_INV_PERMS_SIZE && alignof(VksInvPerms) == 4, "VksInvPerms layout");
static_assert(offsetof(VksInvPerms, owner) == VKS_INV_PERMS_OFFSET_OWNER &&
              offsetof(VksInvPerms, last_owner) == VKS_INV_PERMS_OFFSET_LAST_OWNER &&
              offsetof(VksInvPerms, group) == VKS_INV_PERMS_OFFSET_GROUP &&
              offsetof(VksInvPerms, base_mask) == VKS_INV_PERMS_OFFSET_BASE &&
              offsetof(VksInvPerms, owner_mask) == VKS_INV_PERMS_OFFSET_OWNER_MASK &&
              offsetof(VksInvPerms, everyone_mask) == VKS_INV_PERMS_OFFSET_EVERYONE &&
              offsetof(VksInvPerms, group_mask) == VKS_INV_PERMS_OFFSET_GROUP_MASK &&
              offsetof(VksInvPerms, next_owner_mask) == VKS_INV_PERMS_OFFSET_NEXT_OWNER &&
              offsetof(VksInvPerms, is_group_owned) == VKS_INV_PERMS_OFFSET_GROUP_OWNED,
              "VksInvPerms offsets");
static_assert(sizeof(VksInvSale) == VKS_INV_SALE_SIZE && sizeof(VksInvAggregate) == VKS_INV_AGGREGATE_SIZE,
              "VksInvSale, VksInvAggregate layout");
static_assert(sizeof(bool) == 1, "bool is one byte in the inventory records");
static_assert(sizeof(void*) != 8 ||
                  (sizeof(VksInvObject) == VKS_INV_OBJECT_SIZE && sizeof(VksInvItem) == VKS_INV_ITEM_SIZE &&
                   sizeof(VksInvCategory) == VKS_INV_CATEGORY_SIZE &&
                   sizeof(VksInvItemBlock) == VKS_INV_ITEM_BLOCK_SIZE &&
                   sizeof(VksInvFolderBlock) == VKS_INV_FOLDER_BLOCK_SIZE),
              "inventory view layout");
static_assert(sizeof(void*) != 8 ||
                  (sizeof(VksInvNetHost) == VKS_INV_NET_HOST_SIZE && sizeof(VksInvOffer) == VKS_INV_OFFER_SIZE &&
                   sizeof(VksInvR4bItem) == VKS_INV_R4B_ITEM_SIZE && sizeof(VksInvR4bCat) == VKS_INV_R4B_CAT_SIZE &&
                   sizeof(VksInvR4bViewerObject) == VKS_INV_R4B_VIEWER_OBJECT_SIZE &&
                   sizeof(VksInvR4bTaskObject) == VKS_INV_R4B_TASK_OBJECT_SIZE &&
                   sizeof(VksInvR4bIm) == VKS_INV_R4B_IM_SIZE && sizeof(VksInvR4bMove) == VKS_INV_R4B_MOVE_SIZE &&
                   sizeof(VksInvR4bQuery) == VKS_INV_R4B_QUERY_SIZE &&
                   sizeof(VksInvR4bAction) == VKS_INV_R4B_ACTION_SIZE &&
                   sizeof(VksInvR4bThrottle) == VKS_INV_R4B_THROTTLE_SIZE &&
                   sizeof(VksInvR4bHost) == VKS_INV_R4B_HOST_SIZE),
              "inventory R4b layout");

/* The string a vks_* function writes into (buf, cap). Those functions write
 * at most cap - 1 bytes and a NUL and return the string's full length;
 * vks_ffi_string calls fill(buf, cap) and calls it again with a buffer of
 * the full length when the first was too small, so no result is cut. */
template <typename Fill>
inline std::string vks_ffi_string(Fill fill)
{
    char small[256];
    size_t len = fill(small, sizeof(small));
    if (len < sizeof(small))
    {
        return std::string(small, len);
    }
    std::vector<char> big;
    for (;;)
    {
        big.resize(len + 1);
        const size_t again = fill(big.data(), big.size());
        if (again < big.size())
        {
            return std::string(big.data(), again);
        }
        len = again;
    }
}
#endif

#endif /* VKSTORM_FFI_H */
