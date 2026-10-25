//! Rung three: the message template system via `vkstorm-messages`
//! (`vks_msg_*`: template parsing, the message builder and reader,
//! zero-coding) and the data packers (`vks_dp_*`).
//!
//! The C++ `LLMessageTemplate` objects stay the viewer's representation;
//! each compiles itself on demand into a `VksMsgTemplate` whose blocks and
//! variables carry the canonical string pointers the viewer passes to the
//! builder and reader as keys. Errors the C++ aborted on come back as
//! `VKS_MSG_ERR_*` codes with the message text in `vks_msg_last_error`, and
//! the C++ side aborts with it. Panics are caught at the boundary and
//! reported as `VKS_MSG_ERR_INTERNAL`.

use std::cell::RefCell;
use std::ffi::{c_char, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::slice;
use std::sync::{Arc, OnceLock};

use vkstorm_messages::datapacker::{self, AsciiBuffer, BinaryBuffer, TextKind};
use vkstorm_messages::reader::{decode_number, NumberError};
use vkstorm_messages::template::{Block, BlockType, Deprecation, Encoding, Frequency, Template, Trust, VarType, Variable};
use vkstorm_messages::{parser, zerocode, BuildError, Builder, BuilderError, GetError, Reader, Tokenizer};

pub const VKS_MSG_OK: i32 = 0;
pub const VKS_MSG_ERR_INTERNAL: i32 = -1;
pub const VKS_MSG_ERR_NO_MESSAGE: i32 = -2;
pub const VKS_MSG_ERR_NOT_A_BLOCK: i32 = -3;
pub const VKS_MSG_ERR_SINGLE_REPEATED: i32 = -4;
pub const VKS_MSG_ERR_MULTIPLE_EXCEEDED: i32 = -5;
pub const VKS_MSG_ERR_TOO_MANY_BLOCKS: i32 = -6;
pub const VKS_MSG_ERR_NO_BLOCK: i32 = -7;
pub const VKS_MSG_ERR_NOT_A_VARIABLE: i32 = -8;
pub const VKS_MSG_ERR_SIZE_MISMATCH: i32 = -9;
pub const VKS_MSG_ERR_BUILD: i32 = -10;

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn set_error(message: impl Into<Vec<u8>>) {
    let c = CString::new(message).unwrap_or_default();
    LAST_ERROR.with(|e| *e.borrow_mut() = c);
}

/// The text of the last error reported on this thread, NUL-terminated,
/// valid until the next failing call on the thread.
#[unsafe(no_mangle)]
pub extern "C" fn vks_msg_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

unsafe fn bytes<'a>(p: *const u8, len: usize) -> &'a [u8] {
    if p.is_null() || len == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(p, len) }
    }
}

fn guard<T>(fail: T, f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => {
            set_error("internal failure in vkstorm-messages");
            fail
        }
    }
}

// ---------------------------------------------------------------------------
// tokenizer and parser

/// `LLTemplateTokenizer`.
pub struct VksMsgTokenizer {
    inner: Tokenizer,
    last: Vec<u8>,
}

/// # Safety
/// `text` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_tokenizer_new(text: *const u8, len: usize) -> *mut VksMsgTokenizer {
    let text = unsafe { bytes(text, len) };
    guard(std::ptr::null_mut(), || {
        let s = String::from_utf8_lossy(text);
        Box::into_raw(Box::new(VksMsgTokenizer { inner: Tokenizer::new(&s), last: Vec::new() }))
    })
}

/// # Safety
/// `t` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_tokenizer_clone(t: *const VksMsgTokenizer) -> *mut VksMsgTokenizer {
    if t.is_null() {
        return std::ptr::null_mut();
    }
    let t = unsafe { &*t };
    Box::into_raw(Box::new(VksMsgTokenizer { inner: t.inner.clone(), last: Vec::new() }))
}

/// # Safety
/// `t` must come from `vks_msg_tokenizer_new` or `_clone`; null is ignored.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_tokenizer_free(t: *mut VksMsgTokenizer) {
    if !t.is_null() {
        drop(unsafe { Box::from_raw(t) });
    }
}

/// # Safety
/// `t` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_tokenizer_at_eof(t: *const VksMsgTokenizer) -> i32 {
    unsafe { &*t }.inner.at_eof() as i32
}

/// # Safety
/// `t` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_tokenizer_line(t: *const VksMsgTokenizer) -> u32 {
    unsafe { &*t }.inner.line()
}

/// Advances and returns the token: 1 with `out`/`len` set (valid until the
/// next call on this tokenizer), 0 on error (`vks_msg_last_error`).
///
/// # Safety
/// `t` must be live; `out` and `len` valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_tokenizer_next(t: *mut VksMsgTokenizer, out: *mut *const u8, len: *mut usize) -> i32 {
    let t = unsafe { &mut *t };
    match t.inner.next() {
        Ok(s) => {
            t.last = s.into_bytes();
            unsafe {
                *out = t.last.as_ptr();
                *len = t.last.len();
            }
            1
        }
        Err(e) => {
            set_error(e.message);
            0
        }
    }
}

/// `want`: 1 consumed, 0 not the next token, -1 error.
///
/// # Safety
/// `t` must be live; `tok` must point to `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_tokenizer_want(t: *mut VksMsgTokenizer, tok: *const u8, len: usize) -> i32 {
    let t = unsafe { &mut *t };
    let tok = String::from_utf8_lossy(unsafe { bytes(tok, len) });
    match t.inner.want(&tok) {
        Ok(b) => b as i32,
        Err(e) => {
            set_error(e.message);
            -1
        }
    }
}

