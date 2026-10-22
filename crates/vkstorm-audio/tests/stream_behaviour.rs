//! Stream behaviour end to end against the mock radio server: ICY
//! metadata (split, dedup, encodings, the `ICY 200 OK` status line),
//! playlists, refusals with reasons (video, web pages, HLS, unsupported
//! containers, HTTP errors), reconnects, the status-json side channel,
//! buffering under a slow start and jitter (no rate change at the start),
//! underruns (silence, never stretched time), pause, device rate changes and
//! stop.
//!
//! Needs `--features link-curl`.

#![cfg(feature = "link-curl")]

mod stream_support;

use std::time::{Duration, Instant};

use stream_support::*;
use vkstorm_audio::music::MusicSource;
use vkstorm_audio::stream::resample::expected_out_frames;
use vkstorm_audio::stream::{MusicPlayer, PlayerEvent, PlayerState, StatusPolicy, StreamConfig};

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Plays `path` to its end (pulling as fast as possible).
fn play_to_end(player: &MusicPlayer, url: &str, limit: Duration) -> Pulled {
    player.start(url);
    let mut reader = player.reader();
    pull(&mut reader, 480, None, limit)
}

fn assert_exact_accounting(player: &MusicPlayer, p: &Pulled, rate: u32) {
    let stats = player.stats();
    let mut total = 0;
    for s in &stats.segments {
        assert_eq!(s.out_frames, expected_out_frames(s.in_frames, s.source_rate, rate), "segment {s:?}");
        total += s.out_frames;
    }
    assert_eq!(p.frames, total, "pulled frames");
}

/// The left channel's tone over 3 s from the start (a looped MP3 has the
/// encoder's delay and padding at each seam).
fn left_tone_ok(p: &Pulled, rate: u32) {
    let x = p.channel(2, 0);
    let x = trim_start(&x);
    let r = rate as usize;
    let f = frequency(&x[r / 4..(r / 4 + 3 * r).min(x.len() - r / 4)], rate);
    assert!((f - 440.0).abs() < 0.5, "left channel at {f} Hz");
}

#[test]
fn icy_metadata_split_dedup_and_clean_audio() {
    let srv = server();
    let player = MusicPlayer::new(config(48000));
    let url = srv.url(&format!(
        "/live/tone.mp3?icy=1&metaint=3000&every=3&repeat=1&loops=2&length=1&key=icy1&title={}&title={}",
        enc("Artist One - Song A"),
        enc("Song Without Artist")
    ));
    let p = play_to_end(&player, &url, Duration::from_secs(60));
    assert!(p.ended);
    assert_eq!(player.state(), PlayerState::Ended);
    // The metadata blocks were stripped: the audio is the clean tone and
    // every decoded frame arrived.
    left_tone_ok(&p, 48000);
    assert_exact_accounting(&player, &p, 48000);
    let ev = events(&player);
    assert!(ev.len() >= 4, "{ev:?}");
    assert_eq!(ev[0], meta("Artist One", "Song A"));
    assert_eq!(ev[1], meta("", "Song Without Artist"));
    assert_eq!(ev[2], meta("Artist One", "Song A"));
    // Repeated blocks with the same title are not reported again.
    assert!(ev.windows(2).all(|w| w[0] != w[1]), "{ev:?}");
    // The request asked for ICY metadata and named the viewer.
    let log = srv.log();
    assert!(log.contains("\"query\": \"icy=1&metaint=3000&every=3&repeat=1&loops=2&length=1&key=icy1"), "{log}");
    assert!(log.contains("\"ua\": \"Vulkanstorm-test\", \"icy\": \"1\""), "{log}");
}

