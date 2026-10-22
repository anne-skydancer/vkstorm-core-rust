//! Sound asset decoding against the viewer's `LLVorbisDecodeState`, without
//! the C oracle: every Ogg file of vkstorm-vorbis's fixture corpus decodes
//! to the same cache file bytes (FNV-1a 64), or fails with the same error,
//! as `tools/soloud-oracle/c/asset_oracle.c` over the viewer's libvorbis
//! (`tools/soloud-oracle asset-expected`). The libvorbis builds of the two
//! platforms differ in the last bit on some streams, hence two lists.

use vkstorm_audio::asset::{
    AssetDecodeError, CLIP_REJECT_SAMPLES, CLIP_REJECT_SIZE, decode_sound_asset,
};

#[cfg(windows)]
const EXPECTED: &str = include_str!("mixer_expected/asset-windows-msvc.txt");
#[cfg(not(windows))]
const EXPECTED: &str = include_str!("mixer_expected/asset-linux-x86_64.txt");

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[test]
fn fixture_corpus_matches_viewer_decode() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../vkstorm-vorbis/tests/fixtures");
    let mut failures = Vec::new();
    let mut n = 0;
    for line in EXPECTED
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let f: Vec<&str> = line.splitn(3, ' ').collect();
        let data = std::fs::read(root.join(f[0])).expect("fixture");
        let got = decode_sound_asset(&data);
        let ok = match (&got, f[1]) {
            (Err(e), "error") => e.code_name() == f[2],
            (Ok(w), size) => w.len().to_string() == size && format!("{:016x}", fnv1a(w)) == f[2],
            _ => false,
        };
        if !ok {
            failures.push(format!("{}: {:?}", f[0], got.map(|w| (w.len(), fnv1a(&w)))));
        }
        n += 1;
    }
    assert!(n > 200);
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn limits_are_the_viewers() {
    assert_eq!(CLIP_REJECT_SAMPLES, 7_938_000);
    assert_eq!(CLIP_REJECT_SIZE, 15_876_000);
}

#[test]
fn header_lengths_and_head_fade() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../vkstorm-vorbis/tests/fixtures/clean/viewer");
    let mut checked = 0;
    for entry in std::fs::read_dir(&root).expect("viewer fixtures") {
        let p = entry.expect("entry").path();
        let Ok(w) = decode_sound_asset(&std::fs::read(&p).expect("read")) else {
            continue;
        };
        // Always "mono 44100 16-bit", whatever the stream.
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(
            &w[20..36],
            &[1, 0, 1, 0, 0x44, 0xAC, 0, 0, 0x88, 0x58, 1, 0, 2, 0, 16, 0]
        );
        let data_len = u32::from_le_bytes([w[40], w[41], w[42], w[43]]) as usize;
        assert_eq!(data_len, w.len() - 44);
        assert_eq!(
            u32::from_le_bytes([w[4], w[5], w[6], w[7]]) as usize,
            data_len + 36
        );
        // The first sample is scaled by 0/fade_length (no fade below 8 bytes).
        if data_len >= 8 {
            assert_eq!(&w[44..46], &[0, 0], "{}", p.display());
        }
        checked += 1;
    }
    assert!(checked > 0);
}

#[test]
fn errors() {
    assert_eq!(decode_sound_asset(&[]), Err(AssetDecodeError::EmptyAsset));
    assert!(matches!(
        decode_sound_asset(b"not an ogg file at all"),
        Err(AssetDecodeError::NotOgg(_))
    ));
}
