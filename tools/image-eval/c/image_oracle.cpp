// image_oracle: the viewer's C++ raster codecs, standalone, as the parity
// oracle and the C++ side of the benchmarks for crates/vkstorm-image.
//
// PNG and JPEG are LLPngWrapper and LLImageJPEG verbatim on the viewer's
// prebuilt libpng 1.6.44 (zlib-ng 2.2.3) and libjpeg-turbo 3.0.4. TGA, BMP
// and DXT are the viewer's own code (LLImageTGA, LLImageBMP, LLImageDXT,
// LLImageBase::generateMip) copied with a minimal LLImageRaw stand-in. Where
// the viewer reads outside its buffers on malformed input the oracle is given
// well-formed input only.
//
// Raw rasters on disk: "VKSR", u32 width, u32 height, u32 components, then
// width*height*components bytes, rows bottom-up (LLImageRaw order).
//
// usage:
//   image_oracle <codec>-decode  in out.raw            codec: png jpeg tga bmp dxt
//   image_oracle <codec>-encode  in.raw out [quality]
//   image_oracle tga-process in.tga out.raw domain weight
//   image_oracle png-header in | jpeg-header in | jpeg-dims in
//   image_oracle crc in                 (decode by extension; prints crc32 w h c)
//   image_oracle bench <codec> decode|encode|header in iters [quality]
// Failures print "FAIL: <message>" (the viewer's setLastError text) and exit 1.

#include <png.h>
#include <jpeglib.h>
#include <jerror.h>
#include <zlib.h>

#include <algorithm>
#include <chrono>
#include <csetjmp>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <stdexcept>
#include <string>
#include <vector>

typedef uint8_t U8; typedef uint16_t U16; typedef uint32_t U32; typedef int8_t S8; typedef int16_t S16; typedef int32_t S32; typedef float F32; typedef double F64;
template <class T> T llmax(T a, T b) { return a > b ? a : b; }
template <class T> T llmin(T a, T b) { return a < b ? a : b; }
template <class T> T llclamp(T a, T lo, T hi) { return a < lo ? lo : (a > hi ? hi : a); }
static F32 llclampf(F32 a) { return llmin(llmax(a, 0.f), 1.f); }
static F32 llclampb(F32 a) { return llmin(llmax(a, 0.f), 255.f); }

static std::string g_error;
static void setLastError(const std::string& m) { g_error = m; }

// ---------------------------------------------------------------------------
// LLImageRaw stand-in
// ---------------------------------------------------------------------------
struct Raster
{
    U32 w = 0, h = 0, c = 0;
    std::vector<U8> data;
    bool resize(U32 w_, U32 h_, U32 c_) { w = w_; h = h_; c = c_; data.assign((size_t)w * h * c, 0); return true; }
    U8* getData() { return data.data(); }
    const U8* getData() const { return data.data(); }
    U32 getWidth() const { return w; }
    U32 getHeight() const { return h; }
    U32 getComponents() const { return c; }
    S32 getDataSize() const { return (S32)data.size(); }
    void verticalFlip()
    {
        size_t row = (size_t)w * c;
        std::vector<U8> tmp(row);
        for (U32 y = 0; y < h / 2; ++y)
        {
            memcpy(tmp.data(), &data[y * row], row);
            memcpy(&data[y * row], &data[(h - 1 - y) * row], row);
            memcpy(&data[(h - 1 - y) * row], tmp.data(), row);
        }
    }
    // LLImageRaw::copy for same size, 4 -> 3 components (copyUnscaled4onto3)
    void dropAlpha()
    {
        std::vector<U8> out((size_t)w * h * 3);
        for (size_t i = 0; i < (size_t)w * h; ++i) { out[i * 3] = data[i * 4]; out[i * 3 + 1] = data[i * 4 + 1]; out[i * 3 + 2] = data[i * 4 + 2]; }
        data.swap(out); c = 3;
    }
};

static bool readFile(const char* path, std::vector<U8>& out)
{
    FILE* f = fopen(path, "rb"); if (!f) return false;
    fseek(f, 0, SEEK_END); long n = ftell(f); fseek(f, 0, SEEK_SET);
    out.resize(n > 0 ? (size_t)n : 0);
    bool ok = n <= 0 || fread(out.data(), 1, (size_t)n, f) == (size_t)n;
    fclose(f); return ok;
}
static bool writeFile(const char* path, const void* p, size_t n)
{
    FILE* f = fopen(path, "wb"); if (!f) return false;
    bool ok = fwrite(p, 1, n, f) == n; fclose(f); return ok;
}
static bool readRaw(const char* path, Raster& r)
{
    std::vector<U8> b; if (!readFile(path, b) || b.size() < 16 || memcmp(b.data(), "VKSR", 4) != 0) return false;
    U32 w, h, c; memcpy(&w, &b[4], 4); memcpy(&h, &b[8], 4); memcpy(&c, &b[12], 4);
    if (b.size() != 16 + (size_t)w * h * c) return false;
    r.w = w; r.h = h; r.c = c; r.data.assign(b.begin() + 16, b.end()); return true;
}
static bool writeRaw(const char* path, const Raster& r)
{
    std::vector<U8> b(16 + r.data.size());
    memcpy(&b[0], "VKSR", 4); memcpy(&b[4], &r.w, 4); memcpy(&b[8], &r.h, 4); memcpy(&b[12], &r.c, 4);
    memcpy(&b[16], r.data.data(), r.data.size());
    return writeFile(path, b.data(), b.size());
}

// ---------------------------------------------------------------------------
// PNG: LLPngWrapper verbatim (llpngwrapper.cpp), LLImagePNG glue.
// ---------------------------------------------------------------------------
struct PngError : public std::runtime_error { PngError(png_const_charp msg) : std::runtime_error(msg) {} };

class LLPngWrapper
{
public:
    struct ImageInfo { U16 mWidth; U16 mHeight; S8 mComponents; };
    LLPngWrapper() : mReadPngPtr(NULL), mReadInfoPtr(NULL), mWritePngPtr(NULL), mWriteInfoPtr(NULL), mRowPointers(NULL), mWidth(0), mHeight(0), mBitDepth(0), mColorType(0), mChannels(0), mInterlaceType(0), mCompressionType(0), mFilterMethod(0), mFinalSize(0), mGamma(0.f) {}
    ~LLPngWrapper() { releaseResources(); }
    bool isValidPng(U8* src)
    {
        const int PNG_BYTES_TO_CHECK = 8;
        int sig = png_sig_cmp((png_bytep)src, (png_size_t)0, PNG_BYTES_TO_CHECK);
        if (sig != 0) { mErrorMessage = "Invalid or corrupt PNG file"; return false; }
        return true;
    }
    struct PngDataInfo { U8* mData; U32 mOffset; S32 mDataSize; };
    static void errorHandler(png_structp, png_const_charp msg) { throw PngError(msg); }
    static void readDataCallback(png_structp png_ptr, png_bytep dest, png_size_t length)
    {
        PngDataInfo* dataInfo = (PngDataInfo*)png_get_io_ptr(png_ptr);
        if (dataInfo->mOffset + length > (size_t)dataInfo->mDataSize) { png_error(png_ptr, "Data read error. Requested data size exceeds available data size."); return; }
        memcpy(dest, &dataInfo->mData[dataInfo->mOffset], length);
        dataInfo->mOffset += static_cast<U32>(length);
    }
    static void writeDataCallback(png_structp png_ptr, png_bytep src, png_size_t length)
    {
        PngDataInfo* dataInfo = (PngDataInfo*)png_get_io_ptr(png_ptr);
        if (dataInfo->mOffset + length > (size_t)dataInfo->mDataSize) { png_error(png_ptr, "Data write error. Requested data size exceeds available data size."); return; }
        memcpy(&dataInfo->mData[dataInfo->mOffset], src, length);
        dataInfo->mOffset += static_cast<U32>(length);
    }
    static void writeFlush(png_structp) {}

