//! `LLSaleInfo::importLegacyStream` and `exportLegacyStream`
//! (`llsaleinfo.cpp`).
//!
//! The text is the `sale_info` block of the legacy item form:
//!
//! ```text
//! \tsale_info\t0
//! \t{
//! \t\tsale_type\tnot
//! \t\tsale_price\t10
//! \t}
//! ```
//!
//! The reader starts after the `sale_info\t0` line, which its caller has
//! consumed.

use std::io::Write as _;

use super::reader::Stream;
use super::scanf::{scan_hex, scan_int, sscanf};
use crate::sale::{clamp_price, SaleInfo, SaleType};

/// `MAX_STRING` (`lldefs.h`): the line buffer of the sale-info, item and
/// category readers, so lines of up to 254 bytes.
pub const MAX_STRING: usize = 255;

impl SaleInfo {
    /// `LLSaleInfo::importLegacyStream(input, has_perm_mask, perm_mask)`.
    ///
    /// Clears `has_perm_mask`, then reads lines of up to 254 bytes while the
    /// stream is good, each parsed on its own as `" %254s %254s"` (bug 29):
    /// - a blank line, `{`, an unknown keyword and a keyword without a value
    ///   are skipped; `}` ends the block;
    /// - `sale_type` is looked up by name (an unknown name is `FS_NOT`);
    /// - `sale_price` takes a `%d` value that parses, clamped to
    ///   `0..=S32_MAX`;
    /// - `perm_mask` (deprecated) stores a `%x` value that parses into
    ///   `perm_mask` and sets `has_perm_mask`.
    ///
    /// Always returns true. The stream is left where the C++ leaves it.
    pub fn import_legacy_stream(&mut self, input: &mut Stream<'_>, has_perm_mask: &mut bool, perm_mask: &mut u32) -> bool {
        *has_perm_mask = false;
        while input.good() {
            let line = input.getline(MAX_STRING);
            let scan = sscanf(line, " %254s %254s");
            let keyword = scan.str(0).unwrap_or_default();
            if keyword.is_empty() || keyword == b"{" {
                continue;
            }
            if keyword == b"}" {
                break;
            }
            if scan.ret < 2 {
                continue;
            }
            let value = scan.str(1).unwrap_or_default();
            match keyword {
                b"sale_type" => self.sale_type = SaleType::from_name_cstr(Some(value)),
                b"sale_price" => {
                    if let Some(p) = scan_int(value) {
                        self.price = clamp_price(p);
                    }
                }
                b"perm_mask" => {
                    if let Some(m) = scan_hex(value) {
                        *perm_mask = m;
                        *has_perm_mask = true;
                    }
                }
                // LL_WARNS "unknown keyword ... in sale info import".
                _ => {}
            }
        }
        true
    }

    /// `LLSaleInfo::exportLegacyStream`: appends the block. A sale type
    /// outside `0..FS_COUNT` is written `not` (bug 1, `913990ebe3`; port
    /// rule 2). Always returns true.
    pub fn export_legacy_stream(&self, out: &mut Vec<u8>) -> bool {
        let _ = write!(
            out,
            "\tsale_info\t0\n\t{{\n\t\tsale_type\t{}\n\t\tsale_price\t{}\n\t}}\n",
            self.type_name(),
            self.price
        );
        true
    }
}