/// `wantEOF`: 1 at the end, 0 not, -1 error.
///
/// # Safety
/// `t` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_tokenizer_want_eof(t: *mut VksMsgTokenizer) -> i32 {
    let t = unsafe { &mut *t };
    match t.inner.want_eof() {
        Ok(b) => b as i32,
        Err(e) => {
            set_error(e.message);
            -1
        }
    }
}

/// A parsed message, block or variable description.
pub enum VksMsgDesc {
    Message(Template),
    Block(Block),
    Variable(Variable),
}

#[repr(C)]
pub struct VksMsgInfo {
    pub name: *const u8,
    pub name_len: usize,
    pub number: u32,
    pub frequency: i32,
    pub trust: i32,
    pub encoding: i32,
    pub deprecation: i32,
    pub total_size: i32,
    pub block_count: u32,
}

#[repr(C)]
pub struct VksMsgBlockInfo {
    pub name: *const u8,
    pub name_len: usize,
    pub ty: i32,
    pub number: i32,
    pub total_size: i32,
    pub var_count: u32,
}

#[repr(C)]
pub struct VksMsgVarInfo {
    pub name: *const u8,
    pub name_len: usize,
    pub ty: i32,
    pub size: i32,
}

/// The `version` line: 1 and `out` set, 0 on error.
///
/// # Safety
/// `t` must be live; `out` a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_parse_version(t: *mut VksMsgTokenizer, out: *mut f32) -> i32 {
    let t = unsafe { &mut *t };
    guard(0, || match parser::parse_version(&mut t.inner) {
        Ok(v) => {
            unsafe { *out = v };
            1
        }
        Err(e) => {
            set_error(e.message);
            0
        }
    })
}

fn parse_with<T>(t: *mut VksMsgTokenizer, status: *mut i32, f: impl FnOnce(&mut Tokenizer) -> Result<Option<T>, parser::ParseError>, wrap: impl FnOnce(T) -> VksMsgDesc) -> *mut VksMsgDesc {
    let t = unsafe { &mut *t };
    let (desc, st) = guard((std::ptr::null_mut(), -1), || match f(&mut t.inner) {
        Ok(Some(v)) => (Box::into_raw(Box::new(wrap(v))), 1),
        Ok(None) => (std::ptr::null_mut(), 0),
        Err(e) => {
            set_error(e.message);
            (std::ptr::null_mut(), -1)
        }
    });
    if !status.is_null() {
        unsafe { *status = st };
    }
    desc
}

/// `LLTemplateParser::parseMessage`: a description (status 1), null with
/// status 0 when the next token is not `{`, null with status -1 on error.
///
/// # Safety
/// `t` must be live; `status` may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_parse_message(t: *mut VksMsgTokenizer, status: *mut i32) -> *mut VksMsgDesc {
    parse_with(t, status, parser::parse_message, VksMsgDesc::Message)
}

/// `LLTemplateParser::parseBlock`.
///
/// # Safety
/// As `vks_msg_parse_message`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_parse_block(t: *mut VksMsgTokenizer, status: *mut i32) -> *mut VksMsgDesc {
    parse_with(t, status, parser::parse_block, VksMsgDesc::Block)
}

/// `LLTemplateParser::parseVariable`.
///
/// # Safety
/// As `vks_msg_parse_message`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_parse_variable(t: *mut VksMsgTokenizer, status: *mut i32) -> *mut VksMsgDesc {
    parse_with(t, status, |tok| parser::parse_variable(tok).map(|o| o.map(|(name, ty, size)| Variable { name, key: 0, ty, size })), VksMsgDesc::Variable)
}

/// # Safety
/// `d` must come from a parse call; null is ignored.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_desc_free(d: *mut VksMsgDesc) {
    if !d.is_null() {
        drop(unsafe { Box::from_raw(d) });
    }
}

/// The message of a message description.
///
/// # Safety
/// `d` must be live; `out` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_desc_message(d: *const VksMsgDesc, out: *mut VksMsgInfo) -> i32 {
    let VksMsgDesc::Message(t) = (unsafe { &*d }) else { return 0 };
    unsafe {
        *out = VksMsgInfo {
            name: t.name.as_ptr(),
            name_len: t.name.len(),
            number: t.number,
            frequency: t.frequency as i32,
            trust: t.trust as i32,
            encoding: t.encoding as i32,
            deprecation: t.deprecation as i32,
            total_size: t.total_size,
            block_count: t.blocks.len() as u32,
        };
    }
    1
}

fn desc_block(d: &VksMsgDesc, block: u32) -> Option<&Block> {
    match d {
        VksMsgDesc::Message(t) => t.blocks.get(block as usize),
        VksMsgDesc::Block(b) if block == 0 => Some(b),
        _ => None,
    }
}

/// Block `block` of a message description, or the block of a block description.
///
/// # Safety
/// `d` must be live; `out` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_desc_block(d: *const VksMsgDesc, block: u32, out: *mut VksMsgBlockInfo) -> i32 {
    let Some(b) = desc_block(unsafe { &*d }, block) else { return 0 };
    unsafe {
        *out = VksMsgBlockInfo {
            name: b.name.as_ptr(),
            name_len: b.name.len(),
            ty: b.ty as i32,
            number: b.number,
            total_size: b.total_size,
            var_count: b.variables.len() as u32,
        };
    }
    1
}

