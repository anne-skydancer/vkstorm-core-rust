//! Rung two: LLSD serialization via `vkstorm-llsd` (the `vks_llsd_*` calls).

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::slice;

// ---------------------------------------------------------------------------
// LLSD serialization (rung two)

use vkstorm_llsd::{
    FlatDoc, FlatSink, FloatField, Format, Formatter, Limits, Options, Out, Reader, Source, XmlMode,
};

/// Format ids shared with the C++ side.
const VKS_LLSD_NOTATION: i32 = 0;
const VKS_LLSD_BINARY: i32 = 1;
const VKS_LLSD_XML: i32 = 2;

fn format_from(id: i32) -> Option<Format> {
    match id {
        VKS_LLSD_NOTATION => Some(Format::Notation),
        VKS_LLSD_BINARY => Some(Format::Binary),
        VKS_LLSD_XML => Some(Format::Xml),
        _ => None,
    }
}

/// A byte source supplied by C++: `read` pulls up to `cap` bytes (0 = end of
/// input); `peek` returns the next byte or -1 and is only used when `exact`
/// is set, in which case the parser never reads ahead of what it consumes.
#[repr(C)]
pub struct VksLlsdSource {
    pub ctx: *mut c_void,
    pub read: Option<unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> usize>,
    pub peek: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
    pub exact: i32,
}

struct CSource<'a>(&'a VksLlsdSource);

impl Source for CSource<'_> {
    fn read(&mut self, buf: &mut [u8]) -> usize {
        match self.0.read {
            Some(f) => {
                let n = unsafe { f(self.0.ctx, buf.as_mut_ptr(), buf.len()) };
                n.min(buf.len())
            }
            None => 0,
        }
    }
    fn peek(&mut self) -> Option<u8> {
        match self.0.peek {
            Some(f) => {
                let c = unsafe { f(self.0.ctx) };
                if c < 0 { None } else { Some(c as u8) }
            }
            None => None,
        }
    }
    fn exact(&self) -> bool {
        self.0.exact != 0
    }
}

/// The outcome of a parse: the C++ `parse()` return value, the bytes the
/// parser consumed (prefix included) and the bytes it pulled from the source
/// but left unconsumed, for the caller to put back.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct VksLlsdParseResult {
    pub count: i32,
    pub consumed: usize,
    pub unconsumed: usize,
}

/// A parsed document: a node array plus a byte arena. See `FlatNode` in
/// vkstorm-llsd for the layout.
pub struct VksLlsdDoc {
    doc: FlatDoc,
}

/// Parses one LLSD value of `format` from `src`, after the optional `prefix`
/// bytes. `max_bytes` < 0 is unlimited; `max_depth` < 0 is unlimited;
/// `xml_lines` selects the C++ `parseLines()` end-of-input rule. Fills `out`
/// and returns the document when `out->count` is positive, else null.
///
/// # Safety
/// `src` and `out` must be valid; `prefix` must point to `prefix_len` bytes
/// unless null; the source callbacks must stay valid for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_llsd_parse(
    format: i32,
    src: *const VksLlsdSource,
    prefix: *const u8,
    prefix_len: usize,
    max_bytes: i64,
    max_depth: i32,
    xml_lines: i32,
    out: *mut VksLlsdParseResult,
) -> *mut VksLlsdDoc {
    if src.is_null() || out.is_null() {
        return std::ptr::null_mut();
    }
    let Some(fmt) = format_from(format) else {
        unsafe {
            *out = VksLlsdParseResult {
                count: -1,
                consumed: 0,
                unconsumed: 0,
            }
        };
        return std::ptr::null_mut();
    };
    let prefix: &[u8] = if prefix.is_null() {
        &[]
    } else {
        unsafe { slice::from_raw_parts(prefix, prefix_len) }
    };
    let src = unsafe { &*src };
    let r = catch_unwind(AssertUnwindSafe(|| {
        let mut csrc = CSource(src);
        let mut reader = Reader::with_prefix(prefix, &mut csrc);
        let mut sink = FlatSink::new();
        let limits = Limits::from_cpp(max_bytes, max_depth);
        let mode = if xml_lines != 0 {
            XmlMode::Lines
        } else {
            XmlMode::Parse
        };
        let count = vkstorm_llsd::parse_into(fmt, &mut reader, &mut sink, limits, mode);
        (count, reader.consumed(), reader.unconsumed(), sink.finish())
    }));
    match r {
        Ok((count, consumed, unconsumed, doc)) => {
            unsafe {
                *out = VksLlsdParseResult {
                    count,
                    consumed,
                    unconsumed,
                }
            };
            if count > 0 {
                Box::into_raw(Box::new(VksLlsdDoc { doc }))
            } else {
                std::ptr::null_mut()
            }
        }
        Err(_) => {
            unsafe {
                *out = VksLlsdParseResult {
                    count: -1,
                    consumed: 0,
                    unconsumed: 0,
                }
            };
            std::ptr::null_mut()
        }
    }
}

