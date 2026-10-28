//! Asset types: `LLAssetType` (`llcommon/llassettype.{h,cpp}`), its values,
//! names and lookups.
//!
//! The C++ keeps the table in `LLAssetDictionary`, a `std::map` keyed by the
//! enumeration value. Lookups by value find one entry; lookups by name walk
//! the map in ascending key order (so `AT_NONE`, -1, comes first) and return
//! the first match. [`ENTRIES`] is that map, in that order.

use vkstorm_llsd::Value;

/// An asset type: `LLAssetType::EType`, as the raw integer the viewer stores.
/// Any value is carried, including those without an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct AssetType(pub i32);

impl AssetType {
    pub const TEXTURE: AssetType = AssetType(0);
    pub const SOUND: AssetType = AssetType(1);
    pub const CALLINGCARD: AssetType = AssetType(2);
    pub const LANDMARK: AssetType = AssetType(3);
    pub const SCRIPT: AssetType = AssetType(4);
    pub const CLOTHING: AssetType = AssetType(5);
    pub const OBJECT: AssetType = AssetType(6);
    pub const NOTECARD: AssetType = AssetType(7);
    pub const CATEGORY: AssetType = AssetType(8);
    pub const LSL_TEXT: AssetType = AssetType(10);
    pub const LSL_BYTECODE: AssetType = AssetType(11);
    pub const TEXTURE_TGA: AssetType = AssetType(12);
    pub const BODYPART: AssetType = AssetType(13);
    pub const SOUND_WAV: AssetType = AssetType(17);
    pub const IMAGE_TGA: AssetType = AssetType(18);
    pub const IMAGE_JPEG: AssetType = AssetType(19);
    pub const ANIMATION: AssetType = AssetType(20);
    pub const GESTURE: AssetType = AssetType(21);
    pub const SIMSTATE: AssetType = AssetType(22);
    pub const LINK: AssetType = AssetType(24);
    pub const LINK_FOLDER: AssetType = AssetType(25);
    pub const MARKETPLACE_FOLDER: AssetType = AssetType(26);
    pub const WIDGET: AssetType = AssetType(40);
    pub const PERSON: AssetType = AssetType(45);
    pub const MESH: AssetType = AssetType(49);
    pub const RESERVED_1: AssetType = AssetType(50);
    pub const RESERVED_2: AssetType = AssetType(51);
    pub const RESERVED_3: AssetType = AssetType(52);
    pub const RESERVED_4: AssetType = AssetType(53);
    pub const RESERVED_5: AssetType = AssetType(54);
    pub const RESERVED_6: AssetType = AssetType(55);
    pub const SETTINGS: AssetType = AssetType(56);
    pub const MATERIAL: AssetType = AssetType(57);
    pub const GLTF: AssetType = AssetType(58);
    pub const GLTF_BIN: AssetType = AssetType(59);
    /// `AT_COUNT`: one past the highest value with a slot in the default
    /// inventory type table.
    pub const COUNT: AssetType = AssetType(60);
    pub const UNKNOWN: AssetType = AssetType(255);
    pub const NONE: AssetType = AssetType(-1);
}

/// `LLAssetType::BADLOOKUP`: what the by-value lookups return for a value
/// without an entry.
pub const BADLOOKUP: &str = "llassettype_bad_lookup";

/// One `AssetEntry` of `LLAssetDictionary`.
#[derive(Clone, Copy, Debug)]
pub struct AssetEntry {
    pub asset_type: AssetType,
    /// The dictionary name (`LLDictionaryEntry::mName`), returned by `getDesc`.
    pub desc: &'static str,
    /// `mTypeName`: the machine name (at most 8 characters).
    pub type_name: &'static str,
    /// `mHumanName`; `NULL` for `AT_UNKNOWN` and `AT_NONE`.
    pub human_name: Option<&'static str>,
    pub can_link: bool,
    pub can_fetch: bool,
    pub can_know: bool,
}

const fn e(
    v: i32,
    desc: &'static str,
    type_name: &'static str,
    human_name: Option<&'static str>,
    can_link: bool,
    can_fetch: bool,
    can_know: bool,
) -> AssetEntry {
    AssetEntry {
        asset_type: AssetType(v),
        desc,
        type_name,
        human_name,
        can_link,
        can_fetch,
        can_know,
    }
}

