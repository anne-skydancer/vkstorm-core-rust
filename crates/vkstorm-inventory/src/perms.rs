//! `LLPermissions` rules (`llpermissions.{h,cpp}`): fixing, masks, accumulation,
//! ownership changes, the CRC and `mask_to_string`.

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