/// The document's nodes (16 bytes each) and their count.
///
/// # Safety
/// `d` must come from `vks_llsd_parse` and be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_llsd_doc_nodes(
    d: *const VksLlsdDoc,
    count: *mut usize,
) -> *const vkstorm_llsd::FlatNode {
    let d = unsafe { &*d };
    if !count.is_null() {
        unsafe { *count = d.doc.nodes.len() };
    }
    d.doc.nodes.as_ptr()
}

/// The document's byte arena and its length.
///
/// # Safety
/// `d` must come from `vks_llsd_parse` and be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_llsd_doc_bytes(d: *const VksLlsdDoc, len: *mut usize) -> *const u8 {
    let d = unsafe { &*d };
    if !len.is_null() {
        unsafe { *len = d.doc.bytes.len() };
    }
    d.doc.bytes.as_ptr()
}

/// Releases a document. Null is ignored.
///
/// # Safety
/// `d` must come from `vks_llsd_parse` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_llsd_doc_free(d: *mut VksLlsdDoc) {
    if !d.is_null() {
        drop(unsafe { Box::from_raw(d) });
    }
}

/// Output callback for the formatter: receives chunks of the serialization.
pub type VksLlsdWrite = Option<unsafe extern "C" fn(*mut c_void, *const u8, usize)>;

struct COut {
    ctx: *mut c_void,
    write: VksLlsdWrite,
}

impl Out for COut {
    fn write(&mut self, data: &[u8]) {
        if let Some(f) = self.write {
            if !data.is_empty() {
                unsafe { f(self.ctx, data.as_ptr(), data.len()) };
            }
        }
    }
}

/// A formatter for one document; drive it with the `vks_llsd_fmt_*` calls in
/// document order and end it with `vks_llsd_formatter_finish`.
pub struct VksLlsdFormatter {
    inner: Formatter<COut>,
}

/// Option bits for `vks_llsd_formatter_new`.
const VKS_LLSD_OPT_PRETTY: u32 = 1;
const VKS_LLSD_OPT_PRETTY_BINARY: u32 = 2;
const VKS_LLSD_OPT_BOOLALPHA: u32 = 4;

/// Starts a document of `format`. `precision` and `floatfield` (0 general,
/// 1 fixed, 2 scientific) are the destination stream's real settings. Null
/// for an unknown format.
///
/// # Safety
/// `write`, when set, must stay callable with `ctx` until finish.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_llsd_formatter_new(
    format: i32,
    options: u32,
    precision: i32,
    floatfield: i32,
    ctx: *mut c_void,
    write: VksLlsdWrite,
) -> *mut VksLlsdFormatter {
    let Some(fmt) = format_from(format) else {
        return std::ptr::null_mut();
    };
    let opts = Options {
        pretty: options & VKS_LLSD_OPT_PRETTY != 0,
        pretty_binary: options & VKS_LLSD_OPT_PRETTY_BINARY != 0,
        boolalpha: options & VKS_LLSD_OPT_BOOLALPHA != 0,
        precision: precision.max(0) as usize,
        floatfield: match floatfield {
            1 => FloatField::Fixed,
            2 => FloatField::Scientific,
            _ => FloatField::General,
        },
    };
    let r = catch_unwind(AssertUnwindSafe(|| {
        Formatter::new(fmt, opts, COut { ctx, write })
    }));
    match r {
        Ok(inner) => Box::into_raw(Box::new(VksLlsdFormatter { inner })),
        Err(_) => std::ptr::null_mut(),
    }
}