/// `LLAssetDictionary::LLAssetDictionary()`, in map (ascending value) order.
#[rustfmt::skip]
pub static ENTRIES: [AssetEntry; 30] = [
    //  value  description      type name   human name                    link   fetch  know
    e(-1,  "NONE",          "-1",       None,                         false, false, false),
    e(0,   "TEXTURE",       "texture",  Some("texture"),              true,  false, true),
    e(1,   "SOUND",         "sound",    Some("sound"),                true,  true,  true),
    e(2,   "CALLINGCARD",   "callcard", Some("calling card"),         true,  false, false),
    e(3,   "LANDMARK",      "landmark", Some("landmark"),             true,  true,  true),
    e(4,   "SCRIPT",        "script",   Some("legacy script"),        true,  false, false),
    e(5,   "CLOTHING",      "clothing", Some("clothing"),             true,  true,  true),
    e(6,   "OBJECT",        "object",   Some("object"),               true,  false, false),
    e(7,   "NOTECARD",      "notecard", Some("note card"),            true,  false, true),
    e(8,   "CATEGORY",      "category", Some("folder"),               true,  false, false),
    e(10,  "LSL_TEXT",      "lsltext",  Some("lsl2 script"),          true,  false, false),
    e(11,  "LSL_BYTECODE",  "lslbyte",  Some("lsl bytecode"),         true,  false, false),
    e(12,  "TEXTURE_TGA",   "txtr_tga", Some("tga texture"),          true,  false, false),
    e(13,  "BODYPART",      "bodypart", Some("body part"),            true,  true,  true),
    e(17,  "SOUND_WAV",     "snd_wav",  Some("sound"),                true,  false, false),
    e(18,  "IMAGE_TGA",     "img_tga",  Some("targa image"),          true,  false, false),
    e(19,  "IMAGE_JPEG",    "jpeg",     Some("jpeg image"),           true,  false, false),
    e(20,  "ANIMATION",     "animatn",  Some("animation"),            true,  true,  true),
    e(21,  "GESTURE",       "gesture",  Some("gesture"),              true,  true,  true),
    e(22,  "SIMSTATE",      "simstate", Some("simstate"),             false, false, false),
    e(24,  "LINK",          "link",     Some("sym link"),             false, false, true),
    e(25,  "FOLDER_LINK",   "link_f",   Some("sym folder link"),      false, false, true),
    e(40,  "WIDGET",        "widget",   Some("widget"),               false, false, false),
    e(45,  "PERSON",        "person",   Some("person"),               false, false, false),
    e(49,  "MESH",          "mesh",     Some("mesh"),                 false, false, false),
    e(56,  "SETTINGS",      "settings", Some("settings blob"),        true,  true,  true),
    e(57,  "MATERIAL",      "material", Some("render material"),      true,  true,  true),
    e(58,  "GLTF",          "gltf",     Some("GLTF"),                 true,  true,  true),
    e(59,  "GLTF_BIN",      "glbin",    Some("GLTF binary"),          true,  true,  true),
    e(255, "UNKNOWN",       "invalid",  None,                         false, false, false),
];

/// The bytes of a C string argument as `ll_safe_string` sees them: `NULL` is
/// the empty string, and the string ends at its first NUL.
pub(crate) fn c_str(name: Option<&[u8]>) -> &[u8] {
    let s = name.unwrap_or(b"");
    match s.iter().position(|&b| b == 0) {
        Some(n) => &s[..n],
        None => s,
    }
}

