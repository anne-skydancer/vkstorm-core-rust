//! Parity with the C library on the committed fixtures: every fixture is
//! decoded in every trace mode (the viewer's `ov_read` loop and variants,
//! the packet API with two read sizes) and the trace (all return values,
//! stream information, comments and a hash of the output) must equal the
//! one the C oracle (`tools/vorbis-oracle/c/vorbis_oracle.c`) wrote with
//! the viewer's libogg 1.3.5/libvorbis 1.3.7 on this platform:
//! `expected-windows-msvc.txt` from the prebuilt MSVC x64 package,
//! `expected-linux-x86_64.txt` from the same sources built with the
//! package's Linux flags (gcc 14 and 15 give the same traces).
//!
//! The full corpus comparison against a live C oracle is opt-in (it needs
//! the C oracle and the generated corpus): set `VORBIS_C_ORACLE` and
//! `VORBIS_CORPUS` and run `cargo test -p vkstorm-vorbis -- --ignored`.

mod trace;

use std::fs;
use std::path::Path;

use trace::{Mode, parse_expected, platform_expected_name, run_compare};

fn fixtures_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"))
}

#[test]
fn fixtures_match_the_c_library() {
    let Some(name) = platform_expected_name() else {
        eprintln!("no C reference traces for this platform; skipped");
        return;
    };
    let text = fs::read_to_string(fixtures_dir().join(name)).expect("expected traces");
    let cases = parse_expected(&text);
    assert!(cases.len() > 100, "too few fixture traces: {}", cases.len());
    let mut failures = Vec::new();
    for (file, mode_name, expected) in &cases {
        let mode = Mode::parse(mode_name).expect("mode name");
        let data = fs::read(fixtures_dir().join(file)).expect("fixture file");
        let got = mode.trace(&data);
        if &got != expected {
            failures.push(format!(
                "{file} [{mode_name}]: {}",
                trace::first_diff(expected, &got)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} traces differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

#[test]
fn every_fixture_has_every_mode() {
    let Some(name) = platform_expected_name() else {
        return;
    };
    let text = fs::read_to_string(fixtures_dir().join(name)).expect("expected traces");
    let cases = parse_expected(&text);
    let mut files = Vec::new();
    trace::collect_ogg(fixtures_dir(), &mut files);
    assert_eq!(
        cases.len(),
        files.len() * Mode::all().len(),
        "regenerate the expected traces after changing fixtures"
    );
}

#[test]
#[ignore = "needs the C oracle (VORBIS_C_ORACLE) and a generated corpus (VORBIS_CORPUS)"]
fn corpus_matches_a_live_c_oracle() {
    let (Ok(oracle), Ok(corpus)) = (
        std::env::var("VORBIS_C_ORACLE"),
        std::env::var("VORBIS_CORPUS"),
    ) else {
        eprintln!("VORBIS_C_ORACLE / VORBIS_CORPUS not set; skipped");
        return;
    };
    let work = std::env::temp_dir().join(format!("vkstorm-vorbis-parity-{}", std::process::id()));
    let jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
    let ok = run_compare(&oracle, &corpus, &work.to_string_lossy(), jobs, false)
        .expect("comparison ran");
    let _ = fs::remove_dir_all(&work);
    assert!(
        ok,
        "the port differs from the C library (see the list above)"
    );
}
