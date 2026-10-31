//! `LLPermissions` rules (`llpermissions.{h,cpp}`): fixing, masks, accumulation,
//! ownership changes, the CRC and `mask_to_string`.
//!
//! [`Permissions`] is the flat record of an `LLPermissions`: four ids, five
//! masks and the group-owned flag. Its LLSD forms live in
//! [`crate::llsd::perms`] and its legacy text in [`crate::legacy::perms`].

use crate::types::InventoryType;

/// A UUID as its 16 bytes (`LLUUID::mData`).
pub type Uuid = [u8; 16];

/// `LLUUID::null`.
pub const NULL_ID: Uuid = [0; 16];

/// `LLUUID::getCRC32`: the wrapping sum of the four little-endian 32-bit
/// words.
pub fn uuid_crc32(id: &Uuid) -> u32 {
    id.chunks_exact(4)
        .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
        .fold(0u32, u32::wrapping_add)
}

fn is_null(id: &Uuid) -> bool {
    *id == NULL_ID
}

// ---------------------------------------------------------------------------
// llpermissionsflags.h
// ---------------------------------------------------------------------------

/// `PermissionMask`.
pub type PermissionMask = u32;
/// `PermissionBit`.
pub type PermissionBit = u32;

/// Transfer ownership (fair use: without copy, transfer is always allowed).
pub const PERM_TRANSFER: PermissionBit = 1 << 13;
/// Modify.
pub const PERM_MODIFY: PermissionBit = 1 << 14;
/// Copy.
pub const PERM_COPY: PermissionBit = 1 << 15;
/// OpenSim export (Firestorm).
pub const PERM_EXPORT: PermissionBit = 1 << 16;
/// Grab, translate and rotate objects.
pub const PERM_MOVE: PermissionBit = 1 << 19;
/// Bit 31, never used: `%x` reads assume signed numbers.
pub const PERM_RESERVED: PermissionBit = 1 << 31;

pub const PERM_NONE: PermissionMask = 0x0000_0000;
pub const PERM_ALL: PermissionMask = 0x7FFF_FFFF;
pub const PERM_ITEM_UNRESTRICTED: PermissionMask = PERM_MODIFY | PERM_COPY | PERM_TRANSFER;

/// Which permission mask a change targets (`PERM_BASE` ... `PERM_NEXT_OWNER`).
pub const PERM_BASE: u8 = 0x01;
pub const PERM_OWNER: u8 = 0x02;
pub const PERM_GROUP: u8 = 0x04;
pub const PERM_EVERYONE: u8 = 0x08;
pub const PERM_NEXT_OWNER: u8 = 0x10;

// ---------------------------------------------------------------------------
// mask_to_string
// ---------------------------------------------------------------------------

/// `mask_to_string(U32, bool isOpenSim)`: the debugging form of a mask, one
/// column each for move `V`, modify `M`, copy `C` and transfer `T`, a space
/// where the bit is clear.
///
/// With the `opensim` feature (the viewer's `OPENSIM` build), `is_open_sim`
/// adds a fifth column for export `X`. Without it the argument is ignored and
/// the form is always 4 characters, as in the Release viewer.
pub fn mask_to_string(mask: PermissionMask, is_open_sim: bool) -> String {
    let column = |bit: PermissionBit, c: char| if mask & bit != 0 { c } else { ' ' };
    let mut s = String::with_capacity(5);
    s.push(column(PERM_MOVE, 'V'));
    s.push(column(PERM_MODIFY, 'M'));
    s.push(column(PERM_COPY, 'C'));
    s.push(column(PERM_TRANSFER, 'T'));
    #[cfg(feature = "opensim")]
    if is_open_sim {
        s.push(column(PERM_EXPORT, 'X'));
    }
    #[cfg(not(feature = "opensim"))]
    let _ = is_open_sim;
    s
}

// ---------------------------------------------------------------------------
// LLPermissions
// ---------------------------------------------------------------------------

