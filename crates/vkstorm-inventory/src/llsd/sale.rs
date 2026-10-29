//! Sale info LLSD: `LLSaleInfo::asLLSD`, `fromLLSD`, `ll_create_sd_from_sale_info`
//! and `ll_sale_info_from_sd` (`llsaleinfo.cpp`).
//!
//! The inventory form is `{sale_price: Integer, sale_type: String}`; the
//! hyphenated message form of `packMessage()`/`unpackMessage(LLSD)` (no viewer
//! caller) is `{sale-price: Integer, sale-type: Integer}`.

use vkstorm_llsd::Value;

use super::coerce::{as_integer, as_string, get, has, insert, u32_from_sd};
use crate::sale::{SaleInfo, SaleType, clamp_price};

/// `ST_TYPE_LABEL`.
pub const SALE_TYPE: &str = "sale_type";
/// `ST_PRICE_LABEL`.
pub const SALE_PRICE: &str = "sale_price";
/// The deprecated `perm_mask` key `fromLLSD` reads.
pub const PERM_MASK: &str = "perm_mask";

impl SaleInfo {
    /// `LLSaleInfo::asLLSD()`, `operator LLSD()` and
    /// `ll_create_sd_from_sale_info` (which agree): the type by name (`not`
    /// for a type outside `0..FS_COUNT`) and the price.
    pub fn to_llsd(&self) -> Value {
        let mut sd = Value::Undefined;
        self.fill_llsd(&mut sd);
        sd
    }

    /// `LLSaleInfo::asLLSD(LLSD&)`: sets `sale_type` and `sale_price` in
    /// `sd`, keeping its other keys (a value that is not a map becomes one).
    pub fn fill_llsd(&self, sd: &mut Value) {
        insert(sd, SALE_TYPE, Value::string(self.type_name()));
        insert(sd, SALE_PRICE, Value::Integer(self.price));
    }

    /// `LLSaleInfo::fromLLSD(sd, has_perm_mask, perm_mask)`.
    ///
    /// - A String `sale_type` is looked up by name (as a C string; an
    ///   unknown name is `FS_NOT`); an Integer one is `(S8)(U8)` of its
    ///   value, so 255 is -1 and 200 is -56; any other type leaves the type.
    /// - `sale_price` is always set: `asInteger()` (0 when absent) clamped to
    ///   `0..=S32_MAX`.
    /// - A `perm_mask` key, of any type, sets `has_perm_mask` and stores
    ///   `ll_U32_from_sd` (0 unless Binary of at least 4 bytes); without it
    ///   both are left alone.
    ///
    /// Non-map input reads every key as undefined. Always returns true.
    pub fn from_llsd(&mut self, sd: &Value, has_perm_mask: &mut bool, perm_mask: &mut u32) -> bool {
        match get(sd, SALE_TYPE) {
            Value::String(s) => self.sale_type = SaleType::from_name_cstr(Some(s)),
            Value::Integer(i) => self.sale_type = SaleType(i32::from(*i as u8 as i8)),
            _ => {}
        }
        self.price = clamp_price(as_integer(get(sd, SALE_PRICE)));
        if has(sd, PERM_MASK) {
            *has_perm_mask = true;
            *perm_mask = u32_from_sd(get(sd, PERM_MASK));
        }
        true
    }

    /// `ll_sale_info_from_sd`: from the default record, the type by the name
    /// of `asString()` of `sale_type` (whatever its LLSD type) and the
    /// price `(S32)` clamped.
    pub fn from_sd(sd: &Value) -> SaleInfo {
        let mut s = SaleInfo::DEFAULT;
        s.set_sale_type(SaleType::from_name_cstr(Some(&as_string(get(
            sd, SALE_TYPE,
        )))));
        s.set_sale_price(clamp_price(as_integer(get(sd, SALE_PRICE))));
        s
    }

    /// `LLSaleInfo::packMessage()` (LLSD): the type as `(U8)` and the price.
    pub fn pack_message_llsd(&self) -> Value {
        let mut sd = Value::map();
        insert(
            &mut sd,
            "sale-type",
            Value::Integer(i32::from(self.sale_type.0 as u8)),
        );
        insert(&mut sd, "sale-price", Value::Integer(self.price));
        sd
    }

    /// `LLSaleInfo::unpackMessage(LLSD)`: the type as `(U8)` of
    /// `asInteger()` (so 0..=255), the price clamped.
    pub fn unpack_message_llsd(&mut self, sd: &Value) {
        self.sale_type = SaleType(i32::from(as_integer(get(sd, "sale-type")) as u8));
        self.price = clamp_price(as_integer(get(sd, "sale-price")));
    }
}
