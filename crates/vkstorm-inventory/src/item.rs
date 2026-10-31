//! The `LLInventoryItem` record (`llinventory.{h,cpp}`): its fields, CRC,
//! subtype helpers and `accumulatePermissionSlamBits`.
//!
//! [`Item`] is the flat record of an `LLInventoryItem`: an [`Object`] (the
//! `LLInventoryObject` base) plus the item fields. Its LLSD forms are in
//! [`crate::llsd::item`], its legacy text in [`crate::legacy::item`], and
//! the asset id obfuscation both use in [`crate::shadow`].

use crate::flags::{
    II_FLAGS_OBJECT_PERM_OVERWRITE_EVERYONE, II_FLAGS_OBJECT_PERM_OVERWRITE_GROUP,
    II_FLAGS_OBJECT_SLAM_PERM, II_FLAGS_OBJECT_SLAM_SALE, II_FLAGS_PERM_OVERWRITE_MASK,
    II_FLAGS_SUBTYPE_MASK,
};
use crate::object::Object;
use crate::perms::{NULL_ID, Permissions, Uuid, uuid_crc32};
use crate::sale::SaleInfo;
use crate::text::{self, correct_inventory_description};
use crate::types::{AssetType, InventoryType};

/// `LLInventoryItem` as a flat record.
///
/// The fields are public and set raw, as the viewer's subclasses set them;
/// the setters that do more than store are methods. Names and descriptions
/// are byte strings (`std::string`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Item {
    /// The `LLInventoryObject` fields.
    pub object: Object,
    /// `mPermissions`.
    pub permissions: Permissions,
    /// `mAssetUUID`.
    pub asset: Uuid,
    /// `mDescription`.
    pub description: Vec<u8>,
    /// `mSaleInfo`.
    pub sale_info: SaleInfo,
    /// `mInventoryType`: any integer.
    pub inv_type: InventoryType,
    /// `mFlags`.
    pub flags: u32,
}

impl Default for Item {
    /// `LLInventoryItem::LLInventoryItem()`: the default object (`AT_NONE`),
    /// default permissions and sale info, null asset, `IT_NONE`, flags 0.
    fn default() -> Self {
        Item {
            object: Object::default(),
            permissions: Permissions::DEFAULT,
            asset: NULL_ID,
            description: Vec::new(),
            sale_info: SaleInfo::DEFAULT,
            inv_type: InventoryType::NONE,
            flags: 0,
        }
    }
}