    bool readPng(U8* src, S32 dataSize, Raster* rawImage, ImageInfo* infop = NULL)
    {
        try
        {
            mReadPngPtr = png_create_read_struct(PNG_LIBPNG_VER_STRING, this, &errorHandler, NULL);
            if (mReadPngPtr == NULL) throw PngError("Problem creating png read structure");
            mReadInfoPtr = png_create_info_struct(mReadPngPtr);
            PngDataInfo dataPtr; dataPtr.mData = src; dataPtr.mOffset = 0; dataPtr.mDataSize = dataSize;
            png_set_read_fn(mReadPngPtr, &dataPtr, &readDataCallback);
            png_set_sig_bytes(mReadPngPtr, 0);
            png_read_info(mReadPngPtr, mReadInfoPtr);
            png_get_IHDR(mReadPngPtr, mReadInfoPtr, &mWidth, &mHeight, &mBitDepth, &mColorType, &mInterlaceType, &mCompressionType, &mFilterMethod);
            normalizeImage();
            updateMetaData();
            if (rawImage != NULL)
            {
                if (!rawImage->resize(static_cast<U16>(mWidth), static_cast<U16>(mHeight), mChannels)) throw PngError("Failed to resize image");
                U8* dest = rawImage->getData();
                int offset = mWidth * mChannels;
                mRowPointers = new U8*[mHeight];
                for (U32 i = 0; i < mHeight; i++) mRowPointers[i] = &dest[(mHeight - i - 1) * offset];
                png_read_image(mReadPngPtr, mRowPointers);
                png_read_end(mReadPngPtr, NULL);
            }
            if (infop != NULL) { infop->mHeight = static_cast<U16>(mHeight); infop->mWidth = static_cast<U16>(mWidth); infop->mComponents = mChannels; }
            mFinalSize = dataPtr.mOffset;
        }
        catch (const PngError& msg) { mErrorMessage = msg.what(); releaseResources(); return false; }
        catch (std::bad_alloc&) { mErrorMessage = "LLPngWrapper"; releaseResources(); return false; }
        catch (...) { mErrorMessage = "LLPngWrapper"; releaseResources(); return false; }
        releaseResources();
        return true;
    }
    void normalizeImage()
    {
        if (mColorType == PNG_COLOR_TYPE_PALETTE) png_set_palette_to_rgb(mReadPngPtr);
        if (mColorType == PNG_COLOR_TYPE_GRAY && mBitDepth < 8) png_set_expand_gray_1_2_4_to_8(mReadPngPtr);
        if (mColorType == PNG_COLOR_TYPE_GRAY || mColorType == PNG_COLOR_TYPE_GRAY_ALPHA) png_set_gray_to_rgb(mReadPngPtr);
        if (png_get_valid(mReadPngPtr, mReadInfoPtr, PNG_INFO_tRNS)) png_set_tRNS_to_alpha(mReadPngPtr);
        if (mBitDepth < 8) png_set_packing(mReadPngPtr);
        else if (mBitDepth == 16) png_set_strip_16(mReadPngPtr);
        const F64 SCREEN_GAMMA = 2.2;
        if (png_get_gAMA(mReadPngPtr, mReadInfoPtr, &mGamma)) png_set_gamma(mReadPngPtr, SCREEN_GAMMA, mGamma);
        else png_set_gamma(mReadPngPtr, SCREEN_GAMMA, 1 / SCREEN_GAMMA);
    }
    void updateMetaData()
    {
        png_read_update_info(mReadPngPtr, mReadInfoPtr);
        mWidth = png_get_image_width(mReadPngPtr, mReadInfoPtr);
        mHeight = png_get_image_height(mReadPngPtr, mReadInfoPtr);
        mBitDepth = png_get_bit_depth(mReadPngPtr, mReadInfoPtr);
        mColorType = png_get_color_type(mReadPngPtr, mReadInfoPtr);
        mChannels = png_get_channels(mReadPngPtr, mReadInfoPtr);
    }
    bool writePng(const Raster* rawImage, U8* dest, size_t destSize)
    {
        try
        {
            S8 numComponents = (S8)rawImage->getComponents();
            switch (numComponents)
            {
            case 1: mColorType = PNG_COLOR_TYPE_GRAY; break;
            case 2: mColorType = PNG_COLOR_TYPE_GRAY_ALPHA; break;
            case 3: mColorType = PNG_COLOR_TYPE_RGB; break;
            case 4: mColorType = PNG_COLOR_TYPE_RGB_ALPHA; break;
            default: mColorType = -1;
            }
            if (mColorType == -1) throw PngError("Unsupported image: unexpected number of channels");
            mWritePngPtr = png_create_write_struct(PNG_LIBPNG_VER_STRING, NULL, &errorHandler, NULL);
            if (!mWritePngPtr) throw PngError("Problem creating png write structure");
            mWriteInfoPtr = png_create_info_struct(mWritePngPtr);
            PngDataInfo dataPtr{}; dataPtr.mData = dest; dataPtr.mOffset = 0; dataPtr.mDataSize = static_cast<S32>(destSize);
            png_set_write_fn(mWritePngPtr, &dataPtr, &writeDataCallback, &writeFlush);
            mWidth = rawImage->getWidth(); mHeight = rawImage->getHeight(); mBitDepth = 8; mChannels = numComponents;
            mInterlaceType = PNG_INTERLACE_NONE; mCompressionType = PNG_COMPRESSION_TYPE_DEFAULT; mFilterMethod = PNG_FILTER_TYPE_DEFAULT;
            png_set_IHDR(mWritePngPtr, mWriteInfoPtr, mWidth, mHeight, mBitDepth, mColorType, mInterlaceType, mCompressionType, mFilterMethod);
            const U8* data = rawImage->getData();
            int offset = mWidth * mChannels;
            png_write_info(mWritePngPtr, mWriteInfoPtr);
            const U8* rowPointer;
            for (U32 i = 0; i < mHeight; i++)
            {
                rowPointer = &data[(mHeight - 1 - i) * offset];
                png_write_row(mWritePngPtr, const_cast<png_bytep>(rowPointer));
            }
            png_write_end(mWritePngPtr, mWriteInfoPtr);
            mFinalSize = dataPtr.mOffset;
        }
        catch (const PngError& msg) { mErrorMessage = msg.what(); releaseResources(); return false; }
        releaseResources();
        return true;
    }
    void releaseResources()
    {
        if (mReadPngPtr || mReadInfoPtr) { png_destroy_read_struct(&mReadPngPtr, &mReadInfoPtr, NULL); mReadPngPtr = NULL; mReadInfoPtr = NULL; }
        if (mWritePngPtr || mWriteInfoPtr) { png_destroy_write_struct(&mWritePngPtr, &mWriteInfoPtr); mWritePngPtr = NULL; mWriteInfoPtr = NULL; }
        if (mRowPointers) { delete[] mRowPointers; mRowPointers = NULL; }
    }
    U32 getFinalSize() { return mFinalSize; }
    const std::string& getErrorMessage() { return mErrorMessage; }

    png_structp mReadPngPtr; png_infop mReadInfoPtr; png_structp mWritePngPtr; png_infop mWriteInfoPtr;
    U8** mRowPointers; png_uint_32 mWidth; png_uint_32 mHeight; S32 mBitDepth; S32 mColorType; S32 mChannels;
    S32 mInterlaceType; S32 mCompressionType; S32 mFilterMethod; U32 mFinalSize; F64 mGamma; std::string mErrorMessage;
};

static bool pngHeader(std::vector<U8>& in, U32& w, U32& h, U32& c)
{
    if (in.empty()) { setLastError("Uninitialized instance of LLImagePNG"); return false; }
    LLPngWrapper pngWrapper;
    if (!pngWrapper.isValidPng(in.data())) { setLastError("LLImagePNG data does not have a valid PNG header!"); return false; }
    LLPngWrapper::ImageInfo infop;
    if (!pngWrapper.readPng(in.data(), (S32)in.size(), NULL, &infop)) { setLastError(pngWrapper.getErrorMessage()); return false; }
    w = infop.mWidth; h = infop.mHeight; c = infop.mComponents; return true;
}
static bool pngDecode(std::vector<U8>& in, Raster& out)
{
    U32 w, h, c; if (!pngHeader(in, w, h, c)) return false; // LLImageFormatted::load runs updateData first
    LLPngWrapper pngWrapper;
    if (!pngWrapper.isValidPng(in.data())) { setLastError("LLImagePNG data does not have a valid PNG header!"); return false; }
    if (!pngWrapper.readPng(in.data(), (S32)in.size(), &out)) { setLastError(pngWrapper.getErrorMessage()); return false; }
    return true;
}
static bool pngEncode(const Raster& raw, std::vector<U8>& out)
{
    U32 bufferSize = raw.w * raw.h * raw.c + 8192;
    std::vector<U8> tmp(bufferSize);
    LLPngWrapper pngWrapper;
    if (!pngWrapper.writePng(&raw, tmp.data(), bufferSize)) { setLastError(pngWrapper.getErrorMessage()); return false; }
    out.assign(tmp.begin(), tmp.begin() + pngWrapper.getFinalSize());
    return true;
}

// ---------------------------------------------------------------------------
// JPEG: LLImageJPEG verbatim (llimagejpeg.cpp), memory source and destination.
// ---------------------------------------------------------------------------
static jmp_buf sSetjmpBuffer;
static std::string sJpegMessage;
static void jpegErrorExit(j_common_ptr cinfo) { (*cinfo->err->output_message)(cinfo); jpeg_destroy(cinfo); longjmp(sSetjmpBuffer, 1); }
static void jpegEmitMessage(j_common_ptr cinfo, int msg_level)
{
    struct jpeg_error_mgr* err = cinfo->err;
    if (msg_level < 0) { if (err->num_warnings == 0 || err->trace_level >= 3) (*err->output_message)(cinfo); err->num_warnings++; }
    else if (err->trace_level >= msg_level) (*err->output_message)(cinfo);
}
static void jpegOutputMessage(j_common_ptr cinfo) { char buffer[JMSG_LENGTH_MAX]; (*cinfo->err->format_message)(cinfo, buffer); sJpegMessage = buffer; setLastError(buffer); }
static void decodeInitSource(j_decompress_ptr) {}
static boolean decodeFillInputBuffer(j_decompress_ptr cinfo) { ERREXIT(cinfo, JERR_INPUT_EMPTY); return TRUE; }
static void decodeSkipInputData(j_decompress_ptr cinfo, long num_bytes) { cinfo->src->next_input_byte += (size_t)num_bytes; cinfo->src->bytes_in_buffer -= (size_t)num_bytes; }
static void decodeTermSource(j_decompress_ptr) {}

