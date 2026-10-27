//! Inventory records in UDP message blocks, over the message builder and
//! reader of `vkstorm-messages`: `LLInventoryItem::packMessage`,
//! `LLViewerInventoryItem::packMessage` and `LLInventoryItem::unpackMessage`
//! (`llinventory.cpp`, `llviewerinventory.cpp`); the folder blocks of
//! `LLInventoryCategory` and `LLViewerInventoryCategory`; and the
//! `ObjectProperties` fields of `LLAggregatePermissions`, `LLCategory` and
//! `LLSaleInfo` (`llpermissions.cpp`, `llcategory.cpp`, `llsaleinfo.cpp`).
//!
//! An item travels as one repeat of a block with the `ItemData` layout
//! (`BulkUpdateInventory.ItemData`, `UpdateInventoryItem.InventoryData`,
//! `UpdateTaskInventory.InventoryData`, ...). [`ItemBlock`] is that block's
//! wire record; [`ItemBlockKeys`] holds the caller's keys of its variables
//! (the viewer passes its canonical string pointers, tests take them from a
//! parsed template with [`ItemBlockKeys::from_block`]).
//!
//! The caller starts the message and the block repeat (`newMessage`,
//! `nextBlock`) and adds the variables that are not the item's (such as
//! `CallbackID`), as the viewer's senders do; these functions add the item's
//! variables to the current repeat, or read them from a decoded message.
//!
//! Rules kept from the C++:
//! - the types travel as `S8` and the sale type as `U8`, so they are cut to
//!   their low byte; the creation date travels as `S32` (its low 32 bits);
//! - strings are added with a terminating NUL, and an empty string as no
//!   bytes at all (`addString(const std::string&)`); the builder cuts a
//!   string to 255 bytes with a NUL last;
//! - the block's CRC is the item's [`Item::crc32`], which covers the last
//!   owner and the thumbnail although neither travels in the block;
//! - unpacking reads a string up to its first NUL (within `MTUBYTES`) and
//!   passes it through `replaceNonstandardASCII(' ')`; it reads the type as
//!   `S8` (sign extended) and the sale type as `U8`, runs
//!   `initMasks(inv_type)` after the permissions, clamps the price to
//!   `0..=S32_MAX`, and leaves the last owner, thumbnail, favorite and
//!   runtime as they were. Its verdict is the local CRC against the block's.

use std::fmt;

use vkstorm_messages::{Block, Builder, BuilderError, GetError, MTUBYTES, Reader, VarType};

use crate::aggregate::AggregatePermissions;
use crate::category::{Category, ViewerCategory};
use crate::category_obj::Category as LLCategory;
use crate::item::Item;
use crate::perms::{PermissionMask, Uuid};
use crate::sale::{clamp_price, SaleInfo, SaleType};
use crate::text::replace_nonstandard_ascii;
use crate::types::{AssetType, FolderType, InventoryType};

/// The keys of an item block's variables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemBlockKeys {
    pub item_id: u64,
    pub folder_id: u64,
    pub creator_id: u64,
    pub owner_id: u64,
    pub group_id: u64,
    pub base_mask: u64,
    pub owner_mask: u64,
    pub group_mask: u64,
    pub everyone_mask: u64,
    pub next_owner_mask: u64,
    pub group_owned: u64,
    /// The id slot after the permissions: `AssetID` (the base
    /// `packMessage`, and every `unpackMessage`) or `TransactionID` (the
    /// viewer subclass's `packMessage`).
    pub asset: u64,
    pub ty: u64,
    pub inv_type: u64,
    pub flags: u64,
    pub sale_type: u64,
    pub sale_price: u64,
    pub name: u64,
    pub description: u64,
    pub creation_date: u64,
    pub crc: u64,
}

impl ItemBlockKeys {
    /// The keys of a template block by variable name. The id slot is
    /// `AssetID` when the block has one, else `TransactionID`. `None` when a
    /// variable is missing.
    pub fn from_block(block: &Block) -> Option<ItemBlockKeys> {
        let k = |name: &str| block.variable_by_name(name).map(|v| v.key);
        Some(ItemBlockKeys {
            item_id: k("ItemID")?,
            folder_id: k("FolderID")?,
            creator_id: k("CreatorID")?,
            owner_id: k("OwnerID")?,
            group_id: k("GroupID")?,
            base_mask: k("BaseMask")?,
            owner_mask: k("OwnerMask")?,
            group_mask: k("GroupMask")?,
            everyone_mask: k("EveryoneMask")?,
            next_owner_mask: k("NextOwnerMask")?,
            group_owned: k("GroupOwned")?,
            asset: k("AssetID").or_else(|| k("TransactionID"))?,
            ty: k("Type")?,
            inv_type: k("InvType")?,
            flags: k("Flags")?,
            sale_type: k("SaleType")?,
            sale_price: k("SalePrice")?,
            name: k("Name")?,
            description: k("Description")?,
            creation_date: k("CreationDate")?,
            crc: k("CRC")?,
        })
    }
}

