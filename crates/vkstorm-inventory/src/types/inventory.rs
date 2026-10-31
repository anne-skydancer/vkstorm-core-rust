//! Inventory types: `LLInventoryType` (`llinventorytype.{h,cpp}`), including
//! `defaultForAssetType` and `inventory_and_asset_types_match`.
//!
//! The C++ keeps the table in `LLInventoryDictionary`, a `std::map` keyed by
//! the enumeration value; [`ENTRIES`] is that map in its ascending order,
//! which the name lookup walks.

use super::asset::AssetType;

/// An inventory type: `LLInventoryType::EType`, as the raw integer the viewer
/// stores. Any value is carried, including those without an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct InventoryType(pub i32);

impl InventoryType {
    pub const TEXTURE: InventoryType = InventoryType(0);
    pub const SOUND: InventoryType = InventoryType(1);
    pub const CALLINGCARD: InventoryType = InventoryType(2);
    pub const LANDMARK: InventoryType = InventoryType(3);
    pub const OBJECT: InventoryType = InventoryType(6);
    pub const NOTECARD: InventoryType = InventoryType(7);
    pub const CATEGORY: InventoryType = InventoryType(8);
    pub const ROOT_CATEGORY: InventoryType = InventoryType(9);
    pub const LSL: InventoryType = InventoryType(10);
    pub const SNAPSHOT: InventoryType = InventoryType(15);
    pub const ATTACHMENT: InventoryType = InventoryType(17);
    pub const WEARABLE: InventoryType = InventoryType(18);
    pub const ANIMATION: InventoryType = InventoryType(19);
    pub const GESTURE: InventoryType = InventoryType(20);
    pub const MESH: InventoryType = InventoryType(22);
    pub const WIDGET: InventoryType = InventoryType(23);
    pub const PERSON: InventoryType = InventoryType(24);
    pub const SETTINGS: InventoryType = InventoryType(25);
    pub const MATERIAL: InventoryType = InventoryType(26);
    pub const GLTF: InventoryType = InventoryType(27);
    pub const GLTF_BIN: InventoryType = InventoryType(28);
    pub const COUNT: InventoryType = InventoryType(29);
    pub const UNKNOWN: InventoryType = InventoryType(255);
    pub const NONE: InventoryType = InventoryType(-1);
}

/// An inventory icon: `LLInventoryType::EIconName`, as a raw integer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct IconName(pub i32);

impl IconName {
    pub const TEXTURE: IconName = IconName(0);
    pub const SOUND: IconName = IconName(1);
    pub const CALLINGCARD_ONLINE: IconName = IconName(2);
    pub const CALLINGCARD_OFFLINE: IconName = IconName(3);
    pub const LANDMARK: IconName = IconName(4);
    pub const LANDMARK_VISITED: IconName = IconName(5);
    pub const SCRIPT: IconName = IconName(6);
    pub const CLOTHING: IconName = IconName(7);
    pub const OBJECT: IconName = IconName(8);
    pub const OBJECT_MULTI: IconName = IconName(9);
    pub const NOTECARD: IconName = IconName(10);
    pub const BODYPART: IconName = IconName(11);
    pub const SNAPSHOT: IconName = IconName(12);
    pub const BODYPART_SHAPE: IconName = IconName(13);
    pub const BODYPART_SKIN: IconName = IconName(14);
    pub const BODYPART_HAIR: IconName = IconName(15);
    pub const BODYPART_EYES: IconName = IconName(16);
    pub const CLOTHING_SHIRT: IconName = IconName(17);
    pub const CLOTHING_PANTS: IconName = IconName(18);
    pub const CLOTHING_SHOES: IconName = IconName(19);
    pub const CLOTHING_SOCKS: IconName = IconName(20);
    pub const CLOTHING_JACKET: IconName = IconName(21);
    pub const CLOTHING_GLOVES: IconName = IconName(22);
    pub const CLOTHING_UNDERSHIRT: IconName = IconName(23);
    pub const CLOTHING_UNDERPANTS: IconName = IconName(24);
    pub const CLOTHING_SKIRT: IconName = IconName(25);
    pub const CLOTHING_ALPHA: IconName = IconName(26);
    pub const CLOTHING_TATTOO: IconName = IconName(27);
    pub const CLOTHING_UNIVERSAL: IconName = IconName(28);
    pub const ANIMATION: IconName = IconName(29);
    pub const GESTURE: IconName = IconName(30);
    pub const CLOTHING_PHYSICS: IconName = IconName(31);
    pub const LINKITEM: IconName = IconName(32);
    pub const LINKFOLDER: IconName = IconName(33);
    pub const MESH: IconName = IconName(34);
    pub const SETTINGS: IconName = IconName(35);
    pub const SETTINGS_SKY: IconName = IconName(36);
    pub const SETTINGS_WATER: IconName = IconName(37);
    pub const SETTINGS_DAY: IconName = IconName(38);
    pub const MATERIAL: IconName = IconName(39);
    pub const SCRIPT_LUAU: IconName = IconName(40);
    pub const INVALID: IconName = IconName(41);
    pub const UNKNOWN: IconName = IconName(42);
    pub const COUNT: IconName = IconName(43);
    pub const NONE: IconName = IconName(-1);
}

