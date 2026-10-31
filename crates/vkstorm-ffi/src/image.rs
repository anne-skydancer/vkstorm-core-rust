//! Rung two: the raster codecs other than JPEG 2000 (`vks_png_*`,
//! `vks_tga_*`, `vks_bmp_*`, `vks_dxt_*`) via `vkstorm-image`. The matching
//! declarations are in `indra/llimage/vkstorm_ffi.h`.
//!
//! Every function catches panics at the boundary. Failures return 0 or null
//! and leave the viewer's message (the text `LLImageFormatted::setLastError`
//! received from the C++ implementation) in a thread-local buffer read with
//! `vks_image_last_error`.

use std::cell::RefCell;
use std::ffi::{CString, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::slice;

use vkstorm_image::{Error, Raster, bmp, dxt, png, tga};

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn set_error(message: &str) {
    let c = CString::new(message.replace('\0', " ")).unwrap_or_default();
    LAST_ERROR.with(|e| *e.borrow_mut() = c);
}

/// The message of the last failed `vks_png_*`, `vks_tga_*`, `vks_bmp_*` or
/// `vks_dxt_*` call on this thread, NUL-terminated, valid until the next
/// failing call on the thread.
#[unsafe(no_mangle)]
pub extern "C" fn vks_image_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

/// Run a codec call: panics and errors become a stored message and `None`.
fn guard<T>(f: impl FnOnce() -> Result<T, Error>) -> Option<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(v)) => Some(v),
        Ok(Err(e)) => {
            set_error(&e.message);
            None
        }
        Err(_) => {
            set_error("internal error in the Rust image codec");
            None
        }
    }
}

unsafe fn bytes<'a>(data: *const u8, len: usize) -> Option<&'a [u8]> {
    if data.is_null() {
        set_error("null input");
        None
    } else {
        Some(unsafe { slice::from_raw_parts(data, len) })
    }
}

/// Build a raster from the viewer's `width * height * components` bytes.
unsafe fn raster_from(
    data: *const u8,
    len: usize,
    width: u32,
    height: u32,
    components: u32,
) -> Option<Raster> {
    let b = unsafe { bytes(data, len) }?;
    let need = width as usize * height as usize * components as usize;
    if b.len() < need {
        set_error("Invalid input, no buffer");
        return None;
    }
    Some(Raster {
        width,
        height,
        components,
        data: b[..need].to_vec(),
    })
}

/// Dimensions and component count from a header read.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct VksImageHeader {
    pub width: u32,
    pub height: u32,
    pub components: u32,
}

/// A decoded image, rows bottom-up. Opaque to C++.
pub struct VksRaster {
    inner: Raster,
}

/// An encoded file. Opaque to C++.
pub struct VksBytes {
    bytes: Vec<u8>,
}

fn boxed_raster(r: Option<Raster>) -> *mut VksRaster {
    match r {
        Some(inner) => Box::into_raw(Box::new(VksRaster { inner })),
        None => std::ptr::null_mut(),
    }
}

fn boxed_bytes(b: Option<Vec<u8>>) -> *mut VksBytes {
    match b {
        Some(bytes) => Box::into_raw(Box::new(VksBytes { bytes })),
        None => std::ptr::null_mut(),
    }
}

/// # Safety
/// `r` must come from a `vks_*_decode` call and not have been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_raster_width(r: *const VksRaster) -> u32 {
    unsafe { &*r }.inner.width
}

/// # Safety
/// `r` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_raster_height(r: *const VksRaster) -> u32 {
    unsafe { &*r }.inner.height
}

/// # Safety
/// `r` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_raster_components(r: *const VksRaster) -> u32 {
    unsafe { &*r }.inner.components
}

/// The `width * height * components` bytes, bottom-up, valid until freed.
///
/// # Safety
/// `r` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_raster_data(r: *const VksRaster) -> *const u8 {
    unsafe { &*r }.inner.data.as_ptr()
}

/// # Safety
/// `r` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_raster_len(r: *const VksRaster) -> usize {
    unsafe { &*r }.inner.data.len()
}

/// Release a raster. Null is ignored.
///
/// # Safety
/// `r` must come from a `vks_*_decode` call and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_raster_free(r: *mut VksRaster) {
    if !r.is_null() {
        drop(unsafe { Box::from_raw(r) });
    }
}

/// # Safety
/// `b` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_bytes_data(b: *const VksBytes) -> *const u8 {
    unsafe { &*b }.bytes.as_ptr()
}

