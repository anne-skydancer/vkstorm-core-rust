//! `LLPermissions::importLegacyStream` and `exportLegacyStream`
//! (`llpermissions.cpp`).
//!
//! The text is the `permissions` block of the legacy item and task-inventory
//! forms:
//!
//! ```text
//! \tpermissions 0
//! \t{
//! \t\tbase_mask\t7fffffff
//! ...
//! \t}
//! ```
//!
//! The reader starts after the `permissions 0` line, which its caller has
//! consumed.

use std::io::Write as _;

use vkstorm_llsd::text::{uuid_set, uuid_to_hex};

use super::reader::Stream;
use super::scanf::{scan_hex, scan_int, sscanf};
use crate::perms::{NULL_ID, PERM_NONE, PermissionMask, Permissions, Uuid};

/// `BUFSIZE` of the permissions reader: lines of up to 16383 bytes.
pub const PERMISSIONS_LINE_BUFFER: usize = 16384;

/// `is_legacy_mask_keyword`: the mask keywords, whose line reads as
/// `PERM_NONE` when its value is missing or does not parse.
fn is_mask_keyword(keyword: &[u8]) -> bool {
    matches!(
        keyword,
        b"creator_mask"
            | b"base_mask"
            | b"owner_mask"
            | b"group_mask"
            | b"everyone_mask"
            | b"next_owner_mask"
    )
}

fn push_uuid(out: &mut Vec<u8>, id: &Uuid) {
    uuid_to_hex(id, out);
}

impl Permissions {
    /// `LLPermissions::importLegacyStream`.
    ///
    /// Starts from `init(null, null, null, null)`, then reads lines of up to
    /// 16383 bytes while the stream is good, each parsed on its own as
    /// `" %255s %255s"` (bug 29):
    /// - a blank line, `{`, an unknown keyword, and a keyword other than a
    ///   mask keyword without a value are skipped; `}` ends the block;
    /// - a mask line whose value does not parse as `%x` (or is missing)
    ///   sets that mask to `PERM_NONE` (`e09f935ce8`); `creator_mask` sets
    ///   the base mask and runs `fixFairUse()` only after a parsed value;
    /// - an id line sets the id by `LLUUID::set` (a malformed id is null);
    /// - `group_owned` sets the flag from a `%d` value that parses.
    ///
    /// Then `fix()`. Always returns true. The stream is left where the C++
    /// leaves it: after the `}` line, or with its state bits set.
    pub fn import_legacy_stream(&mut self, input: &mut Stream<'_>) -> bool {
        self.init(&NULL_ID, &NULL_ID, &NULL_ID, &NULL_ID);
        while input.good() {
            let line = input.getline(PERMISSIONS_LINE_BUFFER);
            let scan = sscanf(line, " %255s %255s");
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
            if scan.ret < 2 && !is_mask_keyword(keyword) {
                continue;
            }
            let mask = || -> PermissionMask { scan_hex(value).unwrap_or(PERM_NONE) };
            match keyword {
                b"creator_mask" => match scan_hex(value) {
                    Some(m) => {
                        self.base = m;
                        self.fix_fair_use();
                    }
                    None => self.base = PERM_NONE,
                },
                b"base_mask" => self.base = mask(),
                b"owner_mask" => self.owner_mask = mask(),
                b"group_mask" => self.group_mask = mask(),
                b"everyone_mask" => self.everyone = mask(),
                b"next_owner_mask" => self.next = mask(),
                // sscanf(valuestr, "%255s", uuid_str) copies the token.
                b"creator_id" => self.creator = uuid_set(value),
                b"owner_id" => self.owner = uuid_set(value),
                b"last_owner_id" => self.last_owner = uuid_set(value),
                b"group_id" => self.group = uuid_set(value),
                b"group_owned" => {
                    if let Some(v) = scan_int(value) {
                        self.group_owned = v != 0;
                    }
                }
                // LL_INFOS "unknown keyword ... in permissions import".
                _ => {}
            }
        }
        self.fix();
        true
    }

    /// `LLPermissions::exportLegacyStream`: appends the block, the masks as
    /// `%08x`, the ids lower-case and dashed, and `group_owned\t1` only when
    /// group owned. Always returns true.
    pub fn export_legacy_stream(&self, out: &mut Vec<u8>) -> bool {
        out.extend_from_slice(b"\tpermissions 0\n\t{\n");
        for (key, mask) in [
            ("base_mask", self.base),
            ("owner_mask", self.owner_mask),
            ("group_mask", self.group_mask),
            ("everyone_mask", self.everyone),
            ("next_owner_mask", self.next),
        ] {
            let _ = writeln!(out, "\t\t{key}\t{mask:08x}");
        }
        for (key, id) in [
            ("creator_id", &self.creator),
            ("owner_id", &self.owner),
            ("last_owner_id", &self.last_owner),
            ("group_id", &self.group),
        ] {
            let _ = write!(out, "\t\t{key}\t");
            push_uuid(out, id);
            out.push(b'\n');
        }
        if self.group_owned {
            out.extend_from_slice(b"\t\tgroup_owned\t1\n");
        }
        out.extend_from_slice(b"\t}\n");
        true
    }
}