#[test]
fn icy_200_ok_status_line_and_ogg_comments() {
    let srv = server();
    let player = MusicPlayer::new(config(48000));
    let url = srv.url(&format!(
        "/live/tone.ogg?status=icy&icy=1&metaint=4000&length=1&key=icy2&title={}",
        enc("Icy Artist - Icy Title")
    ));
    let p = play_to_end(&player, &url, Duration::from_secs(60));
    assert!(p.ended, "{:?}", player.state());
    assert_eq!(player.state(), PlayerState::Ended);
    assert_eq!(player.stats().codec, "vorbis");
    left_tone_ok(&p, 48000);
    assert_exact_accounting(&player, &p, 48000);
    let ev = events(&player);
    assert!(ev.contains(&meta("Plain Artist", "Plain Title")), "{ev:?}");
    assert!(ev.contains(&meta("Icy Artist", "Icy Title")), "{ev:?}");
}

#[test]
fn icy_latin1_titles() {
    let srv = server();
    let player = MusicPlayer::new(config(48000));
    let url = srv.url(&format!("/live/tone_mono22.mp3?icy=1&metaint=2000&length=1&enc=latin1&key=icy3&title={}", enc("Café - Crème")));
    let p = play_to_end(&player, &url, Duration::from_secs(60));
    assert!(p.ended);
    assert_eq!(events(&player), vec![meta("Café", "Crème")]);
}

#[test]
fn playlists_try_entries_in_order() {
    let srv = server();
    let player = MusicPlayer::new(config(48000));
    // PLS: the first entry is a 404, the second plays.
    let url = srv.url(&format!("/playlist.pls?e={}&e={}", enc("/status/404"), enc("/media/tone.ogg")));
    let p = play_to_end(&player, &url, Duration::from_secs(60));
    assert!(p.ended);
    assert_eq!(player.stats().codec, "vorbis");
    left_tone_ok(&p, 48000);
    // M3U whose entry redirects.
    let url = srv.url(&format!("/playlist.m3u?e={}", enc("/redirect?to=/media/tone.flac")));
    let p = play_to_end(&player, &url, Duration::from_secs(60));
    assert!(p.ended);
    assert_eq!(player.state(), PlayerState::Ended);
    assert_eq!(player.stats().codec, "flac");
    // Nothing playable.
    let url = srv.url(&format!("/playlist.m3u?e={}&e={}", enc("/status/404"), enc("/media/page.html")));
    let p = play_to_end(&player, &url, Duration::from_secs(60));
    assert!(p.ended && p.frames == 0);
    match player.state() {
        PlayerState::Failed(r) => assert!(r.contains("no playlist entry") && r.contains("web page"), "{r}"),
        s => panic!("{s:?}"),
    }
}

#[test]
fn refusals_with_reasons() {
    let srv = server();
    let cases = [
        ("/media/video.mp4", "video"),
        ("/media/video.ogv", "video (Ogg with Theora)"),
        ("/media/page.html", "web page"),
        ("/media/tone.m4a", "MP4/M4A"),
        ("/hls.m3u8", "HLS"),
        ("/status/404", "404"),
        ("/status/503", "503"),
        ("/media/missing.mp3", "404"),
        ("/media/tone.ogg?ct=video/ogg&key=x", ""), // a mislabelled audio stream plays
        ("/media/tone.mp3?ct=text/html&key=y", ""),
    ];
    for (path, reason) in cases {
        let player = MusicPlayer::new(config(48000));
        let p = play_to_end(&player, &srv.url(path), Duration::from_secs(30));
        assert!(p.ended, "{path}");
        let state = player.state();
        if reason.is_empty() {
            assert_eq!(state, PlayerState::Ended, "{path}");
            assert!(p.frames > 0);
            continue;
        }
        assert_eq!(p.frames, 0, "{path}: audio from a refused stream");
        match state {
            PlayerState::Failed(r) => assert!(r.contains(reason), "{path}: {r}"),
            s => panic!("{path}: {s:?}"),
        }
        assert_eq!(player.is_playing(), 0);
    }
    // Nothing listening.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = dead.local_addr().expect("addr").port();
    drop(dead);
    let player = MusicPlayer::new(config(48000));
    play_to_end(&player, &format!("http://127.0.0.1:{port}/x.mp3"), Duration::from_secs(30));
    match player.state() {
        PlayerState::Failed(r) => assert!(r.starts_with("cannot connect"), "{r}"),
        s => panic!("{s:?}"),
    }
    // Not http(s).
    play_to_end(&player, "ftp://127.0.0.1/x.mp3", Duration::from_secs(5));
    assert!(matches!(player.state(), PlayerState::Failed(r) if r.contains("unsupported URL")));
}

