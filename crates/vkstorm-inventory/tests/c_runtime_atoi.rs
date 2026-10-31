//! `atoi` and `atol` (`exchange::names`), which `LLHost`'s port and the
//! group-notice bucket parser go through, against the C runtime of the
//! platform the tests run on: the UCRT with MSVC on Windows (32-bit
//! `long`), glibc on Linux (64-bit `long`). The crate itself has no
//! `unsafe` code, so the calls into the C runtime live here.

use vkstorm_inventory::exchange::names::{atoi, atol};

/// The platform's own C runtime (the UCRT on Windows, glibc on Linux)
/// agrees with the port on text rules and on values around the `int`,
/// `long` and 64-bit limits.
#[test]
fn atoi_atol_match_the_c_runtime() {
    use std::ffi::{CString, c_char, c_int, c_long};
    unsafe extern "C" {
        #[link_name = "atoi"]
        fn c_atoi(s: *const c_char) -> c_int;
        #[link_name = "atol"]
        fn c_atol(s: *const c_char) -> c_long;
    }
    let cases: &[&str] = &[
        "",
        "x",
        "0",
        "42",
        "  \t\n+7x",
        "-7 8",
        "- 7",
        "--7",
        "0x10",
        "300",
        "2147483647",
        "2147483648",
        "-2147483648",
        "-2147483649",
        "4294967295",
        "4294967296",
        "4294967297",
        "-4294967297",
        "9223372036854775807",
        "9223372036854775808",
        "-9223372036854775808",
        "-9223372036854775809",
        "18446744073709551617",
        "99999999999999999999",
        "-99999999999999999999",
        "123456789012345678901234567890123456789",
    ];
    for case in cases {
        let c = CString::new(*case).unwrap();
        // SAFETY: `c` is a valid NUL-terminated string; atoi and atol only read it.
        let (ci, cl) = unsafe { (c_atoi(c.as_ptr()), c_atol(c.as_ptr())) };
        assert_eq!(atoi(case.as_bytes()), ci, "atoi({case:?})");
        assert_eq!(atol(case.as_bytes()), cl, "atol({case:?})");
        assert_eq!(
            atol(case.as_bytes()) as u32,
            cl as u32,
            "LLHost port {case:?}"
        );
    }
}
