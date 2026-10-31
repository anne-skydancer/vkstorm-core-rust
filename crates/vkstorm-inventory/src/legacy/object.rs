//! `LLInventoryObject::importLegacyStream` and `exportLegacyStream`
//! (`llinventory.cpp`), and the `metadata` line the object, item and
//! category forms share.
//!
//! ```text
//! \tinv_object\t0
//! \t{
//! \t\tobj_id\t<uuid>
//! \t\tparent_id\t<uuid>
//! \t\ttype\t<asset type name>
//! \t\tname\t<name>|
//! \t}
//! ```
//!
//! The reader starts after the `inv_object\t0` line, which its caller has
//! consumed.

use vkstorm_llsd::text::{uuid_set, uuid_to_hex};
use vkstorm_llsd::{Format, Limits, Options, Value};

use super::reader::{Stream, c_str};
use super::sale::MAX_STRING;
use super::scanf::{Scan, sscanf};
use crate::llsd::coerce::{as_boolean, as_string, as_uuid, get, has, insert};
use crate::object::Object;
use crate::perms::{NULL_ID, Uuid};
use crate::text::correct_inventory_name;
use crate::types::AssetType;

/// `" %254s %254s"`: the keyword and value of a line.
pub(crate) const KEYWORD_VALUE: &str = " %254s %254s";

/// Appends `\t\t<key>\t<uuid>\n`.
pub(crate) fn push_id_line(out: &mut Vec<u8>, key: &str, id: &Uuid) {
    out.extend_from_slice(b"\t\t");
    out.extend_from_slice(key.as_bytes());
    out.push(b'\t');
    uuid_to_hex(id, out);
    out.push(b'\n');
}

/// Appends `\t\t<key>\t<c_str(value)>|\n`: a name or description, which
/// `operator<<(const char*)` writes up to its first NUL.
pub(crate) fn push_text_line(out: &mut Vec<u8>, key: &str, value: &[u8]) {
    out.extend_from_slice(b"\t\t");
    out.extend_from_slice(key.as_bytes());
    out.push(b'\t');
    out.extend_from_slice(c_str(value));
    out.extend_from_slice(b"|\n");
}

/// The `metadata` line of the item and category writers:
/// `\t\tmetadata\t` + `LLSDSerialize::toXML` of a map with
/// `thumbnail: {asset_id}` (a non-null thumbnail) and `script: {runtime}`
/// (a non-empty runtime) + `|\n`. The XML ends in its own newline, so the
/// `|` is on a line of its own. Nothing is written when the map would be
/// empty.
pub(crate) fn push_metadata(out: &mut Vec<u8>, thumbnail: &Uuid, runtime: &[u8]) {
    if *thumbnail == NULL_ID && runtime.is_empty() {
        return;
    }
    let mut metadata = Value::Undefined;
    if *thumbnail != NULL_ID {
        let mut t = Value::map();
        insert(&mut t, "asset_id", Value::Uuid(*thumbnail));
        insert(&mut metadata, "thumbnail", t);
    }
    if !runtime.is_empty() {
        let mut s = Value::map();
        insert(&mut s, "runtime", Value::String(runtime.to_vec()));
        insert(&mut metadata, "script", s);
    }
    out.extend_from_slice(b"\t\tmetadata\t");
    out.extend_from_slice(&vkstorm_llsd::format(Format::Xml, &metadata, Options::default()).0);
    out.extend_from_slice(b"|\n");
}

