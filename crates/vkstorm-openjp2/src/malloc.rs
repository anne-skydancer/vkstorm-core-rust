// Memory allocation for the port. By default this is the C runtime's malloc
// family, as OpenJPEG uses. With the `guard-alloc` feature every block gets a
// header and trailer canary that are verified on free, to localise heap
// corruption during development (reports the block size).
use core::ffi::c_void;

#[cfg(not(feature = "guard-alloc"))]
extern "C" {
  fn malloc(_: usize) -> *mut c_void;
  fn calloc(_: usize, _: usize) -> *mut c_void;
  fn realloc(_: *mut c_void, _: usize) -> *mut c_void;
  fn free(_: *mut c_void);
}

#[cfg(not(feature = "guard-alloc"))]
pub(crate) fn opj_malloc(size: usize) -> *mut c_void {
  if size == 0 { return core::ptr::null_mut(); }
  unsafe { malloc(size) }
}
#[cfg(not(feature = "guard-alloc"))]
pub(crate) fn opj_calloc(num: usize, size: usize) -> *mut c_void {
  if num == 0 || size == 0 { return core::ptr::null_mut(); }
  unsafe { calloc(num, size) }
}
#[cfg(not(feature = "guard-alloc"))]
pub(crate) fn opj_realloc(ptr: *mut c_void, new_size: usize) -> *mut c_void {
  if new_size == 0 { return core::ptr::null_mut(); }
  unsafe { realloc(ptr, new_size) }
}
#[cfg(not(feature = "guard-alloc"))]
pub(crate) fn opj_free(ptr: *mut c_void) {
  if !ptr.is_null() { unsafe { free(ptr) } }
}

#[cfg(feature = "guard-alloc")]
mod guard {
  use core::ffi::c_void;
  use std::alloc::{alloc_zeroed, dealloc, Layout};
  const HEAD: usize = 32; // size (8) + magic (8) + 16 bytes of canary
  const TAIL: usize = 16;
  const MAGIC: u64 = 0x5645_4b53_4f50_4a32; // "VEKSOPJ2"
  const CANARY: u8 = 0xC5;

  fn layout(size: usize) -> Layout {
    Layout::from_size_align(HEAD + size + TAIL, 16).unwrap()
  }

  pub unsafe fn malloc(size: usize) -> *mut c_void {
    let base = alloc_zeroed(layout(size));
    if base.is_null() { return core::ptr::null_mut(); }
    *(base as *mut u64) = size as u64;
    *(base.add(8) as *mut u64) = MAGIC;
    core::ptr::write_bytes(base.add(16), CANARY, 16);
    core::ptr::write_bytes(base.add(HEAD + size), CANARY, TAIL);
    base.add(HEAD) as *mut c_void
  }

  unsafe fn check(ptr: *mut c_void, what: &str) -> (*mut u8, usize) {
    let base = (ptr as *mut u8).sub(HEAD);
    let size = *(base as *const u64) as usize;
    let magic = *(base.add(8) as *const u64);
    if magic != MAGIC {
      panic!("guard-alloc: {what} of a pointer not from opj_malloc, or header smashed (size field {size})");
    }
    let head_ok = (0..16).all(|i| *base.add(16 + i) == CANARY);
    let tail_ok = (0..TAIL).all(|i| *base.add(HEAD + size + i) == CANARY);
    if !head_ok || !tail_ok {
      let tail: Vec<u8> = (0..TAIL).map(|i| *base.add(HEAD + size + i)).collect();
      panic!("guard-alloc: block of {size} bytes overrun at {what}: head_ok={head_ok} tail_ok={tail_ok} tail={tail:02x?}");
    }
    (base, size)
  }

  pub unsafe fn free(ptr: *mut c_void) {
    let (base, size) = check(ptr, "free");
    dealloc(base, layout(size));
  }

  pub unsafe fn realloc(ptr: *mut c_void, new_size: usize) -> *mut c_void {
    if ptr.is_null() { return malloc(new_size); }
    let (_, old) = check(ptr, "realloc");
    let n = malloc(new_size);
    if !n.is_null() {
      core::ptr::copy_nonoverlapping(ptr as *const u8, n as *mut u8, old.min(new_size));
    }
    free(ptr);
    n
  }
}

#[cfg(feature = "guard-alloc")]
pub(crate) fn opj_malloc(size: usize) -> *mut c_void {
  if size == 0 { return core::ptr::null_mut(); }
  unsafe { guard::malloc(size) }
}
#[cfg(feature = "guard-alloc")]
pub(crate) fn opj_calloc(num: usize, size: usize) -> *mut c_void {
  if num == 0 || size == 0 { return core::ptr::null_mut(); }
  unsafe { guard::malloc(num * size) } // alloc_zeroed
}
#[cfg(feature = "guard-alloc")]
pub(crate) fn opj_realloc(ptr: *mut c_void, new_size: usize) -> *mut c_void {
  if new_size == 0 { return core::ptr::null_mut(); }
  unsafe { guard::realloc(ptr, new_size) }
}
#[cfg(feature = "guard-alloc")]
pub(crate) fn opj_free(ptr: *mut c_void) {
  if !ptr.is_null() { unsafe { guard::free(ptr) } }
}
