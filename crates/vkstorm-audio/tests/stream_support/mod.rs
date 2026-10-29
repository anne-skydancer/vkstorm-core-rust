//! Shared helpers of the stream tests: the mock radio server (the viewer's
//! `vksaudio_mock_stream_server.py`, copied into the fixtures), player
//! construction from the environment, pulling like a mixer, and tone
//! analysis.
//!
//! Environment:
//! - `VKS_FFMPEG_DIR`: the FFmpeg 9.0.2 libraries (HE-AAC and Opus tests
//!   need them).
//! - `VKS_AUDIO_CA_BUNDLE`: a CA bundle (only TLS tests would need it).
//! - `VKS_PYTHON`: the Python 3 interpreter (default: `python3`, then
//!   `python`).

#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use vkstorm_audio::music::{ChannelLayout, MusicSource};
use vkstorm_audio::stream::{MusicPlayer, MusicReader, PlayerEvent, PlayerState, StreamConfig};

pub fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("music")
}

pub fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixtures().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

pub struct Server {
    child: Child,
    pub port: u16,
}

impl Server {
    fn start() -> Server {
        let script = fixtures().join("vksaudio_mock_stream_server.py");
        let candidates: Vec<String> = match std::env::var("VKS_PYTHON") {
            Ok(p) => vec![p],
            Err(_) => vec!["python3".into(), "python".into()],
        };
        for py in candidates {
            let Ok(mut child) = Command::new(&py)
                .arg("-I")
                .arg(&script)
                .args(["--serve", "0"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
            else {
                continue;
            };
            let mut line = String::new();
            let out = child.stdout.take().expect("stdout");
            if BufReader::new(out).read_line(&mut line).is_ok()
                && let Some(p) = line.trim().strip_prefix("serving on 127.0.0.1:")
                && let Ok(port) = p.parse()
            {
                return Server { child, port };
            }
            let _ = child.kill();
        }
        panic!("cannot start the mock stream server (set VKS_PYTHON)");
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    /// The server's request log.
    pub fn log(&self) -> String {
        let (status, body) = http_get(self.port, "/log");
        assert_eq!(status, 200);
        body
    }

    pub fn get(&self, path: &str) -> (u32, String) {
        http_get(self.port, path)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A plain GET of a sized response (test plumbing; not the code under
/// test). Reads exactly the Content-Length and closes: the server waits for
/// its client to close sized responses.
fn http_get(port: u16, path: &str) -> (u32, String) {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").expect("send");
    let mut buf = Vec::new();
    let mut chunk = [0u8; 65536];
    loop {
        let n = s.read(&mut chunk).expect("read");
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
        let len = head
            .lines()
            .find_map(|l| {
                l.strip_prefix("content-length:")
                    .map(|v| v.trim().to_owned())
            })
            .and_then(|v| v.parse::<usize>().ok());
        if len.is_some_and(|l| buf.len() - end - 4 >= l) {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status = text
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_owned())
        .unwrap_or_default();
    (status, body)
}

/// One server per test binary (it exits when the binary does: its stdin
/// closes).
pub fn server() -> &'static Server {
    static S: OnceLock<Server> = OnceLock::new();
    S.get_or_init(Server::start)
}

/// A fresh server (for tests whose server state must not be shared).
pub fn own_server() -> Server {
    Server::start()
}

pub fn ffmpeg_dir() -> Option<PathBuf> {
    std::env::var_os("VKS_FFMPEG_DIR").map(PathBuf::from)
}

pub fn config(rate: u32) -> StreamConfig {
    StreamConfig {
        device_rate: rate,
        ffmpeg_dir: ffmpeg_dir(),
        ca_bundle: std::env::var_os("VKS_AUDIO_CA_BUNDLE").map(PathBuf::from),
        user_agent: "Vulkanstorm-test".into(),
        ..Default::default()
    }
}

/// What a pull loop collected.
#[derive(Default)]
pub struct Pulled {
    /// Runs of one layout, interleaved.
    pub runs: Vec<(ChannelLayout, Vec<f32>)>,
    pub frames: u64,
    pub ended: bool,
    /// Wall time from the start of pulling to the first audio.
    pub first_audio: Option<Duration>,
    /// Pulls that returned no frames after audio had started (silence the
    /// mixer would output).
    pub empty_after_start: u64,
}

impl Pulled {
    /// All samples of one channel across runs of `channels` channels.
    pub fn channel(&self, channels: u16, c: usize) -> Vec<f32> {
        let mut v = Vec::new();
        for (l, d) in &self.runs {
            if l.channels == channels {
                v.extend(d.chunks_exact(usize::from(channels)).map(|f| f[c]));
            }
        }
        v
    }
}

/// Pulls like a mixer: `block` frames per call, every `period` (None: as
/// fast as possible, sleeping 1 ms while there is nothing), until the
/// reader ends or `limit` passes.
pub fn pull(
    reader: &mut MusicReader,
    block: usize,
    period: Option<Duration>,
    limit: Duration,
) -> Pulled {
    let mut out = vec![0f32; block * 8];
    let mut p = Pulled::default();
    let t0 = Instant::now();
    let mut next = t0;
    while t0.elapsed() < limit {
        let r = reader.pull(&mut out, block);
        if r.frames > 0 {
            p.first_audio.get_or_insert(t0.elapsed());
            let n = r.frames * usize::from(r.layout.channels);
            match p.runs.last_mut() {
                Some((l, d)) if *l == r.layout => d.extend_from_slice(&out[..n]),
                _ => p.runs.push((r.layout, out[..n].to_vec())),
            }
            p.frames += r.frames as u64;
        } else if p.first_audio.is_some() && !r.ended {
            p.empty_after_start += 1;
        }
        if r.ended {
            p.ended = true;
            break;
        }
        match period {
            Some(d) => {
                next += d;
                if let Some(w) = next.checked_duration_since(Instant::now()) {
                    std::thread::sleep(w);
                }
            }
            None if r.frames == 0 => std::thread::sleep(Duration::from_millis(1)),
            None => {}
        }
    }
    p
}

/// Waits for `pred` on the state, up to `limit`.
pub fn wait_state(
    player: &MusicPlayer,
    limit: Duration,
    pred: impl Fn(&PlayerState) -> bool,
) -> PlayerState {
    let t0 = Instant::now();
    loop {
        let s = player.state();
        if pred(&s) || t0.elapsed() > limit {
            return s;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn events(player: &MusicPlayer) -> Vec<PlayerEvent> {
    std::iter::from_fn(|| player.poll_event()).collect()
}

pub fn meta(artist: &str, title: &str) -> PlayerEvent {
    PlayerEvent::Metadata {
        artist: artist.into(),
        title: title.into(),
    }
}

/// Amplitude of the `freq` component of `x` (Goertzel, rectangular window
/// over a whole number of periods where possible).
pub fn amplitude(x: &[f32], rate: u32, freq: f64) -> f64 {
    let n = x.len();
    if n == 0 {
        return 0.0;
    }
    let periods = (n as f64 * freq / f64::from(rate)).floor().max(1.0);
    let n = ((periods * f64::from(rate) / freq).round() as usize).min(n);
    let w = 2.0 * std::f64::consts::PI * freq / f64::from(rate);
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &v in &x[..n] {
        let s = f64::from(v) + coeff * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
    2.0 * power.max(0.0).sqrt() / n as f64
}

/// The frequency of a sine from its rising zero crossings (linearly
/// interpolated), over `x`.
pub fn frequency(x: &[f32], rate: u32) -> f64 {
    let mut first = None;
    let mut last = 0.0;
    let mut count = 0usize;
    for i in 1..x.len() {
        let (a, b) = (x[i - 1], x[i]);
        if a < 0.0 && b >= 0.0 {
            let t = (i - 1) as f64 + f64::from(-a) / f64::from(b - a);
            if first.is_none() {
                first = Some(t);
            } else {
                count += 1;
            }
            last = t;
        }
    }
    match first {
        Some(f) if count > 0 => count as f64 * f64::from(rate) / (last - f),
        _ => 0.0,
    }
}

/// Skips leading near-silence (decoder start-up, fades) and returns the
/// rest.
pub fn trim_start(x: &[f32]) -> &[f32] {
    let i = x.iter().position(|v| v.abs() > 0.05).unwrap_or(x.len());
    &x[i..]
}
