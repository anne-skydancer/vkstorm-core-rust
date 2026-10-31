//! The `LLInventoryObject` record (`llinventory.{h,cpp}`): the fields shared by
//! items and categories.
//!
//! [`Object`] is the flat record of an `LLInventoryObject`. The item and
//! category records ([`crate::item::Item`], [`crate::category::Category`])
//! each hold one, as the C++ classes derive from it. Its legacy text form is
//! in [`crate::legacy::object`].

use crate::perms::{NULL_ID, Uuid};
use crate::text::{self, correct_inventory_name};
use crate::types::AssetType;

/// `LLInventoryObject` as a flat record.
///
/// The fields are public and set raw, as the viewer's subclasses set them;
/// the setters that do more than store ([`rename`](Self::rename),
/// [`set_type`](Self::set_type)) are methods. Names and runtimes are byte
/// strings (`std::string`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Object {
    /// `mUUID`.
    pub id: Uuid,
    /// `mParentUUID`.
    pub parent: Uuid,
    /// `mThumbnailUUID`.
    pub thumbnail: Uuid,
    /// `mType`: the asset type, any integer.
    pub asset_type: AssetType,
    /// `mFavorite`.
    pub favorite: bool,
    /// `mCreationDate` (`time_t`, 64-bit).
    pub creation_date: i64,
    /// `mRuntime`: the script runtime (`luau`), empty for none.
    pub runtime: Vec<u8>,
    /// `mName`.
    pub name: Vec<u8>,
}

impl Default for Object {
    /// `LLInventoryObject::LLInventoryObject()`: null ids, `AT_NONE`, date 0,
    /// not favorite, empty strings.
    fn default() -> Self {
        Object {
            id: NULL_ID,
            parent: NULL_ID,
            thumbnail: NULL_ID,
            asset_type: AssetType::NONE,
            favorite: false,
            creation_date: 0,
            runtime: Vec::new(),
            name: Vec::new(),
        }
    }
}

impl Object {
    /// `LLInventoryObject(uuid, parent_uuid, type, name)`: the fields given,
    /// the name through `correctInventoryName`; date 0, not favorite, null
    /// thumbnail, no runtime.
    pub fn new(id: &Uuid, parent: &Uuid, asset_type: AssetType, name: &[u8]) -> Object {
        Object {
            id: *id,
            parent: *parent,
            asset_type,
            name: correct_inventory_name(name),
            ..Object::default()
        }
    }

    /// `LLInventoryObject::copyObject`: the id, parent, type, name,
    /// thumbnail, favorite and runtime. The creation date is not copied.
    pub fn copy_object(&mut self, other: &Object) {
        self.id = other.id;
        self.parent = other.parent;
        self.asset_type = other.asset_type;
        self.name.clone_from(&other.name);
        self.thumbnail = other.thumbnail;
        self.favorite = other.favorite;
        self.runtime.clone_from(&other.runtime);
    }

    /// `getActualType`, and `LLInventoryObject::getType` (the viewer's
    /// subclasses resolve links in their override): the stored type.
    pub fn actual_type(&self) -> AssetType {
        self.asset_type
    }

    /// `getIsLinkType`: the type is `AT_LINK` or `AT_LINK_FOLDER`.
    pub fn is_link_type(&self) -> bool {
        self.asset_type.is_link_type()
    }

    /// `LLInventoryObject::getLinkedUUID`: the object's own id.
    pub fn linked_id(&self) -> Uuid {
        self.id
    }

    /// `rename(n)`: the name through `correctInventoryName`, kept when the
    /// result is empty or equal to the current name.
    pub fn rename(&mut self, new_name: &[u8]) {
        if let Some(n) = text::rename(&self.name, new_name) {
            self.name = n;
        }
    }

    /// `setType`: the type; a type other than `AT_LSL_TEXT` clears the
    /// runtime.
    pub fn set_type(&mut self, asset_type: AssetType) {
        self.asset_type = asset_type;
        if asset_type != AssetType::LSL_TEXT {
            self.runtime.clear();
        }
    }
}