/// Variable `var` of block `block`, or the variable of a variable description.
///
/// # Safety
/// `d` must be live; `out` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_desc_variable(d: *const VksMsgDesc, block: u32, var: u32, out: *mut VksMsgVarInfo) -> i32 {
    let d = unsafe { &*d };
    let v = match d {
        VksMsgDesc::Variable(v) if block == 0 && var == 0 => Some(v),
        _ => desc_block(d, block).and_then(|b| b.variables.get(var as usize)),
    };
    let Some(v) = v else { return 0 };
    unsafe {
        *out = VksMsgVarInfo { name: v.name.as_ptr(), name_len: v.name.len(), ty: v.ty as i32, size: v.size };
    }
    1
}

// ---------------------------------------------------------------------------
// compiled templates

/// A template compiled from an `LLMessageTemplate`: built up with the
/// `add_*` calls, then shared by builders and readers.
pub struct VksMsgTemplate {
    inner: Template,
    shared: OnceLock<Arc<Template>>,
}

impl VksMsgTemplate {
    fn arc(&self) -> Arc<Template> {
        self.shared.get_or_init(|| Arc::new(self.inner.clone())).clone()
    }
}

/// # Safety
/// `name` must point to `name_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_template_new(name: *const u8, name_len: usize, key: u64, number: u32, frequency: i32, trust: i32, encoding: i32, deprecation: i32) -> *mut VksMsgTemplate {
    let name = String::from_utf8_lossy(unsafe { bytes(name, name_len) }).into_owned();
    let Some(frequency) = Frequency::from_i32(frequency) else { return std::ptr::null_mut() };
    let mut t = Template::new(name, key, number, frequency);
    t.trust = if trust == 0 { Trust::Trusted } else { Trust::NotTrusted };
    t.encoding = if encoding == 0 { Encoding::Unencoded } else { Encoding::ZeroCoded };
    t.deprecation = Deprecation::from_i32(deprecation).unwrap_or(Deprecation::NotDeprecated);
    Box::into_raw(Box::new(VksMsgTemplate { inner: t, shared: OnceLock::new() }))
}

/// Adds a block: its index, or -1 (duplicate name, bad type, or already in use).
///
/// # Safety
/// `t` must be live; `name` must point to `name_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_template_add_block(t: *mut VksMsgTemplate, name: *const u8, name_len: usize, key: u64, ty: i32, number: i32) -> i32 {
    let t = unsafe { &mut *t };
    if t.shared.get().is_some() {
        return -1;
    }
    let name = String::from_utf8_lossy(unsafe { bytes(name, name_len) }).into_owned();
    let Some(ty) = BlockType::from_i32(ty) else { return -1 };
    match t.inner.add_block(Block::new(name, key, ty, number)) {
        Ok(i) => i as i32,
        Err(e) => {
            set_error(e);
            -1
        }
    }
}

/// Adds a variable to block `block`: its index, or -1.
///
/// # Safety
/// `t` must be live; `name` must point to `name_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_template_add_variable(t: *mut VksMsgTemplate, block: u32, name: *const u8, name_len: usize, key: u64, ty: i32, size: i32) -> i32 {
    let t = unsafe { &mut *t };
    if t.shared.get().is_some() {
        return -1;
    }
    let name = String::from_utf8_lossy(unsafe { bytes(name, name_len) }).into_owned();
    let Some(ty) = VarType::from_i32(ty) else { return -1 };
    let Some(b) = t.inner.blocks.get_mut(block as usize) else { return -1 };
    match b.add_variable(name, key, ty, size) {
        Ok(i) => i as i32,
        Err(e) => {
            set_error(e);
            -1
        }
    }
}

/// The block's total size (-1 with a variable-length field) and the
/// message's, as `LLMessageBlock::mTotalSize` / `LLMessageTemplate::mTotalSize`.
///
/// # Safety
/// `t` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_template_total_size(t: *const VksMsgTemplate, block: i32) -> i32 {
    let t = unsafe { &*t };
    if block < 0 {
        t.inner.total_size
    } else {
        t.inner.blocks.get(block as usize).map_or(0, |b| b.total_size)
    }
}

/// # Safety
/// `t` must come from `vks_msg_template_new`; null is ignored. Builders and
/// readers that hold the template keep it alive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_template_free(t: *mut VksMsgTemplate) {
    if !t.is_null() {
        drop(unsafe { Box::from_raw(t) });
    }
}

// ---------------------------------------------------------------------------
// the builder

pub struct VksMsgBuilder {
    inner: Builder,
}

#[unsafe(no_mangle)]
pub extern "C" fn vks_msg_builder_new() -> *mut VksMsgBuilder {
    Box::into_raw(Box::new(VksMsgBuilder { inner: Builder::new() }))
}

/// # Safety
/// `b` must come from `vks_msg_builder_new`; null is ignored.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_builder_free(b: *mut VksMsgBuilder) {
    if !b.is_null() {
        drop(unsafe { Box::from_raw(b) });
    }
}

/// `newMessage`: 1 when the message is deprecated (the caller warns), else 0.
///
/// # Safety
/// `b` and `t` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_builder_new_message(b: *mut VksMsgBuilder, t: *const VksMsgTemplate) -> i32 {
    let b = unsafe { &mut *b };
    let t = unsafe { &*t };
    guard(0, || b.inner.new_message(t.arc()) as i32)
}