/// # Safety
/// `b` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_bytes_len(b: *const VksBytes) -> usize {
    unsafe { &*b }.bytes.len()
}

/// Release an encoded file. Null is ignored.
///
/// # Safety
/// `b` must come from a `vks_*_encode` call and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_bytes_free(b: *mut VksBytes) {
    if !b.is_null() {
        drop(unsafe { Box::from_raw(b) });
    }
}

// ---------------------------------------------------------------------------
// PNG
// ---------------------------------------------------------------------------

/// Read the PNG header: 1 on success and `out` filled, 0 on failure.
///
/// # Safety
/// `data` must point to `len` readable bytes; `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_png_header(
    data: *const u8,
    len: usize,
    out: *mut VksImageHeader,
) -> i32 {
    let Some(b) = (unsafe { bytes(data, len) }) else {
        return 0;
    };
    match guard(|| png::header(b)) {
        Some(h) if !out.is_null() => {
            unsafe {
                *out = VksImageHeader {
                    width: h.width,
                    height: h.height,
                    components: h.components,
                }
            };
            1
        }
        _ => 0,
    }
}

/// Decode a PNG. Null on failure.
///
/// # Safety
/// `data` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_png_decode(data: *const u8, len: usize) -> *mut VksRaster {
    let Some(b) = (unsafe { bytes(data, len) }) else {
        return std::ptr::null_mut();
    };
    boxed_raster(guard(|| png::decode(b)))
}

/// Encode `width * height * components` bottom-up bytes as PNG. Null on failure.
///
/// # Safety
/// `data` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_png_encode(
    data: *const u8,
    len: usize,
    width: u32,
    height: u32,
    components: u32,
) -> *mut VksBytes {
    let Some(r) = (unsafe { raster_from(data, len, width, height, components) }) else {
        return std::ptr::null_mut();
    };
    boxed_bytes(guard(|| png::encode(&r)))
}

// ---------------------------------------------------------------------------
// TGA
// ---------------------------------------------------------------------------

/// What `LLImageTGA::updateData` keeps from the header.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct VksTgaHeader {
    pub width: u32,
    pub height: u32,
    pub components: u32,
    pub image_type: u32,
    pub origin_top: u32,
    pub origin_right: u32,
}

/// Parse the TGA header: 1 on success and `out` filled, 0 on failure.
///
/// # Safety
/// `data` must point to `len` readable bytes; `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_tga_header(
    data: *const u8,
    len: usize,
    out: *mut VksTgaHeader,
) -> i32 {
    let Some(b) = (unsafe { bytes(data, len) }) else {
        return 0;
    };
    match guard(|| tga::parse(b)) {
        Some(i) if !out.is_null() => {
            unsafe {
                *out = VksTgaHeader {
                    width: i.width,
                    height: i.height,
                    components: i.components,
                    image_type: i.image_type as u32,
                    origin_top: i.origin_top as u32,
                    origin_right: i.origin_right as u32,
                }
            };
            1
        }
        _ => 0,
    }
}

/// Decode a TGA. Null on failure.
///
/// # Safety
/// `data` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_tga_decode(data: *const u8, len: usize) -> *mut VksRaster {
    let Some(b) = (unsafe { bytes(data, len) }) else {
        return std::ptr::null_mut();
    };
    boxed_raster(guard(|| tga::decode(b)))
}

/// `LLImageTGA::decodeAndProcess`: a monochrome run-length TGA through the
/// avatar alpha-gradient function. Null on failure.
///
/// # Safety
/// `data` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_tga_decode_and_process(
    data: *const u8,
    len: usize,
    domain: f32,
    weight: f32,
) -> *mut VksRaster {
    let Some(b) = (unsafe { bytes(data, len) }) else {
        return std::ptr::null_mut();
    };
    boxed_raster(guard(|| tga::decode_and_process(b, domain, weight)))
}

/// Encode as uncompressed TGA. Null on failure.
///
/// # Safety
/// `data` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_tga_encode(
    data: *const u8,
    len: usize,
    width: u32,
    height: u32,
    components: u32,
) -> *mut VksBytes {
    let Some(r) = (unsafe { raster_from(data, len, width, height, components) }) else {
        return std::ptr::null_mut();
    };
    boxed_bytes(guard(|| tga::encode(&r)))
}

// ---------------------------------------------------------------------------
// BMP
// ---------------------------------------------------------------------------

