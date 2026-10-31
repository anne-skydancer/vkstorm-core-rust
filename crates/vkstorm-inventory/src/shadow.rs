//! Asset id obfuscation (`shadow_id`): the `MAGIC_ID` XOR of `llinventory.cpp`,
//! as `LLXORCipher` (`llmessage/llxorcipher.cpp`) applies it.
//!
//! An item whose base mask lacks any of modify, copy and transfer, and whose
//! asset id is not null, is written with its asset id XORed with
//! [`MAGIC_ID`] under the `shadow_id` key, in its legacy text
//! (`exportLegacyStream` with the asset key) and its LLSD (`asLLSD`). The
//! readers XOR a `shadow_id` back. Each call creates a fresh `LLXORCipher`
//! over the 16 key bytes and runs it over exactly 16 bytes, so the cipher's
//! running pad position never carries over: the transform is a plain
//! byte-wise XOR and its own inverse.

use crate::perms::{NULL_ID, PERM_ITEM_UNRESTRICTED, PermissionMask, Uuid};

/// `MAGIC_ID` (`llinventory.cpp`): `3c115e51-04f4-523c-9fa6-98aff1034730`.
pub const MAGIC_ID: Uuid = [
    0x3c, 0x11, 0x5e, 0x51, 0x04, 0xf4, 0x52, 0x3c, 0x9f, 0xa6, 0x98, 0xaf, 0xf1, 0x03, 0x47, 0x30,
];

/// `LLXORCipher(MAGIC_ID.mData, UUID_BYTES).encrypt(id.mData, UUID_BYTES)`:
/// the shadow id of an asset id.
pub fn shadow_id(asset: &Uuid) -> Uuid {
    let mut out = *asset;
    for (b, k) in out.iter_mut().zip(MAGIC_ID) {
        *b ^= k;
    }
    out
}

/// `LLXORCipher(MAGIC_ID.mData, UUID_BYTES).decrypt(id.mData, UUID_BYTES)`:
/// the asset id of a shadow id. The XOR is its own inverse, so this equals
/// [`shadow_id`].
pub fn unshadow_id(shadow: &Uuid) -> Uuid {
    shadow_id(shadow)
}

/// How an item's asset id is written by `exportLegacyStream` (with the asset
/// key) and `asLLSD`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExportedAssetId {
    /// `asset_id`: the asset id as it is.
    Asset(Uuid),
    /// `shadow_id`: the obfuscated asset id.
    Shadow(Uuid),
}

impl ExportedAssetId {
    /// The key the id is written under: `asset_id` or `shadow_id`.
    pub fn label(&self) -> &'static str {
        match self {
            ExportedAssetId::Asset(_) => "asset_id",
            ExportedAssetId::Shadow(_) => "shadow_id",
        }
    }

    /// The id written.
    pub fn id(&self) -> &Uuid {
        match self {
            ExportedAssetId::Asset(id) | ExportedAssetId::Shadow(id) => id,
        }
    }
}

/// The writers' choice ("check for permissions to see the asset id"): the
/// asset id itself when the base mask holds all of `PERM_ITEM_UNRESTRICTED`
/// (modify, copy and transfer) or the id is null, else its shadow id.
pub fn exported_asset_id(base_mask: PermissionMask, asset: &Uuid) -> ExportedAssetId {
    if (base_mask & PERM_ITEM_UNRESTRICTED) == PERM_ITEM_UNRESTRICTED || *asset == NULL_ID {
        ExportedAssetId::Asset(*asset)
    } else {
        ExportedAssetId::Shadow(shadow_id(asset))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xor_round_trip() {
        assert_eq!(shadow_id(&MAGIC_ID), NULL_ID);
        assert_eq!(shadow_id(&NULL_ID), MAGIC_ID);
        assert_eq!(shadow_id(&MAGIC_ID.map(|b| !b)), [0xff; 16]);
        let a = [7u8; 16];
        assert_eq!(unshadow_id(&shadow_id(&a)), a);
    }

    #[test]
    fn choice() {
        let a = [7u8; 16];
        assert_eq!(
            exported_asset_id(0x7fff_ffff, &a),
            ExportedAssetId::Asset(a)
        );
        assert_eq!(
            exported_asset_id(0x0000_e000, &a),
            ExportedAssetId::Asset(a)
        );
        assert_eq!(
            exported_asset_id(0x7fff_bfff, &a),
            ExportedAssetId::Shadow(shadow_id(&a))
        );
        assert_eq!(
            exported_asset_id(0, &NULL_ID),
            ExportedAssetId::Asset(NULL_ID)
        );
    }
}