static bool jpegHeader(std::vector<U8>& in, U32& w, U32& h, U32& c)
{
    if (in.empty()) { setLastError("Uninitialized instance of LLImageJPEG"); return false; }
    struct jpeg_decompress_struct cinfo; struct jpeg_error_mgr jerr;
    cinfo.err = jpeg_std_error(&jerr);
    jerr.error_exit = &jpegErrorExit; jerr.emit_message = &jpegEmitMessage; jerr.output_message = &jpegOutputMessage;
    if (setjmp(sSetjmpBuffer)) { jpeg_destroy_decompress(&cinfo); return false; }
    jpeg_create_decompress(&cinfo);
    cinfo.src = (struct jpeg_source_mgr*)(*cinfo.mem->alloc_small)((j_common_ptr)&cinfo, JPOOL_PERMANENT, sizeof(struct jpeg_source_mgr));
    cinfo.src->init_source = &decodeInitSource; cinfo.src->fill_input_buffer = &decodeFillInputBuffer; cinfo.src->skip_input_data = &decodeSkipInputData;
    cinfo.src->resync_to_restart = jpeg_resync_to_restart; cinfo.src->term_source = &decodeTermSource;
    cinfo.src->bytes_in_buffer = in.size(); cinfo.src->next_input_byte = in.data();
    jpeg_read_header(&cinfo, TRUE);
    w = cinfo.image_width; h = cinfo.image_height; c = 3;
    jpeg_destroy_decompress(&cinfo);
    return true;
}
// Returns false when the viewer would have reported an error (it always returns "done").
static bool jpegDecode(std::vector<U8>& in, Raster& out)
{
    U32 w, h, c; if (!jpegHeader(in, w, h, c)) return false;
    struct jpeg_decompress_struct cinfo; struct jpeg_error_mgr jerr;
    cinfo.err = jpeg_std_error(&jerr);
    jerr.error_exit = &jpegErrorExit; jerr.emit_message = &jpegEmitMessage; jerr.output_message = &jpegOutputMessage;
    if (setjmp(sSetjmpBuffer)) { jpeg_destroy_decompress(&cinfo); return false; }
    jpeg_create_decompress(&cinfo);
    cinfo.src = (struct jpeg_source_mgr*)(*cinfo.mem->alloc_small)((j_common_ptr)&cinfo, JPOOL_PERMANENT, sizeof(struct jpeg_source_mgr));
    cinfo.src->init_source = &decodeInitSource; cinfo.src->fill_input_buffer = &decodeFillInputBuffer; cinfo.src->skip_input_data = &decodeSkipInputData;
    cinfo.src->resync_to_restart = jpeg_resync_to_restart; cinfo.src->term_source = &decodeTermSource;
    cinfo.src->bytes_in_buffer = in.size(); cinfo.src->next_input_byte = in.data();
    jpeg_read_header(&cinfo, TRUE);
    out.resize(cinfo.image_width, cinfo.image_height, 3);
    U8* raw_image_data = out.getData();
    cinfo.out_color_components = 3; cinfo.out_color_space = JCS_RGB;
    jpeg_start_decompress(&cinfo);
    S32 row_stride = cinfo.output_width * cinfo.output_components;
    raw_image_data += row_stride * (cinfo.output_height - 1);
    while (cinfo.output_scanline < cinfo.output_height) { jpeg_read_scanlines(&cinfo, &raw_image_data, 1); raw_image_data -= row_stride; }
    jpeg_finish_decompress(&cinfo);
    jpeg_destroy_decompress(&cinfo);
    if (jerr.num_warnings != 0) { setLastError("Unable to decode JPEG image."); return false; }
    return true;
}
struct JpegDest { U8* mOutputBuffer; S32 mOutputBufferSize; std::vector<U8>* out; };
static void encodeInitDestination(j_compress_ptr cinfo) { JpegDest* self = (JpegDest*)cinfo->client_data; cinfo->dest->next_output_byte = self->mOutputBuffer; cinfo->dest->free_in_buffer = self->mOutputBufferSize; }
static boolean encodeEmptyOutputBuffer(j_compress_ptr cinfo)
{
    JpegDest* self = (JpegDest*)cinfo->client_data;
    S32 new_buffer_size = self->mOutputBufferSize * 2;
    U8* new_buffer = new U8[new_buffer_size];
    memcpy(new_buffer, self->mOutputBuffer, self->mOutputBufferSize);
    delete[] self->mOutputBuffer; self->mOutputBuffer = new_buffer;
    cinfo->dest->next_output_byte = self->mOutputBuffer + self->mOutputBufferSize; cinfo->dest->free_in_buffer = self->mOutputBufferSize;
    self->mOutputBufferSize = new_buffer_size;
    return TRUE;
}
static void encodeTermDestination(j_compress_ptr cinfo)
{
    JpegDest* self = (JpegDest*)cinfo->client_data;
    S32 file_bytes = (S32)(self->mOutputBufferSize - cinfo->dest->free_in_buffer);
    self->out->assign(self->mOutputBuffer, self->mOutputBuffer + file_bytes);
}
static bool jpegEncode(const Raster& raw, std::vector<U8>& out, S32 mEncodeQuality)
{
    switch (raw.c) { case 1: case 3: break; default: setLastError("Unable to encode a JPEG image that doesn't have 1 or 3 components."); return false; }
    JpegDest dest; dest.mOutputBufferSize = raw.w * raw.h * raw.c + 1024; dest.mOutputBuffer = new U8[dest.mOutputBufferSize]; dest.out = &out;
    struct jpeg_compress_struct cinfo; cinfo.client_data = &dest;
    struct jpeg_error_mgr jerr; cinfo.err = jpeg_std_error(&jerr);
    jerr.error_exit = &jpegErrorExit; jerr.emit_message = &jpegEmitMessage; jerr.output_message = &jpegOutputMessage;
    if (setjmp(sSetjmpBuffer)) { jpeg_destroy_compress(&cinfo); delete[] dest.mOutputBuffer; return false; }
    jpeg_create_compress(&cinfo);
    cinfo.dest = (struct jpeg_destination_mgr*)(*cinfo.mem->alloc_small)((j_common_ptr)&cinfo, JPOOL_PERMANENT, sizeof(struct jpeg_destination_mgr));
    cinfo.dest->next_output_byte = dest.mOutputBuffer; cinfo.dest->free_in_buffer = dest.mOutputBufferSize;
    cinfo.dest->init_destination = &encodeInitDestination; cinfo.dest->empty_output_buffer = &encodeEmptyOutputBuffer; cinfo.dest->term_destination = &encodeTermDestination;
    cinfo.image_width = raw.w; cinfo.image_height = raw.h;
    if (raw.c == 1) { cinfo.input_components = 1; cinfo.in_color_space = JCS_GRAYSCALE; } else { cinfo.input_components = 3; cinfo.in_color_space = JCS_RGB; }
    jpeg_set_defaults(&cinfo);
    jpeg_set_quality(&cinfo, mEncodeQuality, TRUE);
    jpeg_start_compress(&cinfo, TRUE);
    S32 row_stride = raw.w * raw.c;
    const U8* last_row_data = raw.getData() + (raw.h - 1) * row_stride;
    JSAMPROW row_pointer[1];
    while (cinfo.next_scanline < cinfo.image_height) { row_pointer[0] = (JSAMPROW)(last_row_data - (cinfo.next_scanline * row_stride)); jpeg_write_scanlines(&cinfo, row_pointer, 1); }
    jpeg_finish_compress(&cinfo);
    delete[] dest.mOutputBuffer;
    jpeg_destroy_compress(&cinfo);
    return true;
}
// LLImageDimensionsInfo::getImageDimensionsJpeg over a memory source.
static bool jpegDims(std::vector<U8>& in, U32& w, U32& h)
{
    if (in.size() < 2 || in[0] != 0xFF || in[1] != 0xD8) { setLastError("Not a JPEG"); return false; }
    struct jpeg_decompress_struct cinfo{}; struct jpeg_error_mgr jerr; cinfo.err = jpeg_std_error(&jerr);
    jerr.error_exit = &jpegErrorExit; jerr.output_message = &jpegOutputMessage;
    if (setjmp(sSetjmpBuffer)) { jpeg_destroy_decompress(&cinfo); return false; }
    jpeg_create_decompress(&cinfo);
    jpeg_mem_src(&cinfo, in.data(), (unsigned long)in.size());
    jpeg_read_header(&cinfo, TRUE);
    cinfo.out_color_space = JCS_RGB;
    jpeg_start_decompress(&cinfo);
    w = cinfo.output_width; h = cinfo.output_height;
    jpeg_destroy_decompress(&cinfo);
    return true;
}

// ---------------------------------------------------------------------------
// TGA: LLImageTGA verbatim (llimagetga.cpp).
// ---------------------------------------------------------------------------
class LLImageTGA
{
public:
    std::vector<U8> mData;
    U32 mWidth = 0, mHeight = 0, mComponents = 0;
    U8* getData() { return mData.data(); }
    S32 getDataSize() { return (S32)mData.size(); }
    U32 getWidth() { return mWidth; } U32 getHeight() { return mHeight; } U32 getComponents() { return mComponents; }
    void setSize(S32 w, S32 h, S32 c) { mWidth = (U16)w; mHeight = (U16)h; mComponents = (S8)c; }

    LLImageTGA() : mColorMap(NULL), mColorMapStart(0), mColorMapLength(0), mColorMapBytesPerEntry(0), mIs15Bit(false), mAttributeBits(0), mColorMapDepth(0), mColorMapIndexHi(0), mColorMapIndexLo(0), mColorMapLengthHi(0), mColorMapLengthLo(0), mColorMapType(0), mDataOffset(0), mHeightHi(0), mHeightLo(0), mIDLength(0), mImageType(0), mInterleave(0), mOriginRightBit(0), mOriginTopBit(0), mPixelSize(0), mWidthHi(0), mWidthLo(0), mXOffsetHi(0), mXOffsetLo(0), mYOffsetHi(0), mYOffsetLo(0) {}
    ~LLImageTGA() { delete[] mColorMap; }

