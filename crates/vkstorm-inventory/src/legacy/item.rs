//! `LLInventoryItem::importLegacyStream` and `exportLegacyStream`
//! (`llinventory.cpp`).
//!
//! ```text
//! \tinv_item\t0
//! \t{
//! \t\titem_id\t<uuid>
//! \t\tparent_id\t<uuid>
//! \tpermissions 0
//! \t{ ... \t}
//! \t\tmetadata\t<llsd>...</llsd>\n|        (thumbnail or runtime only)
//! \t\tasset_id\t<uuid>                     (or shadow_id)
//! \t\ttype\t<asset type name>
//! \t\tinv_type\t<inventory type name>       (a named type only)
//! \t\tflags\t%08x
//! \tsale_info\t0
//! \t{ ... \t}
//! \t\tname\t<name>|
//! \t\tdesc\t<description>|
//! \t\tcreation_date\t<time_t>
//! \t}
//! ```
//!
//! The reader starts after the `inv_item\t0` line, which its caller has
//! consumed.

use std::io::Write as _;

use vkstorm_llsd::text::uuid_set;

use super::object::{
    KEYWORD_VALUE, apply_metadata, push_id_line, push_metadata, push_text_line, push_type_line,
    rescanned_value,
};
use super::reader::Stream;
use super::sale::MAX_STRING;
use super::scanf::{scan_hex, scan_int, sscanf};
use crate::item::Item;
use crate::perms::{NULL_ID, PERM_COPY, PERM_NONE, PERM_TRANSFER};
use crate::shadow::{exported_asset_id, unshadow_id};
use crate::text::{replace_char, replace_nonstandard_ascii};
use crate::types::{AssetType, InventoryType};

/// The `name` and `desc` lines' second scan: the keyword, the tabs after
/// it, then the text up to the first `|`.
const NAME_FORMAT: &str = " %254s%254[\t]%254[^|]";

/// The text of a `name` or `desc` line: the third conversion of
/// [`NAME_FORMAT`] when stored, else the line's first value token; a value
/// starting with `|` is empty (the C++'s "sscanf chokes and puts | in
/// valuestr if there's no name").
fn text_value(line: &[u8], first: &[u8]) -> Vec<u8> {
    let mut v = rescanned_value(first, &sscanf(line, NAME_FORMAT), 2);
    if v.first() == Some(&b'|') {
        v.clear();
    }
    v
}