impl Item {
    /// The full constructor `LLInventoryItem(uuid, parent_uuid, permissions,
    /// asset_uuid, type, inv_type, name, desc, sale_info, flags,
    /// creation_date_utc)`.
    ///
    /// The name goes through `correctInventoryName` (the object
    /// constructor); the description through `replaceNonstandardASCII` and
    /// `replaceChar('|', ' ')`, untrimmed and of any length; the sale info is
    /// stored as given; then `initMasks(inv_type)`. The date is an `S32`, as
    /// the C++ parameter.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: &Uuid,
        parent: &Uuid,
        permissions: &Permissions,
        asset: &Uuid,
        asset_type: AssetType,
        inv_type: InventoryType,
        name: &[u8],
        desc: &[u8],
        sale_info: &SaleInfo,
        flags: u32,
        creation_date: i32,
    ) -> Item {
        let mut object = Object::new(id, parent, asset_type, name);
        object.creation_date = i64::from(creation_date);
        let mut item = Item {
            object,
            permissions: *permissions,
            asset: *asset,
            description: correct_inventory_description(desc),
            sale_info: *sale_info,
            inv_type,
            flags,
        };
        item.permissions.init_masks_for_type(inv_type);
        item
    }

    /// `LLInventoryItem::copyItem`: every field (`copyObject`, which leaves
    /// out the date, then the item fields, the date included).
    pub fn copy_item(&mut self, other: &Item) {
        self.object.copy_object(&other.object);
        self.permissions = other.permissions;
        self.asset = other.asset;
        self.object.thumbnail = other.object.thumbnail;
        self.description.clone_from(&other.description);
        self.sale_info = other.sale_info;
        self.inv_type = other.inv_type;
        self.flags = other.flags;
        self.object.creation_date = other.object.creation_date;
        self.object.runtime.clone_from(&other.object.runtime);
    }

    /// The copy constructor `LLInventoryItem(const LLInventoryItem*)`: a
    /// default item, then [`copy_item`](Self::copy_item); every field equal.
    pub fn copy_from(other: &Item) -> Item {
        let mut item = Item::default();
        item.copy_item(other);
        item
    }

    /// `getType` / `getActualType`: the stored asset type.
    pub fn asset_type(&self) -> AssetType {
        self.object.asset_type
    }

    /// `getIsLinkType`.
    pub fn is_link_type(&self) -> bool {
        self.object.is_link_type()
    }

    /// `LLInventoryItem::getLinkedUUID`: the asset id of a link (`AT_LINK`,
    /// `AT_LINK_FOLDER`), else the item's own id.
    pub fn linked_id(&self) -> Uuid {
        if self.object.asset_type.is_link_type() {
            self.asset
        } else {
            self.object.linked_id()
        }
    }

    /// `getCreatorUUID`: the permissions' creator.
    pub fn creator(&self) -> Uuid {
        self.permissions.creator
    }

    /// `setCreator`: the permissions' creator, raw.
    pub fn set_creator(&mut self, creator: &Uuid) {
        self.permissions.creator = *creator;
    }

    /// `LLInventoryItem::getCRC32` (also `LLViewerInventoryItem::getCRC32`):
    /// the wrapping sum of the id, parent, permissions and asset CRCs, the
    /// type and inventory type (as `U32` of their signed values), the flags,
    /// the sale info CRC, the low 32 bits of the creation date and the
    /// thumbnail CRC. The name, description, favorite and runtime are not
    /// covered; the permissions CRC covers the last owner.
    pub fn crc32(&self) -> u32 {
        uuid_crc32(&self.object.id)
            .wrapping_add(uuid_crc32(&self.object.parent))
            .wrapping_add(self.permissions.crc32())
            .wrapping_add(uuid_crc32(&self.asset))
            .wrapping_add(self.object.asset_type.0 as u32)
            .wrapping_add(self.inv_type.0 as u32)
            .wrapping_add(self.flags)
            .wrapping_add(self.sale_info.crc32())
            .wrapping_add(self.object.creation_date as u32)
            .wrapping_add(uuid_crc32(&self.object.thumbnail))
    }

    /// `setDescription(d)`: the description through
    /// `correctInventoryDescription`; an empty result replaces it too.
    pub fn set_description(&mut self, desc: &[u8]) {
        if let Some(d) = text::set_description(&self.description, desc) {
            self.description = d;
        }
    }

    /// `rename(n)` (`LLInventoryObject::rename`).
    pub fn rename(&mut self, new_name: &[u8]) {
        self.object.rename(new_name);
    }

    /// `setPermissions`: the permissions, then `initMasks(mInventoryType)`
    /// (every mask `PERM_ALL` for calling cards and landmarks).
    pub fn set_permissions(&mut self, permissions: &Permissions) {
        self.permissions = *permissions;
        self.permissions.init_masks_for_type(self.inv_type);
    }

    /// `getInventorySubType`: the low byte of the flags.
    pub fn inventory_sub_type(&self) -> u8 {
        (self.flags & II_FLAGS_SUBTYPE_MASK) as u8
    }

    /// `setInventorySubType`: replaces the low byte of the flags.
    pub fn set_inventory_sub_type(&mut self, sub_type: u8) {
        self.flags =
            (self.flags & !II_FLAGS_SUBTYPE_MASK) | (u32::from(sub_type) & II_FLAGS_SUBTYPE_MASK);
    }

    /// `accumulatePermissionSlamBits(old_item)`.
    ///
    /// The flags become the old item's flags or'ed with this item's flags
    /// without the `II_FLAGS_PERM_OVERWRITE_MASK` bits. For an `AT_OBJECT`
    /// item, a next-owner, everyone or group mask that differs from the old
    /// item's sets `SLAM_PERM`, `PERM_OVERWRITE_EVERYONE` or
    /// `PERM_OVERWRITE_GROUP`, and differing sale info sets `SLAM_SALE`.
    pub fn accumulate_permission_slam_bits(&mut self, old_item: &Item) {
        self.flags = old_item.flags | (self.flags & !II_FLAGS_PERM_OVERWRITE_MASK);
        if self.object.asset_type == AssetType::OBJECT {
            let (old, new) = (&old_item.permissions, &self.permissions);
            let mut set = 0;
            if old.next != new.next {
                set |= II_FLAGS_OBJECT_SLAM_PERM;
            }
            if old.everyone != new.everyone {
                set |= II_FLAGS_OBJECT_PERM_OVERWRITE_EVERYONE;
            }
            if old.group_mask != new.group_mask {
                set |= II_FLAGS_OBJECT_PERM_OVERWRITE_GROUP;
            }
            if old_item.sale_info != self.sale_info {
                set |= II_FLAGS_OBJECT_SLAM_SALE;
            }
            self.flags |= set;
        }
    }

    /// The `inventory_and_asset_types_match` repair the readers end with
    /// (`importLegacyStream`, `fromLLSD`): an `IT_NONE` inventory type, or
    /// one that does not match the asset type, becomes the asset type's
    /// default; then `initMasks(inv_type)`.
    pub(crate) fn repair_inventory_type(&mut self) {
        if self.inv_type == InventoryType::NONE
            || !crate::types::inventory_and_asset_types_match(self.inv_type, self.object.asset_type)
        {
            self.inv_type = InventoryType::default_for_asset_type(self.object.asset_type);
        }
        self.permissions.init_masks_for_type(self.inv_type);
    }
}