macro_rules! fmt_fn {
    ($name:ident, |$f:ident $(, $arg:ident : $ty:ty)*| $body:expr) => {
        /// One formatter call; see the C header.
        ///
        /// # Safety
        /// `f` must come from `vks_llsd_formatter_new` and be live; pointer
        /// arguments must be readable for their lengths.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(f: *mut VksLlsdFormatter $(, $arg: $ty)*) {
            if f.is_null() {
                return;
            }
            let $f = unsafe { &mut (*f).inner };
            $body
        }
    };
}

unsafe fn bytes<'a>(p: *const u8, len: usize) -> &'a [u8] {
    if p.is_null() || len == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(p, len) }
    }
}

fmt_fn!(vks_llsd_fmt_undef, |f| f.undef());
fmt_fn!(vks_llsd_fmt_boolean, |f, v: i32| f.boolean(v != 0));
fmt_fn!(vks_llsd_fmt_integer, |f, v: i32| f.integer(v));
fmt_fn!(vks_llsd_fmt_real, |f, v: f64| f.real(v));
fmt_fn!(
    vks_llsd_fmt_real_text,
    |f, v: f64, p: *const u8, len: usize| f.real_text(v, unsafe { bytes(p, len) })
);
fmt_fn!(vks_llsd_fmt_uuid, |f, p: *const u8| {
    let mut u = [0u8; 16];
    if !p.is_null() {
        u.copy_from_slice(unsafe { slice::from_raw_parts(p, 16) });
    }
    f.uuid(&u)
});
fmt_fn!(vks_llsd_fmt_string, |f, p: *const u8, len: usize| f
    .string(unsafe { bytes(p, len) }));
fmt_fn!(vks_llsd_fmt_date, |f, seconds: f64| f.date(seconds));
fmt_fn!(vks_llsd_fmt_uri, |f, p: *const u8, len: usize| f
    .uri(unsafe { bytes(p, len) }));
fmt_fn!(vks_llsd_fmt_binary, |f, p: *const u8, len: usize| f
    .binary(unsafe { bytes(p, len) }));
fmt_fn!(vks_llsd_fmt_map_begin, |f, count: usize| f.begin_map(count));
fmt_fn!(vks_llsd_fmt_key, |f, p: *const u8, len: usize| f
    .key(unsafe { bytes(p, len) }));
fmt_fn!(vks_llsd_fmt_map_end, |f| f.end_map());
fmt_fn!(vks_llsd_fmt_array_begin, |f, count: usize| f
    .begin_array(count));
fmt_fn!(vks_llsd_fmt_array_end, |f| f.end_array());

/// Ends the document, flushes the remaining output, frees the formatter and
/// returns the number of values written (-1 if the formatter panicked).
///
/// # Safety
/// `f` must come from `vks_llsd_formatter_new` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_llsd_formatter_finish(f: *mut VksLlsdFormatter) -> i32 {
    if f.is_null() {
        return -1;
    }
    let b = unsafe { Box::from_raw(f) };
    match catch_unwind(AssertUnwindSafe(|| b.inner.finish().0)) {
        Ok(n) => n,
        Err(_) => -1,
    }
}

/// Writes the notation (`kind` 0) or XML (`kind` 1) escape of `len` bytes at
/// `p` to `write`.
///
/// # Safety
/// `p` must be readable for `len` bytes; `write` must be callable with `ctx`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_llsd_escape(
    kind: i32,
    p: *const u8,
    len: usize,
    ctx: *mut c_void,
    write: VksLlsdWrite,
) {
    let input = unsafe { bytes(p, len) };
    let mut out = Vec::with_capacity(len + 8);
    match kind {
        0 => vkstorm_llsd::text::notation_escape(input, &mut out),
        _ => vkstorm_llsd::text::xml_escape(input, &mut out),
    }
    COut { ctx, write }.write(&out);
}