fn reconnect_config() -> StreamConfig {
    StreamConfig { reconnect_delays: vec![ms(300), ms(300)], ..config(48000) }
}

#[test]
fn reconnect_after_a_drop_continues() {
    let srv = server();
    let player = MusicPlayer::new(reconnect_config());
    // The first connection drops after 40000 bytes (a reset); the second
    // plays to a clean end.
    let p = play_to_end(&player, &srv.url("/live/tone.mp3?drop=40000&key=rc1&lengthafter=1"), Duration::from_secs(60));
    assert!(p.ended);
    assert_eq!(player.state(), PlayerState::Ended);
    let stats = player.stats();
    assert_eq!(stats.reconnects, 1);
    assert_eq!(stats.segments.len(), 2, "{:?}", stats.segments);
    assert!(stats.segments[0].in_frames < stats.segments[1].in_frames);
    assert_exact_accounting(&player, &p, 48000);
    let log = srv.log();
    assert_eq!(log.matches("key=rc1").count(), 2, "{log}");
}

#[test]
fn reconnect_gives_up_with_a_reason() {
    let srv = server();
    let player = MusicPlayer::new(reconnect_config());
    let p = play_to_end(&player, &srv.url("/live/tone.mp3?drop=40000&key=rc2&maxconn=1"), Duration::from_secs(60));
    assert!(p.ended);
    assert!(p.frames > 0, "the first connection's audio played");
    let stats = player.stats();
    assert_eq!(stats.reconnects, 2);
    match player.state() {
        PlayerState::Failed(r) => assert!(r.contains("could not be reconnected") && r.contains("503"), "{r}"),
        s => panic!("{s:?}"),
    }
}

#[test]
fn slow_start_and_jitter_no_rate_change() {
    let srv = server();
    let player = MusicPlayer::new(config(48000));
    // 1.5 s before the server answers, a 16 KB burst, then real time with
    // a 0.7 s stall; the mixer pulls 10 ms blocks in real time.
    let url = srv.url("/live/tone.mp3?slowstart=1500&pace=realtime&burst=16000&stall=700@40000&loops=2&length=1&key=slow1");
    player.start(&url);
    let mut reader = player.reader();
    let p = pull(&mut reader, 480, Some(ms(10)), Duration::from_secs(40));
    assert!(p.ended, "{:?}", player.state());
    let first = p.first_audio.expect("audio");
    assert!(first >= ms(1500), "audio started after {first:?}");
    // Once started, the 3 s buffer rides out the stall: no silence.
    assert_eq!(player.stats().underruns, 0);
    assert_eq!(p.empty_after_start, 0);
    // The first second is at the right rate.
    let x = p.channel(2, 0);
    let x = trim_start(&x);
    let f = frequency(&x[..48000], 48000);
    assert!((f - 440.0).abs() < 0.5, "first second at {f} Hz");
    left_tone_ok(&p, 48000);
    assert_exact_accounting(&player, &p, 48000);
}

#[test]
fn underruns_give_silence_not_stretched_time() {
    let srv = server();
    let cfg = StreamConfig { start_buffer: ms(500), ..config(48000) };
    let player = MusicPlayer::new(cfg);
    // The server delivers at 0.6x real time: the ring runs dry repeatedly.
    player.start(&srv.url("/live/tone_mono22.mp3?pace=0.6&length=1&key=under1"));
    let mut reader = player.reader();
    let p = pull(&mut reader, 480, Some(ms(10)), Duration::from_secs(30));
    assert!(p.ended);
    assert!(player.stats().underruns > 0);
    assert!(p.empty_after_start > 0);
    // The frames that were delivered are contiguous: concatenated they are
    // the unbroken tone at its exact frequency (nothing skipped, repeated
    // or resampled differently).
    let x = p.channel(1, 0);
    let x = trim_start(&x);
    let f = frequency(&x[4800..x.len() - 4800], 48000);
    assert!((f - 440.0).abs() < 0.2, "{f} Hz");
    assert_exact_accounting(&player, &p, 48000);
}