/// Parse the BMP headers: 1 on success and `out` filled (components as
/// `LLImageBMP` reports them: 1 for 8-bit files), 0 on failure.
///
/// # Safety
/// `data` must point to `len` readable bytes; `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_bmp_header(
    data: *const u8,
    len: usize,
    out: *mut VksImageHeader,
) -> i32 {
    let Some(b) = (unsafe { bytes(data, len) }) else {
        return 0;
    };
    match guard(|| bmp::parse(b)) {
        Some(i) if !out.is_null() => {
            unsafe {
                *out = VksImageHeader {
                    width: i.width,
                    height: i.height,
                    components: i.components,
                }
            };
            1
        }
        _ => 0,
    }
}

/// Decode a BMP to 3 components. Null on failure.
///
/// # Safety
/// `data` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_bmp_decode(data: *const u8, len: usize) -> *mut VksRaster {
    let Some(b) = (unsafe { bytes(data, len) }) else {
        return std::ptr::null_mut();
    };
    boxed_raster(guard(|| bmp::decode(b)))
}

/// Encode as 8-bit (1 or 2 components) or 24-bit BMP; `pels_per_meter`
/// fills the resolution fields. Null on failure.
///
/// # Safety
/// `data` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_bmp_encode(
    data: *const u8,
    len: usize,
    width: u32,
    height: u32,
    components: u32,
    pels_per_meter: i32,
) -> *mut VksBytes {
    let Some(r) = (unsafe { raster_from(data, len, width, height, components) }) else {
        return std::ptr::null_mut();
    };
    boxed_bytes(guard(|| bmp::encode(&r, pels_per_meter)))
}

// ---------------------------------------------------------------------------
// DXT
// ---------------------------------------------------------------------------

/// What `LLImageDXT::updateData` keeps: the full-image dimensions, the
/// format code (`LLImageDXT::EFileFormat`), the header size and the discard
/// level the data supports.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct VksDxtHeader {
    pub width: u32,
    pub height: u32,
    pub components: u32,
    pub format: i32,
    pub header_size: i32,
    pub discard_level: i32,
}

fn dxt_info(h: &VksDxtHeader) -> dxt::Info {
    dxt::Info {
        format: dxt::Format::from_code(h.format),
        header_size: h.header_size,
        width: h.width,
        height: h.height,
        components: h.components,
        discard_level: h.discard_level,
    }
}

/// Parse the DXT header: 1 on success and `out` filled, 0 on failure.
///
/// # Safety
/// `data` must point to `len` readable bytes; `out` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dxt_header(
    data: *const u8,
    len: usize,
    out: *mut VksDxtHeader,
) -> i32 {
    let Some(b) = (unsafe { bytes(data, len) }) else {
        return 0;
    };
    match guard(|| dxt::parse(b)) {
        Some(i) if !out.is_null() => {
            unsafe {
                *out = VksDxtHeader {
                    width: i.width,
                    height: i.height,
                    components: i.components,
                    format: i.format.code(),
                    header_size: i.header_size,
                    discard_level: i.discard_level,
                }
            };
            1
        }
        _ => 0,
    }
}

/// Decode the mip at the header's discard level. Null on failure.
///
/// # Safety
/// `data` must point to `len` readable bytes; `header` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dxt_decode(
    data: *const u8,
    len: usize,
    header: *const VksDxtHeader,
) -> *mut VksRaster {
    let Some(b) = (unsafe { bytes(data, len) }) else {
        return std::ptr::null_mut();
    };
    if header.is_null() {
        return std::ptr::null_mut();
    }
    let info = dxt_info(unsafe { &*header });
    boxed_raster(guard(|| dxt::decode(b, &info)))
}

/// `LLImageDXT::getMipData` for `discard` (-1 for the header's level). Null on failure.
///
/// # Safety
/// `data` must point to `len` readable bytes; `header` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dxt_mip_data(
    data: *const u8,
    len: usize,
    header: *const VksDxtHeader,
    discard: i32,
) -> *mut VksRaster {
    let Some(b) = (unsafe { bytes(data, len) }) else {
        return std::ptr::null_mut();
    };
    if header.is_null() {
        return std::ptr::null_mut();
    }
    let info = dxt_info(unsafe { &*header });
    boxed_raster(guard(|| dxt::mip_data(b, &info, discard)))
}

/// Encode a 1, 3 or 4 component raster as a DDS-headered file with its mip
/// chain. Null on failure.
///
/// # Safety
/// `data` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dxt_encode(
    data: *const u8,
    len: usize,
    width: u32,
    height: u32,
    components: u32,
) -> *mut VksBytes {
    let Some(r) = (unsafe { raster_from(data, len, width, height, components) }) else {
        return std::ptr::null_mut();
    };
    boxed_bytes(guard(|| dxt::encode(&r)))
}

