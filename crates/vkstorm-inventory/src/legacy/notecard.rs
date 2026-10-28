//! `LLNotecard::importStream` and `exportStream` (`llnotecard.cpp`): the
//! notecard asset, a text with embedded inventory items.
//!
//! ```text
//! Linden text version 2
//! {
//! LLEmbeddedItems version 1
//! {
//! count <n>
//! {
//! ext char index <k>
//! \tinv_item\t0
//! \t{ ... \t}                       (the item's legacy form)
//! }
//! ...
//! }
//! Text length <bytes>
//! <text>}
//! ```
//!
//! The reader is the C++ stream code line for line: `>> std::ws`, the
//! `llstreamtools` literal match ([`Stream::expect`]), formatted integer
//! extraction, one `getline` of the `Text length` line and `fullread` of the
//! text. It stops at the first failure and leaves the stream there; the
//! items read before it stay in the notecard.

use std::io::Write as _;

use super::reader::{c_str, Stream};
use super::scanf::{sscanf, Arg};
use crate::item::Item;

/// `STD_STRING_BUF_SIZE` (`lldefs.h`): the buffer of the `Text length` line.
const STD_STRING_BUF_SIZE: usize = 255;

/// `LLNotecard`: its version, the embedded-items version, the items and the
/// text. The fields are public, as the viewer reaches them through getters
/// and setters that only store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notecard {
    /// `mMaxText`: the largest text length the reader accepts.
    pub max_text: i32,
    /// `mVersion` (`getVersion`).
    pub version: i32,
    /// `mEmbeddedVersion` (`getEmbeddedVersion`).
    pub embedded_version: i32,
    /// `mItems` (`getItems`, `setItems`).
    pub items: Vec<Item>,
    /// `mText` (`getText`, `setText`).
    pub text: Vec<u8>,
}

impl Default for Notecard {
    /// `LLNotecard()`: the limit [`Notecard::MAX_SIZE`].
    fn default() -> Self {
        Notecard::new(Notecard::MAX_SIZE)
    }
}

impl Notecard {
    /// `LLNotecard::MAX_SIZE`.
    pub const MAX_SIZE: i32 = 65536;

    /// `LLNotecard(max_text)`: versions 0, no items, no text.
    pub fn new(max_text: i32) -> Notecard {
        Notecard { max_text, version: 0, embedded_version: 0, items: Vec::new(), text: Vec::new() }
    }

    /// `LLNotecard::importEmbeddedItemsStream`: the `LLEmbeddedItems`
    /// block. Each item is a fresh `LLInventoryItem` read by
    /// [`Item::import_legacy_stream`] and kept even when a later step fails.
    fn import_embedded_items_stream(&mut self, str: &mut Stream<'_>) -> bool {
        str.skip_ws();
        str.expect(b"LLEmbeddedItems version");
        str.extract_i32(&mut self.embedded_version);
        str.expect(b"\n");
        if str.fail() {
            // LL_WARNS "Invalid Linden text file header".
            return false;
        }
        if self.embedded_version != 1 {
            return false;
        }
        str.skip_ws();
        str.expect(b"{\n");
        if str.fail() {
            return false;
        }
        let mut count = 0i32;
        str.skip_ws();
        str.expect(b"count ");
        str.extract_i32(&mut count);
        str.expect(b"\n");
        if str.fail() || count < 0 {
            return false;
        }
        for _ in 0..count {
            str.skip_ws();
            str.expect(b"{\n");
            if str.fail() {
                return false;
            }
            let mut index = 0u32;
            str.skip_ws();
            str.expect(b"ext char index ");
            str.extract_u32(&mut index);
            str.expect(b"\n");
            if str.fail() {
                return false;
            }
            str.skip_ws();
            str.expect(b"inv_item\t0\n");
            if str.fail() {
                return false;
            }
            let mut item = Item::default();
            if !item.import_legacy_stream(str) {
                return false;
            }
            self.items.push(item);
            str.skip_ws();
            str.expect(b"}\n");
            if str.fail() {
                return false;
            }
        }
        str.skip_ws();
        str.expect(b"}\n");
        !str.fail()
    }

    /// `LLNotecard::importStream`.
    ///
    /// Reads the `Linden text version` header (1 or 2), `{`, the embedded
    /// items, then the `Text length %d` line (one `getline` of up to 254
    /// bytes) and that many bytes of text by `fullread`. A length outside
    /// `0..=max_text` or a text shorter than its length fails. On success the
    /// text is the bytes read up to the first NUL (`std::string(char*)`);
    /// the closing `}` is not read. The version, embedded version and items
    /// read before a failure stay set; the text does not change on failure.
    pub fn import_stream(&mut self, str: &mut Stream<'_>) -> bool {
        str.skip_ws();
        str.expect(b"Linden text version ");
        str.extract_i32(&mut self.version);
        str.expect(b"\n");
        if str.fail() {
            return false;
        }
        if self.version != 1 && self.version != 2 {
            return false;
        }
        str.skip_ws();
        str.expect(b"{\n");
        if str.fail() {
            return false;
        }
        if !self.import_embedded_items_stream(str) {
            return false;
        }
        let line = str.getline(STD_STRING_BUF_SIZE);
        if str.fail() {
            return false;
        }
        let text_len = match sscanf(line, "Text length %d").args.first() {
            Some(Arg::Int(n)) => *n,
            _ => return false,
        };
        if text_len > self.max_text || text_len < 0 {
            return false;
        }
        let text = fullread(str, text_len as usize);
        if str.fail() {
            return false;
        }
        self.text = c_str(text).to_vec();
        true
    }

    /// `LLNotecard::exportStream`: version 2, the items as
    /// [`Item::export_legacy_stream`] with the asset key, then the text
    /// length and the text (every byte). Always returns true.
    pub fn export_stream(&self, out: &mut Vec<u8>) -> bool {
        out.extend_from_slice(b"Linden text version 2\n{\n");
        out.extend_from_slice(b"LLEmbeddedItems version 1\n{\n");
        let _ = writeln!(out, "count {}", self.items.len() as i32);
        for (idx, item) in self.items.iter().enumerate() {
            let _ = write!(out, "{{\next char index {idx}\n");
            item.export_legacy_stream(out, true);
            out.extend_from_slice(b"}\n");
        }
        out.extend_from_slice(b"}\n");
        let _ = writeln!(out, "Text length {}", self.text.len() as i32);
        out.extend_from_slice(&self.text);
        out.extend_from_slice(b"}\n");
        true
    }
}

/// `fullread` (`llstreamtools.cpp`): `read` until `requested` bytes are in or
/// a read gets nothing, clearing a failure between reads. Over an in-memory
/// stream the first short read already reached the end, so the retry gets
/// nothing and the stream keeps `eofbit` and `failbit`.
fn fullread<'a>(str: &mut Stream<'a>, requested: usize) -> &'a [u8] {
    let start = str.pos();
    let mut got = str.read(requested).len();
    let mut total = got;
    while got != 0 && total < requested {
        if str.fail() {
            if str.bad() {
                break;
            }
            str.clear();
        }
        got = str.read(requested - total).len();
        total += got;
    }
    &str.input()[start..start + total]
}
