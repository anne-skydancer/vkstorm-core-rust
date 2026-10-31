//! `LLSD` accessors with the C++ class's conversion rules, over the
//! [`Value`] tree of vkstorm-llsd: a missing key or index is undefined,
//! and the scalar conversions follow `llsd.cpp`'s `Impl*` classes.

use vkstorm_llsd::{Format, Limits, Value};

use crate::Uuid;

static UNDEF: Value = Value::Undefined;
static EMPTY: Vec<u8> = Vec::new();

/// `sd[key]` on a const `LLSD`: undefined unless a map with the key.
pub fn get<'a>(v: &'a Value, key: &str) -> &'a Value {
    match v {
        Value::Map(m) => m.get(key.as_bytes()).unwrap_or(&UNDEF),
        _ => &UNDEF,
    }
}

/// `sd[i]` on a const `LLSD`: undefined unless an array with the index.
pub fn index(v: &Value, i: usize) -> &Value {
    match v {
        Value::Array(a) => a.get(i).unwrap_or(&UNDEF),
        _ => &UNDEF,
    }
}

/// `sd.has(key)`.
pub fn has(v: &Value, key: &str) -> bool {
    matches!(v, Value::Map(m) if m.contains_key(key.as_bytes()))
}

/// `sd.size()`: elements of a map or array, bytes of a string, else 0.
pub fn size(v: &Value) -> usize {
    match v {
        Value::Map(m) => m.len(),
        Value::Array(a) => a.len(),
        Value::String(s) => s.len(),
        _ => 0,
    }
}

pub fn is_map(v: &Value) -> bool {
    matches!(v, Value::Map(_))
}

pub fn is_uuid(v: &Value) -> bool {
    matches!(v, Value::Uuid(_))
}

/// `asInteger()`.
pub fn as_integer(v: &Value) -> i32 {
    match v {
        Value::Boolean(b) => *b as i32,
        Value::Integer(i) => *i,
        Value::Real(r) => real_to_int(*r),
        Value::String(s) => real_to_int(string_to_real(s)),
        Value::Date(d) => *d as i32,
        _ => 0,
    }
}

/// `asReal()`.
pub fn as_real(v: &Value) -> f64 {
    match v {
        Value::Boolean(b) => *b as i32 as f64,
        Value::Integer(i) => *i as f64,
        Value::Real(r) => *r,
        Value::String(s) => string_to_real(s),
        Value::Date(d) => *d,
        _ => 0.0,
    }
}

/// `asBoolean()`.
pub fn as_boolean(v: &Value) -> bool {
    match v {
        Value::Boolean(b) => *b,
        Value::Integer(i) => *i != 0,
        Value::Real(r) => !r.is_nan() && *r != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Uuid(u) => *u != [0; 16],
        Value::Binary(b) => !b.is_empty(),
        Value::Map(m) => !m.is_empty(),
        Value::Array(a) => !a.is_empty(),
        _ => false,
    }
}

/// `asBinary()`: the bytes, empty for any other type.
pub fn as_binary(v: &Value) -> &Vec<u8> {
    match v {
        Value::Binary(b) => b,
        _ => &EMPTY,
    }
}

/// `asUUID()` of a UUID value.
pub fn as_uuid(v: &Value) -> Uuid {
    match v {
        Value::Uuid(u) => *u,
        _ => [0; 16],
    }
}

/// `asString()` for the string kinds used here.
pub fn as_string(v: &Value) -> Vec<u8> {
    match v {
        Value::String(s) => s.clone(),
        Value::Uri(s) => s.clone(),
        Value::Integer(i) => i.to_string().into_bytes(),
        Value::Boolean(b) => {
            if *b {
                b"true".to_vec()
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

/// `(LLSD::Integer)real` with the NaN check: a C++ float-to-int cast,
/// saturating here where the C++ was undefined.
fn real_to_int(r: f64) -> i32 {
    if r.is_nan() { 0 } else { r as i32 }
}

/// `std::istringstream >> F64`: the leading number of the string.
fn string_to_real(s: &[u8]) -> f64 {
    let text = String::from_utf8_lossy(s);
    let t = text.trim_start();
    // Longest numeric prefix the stream extraction would take.
    let mut end = 0;
    let b = t.as_bytes();
    let mut seen_digit = false;
    let mut seen_dot = false;
    let mut seen_exp = false;
    while end < b.len() {
        let c = b[end];
        if c.is_ascii_digit() {
            seen_digit = true;
        } else if (c == b'+' || c == b'-') && (end == 0 || b[end - 1] == b'e' || b[end - 1] == b'E')
        {
        } else if c == b'.' && !seen_dot && !seen_exp {
            seen_dot = true;
        } else if (c == b'e' || c == b'E') && seen_digit && !seen_exp {
            seen_exp = true;
        } else {
            break;
        }
        end += 1;
    }
    if !seen_digit {
        return 0.0;
    }
    let mut candidate = &t[..end];
    while !candidate.is_empty() {
        if let Ok(v) = candidate.parse::<f64>() {
            return v;
        }
        candidate = &candidate[..candidate.len() - 1];
    }
    0.0
}

/// `LLVector3::setValue(sd)`: `(F32)sd[i].asReal()` per component.
pub fn vector3(v: &Value) -> [f32; 3] {
    [
        as_real(index(v, 0)) as f32,
        as_real(index(v, 1)) as f32,
        as_real(index(v, 2)) as f32,
    ]
}

/// `LLVector2::setValue(sd)`.
pub fn vector2(v: &Value) -> [f32; 2] {
    [as_real(index(v, 0)) as f32, as_real(index(v, 1)) as f32]
}

/// `LLSDSerialize::fromBinary(sd, stream, max_bytes, max_depth)` over a
/// byte slice: the value and the number of bytes the parser consumed
/// (`stream.tellg()` afterwards), or `None` on failure.
pub fn parse_binary(bytes: &[u8], max_bytes: i64, max_depth: i32) -> Option<(Value, usize)> {
    let mut src = vkstorm_llsd::SliceSource::new(bytes);
    let mut r = vkstorm_llsd::Reader::new(&mut src);
    let mut b = vkstorm_llsd::ValueBuilder::new(vkstorm_llsd::key_policy(Format::Binary));
    let n = vkstorm_llsd::parse_into(
        Format::Binary,
        &mut r,
        &mut b,
        Limits::from_cpp(max_bytes, max_depth),
        vkstorm_llsd::XmlMode::Parse,
    );
    if n > 0 {
        let consumed = r.consumed();
        b.finish().map(|v| (v, consumed))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert_eq!(as_integer(&Value::Real(3.9)), 3);
        assert_eq!(as_integer(&Value::Real(-3.9)), -3);
        assert_eq!(as_integer(&Value::String(b"1.23".to_vec())), 1);
        assert_eq!(as_integer(&Value::String(b"  42abc".to_vec())), 42);
        assert_eq!(as_integer(&Value::String(b"abc".to_vec())), 0);
        assert_eq!(as_integer(&Value::Boolean(true)), 1);
        assert_eq!(as_integer(&Value::Undefined), 0);
        assert_eq!(as_real(&Value::Integer(7)), 7.0);
        assert_eq!(as_real(&Value::String(b"1e3x".to_vec())), 1000.0);
        assert_eq!(
            vector3(&Value::Array(vec![Value::Real(1.5), Value::Integer(2)])),
            [1.5, 2.0, 0.0]
        );
    }
}