impl Item {
    /// `LLInventoryItem::importLegacyStream`.
    ///
    /// Sets the inventory type to `IT_NONE` and the asset id to null, then
    /// reads lines of up to 254 bytes while the last block read succeeded
    /// and the stream is good, each parsed on its own as `" %254s %254s"`:
    /// - a blank line and `{` are skipped, `}` ends the block;
    /// - a keyword without a value is skipped, except `permissions`,
    ///   `sale_info` and `metadata`;
    /// - `item_id`, `parent_id` and `asset_id` set the ids by `LLUUID::set`;
    ///   `shadow_id` sets the asset id to its [`unshadow_id`];
    /// - `permissions` reads the permissions block from the same stream;
    /// - `sale_info` reads the sale block; a deprecated `perm_mask` in it
    ///   (`PERM_NONE` meaning the owner mask, and transfer added when copy
    ///   is missing) becomes the next-owner mask;
    /// - `type` and `inv_type` set the types by name (`AT_UNKNOWN`,
    ///   `IT_NONE` when unknown);
    /// - `metadata` sets the runtime, thumbnail and favorite;
    /// - `flags` a `%x` value and `creation_date` a `%d` value that parse;
    /// - `name` and `desc` re-read the line as `" %254s%254[\t]%254[^|]"`;
    ///   the name takes `replaceNonstandardASCII` and `replaceChar('|')`,
    ///   the description `replaceNonstandardASCII` only;
    /// - an unknown keyword is skipped (the C++ logs a warning).
    ///
    /// Then an `IT_NONE` inventory type, or one that does not match the
    /// asset type, becomes the asset type's default, and
    /// `initMasks(inv_type)`. Returns the last block's result (always true).
    pub fn import_legacy_stream(&mut self, input: &mut Stream<'_>) -> bool {
        let mut success = true;
        self.inv_type = InventoryType::NONE;
        self.asset = NULL_ID;
        while success && input.good() {
            let line = input.getline(MAX_STRING);
            let scan = sscanf(line, KEYWORD_VALUE);
            if scan.ret < 1 {
                continue;
            }
            let keyword = scan.str(0).unwrap_or_default();
            let value = scan.str(1).unwrap_or_default();
            if keyword == b"{" {
                continue;
            }
            if keyword == b"}" {
                break;
            }
            if scan.ret < 2 && !matches!(keyword, b"permissions" | b"sale_info" | b"metadata") {
                continue;
            }
            match keyword {
                b"item_id" => self.object.id = uuid_set(value),
                b"parent_id" => self.object.parent = uuid_set(value),
                b"permissions" => success = self.permissions.import_legacy_stream(input),
                b"sale_info" => {
                    let (mut has_perm_mask, mut perm_mask) = (false, 0u32);
                    success = self.sale_info.import_legacy_stream(
                        input,
                        &mut has_perm_mask,
                        &mut perm_mask,
                    );
                    if has_perm_mask {
                        self.apply_legacy_perm_mask(perm_mask);
                    }
                }
                b"shadow_id" => self.asset = unshadow_id(&uuid_set(value)),
                b"asset_id" => self.asset = uuid_set(value),
                b"type" => self.object.asset_type = AssetType::from_name_cstr(Some(value)),
                b"metadata" => apply_metadata(&mut self.object, value),
                b"inv_type" => self.inv_type = InventoryType::from_name(value),
                b"flags" => {
                    if let Some(f) = scan_hex(value) {
                        self.flags = f;
                    }
                }
                b"name" => {
                    let mut name = text_value(line, value);
                    replace_nonstandard_ascii(&mut name, b' ');
                    replace_char(&mut name, b'|', b' ');
                    self.object.name = name;
                }
                b"desc" => {
                    let mut desc = text_value(line, value);
                    replace_nonstandard_ascii(&mut desc, b' ');
                    self.description = desc;
                }
                b"creation_date" => {
                    if let Some(date) = scan_int(value) {
                        self.object.creation_date = i64::from(date);
                    }
                }
                // LL_WARNS "unknown keyword ... in inventory import of item".
                _ => {}
            }
        }
        self.repair_inventory_type();
        success
    }

    /// The deprecated `perm_mask` of a sale block (legacy and LLSD readers):
    /// `PERM_NONE` means the owner mask; without copy, transfer is added
    /// (fair use); the result is the next-owner mask, raw.
    pub(crate) fn apply_legacy_perm_mask(&mut self, mut perm_mask: u32) {
        if perm_mask == PERM_NONE {
            perm_mask = self.permissions.owner_mask;
        }
        if perm_mask & PERM_COPY == 0 {
            perm_mask |= PERM_TRANSFER;
        }
        self.permissions.next = perm_mask;
    }

    /// `LLInventoryItem::exportLegacyStream(output, include_asset_key)`:
    /// appends the block. With the asset key, the asset id is written as
    /// `asset_id` or as `shadow_id` (see [`crate::shadow`]); without it, a
    /// null `asset_id`. The `inv_type` line is left out for an inventory
    /// type without a name; the date is the full `time_t`. Always returns
    /// true.
    pub fn export_legacy_stream(&self, out: &mut Vec<u8>, include_asset_key: bool) -> bool {
        out.extend_from_slice(b"\tinv_item\t0\n\t{\n");
        push_id_line(out, "item_id", &self.object.id);
        push_id_line(out, "parent_id", &self.object.parent);
        self.permissions.export_legacy_stream(out);
        push_metadata(out, &self.object.thumbnail, &self.object.runtime);
        if include_asset_key {
            let e = exported_asset_id(self.permissions.base, &self.asset);
            push_id_line(out, e.label(), e.id());
        } else {
            push_id_line(out, "asset_id", &NULL_ID);
        }
        push_type_line(out, "type", self.object.asset_type.lookup());
        let inv_type = self.inv_type.lookup();
        if !inv_type.is_empty() {
            push_type_line(out, "inv_type", inv_type);
        }
        let _ = writeln!(out, "\t\tflags\t{:08x}", self.flags);
        self.sale_info.export_legacy_stream(out);
        push_text_line(out, "name", &self.object.name);
        push_text_line(out, "desc", &self.description);
        let _ = writeln!(out, "\t\tcreation_date\t{}", self.object.creation_date);
        out.extend_from_slice(b"\t}\n");
        true
    }
}