#[test]
fn status_json_side_channel() {
    let srv = own_server();
    let path = format!(
        "/live/tone.mp3?icy=1&metaint=8000&pace=realtime&loops=4&length=1&title={}&title={}&every=2",
        enc("InBand - Song"),
        enc("InBand - Next")
    );
    // The C++ matched the listen URL against the stream URL's whole path,
    // query included; so does the port (the mock's options are a query).
    let (status, _) = srv.get(&format!(
        "/config/status?at={}&listen={}&title={}",
        enc("/status-json.xsl"),
        enc(&path),
        enc("Status Artist - Status Title")
    ));
    assert_eq!(status, 200);
    let player = MusicPlayer::new(config(48000));
    let t0 = Instant::now();
    player.start(&srv.url(&path));
    let mut reader = player.reader();
    let mut seen = Vec::new();
    // Play in real time for 9 s.
    let mut out = vec![0f32; 480 * 8];
    while t0.elapsed() < Duration::from_secs(9) {
        reader.pull(&mut out, 480);
        seen.extend(events(&player));
        std::thread::sleep(ms(10));
    }
    let pos_status = seen
        .iter()
        .position(|e| *e == meta("Status Artist", "Status Title"))
        .unwrap_or_else(|| panic!("no status metadata: {seen:?} {:?} {}", player.state(), srv.log()));
    assert!(seen[..pos_status].contains(&meta("InBand", "Song")), "{seen:?}");
    // The side channel wins: no in-band updates after it.
    assert!(seen[pos_status + 1..].is_empty(), "{seen:?}");
    // Requests: the walk up from /live/, at 5 s after the start.
    let log = srv.log();
    let t_of = |needle: &str| -> Option<f64> {
        let i = log.find(needle)?;
        let rest = &log[i..];
        let t = rest.find("\"t\": ")? + 5;
        rest[t..].split(['}', ',']).next()?.trim().parse().ok()
    };
    let t_stream = t_of("\"path\": \"/live/tone.mp3\"").expect("stream request");
    let t_first = t_of("\"path\": \"/live/status-json.xsl\"").expect("first candidate");
    let t_root = t_of("\"path\": \"/status-json.xsl\"").expect("second candidate");
    assert!(t_first - t_stream >= 4900.0, "first poll {} ms after the stream request", t_first - t_stream);
    assert!(t_root >= t_first);
    player.stop();
    assert_eq!(events(&player), vec![PlayerEvent::MetadataCleared]);
    assert_eq!(player.state(), PlayerState::Stopped);
}

#[test]
fn status_json_gives_up_after_three_failed_polls() {
    let srv = own_server();
    let cfg = StreamConfig {
        status: StatusPolicy { first_poll: ms(200), interval: ms(300), max_failures: 3 },
        ..config(48000)
    };
    let player = MusicPlayer::new(cfg);
    player.start(&srv.url("/live/tone.mp3?pace=realtime&length=1&key=gu1"));
    let mut reader = player.reader();
    pull(&mut reader, 480, Some(ms(10)), Duration::from_secs(4));
    let log = srv.log();
    // Three polls, each trying /live/status-json.xsl and /status-json.xsl.
    assert_eq!(log.matches("status-json.xsl").count(), 6, "{log}");
}

