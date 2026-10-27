//! The `LLInventoryCategory` record (`llinventory.{h,cpp}`): its fields and
//! preferred type, and the cache keys of the viewer's
//! `LLViewerInventoryCategory` (`llviewerinventory.{h,cpp}`).
//!
//! [`Category`] is the flat record of an `LLInventoryCategory`: an
//! [`Object`] plus the preferred type. [`ViewerCategory`] adds the owner and
//! version the viewer subclass writes to and reads from its inventory cache.
//! Their LLSD forms are in [`crate::llsd::category`] and the legacy text in
//! [`crate::legacy::category`].

use crate::object::Object;
use crate::perms::{Uuid, NULL_ID};
use crate::types::{AssetType, FolderType};

/// `LLInventoryCategory` as a flat record.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Category {
    /// The `LLInventoryObject` fields.
    pub object: Object,
    /// `mPreferredType`: any integer.
    pub preferred_type: FolderType,
}

impl Default for Category {
    /// `LLInventoryCategory::LLInventoryCategory()`: the default object with
    /// type `AT_CATEGORY`, preferred type `FT_NONE`.
    fn default() -> Self {
        Category {
            object: Object { asset_type: AssetType::CATEGORY, ..Object::default() },
            preferred_type: FolderType::NONE,
        }
    }
}

impl Category {
    /// `LLInventoryCategory(uuid, parent_uuid, preferred_type, name)`: the
    /// object constructor with `AT_CATEGORY` (the name through
    /// `correctInventoryName`), and the preferred type.
    pub fn new(id: &Uuid, parent: &Uuid, preferred_type: FolderType, name: &[u8]) -> Category {
        Category { object: Object::new(id, parent, AssetType::CATEGORY, name), preferred_type }
    }

    /// `copyCategory`: `copyObject` (not the date) and the preferred type.
    pub fn copy_category(&mut self, other: &Category) {
        self.object.copy_object(&other.object);
        self.preferred_type = other.preferred_type;
    }

    /// The copy constructor `LLInventoryCategory(const LLInventoryCategory*)`:
    /// a default object (date 0), then [`copy_category`](Self::copy_category).
    pub fn copy_from(other: &Category) -> Category {
        let mut c = Category { object: Object::default(), preferred_type: FolderType::NONE };
        c.copy_category(other);
        c
    }
}

/// `LLViewerInventoryCategory::VERSION_UNKNOWN`.
pub const VERSION_UNKNOWN: i32 = -1;

/// The fields of an `LLViewerInventoryCategory` its cache keys cover: the
/// category, `mOwnerID` and `mVersion`. The viewer keeps the rest
/// (descendent counts, fetch state) itself.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ViewerCategory {
    /// The `LLInventoryCategory` fields.
    pub category: Category,
    /// `mOwnerID`.
    pub owner: Uuid,
    /// `mVersion`.
    pub version: i32,
}

impl Default for ViewerCategory {
    /// A default category, null owner, [`VERSION_UNKNOWN`].
    fn default() -> Self {
        ViewerCategory { category: Category::default(), owner: NULL_ID, version: VERSION_UNKNOWN }
    }
}
