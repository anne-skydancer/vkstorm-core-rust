//! The C++ `LLSD` coercions the readers rely on (`asInteger`, `asString`,
//! `asUUID`, `asBoolean`, `llcommon/llsd.cpp`), transcribed over `Value`.
//!
//! Each function names the `Impl*` member it mirrors. A coercion the C++
//! class does not define for a type gives that type's default (`0`, `""`,
//! the null UUID, `false`, empty binary), as `LLSD::Impl`'s base members do.
//!
//! Port rule 1 (`fixtures/model/README.md`): a String converts to a number
//! as the viewer's own C++ `LLSD` does on the same platform, through
//! vkstorm-llsd's `string_as_real_native`: the MSVC runtime's
//! `istringstream >> F64` under the MSVC target environment (hexadecimal
//! floating point accepted, `"0x10"` is 16), libstdc++'s otherwise (no
//! hexadecimal, `"0x10"` is 0; overflow stores `DBL_MAX`). The fixtures
//! record both values as `RT(win=...;linux=...)`.

use std::collections::BTreeMap;

use vkstorm_llsd::Value;
use vkstorm_llsd::text::{
    date_format, fmt_g, real_as_integer, string_as_real_native, uuid_set, uuid_to_hex,
};

use crate::perms::{NULL_ID, Uuid};

static UNDEF: Value = Value::Undefined;

/// `sd[key]` on a const `LLSD`: the value of a map's key, else undefined
/// (a missing key, or any value that is not a map).
pub fn get<'a>(sd: &'a Value, key: &str) -> &'a Value {
    match sd {
        Value::Map(m) => m.get(key.as_bytes()).unwrap_or(&UNDEF),
        _ => &UNDEF,
    }
}

/// `LLSD::has(key)`: false for anything but a map holding the key.
pub fn has(sd: &Value, key: &str) -> bool {
    matches!(sd, Value::Map(m) if m.contains_key(key.as_bytes()))
}

/// `LLSD::isString`.
pub fn is_string(sd: &Value) -> bool {
    matches!(sd, Value::String(_))
}

/// `LLSD::isInteger`.
pub fn is_integer(sd: &Value) -> bool {
    matches!(sd, Value::Integer(_))
}

/// The map of a non-const `LLSD`'s `operator[](key)`: `makeMap` turns any
/// value that is not a map into an empty map first.
pub fn make_map(sd: &mut Value) -> &mut BTreeMap<Vec<u8>, Value> {
    if !matches!(sd, Value::Map(_)) {
        *sd = Value::map();
    }
    match sd {
        Value::Map(m) => m,
        _ => unreachable!("made a map above"),
    }
}

/// `sd[key] = value` on a non-const `LLSD` (see [`make_map`]).
pub fn insert(sd: &mut Value, key: &str, value: Value) {
    make_map(sd).insert(key.as_bytes().to_vec(), value);
}

/// `asInteger()`: `ImplBoolean` 0/1; `ImplInteger`; `ImplReal` a C cast
/// (0 for NaN; out of range is `INT_MIN`, as x86 converts); `ImplString`
/// `(int)asReal()`; `ImplDate` `(Integer)secondsSinceEpoch()`; else 0.
pub fn as_integer(sd: &Value) -> i32 {
    match sd {
        Value::Boolean(b) => i32::from(*b),
        Value::Integer(i) => *i,
        Value::Real(r) if r.is_nan() => 0,
        Value::Real(r) => real_as_integer(*r),
        Value::String(s) => real_as_integer(string_as_real_native(s)),
        Value::Date(d) => real_as_integer(*d),
        _ => 0,
    }
}

/// `asReal()`: `ImplBoolean` 0/1; `ImplInteger`; `ImplReal`; `ImplString`
/// (`istringstream >> F64`, then the whole string must be consumed);
/// `ImplDate` seconds; else 0.
pub fn as_real(sd: &Value) -> f64 {
    match sd {
        Value::Boolean(b) => f64::from(u8::from(*b)),
        Value::Integer(i) => f64::from(*i),
        Value::Real(r) => *r,
        Value::String(s) => string_as_real_native(s),
        Value::Date(d) => *d,
        _ => 0.0,
    }
}

/// `asBoolean()`: `ImplBoolean`; `ImplInteger` non-zero; `ImplReal`
/// non-zero and not NaN; `ImplString` non-empty; a map or array non-empty;
/// false for every other type (a UUID, date, URI or binary included).
pub fn as_boolean(sd: &Value) -> bool {
    match sd {
        Value::Boolean(b) => *b,
        Value::Integer(i) => *i != 0,
        Value::Real(r) => !r.is_nan() && *r != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Map(m) => !m.is_empty(),
        Value::Array(a) => !a.is_empty(),
        _ => false,
    }
}

/// `asString()`: `ImplBoolean` `"true"` or `""`; `ImplInteger` `%d`;
/// `ImplReal` `%lg`; `ImplString`; `ImplUUID` lower-case dashed;
/// `ImplDate` ISO 8601; `ImplURI`; else empty (binary, maps, arrays).
pub fn as_string(sd: &Value) -> Vec<u8> {
    match sd {
        Value::Boolean(b) => {
            if *b {
                b"true".to_vec()
            } else {
                Vec::new()
            }
        }
        Value::Integer(i) => i.to_string().into_bytes(),
        Value::Real(r) => {
            let mut out = Vec::new();
            fmt_g(*r, 6, &mut out);
            out
        }
        Value::String(s) | Value::Uri(s) => s.clone(),
        Value::Uuid(u) => {
            let mut out = Vec::with_capacity(36);
            uuid_to_hex(u, &mut out);
            out
        }
        Value::Date(d) => {
            let mut out = Vec::new();
            date_format(*d, &mut out);
            out
        }
        _ => Vec::new(),
    }
}

/// `asUUID()`: `ImplUUID`; `ImplString` `LLUUID(string)` (`LLUUID::set`
/// leniency); else null.
pub fn as_uuid(sd: &Value) -> Uuid {
    match sd {
        Value::Uuid(u) => *u,
        Value::String(s) => uuid_set(s),
        _ => NULL_ID,
    }
}

/// `asBinary()`: `ImplBinary`'s bytes, else empty.
pub fn as_binary(sd: &Value) -> &[u8] {
    match sd {
        Value::Binary(b) => b,
        _ => &[],
    }
}

/// `ll_U32_from_sd` (`llsdutil.cpp`): the first four bytes of `asBinary()`
/// in network order, 0 when there are fewer than four.
pub fn u32_from_sd(sd: &Value) -> u32 {
    match as_binary(sd) {
        [a, b, c, d, ..] => u32::from_be_bytes([*a, *b, *c, *d]),
        _ => 0,
    }
}