    static const U8 s5to8bits[32];
    inline void decodeTruecolorPixel15(U8* dst, const U8* src)
    {
        U32 t = U32(src[0]) + (U32(src[1]) << 8);
        dst[2] = s5to8bits[t & 0x1F]; t >>= 5; dst[1] = s5to8bits[t & 0x1F]; t >>= 5; dst[0] = s5to8bits[t & 0x1F];
    }
    bool updateData()
    {
        if (mData.empty()) { setLastError("LLImageTGA uninitialized"); return false; }
        U8 flags; U8 junk[256];
        mDataOffset = 0;
        mIDLength = *(getData() + mDataOffset++); mColorMapType = *(getData() + mDataOffset++); mImageType = *(getData() + mDataOffset++);
        mColorMapIndexLo = *(getData() + mDataOffset++); mColorMapIndexHi = *(getData() + mDataOffset++);
        mColorMapLengthLo = *(getData() + mDataOffset++); mColorMapLengthHi = *(getData() + mDataOffset++); mColorMapDepth = *(getData() + mDataOffset++);
        mXOffsetLo = *(getData() + mDataOffset++); mXOffsetHi = *(getData() + mDataOffset++); mYOffsetLo = *(getData() + mDataOffset++); mYOffsetHi = *(getData() + mDataOffset++);
        mWidthLo = *(getData() + mDataOffset++); mWidthHi = *(getData() + mDataOffset++); mHeightLo = *(getData() + mDataOffset++); mHeightHi = *(getData() + mDataOffset++);
        mPixelSize = *(getData() + mDataOffset++); flags = *(getData() + mDataOffset++);
        mAttributeBits = flags & 0xf; mOriginRightBit = (flags & 0x10) >> 4; mOriginTopBit = (flags & 0x20) >> 5; mInterleave = (flags & 0xc0) >> 6;
        switch (mImageType)
        {
        case 0: setLastError("Unable to load file.  TGA file contains no image data."); return false;
        case 1: if (8 != mPixelSize) { setLastError("Unable to load file.  Colormapped images must have 8 bits per pixel."); return false; } break;
        case 2: break;
        case 3: if (8 != mPixelSize) { setLastError("Unable to load file.  Monochrome images must have 8 bits per pixel."); return false; } break;
        case 9: break;
        case 10: break;
        case 11: if (8 != mPixelSize) { setLastError("Unable to load file.  Monochrome images must have 8 bits per pixel."); return false; } break;
        default: setLastError("Unable to load file.  Unrecoginzed TGA image type."); return false;
        }
        if (mIDLength) { memcpy(junk, getData() + mDataOffset, mIDLength); mDataOffset += mIDLength; }
        S32 color_map_bytes = 0;
        if ((1 == mColorMapType) && (mColorMapDepth > 0))
        {
            mColorMapStart = (S32(mColorMapIndexHi) << 8) + mColorMapIndexLo;
            mColorMapLength = (S32(mColorMapLengthHi) << 8) + mColorMapLengthLo;
            if (mColorMapDepth > 24) mColorMapBytesPerEntry = 4; else if (mColorMapDepth > 16) mColorMapBytesPerEntry = 3; else if (mColorMapDepth > 8) mColorMapBytesPerEntry = 2; else mColorMapBytesPerEntry = 1;
            color_map_bytes = mColorMapLength * mColorMapBytesPerEntry;
            if ((1 == mImageType) || (9 == mImageType)) { mColorMap = new U8[color_map_bytes]; memcpy(mColorMap, getData() + mDataOffset, color_map_bytes); }
            mDataOffset += color_map_bytes;
        }
        S32 height = (S32(mHeightHi) << 8) + mHeightLo; S32 width = (S32(mWidthHi) << 8) + mWidthLo;
        S32 bits_per_pixel = mColorMap ? mColorMapDepth : mPixelSize;
        S32 components;
        switch (bits_per_pixel)
        {
        case 24: components = 3; break;
        case 32: components = 4; mAttributeBits = 8; break;
        case 15: case 16: components = 3; mIs15Bit = true; break;
        case 8: components = 1; break;
        default: setLastError("Unable to load file. Unknown pixel size."); return false;
        }
        setSize(width, height, components);
        return true;
    }
    bool decode(Raster* raw_image)
    {
        if (mData.empty()) { setLastError("LLImageTGA trying to decode an image with no data!"); return false; }
        if (!raw_image->resize(getWidth(), getHeight(), getComponents())) { setLastError("LLImageTGA::out of memory"); return false; }
        if ((getComponents() != 1) && (getComponents() != 3) && (getComponents() != 4)) { setLastError("TGA images with a number of components other than 1, 3, and 4 are not supported."); return false; }
        if (mOriginRightBit) { setLastError("TGA images with origin on right side are not supported."); return false; }
        bool flipped = (mOriginTopBit != 0); bool rle_compressed = ((mImageType & 0x08) != 0);
        if (mColorMap) return decodeColorMap(raw_image, rle_compressed, flipped);
        return decodeTruecolor(raw_image, rle_compressed, flipped);
    }
    bool decodeTruecolor(Raster* raw_image, bool rle, bool flipped)
    {
        bool success = false; bool alpha_opaque = false;
        if (rle)
        {
            switch (getComponents())
            {
            case 1: success = decodeTruecolorRle8(raw_image); break;
            case 3: success = mIs15Bit ? decodeTruecolorRle15(raw_image) : decodeTruecolorRle24(raw_image); break;
            case 4: success = decodeTruecolorRle32(raw_image, alpha_opaque); if (alpha_opaque) raw_image->dropAlpha(); break;
            }
        }
        else
        {
            bool alpha_opaque2;
            success = decodeTruecolorNonRle(raw_image, alpha_opaque2);
            if (alpha_opaque2 && raw_image->getComponents() == 4) raw_image->dropAlpha();
        }
        if (success && flipped) raw_image->verticalFlip();
        return success;
    }
    bool decodeTruecolorNonRle(Raster* raw_image, bool& alpha_opaque)
    {
        alpha_opaque = true;
        U8* dst = raw_image->getData(); U8* src = getData() + mDataOffset;
        S32 pixels = getWidth() * getHeight();
        if (pixels * (mIs15Bit ? 2 : (S32)getComponents()) > getDataSize() - (S32)mDataOffset) return false;
        if (getComponents() == 4) { while (pixels--) { dst[0] = src[2]; dst[1] = src[1]; dst[2] = src[0]; dst[3] = src[3]; if (dst[3] != 255) alpha_opaque = false; dst += 4; src += 4; } }
        else if (getComponents() == 3)
        {
            if (mIs15Bit) { while (pixels--) { decodeTruecolorPixel15(dst, src); dst += 3; src += 2; } }
            else { while (pixels--) { dst[0] = src[2]; dst[1] = src[1]; dst[2] = src[0]; dst += 3; src += 3; } }
        }
        else if (getComponents() == 1) memcpy(dst, src, pixels);
        return true;
    }
    void decodeColorMapPixel8(U8* dst, const U8* src) { S32 index = llclamp(*src - mColorMapStart, 0, mColorMapLength - 1); dst[0] = mColorMap[index]; }
    void decodeColorMapPixel15(U8* dst, const U8* src) { S32 index = llclamp(*src - mColorMapStart, 0, mColorMapLength - 1); decodeTruecolorPixel15(dst, mColorMap + 2 * index); }
    void decodeColorMapPixel24(U8* dst, const U8* src) { S32 index = 3 * llclamp(*src - mColorMapStart, 0, mColorMapLength - 1); dst[0] = mColorMap[index + 2]; dst[1] = mColorMap[index + 1]; dst[2] = mColorMap[index + 0]; }
    void decodeColorMapPixel32(U8* dst, const U8* src) { S32 index = 4 * llclamp(*src - mColorMapStart, 0, mColorMapLength - 1); dst[0] = mColorMap[index + 2]; dst[1] = mColorMap[index + 1]; dst[2] = mColorMap[index + 0]; dst[3] = mColorMap[index + 3]; }
    bool decodeColorMap(Raster* raw_image, bool rle, bool flipped)
    {
        if (8 != mPixelSize) return false;
        U8* src = getData() + mDataOffset; U8* dst = raw_image->getData();
        void (LLImageTGA::*pixel_decoder)(U8*, const U8*);
        switch (mColorMapBytesPerEntry)
        {
        case 1: pixel_decoder = &LLImageTGA::decodeColorMapPixel8; break;
        case 2: pixel_decoder = &LLImageTGA::decodeColorMapPixel15; break;
        case 3: pixel_decoder = &LLImageTGA::decodeColorMapPixel24; break;
        case 4: pixel_decoder = &LLImageTGA::decodeColorMapPixel32; break;
        default: return false;
        }
        if (rle)
        {
            U8* last_dst = dst + getComponents() * (getHeight() * getWidth() - 1);
            while (dst <= last_dst)
            {
                U8 block_header_byte = *src; src++;
                U8 block_pixel_count = (block_header_byte & 0x7F) + 1;
                if (block_header_byte & 0x80) { do { (this->*pixel_decoder)(dst, src); dst += getComponents(); block_pixel_count--; } while (block_pixel_count > 0); src++; }
                else { do { (this->*pixel_decoder)(dst, src); dst += getComponents(); src++; block_pixel_count--; } while (block_pixel_count > 0); }
            }
            raw_image->verticalFlip();
        }
        else
        {
            S32 src_row_bytes = getWidth(); S32 dst_row_bytes = getWidth() * getComponents();
            if (flipped) { U8* src_last_row_start = src + (getHeight() - 1) * src_row_bytes; src = src_last_row_start; src_row_bytes *= -1; }
            S32 i, j;
            for (S32 row = 0; row < (S32)getHeight(); row++)
            {
                for (i = 0, j = 0; j < (S32)getWidth(); i += getComponents(), j++) (this->*pixel_decoder)(dst + i, src + j);
                dst += dst_row_bytes; src += src_row_bytes;
            }
        }
        return true;
    }
    bool encode(const Raster* raw_image)
    {
        mData.clear();
        setSize(raw_image->getWidth(), raw_image->getHeight(), raw_image->getComponents());
        mIDLength = 0; mColorMapType = 0;
        switch (getComponents()) { case 1: mImageType = 3; break; case 2: case 3: case 4: mImageType = 2; break; default: return false; }
        mColorMapIndexLo = 0; mColorMapIndexHi = 0; mColorMapLengthLo = 0; mColorMapLengthHi = 0; mColorMapDepth = 0;
        mXOffsetLo = 0; mXOffsetHi = 0; mYOffsetLo = 0; mYOffsetHi = 0;
        mWidthLo = U8(getWidth() & 0xFF); mWidthHi = U8((getWidth() >> 8) & 0xFF); mHeightLo = U8(getHeight() & 0xFF); mHeightHi = U8((getHeight() >> 8) & 0xFF);
        S32 bytes_per_pixel;
        switch (getComponents()) { case 1: bytes_per_pixel = 1; break; case 3: bytes_per_pixel = 3; break; case 2: case 4: bytes_per_pixel = 4; break; default: return false; }
        mPixelSize = U8(bytes_per_pixel * 8);
        mAttributeBits = (4 == bytes_per_pixel) ? 8 : 0; mOriginRightBit = 0; mOriginTopBit = 0; mInterleave = 0;
        const S32 TGA_HEADER_SIZE = 18; const S32 COLOR_MAP_SIZE = 0;
        mDataOffset = TGA_HEADER_SIZE + mIDLength + COLOR_MAP_SIZE;
        S32 pixels = getWidth() * getHeight();
        S32 datasize = mDataOffset + bytes_per_pixel * pixels;
        mData.assign(datasize, 0);
        U8* dst = mData.data();
        *(dst++) = mIDLength; *(dst++) = mColorMapType; *(dst++) = mImageType; *(dst++) = mColorMapIndexLo; *(dst++) = mColorMapIndexHi; *(dst++) = mColorMapLengthLo; *(dst++) = mColorMapLengthHi; *(dst++) = mColorMapDepth;
        *(dst++) = mXOffsetLo; *(dst++) = mXOffsetHi; *(dst++) = mYOffsetLo; *(dst++) = mYOffsetHi; *(dst++) = mWidthLo; *(dst++) = mWidthHi; *(dst++) = mHeightLo; *(dst++) = mHeightHi; *(dst++) = mPixelSize;
        *(dst++) = ((mInterleave & 3) << 5) | ((mOriginTopBit & 1) << 4) | ((mOriginRightBit & 1) << 3) | ((mAttributeBits & 0xF) << 0);
        const U8* src = raw_image->getData();
        S32 i = 0, j = 0;
        switch (getComponents())
        {
        case 1: memcpy(dst, src, bytes_per_pixel * pixels); break;
        case 2: while (pixels--) { dst[i + 0] = src[j + 0]; dst[i + 1] = src[j + 0]; dst[i + 2] = src[j + 0]; dst[i + 3] = src[j + 1]; i += 4; j += 2; } break;
        case 3: while (pixels--) { dst[i + 0] = src[i + 2]; dst[i + 1] = src[i + 1]; dst[i + 2] = src[i + 0]; i += 3; } break;
        case 4: while (pixels--) { dst[i + 0] = src[i + 2]; dst[i + 1] = src[i + 1]; dst[i + 2] = src[i + 0]; dst[i + 3] = src[i + 3]; i += 4; } break;
        }
        return true;
    }
    bool decodeTruecolorRle32(Raster* raw_image, bool& alpha_opaque)
    {
        alpha_opaque = true;
        U8* dst = raw_image->getData(); U32* dst_pixels = (U32*)dst;
        U8* src = getData() + mDataOffset; U8* last_src = src + getDataSize();
        U32 rgba; U8* rgba_byte_p = (U8*)&rgba;
        U32* last_dst_pixel = dst_pixels + getHeight() * getWidth() - 1;
        while (dst_pixels <= last_dst_pixel)
        {
            if (src >= last_src) return false;
            U8 block_header_byte = *src; src++;
            U32 block_pixel_count = (block_header_byte & 0x7F) + 1;
            if (block_header_byte & 0x80)
            {
                if (src + 3 >= last_src) return false;
                rgba_byte_p[0] = src[2]; rgba_byte_p[1] = src[1]; rgba_byte_p[2] = src[0]; rgba_byte_p[3] = src[3];
                if (rgba_byte_p[3] != 255) alpha_opaque = false;
                src += 4; U32 value = rgba;
                do { *dst_pixels = value; dst_pixels++; block_pixel_count--; } while (block_pixel_count > 0);
            }
            else
            {
                do
                {
                    if (src + 3 >= last_src) return false;
                    ((U8*)dst_pixels)[0] = src[2]; ((U8*)dst_pixels)[1] = src[1]; ((U8*)dst_pixels)[2] = src[0]; ((U8*)dst_pixels)[3] = src[3];
                    if (src[3] != 255) alpha_opaque = false;
                    src += 4; dst_pixels++; block_pixel_count--;
                } while (block_pixel_count > 0);
            }
        }
        return true;
    }
    bool decodeTruecolorRle15(Raster* raw_image)
    {
        U8* dst = raw_image->getData(); U8* src = getData() + mDataOffset;
        U8* last_src = src + getDataSize(); U8* last_dst = dst + getComponents() * (getHeight() * getWidth() - 1);
        while (dst <= last_dst)
        {
            if (src >= last_src) return false;
            U8 block_header_byte = *src; src++;
            U8 block_pixel_count = (block_header_byte & 0x7F) + 1;
            if (block_header_byte & 0x80) { do { if (src + 2 >= last_src) return false; decodeTruecolorPixel15(dst, src); dst += 3; block_pixel_count--; } while (block_pixel_count > 0); src += 2; }
            else { do { if (src + 2 >= last_src) return false; decodeTruecolorPixel15(dst, src); dst += 3; src += 2; block_pixel_count--; } while (block_pixel_count > 0); }
        }
        return true;
    }
    bool decodeTruecolorRle24(Raster* raw_image)
    {
        U8* dst = raw_image->getData(); U8* src = getData() + mDataOffset;
        U8* last_src = src + getDataSize(); U8* last_dst = dst + getComponents() * (getHeight() * getWidth() - 1);
        while (dst <= last_dst)
        {
            if (src >= last_src) return false;
            U8 block_header_byte = *src; src++;
            U8 block_pixel_count = (block_header_byte & 0x7F) + 1;
            if (block_header_byte & 0x80) { do { if (src + 2 >= last_src) return false; dst[0] = src[2]; dst[1] = src[1]; dst[2] = src[0]; dst += 3; block_pixel_count--; } while (block_pixel_count > 0); src += 3; }
            else { do { if (src + 2 >= last_src) return false; dst[0] = src[2]; dst[1] = src[1]; dst[2] = src[0]; dst += 3; src += 3; block_pixel_count--; } while (block_pixel_count > 0); }
        }
        return true;
    }
    bool decodeTruecolorRle8(Raster* raw_image)
    {
        U8* dst = raw_image->getData(); U8* src = getData() + mDataOffset;
        U8* last_src = src + getDataSize(); U8* last_dst = dst + getHeight() * getWidth() - 1;
        while (dst <= last_dst)
        {
            if (src >= last_src) return false;
            U8 block_header_byte = *src; src++;
            U8 block_pixel_count = (block_header_byte & 0x7F) + 1;
            if (block_header_byte & 0x80) { if (src >= last_src) return false; memset(dst, *src, block_pixel_count); dst += block_pixel_count; src++; }
            else { do { if (src >= last_src) return false; *dst = *src; dst++; src++; block_pixel_count--; } while (block_pixel_count > 0); }
        }
        return true;
    }
    bool decodeAndProcess(Raster* raw_image, F32 domain, F32 weight)
    {
        if (mData.empty()) { setLastError("LLImageTGA trying to decode an image with no data!"); return false; }
        if ((getComponents() != 1) || (mImageType != 11) || mOriginTopBit || mOriginRightBit) { setLastError("LLImageTGA trying to alpha-gradient process an image that's not a standard RLE, one component image"); return false; }
        raw_image->resize(getWidth(), getHeight(), getComponents());
        U8* dst = raw_image->getData(); U8* src = getData() + mDataOffset; U8* last_dst = dst + getHeight() * getWidth() - 1;
        if (domain > 0)
        {
            const S32 LUT_LEN = 256; U8 lut[LUT_LEN]; S32 i;
            F32 scale = 1.f / domain; F32 offset = (1.f - domain) * llclampf(1.f - weight); F32 bias = -(scale * offset);
            for (i = 0; i < LUT_LEN; i++) lut[i] = (U8)llclampb(255.f * (i / 255.f * scale + bias));
            while (dst <= last_dst)
            {
                U8 block_header_byte = *src; src++;
                U8 block_pixel_count = (block_header_byte & 0x7F) + 1;
                if (block_header_byte & 0x80) { memset(dst, lut[*src], block_pixel_count); dst += block_pixel_count; src++; }
                else { do { *dst = lut[*src]; dst++; src++; block_pixel_count--; } while (block_pixel_count > 0); }
            }
        }
        else
        {
            const U8 threshold = (U8)(0xFF * llclampf(1.f - weight));
            while (dst <= last_dst)
            {
                U8 block_header_byte = *src; src++;
                U8 block_pixel_count = (block_header_byte & 0x7F) + 1;
                if (block_header_byte & 0x80) { memset(dst, ((*src >= threshold) ? 0xFF : 0), block_pixel_count); dst += block_pixel_count; src++; }
                else { do { *dst = (*src >= threshold) ? 0xFF : 0; dst++; src++; block_pixel_count--; } while (block_pixel_count > 0); }
            }
        }
        return true;
    }