#[test]
fn pause_and_resume() {
    let srv = server();
    // A live stream: pause closes the connection, resume reconnects.
    let player = MusicPlayer::new(config(48000));
    player.start(&srv.url("/live/tone.mp3?pace=realtime&burst=200000&loops=10&key=pz1"));
    let st = wait_state(&player, Duration::from_secs(10), |s| *s == PlayerState::Playing);
    assert_eq!(st, PlayerState::Playing);
    let mut reader = player.reader();
    let mut out = vec![0f32; 480 * 8];
    assert!(reader.pull(&mut out, 480).frames > 0);
    player.pause(true);
    assert_eq!(player.state(), PlayerState::Paused);
    assert_eq!(player.is_playing(), 2);
    let r = reader.pull(&mut out, 480);
    assert_eq!((r.frames, r.ended), (0, false));
    player.pause(false);
    let st = wait_state(&player, Duration::from_secs(10), |s| *s == PlayerState::Playing);
    assert_eq!(st, PlayerState::Playing);
    assert!(reader.pull(&mut out, 480).frames > 0);
    assert_eq!(srv.log().matches("key=pz1").count(), 2);
    player.stop();

    // A finite stream keeps its position: every frame arrives once.
    let player = MusicPlayer::new(config(48000));
    player.start(&srv.url("/media/tone.ogg?key=pz2"));
    let mut reader = player.reader();
    let st = wait_state(&player, Duration::from_secs(10), |s| *s == PlayerState::Playing);
    assert_eq!(st, PlayerState::Playing);
    let a = pull(&mut reader, 480, Some(ms(10)), ms(200));
    player.pause(true);
    assert_eq!(player.state(), PlayerState::Paused);
    std::thread::sleep(ms(300));
    player.pause(false);
    let b = pull(&mut reader, 480, None, Duration::from_secs(30));
    assert!(b.ended);
    let total: u64 = player.stats().segments.iter().map(|s| s.out_frames).sum();
    assert_eq!(a.frames + b.frames, total);
    assert_eq!(srv.log().matches("key=pz2").count(), 1);
}

#[test]
fn device_rate_change_mid_stream() {
    let srv = server();
    let player = MusicPlayer::new(config(48000));
    player.start(&srv.url("/media/tone.ogg?key=dr1"));
    let mut reader = player.reader();
    let mut out = vec![0f32; 480 * 8];
    // One second at 48 kHz.
    let mut before = Vec::new();
    while before.len() < 96000 {
        let r = reader.pull(&mut out, 480);
        before.extend_from_slice(&out[..r.frames * 2]);
        if r.frames == 0 {
            std::thread::sleep(ms(1));
        }
    }
    player.set_device_rate(44100);
    let p = pull(&mut reader, 441, None, Duration::from_secs(30));
    assert!(p.ended);
    let x = p.channel(2, 0);
    let f = frequency(&x[4410..x.len() - 4410], 44100);
    assert!((f - 440.0).abs() < 0.5, "after the change: {f} Hz");
    // 4 s in all: 1 s at 48 kHz, the rest at 44.1 kHz.
    let rest = p.frames as f64 / 44100.0;
    assert!((rest - 3.0).abs() < 0.01, "{rest} s after the change");
}

#[test]
fn stop_ends_the_reader_and_clears_metadata() {
    let srv = server();
    let player = MusicPlayer::new(config(48000));
    // A label after the URL (FMOD allowed it) is ignored.
    player.start(&format!("{} My Station", srv.url("/media/tone.ogg?key=st1")));
    assert_eq!(player.url(), srv.url("/media/tone.ogg?key=st1"));
    let mut reader = player.reader();
    wait_state(&player, Duration::from_secs(10), |s| *s == PlayerState::Playing);
    assert_eq!(events(&player), vec![meta("Plain Artist", "Plain Title")]);
    assert_eq!(player.current_metadata(), Some(("Plain Artist".into(), "Plain Title".into())));
    player.stop();
    let mut out = vec![0f32; 480 * 8];
    let r = reader.pull(&mut out, 480);
    assert_eq!((r.frames, r.ended), (0, true));
    assert_eq!(player.state(), PlayerState::Stopped);
    assert_eq!(player.is_playing(), 0);
    assert_eq!(events(&player), vec![PlayerEvent::MetadataCleared]);
    assert_eq!(player.current_metadata(), None);
    // A new start: the old reader stays ended, a new one plays.
    player.start(&srv.url("/media/tone.flac?key=st2"));
    assert!(reader.pull(&mut out, 480).ended);
    let mut reader = player.reader();
    let p = pull(&mut reader, 480, None, Duration::from_secs(30));
    assert!(p.ended && p.frames > 0);
}
