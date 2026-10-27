//! `LLInventoryCategory::importLegacyStream` and `exportLegacyStream`
//! (`llinventory.cpp`).
//!
//! ```text
//! \tinv_category\t0
//! \t{
//! \t\tcat_id\t<uuid>
//! \t\tparent_id\t<uuid>
//! \t\ttype\t<asset type name>
//! \t\tpref_type\t<folder type name>
//! \t\tname\t<name>|
//! \t\tmetadata\t<llsd>...</llsd>\n|        (a thumbnail only)
//! \t}
//! ```
//!
//! The reader starts after the `inv_category\t0` line, which its caller has
//! consumed.

use vkstorm_llsd::text::uuid_set;

use super::object::{apply_metadata, push_id_line, push_metadata, push_text_line, push_type_line, rescanned_value, KEYWORD_VALUE};
use super::reader::Stream;
use super::sale::MAX_STRING;
use super::scanf::sscanf;
use crate::category::Category;
use crate::text::sanitize_in_place;
use crate::types::{AssetType, FolderType};

impl Category {
    /// `LLInventoryCategory::importLegacyStream`.
    ///
    /// Over the category as it is, reads lines of up to 254 bytes while the
    /// stream is good, each parsed on its own as `" %254s %254s"`:
    /// - a blank line and `{` are skipped, `}` ends the block;
    /// - a keyword without a value is skipped, except `metadata`;
    /// - `cat_id` and `parent_id` set the ids by `LLUUID::set`;
    /// - `type` sets the asset type by name (`AT_UNKNOWN` when unknown) and
    ///   `pref_type` the preferred type (`FT_NONE` when unknown);
    /// - `name` re-reads the line as `" %254s %254[^|]"` (leading white
    ///   space skipped), then `replaceNonstandardASCII` and
    ///   `replaceChar('|')`, untrimmed;
    /// - `metadata` sets the runtime, thumbnail and favorite;
    /// - an unknown keyword is skipped (the C++ logs a warning).
    ///
    /// Always returns true.
    pub fn import_legacy_stream(&mut self, input: &mut Stream<'_>) -> bool {
        while input.good() {
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
            if scan.ret < 2 && keyword != b"metadata" {
                continue;
            }
            match keyword {
                b"cat_id" => self.object.id = uuid_set(value),
                b"parent_id" => self.object.parent = uuid_set(value),
                b"type" => self.object.asset_type = AssetType::from_name_cstr(Some(value)),
                b"pref_type" => self.preferred_type = FolderType::from_name(value),
                b"name" => {
                    let rescan = sscanf(line, " %254s %254[^|]");
                    let mut name = rescanned_value(value, &rescan, 1);
                    sanitize_in_place(&mut name);
                    self.object.name = name;
                }
                b"metadata" => apply_metadata(&mut self.object, value),
                // LL_WARNS "unknown keyword ... in inventory import category".
                _ => {}
            }
        }
        true
    }

    /// `LLInventoryCategory::exportLegacyStream`: appends the block; a
    /// `metadata` line holds the thumbnail only (never the runtime or the
    /// favorite). Always returns true; the asset key flag is unused.
    pub fn export_legacy_stream(&self, out: &mut Vec<u8>) -> bool {
        out.extend_from_slice(b"\tinv_category\t0\n\t{\n");
        push_id_line(out, "cat_id", &self.object.id);
        push_id_line(out, "parent_id", &self.object.parent);
        push_type_line(out, "type", self.object.asset_type.lookup());
        push_type_line(out, "pref_type", self.preferred_type.lookup());
        push_text_line(out, "name", &self.object.name);
        push_metadata(out, &self.object.thumbnail, &[]);
        out.extend_from_slice(b"\t}\n");
        true
    }
}
