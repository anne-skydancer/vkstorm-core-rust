//! Fixture-driven parity with the viewer's former C++ implementation.
//!
//! `tests/fixtures/manifest.txt` and the files beside it were produced by
//! the viewer's `llsdserialize_tool dump` from the C++ oracle (see
//! `tools/llsd-fixtures/README.md`). Each parse case gives the input, the
//! limits, the count the C++ parser returned and, for a positive count, the
//! parsed value in the C++ binary serialization. Each format case gives a
//! value (binary serialization), the formatter settings and the exact bytes
//! the C++ formatter wrote.

use std::fs;
use std::path::{Path, PathBuf};

use vkstorm_llsd::{FloatField, Format, Formatter, Limits, Options, Value, XmlMode, format, parse};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn read(name: &str) -> Vec<u8> {
    fs::read(fixtures_dir().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn format_from(id: &str) -> Format {
    match id {
        "0" => Format::Notation,
        "1" => Format::Binary,
        "2" => Format::Xml,
        other => panic!("format {other}"),
    }
}

/// A value from its C++ binary serialization, parsed by the (separately
/// verified) binary parser.
fn value_from_binary(bytes: &[u8]) -> Value {
    let (n, v) = parse(Format::Binary, bytes, Limits::UNLIMITED);
    assert!(n > 0, "binary fixture value");
    v.unwrap()
}

struct Counts {
    parse: usize,
    format: usize,
}

fn run() -> Counts {
    let manifest = String::from_utf8(read("manifest.txt")).unwrap();
    let mut counts = Counts {
        parse: 0,
        format: 0,
    };
    for line in manifest.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split(' ').collect();
        match f[0] {
            "parse" => {
                // parse <name> <format> <max_bytes> <max_depth> <lines> <count>
                let name = f[1];
                let fmt = format_from(f[2]);
                let max_bytes: i64 = f[3].parse().unwrap();
                let max_depth: i32 = f[4].parse().unwrap();
                let lines = f[5] == "1";
                let expected_count: i32 = f[6].parse().unwrap();
                let input = read(&format!("{name}.in"));
                let limits = Limits::from_cpp(max_bytes, max_depth);
                let mode = if lines {
                    XmlMode::Lines
                } else {
                    XmlMode::Parse
                };
                let mut src = vkstorm_llsd::SliceSource::new(&input);
                let mut r = vkstorm_llsd::Reader::new(&mut src);
                let mut b = vkstorm_llsd::ValueBuilder::new(vkstorm_llsd::key_policy(fmt));
                let n = vkstorm_llsd::parse_into(fmt, &mut r, &mut b, limits, mode);
                assert_eq!(
                    n,
                    expected_count,
                    "{name}: count ({})",
                    String::from_utf8_lossy(&input[..input.len().min(80)])
                );
                if expected_count > 0 {
                    let expected = value_from_binary(&read(&format!("{name}.out")));
                    let got = b.finish().expect("value");
                    assert_eq!(got, expected, "{name}: value");
                }
                counts.parse += 1;
            }
            "format" => {
                // format <name> <value> <format> <options> <boolalpha> <real_format> <count>
                let name = f[1];
                let value = value_from_binary(&read(&format!("{}.val", f[2])));
                let fmt = format_from(f[3]);
                let options: u32 = f[4].parse().unwrap();
                let boolalpha = f[5] == "1";
                let real_format = f[6];
                let expected_count: i32 = f[7].parse().unwrap();
                let expected = read(&format!("{name}.expect"));
                let opts = Options {
                    pretty: options & 1 != 0,
                    pretty_binary: options & 2 != 0,
                    boolalpha,
                    precision: 6,
                    floatfield: FloatField::General,
                };
                let (got, n) = if real_format == "-" {
                    format(fmt, &value, opts)
                } else {
                    // The viewer pre-formats reals with its realFormat through
                    // llformat(); do the same with the C runtime's printf.
                    let mut fm = Formatter::new(fmt, opts, Vec::new());
                    emit_with_real_format(&value, &mut fm, real_format);
                    let (n, out) = fm.finish();
                    (out, n)
                };
                assert_eq!(n, expected_count, "{name}: count");
                assert!(
                    got == expected,
                    "{name}: bytes\n got: {}\n want: {}",
                    String::from_utf8_lossy(&got[..got.len().min(200)]),
                    String::from_utf8_lossy(&expected[..expected.len().min(200)])
                );
                counts.format += 1;
            }
            other => panic!("manifest line {other}"),
        }
    }
    counts
}

/// `printf(real_format, v)` for the formats the fixtures use.
fn printf_real(real_format: &str, v: f64) -> Vec<u8> {
    let mut out = Vec::new();
    match real_format {
        "%.2f" => vkstorm_llsd::text::fmt_f(v, 2, &mut out),
        "%.0f" => vkstorm_llsd::text::fmt_f(v, 0, &mut out),
        "%e" => vkstorm_llsd::text::fmt_e(v, 6, &mut out),
        other => panic!("real format {other}"),
    }
    out
}

fn emit_with_real_format(v: &Value, f: &mut Formatter<Vec<u8>>, real_format: &str) {
    match v {
        Value::Real(r) => f.real_text(*r, &printf_real(real_format, *r)),
        Value::Map(m) => {
            f.begin_map(m.len());
            for (k, v) in m {
                f.key(k);
                emit_with_real_format(v, f, real_format);
            }
            f.end_map();
        }
        Value::Array(a) => {
            f.begin_array(a.len());
            for v in a {
                emit_with_real_format(v, f, real_format);
            }
            f.end_array();
        }
        other => other.emit(f),
    }
}

#[test]
fn oracle_fixtures() {
    let counts = run();
    assert!(counts.parse > 300, "parse cases: {}", counts.parse);
    assert!(counts.format > 2000, "format cases: {}", counts.format);
}