/// `LLImageDXT::convertToDXR`: the file rewritten with DXR four-ccs and the
/// mips reordered, or null when there is nothing to convert or on failure.
///
/// # Safety
/// `data` must point to `len` readable bytes; `header` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dxt_convert_to_dxr(
    data: *const u8,
    len: usize,
    header: *const VksDxtHeader,
) -> *mut VksBytes {
    let Some(b) = (unsafe { bytes(data, len) }) else {
        return std::ptr::null_mut();
    };
    if header.is_null() {
        return std::ptr::null_mut();
    }
    let info = dxt_info(unsafe { &*header });
    boxed_bytes(guard(|| dxt::convert_to_dxr(b, &info)).flatten())
}

/// `LLImageDXT::calcDataSize`: bytes through the mip at `discard` (-1 for
/// the header's level); -1 on failure.
///
/// # Safety
/// `header` must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dxt_data_size(header: *const VksDxtHeader, discard: i32) -> i32 {
    if header.is_null() {
        return -1;
    }
    let info = dxt_info(unsafe { &*header });
    guard(|| dxt::data_size(&info, discard)).unwrap_or(-1)
}

/// `LLImageDXT::getMipOffset` for an image of `width` x `height`; -1 on failure.
#[unsafe(no_mangle)]
pub extern "C" fn vks_dxt_mip_offset(
    format: i32,
    header_size: i32,
    width: i32,
    height: i32,
    discard: i32,
) -> i32 {
    guard(|| {
        dxt::mip_offset(
            dxt::Format::from_code(format),
            header_size,
            width,
            height,
            discard,
        )
    })
    .unwrap_or(-1)
}

/// `LLImageDXT::formatBits`; 0 for an unknown format.
#[unsafe(no_mangle)]
pub extern "C" fn vks_dxt_format_bits(format: i32) -> i32 {
    guard(|| dxt::format_bits(dxt::Format::from_code(format))).unwrap_or(0)
}

/// `LLImageDXT::formatBytes`; 0 for an unknown format.
#[unsafe(no_mangle)]
pub extern "C" fn vks_dxt_format_bytes(format: i32, width: i32, height: i32) -> i32 {
    guard(|| dxt::format_bytes(dxt::Format::from_code(format), width, height)).unwrap_or(0)
}

/// `LLImageDXT::formatComponents`; 0 for an unknown format.
#[unsafe(no_mangle)]
pub extern "C" fn vks_dxt_format_components(format: i32) -> i32 {
    guard(|| dxt::format_components(dxt::Format::from_code(format))).unwrap_or(0)
}

/// `LLImageDXT::getFormat`: the format code for a pixel-format four-cc.
#[unsafe(no_mangle)]
pub extern "C" fn vks_dxt_format_from_fourcc(fourcc: i32) -> i32 {
    dxt::Format::from_fourcc(fourcc).code()
}

/// `LLImageDXT::getFourCC`.
#[unsafe(no_mangle)]
pub extern "C" fn vks_dxt_fourcc(format: i32) -> i32 {
    dxt::Format::from_code(format).fourcc()
}

/// `LLImageDXT::calcNumMips`.
#[unsafe(no_mangle)]
pub extern "C" fn vks_dxt_calc_num_mips(width: i32, height: i32) -> i32 {
    dxt::calc_num_mips(width, height)
}

/// `LLImageDXT::calcDiscardWidthHeight`.
///
/// # Safety
/// `width` and `height` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dxt_calc_discard_width_height(
    discard: i32,
    format: i32,
    width: *mut i32,
    height: *mut i32,
) {
    if width.is_null() || height.is_null() {
        return;
    }
    let (mut w, mut h) = unsafe { (*width, *height) };
    dxt::calc_discard_width_height(discard, dxt::Format::from_code(format), &mut w, &mut h);
    unsafe {
        *width = w;
        *height = h;
    }
}

/// `LLImageDXT::checkMinWidthHeight`.
///
/// # Safety
/// `width` and `height` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dxt_check_min_width_height(
    format: i32,
    width: *mut i32,
    height: *mut i32,
) {
    if width.is_null() || height.is_null() {
        return;
    }
    let (mut w, mut h) = unsafe { (*width, *height) };
    dxt::check_min_width_height(dxt::Format::from_code(format), &mut w, &mut h);
    unsafe {
        *width = w;
        *height = h;
    }
}