/// # Safety
/// `b` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_builder_clear(b: *mut VksMsgBuilder) {
    unsafe { &mut *b }.inner.clear_message();
}

fn builder_error(e: BuilderError) -> i32 {
    set_error(e.to_string());
    match e {
        BuilderError::NoMessage => VKS_MSG_ERR_NO_MESSAGE,
        BuilderError::NotABlock => VKS_MSG_ERR_NOT_A_BLOCK,
        BuilderError::SingleRepeated => VKS_MSG_ERR_SINGLE_REPEATED,
        BuilderError::MultipleExceeded { .. } => VKS_MSG_ERR_MULTIPLE_EXCEEDED,
        BuilderError::TooManyBlocks => VKS_MSG_ERR_TOO_MANY_BLOCKS,
        BuilderError::NoBlock => VKS_MSG_ERR_NO_BLOCK,
        BuilderError::NotAVariable => VKS_MSG_ERR_NOT_A_VARIABLE,
        BuilderError::FixedSizeMismatch { .. } => VKS_MSG_ERR_SIZE_MISMATCH,
    }
}

/// `nextBlock`: `VKS_MSG_OK` or an error code.
///
/// # Safety
/// `b` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_builder_next_block(b: *mut VksMsgBuilder, key: u64) -> i32 {
    let b = unsafe { &mut *b };
    guard(VKS_MSG_ERR_INTERNAL, || match b.inner.next_block(key) {
        Ok(()) => VKS_MSG_OK,
        Err(e) => builder_error(e),
    })
}

/// `removeLastBlock`: 1 when a repeat was dropped.
///
/// # Safety
/// `b` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_builder_remove_last_block(b: *mut VksMsgBuilder) -> i32 {
    let b = unsafe { &mut *b };
    guard(0, || b.inner.remove_last_block() as i32)
}

/// `addData`: on success a non-negative flag set (1: a Variable 1 field
/// was clamped to 255 bytes, 2: the data type does not match the
/// template's), else an error code.
///
/// # Safety
/// `b` must be live; `data` must point to `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_builder_add(b: *mut VksMsgBuilder, key: u64, data: *const u8, len: usize, ty: i32) -> i32 {
    let b = unsafe { &mut *b };
    let data = unsafe { bytes(data, len) };
    let ty = VarType::from_i32(ty).unwrap_or(VarType::Fixed);
    guard(VKS_MSG_ERR_INTERNAL, || match b.inner.add_data(key, data, ty) {
        Ok(o) => (o.clamped as i32) | ((o.type_mismatch as i32) << 1),
        Err(e) => builder_error(e),
    })
}

/// `isMessageFull`: 1/0, or an error code (`has_key` 0 asks without a block).
///
/// # Safety
/// `b` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_builder_is_full(b: *const VksMsgBuilder, key: u64, has_key: i32) -> i32 {
    let b = unsafe { &*b };
    guard(VKS_MSG_ERR_INTERNAL, || match b.inner.is_message_full(if has_key != 0 { Some(key) } else { None }) {
        Ok(f) => f as i32,
        Err(e) => builder_error(e),
    })
}

/// # Safety
/// `b` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_builder_message_size(b: *const VksMsgBuilder) -> i32 {
    unsafe { &*b }.inner.message_size()
}

/// # Safety
/// `b` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_builder_is_built(b: *const VksMsgBuilder) -> i32 {
    unsafe { &*b }.inner.is_built() as i32
}

/// # Safety
/// `b` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_builder_set_built(b: *mut VksMsgBuilder, built: i32) {
    unsafe { &mut *b }.inner.set_built(built != 0);
}

/// # Safety
/// `b` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_builder_is_clear(b: *const VksMsgBuilder) -> i32 {
    unsafe { &*b }.inner.is_clear() as i32
}

/// `buildMessage`: the packed size, or an error code (text in
/// `vks_msg_last_error`).
///
/// # Safety
/// `b` must be live; `buffer` must point to `size` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_builder_build(b: *mut VksMsgBuilder, buffer: *mut u8, size: usize, offset: u8) -> i64 {
    let b = unsafe { &mut *b };
    if buffer.is_null() {
        return VKS_MSG_ERR_BUILD as i64;
    }
    let buffer = unsafe { slice::from_raw_parts_mut(buffer, size) };
    guard(VKS_MSG_ERR_INTERNAL as i64, || match b.inner.build_message(buffer, offset) {
        Ok(n) => n as i64,
        Err(e) => {
            let code = match e {
                BuildError::NoMessage => VKS_MSG_ERR_NO_MESSAGE,
                _ => VKS_MSG_ERR_BUILD,
            };
            set_error(e.to_string());
            code as i64
        }
    })
}

/// The repeats added for template block `block`.
///
/// # Safety
/// `b` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_builder_block_repeats(b: *const VksMsgBuilder, block: u32) -> i32 {
    unsafe { &*b }.inner.block_count(block as usize)
}

/// The data of variable `var` in repeat `repeat` of template block `block`,
/// valid until the builder changes; null when unset. `data_size` receives
/// the size-field width (-1 for a fixed field).
///
/// # Safety
/// `b` must be live; `len` and `data_size` valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_builder_slot(b: *const VksMsgBuilder, block: u32, repeat: u32, var: u32, len: *mut usize, data_size: *mut i32) -> *const u8 {
    let b = unsafe { &*b };
    match b.inner.slot_data(block as usize, repeat as usize, var as usize) {
        Some((d, ds)) => {
            unsafe {
                *len = d.len();
                *data_size = ds;
            }
            d.as_ptr()
        }
        None => std::ptr::null(),
    }
}

