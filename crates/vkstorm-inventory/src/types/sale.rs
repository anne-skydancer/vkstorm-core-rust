//! Sale types: `LLSaleInfo::EForSale` (`llsaleinfo.{h,cpp}`) and its name
//! lookups.

use super::asset::c_str;

/// A sale type: `LLSaleInfo::EForSale`, as the raw integer the viewer
/// stores. Any value is carried, including those without a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct SaleType(pub i32);

impl SaleType {
    /// Not for sale.
    pub const NOT: SaleType = SaleType(0);
    /// The original is for sale.
    pub const ORIGINAL: SaleType = SaleType(1);
    /// A copy is for sale.
    pub const COPY: SaleType = SaleType(2);
    /// Tasks only: the contents are for sale.
    pub const CONTENTS: SaleType = SaleType(3);
    pub const COUNT: SaleType = SaleType(4);
}

/// `DEFAULT_PRICE` (`llsaleinfo.h`): the L$ default price for objects.
pub const DEFAULT_PRICE: i32 = 10;

/// `FOR_SALE_NAMES`, indexed by sale type.
pub static NAMES: [&str; SaleType::COUNT.0 as usize] = ["not", "orig", "copy", "cntn"];

impl SaleType {
    /// `LLSaleInfo::lookup(EForSale)`: the name of `0..FS_COUNT`, else `None`
    /// (`NULL`).
    pub fn lookup(self) -> Option<&'static str> {
        usize::try_from(self.0).ok().and_then(|i| NAMES.get(i)).copied()
    }

    /// `LLSaleInfo::lookup(const char*)`: the type whose name equals `name`
    /// (compared as a C string, so it ends at its first NUL), else `FS_NOT`.
    ///
    /// The C++ passes `name` to `strcmp`, so a `NULL` name is undefined
    /// behaviour there; here it is the empty string, which gives `FS_NOT`.
    pub fn from_name_cstr(name: Option<&[u8]>) -> SaleType {
        let name = c_str(name);
        NAMES
            .iter()
            .position(|n| n.as_bytes() == name)
            .map_or(SaleType::NOT, |i| SaleType(i as i32))
    }
}
