//! `LLAggregatePermissions` (`llpermissions.{h,cpp}`): the aggregate rules and
//! their packed `U8` form.
//!
//! An aggregate summarises copy, modify and transfer over a set of masks, each
//! as one of four values ([`AP_EMPTY`] ... [`AP_ALL`]).

use crate::perms::{PERM_COPY, PERM_MODIFY, PERM_TRANSFER, PermissionBit, PermissionMask};

/// `LLAggregatePermissions::AP_EMPTY`: nothing aggregated yet.
pub const AP_EMPTY: u8 = 0x00;
/// `AP_NONE`: no aggregated mask has the bit.
pub const AP_NONE: u8 = 0x01;
/// `AP_SOME`: some have it.
pub const AP_SOME: u8 = 0x02;
/// `AP_ALL`: every one has it.
pub const AP_ALL: u8 = 0x03;

/// `PI_COPY`, `PI_MODIFY` and `PI_TRANSFER`.
const PI_COPY: usize = 0;
const PI_MODIFY: usize = 1;
const PI_TRANSFER: usize = 2;

/// `LLAggregatePermissions`: `mBits`, indexed copy, modify, transfer.
///
/// The values come from [`aggregate_mask`](Self::aggregate_mask),
/// [`aggregate`](Self::aggregate) or [`from_u8`](Self::from_u8), so they are
/// always 0..=3.
///
/// The layout is the C++ object's (`U8 mBits[PI_COUNT]`, 3 bytes, no virtual
/// functions), so the C ABI reads and writes the viewer's objects in place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct AggregatePermissions {
    bits: [u8; 3],
}

// The C++ layout of `LLAggregatePermissions` (see `AggregatePermissions`).
const _: () = assert!(std::mem::size_of::<AggregatePermissions>() == 3);

impl AggregatePermissions {
    /// `LLAggregatePermissions::empty` and the default constructor: every
    /// value [`AP_EMPTY`].
    pub const EMPTY: AggregatePermissions = AggregatePermissions {
        bits: [AP_EMPTY; 3],
    };

    /// `perm2PermIndex`: the index of copy, modify or transfer; `None`
    /// (`PI_END`) for any other value, combinations included.
    fn index(bit: PermissionBit) -> Option<usize> {
        match bit {
            PERM_COPY => Some(PI_COPY),
            PERM_MODIFY => Some(PI_MODIFY),
            PERM_TRANSFER => Some(PI_TRANSFER),
            _ => None,
        }
    }

    /// `getValue(PermissionBit)`: the value of copy, modify or transfer;
    /// [`AP_EMPTY`] for any other bit.
    pub fn value(&self, bit: PermissionBit) -> u8 {
        Self::index(bit).map_or(AP_EMPTY, |i| self.bits[i])
    }

    /// `getU8`: `00TTMMCC`, as `packMessage` sends it.
    pub fn to_u8(&self) -> u8 {
        (self.bits[PI_TRANSFER] << 4) | (self.bits[PI_MODIFY] << 2) | self.bits[PI_COPY]
    }

    /// The state `unpackMessage` reads from a message byte: two bits each
    /// for copy, modify and transfer; bits 6 and 7 are ignored.
    pub fn from_u8(byte: u8) -> AggregatePermissions {
        AggregatePermissions {
            bits: [byte & 3, (byte >> 2) & 3, (byte >> 4) & 3],
        }
    }

    /// `isEmpty`: every value is [`AP_EMPTY`].
    pub fn is_empty(&self) -> bool {
        self.bits.iter().all(|&b| b == AP_EMPTY)
    }

    /// `aggregate(PermissionMask)`: copy, modify and transfer of `mask`
    /// through `aggregateBit`.
    pub fn aggregate_mask(&mut self, mask: PermissionMask) {
        self.aggregate_bit(PI_COPY, mask & PERM_COPY != 0);
        self.aggregate_bit(PI_MODIFY, mask & PERM_MODIFY != 0);
        self.aggregate_bit(PI_TRANSFER, mask & PERM_TRANSFER != 0);
    }

    /// `aggregate(const LLAggregatePermissions&)`: each value through
    /// `aggregateIndex`.
    pub fn aggregate(&mut self, other: &AggregatePermissions) {
        for idx in PI_COPY..=PI_TRANSFER {
            self.aggregate_index(idx, other.bits[idx]);
        }
    }

    /// `aggregateBit`: empty becomes all or none; none becomes some when
    /// allowed; all becomes some when not; some stays.
    fn aggregate_bit(&mut self, idx: usize, allowed: bool) {
        let b = &mut self.bits[idx];
        *b = match (*b, allowed) {
            (AP_EMPTY, true) => AP_ALL,
            (AP_EMPTY, false) => AP_NONE,
            (AP_NONE, true) | (AP_ALL, false) => AP_SOME,
            (v, _) => v,
        };
    }

    /// `aggregateIndex`: empty takes the other value; none meeting some or
    /// all, and all meeting none or some, become some; anything else stays.
    fn aggregate_index(&mut self, idx: usize, bits: u8) {
        let b = &mut self.bits[idx];
        *b = match (*b, bits) {
            (AP_EMPTY, v) => v,
            (AP_NONE, AP_SOME | AP_ALL) | (AP_ALL, AP_NONE | AP_SOME) => AP_SOME,
            (v, _) => v,
        };
    }
}