// ---------------------------------------------------------------------------
// the reader

pub struct VksMsgReader {
    inner: Reader,
    events: Vec<vkstorm_messages::RanOffEnd>,
}

#[unsafe(no_mangle)]
pub extern "C" fn vks_msg_reader_new() -> *mut VksMsgReader {
    Box::into_raw(Box::new(VksMsgReader { inner: Reader::new(), events: Vec::new() }))
}

/// # Safety
/// `r` must come from `vks_msg_reader_new`; null is ignored.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_reader_free(r: *mut VksMsgReader) {
    if !r.is_null() {
        drop(unsafe { Box::from_raw(r) });
    }
}

/// The message number of a packet: the number (>= 0), -1 for an empty
/// buffer, -2 for one too short to carry its number.
///
/// # Safety
/// `buffer` must point to `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_decode_number(buffer: *const u8, size: usize) -> i64 {
    let buffer = unsafe { bytes(buffer, size) };
    match decode_number(buffer) {
        Ok(n) => n as i64,
        Err(NumberError::Empty) => -1,
        Err(NumberError::TooShort) => -2,
    }
}

/// `validateMessage`'s bookkeeping: the template found for the number and
/// the packet size.
///
/// # Safety
/// `r` and `t` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_reader_set_message(r: *mut VksMsgReader, t: *const VksMsgTemplate, receive_size: i32) {
    let r = unsafe { &mut *r };
    let t = unsafe { &*t };
    r.events.clear();
    r.inner.set_message(t.arc(), receive_size);
}

/// `decodeData`: 1 when the message had blocks (dispatch it), 0 when it
/// had none ("empty message"). `events` receives the number of reads past
/// the end, each available through `vks_msg_reader_event`.
///
/// # Safety
/// `r` must be live; `buffer` must point to `size` bytes; `events` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_reader_decode(r: *mut VksMsgReader, buffer: *const u8, size: usize, events: *mut i32) -> i32 {
    let r = unsafe { &mut *r };
    let buffer = unsafe { bytes(buffer, size) };
    guard(0, || {
        let out = r.inner.decode_data(buffer);
        r.events = out.ran_off_end;
        if !events.is_null() {
            unsafe { *events = r.events.len() as i32 };
        }
        out.has_blocks as i32
    })
}

/// Read-past-the-end event `i` of the last decode.
///
/// # Safety
/// `r` must be live; `position` and `wanted` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_reader_event(r: *const VksMsgReader, i: u32, position: *mut i32, wanted: *mut i32) -> i32 {
    let r = unsafe { &*r };
    match r.events.get(i as usize) {
        Some(e) => {
            unsafe {
                *position = e.position;
                *wanted = e.wanted;
            }
            1
        }
        None => 0,
    }
}

/// # Safety
/// `r` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_reader_clear(r: *mut VksMsgReader) {
    let r = unsafe { &mut *r };
    r.events.clear();
    r.inner.clear();
}

/// # Safety
/// `r` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_reader_message_size(r: *const VksMsgReader) -> i32 {
    unsafe { &*r }.inner.message_size()
}

fn get_error(e: GetError) -> i32 {
    match e {
        GetError::NoMessage => VKS_MSG_ERR_NO_MESSAGE,
        GetError::BlockNotFound => VKS_MSG_ERR_NOT_A_BLOCK,
        GetError::VariableNotFound => VKS_MSG_ERR_NOT_A_VARIABLE,
        GetError::SizeMismatch { .. } => VKS_MSG_ERR_SIZE_MISMATCH,
    }
}

/// `getData`: copies the variable's bytes to `dst` (at most `max_size`
/// of them). Returns 0, 1 when cut to `max_size` (the caller warns), or an
/// error code; `actual` receives the variable's size.
///
/// # Safety
/// `r` must be live; `dst` must have room for `min(size of the variable,
/// max_size)` bytes; `actual` may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_reader_get(r: *const VksMsgReader, block: u64, blocknum: i32, var: u64, dst: *mut u8, size: i32, max_size: i32, actual: *mut i32) -> i32 {
    let r = unsafe { &*r };
    match r.inner.get(block, blocknum, var, size) {
        Ok(d) => {
            if !actual.is_null() {
                unsafe { *actual = d.len() as i32 };
            }
            let n = if max_size >= 0 { d.len().min(max_size as usize) } else { d.len() };
            if n > 0 && !dst.is_null() {
                unsafe { std::ptr::copy_nonoverlapping(d.as_ptr(), dst, n) };
            }
            (n < d.len()) as i32
        }
        Err(GetError::SizeMismatch { actual: a }) => {
            if !actual.is_null() {
                unsafe { *actual = a };
            }
            VKS_MSG_ERR_SIZE_MISMATCH
        }
        Err(e) => get_error(e),
    }
}

/// `getNumberOfBlocks`: the repeats (0 when absent) or an error code.
///
/// # Safety
/// `r` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_reader_number_of_blocks(r: *const VksMsgReader, block: u64) -> i32 {
    match unsafe { &*r }.inner.number_of_blocks(block) {
        Ok(n) => n,
        Err(e) => get_error(e),
    }
}