    U32 mDataOffset; U8 mIDLength; U8 mColorMapType; U8 mImageType; U8 mColorMapIndexLo; U8 mColorMapIndexHi; U8 mColorMapLengthLo; U8 mColorMapLengthHi; U8 mColorMapDepth;
    U8 mXOffsetLo; U8 mXOffsetHi; U8 mYOffsetLo; U8 mYOffsetHi; U8 mWidthLo; U8 mWidthHi; U8 mHeightLo; U8 mHeightHi; U8 mPixelSize; U8 mAttributeBits; U8 mOriginRightBit; U8 mOriginTopBit; U8 mInterleave;
    U8* mColorMap; S32 mColorMapStart; S32 mColorMapLength; S32 mColorMapBytesPerEntry; bool mIs15Bit;
};
const U8 LLImageTGA::s5to8bits[32] = { 0, 8, 16, 25, 33, 41, 49, 58, 66, 74, 82, 90, 99, 107, 115, 123, 132, 140, 148, 156, 165, 173, 181, 189, 197, 206, 214, 222, 230, 239, 247, 255 };

// ---------------------------------------------------------------------------
// BMP: LLImageBMP verbatim (llimagebmp.cpp).
// ---------------------------------------------------------------------------
struct LLBMPHeader { S32 mSize; S32 mWidth; S32 mHeight; S16 mPlanes; S16 mBitsPerPixel; S16 mCompression; S16 mAlignmentPadding; S32 mImageSize; S32 mHorzPelsPerMeter; S32 mVertPelsPerMeter; S32 mNumColors; S32 mNumColorsImportant; };
struct Win95BmpHeaderExtension { U32 mReadMask; U32 mGreenMask; U32 mBlueMask; U32 mAlphaMask; U32 mColorSpaceType; U16 mRed[3]; U16 mGreen[3]; U16 mBlue[3]; U32 mGamma[3]; };

