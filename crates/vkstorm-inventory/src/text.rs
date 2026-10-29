//! Name and description normalisation: `LLInventoryObject::correctInventoryName`
//! and `LLInventoryItem::correctInventoryDescription` (`llinventory.cpp`), the
//! `LLStringUtil` primitives they are built from (`llcommon/llstring.h`), and
//! the name and description handling of the item and category constructors,
//! `LLInventoryObject::rename` and `LLInventoryItem::setDescription`.
//!
//! C++ `std::string` holds bytes, not UTF-8, so every function here works on
//! byte strings. Embedded NULs are ordinary bytes. The rules are:
//!
//! - [`replace_nonstandard_ascii`] replaces every byte that is negative as a
//!   signed `char` (0x80-0xff) or below 0x20, except LF, so a multi-byte UTF-8
//!   sequence becomes one space per byte. DEL (0x7f) is kept.
//! - Names then have `|` replaced by a space, are trimmed at both ends and are
//!   cut to [`DB_INV_ITEM_NAME_STR_LEN`] bytes. Because the replacement runs
//!   first, only ASCII remains when the cut is made, so a cut never lands
//!   inside a UTF-8 sequence.
//! - Descriptions have the same replacements, but are neither trimmed nor cut.
//!
//! `LLStringUtil::trim` uses `isspace` of the current C locale. Every byte it
//! sees has already been through [`replace_nonstandard_ascii`] (LF or
//! 0x20-0x7f), and on these bytes `isspace` is the same in every ASCII locale
//! of both C runtimes, so the corpus (C2) records no `RT(...)` value here.

/// `DB_INV_ITEM_NAME_STR_LEN` (`llmessage/lldbstrings.h`): the byte length an
/// inventory name is cut to by [`correct_inventory_name`].
pub const DB_INV_ITEM_NAME_STR_LEN: usize = 63;

/// `DB_INV_ITEM_DESC_STR_LEN` (`llmessage/lldbstrings.h`). The normalisers do
/// not apply it: [`correct_inventory_description`] keeps the whole length.
pub const DB_INV_ITEM_DESC_STR_LEN: usize = 127;

/// `LLStringOps::isSpace(char)`, `isspace((unsigned char)c)` in the "C"
/// locale: space, TAB, LF, VT, FF and CR.
pub const fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `LLStringUtil::replaceNonstandardASCII(string, replacement)`: every byte
/// with `S8(c) < 32` other than LF (`\n`) becomes `replacement`. That is the
/// control bytes 0x00-0x1f except LF and every byte 0x80-0xff; DEL (0x7f) is
/// kept.
pub fn replace_nonstandard_ascii(s: &mut [u8], replacement: u8) {
    for c in s.iter_mut() {
        if (*c as i8) < 32 && *c != b'\n' {
            *c = replacement;
        }
    }
}

/// `LLStringUtil::replaceChar(string, target, replacement)`: every `target`
/// byte becomes `replacement`.
pub fn replace_char(s: &mut [u8], target: u8, replacement: u8) {
    for c in s.iter_mut() {
        if *c == target {
            *c = replacement;
        }
    }
}

/// `LLStringUtil::trim` (`trimHead` then `trimTail`): the slice without the
/// [`is_space`] bytes at both ends.
pub fn trim(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|&c| !is_space(c)).unwrap_or(s.len());
    let end = s
        .iter()
        .rposition(|&c| !is_space(c))
        .map_or(start, |i| i + 1);
    &s[start..end]
}

/// The replacements common to names and descriptions, in place:
/// `replaceNonstandardASCII(s, ' ')` then `replaceChar(s, '|', ' ')`. The
/// legacy, LLSD and message readers apply this pair (or its first half) to
/// the fields they read.
pub fn sanitize_in_place(s: &mut [u8]) {
    replace_nonstandard_ascii(s, b' ');
    replace_char(s, b'|', b' ');
}

/// `LLInventoryObject::correctInventoryName`: [`sanitize_in_place`], then
/// [`trim`], then `LLStringUtil::truncate` to [`DB_INV_ITEM_NAME_STR_LEN`]
/// bytes.
///
/// The item, category and object constructors apply it to their name.
pub fn correct_inventory_name(name: &[u8]) -> Vec<u8> {
    let mut s = name.to_vec();
    sanitize_in_place(&mut s);
    let t = trim(&s);
    t[..t.len().min(DB_INV_ITEM_NAME_STR_LEN)].to_vec()
}

/// `LLInventoryItem::correctInventoryDescription`: [`sanitize_in_place`]
/// over a copy. No trimming, no length limit.
///
/// The item constructor applies the same two replacements to its
/// description.
pub fn correct_inventory_description(desc: &[u8]) -> Vec<u8> {
    let mut s = desc.to_vec();
    sanitize_in_place(&mut s);
    s
}

/// `LLInventoryObject::rename(n)`: the name the object has afterwards, or
/// `None` when it keeps `current`. The corrected name replaces the current
/// one unless it is empty or equal to it.
pub fn rename(current: &[u8], new_name: &[u8]) -> Option<Vec<u8>> {
    let n = correct_inventory_name(new_name);
    (!n.is_empty() && n != current).then_some(n)
}

/// `LLInventoryItem::setDescription(d)`: the description the item has
/// afterwards, or `None` when it keeps `current`. Unlike [`rename`], an empty
/// corrected description does replace the current one.
pub fn set_description(current: &[u8], new_desc: &[u8]) -> Option<Vec<u8>> {
    let d = correct_inventory_description(new_desc);
    (d != current).then_some(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_rules() {
        assert_eq!(correct_inventory_name(b"\x00"), b"");
        assert_eq!(correct_inventory_name(b" a|b\x7f\n"), b"a b\x7f");
        assert_eq!(correct_inventory_name("caf\u{e9}".as_bytes()), b"caf");
        assert_eq!(
            correct_inventory_name(&[b'a'; 70]).len(),
            DB_INV_ITEM_NAME_STR_LEN
        );
        assert_eq!(correct_inventory_description(b"\xc3\xa9|\n"), b"   \n");
        assert_eq!(rename(b"prev", b"  "), None);
        assert_eq!(rename(b"prev", b"prev "), None);
        assert_eq!(set_description(b"x", b""), Some(Vec::new()));
    }
}