/// `getSize(block, blocknum, var)`: the size or an error code.
///
/// # Safety
/// `r` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_reader_size(r: *const VksMsgReader, block: u64, blocknum: i32, var: u64) -> i32 {
    match unsafe { &*r }.inner.size(block, blocknum, var) {
        Ok(n) => n,
        Err(e) => get_error(e),
    }
}

/// The repeats of template block `block` in the decoded message.
///
/// # Safety
/// `r` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_reader_block_repeats(r: *const VksMsgReader, block: u32) -> i32 {
    unsafe { &*r }.inner.block_count(block as usize)
}

/// The bytes of variable `var` in repeat `repeat` of template block
/// `block`, valid until the next decode or clear; null when absent.
///
/// # Safety
/// `r` must be live; `len` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_reader_slot(r: *const VksMsgReader, block: u32, repeat: u32, var: u32, len: *mut usize) -> *const u8 {
    match unsafe { &*r }.inner.slot_data(block as usize, repeat as usize, var as usize) {
        Some(d) => {
            unsafe { *len = d.len() };
            d.as_ptr()
        }
        None => std::ptr::null(),
    }
}

// ---------------------------------------------------------------------------
// zero-coding

/// `zero_code`: the coded length written to `out` (flag set) when coding
/// made the packet smaller, 0 when it would not, -1 on bad arguments.
///
/// # Safety
/// `packet` must point to `len` bytes; `out` to `out_len` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_zero_code(packet: *const u8, len: usize, out: *mut u8, out_len: usize) -> i32 {
    if packet.is_null() || out.is_null() {
        return -1;
    }
    let packet = unsafe { bytes(packet, len) };
    let out = unsafe { slice::from_raw_parts_mut(out, out_len) };
    guard(-1, || zerocode::zero_code(packet, out).map_or(0, |n| n as i32))
}

/// `zeroCodeAdjustCurrentSendTotal`'s count: the (non-positive) size change.
///
/// # Safety
/// `packet` must point to `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_zero_code_gain(packet: *const u8, len: usize) -> i32 {
    let packet = unsafe { bytes(packet, len) };
    guard(0, || zerocode::zero_code_gain(packet))
}

/// `zeroCodeExpand` for a packet with the flag set: the expanded length in
/// `out` (flag cleared), with `overruns` the number of runs that passed
/// the end of the buffer and were written from its start as the C++ did;
/// -1 or -2 when a byte or a wrap would pass the end (`out` then holds
/// nothing useful and the caller treats the packet as empty).
///
/// # Safety
/// `packet` must point to `len` bytes; `out` to `out_len` writable bytes;
/// `overruns` may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_msg_zero_code_expand(packet: *const u8, len: usize, out: *mut u8, out_len: usize, overruns: *mut i32) -> i32 {
    if packet.is_null() || out.is_null() {
        return -1;
    }
    let packet = unsafe { bytes(packet, len) };
    let out = unsafe { slice::from_raw_parts_mut(out, out_len) };
    guard(-1, || {
        let e = zerocode::zero_code_expand(packet, out);
        if !overruns.is_null() {
            unsafe { *overruns = e.overruns as i32 };
        }
        match e.error {
            None => e.len as i32,
            Some(zerocode::ExpandError::PastEnd1) => -1,
            Some(zerocode::ExpandError::PastEnd2) => -2,
        }
    })
}

// ---------------------------------------------------------------------------
// data packers

/// A data packer's buffer state for one call: the C++ keeps `data`,
/// `size`, `cur` (`mCurBufferp - mBufferp`) and the flags in its object.
#[repr(C)]
pub struct VksDpBuffer {
    pub data: *mut u8,
    pub size: i32,
    pub cur: i32,
    pub write_enabled: i32,
    pub include_names: i32,
}

pub const VKS_DP_STRING: i32 = 0;
pub const VKS_DP_BINARY: i32 = 1;
pub const VKS_DP_BINARY_FIXED: i32 = 2;
pub const VKS_DP_INT: i32 = 3;
pub const VKS_DP_UINT: i32 = 4;
pub const VKS_DP_FLOATS: i32 = 5;
pub const VKS_DP_BYTES: i32 = 6;
pub const VKS_DP_UUID: i32 = 7;
pub const VKS_DP_FLOATS_SHORT: i32 = 8;

fn kind_from(k: i32) -> Option<TextKind> {
    Some(match k {
        VKS_DP_STRING => TextKind::String,
        VKS_DP_BINARY => TextKind::Binary,
        VKS_DP_BINARY_FIXED => TextKind::BinaryFixed,
        VKS_DP_INT => TextKind::Int,
        VKS_DP_UINT => TextKind::Uint,
        VKS_DP_FLOATS => TextKind::Floats,
        VKS_DP_BYTES => TextKind::Bytes,
        VKS_DP_UUID => TextKind::Uuid,
        VKS_DP_FLOATS_SHORT => TextKind::FloatsShort,
        _ => return None,
    })
}