/// An item block as it travels: every value already in its wire type.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ItemBlock {
    pub item_id: Uuid,
    pub folder_id: Uuid,
    pub creator_id: Uuid,
    pub owner_id: Uuid,
    pub group_id: Uuid,
    pub base_mask: PermissionMask,
    pub owner_mask: PermissionMask,
    pub group_mask: PermissionMask,
    pub everyone_mask: PermissionMask,
    pub next_owner_mask: PermissionMask,
    pub group_owned: bool,
    /// `AssetID` or `TransactionID` (see [`ItemBlockKeys::asset`]).
    pub asset: Uuid,
    pub ty: i8,
    pub inv_type: i8,
    pub flags: u32,
    pub sale_type: u8,
    pub sale_price: i32,
    /// Without the terminating NUL, which packing adds.
    pub name: Vec<u8>,
    pub description: Vec<u8>,
    pub creation_date: i32,
    pub crc: u32,
}

impl ItemBlock {
    /// `LLInventoryItem::packMessage`'s values: the asset id in the id slot.
    pub fn from_item(item: &Item) -> ItemBlock {
        Self::with_id_slot(item, &item.asset)
    }

    /// `LLViewerInventoryItem::packMessage`'s values: the item's transaction
    /// id in the id slot in place of the asset id (the CRC is the same).
    pub fn from_viewer_item(item: &Item, transaction_id: &Uuid) -> ItemBlock {
        Self::with_id_slot(item, transaction_id)
    }

    fn with_id_slot(item: &Item, slot: &Uuid) -> ItemBlock {
        let p = &item.permissions;
        ItemBlock {
            item_id: item.object.id,
            folder_id: item.object.parent,
            creator_id: p.creator,
            owner_id: p.owner,
            group_id: p.group,
            base_mask: p.base,
            owner_mask: p.owner_mask,
            group_mask: p.group_mask,
            everyone_mask: p.everyone,
            next_owner_mask: p.next,
            group_owned: p.group_owned,
            asset: *slot,
            ty: item.object.asset_type.0 as i8,
            inv_type: item.inv_type.0 as i8,
            flags: item.flags,
            sale_type: item.sale_info.sale_type.0 as u8,
            sale_price: item.sale_info.price,
            name: item.object.name.clone(),
            description: item.description.clone(),
            creation_date: item.object.creation_date as i32,
            crc: item.crc32(),
        }
    }
}

/// A failure of packing or unpacking an item block: the message system's
/// error (the C++ `LL_ERRS` cases).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageError {
    Builder(BuilderError),
    Get(GetError),
}

impl fmt::Display for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MessageError::Builder(e) => write!(f, "{e}"),
            MessageError::Get(e) => write!(f, "{e:?}"),
        }
    }
}

impl std::error::Error for MessageError {}

impl From<BuilderError> for MessageError {
    fn from(e: BuilderError) -> Self {
        MessageError::Builder(e)
    }
}

impl From<GetError> for MessageError {
    fn from(e: GetError) -> Self {
        MessageError::Get(e)
    }
}

/// `addStringFast(var, const std::string&)`: the bytes and a NUL, or no
/// bytes for an empty string.
pub fn add_string(builder: &mut Builder, key: u64, s: &[u8]) -> Result<(), BuilderError> {
    if s.is_empty() {
        builder.add_data(key, &[], VarType::Variable)?;
    } else {
        let mut data = Vec::with_capacity(s.len() + 1);
        data.extend_from_slice(s);
        data.push(0);
        builder.add_data(key, &data, VarType::Variable)?;
    }
    Ok(())
}

/// `getStringFast(block, var, std::string&, blocknum)`: the field's bytes
/// (at most `MTUBYTES`) up to the first NUL.
pub fn get_string(reader: &Reader, block: u64, block_num: i32, key: u64) -> Result<Vec<u8>, GetError> {
    let data = reader.get(block, block_num, key, 0)?;
    let data = &data[..data.len().min(MTUBYTES as usize)];
    let end = data.iter().position(|&c| c == 0).unwrap_or(data.len());
    Ok(data[..end].to_vec())
}

