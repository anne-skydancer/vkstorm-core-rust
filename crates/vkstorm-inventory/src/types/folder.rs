//! Folder types: `LLFolderType` (`llfoldertype.{h,cpp}`), its values, names,
//! lookups and predicates.
//!
//! The C++ keeps the table in `LLFolderDictionary`, a `std::map` keyed by the
//! enumeration value, with one `"ensemble"` entry per value of
//! `FT_ENSEMBLE_START..=FT_ENSEMBLE_END`. [`ENTRIES`] holds that map in its
//! ascending order, with the ensemble values as one ranged entry.

use vkstorm_llsd::Value;

use super::asset::AssetType;

/// A folder type: `LLFolderType::EType`, as the raw integer the viewer
/// stores. Any value is carried, including those without an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct FolderType(pub i32);

impl FolderType {
    pub const TEXTURE: FolderType = FolderType(0);
    pub const SOUND: FolderType = FolderType(1);
    pub const CALLINGCARD: FolderType = FolderType(2);
    pub const LANDMARK: FolderType = FolderType(3);
    pub const CLOTHING: FolderType = FolderType(5);
    pub const OBJECT: FolderType = FolderType(6);
    pub const NOTECARD: FolderType = FolderType(7);
    pub const ROOT_INVENTORY: FolderType = FolderType(8);
    pub const LSL_TEXT: FolderType = FolderType(10);
    pub const BODYPART: FolderType = FolderType(13);
    pub const TRASH: FolderType = FolderType(14);
    pub const SNAPSHOT_CATEGORY: FolderType = FolderType(15);
    pub const LOST_AND_FOUND: FolderType = FolderType(16);
    pub const ANIMATION: FolderType = FolderType(20);
    pub const GESTURE: FolderType = FolderType(21);
    pub const FAVORITE: FolderType = FolderType(23);
    pub const ENSEMBLE_START: FolderType = FolderType(26);
    pub const ENSEMBLE_END: FolderType = FolderType(45);
    pub const CURRENT_OUTFIT: FolderType = FolderType(46);
    pub const OUTFIT: FolderType = FolderType(47);
    pub const MY_OUTFITS: FolderType = FolderType(48);
    pub const MESH: FolderType = FolderType(49);
    pub const INBOX: FolderType = FolderType(50);
    pub const OUTBOX: FolderType = FolderType(51);
    pub const BASIC_ROOT: FolderType = FolderType(52);
    pub const MARKETPLACE_LISTINGS: FolderType = FolderType(53);
    pub const MARKETPLACE_STOCK: FolderType = FolderType(54);
    pub const MARKETPLACE_VERSION: FolderType = FolderType(55);
    pub const SETTINGS: FolderType = FolderType(56);
    pub const MATERIAL: FolderType = FolderType(57);
    /// Firestorm's virtual system folders (no dictionary entry).
    pub const FIRESTORM: FolderType = FolderType(58);
    pub const PHOENIX: FolderType = FolderType(59);
    pub const RLV: FolderType = FolderType(60);
    /// OpenSim hypergrid suitcase.
    pub const MY_SUITCASE: FolderType = FolderType(100);
    pub const COUNT: FolderType = FolderType(101);
    pub const NONE: FolderType = FolderType(-1);
}

/// `LLFolderType::badLookup()`: what the by-value name lookup returns for a
/// value without an entry.
pub const BAD_LOOKUP: &str = "llfoldertype_bad_lookup";

/// One `FolderEntry` of `LLFolderDictionary`, covering the values
/// `first..=last` (a single value except for the ensemble range).
#[derive(Clone, Copy, Debug)]
pub struct FolderEntry {
    pub first: FolderType,
    pub last: FolderType,
    /// The dictionary name (at most 8 characters).
    pub name: &'static str,
    /// Can the viewer not change folders of this type?
    pub is_protected: bool,
    /// Always made before the first login?
    pub is_automatic: bool,
    /// Should exist as a unique copy under the root?
    pub is_singleton: bool,
}

const fn e(v: i32, name: &'static str, is_protected: bool, is_automatic: bool, is_singleton: bool) -> FolderEntry {
    FolderEntry {
        first: FolderType(v),
        last: FolderType(v),
        name,
        is_protected,
        is_automatic,
        is_singleton,
    }
}