unsafe fn with_binary<T>(s: *mut VksDpBuffer, f: impl FnOnce(&mut BinaryBuffer<'_>) -> T) -> T {
    let s = unsafe { &mut *s };
    let data: &mut [u8] = if s.data.is_null() || s.size <= 0 { &mut [] } else { unsafe { slice::from_raw_parts_mut(s.data, s.size as usize) } };
    let mut b = BinaryBuffer { data, cur: s.cur, size: s.size, write_enabled: s.write_enabled != 0 };
    let r = f(&mut b);
    s.cur = b.cur;
    r
}

/// Packs a fixed-size value's bytes.
///
/// # Safety
/// `s` must be valid with `data` pointing to `size` bytes (or null when
/// writes are disabled); `v` must point to `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dp_bin_pack(s: *mut VksDpBuffer, v: *const u8, len: usize) -> i32 {
    let v = unsafe { bytes(v, len) };
    unsafe { with_binary(s, |b| b.pack_bytes(v) as i32) }
}

/// Unpacks a fixed-size value's bytes into `dst`.
///
/// # Safety
/// As `vks_dp_bin_pack`; `dst` must have room for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dp_bin_unpack(s: *mut VksDpBuffer, dst: *mut u8, len: usize) -> i32 {
    unsafe {
        with_binary(s, |b| match b.unpack_bytes(len) {
            Some(d) => {
                std::ptr::copy_nonoverlapping(d.as_ptr(), dst, d.len());
                1
            }
            None => 0,
        })
    }
}

/// `packS16` (advances even when refused).
///
/// # Safety
/// As `vks_dp_bin_pack`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dp_bin_pack_s16(s: *mut VksDpBuffer, v: i16) -> i32 {
    unsafe { with_binary(s, |b| b.pack_s16(v) as i32) }
}

/// `unpackS16` (advances even when refused).
///
/// # Safety
/// As `vks_dp_bin_pack`; `out` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dp_bin_unpack_s16(s: *mut VksDpBuffer, out: *mut i16) -> i32 {
    unsafe {
        with_binary(s, |b| match b.unpack_s16() {
            Some(v) => {
                *out = v;
                1
            }
            None => 0,
        })
    }
}

/// `packString`: the bytes (up to a NUL) and a NUL.
///
/// # Safety
/// As `vks_dp_bin_pack`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dp_bin_pack_string(s: *mut VksDpBuffer, v: *const u8, len: usize) -> i32 {
    let v = unsafe { bytes(v, len) };
    unsafe { with_binary(s, |b| b.pack_string(v) as i32) }
}

/// `unpackString`: `out`/`len` point into the buffer (without the NUL).
///
/// # Safety
/// As `vks_dp_bin_pack`; `out` and `len` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dp_bin_unpack_string(s: *mut VksDpBuffer, out: *mut *const u8, len: *mut usize) -> i32 {
    unsafe {
        with_binary(s, |b| match b.unpack_string() {
            Some(d) => {
                *out = d.as_ptr();
                *len = d.len();
                1
            }
            None => 0,
        })
    }
}

/// `packBinaryData`: a 4-byte size and the bytes.
///
/// # Safety
/// As `vks_dp_bin_pack`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dp_bin_pack_binary(s: *mut VksDpBuffer, v: *const u8, len: usize) -> i32 {
    let v = unsafe { bytes(v, len) };
    unsafe { with_binary(s, |b| b.pack_binary(v) as i32) }
}

/// `unpackBinaryData`: `out`/`len` point into the buffer.
///
/// # Safety
/// As `vks_dp_bin_pack`; `out` and `len` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dp_bin_unpack_binary(s: *mut VksDpBuffer, out: *mut *const u8, len: *mut usize) -> i32 {
    unsafe {
        with_binary(s, |b| match b.unpack_binary() {
            Some(d) => {
                *out = d.as_ptr();
                *len = d.len();
                1
            }
            None => 0,
        })
    }
}

unsafe fn with_ascii<T>(s: *mut VksDpBuffer, f: impl FnOnce(&mut AsciiBuffer<'_>) -> T) -> T {
    let s = unsafe { &mut *s };
    let data: &mut [u8] = if s.data.is_null() || s.size <= 0 { &mut [] } else { unsafe { slice::from_raw_parts_mut(s.data, s.size as usize) } };
    let mut b = AsciiBuffer { data, cur: s.cur, size: s.size, write_enabled: s.write_enabled != 0, include_names: s.include_names != 0 };
    let r = f(&mut b);
    s.cur = b.cur;
    r
}

/// A text `pack*`: writes the name (when included) and the value line.
/// Returns the C++ result (0 only for a UUID that did not fit); `truncated`
/// says whether anything was cut (the C++ warned).
///
/// # Safety
/// As `vks_dp_bin_pack`; the arrays must have the given lengths;
/// `truncated` may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dp_ascii_pack(
    s: *mut VksDpBuffer,
    kind: i32,
    name: *const u8,
    name_len: usize,
    ints: *const i64,
    nints: usize,
    floats: *const f32,
    nfloats: usize,
    data: *const u8,
    ndata: usize,
    truncated: *mut i32,
) -> i32 {
    let Some(kind) = kind_from(kind) else { return 0 };
    let name = unsafe { bytes(name, name_len) };
    let ints: &[i64] = if ints.is_null() { &[] } else { unsafe { slice::from_raw_parts(ints, nints) } };
    let floats: &[f32] = if floats.is_null() { &[] } else { unsafe { slice::from_raw_parts(floats, nfloats) } };
    let data = unsafe { bytes(data, ndata) };
    let (ok, cut) = unsafe { with_ascii(s, |b| b.pack(kind, name, ints, floats, data)) };
    if !truncated.is_null() {
        unsafe { *truncated = cut as i32 };
    }
    ok as i32
}