class LLImageBMP
{
public:
    std::vector<U8> mData; U32 mWidth = 0, mHeight = 0, mComponents = 0;
    U8* getData() { return mData.data(); } S32 getDataSize() { return (S32)mData.size(); }
    S32 getWidth() { return mWidth; } S32 getHeight() { return mHeight; } S32 getComponents() { return mComponents; }
    void setSize(S32 w, S32 h, S32 c) { mWidth = (U16)w; mHeight = (U16)h; mComponents = (S8)c; }
    S32 mColorPaletteColors = 0; U8* mColorPalette = NULL; S32 mBitmapOffset = 0; S32 mBitsPerPixel = 0; U32 mBitfieldMask[4] = {0, 0, 0, 0}; bool mOriginAtTop = false;
    ~LLImageBMP() { delete[] mColorPalette; }
    bool updateData()
    {
        U8* mdata = getData();
        if (mData.empty()) { setLastError("Uninitialized instance of LLImageBMP"); return false; }
        const S32 FILE_HEADER_SIZE = 14;
        if ((mdata[0] != 'B') || (mdata[1] != 'M'))
        {
            if ((mdata[0] != 'B') || (mdata[1] != 'A')) { setLastError("OS/2 bitmap array BMP files are not supported"); return false; }
            else { setLastError("Does not appear to be a bitmap file"); return false; }
        }
        mBitmapOffset = mdata[13]; mBitmapOffset <<= 8; mBitmapOffset += mdata[12]; mBitmapOffset <<= 8; mBitmapOffset += mdata[11]; mBitmapOffset <<= 8; mBitmapOffset += mdata[10];
        const S32 BITMAP_HEADER_SIZE = 40;
        LLBMPHeader header; memcpy((void*)&header, mdata + FILE_HEADER_SIZE, BITMAP_HEADER_SIZE);
        bool windows_nt_version = false; bool windows_95_version = false;
        if (12 == header.mSize) { setLastError("Windows 2.x and OS/2 1.x BMP files are not supported"); return false; }
        else if (40 == header.mSize) { if (3 == header.mCompression) windows_nt_version = true; }
        else if (12 <= header.mSize && header.mSize <= 64) { setLastError("OS/2 2.x BMP files are not supported"); return false; }
        else if (108 == header.mSize) windows_95_version = true;
        else if (108 < header.mSize) windows_95_version = true;
        S32 width = header.mWidth; S32 height = header.mHeight;
        if (height < 0) { mOriginAtTop = true; height = -height; } else mOriginAtTop = false;
        mBitsPerPixel = header.mBitsPerPixel;
        S32 components;
        switch (mBitsPerPixel)
        {
        case 8: components = 1; break;
        case 24: case 32: components = 3; break;
        case 1: case 4: case 16: setLastError("Unsupported bit depth"); return false;
        default: setLastError("Unrecognized bit depth"); return false;
        }
        setSize(width, height, components);
        switch (header.mCompression)
        {
        case 0: break;
        case 1: setLastError("8 bit RLE compression not supported."); return false;
        case 2: setLastError("4 bit RLE compression not supported."); return false;
        case 3: break;
        default: setLastError("Unsupported compression format."); return false;
        }
        S32 extension_size = 0;
        if (windows_nt_version)
        {
            if ((16 != header.mBitsPerPixel) && (32 != header.mBitsPerPixel)) { setLastError("Bitfield encoding requires 16 or 32 bits per pixel."); return false; }
            if (0 != header.mNumColors) { setLastError("Bitfield encoding is not compatible with a color table."); return false; }
            extension_size = 4 * 3;
            memcpy(mBitfieldMask, mdata + FILE_HEADER_SIZE + BITMAP_HEADER_SIZE, extension_size);
        }
        else if (windows_95_version)
        {
            Win95BmpHeaderExtension win_95_extension; extension_size = sizeof(win_95_extension);
            memcpy(&win_95_extension, mdata + FILE_HEADER_SIZE + BITMAP_HEADER_SIZE, sizeof(win_95_extension));
            if (3 == header.mCompression) memcpy(mBitfieldMask, mdata + FILE_HEADER_SIZE + BITMAP_HEADER_SIZE, 4 * 4);
        }
        S32 color_palette_size = 0; mColorPaletteColors = 0;
        if (header.mBitsPerPixel < 16) { if (0 == header.mNumColors) mColorPaletteColors = (1 << header.mBitsPerPixel); else mColorPaletteColors = header.mNumColors; }
        color_palette_size = mColorPaletteColors * 4;
        if (0 != mColorPaletteColors) { mColorPalette = new U8[color_palette_size]; memcpy(mColorPalette, mdata + FILE_HEADER_SIZE + BITMAP_HEADER_SIZE + extension_size, color_palette_size); }
        return true;
    }
    bool decode(Raster* raw_image)
    {
        const U8* mdata = getData();
        if (mData.empty()) { setLastError("llimagebmp trying to decode an image with no data!"); return false; }
        if (!raw_image->resize(getWidth(), getHeight(), 3)) { setLastError("llimagebmp failed to resize image!"); return false; }
        const U8* src = mdata + mBitmapOffset; U8* dst = raw_image->getData();
        bool success = false;
        switch (mBitsPerPixel)
        {
        case 8: if (mColorPaletteColors >= 256) success = decodeColorTable8(dst, src); break;
        case 16: success = decodeColorMask16(dst, src); break;
        case 24: success = decodeTruecolor24(dst, src); break;
        case 32: success = decodeColorMask32(dst, src); break;
        }
        if (success && mOriginAtTop) raw_image->verticalFlip();
        return success;
    }
    U32 countTrailingZeros(U32 m) { U32 shift_count = 0; while (!(m & 1)) { shift_count++; m >>= 1; } return shift_count; }
    bool decodeColorMask16(U8* dst, const U8* src)
    {
        if (!mBitfieldMask[0] && !mBitfieldMask[1] && !mBitfieldMask[2]) { mBitfieldMask[0] = 0x00007C00; mBitfieldMask[1] = 0x000003E0; mBitfieldMask[2] = 0x0000001F; }
        S32 src_row_span = getWidth() * 2; S32 alignment_bytes = (3 * src_row_span) % 4;
        U32 r_shift = countTrailingZeros(mBitfieldMask[2]); U32 g_shift = countTrailingZeros(mBitfieldMask[1]); U32 b_shift = countTrailingZeros(mBitfieldMask[0]);
        for (S32 row = 0; row < getHeight(); row++) { for (S32 col = 0; col < getWidth(); col++) { U32 value = *((U16*)src); dst[0] = U8((value & mBitfieldMask[2]) >> r_shift); dst[1] = U8((value & mBitfieldMask[1]) >> g_shift); dst[2] = U8((value & mBitfieldMask[0]) >> b_shift); src += 2; dst += 3; } src += alignment_bytes; }
        return true;
    }
    bool decodeColorMask32(U8* dst, const U8* src)
    {
        if (!mBitfieldMask[0] && !mBitfieldMask[1] && !mBitfieldMask[2]) { mBitfieldMask[0] = 0x00FF0000; mBitfieldMask[1] = 0x0000FF00; mBitfieldMask[2] = 0x000000FF; }
        if (getWidth() * getHeight() * 4 > getDataSize() - mBitmapOffset) return false;
        S32 src_row_span = getWidth() * 4; S32 alignment_bytes = (3 * src_row_span) % 4;
        U32 r_shift = countTrailingZeros(mBitfieldMask[0]); U32 g_shift = countTrailingZeros(mBitfieldMask[1]); U32 b_shift = countTrailingZeros(mBitfieldMask[2]);
        for (S32 row = 0; row < getHeight(); row++) { for (S32 col = 0; col < getWidth(); col++) { U32 value = *((U32*)src); dst[0] = U8((value & mBitfieldMask[0]) >> r_shift); dst[1] = U8((value & mBitfieldMask[1]) >> g_shift); dst[2] = U8((value & mBitfieldMask[2]) >> b_shift); src += 4; dst += 3; } src += alignment_bytes; }
        return true;
    }
    bool decodeColorTable8(U8* dst, const U8* src)
    {
        S32 src_row_span = getWidth() * 1; S32 alignment_bytes = (3 * src_row_span) % 4;
        if ((getWidth() * getHeight()) + getHeight() * alignment_bytes > getDataSize() - mBitmapOffset) return false;
        for (S32 row = 0; row < getHeight(); row++) { for (S32 col = 0; col < getWidth(); col++) { S32 index = 4 * src[0]; dst[0] = mColorPalette[index + 2]; dst[1] = mColorPalette[index + 1]; dst[2] = mColorPalette[index + 0]; src++; dst += 3; } src += alignment_bytes; }
        return true;
    }
    bool decodeTruecolor24(U8* dst, const U8* src)
    {
        S32 src_row_span = getWidth() * 3; S32 alignment_bytes = (3 * src_row_span) % 4;
        if ((getWidth() * getHeight() * 3) + getHeight() * alignment_bytes > getDataSize() - mBitmapOffset) return false;
        for (S32 row = 0; row < getHeight(); row++) { for (S32 col = 0; col < getWidth(); col++) { dst[0] = src[2]; dst[1] = src[1]; dst[2] = src[0]; src += 3; dst += 3; } src += alignment_bytes; }
        return true;
    }
    bool encode(const Raster* raw_image, S32 pels_per_meter)
    {
        S32 src_components = raw_image->getComponents(); S32 dst_components = (src_components < 3) ? 1 : 3;
        setSize(raw_image->getWidth(), raw_image->getHeight(), dst_components);
        U8 magic[14]; LLBMPHeader header; int header_bytes = 14 + sizeof(header);
        if (getComponents() == 1) header_bytes += 1024;
        int line_bytes = getComponents() * getWidth(); int alignment_bytes = (3 * line_bytes) % 4; line_bytes += alignment_bytes;
        int file_bytes = line_bytes * getHeight() + header_bytes;
        mData.assign(file_bytes, 0);
        magic[0] = 'B'; magic[1] = 'M'; magic[2] = (U8)file_bytes; magic[3] = (U8)(file_bytes >> 8); magic[4] = (U8)(file_bytes >> 16); magic[5] = (U8)(file_bytes >> 24);
        magic[6] = magic[7] = magic[8] = magic[9] = 0; magic[10] = (U8)header_bytes; magic[11] = (U8)(header_bytes >> 8); magic[12] = (U8)(header_bytes >> 16); magic[13] = (U8)(header_bytes >> 24);
        header.mSize = 40; header.mWidth = getWidth(); header.mHeight = getHeight(); header.mPlanes = 1; header.mBitsPerPixel = (getComponents() == 1) ? 8 : 24; header.mCompression = 0; header.mAlignmentPadding = 0; header.mImageSize = 0;
        header.mHorzPelsPerMeter = header.mVertPelsPerMeter = pels_per_meter; header.mNumColors = header.mNumColorsImportant = 0;
        U8* mdata = getData(); U32 cur_pos = 0;
        memcpy(mdata, magic, 14); cur_pos += 14; memcpy(mdata + cur_pos, &header, 40); cur_pos += 40;
        if (getComponents() == 1) { for (S32 n = 0; n < 256; n++) { mdata[cur_pos++] = (U8)n; mdata[cur_pos++] = (U8)n; mdata[cur_pos++] = (U8)n; mdata[cur_pos++] = 0; } }
        const U8* src = raw_image->getData(); U8* dst = mdata + cur_pos;
        for (S32 row = 0; row < getHeight(); row++)
        {
            for (S32 col = 0; col < getWidth(); col++)
            {
                switch (src_components)
                {
                case 1: *dst++ = *src++; break;
                case 2: { U32 lum = src[0]; U32 alpha = src[1]; *dst++ = (U8)(lum * alpha / 255); src += 2; break; }
                case 3: case 4: dst[0] = src[2]; dst[1] = src[1]; dst[2] = src[0]; src += src_components; dst += 3; break;
                }
            }
            for (S32 i = 0; i < alignment_bytes; i++) *dst++ = 0;
        }
        return true;
    }
};