/// `LLPermissions` as a flat record.
///
/// The fields are public and set raw, as the viewer's `setMask*` and
/// `yesReallySetOwner` set them: nothing is fixed until a rule runs. The
/// rules are the C++ member functions of the same names.
///
/// The layout is the C++ object's: its data members in their declaration
/// order (`mCreator`, `mOwner`, `mLastOwner`, `mGroup`, `mMaskBase`,
/// `mMaskOwner`, `mMaskEveryone`, `mMaskGroup`, `mMaskNextOwner`,
/// `mIsGroupOwned`), 88 bytes with alignment 4. `LLPermissions` has no
/// virtual functions and no base class, so the C ABI (`vkstorm-ffi`) reads and
/// writes the viewer's objects in place. Keep the field order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct Permissions {
    /// `mCreator`.
    pub creator: Uuid,
    /// `mOwner`.
    pub owner: Uuid,
    /// `mLastOwner`.
    pub last_owner: Uuid,
    /// `mGroup`.
    pub group: Uuid,
    /// `mMaskBase`.
    pub base: PermissionMask,
    /// `mMaskOwner`.
    pub owner_mask: PermissionMask,
    /// `mMaskEveryone`.
    pub everyone: PermissionMask,
    /// `mMaskGroup`.
    pub group_mask: PermissionMask,
    /// `mMaskNextOwner`.
    pub next: PermissionMask,
    /// `mIsGroupOwned`.
    pub group_owned: bool,
}

// The C++ layout of `LLPermissions` (see `Permissions`).
const _: () = {
    use std::mem::{align_of, offset_of, size_of};
    assert!(size_of::<Permissions>() == 88 && align_of::<Permissions>() == 4);
    assert!(offset_of!(Permissions, creator) == 0);
    assert!(offset_of!(Permissions, owner) == 16);
    assert!(offset_of!(Permissions, last_owner) == 32);
    assert!(offset_of!(Permissions, group) == 48);
    assert!(offset_of!(Permissions, base) == 64);
    assert!(offset_of!(Permissions, owner_mask) == 68);
    assert!(offset_of!(Permissions, everyone) == 72);
    assert!(offset_of!(Permissions, group_mask) == 76);
    assert!(offset_of!(Permissions, next) == 80);
    assert!(offset_of!(Permissions, group_owned) == 84);
};