/// `getValueStr`: the next value line (at most 511 bytes) into `out`,
/// after the name when names are included. 1 and `len` set, 0 on a name
/// mismatch.
///
/// # Safety
/// As `vks_dp_bin_pack`; `out` must have room for `cap` bytes; `len` valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dp_ascii_next_value(s: *mut VksDpBuffer, name: *const u8, name_len: usize, out: *mut u8, cap: usize, len: *mut usize) -> i32 {
    let name = unsafe { bytes(name, name_len) };
    match unsafe { with_ascii(s, |b| b.next_value(name)) } {
        Some(v) => {
            let n = v.len().min(cap);
            unsafe {
                std::ptr::copy_nonoverlapping(v.as_ptr(), out, n);
                *len = n;
            }
            1
        }
        None => 0,
    }
}

/// Reads a value line of `kind` (`count` scalars for floats and bytes, or
/// the fixed byte count) into the slots: `ints`, `floats` and `data` get
/// what was assigned, `size` the declared size of a `Binary`, `ndata` the
/// bytes stored. Returns the number of scalars assigned.
///
/// # Safety
/// `text` must point to `len` bytes; the output arrays must have room for
/// `count` entries (`data` for `data_cap` bytes); pointers may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dp_ascii_parse(
    kind: i32,
    text: *const u8,
    len: usize,
    count: usize,
    ints: *mut i64,
    floats: *mut f32,
    data: *mut u8,
    data_cap: usize,
    size: *mut i32,
    ndata: *mut usize,
) -> i32 {
    let Some(kind) = kind_from(kind) else { return 0 };
    let text = unsafe { bytes(text, len) };
    let p = datapacker::parse_value(kind, text, count);
    unsafe {
        if !ints.is_null() {
            for (i, v) in p.ints.iter().enumerate().take(count.max(1)) {
                *ints.add(i) = *v;
            }
        }
        if !floats.is_null() {
            for (i, v) in p.floats.iter().enumerate().take(count) {
                *floats.add(i) = *v;
            }
        }
        let n = p.bytes.len().min(data_cap);
        if !data.is_null() && n > 0 {
            std::ptr::copy_nonoverlapping(p.bytes.as_ptr(), data, n);
        }
        if !ndata.is_null() {
            *ndata = n;
        }
        if !size.is_null() {
            *size = p.size;
        }
    }
    p.assigned as i32
}

/// The value line (with its newline) a text packer writes for a value,
/// for the file packer. Returns the full length; writes `min(len, cap)`.
///
/// # Safety
/// The arrays must have the given lengths; `out` must have room for `cap`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dp_ascii_format(kind: i32, ints: *const i64, nints: usize, floats: *const f32, nfloats: usize, data: *const u8, ndata: usize, out: *mut u8, cap: usize) -> usize {
    let Some(kind) = kind_from(kind) else { return 0 };
    let ints: &[i64] = if ints.is_null() { &[] } else { unsafe { slice::from_raw_parts(ints, nints) } };
    let floats: &[f32] = if floats.is_null() { &[] } else { unsafe { slice::from_raw_parts(floats, nfloats) } };
    let data = unsafe { bytes(data, ndata) };
    let mut v = Vec::new();
    datapacker::format_value(kind, ints, floats, data, &mut v);
    let n = v.len().min(cap);
    if n > 0 && !out.is_null() {
        unsafe { std::ptr::copy_nonoverlapping(v.as_ptr(), out, n) };
    }
    v.len()
}

/// `sscanf(line, "%511s %511[^\n]")`: the keyword and the value of a line
/// (pointers into `line`; empty when absent).
///
/// # Safety
/// `line` must point to `len` bytes; the outputs valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vks_dp_ascii_split(line: *const u8, len: usize, key: *mut *const u8, key_len: *mut usize, value: *mut *const u8, value_len: *mut usize) {
    let line = unsafe { bytes(line, len) };
    let (k, v) = vkstorm_messages::ascii::split_keyword_value(line);
    unsafe {
        *key = k.as_ptr();
        *key_len = k.len();
        *value = v.as_ptr();
        *value_len = v.len();
    }
}

/// `packFixed`'s conversion: the integer to pack and, in `total_bits`, the
/// width that chooses U8, U16 or U32 (over 31 is an error the caller aborts on).
#[unsafe(no_mangle)]
pub extern "C" fn vks_dp_fixed_pack(value: f32, is_signed: i32, int_bits: u32, frac_bits: u32, total_bits: *mut i32) -> u32 {
    let (bits, fixed) = datapacker::pack_fixed(value, is_signed != 0, int_bits, frac_bits);
    if !total_bits.is_null() {
        unsafe { *total_bits = bits };
    }
    fixed
}

/// `unpackFixed`'s conversion.
#[unsafe(no_mangle)]
pub extern "C" fn vks_dp_fixed_unpack(fixed: u32, is_signed: i32, int_bits: u32, frac_bits: u32) -> f32 {
    datapacker::unpack_fixed(fixed, is_signed != 0, int_bits, frac_bits)
}

/// `packFixed`'s width: `int_bits + frac_bits (+ 1 when signed)`.
#[unsafe(no_mangle)]
pub extern "C" fn vks_dp_fixed_total_bits(is_signed: i32, int_bits: u32, frac_bits: u32) -> i32 {
    datapacker::fixed_total_bits(is_signed != 0, int_bits, frac_bits)
}