/// Adds the block's variables to the builder's current block repeat, in
/// the C++ order (the wire order is the template's).
pub fn pack_item_block(builder: &mut Builder, keys: &ItemBlockKeys, b: &ItemBlock) -> Result<(), BuilderError> {
    builder.add_data(keys.item_id, &b.item_id, VarType::LLUUID)?;
    builder.add_data(keys.folder_id, &b.folder_id, VarType::LLUUID)?;
    builder.add_data(keys.creator_id, &b.creator_id, VarType::LLUUID)?;
    builder.add_data(keys.owner_id, &b.owner_id, VarType::LLUUID)?;
    builder.add_data(keys.group_id, &b.group_id, VarType::LLUUID)?;
    builder.add_data(keys.base_mask, &b.base_mask.to_le_bytes(), VarType::U32)?;
    builder.add_data(keys.owner_mask, &b.owner_mask.to_le_bytes(), VarType::U32)?;
    builder.add_data(keys.group_mask, &b.group_mask.to_le_bytes(), VarType::U32)?;
    builder.add_data(keys.everyone_mask, &b.everyone_mask.to_le_bytes(), VarType::U32)?;
    builder.add_data(keys.next_owner_mask, &b.next_owner_mask.to_le_bytes(), VarType::U32)?;
    builder.add_data(keys.group_owned, &[u8::from(b.group_owned)], VarType::Bool)?;
    builder.add_data(keys.asset, &b.asset, VarType::LLUUID)?;
    builder.add_data(keys.ty, &b.ty.to_le_bytes(), VarType::S8)?;
    builder.add_data(keys.inv_type, &b.inv_type.to_le_bytes(), VarType::S8)?;
    builder.add_data(keys.flags, &b.flags.to_le_bytes(), VarType::U32)?;
    builder.add_data(keys.sale_type, &[b.sale_type], VarType::U8)?;
    builder.add_data(keys.sale_price, &b.sale_price.to_le_bytes(), VarType::S32)?;
    add_string(builder, keys.name, &b.name)?;
    add_string(builder, keys.description, &b.description)?;
    builder.add_data(keys.creation_date, &b.creation_date.to_le_bytes(), VarType::S32)?;
    builder.add_data(keys.crc, &b.crc.to_le_bytes(), VarType::U32)?;
    Ok(())
}

/// `LLInventoryItem::packMessage`: the item, its asset id in `keys.asset`.
pub fn pack_item(builder: &mut Builder, keys: &ItemBlockKeys, item: &Item) -> Result<(), BuilderError> {
    pack_item_block(builder, keys, &ItemBlock::from_item(item))
}

/// `LLViewerInventoryItem::packMessage`: the item, its transaction id in
/// `keys.asset` (the `TransactionID` key).
pub fn pack_viewer_item(
    builder: &mut Builder,
    keys: &ItemBlockKeys,
    item: &Item,
    transaction_id: &Uuid,
) -> Result<(), BuilderError> {
    pack_item_block(builder, keys, &ItemBlock::from_viewer_item(item, transaction_id))
}

fn get_fixed<const N: usize>(reader: &Reader, block: u64, num: i32, key: u64) -> Result<[u8; N], GetError> {
    let data = reader.get(block, num, key, N as i32)?;
    let mut out = [0u8; N];
    out.copy_from_slice(data);
    Ok(out)
}

/// Reads repeat `block_num` of block `block` as an [`ItemBlock`], the
/// strings as `getStringFast` returns them (unsanitised).
pub fn read_item_block(reader: &Reader, block: u64, block_num: i32, keys: &ItemBlockKeys) -> Result<ItemBlock, GetError> {
    let id = |key| get_fixed::<16>(reader, block, block_num, key);
    let u32_ = |key| get_fixed::<4>(reader, block, block_num, key).map(u32::from_le_bytes);
    let u8_ = |key| get_fixed::<1>(reader, block, block_num, key).map(|b| b[0]);
    Ok(ItemBlock {
        item_id: id(keys.item_id)?,
        folder_id: id(keys.folder_id)?,
        creator_id: id(keys.creator_id)?,
        owner_id: id(keys.owner_id)?,
        group_id: id(keys.group_id)?,
        base_mask: u32_(keys.base_mask)?,
        owner_mask: u32_(keys.owner_mask)?,
        group_mask: u32_(keys.group_mask)?,
        everyone_mask: u32_(keys.everyone_mask)?,
        next_owner_mask: u32_(keys.next_owner_mask)?,
        group_owned: u8_(keys.group_owned)? != 0,
        asset: id(keys.asset)?,
        ty: u8_(keys.ty)? as i8,
        inv_type: u8_(keys.inv_type)? as i8,
        flags: u32_(keys.flags)?,
        sale_type: u8_(keys.sale_type)?,
        sale_price: u32_(keys.sale_price)? as i32,
        name: get_string(reader, block, block_num, keys.name)?,
        description: get_string(reader, block, block_num, keys.description)?,
        creation_date: u32_(keys.creation_date)? as i32,
        crc: u32_(keys.crc)?,
    })
}

