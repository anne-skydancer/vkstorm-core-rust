//! The harnesses' reproduction of `std::unordered_map<LLUUID, T>` order
//! (`support_unordered`) against the real standard library. Runs when
//! `VKS_CXX` names a C++ compiler (CI sets it: clang++ on Windows, g++ on
//! Linux): the probe `tests/cxx/unordered_order_probe.cpp` is compiled with
//! it and fed the same insert, erase and clear sequences.

#[path = "support_unordered/mod.rs"]
mod support_unordered;

use std::path::PathBuf;
use std::process::{Command, Stdio};

use support_unordered::{Library, UnorderedOrder, Uuid};

fn hex(id: &Uuid) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

/// xorshift64* for the sequences.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
}

#[test]
fn matches_the_standard_library() {
    let Ok(cxx) = std::env::var("VKS_CXX") else {
        eprintln!("VKS_CXX not set: unordered_map order not compared with a C++ standard library");
        return;
    };
    let library = Library::native();
    let dir = std::env::temp_dir();
    let exe = dir.join(if cfg!(windows) {
        "unordered_order_probe.exe"
    } else {
        "unordered_order_probe"
    });
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/cxx/unordered_order_probe.cpp");
    let st = Command::new(&cxx)
        .args(["-std=c++17", "-O1", "-o"])
        .arg(&exe)
        .arg(&src)
        .status()
        .expect("cannot run VKS_CXX");
    assert!(st.success(), "probe failed to compile");

    // One container for the whole script, as the probe keeps one (clear
    // keeps MSVC's bucket count). Patterned ids like the fixtures' (shared
    // prefix, low bytes varying) and random ones; erases and prints between.
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut script = String::new();
    let mut ord = UnorderedOrder::new(library);
    let mut want: Vec<String> = Vec::new();
    let line = |o: &UnorderedOrder| o.order().iter().map(hex).collect::<Vec<_>>().join(" ");
    for round in 0..40 {
        let mut live: Vec<Uuid> = Vec::new();
        let n = [5, 9, 64, 65, 300, 513, 600, 4200][round % 8];
        for step in 0..n {
            let mut id = [0u8; 16];
            if rng.next() % 2 == 0 {
                id[0] = 0xc0;
                id[14] = (rng.next() % 4) as u8;
                id[15] = rng.next() as u8;
            } else {
                for x in id.iter_mut() {
                    *x = rng.next() as u8;
                }
            }
            ord.insert(id);
            if !live.contains(&id) {
                live.push(id);
            }
            script.push_str(&format!("i {}\n", hex(&id)));
            if step % 7 == 6 {
                let k = live.remove((rng.next() % live.len() as u64) as usize);
                ord.erase(&k);
                script.push_str(&format!("e {}\n", hex(&k)));
            }
            if step % 50 == 49 {
                script.push_str("p\n");
                want.push(line(&ord));
            }
        }
        script.push_str("p\nc\n");
        want.push(line(&ord));
        ord.clear();
    }
    let script_path = dir.join("unordered_order_probe.txt");
    std::fs::write(&script_path, &script).unwrap();
    let out = Command::new(&exe)
        .stdin(std::fs::File::open(&script_path).unwrap())
        .stdout(Stdio::piped())
        .output()
        .expect("cannot run the probe");
    assert!(out.status.success(), "probe failed: {:?}", out.status);
    let got: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(got.len(), want.len());
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        assert_eq!(g, w, "print {i} differs");
    }
    eprintln!(
        "{} orders compared with the C++ standard library",
        want.len()
    );
}
