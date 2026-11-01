//! What the model-side rules read from `LLInventoryModel` (`gInventory`),
//! and the viewer item's link-following type.
//!
//! The model containers stay C++. The rules that consult them (the cache
//! decode's item filter, the login load's broken-link and parent checks)
//! read them through [`ModelLookup`]: the stored type of the item or folder
//! under an id, if the model has one. The C ABI can implement it by
//! resolving the ids a call reads before making it ([`ModelObjects`]; the
//! login load lists them with [`crate::login::lookup_ids`]), so no callback
//! into C++ is needed.

use std::collections::BTreeMap;

use crate::item::Item;
use crate::perms::Uuid;
use crate::types::AssetType;

/// `LLViewerInventoryCategory::DESCENDENT_COUNT_UNKNOWN`.
pub const DESCENDENT_COUNT_UNKNOWN: i32 = -1;

/// A read-only view of the objects of the C++ model.
pub trait ModelLookup {
    /// `gInventory.getItem(id)`'s stored type (`getActualType`), or `None`
    /// when the model has no item under `id`.
    fn item_type(&self, id: &Uuid) -> Option<AssetType>;
    /// `gInventory.getCategory(id)`'s stored type (`AT_CATEGORY` for every
    /// folder the viewer makes), or `None` when the model has no folder
    /// under `id`.
    fn category_type(&self, id: &Uuid) -> Option<AssetType>;
    /// `gInventory.getCategory(id)->getVersion()`, or `None` when the model
    /// has no folder under `id`.
    fn category_version(&self, id: &Uuid) -> Option<i32>;
}

/// An empty model (no objects).
#[derive(Clone, Copy, Debug, Default)]
pub struct EmptyModel;

impl ModelLookup for EmptyModel {
    fn item_type(&self, _id: &Uuid) -> Option<AssetType> {
        None
    }
    fn category_type(&self, _id: &Uuid) -> Option<AssetType> {
        None
    }
    fn category_version(&self, _id: &Uuid) -> Option<i32> {
        None
    }
}

/// The objects of a model by id, as the C++ wrapper can collect them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelObjects {
    /// Item id to stored type.
    pub items: BTreeMap<Uuid, AssetType>,
    /// Folder id to (stored type, version).
    pub categories: BTreeMap<Uuid, (AssetType, i32)>,
}

impl ModelLookup for ModelObjects {
    fn item_type(&self, id: &Uuid) -> Option<AssetType> {
        self.items.get(id).copied()
    }
    fn category_type(&self, id: &Uuid) -> Option<AssetType> {
        self.categories.get(id).map(|c| c.0)
    }
    fn category_version(&self, id: &Uuid) -> Option<i32> {
        self.categories.get(id).map(|c| c.1)
    }
}

/// `LLViewerInventoryItem::getType()`: an `AT_LINK` whose target is an item
/// of the model that is not itself a link has the target's type
/// (`getLinkedItem`); an `AT_LINK_FOLDER` whose target is a folder of the
/// model has the folder's type (`getLinkedCategory`); otherwise the stored
/// type.
pub fn viewer_item_type(item: &Item, lookup: &dyn ModelLookup) -> AssetType {
    followed_type(item, lookup).0
}

/// [`viewer_item_type`], and whether following the link logged
/// `getLinkedItem`'s `LL_WARNS(LOG_INV)` "Warning: Accessing link to link"
/// (`newview/llviewerinventory.cpp`): an `AT_LINK` whose target is an item of
/// the model with a link type. The C++ logs that line on every call of a
/// getter that follows the link (`getType`, `getName`, `getAssetUUID`, ...),
/// so a caller reports it once per such call it stands for.
pub fn followed_type(item: &Item, lookup: &dyn ModelLookup) -> (AssetType, bool) {
    let t = item.object.asset_type;
    let target = if t == AssetType::LINK {
        lookup.item_type(&item.asset)
    } else if t == AssetType::LINK_FOLDER {
        lookup.category_type(&item.asset)
    } else {
        None
    };
    match target {
        Some(target) if t == AssetType::LINK && target.is_link_type() => (t, true),
        Some(target) => (target, false),
        None => (t, false),
    }
}

/// `LLViewerInventoryItem::getIsBrokenLink()`: the viewer type is still a
/// link type (the target is not in the model, or is a link).
pub fn is_broken_link(item: &Item, lookup: &dyn ModelLookup) -> bool {
    viewer_item_type(item, lookup).is_link_type()
}

/// [`is_broken_link`], and whether the check logged "Warning: Accessing link
/// to link" (see [`followed_type`]).
pub fn broken_link_check(item: &Item, lookup: &dyn ModelLookup) -> (bool, bool) {
    let (t, link_to_link) = followed_type(item, lookup);
    (t.is_link_type(), link_to_link)
}

/// A hasher for UUID keys ([`UuidMap`], [`UuidSet`]): the model's lookups
/// by id, where the order does not matter. Both halves of the id are mixed,
/// so ids that differ only in a few bytes (synthetic ids, `pid()` in the
/// recorders) spread as well as random ones. Not for keys an attacker
/// chooses to collide; ids here come from the viewer's own model.
#[derive(Clone, Copy, Debug, Default)]
pub struct UuidHasher {
    state: u64,
}

impl std::hash::Hasher for UuidHasher {
    fn finish(&self) -> u64 {
        let h = self.state.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        h ^ (h >> 29)
    }

    fn write(&mut self, bytes: &[u8]) {
        if let Ok(id) = <&[u8; 16]>::try_from(bytes) {
            let lo = u64::from_le_bytes([id[0], id[1], id[2], id[3], id[4], id[5], id[6], id[7]]);
            let hi =
                u64::from_le_bytes([id[8], id[9], id[10], id[11], id[12], id[13], id[14], id[15]]);
            self.state ^= lo.wrapping_mul(0xBF58_476D_1CE4_E5B9) ^ hi.rotate_left(32);
        } else {
            for &b in bytes {
                self.state = (self.state ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3);
            }
        }
    }

    // The slice length a `[u8; 16]` hashes first is the same for every key.
    fn write_usize(&mut self, _len: usize) {}
}

/// Builds [`UuidHasher`]s.
pub type UuidBuildHasher = std::hash::BuildHasherDefault<UuidHasher>;
/// A hash map by UUID.
pub type UuidMap<V> = std::collections::HashMap<Uuid, V, UuidBuildHasher>;
/// A hash set of UUIDs.
pub type UuidSet = std::collections::HashSet<Uuid, UuidBuildHasher>;

#[cfg(test)]
mod hasher_tests {
    use super::*;
    use std::hash::BuildHasher;

    /// Ids that differ only in their last bytes still differ in the low
    /// bits a hash table uses.
    #[test]
    fn last_bytes_spread() {
        let b = UuidBuildHasher::default();
        let mut low: std::collections::BTreeSet<u64> = Default::default();
        for n in 0u32..4096 {
            let mut id = [0u8; 16];
            id[0..4].copy_from_slice(&0xa200_0000u32.to_be_bytes());
            id[6] = 0x40;
            id[8] = 0x80;
            id[12..16].copy_from_slice(&n.wrapping_mul(0x9E37_79B1).to_be_bytes());
            low.insert(b.hash_one(id) & 0xfff);
        }
        assert!(low.len() > 2400, "{} distinct buckets of 4096", low.len());
    }
}