/// `ScriptSubtype_t::SST_LSL`.
pub const SST_LSL: i32 = 0;
/// `ScriptSubtype_t::SST_LUA`.
pub const SST_LUA: i32 = 1;

/// One `InventoryEntry` of `LLInventoryDictionary`.
#[derive(Clone, Copy, Debug)]
pub struct InventoryEntry {
    pub inventory_type: InventoryType,
    /// The dictionary name (`mName`), the legacy and LLSD token.
    pub name: &'static str,
    /// `mHumanName`.
    pub human_name: &'static str,
    /// `mAssetTypes`: the asset types this inventory type may carry.
    pub asset_types: &'static [AssetType],
}

const fn e(
    v: i32,
    name: &'static str,
    human_name: &'static str,
    asset_types: &'static [AssetType],
) -> InventoryEntry {
    InventoryEntry {
        inventory_type: InventoryType(v),
        name,
        human_name,
        asset_types,
    }
}

/// `LLInventoryDictionary::LLInventoryDictionary()`, in map (ascending value)
/// order.
#[rustfmt::skip]
pub static ENTRIES: [InventoryEntry; 21] = [
    e(0,  "texture",   "texture",         &[AssetType::TEXTURE]),
    e(1,  "sound",     "sound",           &[AssetType::SOUND]),
    e(2,  "callcard",  "calling card",    &[AssetType::CALLINGCARD]),
    e(3,  "landmark",  "landmark",        &[AssetType::LANDMARK]),
    e(6,  "object",    "object",          &[AssetType::OBJECT]),
    e(7,  "notecard",  "note card",       &[AssetType::NOTECARD]),
    e(8,  "category",  "folder",          &[]),
    e(9,  "root",      "root",            &[]),
    e(10, "script",    "script",          &[AssetType::LSL_TEXT, AssetType::LSL_BYTECODE]),
    e(15, "snapshot",  "snapshot",        &[AssetType::TEXTURE]),
    e(17, "attach",    "attachment",      &[AssetType::OBJECT]),
    e(18, "wearable",  "wearable",        &[AssetType::CLOTHING, AssetType::BODYPART]),
    e(19, "animation", "animation",       &[AssetType::ANIMATION]),
    e(20, "gesture",   "gesture",         &[AssetType::GESTURE]),
    e(22, "mesh",      "mesh",            &[AssetType::MESH]),
    e(23, "widget",    "widget",          &[AssetType::WIDGET]),
    e(24, "person",    "person",          &[AssetType::PERSON]),
    e(25, "settings",  "settings",        &[AssetType::SETTINGS]),
    e(26, "material",  "render material", &[AssetType::MATERIAL]),
    e(27, "gltf",      "gltf",            &[AssetType::GLTF]),
    e(28, "glbin",     "glbin",           &[AssetType::GLTF_BIN]),
];

