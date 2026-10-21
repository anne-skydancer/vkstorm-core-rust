//! Behaviour of the public API on committed fixtures: file and packet
//! decoding agree, stream information, comment queries, chained links,
//! error values.

use std::fs;
use std::path::Path;

use vkstorm_vorbis::ogg::{PacketOut, Page, StreamState, SyncState};
use vkstorm_vorbis::{Decoder, Error, OV_EINVAL, OV_ENOTVORBIS, VorbisFile};

fn fixture(name: &str) -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Decodes with the viewer's call sequence; returns (pcm, returns).
fn read_all(vf: &mut VorbisFile) -> (Vec<u8>, Vec<isize>) {
    let mut pcm = Vec::new();
    let mut rets = Vec::new();
    let mut buf = [0u8; 4096];
    let mut section = 0;
    loop {
        let r = vf.read(&mut buf, false, 2, true, &mut section);
        rets.push(r);
        if r <= 0 {
            break;
        }
        pcm.extend_from_slice(&buf[..r as usize]);
    }
    (pcm, rets)
}

/// Decodes every logical stream with the packet API and converts the
/// floats as `ov_read` does (round half to even, clip to 16 bits).
fn packets_to_s16(data: &[u8]) -> Vec<u8> {
    let mut oy = SyncState::new();
    oy.write(data);
    let mut page = Page::new();
    let mut os: Option<StreamState> = None;
    let mut dec = Decoder::new();
    let mut headers = 0;
    let mut out = Vec::new();
    while oy.pageout(&mut page) != 0 {
        if page.bos() {
            os = Some(StreamState::new(page.serialno()));
            dec = Decoder::new();
            headers = 0;
        }
        let Some(st) = os.as_mut() else { continue };
        st.pagein(&page);
        loop {
            let p = match st.packetout() {
                PacketOut::None => break,
                PacketOut::Hole => continue,
                PacketOut::Packet(p) => p,
            };
            if headers < 3 {
                dec.headerin_packet(&p).expect("header");
                headers += 1;
                continue;
            }
            dec.synthesis(&p).expect("audio packet");
            let pcm = dec.pcmout();
            for i in 0..pcm.len() {
                for c in 0..pcm.channels() {
                    let v = ((pcm.channel(c)[i] * 32768.0) as f64)
                        .round_ties_even()
                        .clamp(-32768.0, 32767.0) as i16;
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
            let n = pcm.len();
            dec.read(n).expect("read");
        }
    }
    out
}

#[test]
fn viewer_asset_decodes_to_its_length() {
    let data = fixture("clean/viewer/v16m_mix_0044100.ogg");
    let mut vf = VorbisFile::open(&data).expect("open");
    assert_eq!(vf.streams(), 1);
    assert_eq!(vf.pcm_total(-1), 44100);
    let vi = vf.info(-1).expect("info");
    assert_eq!((vi.channels, vi.rate), (1, 44100));
    let (pcm, rets) = read_all(&mut vf);
    assert_eq!(pcm.len(), 44100 * 2);
    assert_eq!(*rets.last().unwrap_or(&-1), 0);
    assert!(rets.iter().all(|&r| (0..=4096).contains(&r)));
    // after the end: EOF again
    let mut buf = [0u8; 4096];
    let mut s = 0;
    assert_eq!(vf.read(&mut buf, false, 2, true, &mut s), 0);
}

#[test]
fn packet_api_matches_ov_read_for_single_links() {
    for name in [
        "clean/viewer/v16m_mix_0044100.ogg",
        "clean/viewer/v16s_mix_0044100.ogg",
    ] {
        let data = fixture(name);
        let mut vf = VorbisFile::open(&data).expect("open");
        let (pcm, _) = read_all(&mut vf);
        assert_eq!(packets_to_s16(&data), pcm, "{name}");
    }
}

#[test]
fn comments_and_case_insensitive_queries() {
    let data = fixture("clean/comments/several.ogg");
    let vf = VorbisFile::open(&data).expect("open");
    let vc = vf.comment(-1).expect("comment");
    assert_eq!(vc.query_str("title", 0).as_deref(), Some("Test title"));
    assert_eq!(vc.query_str("ARTIST", 0).as_deref(), Some("Vulkanstorm"));
    assert_eq!(vc.query("ARTIST", 1), None);
    assert_eq!(vc.query_count("tracknumber"), 1);
    assert!(vc.vendor_str().starts_with("Xiph.Org libVorbis"));

    let data = fixture("clean/comments/mixed_case.ogg");
    let vf = VorbisFile::open(&data).expect("open");
    let vc = vf.comment(0).expect("comment");
    assert_eq!(vc.query_count("TITLE"), 2);
    assert_eq!(vc.query_str("TITLE", 1).as_deref(), Some("second title"));
    assert_eq!(vc.query_str("Artist", 0).as_deref(), Some("lower"));
}

#[test]
fn chained_links_report_their_own_parameters() {
    let data = fixture("clean/chained/chain_44m_22s_8m.ogg");
    let mut vf = VorbisFile::open(&data).expect("open");
    assert_eq!(vf.streams(), 3);
    let params: Vec<(i32, i64, i64)> = (0..3)
        .map(|l| {
            (
                vf.info(l).map_or(0, |i| i.channels),
                vf.info(l).map_or(0, |i| i.rate),
                vf.pcm_total(l),
            )
        })
        .collect();
    assert_eq!(
        params,
        vec![(1, 44100, 22050), (2, 22050, 11025), (1, 8000, 4000)]
    );
    assert!(vf.info(3).is_none());
    assert_eq!(vf.pcm_total(3), OV_EINVAL as i64);
    // libvorbisfile 1.3.7's seekable open of this chain resumes link 0 with
    // its packets already consumed by the initial ov_raw_seek scan (the C
    // does the same), so reading starts in link 1
    let mut buf = [0u8; 4096];
    let mut section = -1;
    let mut sections = Vec::new();
    loop {
        let r = vf.read(&mut buf, false, 2, true, &mut section);
        if r <= 0 {
            break;
        }
        if sections.last() != Some(&section) {
            sections.push(section);
        }
    }
    assert_eq!(sections, vec![1, 2]);
}

#[test]
fn non_vorbis_input_is_rejected() {
    assert_eq!(
        VorbisFile::open(&[]).err(),
        Some(Error::from_code(OV_ENOTVORBIS).unwrap())
    );
    assert_eq!(
        VorbisFile::open(b"OggS this is not a page").err(),
        Some(Error::NotVorbis)
    );
    let mut dec = Decoder::new();
    assert_eq!(dec.headerin(b"\x01vorbix", true), Err(Error::NotVorbis));
    assert_eq!(dec.headerin(b"\x03vorbis", true), Err(Error::BadHeader));
    assert!(!dec.headers_complete());
    assert_eq!(dec.synthesis_init(), Err(Error::Inval));
}

#[test]
fn error_codes_round_trip() {
    for code in [
        -1, -2, -3, -128, -129, -130, -131, -132, -133, -134, -135, -136, -137, -138,
    ] {
        assert_eq!(Error::from_code(code).map(Error::code), Some(code));
    }
    assert_eq!(Error::from_code(0), None);
}

#[test]
fn eight_bit_and_big_endian_packing() {
    let data = fixture("clean/viewer/v16s_mix_0044100.ogg");
    let mut a = VorbisFile::open(&data).expect("open");
    let mut b = VorbisFile::open(&data).expect("open");
    let mut s = 0;
    let mut le = [0u8; 4000];
    let mut be = [0u8; 4000];
    let n = a.read(&mut le, false, 2, true, &mut s);
    let m = b.read(&mut be, true, 2, true, &mut s);
    assert_eq!(n, m);
    for i in (0..n as usize).step_by(2) {
        assert_eq!((le[i], le[i + 1]), (be[i + 1], be[i]));
    }
    // a buffer shorter than one frame is an error, as in the C
    let mut tiny = [0u8; 1];
    assert_eq!(
        a.read(&mut tiny, false, 2, true, &mut s),
        OV_EINVAL as isize
    );
}