/// `LLInventoryItem::unpackMessage(msg, block, block_num)` into `item`:
/// returns whether the item's CRC after the read equals the block's. The
/// last owner, thumbnail, favorite and runtime keep their values. On an
/// error `item` is unchanged.
pub fn unpack_item(
    reader: &Reader,
    block: u64,
    block_num: i32,
    keys: &ItemBlockKeys,
    item: &mut Item,
) -> Result<bool, GetError> {
    let b = read_item_block(reader, block, block_num, keys)?;
    Ok(apply_item_block(&b, item))
}

/// The assignments of `unpackMessage` from a read block; returns the CRC
/// verdict.
pub fn apply_item_block(b: &ItemBlock, item: &mut Item) -> bool {
    item.object.id = b.item_id;
    item.object.parent = b.folder_id;
    let p = &mut item.permissions;
    p.creator = b.creator_id;
    p.owner = b.owner_id;
    p.group = b.group_id;
    p.base = b.base_mask;
    p.owner_mask = b.owner_mask;
    p.group_mask = b.group_mask;
    p.everyone = b.everyone_mask;
    p.next = b.next_owner_mask;
    p.group_owned = b.group_owned;
    item.asset = b.asset;
    item.object.asset_type = AssetType(i32::from(b.ty));
    item.inv_type = InventoryType(i32::from(b.inv_type));
    item.permissions.init_masks_for_type(item.inv_type);
    item.flags = b.flags;
    item.sale_info.sale_type = SaleType(i32::from(b.sale_type));
    item.sale_info.price = clamp_price(b.sale_price);
    item.object.name.clone_from(&b.name);
    replace_nonstandard_ascii(&mut item.object.name, b' ');
    item.description.clone_from(&b.description);
    replace_nonstandard_ascii(&mut item.description, b' ');
    item.object.creation_date = i64::from(b.creation_date);
    item.crc32() == b.crc
}

// ---------------------------------------------------------------------------
// Folder blocks: LLInventoryCategory::packMessage / unpackMessage
// ---------------------------------------------------------------------------

/// The keys of a folder block's variables (`FolderData` of
/// `UpdateInventoryFolder`, `BulkUpdateInventory`, ...).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FolderBlockKeys {
    pub folder_id: u64,
    pub parent_id: u64,
    pub ty: u64,
    pub name: u64,
}

impl FolderBlockKeys {
    /// The keys of a template block by variable name; `None` when a
    /// variable is missing.
    pub fn from_block(block: &Block) -> Option<FolderBlockKeys> {
        let k = |name: &str| block.variable_by_name(name).map(|v| v.key);
        Some(FolderBlockKeys { folder_id: k("FolderID")?, parent_id: k("ParentID")?, ty: k("Type")?, name: k("Name")? })
    }
}

/// `LLInventoryCategory::packMessage` (and `LLViewerInventoryCategory`'s,
/// which is the same): the id, parent, preferred type as `S8` and name.
pub fn pack_category(builder: &mut Builder, keys: &FolderBlockKeys, category: &Category) -> Result<(), BuilderError> {
    let o = &category.object;
    builder.add_data(keys.folder_id, &o.id, VarType::LLUUID)?;
    builder.add_data(keys.parent_id, &o.parent, VarType::LLUUID)?;
    builder.add_data(keys.ty, &(category.preferred_type.0 as i8).to_le_bytes(), VarType::S8)?;
    add_string(builder, keys.name, &o.name)
}

/// `LLViewerInventoryCategory::packMessage`: [`pack_category`] of its
/// category (the owner and version do not travel).
pub fn pack_viewer_category(
    builder: &mut Builder,
    keys: &FolderBlockKeys,
    category: &ViewerCategory,
) -> Result<(), BuilderError> {
    pack_category(builder, keys, &category.category)
}