/// `DEFAULT_ASSET_FOR_INV_TYPE`: the default inventory type of each asset
/// type `0..AT_COUNT`.
#[rustfmt::skip]
static DEFAULT_FOR_ASSET: [InventoryType; AssetType::COUNT.0 as usize] = {
    use InventoryType as I;
    [
        I::TEXTURE,     // 0    AT_TEXTURE
        I::SOUND,       // 1    AT_SOUND
        I::CALLINGCARD, // 2    AT_CALLINGCARD
        I::LANDMARK,    // 3    AT_LANDMARK
        I::LSL,         // 4    AT_SCRIPT
        I::WEARABLE,    // 5    AT_CLOTHING
        I::OBJECT,      // 6    AT_OBJECT
        I::NOTECARD,    // 7    AT_NOTECARD
        I::CATEGORY,    // 8    AT_CATEGORY
        I::NONE,        // 9    (null entry)
        I::LSL,         // 10   AT_LSL_TEXT
        I::LSL,         // 11   AT_LSL_BYTECODE
        I::TEXTURE,     // 12   AT_TEXTURE_TGA
        I::WEARABLE,    // 13   AT_BODYPART
        I::CATEGORY,    // 14   AT_TRASH
        I::CATEGORY,    // 15   AT_SNAPSHOT_CATEGORY
        I::CATEGORY,    // 16   AT_LOST_AND_FOUND
        I::SOUND,       // 17   AT_SOUND_WAV
        I::NONE,        // 18   AT_IMAGE_TGA
        I::NONE,        // 19   AT_IMAGE_JPEG
        I::ANIMATION,   // 20   AT_ANIMATION
        I::GESTURE,     // 21   AT_GESTURE
        I::NONE,        // 22   AT_SIMSTATE
        I::NONE,        // 23
        I::NONE,        // 24   AT_LINK
        I::NONE,        // 25   AT_LINK_FOLDER
        I::NONE, I::NONE, I::NONE, I::NONE, I::NONE, I::NONE, I::NONE,   // 26-32
        I::NONE, I::NONE, I::NONE, I::NONE, I::NONE, I::NONE, I::NONE,   // 33-39
        I::WIDGET,      // 40   AT_WIDGET
        I::NONE, I::NONE, I::NONE, I::NONE,                               // 41-44
        I::PERSON,      // 45   AT_PERSON
        I::NONE, I::NONE, I::NONE,                                        // 46-48
        I::MESH,        // 49   AT_MESH
        I::NONE, I::NONE, I::NONE, I::NONE, I::NONE, I::NONE,             // 50-55 AT_RESERVED_*
        I::SETTINGS,    // 56   AT_SETTINGS
        I::MATERIAL,    // 57   AT_MATERIAL
        I::GLTF,        // 58   AT_GLTF
        I::GLTF_BIN,    // 59   AT_GLTF_BIN
    ]
};

impl InventoryType {
    /// The dictionary entry of this value (`LLInventoryDictionary::lookup(EType)`).
    pub fn entry(self) -> Option<&'static InventoryEntry> {
        ENTRIES
            .binary_search_by_key(&self.0, |e| e.inventory_type.0)
            .ok()
            .map(|i| &ENTRIES[i])
    }

    /// `LLInventoryType::lookup(EType)`: the name, or the empty string for a
    /// value without an entry.
    pub fn lookup(self) -> &'static str {
        self.entry().map_or("", |e| e.name)
    }

    /// `LLInventoryType::lookupHumanReadable`: the human name, or the empty
    /// string for a value without an entry.
    pub fn lookup_human_readable(self) -> &'static str {
        self.entry().map_or("", |e| e.human_name)
    }

    /// `LLInventoryType::lookup(const std::string&)`: the first entry, in
    /// value order, whose name equals `name` exactly, else `IT_NONE`
    /// (`LLDictionary::notFound`).
    pub fn from_name(name: &[u8]) -> InventoryType {
        ENTRIES
            .iter()
            .find(|e| e.name.as_bytes() == name)
            .map_or(InventoryType::NONE, |e| e.inventory_type)
    }

    /// `LLInventoryType::defaultForAssetType`: the default inventory type of
    /// an asset type in `0..AT_COUNT`, else `IT_UNKNOWN`.
    pub fn default_for_asset_type(asset_type: AssetType) -> InventoryType {
        usize::try_from(asset_type.0)
            .ok()
            .and_then(|i| DEFAULT_FOR_ASSET.get(i))
            .copied()
            .unwrap_or(InventoryType::UNKNOWN)
    }

    /// `LLInventoryType::cannotRestrictPermissions`: calling cards and
    /// landmarks.
    pub fn cannot_restrict_permissions(self) -> bool {
        self == InventoryType::CALLINGCARD || self == InventoryType::LANDMARK
    }

    /// `LLInventoryType::showInWorldPermissions`: every type but settings.
    pub fn show_in_world_permissions(self) -> bool {
        self != InventoryType::SETTINGS
    }
}

/// `inventory_and_asset_types_match`: a link asset type matches any inventory
/// type; otherwise the inventory type must have an entry listing the asset
/// type.
pub fn inventory_and_asset_types_match(
    inventory_type: InventoryType,
    asset_type: AssetType,
) -> bool {
    if asset_type.is_link_type() {
        return true;
    }
    inventory_type
        .entry()
        .is_some_and(|e| e.asset_types.contains(&asset_type))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_in_map_order() {
        assert!(
            ENTRIES
                .windows(2)
                .all(|w| w[0].inventory_type.0 < w[1].inventory_type.0)
        );
    }
}
