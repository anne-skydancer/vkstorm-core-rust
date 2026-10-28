//! What the model-side rules read from `LLInventoryModel` (`gInventory`),
//! and the viewer item's link-following type.
//!
//! The model containers stay C++. The rules that consult them (the cache
//! decode's item filter, the login load's broken-link and parent checks)
//! read them through [`ModelLookup`]: the stored type of the item or folder
//! under an id, if the model has one. The C ABI can implement it by
//! resolving the ids a call reads before making it ([`ModelObjects`]), so no
//! callback into C++ is needed.

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
    let t = item.object.asset_type;
    if t == AssetType::LINK {
        if let Some(target) = lookup.item_type(&item.asset) {
            if !target.is_link_type() {
                return target;
            }
        }
    } else if t == AssetType::LINK_FOLDER {
        if let Some(target) = lookup.category_type(&item.asset) {
            return target;
        }
    }
    t
}

/// `LLViewerInventoryItem::getIsBrokenLink()`: the viewer type is still a
/// link type (the target is not in the model, or is a link).
pub fn is_broken_link(item: &Item, lookup: &dyn ModelLookup) -> bool {
    viewer_item_type(item, lookup).is_link_type()
}