/// `LLInventoryCategory::unpackMessage(msg, block, block_num)`: the id,
/// parent, preferred type (`S8`, sign extended) and name (through
/// `replaceNonstandardASCII(' ')` only: not trimmed, `|` kept). The type,
/// thumbnail, favorite, date and runtime keep their values. On an error
/// `category` is unchanged.
pub fn unpack_category(
    reader: &Reader,
    block: u64,
    block_num: i32,
    keys: &FolderBlockKeys,
    category: &mut Category,
) -> Result<(), GetError> {
    let id = get_fixed::<16>(reader, block, block_num, keys.folder_id)?;
    let parent = get_fixed::<16>(reader, block, block_num, keys.parent_id)?;
    let ty = get_fixed::<1>(reader, block, block_num, keys.ty)?[0] as i8;
    let mut name = get_string(reader, block, block_num, keys.name)?;
    replace_nonstandard_ascii(&mut name, b' ');
    category.object.id = id;
    category.object.parent = parent;
    category.preferred_type = FolderType(i32::from(ty));
    category.object.name = name;
    Ok(())
}

// ---------------------------------------------------------------------------
// ObjectProperties fields: aggregate permissions, LLCategory, sale info
// ---------------------------------------------------------------------------

/// `LLAggregatePermissions::packMessage(msg, field)`: `getU8` as a `U8`.
pub fn pack_aggregate(builder: &mut Builder, key: u64, agg: &AggregatePermissions) -> Result<(), BuilderError> {
    builder.add_data(key, &[agg.to_u8()], VarType::U8)?;
    Ok(())
}

/// `LLAggregatePermissions::unpackMessage(msg, block, field, block_num)`:
/// two bits each for copy, modify and transfer; the top two bits dropped.
pub fn unpack_aggregate(reader: &Reader, block: u64, block_num: i32, key: u64) -> Result<AggregatePermissions, GetError> {
    Ok(AggregatePermissions::from_u8(get_fixed::<1>(reader, block, block_num, key)?[0]))
}

/// `LLCategory::packMessage`: `Category` as `getU32`.
pub fn pack_llcategory(builder: &mut Builder, key: u64, category: LLCategory) -> Result<(), BuilderError> {
    builder.add_data(key, &category.as_u32().to_le_bytes(), VarType::U32)?;
    Ok(())
}

/// `LLCategory::unpackMessage` (repeat 0) and `unpackMultiMessage`: `init`
/// of the `U32`.
pub fn unpack_llcategory(reader: &Reader, block: u64, block_num: i32, key: u64) -> Result<LLCategory, GetError> {
    Ok(LLCategory::new(u32::from_le_bytes(get_fixed::<4>(reader, block, block_num, key)?)))
}

/// The keys of the `SaleType` and `SalePrice` variables of a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaleKeys {
    pub sale_type: u64,
    pub sale_price: u64,
}

impl SaleKeys {
    pub fn from_block(block: &Block) -> Option<SaleKeys> {
        let k = |name: &str| block.variable_by_name(name).map(|v| v.key);
        Some(SaleKeys { sale_type: k("SaleType")?, sale_price: k("SalePrice")? })
    }
}

/// `LLSaleInfo::packMessage(msg)`: the sale type's low byte as `U8` and the
/// price as `S32`, unclamped.
pub fn pack_sale(builder: &mut Builder, keys: &SaleKeys, sale: &SaleInfo) -> Result<(), BuilderError> {
    builder.add_data(keys.sale_type, &[sale.sale_type.0 as u8], VarType::U8)?;
    builder.add_data(keys.sale_price, &sale.price.to_le_bytes(), VarType::S32)?;
    Ok(())
}

/// `LLSaleInfo::unpackMessage(msg, block)` (repeat 0) and
/// `unpackMultiMessage(msg, block, block_num)`: the sale type as `U8`
/// (0..=255), the price clamped to `0..=S32_MAX`. On an error `sale` is
/// unchanged.
pub fn unpack_sale(reader: &Reader, block: u64, block_num: i32, keys: &SaleKeys, sale: &mut SaleInfo) -> Result<(), GetError> {
    let ty = get_fixed::<1>(reader, block, block_num, keys.sale_type)?[0];
    let price = i32::from_le_bytes(get_fixed::<4>(reader, block, block_num, keys.sale_price)?);
    sale.sale_type = SaleType(i32::from(ty));
    sale.price = clamp_price(price);
    Ok(())
}