/// `LLFolderDictionary::LLFolderDictionary()`, in map (ascending value) order.
#[rustfmt::skip]
pub static ENTRIES: [FolderEntry; 31] = [
    //  value  name       protected automatic singleton
    e(-1,  "-1",       false, false, false),
    e(0,   "texture",  true,  true,  true),
    e(1,   "sound",    true,  true,  true),
    e(2,   "callcard", true,  true,  false),
    e(3,   "landmark", true,  false, false),
    e(5,   "clothing", true,  true,  true),
    e(6,   "object",   true,  true,  true),
    e(7,   "notecard", true,  true,  true),
    e(8,   "root_inv", true,  true,  true),
    e(10,  "lsltext",  true,  true,  true),
    e(13,  "bodypart", true,  true,  true),
    e(14,  "trash",    true,  false, true),
    e(15,  "snapshot", true,  true,  true),
    e(16,  "lstndfnd", true,  true,  true),
    e(20,  "animatn",  true,  true,  true),
    e(21,  "gesture",  true,  true,  true),
    e(23,  "favorite", true,  false, true),
    FolderEntry {
        first: FolderType::ENSEMBLE_START,
        last: FolderType::ENSEMBLE_END,
        name: "ensemble",
        is_protected: false,
        is_automatic: false,
        is_singleton: false,
    },
    e(46,  "current",  true,  false, true),
    e(47,  "outfit",   false, false, false),
    e(48,  "my_otfts", true,  false, true),
    e(49,  "mesh",     true,  false, false),
    e(50,  "inbox",    true,  false, true),
    e(51,  "outbox",   false, false, false),
    e(52,  "basic_rt", true,  false, false),
    e(53,  "merchant", false, false, false),
    e(54,  "stock",    false, false, false),
    e(55,  "version",  false, false, false),
    e(56,  "settings", true,  false, true),
    e(57,  "material", true,  false, true),
    e(100, "suitcase", true,  false, true),
];

impl FolderType {
    /// The dictionary entry of this value (`LLFolderDictionary::lookup(EType)`).
    pub fn entry(self) -> Option<&'static FolderEntry> {
        ENTRIES.iter().find(|e| e.first <= self && self <= e.last)
    }

    /// `LLFolderType::lookup(EType)`: the name, or [`BAD_LOOKUP`].
    pub fn lookup(self) -> &'static str {
        self.entry().map_or(BAD_LOOKUP, |e| e.name)
    }

    /// `LLFolderType::lookup(const std::string&)`: the first value, in value
    /// order, whose name equals `name` exactly (`"ensemble"` gives
    /// `FT_ENSEMBLE_START`), else `FT_NONE`.
    pub fn from_name(name: &[u8]) -> FolderType {
        ENTRIES
            .iter()
            .find(|e| e.name.as_bytes() == name)
            .map_or(FolderType::NONE, |e| e.first)
    }

    /// `LLFolderType::lookupIsProtectedType`: true without an entry.
    pub fn is_protected(self) -> bool {
        self.entry().is_none_or(|e| e.is_protected)
    }

    /// `LLFolderType::lookupIsAutomaticType`: true without an entry.
    pub fn is_automatic(self) -> bool {
        self.entry().is_none_or(|e| e.is_automatic)
    }

    /// `LLFolderType::lookupIsSingletonType`: true without an entry.
    pub fn is_singleton(self) -> bool {
        self.entry().is_none_or(|e| e.is_singleton)
    }

    /// `LLFolderType::lookupIsEnsembleType`: `FT_ENSEMBLE_START..=FT_ENSEMBLE_END`.
    pub fn is_ensemble(self) -> bool {
        FolderType::ENSEMBLE_START <= self && self <= FolderType::ENSEMBLE_END
    }

    /// `LLFolderType::folderTypeToAssetType`: the same integer (the C++ only
    /// logs a warning when the asset type has no name).
    pub fn to_asset_type(self) -> AssetType {
        AssetType(self.0)
    }

    /// `LLFolderType::assetTypeToFolderType`: the same integer (the C++ only
    /// logs a warning when the folder type has no name).
    pub fn from_asset_type(asset_type: AssetType) -> FolderType {
        FolderType(asset_type.0)
    }

    /// `LLFolderType::getTypeNames`: the names of the values
    /// `FT_TEXTURE..FT_COUNT` that have an entry, the ensemble range left
    /// out, as an LLSD array.
    pub fn type_names() -> Value {
        Value::Array(
            ENTRIES
                .iter()
                .filter(|e| !e.first.is_ensemble() && (FolderType::TEXTURE..FolderType::COUNT).contains(&e.first))
                .map(|e| Value::string(e.name))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_in_map_order() {
        assert!(ENTRIES.windows(2).all(|w| w[0].last < w[1].first));
        assert!(ENTRIES.iter().all(|e| e.name.len() <= 8));
    }
}
