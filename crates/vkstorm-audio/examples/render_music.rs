//! Renders parcel music through the engine on the null device into a WAV
//! file, paced like a device (one period at a time on the wall clock), with
//! the viewer's fade policy: a login-style hold at zero, the fade-in, a
//! steady part and the fade-out, each transition completed on the device
//! clock. For listening checks of fades, starts and speaker fill.
//!
//! ```text
//! cargo run --release -p vkstorm-audio --features link-curl --example render_music -- \
//!     --url http://127.0.0.1:PORT/live/music.ogg --out music.wav \
//!     [--channels 2|6|8] [--spatial 0|21|41|51|71] [--hold 2] [--fade-in 3] \
//!     [--steady 6] [--fade-out 2] [--late-ms 0] [--load-threads 0] [--fast]
//! ```
//!
//! `--late-ms N` makes every 7th period late by N ms and catches up in the
//! next callback (a device thread starved for CPU); `--load-threads K` keeps
//! K threads spinning for the whole render (a busy machine). `--fast`
//! renders without pacing, waiting for decoded audio instead (no underruns
//! by construction). The output is 32-bit float, 48 kHz, WAVE_FORMAT_EXTENSIBLE
//! with the device's channel mask. Prints a summary line of the counters.

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use vkstorm_audio::device::NullConfig;
use vkstorm_audio::engine::Engine;
use vkstorm_audio::mixer::TransitionState;
use vkstorm_audio::stream::{MusicPlayer, PlayerState, StreamConfig};

const RATE: u32 = 48000;
const PERIOD: usize = 1024;
const LATENCY: u64 = 2048;

struct Args {
    url: String,
    out: String,
    channels: u16,
    spatial: i32,
    hold: f64,
    fade_in: f32,
    steady: f64,
    fade_out: f32,
    late_ms: u64,
    load_threads: usize,
    fast: bool,
}

