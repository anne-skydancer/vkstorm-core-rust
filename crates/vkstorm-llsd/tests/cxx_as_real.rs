//! `string_as_real_native` against the real C++ runtime.
//!
//! Runs only when `VKS_CXX` names a C++ compiler: it compiles
//! `tests/cxx/as_real_probe.cpp` (the viewer's `ImplString::asReal`,
//! `indra/llcommon/llsd.cpp`) with it, feeds it a fixed set of edge strings
//! and a seeded random set, and requires every result to match this crate's
//! value bit for bit. With clang++ or MSVC on Windows that checks the MSVC
//! form; with g++ on Linux, the libstdc++ form.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use vkstorm_llsd::text::string_as_real_native;

const EDGES: &[&str] = &[
    "", " ", "\t", "\n", "\r\n", "0", "-0", "+0", "00", "007.5", "1", "-1", "+1", "--1", "+-1", "-+1", " 1", "1 ",
    "\t1", "\n1", "1\n", " 1 ", ".", "-.", "+.", ".5", "-.5", "5.", "5..", "1.2.3", "..5", "1e", "1E", "1e+", "1e-",
    "1e5", "1E5", "1e+5", "1e-5", "1e5x", "1e5.", "1e5e", "1.e5", ".e5", "e5", "-e5", "1ee5", "1e+-5", "0x", "0X",
    "0x10", "-0x10", "+0x10", "0x1p3", "0X1P-2", "0x1.8p1", "0xg", "0x.", "0x1p", "0x1p+", "0x10 ", "x10", "inf",
    "-inf", "+inf", "INF", "infinity", "Infinity", "nan", "NAN", "nan(1)", "-nan", "1e999", "-1e999", "1e308",
    "1.7976931348623157e308", "1.7976931348623158e308", "1.7976931348623159e308", "1.8e308", "-1.8e308",
    "1e-307", "2.2250738585072014e-308", "2.5e-308", "1e-310", "4.9e-324", "5e-324", "2.5e-324", "2.4703282292062328e-324",
    "2e-324", "1e-400", "1e-999", "-1e-999", "123456789012345678901234567890", "0.1", "0.30000000000000004",
    "9007199254740993", "1.00000000000000011102230246251565404236316680908203125", "2147483647", "2147483648",
    "-2147483649", "4294967296", "1,5", "1_000", "١", "\u{a0}1", "1\0", "\0", "7", "x7", "0x7",
];

/// xorshift64*, as the inventory corpora's generators.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

fn inputs() -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = EDGES.iter().map(|s| s.as_bytes().to_vec()).collect();
    for n in [17usize, 40, 309, 400, 800] {
        v.push(vec![b'9'; n]);
        let mut s = b"0.".to_vec();
        s.extend(std::iter::repeat_n(b'0', n));
        s.push(b'1');
        v.push(s);
        let mut s = b"1".to_vec();
        s.extend(std::iter::repeat_n(b'0', n));
        s.extend_from_slice(b"e-");
        s.extend_from_slice(n.to_string().as_bytes());
        v.push(s);
    }
    const ALPHABET: &[u8] = b"0123456789000111..eEeE+-+-xXpPinfaINFAN \t\n\r";
    let mut rng = Rng(0x5EED_C0E2_2026_1028);
    for _ in 0..30_000 {
        let len = (rng.next() % 14) as usize;
        let s = (0..len).map(|_| ALPHABET[(rng.next() % ALPHABET.len() as u64) as usize]).collect();
        v.push(s);
    }
    // Numbers that look ordinary, to exercise rounding.
    for _ in 0..10_000 {
        let mantissa = rng.next() % 10_000_000_000_000_000;
        let exp = (rng.next() % 700) as i64 - 350;
        let point = (rng.next() % 18) as usize;
        let mut digits = mantissa.to_string();
        if point < digits.len() {
            digits.insert(digits.len() - point, '.');
        }
        v.push(format!("{digits}e{exp}").into_bytes());
    }
    v
}

#[test]
fn string_as_real_matches_cxx_runtime() {
    let Ok(cxx) = std::env::var("VKS_CXX") else {
        eprintln!("VKS_CXX not set; skipping the C++ runtime comparison");
        return;
    };
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src = manifest.join("tests/cxx/as_real_probe.cpp");
    let out_dir = std::env::temp_dir().join(format!("vks_as_real_probe_{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let exe = out_dir.join(if cfg!(windows) { "as_real_probe.exe" } else { "as_real_probe" });
    let status = Command::new(&cxx)
        .args(["-O1", "-std=c++17", "-o"])
        .arg(&exe)
        .arg(&src)
        .status()
        .unwrap_or_else(|e| panic!("cannot run {cxx}: {e}"));
    assert!(status.success(), "{cxx} failed to compile the probe");

    let inputs = inputs();
    let mut stdin_text = String::new();
    for s in &inputs {
        for b in s {
            stdin_text.push_str(&format!("{b:02x}"));
        }
        stdin_text.push('\n');
    }
    let mut child = Command::new(&exe).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(stdin_text.as_bytes()).unwrap());
    let output = child.wait_with_output().unwrap();
    writer.join().unwrap();
    assert!(output.status.success());
    let _ = std::fs::remove_dir_all(&out_dir);

    let lines: Vec<&str> = std::str::from_utf8(&output.stdout).unwrap().lines().collect();
    assert_eq!(lines.len(), inputs.len(), "probe output line count");
    let mut mismatches = Vec::new();
    for (s, line) in inputs.iter().zip(&lines) {
        let expected = u64::from_str_radix(line.trim(), 16).unwrap();
        let actual = string_as_real_native(s).to_bits();
        if expected != actual {
            mismatches.push(format!(
                "{:?}: C++ {:016x} ({}), crate {:016x} ({})",
                String::from_utf8_lossy(s),
                expected,
                f64::from_bits(expected),
                actual,
                f64::from_bits(actual)
            ));
        }
    }
    eprintln!("compared {} strings against {cxx}", inputs.len());
    assert!(
        mismatches.is_empty(),
        "{} of {} strings differ:\n{}",
        mismatches.len(),
        inputs.len(),
        mismatches.iter().take(40).cloned().collect::<Vec<_>>().join("\n")
    );
}
