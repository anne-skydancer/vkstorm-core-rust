//! The task inventory file (`LLViewerObject::loadTaskInvFile`'s read loop).
//!
//! The simulator sends the contents of an object as a legacy text file
//! (through an xfer into the cache): `inv_object` and `inv_item` blocks in
//! the forms of [`crate::legacy::object`] and [`crate::legacy::item`].
//!
//! The loop, as the C++:
//! - reads a line with `getline(buffer, MAX_STRING)` while the stream is
//!   good, and scans its first token as `" %254s"`;
//! - a line with no token (blank, white space only, or nothing left) ends
//!   the file (`sscanf` returned `EOF`): so does any blank line between
//!   blocks;
//! - `inv_item` reads an `LLViewerInventoryItem` from the same stream (the
//!   item reader, then `mIsComplete`), `inv_object` an `LLInventoryObject`
//!   renamed `Contents`;
//! - any other token counts as unknown; the 26th unknown token ends the
//!   file (`MAX_INV_FILE_READ_FAILS` = 25 are tolerated);
//! - a line of 255 bytes or more sets `failbit` (its first 255 bytes are
//!   still scanned) and so ends the loop.
//!
//! The entries are returned in file order; the loader pushes each to the
//! front of the list, so the list ends up in reverse file order.

use super::{Entry, MAX_INV_FILE_READ_FAILS, MAX_STRING, ViewerItem};
use crate::legacy::reader::Stream;
use crate::legacy::scanf::{EOF, sscanf};
use crate::object::Object;

/// The entries of a task inventory file, in file order.
pub fn read_task_inventory(bytes: &[u8]) -> Vec<Entry> {
    let mut input = Stream::new(bytes);
    let mut fail_count = 0u32;
    let mut entries = Vec::new();
    while input.good() {
        let line = input.getline(MAX_STRING);
        let scan = sscanf(line, " %254s");
        if scan.ret == EOF {
            // LL_WARNS "Issue reading from file" (a blank file or line).
            break;
        }
        match scan.str(0).unwrap_or_default() {
            b"inv_item" => {
                let mut item = ViewerItem::default();
                item.item.import_legacy_stream(&mut input);
                item.complete = true;
                entries.push(Entry::Item(item));
            }
            b"inv_object" => {
                let mut object = Object::default();
                object.import_legacy_stream(&mut input);
                object.rename(b"Contents");
                entries.push(Entry::Object(object));
            }
            _ if fail_count >= MAX_INV_FILE_READ_FAILS => {
                // LL_WARNS "Encountered too many unknowns".
                break;
            }
            _ => fail_count += 1,
        }
    }
    entries
}