// ---------------------------------------------------------------------------
// DXT: LLImageDXT verbatim (llimagedxt.cpp) with LLImageBase::generateMip.
// ---------------------------------------------------------------------------
static const S32 MAX_IMAGE_MIP = 12;
static void generateMip(const U8* indata, U8* mipdata, S32 width, S32 height, S32 nchannels)
{
    U8* data = mipdata; S32 in_width = width * 2;
    for (S32 h = 0; h < height; h++)
    {
        for (S32 w = 0; w < width; w++)
        {
            for (S32 k = 0; k < nchannels; ++k) data[k] = (U8)(((U32)(indata[k]) + indata[nchannels + k] + indata[nchannels * in_width + k] + indata[nchannels * in_width + nchannels + k]) >> 2);
            indata += nchannels * 2; data += nchannels;
        }
        indata += nchannels * in_width;
    }
}
class LLImageDXT
{
public:
    enum EFileFormat { FORMAT_UNKNOWN = 0, FORMAT_I8 = 1, FORMAT_A8, FORMAT_RGB8, FORMAT_RGBA8, FORMAT_DXT1, FORMAT_DXT2, FORMAT_DXT3, FORMAT_DXT4, FORMAT_DXT5, FORMAT_DXR1, FORMAT_DXR2, FORMAT_DXR3, FORMAT_DXR4, FORMAT_DXR5, FORMAT_NOFILE = 0xff };
    struct dxtfile_header_old_t { S32 format; S32 maxlevel; S32 maxwidth; S32 maxheight; };
    struct dxtfile_header_t { S32 fourcc; S32 header_size; S32 flags; S32 maxheight; S32 maxwidth; S32 image_size; S32 depth; S32 num_mips; S32 reserved[11]; struct pixel_format { S32 struct_size; S32 flags; S32 fourcc; S32 bit_count; S32 r_mask; S32 g_mask; S32 b_mask; S32 a_mask; } pixel_fmt; S32 caps[4]; S32 reserved2; };
    std::vector<U8> mData; U32 mWidth = 0, mHeight = 0, mComponents = 0; EFileFormat mFileFormat = FORMAT_UNKNOWN; S32 mHeaderSize = 0; S8 mDiscardLevel = 0;
    U8* getData() { return mData.data(); } S32 getDataSize() { return (S32)mData.size(); }
    S32 getWidth() { return mWidth; } S32 getHeight() { return mHeight; } S32 getComponents() { return mComponents; }
    void setSize(S32 w, S32 h, S32 c) { mWidth = (U16)w; mHeight = (U16)h; mComponents = (S8)c; }
    static void checkMinWidthHeight(EFileFormat format, S32& width, S32& height) { S32 mindim = (format >= FORMAT_DXT1 && format <= FORMAT_DXR5) ? 4 : 1; width = llmax(width, mindim); height = llmax(height, mindim); }
    static S32 formatBits(EFileFormat format)
    {
        switch (format) { case FORMAT_DXT1: return 4; case FORMAT_DXR1: return 4; case FORMAT_I8: return 8; case FORMAT_A8: return 8; case FORMAT_DXT3: return 8; case FORMAT_DXR3: return 8; case FORMAT_DXR5: return 8; case FORMAT_DXT5: return 8; case FORMAT_RGB8: return 24; case FORMAT_RGBA8: return 32; default: throw std::runtime_error("LLImageDXT::Unknown format: " + std::to_string((int)format)); }
    }
    static S32 formatBytes(EFileFormat format, S32 width, S32 height) { checkMinWidthHeight(format, width, height); S32 bytes = ((width * height * formatBits(format) + 7) >> 3); return (bytes + 3) & ~3; }
    static S32 formatComponents(EFileFormat format)
    {
        switch (format) { case FORMAT_DXT1: return 3; case FORMAT_DXR1: return 3; case FORMAT_I8: return 1; case FORMAT_A8: return 1; case FORMAT_DXT3: return 4; case FORMAT_DXR3: return 4; case FORMAT_DXT5: return 4; case FORMAT_DXR5: return 4; case FORMAT_RGB8: return 3; case FORMAT_RGBA8: return 4; default: throw std::runtime_error("LLImageDXT::Unknown format: " + std::to_string((int)format)); }
    }
    static EFileFormat getFormat(S32 fourcc)
    {
        switch (fourcc) { case 0x20203849: return FORMAT_I8; case 0x20203841: return FORMAT_A8; case 0x20424752: return FORMAT_RGB8; case 0x41424752: return FORMAT_RGBA8; case 0x31525844: return FORMAT_DXR1; case 0x32525844: return FORMAT_DXR2; case 0x33525844: return FORMAT_DXR3; case 0x34525844: return FORMAT_DXR4; case 0x35525844: return FORMAT_DXR5; case 0x31545844: return FORMAT_DXT1; case 0x32545844: return FORMAT_DXT2; case 0x33545844: return FORMAT_DXT3; case 0x34545844: return FORMAT_DXT4; case 0x35545844: return FORMAT_DXT5; default: return FORMAT_UNKNOWN; }
    }
    static S32 getFourCC(EFileFormat format)
    {
        switch (format) { case FORMAT_I8: return 0x20203849; case FORMAT_A8: return 0x20203841; case FORMAT_RGB8: return 0x20424752; case FORMAT_RGBA8: return 0x41424752; case FORMAT_DXR1: return 0x31525844; case FORMAT_DXR2: return 0x32525844; case FORMAT_DXR3: return 0x33525844; case FORMAT_DXR4: return 0x34525844; case FORMAT_DXR5: return 0x35525844; case FORMAT_DXT1: return 0x31545844; case FORMAT_DXT2: return 0x32545844; case FORMAT_DXT3: return 0x33545844; case FORMAT_DXT4: return 0x34545844; case FORMAT_DXT5: return 0x35545844; default: return 0; }
    }
    static void calcDiscardWidthHeight(S32 discard_level, EFileFormat format, S32& width, S32& height) { while (discard_level > 0 && width > 1 && height > 1) { discard_level--; width >>= 1; height >>= 1; } checkMinWidthHeight(format, width, height); }
    static S32 calcNumMips(S32 width, S32 height) { S32 nmips = 0; while (width > 0 && height > 0) { width >>= 1; height >>= 1; nmips++; } return nmips; }
    S32 calcDataSize(S32 discard_level)
    {
        if (mFileFormat == FORMAT_UNKNOWN) throw std::runtime_error("calcDataSize called with unloaded LLImageDXT");
        if (discard_level < 0) discard_level = mDiscardLevel;
        S32 bytes = getMipOffset(discard_level); S32 w = getWidth() >> discard_level; S32 h = getHeight() >> discard_level;
        bytes += formatBytes(mFileFormat, w, h); return bytes;
    }
    S32 calcDiscardLevelBytes(S32 bytes) { S32 discard_level = 0; while (1) { S32 bytes_needed = calcDataSize(discard_level); if (bytes_needed <= bytes) break; discard_level++; if (discard_level > MAX_IMAGE_MIP) return -1; } return discard_level; }
    bool updateData()
    {
        U8* data = getData(); S32 data_size = getDataSize();
        if (!data || !data_size) { setLastError("LLImageDXT uninitialized"); return false; }
        S32 width, height, miplevelmax;
        dxtfile_header_t* header = (dxtfile_header_t*)data;
        if (header->fourcc != 0x20534444)
        {
            dxtfile_header_old_t* oldheader = (dxtfile_header_old_t*)header; mHeaderSize = sizeof(dxtfile_header_old_t); mFileFormat = EFileFormat(oldheader->format);
            miplevelmax = llmin(oldheader->maxlevel, MAX_IMAGE_MIP); width = oldheader->maxwidth; height = oldheader->maxheight;
        }
        else { mHeaderSize = sizeof(dxtfile_header_t); mFileFormat = getFormat(header->pixel_fmt.fourcc); miplevelmax = llmin(header->num_mips - 1, MAX_IMAGE_MIP); width = header->maxwidth; height = header->maxheight; }
        if (data_size < mHeaderSize) throw std::runtime_error("LLImageDXT: not enough data");
        S32 ncomponents = formatComponents(mFileFormat);
        setSize(width, height, ncomponents);
        S32 discard = calcDiscardLevelBytes(data_size); discard = llmin(discard, miplevelmax); mDiscardLevel = (S8)discard;
        return true;
    }
    S32 getMipOffset(S32 discard)
    {
        if (mFileFormat >= FORMAT_DXT1 && mFileFormat <= FORMAT_DXT5) throw std::runtime_error("getMipOffset called with old (unsupported) format");
        S32 width = getWidth(), height = getHeight(); S32 num_mips = calcNumMips(width, height);
        discard = llclamp(discard, 0, num_mips - 1); S32 last_mip = num_mips - 1 - discard;
        S32 offset = mHeaderSize;
        for (S32 mipidx = num_mips - 1; mipidx >= 0; mipidx--) { if (mipidx < last_mip) offset += formatBytes(mFileFormat, width, height); width >>= 1; height >>= 1; }
        return offset;
    }
    bool decode(Raster* raw_image)
    {
        if (mFileFormat >= FORMAT_DXT1 && mFileFormat <= FORMAT_DXR5) { setLastError("Attempt to decode compressed LLImageDXT to Raw (unsupported)"); return false; }
        S32 width = getWidth(), height = getHeight(); S32 ncomponents = getComponents(); U8* data = NULL;
        if (mDiscardLevel >= 0) { data = getData() + getMipOffset(mDiscardLevel); calcDiscardWidthHeight(mDiscardLevel, mFileFormat, width, height); }
        else data = getData() + getMipOffset(0);
        S32 image_size = formatBytes(mFileFormat, width, height);
        if ((!getData()) || (data + image_size > getData() + getDataSize())) { setLastError("LLImageDXT trying to decode an image with not enough data!"); return false; }
        raw_image->resize(width, height, ncomponents);
        // The viewer memcpys image_size (padded) bytes; the raster holds width*height*components.
        memcpy(raw_image->getData(), data, llmin(image_size, raw_image->getDataSize()));
        return true;
    }
    bool encode(const Raster* raw_image)
    {
        S32 ncomponents = raw_image->getComponents(); EFileFormat format;
        switch (ncomponents) { case 1: format = FORMAT_A8; break; case 3: format = FORMAT_RGB8; break; case 4: format = FORMAT_RGBA8; break; default: setLastError("LLImageDXT::encode: Unhandled channel number: " + std::to_string(ncomponents)); return false; }
        S32 width = raw_image->getWidth(); S32 height = raw_image->getHeight();
        setSize(width, height, ncomponents); mHeaderSize = sizeof(dxtfile_header_t); mFileFormat = format;
        S32 nmips = calcNumMips(width, height); S32 w = width; S32 h = height; S32 totbytes = mHeaderSize;
        for (S32 mip = 0; mip < nmips; mip++) { totbytes += formatBytes(format, w, h); w >>= 1; h >>= 1; }
        mData.assign(totbytes, 0); // zeroed: the viewer left padding bytes uninitialised
        U8* data = getData(); dxtfile_header_t* header = (dxtfile_header_t*)data;
        memset(header, 0, mHeaderSize); header->fourcc = 0x20534444; header->pixel_fmt.fourcc = getFourCC(format); header->num_mips = nmips; header->maxwidth = width; header->maxheight = height;
        U8* prev_mipdata = 0; w = width, h = height;
        for (S32 mip = 0; mip < nmips; mip++)
        {
            U8* mipdata = data + getMipOffset(mip); S32 bytes = formatBytes(format, w, h);
            if (mip == 0) memcpy(mipdata, raw_image->getData(), llmin(bytes, raw_image->getDataSize())); // the viewer reads `bytes` (over-read of up to 3 bytes)
            else generateMip(prev_mipdata, mipdata, w, h, ncomponents);
            w >>= 1; h >>= 1; checkMinWidthHeight(format, w, h); prev_mipdata = mipdata;
        }
        return true;
    }
};

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------
static bool decodeByCodec(const std::string& codec, std::vector<U8>& in, Raster& out)
{
    if (codec == "png") return pngDecode(in, out);
    if (codec == "jpeg") return jpegDecode(in, out);
    if (codec == "tga") { LLImageTGA t; t.mData.swap(in); bool ok = t.updateData() && t.decode(&out); t.mData.swap(in); return ok; }
    if (codec == "bmp") { LLImageBMP b; b.mData.swap(in); bool ok = b.updateData() && b.decode(&out); b.mData.swap(in); return ok; }
    if (codec == "dxt") { LLImageDXT d; d.mData.swap(in); bool ok = false; try { ok = d.updateData() && d.decode(&out); } catch (...) { d.mData.swap(in); throw; } d.mData.swap(in); return ok; }
    setLastError("unknown codec " + codec); return false;
}
static bool encodeByCodec(const std::string& codec, const Raster& in, std::vector<U8>& out, int quality)
{
    if (codec == "png") return pngEncode(in, out);
    if (codec == "jpeg") return jpegEncode(in, out, quality);
    if (codec == "tga") { LLImageTGA t; if (!t.encode(&in)) return false; out = t.mData; return true; }
    if (codec == "bmp") { LLImageBMP b; if (!b.encode(&in, 0)) return false; out = b.mData; return true; }
    if (codec == "dxt") { LLImageDXT d; if (!d.encode(&in)) return false; out = d.mData; return true; }
    setLastError("unknown codec " + codec); return false;
}
static bool headerByCodec(const std::string& codec, std::vector<U8>& in, U32& w, U32& h, U32& c)
{
    if (codec == "png") return pngHeader(in, w, h, c);
    if (codec == "jpeg") return jpegHeader(in, w, h, c);
    if (codec == "tga") { LLImageTGA t; t.mData.swap(in); bool ok = t.updateData(); w = t.mWidth; h = t.mHeight; c = t.mComponents; t.mData.swap(in); return ok; }
    if (codec == "bmp") { LLImageBMP b; b.mData.swap(in); bool ok = b.updateData(); w = b.mWidth; h = b.mHeight; c = b.mComponents; b.mData.swap(in); return ok; }
    if (codec == "dxt") { LLImageDXT d; d.mData.swap(in); bool ok = false; try { ok = d.updateData(); } catch (...) { d.mData.swap(in); throw; } w = d.mWidth; h = d.mHeight; c = d.mComponents; d.mData.swap(in); return ok; }
    setLastError("unknown codec " + codec); return false;
}
static std::string extensionCodec(const std::string& path)
{
    size_t dot = path.rfind('.'); std::string e = dot == std::string::npos ? "" : path.substr(dot + 1);
    for (char& ch : e) ch = (char)tolower(ch);
    if (e == "jpg" || e == "jpeg") return "jpeg"; if (e == "mip") return "dxt"; return e;
}
static int fail(const std::string& what) { printf("FAIL: %s\n", what.empty() ? "(no message)" : what.c_str()); return 1; }

