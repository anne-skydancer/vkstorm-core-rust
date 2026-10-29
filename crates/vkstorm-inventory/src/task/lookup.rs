//! The lookups over `mInventory` (`llviewerobject.cpp`): first match in list
//! order everywhere, indices into the entries front to back.
//!
//! The asset lookups of the C++ cast every entry of a matching type to
//! `LLViewerInventoryItem` without a check; a plain `LLInventoryObject`
//! reached that way (an `inv_object` of a type other than category or none)
//! is read past its end, which is undefined. Here a plain object has no
//! asset and never matches.

use super::Entry;
use crate::perms::Uuid;
use crate::types::AssetType;

/// `getInventoryObject(id)`: the first entry with the id.
pub fn object_by_id(entries: &[Entry], id: &Uuid) -> Option<usize> {
    entries.iter().position(|e| e.id() == *id)
}

/// `getInventoryItem(id)`: the first entry with the id, when it is an item
/// that is not of type `AT_CATEGORY`.
pub fn item_by_id(entries: &[Entry], id: &Uuid) -> Option<usize> {
    let i = object_by_id(entries, id)?;
    let e = &entries[i];
    (e.asset_type() != AssetType::CATEGORY && e.as_item().is_some()).then_some(i)
}

/// `getInventoryContents`: every entry not of type `AT_CATEGORY`.
pub fn contents(entries: &[Entry]) -> Vec<usize> {
    (0..entries.len()).filter(|&i| entries[i].asset_type() != AssetType::CATEGORY).collect()
}

/// `getInventoryRoot`: the last entry.
pub fn root(entries: &[Entry]) -> Option<usize> {
    entries.len().checked_sub(1)
}

/// `getInventoryItemByAsset(asset)`: the first item of a type other than
/// `AT_CATEGORY` and `AT_NONE` with the asset.
pub fn item_by_asset(entries: &[Entry], asset: &Uuid) -> Option<usize> {
    entries.iter().position(|e| {
        let t = e.asset_type();
        t != AssetType::CATEGORY && t != AssetType::NONE && e.as_item().is_some_and(|v| v.item.asset == *asset)
    })
}

/// `getInventoryItemByAsset(asset, type)`: the first item of the type with
/// the asset; none for `AT_CATEGORY` (the C++ asserts).
pub fn item_by_asset_and_type(entries: &[Entry], asset: &Uuid, asset_type: AssetType) -> Option<usize> {
    if asset_type == AssetType::CATEGORY {
        return None;
    }
    entries
        .iter()
        .position(|e| e.asset_type() == asset_type && e.as_item().is_some_and(|v| v.item.asset == *asset))
}

/// `countInventoryContents(type)`.
pub fn count_of_type(entries: &[Entry], asset_type: AssetType) -> usize {
    entries.iter().filter(|e| e.asset_type() == asset_type).count()
}

/// `isAssetInInventory(item, type)` for an item with `asset`: the asset is
/// pending, or an item of the type has it.
pub fn is_asset_in_inventory(entries: Option<&[Entry]>, pending: &[Uuid], asset: &Uuid, asset_type: AssetType) -> bool {
    let is_fetching = pending.contains(asset);
    let is_fetched = entries.is_some_and(|e| item_by_asset_and_type(e, asset, asset_type).is_some());
    is_fetched || is_fetching
}
