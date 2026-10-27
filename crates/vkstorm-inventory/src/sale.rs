//! `LLSaleInfo` rules and CRC (`llsaleinfo.{h,cpp}`).
//!
//! [`SaleInfo`] is the flat record of an `LLSaleInfo`. Its LLSD forms live in
//! [`crate::llsd::sale`] and its legacy text in [`crate::legacy::sale`].

pub use crate::types::sale::{SaleType, DEFAULT_PRICE};

/// `llclamp(price, 0, S32_MAX)`: a negative price is 0.
pub fn clamp_price(price: i32) -> i32 {
    price.max(0)
}

/// `LLSaleInfo` as a flat record: the sale type and the price, both raw.
///
/// The fields are public and may hold any value, as the C++ object can (a
/// sale type outside `0..FS_COUNT`, a negative price from a raw store or
/// from [`accumulate`](Self::accumulate)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SaleInfo {
    /// `mSaleType`.
    pub sale_type: SaleType,
    /// `mSalePrice`.
    pub price: i32,
}

impl Default for SaleInfo {
    /// `LLSaleInfo::LLSaleInfo()`: [`SaleInfo::DEFAULT`].
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl SaleInfo {
    /// `LLSaleInfo::DEFAULT` and the default constructor: not for sale, at
    /// `DEFAULT_PRICE`.
    pub const DEFAULT: SaleInfo = SaleInfo { sale_type: SaleType::NOT, price: DEFAULT_PRICE };

    /// `LLSaleInfo(EForSale, S32)`: the type as given, the price clamped to
    /// `0..=S32_MAX`.
    pub fn new(sale_type: SaleType, price: i32) -> SaleInfo {
        SaleInfo { sale_type, price: clamp_price(price) }
    }

    /// `isForSale`: any type but `FS_NOT`.
    pub fn is_for_sale(&self) -> bool {
        self.sale_type != SaleType::NOT
    }

    /// `getCRC32`: the price plus the type times `0x07073096`, wrapping (the
    /// C++ product is a signed `int` multiplication).
    pub fn crc32(&self) -> u32 {
        (self.price as u32).wrapping_add(self.sale_type.0.wrapping_mul(0x0707_3096) as u32)
    }

    /// `setSaleType`: raw.
    pub fn set_sale_type(&mut self, sale_type: SaleType) {
        self.sale_type = sale_type;
    }

    /// `setSalePrice`: clamped to `0..=S32_MAX`.
    pub fn set_sale_price(&mut self, price: i32) {
        self.price = clamp_price(price);
    }

    /// `accumulate`: a differing type makes it `FS_NOT`; the prices add,
    /// unclamped.
    ///
    /// The C++ addition is signed `int` overflow, undefined behaviour that
    /// both recorded compilers wrap (runtime edge 4); this wraps.
    pub fn accumulate(&mut self, other: &SaleInfo) {
        if self.sale_type != other.sale_type {
            self.sale_type = SaleType::NOT;
        }
        self.price = self.price.wrapping_add(other.price);
    }

    /// `lookup(mSaleType)` with `FS_NOT`'s name for a type outside
    /// `0..FS_COUNT`, as `exportLegacyStream`, `asLLSD` and
    /// `ll_create_sd_from_sale_info` write it (bug 1).
    pub fn type_name(&self) -> &'static str {
        self.sale_type.lookup().unwrap_or("not")
    }
}