/// The readers' `metadata` branch, over the line's value token (at most 254
/// bytes, no white space). A token starting with `<llsd>` is parsed by
/// `LLSDSerialize::fromXML` (a failed parse leaves it undefined); any other
/// token is ignored (the C++ notes a notation form it does not support).
/// Then:
/// - the runtime becomes `metadata.script.runtime` as a string (empty when
///   absent);
/// - the thumbnail becomes `metadata.thumbnail.asset_id`, null when either
///   key is missing;
/// - the favorite becomes `metadata.favorite.toggled` as a boolean, false
///   when either key is missing.
pub(crate) fn apply_metadata(object: &mut Object, value: &[u8]) {
    let metadata = if value.starts_with(b"<llsd>") {
        match vkstorm_llsd::parse(Format::Xml, value, Limits::UNLIMITED) {
            (n, Some(v)) if n > 0 => v,
            _ => Value::Undefined,
        }
    } else {
        Value::Undefined
    };
    object.runtime = as_string(get(get(&metadata, "script"), "runtime"));
    object.thumbnail = if has(&metadata, "thumbnail") {
        let thumbnail = get(&metadata, "thumbnail");
        if has(thumbnail, "asset_id") {
            as_uuid(get(thumbnail, "asset_id"))
        } else {
            NULL_ID
        }
    } else {
        NULL_ID
    };
    object.favorite = if has(&metadata, "favorite") {
        let favorite = get(&metadata, "favorite");
        has(favorite, "toggled") && as_boolean(get(favorite, "toggled"))
    } else {
        false
    };
}

/// The value a `name` (or `desc`) line's second `sscanf` leaves in
/// `valuestr`: the conversion at `index` when it was stored, else what the
/// first `" %254s %254s"` stored there (`first`).
pub(crate) fn rescanned_value(first: &[u8], rescan: &Scan, index: usize) -> Vec<u8> {
    rescan.str(index).unwrap_or(first).to_vec()
}

impl Object {
    /// `LLInventoryObject::importLegacyStream`.
    ///
    /// Over the object as it is, reads lines of up to 254 bytes while the
    /// stream is good, each parsed on its own as `" %254s %254s"`:
    /// - a blank line and `{` are skipped, `}` ends the block;
    /// - a keyword without a value is skipped, except `metadata`;
    /// - `obj_id` and `parent_id` set the ids by `LLUUID::set`;
    /// - `type` sets the asset type by name (`AT_UNKNOWN` when unknown);
    /// - `metadata` sets the runtime, thumbnail and favorite (see the
    ///   module's `apply_metadata`);
    /// - `name` re-reads the line as `" %254s %254[^|]"` (leading white
    ///   space of the name skipped, up to the first `|`), then
    ///   `correctInventoryName`;
    /// - an unknown keyword is skipped (the C++ logs a warning).
    ///
    /// Always returns true. The stream is left after the `}` line, or with
    /// its state bits set.
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
                b"obj_id" => self.id = uuid_set(value),
                b"parent_id" => self.parent = uuid_set(value),
                b"type" => self.asset_type = AssetType::from_name_cstr(Some(value)),
                b"metadata" => apply_metadata(self, value),
                b"name" => {
                    let rescan = sscanf(line, " %254s %254[^|]");
                    self.name = correct_inventory_name(&rescanned_value(value, &rescan, 1));
                }
                // LL_WARNS "unknown keyword ... in LLInventoryObject::importLegacyStream()".
                _ => {}
            }
        }
        true
    }

    /// `LLInventoryObject::exportLegacyStream`: appends the block (the id,
    /// parent, type name and name; no metadata). Always returns true; the
    /// asset key flag is unused.
    pub fn export_legacy_stream(&self, out: &mut Vec<u8>) -> bool {
        out.extend_from_slice(b"\tinv_object\t0\n\t{\n");
        push_id_line(out, "obj_id", &self.id);
        push_id_line(out, "parent_id", &self.parent);
        push_type_line(out, "type", self.asset_type.lookup());
        push_text_line(out, "name", &self.name);
        out.extend_from_slice(b"\t}\n");
        true
    }
}

/// Appends `\t\t<key>\t<name>\n`.
pub(crate) fn push_type_line(out: &mut Vec<u8>, key: &str, name: &str) {
    out.extend_from_slice(b"\t\t");
    out.extend_from_slice(key.as_bytes());
    out.push(b'\t');
    out.extend_from_slice(name.as_bytes());
    out.push(b'\n');
}