fn parse() -> Args {
    let mut a = Args {
        url: String::new(),
        out: "music.wav".into(),
        channels: 2,
        spatial: 0,
        hold: 2.0,
        fade_in: 3.0,
        steady: 6.0,
        fade_out: 2.0,
        late_ms: 0,
        load_threads: 0,
        fast: false,
    };
    let v: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < v.len() {
        let next = |i: usize| v.get(i + 1).cloned().unwrap_or_default();
        match v[i].as_str() {
            "--url" => a.url = next(i),
            "--out" => a.out = next(i),
            "--channels" => a.channels = next(i).parse().unwrap_or(2),
            "--spatial" => a.spatial = next(i).parse().unwrap_or(0),
            "--hold" => a.hold = next(i).parse().unwrap_or(2.0),
            "--fade-in" => a.fade_in = next(i).parse().unwrap_or(3.0),
            "--steady" => a.steady = next(i).parse().unwrap_or(6.0),
            "--fade-out" => a.fade_out = next(i).parse().unwrap_or(2.0),
            "--late-ms" => a.late_ms = next(i).parse().unwrap_or(0),
            "--load-threads" => a.load_threads = next(i).parse().unwrap_or(0),
            "--fast" => {
                a.fast = true;
                i += 1;
                continue;
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
        i += 2;
    }
    if a.url.is_empty() {
        eprintln!("--url is required");
        std::process::exit(2);
    }
    a
}

fn write_wav(path: &str, channels: u16, mask: u32, samples: &[f32]) -> std::io::Result<()> {
    let data_len = (samples.len() * 4) as u32;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all(b"RIFF")?;
    f.write_all(&(4 + 8 + 40 + 8 + data_len).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&40u32.to_le_bytes())?;
    f.write_all(&0xFFFEu16.to_le_bytes())?; // WAVE_FORMAT_EXTENSIBLE
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&RATE.to_le_bytes())?;
    f.write_all(&(RATE * 4 * channels as u32).to_le_bytes())?;
    f.write_all(&(4 * channels).to_le_bytes())?;
    f.write_all(&32u16.to_le_bytes())?;
    f.write_all(&22u16.to_le_bytes())?;
    f.write_all(&32u16.to_le_bytes())?; // valid bits
    f.write_all(&mask.to_le_bytes())?;
    // KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
    f.write_all(&[
        0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B,
        0x71,
    ])?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in samples {
        f.write_all(&s.to_le_bytes())?;
    }
    f.flush()
}

fn main() {
    let a = parse();
    let stop_load = Arc::new(AtomicBool::new(false));
    let load: Vec<_> = (0..a.load_threads)
        .map(|_| {
            let stop = stop_load.clone();
            std::thread::spawn(move || {
                let mut x = 1u64;
                while !stop.load(Ordering::Relaxed) {
                    x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                    std::hint::black_box(x);
                }
            })
        })
        .collect();

    let mut engine = Engine::new_null(NullConfig {
        rate: RATE,
        channels: a.channels,
        channel_mask: 0,
        headphones: false,
        clip: true,
    })
    .expect("null device");
    let mask = match a.channels {
        6 => 0x3F,
        8 => 0x63F,
        _ => 0x3,
    };
    let cfg = StreamConfig {
        device_rate: RATE,
        ffmpeg_dir: std::env::var_os("VKS_FFMPEG_DIR").map(Into::into),
        ..StreamConfig::default()
    };
    let player = MusicPlayer::new(cfg);
    {
        let mut m = engine.mixer();
        m.music.set_volume(1.0);
        m.music.set_spatial_layout(a.spatial);
        // The login hold: the stream starts at exactly zero
        m.music.set_envelope(0.0);
    }
    player.start(&a.url);
    drop(engine.attach_music(Box::new(player.reader())));

    let ch = a.channels as usize;
    let mut out: Vec<f32> = Vec::new();
    let mut rendered: u64 = 0;
    let started = Instant::now();
    let mut period_no = 0u64;
    let mut phase = 0; // 0 hold, 1 fading in, 2 steady, 3 fading out, 4 tail
    let mut phase_frame = 0u64;
    let mut serial = 0u32;
    let mut tail_left = RATE as u64 / 2;
    let render_period =
        |engine: &mut Engine, out: &mut Vec<f32>, rendered: &mut u64, frames: usize| {
            let v = engine.render_null(frames);
            out.extend_from_slice(&v);
            *rendered += frames as u64;
            engine.set_null_clock(rendered.saturating_sub(LATENCY));
        };
    loop {
        if a.fast {
            // Wait for decoded audio (the device's period passing)
            let t0 = Instant::now();
            while player.stats().buffered_frames < PERIOD
                && matches!(
                    player.state(),
                    PlayerState::Connecting | PlayerState::Buffering | PlayerState::Playing
                )
                && t0.elapsed() < Duration::from_secs(10)
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            render_period(&mut engine, &mut out, &mut rendered, PERIOD);
        } else {
            // Pace on the wall clock; every 7th period is late by late_ms
            // and the next callback catches up (two periods at once)
            period_no += 1;
            let due = started + Duration::from_secs_f64(rendered as f64 / RATE as f64);
            let mut wake = due;
            if a.late_ms > 0 && period_no.is_multiple_of(7) {
                wake += Duration::from_millis(a.late_ms);
            }
            let now = Instant::now();
            if wake > now {
                std::thread::sleep(wake - now);
            }
            let behind = Instant::now()
                .saturating_duration_since(started)
                .as_secs_f64()
                * RATE as f64;
            let mut frames = PERIOD;
            while (rendered + frames as u64 + PERIOD as u64) as f64 <= behind && frames < 8 * PERIOD
            {
                frames += PERIOD;
            }
            let mut left = frames;
            while left > 0 {
                let n = left.min(PERIOD);
                render_period(&mut engine, &mut out, &mut rendered, n);
                left -= n;
            }
        }
        let music_frames = rendered - phase_frame;
        let state = engine.music_transition_state(serial);
        match phase {
            0 if music_frames as f64 >= a.hold * RATE as f64
                && matches!(player.state(), PlayerState::Playing | PlayerState::Ended) =>
            {
                serial += 1;
                engine.mixer().music.transition(serial, 1.0, a.fade_in);
                phase = 1;
                phase_frame = rendered;
            }
            1 if state == TransitionState::Complete => {
                phase = 2;
                phase_frame = rendered;
            }
            2 if music_frames as f64 >= a.steady * RATE as f64 => {
                serial += 1;
                engine.mixer().music.transition(serial, 0.0, a.fade_out);
                phase = 3;
                phase_frame = rendered;
            }
            3 if state == TransitionState::Complete => {
                phase = 4;
                phase_frame = rendered;
            }
            4 => {
                tail_left = tail_left.saturating_sub(PERIOD as u64);
                if tail_left == 0 {
                    break;
                }
            }
            _ => {}
        }
        if matches!(player.state(), PlayerState::Failed(_)) {
            eprintln!("stream failed: {:?}", player.state());
            break;
        }
        if rendered > 120 * RATE as u64 {
            eprintln!("giving up after 120 s of output (phase {phase})");
            break;
        }
    }
    let st = player.stats();
    player.stop();
    drop(engine.detach_music());
    stop_load.store(true, Ordering::Relaxed);
    for t in load {
        let _ = t.join();
    }
    let (bus_in, bus_out, bus_under) = {
        let m = engine.mixer();
        (
            m.music.input_frames(),
            m.music.output_frames(),
            m.music.underrun_frames(),
        )
    };
    write_wav(&a.out, a.channels, mask, &out).expect("write wav");
    let seg = st.segments.first();
    println!(
        "frames={} seconds={:.3} codec={} pushed={} pulled={} underruns={} bus_in={} bus_out={} bus_underrun={} source_frames={} device_frames={} source_rate={}",
        out.len() / ch,
        (out.len() / ch) as f64 / RATE as f64,
        st.codec,
        st.pushed_frames,
        st.pulled_frames,
        st.underruns,
        bus_in,
        bus_out,
        bus_under,
        seg.map_or(0, |s| s.in_frames),
        seg.map_or(0, |s| s.out_frames),
        seg.map_or(0, |s| s.source_rate)
    );
}