impl Default for Permissions {
    /// `LLPermissions::LLPermissions()`: [`Permissions::DEFAULT`].
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Permissions {
    /// `LLPermissions::DEFAULT` and the default constructor
    /// (`init(null, null, null, null)`): null ids, every mask `PERM_ALL`,
    /// not group owned.
    pub const DEFAULT: Permissions = Permissions {
        creator: NULL_ID,
        owner: NULL_ID,
        last_owner: NULL_ID,
        group: NULL_ID,
        base: PERM_ALL,
        owner_mask: PERM_ALL,
        group_mask: PERM_ALL,
        everyone: PERM_ALL,
        next: PERM_ALL,
        group_owned: false,
    };

    /// `LLPermissions::init`: the ids, every mask `PERM_ALL`, then
    /// `fixOwnership()`.
    pub fn init(&mut self, creator: &Uuid, owner: &Uuid, last_owner: &Uuid, group: &Uuid) {
        *self = Permissions {
            creator: *creator,
            owner: *owner,
            last_owner: *last_owner,
            group: *group,
            ..Self::DEFAULT
        };
        self.fix_ownership();
    }

    /// `LLPermissions::initMasks(base, owner, everyone, group, next)`: the
    /// masks, then `fixFairUse()` and `fix()`. The argument order is the
    /// C++ one (everyone before group).
    pub fn init_masks(
        &mut self,
        base: PermissionMask,
        owner: PermissionMask,
        everyone: PermissionMask,
        group: PermissionMask,
        next: PermissionMask,
    ) {
        self.base = base;
        self.owner_mask = owner;
        self.everyone = everyone;
        self.group_mask = group;
        self.next = next;
        self.fix_fair_use();
        self.fix();
    }

    /// `LLPermissions::initMasks(LLInventoryType::EType)`: every mask
    /// `PERM_ALL` for the types that cannot restrict permissions (calling
    /// cards and landmarks); nothing otherwise.
    pub fn init_masks_for_type(&mut self, inv_type: InventoryType) {
        if inv_type.cannot_restrict_permissions() {
            self.init_masks(PERM_ALL, PERM_ALL, PERM_ALL, PERM_ALL, PERM_ALL);
        }
    }

    /// `LLPermissions::getOwnership`: the owner and `false` when the owner is
    /// set, else the group and `true` when group owned, else `None` (where
    /// the C++ returns false and leaves its out arguments unchanged).
    pub fn ownership(&self) -> Option<(Uuid, bool)> {
        if !is_null(&self.owner) {
            Some((self.owner, false))
        } else if self.group_owned {
            Some((self.group, true))
        } else {
            None
        }
    }

    /// `LLPermissions::getSafeOwner` of an owned record: the owner, else the
    /// group when group owned. `None` where the C++ logs a warning and
    /// returns a freshly generated random UUID.
    pub fn safe_owner(&self) -> Option<Uuid> {
        self.ownership().map(|(id, _)| id)
    }

    /// `LLPermissions::isOwned`.
    pub fn is_owned(&self) -> bool {
        !is_null(&self.owner) || self.group_owned
    }

    /// `LLPermissions::isGroupOwned`.
    pub fn is_group_owned(&self) -> bool {
        self.group_owned
    }

    /// `LLPermissions::getCRC32`: the four id CRCs plus the base, owner,
    /// everyone and group masks, wrapping. The next-owner mask and the
    /// group-owned flag are not covered.
    pub fn crc32(&self) -> u32 {
        uuid_crc32(&self.creator)
            .wrapping_add(uuid_crc32(&self.owner))
            .wrapping_add(uuid_crc32(&self.last_owner))
            .wrapping_add(uuid_crc32(&self.group))
            .wrapping_add(self.base)
            .wrapping_add(self.owner_mask)
            .wrapping_add(self.everyone)
            .wrapping_add(self.group_mask)
    }

    /// `LLPermissions::set`: copies every field.
    pub fn set(&mut self, from: &Permissions) {
        *self = *from;
    }

    /// `LLPermissions::fix`: the mask hierarchy. Owner within base, group
    /// within owner, next within base, everyone within owner and never
    /// modify; a base without transfer on a record that is not group owned
    /// also takes copy from group and everyone.
    pub fn fix(&mut self) {
        self.owner_mask &= self.base;
        self.group_mask &= self.owner_mask;
        self.next &= self.base;
        self.everyone &= self.owner_mask;
        self.everyone &= !PERM_MODIFY;
        if self.base & PERM_TRANSFER == 0 && !self.group_owned {
            self.group_mask &= !PERM_COPY;
            self.everyone &= !PERM_COPY;
        }
    }

    /// `LLPermissions::fixFairUse`: base always gains move, and transfer
    /// when it has no copy; a next-owner mask that is not `PERM_NONE` gains
    /// move.
    pub fn fix_fair_use(&mut self) {
        self.base |= PERM_MOVE;
        if self.base & PERM_COPY == 0 {
            self.base |= PERM_TRANSFER;
        }
        if self.next != PERM_NONE {
            self.next |= PERM_MOVE;
        }
    }

    /// `LLPermissions::fixOwnership`: group owned exactly when the owner is
    /// null and the group is not.
    pub fn fix_ownership(&mut self) {
        self.group_owned = is_null(&self.owner) && !is_null(&self.group);
    }

    /// `LLPermissions::accumulate`: each differing id becomes null, each
    /// mask is intersected, then `fix()`. The group-owned flag is kept.
    pub fn accumulate(&mut self, perm: &Permissions) {
        if perm.creator != self.creator {
            self.creator = NULL_ID;
        }
        if perm.owner != self.owner {
            self.owner = NULL_ID;
        }
        if perm.last_owner != self.last_owner {
            self.last_owner = NULL_ID;
        }
        if perm.group != self.group {
            self.group = NULL_ID;
        }
        self.base &= perm.base;
        self.owner_mask &= perm.owner_mask;
        self.group_mask &= perm.group_mask;
        self.everyone &= perm.everyone;
        self.next &= perm.next;
        self.fix();
    }

    /// `LLPermissions::setOwnerAndGroup`. Allowed for the system (null
    /// agent), on an unowned record, or for the owner when the owner stays
    /// or the owner mask has transfer. An owner change (or, on a record
    /// that stays unowned, a group change) makes the next-owner mask the
    /// base, and `is_atomic` then adds transfer to a base without copy.
    /// Then the group, `fixOwnership()`, move on the base and on a
    /// non-empty next-owner mask, and `fix()`. Returns whether it was
    /// allowed.
    pub fn set_owner_and_group(
        &mut self,
        agent: &Uuid,
        owner: &Uuid,
        group: &Uuid,
        is_atomic: bool,
    ) -> bool {
        let allowed = is_null(agent)
            || is_null(&self.owner)
            || (*agent == self.owner
                && (*owner == self.owner || self.owner_mask & PERM_TRANSFER != 0));
        if allowed {
            if is_null(&self.last_owner) || (!is_null(&self.owner) && *owner != self.last_owner) {
                self.last_owner = self.owner;
            }
            if self.owner != *owner
                || (is_null(&self.owner) && is_null(owner) && self.group != *group)
            {
                self.base = self.next;
                self.owner = *owner;
                if is_atomic && self.base & PERM_COPY == 0 {
                    self.base |= PERM_TRANSFER;
                }
            }
            self.group = *group;
            self.fix_ownership();
            self.base |= PERM_MOVE;
            if self.next != PERM_NONE {
                self.next |= PERM_MOVE;
            }
            self.fix();
        }
        allowed
    }

    /// `LLPermissions::setLastOwner`: only on a group-owned record.
    pub fn set_last_owner(&mut self, last_owner: &Uuid) {
        if self.group_owned {
            self.last_owner = *last_owner;
        }
    }

    /// `LLPermissions::yesReallySetOwner`: the owner and the flag, raw.
    pub fn yes_really_set_owner(&mut self, owner: &Uuid, group_owned: bool) {
        self.owner = *owner;
        self.group_owned = group_owned;
    }

    /// `LLPermissions::deedToGroup`: to a non-null group, by the system, or
    /// by an agent when the group is the record's, the owner mask has
    /// transfer and the group mask has move. The owner becomes the last
    /// owner, the next-owner mask the base and the group mask `PERM_NONE`;
    /// then `fixFairUse()` and `fix()`.
    pub fn deed_to_group(&mut self, agent: &Uuid, group: &Uuid) -> bool {
        if !is_null(group)
            && (is_null(agent)
                || (*group == self.group
                    && self.owner_mask & PERM_TRANSFER != 0
                    && self.group_mask & PERM_MOVE != 0))
        {
            if !is_null(&self.owner) {
                self.last_owner = self.owner;
                self.owner = NULL_ID;
            }
            self.base = self.next;
            self.group_mask = PERM_NONE;
            self.group = *group;
            self.group_owned = true;
            self.fix_fair_use();
            self.fix();
            return true;
        }
        false
    }

    fn apply_bits(mask: &mut PermissionMask, set: bool, bits: PermissionMask) {
        if set {
            *mask |= bits;
        } else {
            *mask &= !bits;
        }
    }

    /// Who may change the group, everyone and next-owner bits: the system,
    /// the owner, or a member of the record's (non-null) group.
    fn may_set_shared_bits(&self, agent: &Uuid, group: &Uuid) -> bool {
        is_null(agent) || *agent == self.owner || (*group == self.group && !is_null(&self.group))
    }

    /// `LLPermissions::setBaseBits`: the system only; then `fix()`.
    pub fn set_base_bits(&mut self, agent: &Uuid, set: bool, bits: PermissionMask) -> bool {
        let ownership = is_null(agent);
        if ownership {
            Self::apply_bits(&mut self.base, set, bits);
            self.fix();
        }
        ownership
    }

    /// `LLPermissions::setOwnerBits`: the system or the owner; then `fix()`.
    pub fn set_owner_bits(&mut self, agent: &Uuid, set: bool, bits: PermissionMask) -> bool {
        let ownership = is_null(agent) || *agent == self.owner;
        if ownership {
            Self::apply_bits(&mut self.owner_mask, set, bits);
            self.fix();
        }
        ownership
    }

    /// `LLPermissions::setGroupBits`: the system, the owner or a group
    /// member; then `fix()`.
    pub fn set_group_bits(
        &mut self,
        agent: &Uuid,
        group: &Uuid,
        set: bool,
        bits: PermissionMask,
    ) -> bool {
        let ownership = self.may_set_shared_bits(agent, group);
        if ownership {
            Self::apply_bits(&mut self.group_mask, set, bits);
            self.fix();
        }
        ownership
    }

    /// `LLPermissions::setEveryoneBits`: the system, the owner or a group
    /// member; then `fix()`.
    pub fn set_everyone_bits(
        &mut self,
        agent: &Uuid,
        group: &Uuid,
        set: bool,
        bits: PermissionMask,
    ) -> bool {
        let ownership = self.may_set_shared_bits(agent, group);
        if ownership {
            Self::apply_bits(&mut self.everyone, set, bits);
            self.fix();
        }
        ownership
    }

    /// `LLPermissions::setNextOwnerBits`: the system, the owner or a group
    /// member; a next-owner mask without copy then gains transfer, then
    /// `fix()`.
    pub fn set_next_owner_bits(
        &mut self,
        agent: &Uuid,
        group: &Uuid,
        set: bool,
        bits: PermissionMask,
    ) -> bool {
        let ownership = self.may_set_shared_bits(agent, group);
        if ownership {
            Self::apply_bits(&mut self.next, set, bits);
            if self.next & PERM_COPY == 0 {
                self.next |= PERM_TRANSFER;
            }
            self.fix();
        }
        ownership
    }

    /// `LLPermissions::allowOperationBy(op, requester, group)`: the system
    /// always; the owning group, or the owner of a record that is not group
    /// owned, by the owner mask; a member of the record's group by the
    /// group or everyone mask; anyone else by the everyone mask. Any bit of
    /// `op` suffices.
    pub fn allow_operation_by(&self, op: PermissionBit, requester: &Uuid, group: &Uuid) -> bool {
        if is_null(requester) {
            true
        } else if self.group_owned && self.group == *requester {
            self.owner_mask & op != 0
        } else if !self.group_owned && self.owner == *requester {
            self.owner_mask & op != 0
        } else if !is_null(&self.group) && self.group == *group {
            self.group_mask & op != 0 || self.everyone & op != 0
        } else {
            self.everyone & op != 0
        }
    }

    /// `LLPermissions::allowModifyBy(agent, group)`; [`NULL_ID`] as the
    /// group is the one-argument form.
    pub fn allow_modify_by(&self, agent: &Uuid, group: &Uuid) -> bool {
        self.allow_operation_by(PERM_MODIFY, agent, group)
    }

    /// `LLPermissions::allowCopyBy(agent, group)`; [`NULL_ID`] as the group
    /// is the one-argument form.
    pub fn allow_copy_by(&self, agent: &Uuid, group: &Uuid) -> bool {
        self.allow_operation_by(PERM_COPY, agent, group)
    }

    /// `LLPermissions::allowMoveBy(agent, group)`; [`NULL_ID`] as the group
    /// is the one-argument form.
    pub fn allow_move_by(&self, agent: &Uuid, group: &Uuid) -> bool {
        self.allow_operation_by(PERM_MOVE, agent, group)
    }

    /// `LLPermissions::allowTransferTo`: a group-owned record asks its group
    /// as the owner; otherwise true for the owner, else the owner's own
    /// transfer permission (always granted when the owner is null).
    pub fn allow_transfer_to(&self, agent: &Uuid) -> bool {
        if self.group_owned {
            self.allow_operation_by(PERM_TRANSFER, &self.group, &self.group)
        } else {
            self.owner == *agent || self.allow_operation_by(PERM_TRANSFER, &self.owner, &NULL_ID)
        }
    }

    /// `LLPermissions::allowExportBy`: the agent is both owner and creator.
    pub fn allow_export_by(&self, agent: &Uuid) -> bool {
        *agent == self.owner && *agent == self.creator
    }

    /// `LLPermissions::allowOpenSimExportBy` (OpenSim builds): the creator,
    /// else the export permission.
    #[cfg(feature = "opensim")]
    pub fn allow_open_sim_export_by(&self, agent: &Uuid) -> bool {
        self.creator == *agent || self.allow_operation_by(PERM_EXPORT, agent, &NULL_ID)
    }
}