int main(int argc, char** argv)
{
    if (argc < 3) { fprintf(stderr, "usage: see source\n"); return 2; }
    std::string cmd = argv[1];
    try
    {
        if (cmd == "bench")
        {
            if (argc < 6) return 2;
            std::string codec = argv[2], op = argv[3]; const char* path = argv[4]; int iters = atoi(argv[5]); int quality = argc > 6 ? atoi(argv[6]) : 75;
            double best = 1e300, total = 0; size_t out_size = 0;
            std::vector<U8> in; Raster raw;
            if (op == "encode") { if (!readRaw(path, raw)) return fail("cannot read raw " + std::string(path)); }
            else if (!readFile(path, in)) return fail("cannot read " + std::string(path));
            for (int i = 0; i < iters; ++i)
            {
                auto t0 = std::chrono::steady_clock::now();
                bool ok;
                if (op == "encode") { std::vector<U8> out; ok = encodeByCodec(codec, raw, out, quality); out_size = out.size(); }
                else if (op == "header") { U32 w, h, c; ok = headerByCodec(codec, in, w, h, c); out_size = (size_t)w * h * c; }
                else { Raster out; ok = decodeByCodec(codec, in, out); out_size = out.data.size(); }
                auto t1 = std::chrono::steady_clock::now();
                if (!ok) return fail(g_error);
                double ms = std::chrono::duration<double, std::milli>(t1 - t0).count();
                best = llmin(best, ms); total += ms;
            }
            printf("%.4f %.4f %zu\n", best, total / iters, out_size);
            return 0;
        }
        if (cmd == "crc")
        {
            std::vector<U8> in; if (!readFile(argv[2], in)) return fail("cannot read");
            Raster out; if (!decodeByCodec(extensionCodec(argv[2]), in, out)) return fail(g_error);
            unsigned long crc = crc32(0L, out.data.data(), (uInt)out.data.size());
            printf("%08lx %u %u %u\n", crc, out.w, out.h, out.c); return 0;
        }
        if (cmd == "crc-list")
        {
            // One line per path in the list file: "<crc32> <w> <h> <c> <path>" or "FAIL <path>".
            FILE* lf = fopen(argv[2], "r"); if (!lf) return fail("cannot read list");
            char line[4096];
            while (fgets(line, sizeof(line), lf))
            {
                std::string path(line); while (!path.empty() && (path.back() == '\n' || path.back() == '\r')) path.pop_back();
                if (path.empty()) continue;
                std::vector<U8> in; Raster out; bool ok = readFile(path.c_str(), in);
                if (ok) { try { ok = decodeByCodec(extensionCodec(path), in, out); } catch (const std::exception&) { ok = false; } }
                if (!ok) { printf("FAIL %s\n", path.c_str()); continue; }
                unsigned long crc = crc32(0L, out.data.data(), (uInt)out.data.size());
                printf("%08lx %u %u %u %s\n", crc, out.w, out.h, out.c, path.c_str());
            }
            fclose(lf); return 0;
        }
        if (cmd == "jpeg-dims")
        {
            std::vector<U8> in; if (!readFile(argv[2], in)) return fail("cannot read");
            U32 w, h; if (!jpegDims(in, w, h)) return fail(g_error); printf("%u %u\n", w, h); return 0;
        }
        size_t dash = cmd.find('-'); if (dash == std::string::npos) return 2;
        std::string codec = cmd.substr(0, dash), op = cmd.substr(dash + 1);
        if (op == "header")
        {
            std::vector<U8> in; if (!readFile(argv[2], in)) return fail("cannot read");
            U32 w, h, c; if (!headerByCodec(codec, in, w, h, c)) return fail(g_error); printf("%u %u %u\n", w, h, c); return 0;
        }
        if (argc < 4) return 2;
        if (op == "decode")
        {
            std::vector<U8> in; if (!readFile(argv[2], in)) return fail("cannot read");
            Raster out; if (!decodeByCodec(codec, in, out)) return fail(g_error);
            if (!writeRaw(argv[3], out)) return fail("cannot write"); printf("OK %u %u %u\n", out.w, out.h, out.c); return 0;
        }
        if (op == "encode")
        {
            Raster raw; if (!readRaw(argv[2], raw)) return fail("cannot read raw");
            int quality = argc > 4 ? atoi(argv[4]) : 75;
            std::vector<U8> out; if (!encodeByCodec(codec, raw, out, quality)) return fail(g_error);
            if (!writeFile(argv[3], out.data(), out.size())) return fail("cannot write"); printf("OK %zu\n", out.size()); return 0;
        }
        if (op == "process" && codec == "tga")
        {
            if (argc < 6) return 2;
            std::vector<U8> in; if (!readFile(argv[2], in)) return fail("cannot read");
            LLImageTGA t; t.mData = in; if (!t.updateData()) return fail(g_error);
            Raster out; if (!t.decodeAndProcess(&out, (F32)atof(argv[4]), (F32)atof(argv[5]))) return fail(g_error);
            if (!writeRaw(argv[3], out)) return fail("cannot write"); printf("OK %u %u %u\n", out.w, out.h, out.c); return 0;
        }
    }
    catch (const std::exception& e) { return fail(e.what()); }
    return 2;
}