impl AssetType {
    /// The dictionary entry of this value (`LLAssetDictionary::lookup(EType)`).
    pub fn entry(self) -> Option<&'static AssetEntry> {
        ENTRIES
            .binary_search_by_key(&self.0, |e| e.asset_type.0)
            .ok()
            .map(|i| &ENTRIES[i])
    }

    /// `LLAssetType::lookup(EType)`: the type name, or [`BADLOOKUP`].
    pub fn lookup(self) -> &'static str {
        self.entry().map_or(BADLOOKUP, |e| e.type_name)
    }

    /// `LLAssetType::lookupHumanReadable(EType)`: the human name; `None`
    /// (`NULL`) for `AT_UNKNOWN` and `AT_NONE`; [`BADLOOKUP`] for a value
    /// without an entry.
    pub fn lookup_human_readable(self) -> Option<&'static str> {
        match self.entry() {
            Some(e) => e.human_name,
            None => Some(BADLOOKUP),
        }
    }

    /// `LLAssetType::getDesc`: the dictionary name, or [`BADLOOKUP`].
    pub fn desc(self) -> &'static str {
        self.entry().map_or(BADLOOKUP, |e| e.desc)
    }

    /// `LLAssetType::lookupCanLink`: false without an entry.
    pub fn can_link(self) -> bool {
        self.entry().is_some_and(|e| e.can_link)
    }

    /// `LLAssetType::lookupIsLinkType`: `AT_LINK` or `AT_LINK_FOLDER`.
    pub fn is_link_type(self) -> bool {
        self == AssetType::LINK || self == AssetType::LINK_FOLDER
    }

    /// `LLAssetType::lookupIsAssetFetchByIDAllowed`: false without an entry.
    pub fn is_asset_fetch_by_id_allowed(self) -> bool {
        self.entry().is_some_and(|e| e.can_fetch)
    }

    /// `LLAssetType::lookupIsAssetIDKnowable`: false without an entry.
    pub fn is_asset_id_knowable(self) -> bool {
        self.entry().is_some_and(|e| e.can_know)
    }

    /// `LLAssetType::lookup(const std::string&)`: the first entry, in value
    /// order, whose type name equals `name` exactly, else `AT_UNKNOWN`.
    /// `"-1"` gives `AT_NONE` and `"invalid"` `AT_UNKNOWN`.
    pub fn from_name(name: &[u8]) -> AssetType {
        ENTRIES
            .iter()
            .find(|e| e.type_name.as_bytes() == name)
            .map_or(AssetType::UNKNOWN, |e| e.asset_type)
    }

    /// `LLAssetType::lookup(const char*)`: `NULL` is the empty string (so
    /// `AT_UNKNOWN`), and the name ends at its first NUL.
    pub fn from_name_cstr(name: Option<&[u8]>) -> AssetType {
        Self::from_name(c_str(name))
    }

    /// `LLAssetType::lookupHumanReadable(const std::string&)`: the first
    /// entry, in value order, with a human name equal to `name`, else
    /// `AT_NONE`. `"sound"` gives `AT_SOUND`, never `AT_SOUND_WAV`.
    pub fn from_human_readable(name: &[u8]) -> AssetType {
        ENTRIES
            .iter()
            .find(|e| e.human_name.is_some_and(|h| h.as_bytes() == name))
            .map_or(AssetType::NONE, |e| e.asset_type)
    }

    /// `LLAssetType::lookupHumanReadable(const char*)`: `NULL` is the empty
    /// string (so `AT_NONE`), and the name ends at its first NUL.
    pub fn from_human_readable_cstr(name: Option<&[u8]>) -> AssetType {
        Self::from_human_readable(c_str(name))
    }

    /// `LLAssetType::getType`: the entry whose dictionary name equals `name`
    /// upper cased, else `AT_NONE` (`LLDictionary::notFound`). The C++ upper
    /// cases each byte with `toupper`; only ASCII can reach an entry name
    /// (all upper case ASCII), so ASCII upper casing gives the same result in
    /// any locale.
    pub fn from_desc(name: &[u8]) -> AssetType {
        ENTRIES
            .iter()
            .find(|e| {
                e.desc.len() == name.len()
                    && e.desc.bytes().zip(name).all(|(d, &n)| d == n.to_ascii_uppercase())
            })
            .map_or(AssetType::NONE, |e| e.asset_type)
    }

    /// `LLAssetType::getTypeNames`: the type names of the values
    /// `AT_TEXTURE..AT_COUNT` that have an entry, as an LLSD array.
    pub fn type_names() -> Value {
        Value::Array(
            ENTRIES
                .iter()
                .filter(|e| (AssetType::TEXTURE.0..AssetType::COUNT.0).contains(&e.asset_type.0))
                .map(|e| Value::string(e.type_name))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_in_map_order() {
        assert!(ENTRIES.windows(2).all(|w| w[0].asset_type.0 < w[1].asset_type.0));
        assert!(ENTRIES.iter().all(|e| e.type_name.len() <= 8));
    }
}
