//! Test material: WAV fixtures (written in the formats dr_wav reads) and the
//! scene scripts both runners execute.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// `sin` without libm: the scripts and fixtures must be byte-identical on
/// every platform (the platform's `sin`/`cos`/`pow` may differ in the last
/// bit), so the generator uses plain IEEE arithmetic (range reduction and a
/// Taylor series; accuracy is irrelevant, determinism is not).
pub fn dsin(x: f64) -> f64 {
    let two_pi = std::f64::consts::TAU;
    let k = (x / two_pi).round();
    let r = x - k * two_pi;
    let r2 = r * r;
    let mut term = r;
    let mut sum = r;
    for n in 1..12 {
        term *= -r2 / ((2 * n) as f64 * (2 * n + 1) as f64);
        sum += term;
    }
    sum
}

pub fn dcos(x: f64) -> f64 {
    dsin(x + std::f64::consts::FRAC_PI_2)
}

fn sinf(x: f32) -> f32 {
    dsin(x as f64) as f32
}

fn cosf(x: f32) -> f32 {
    dcos(x as f64) as f32
}

/// Deterministic noise (LCG), independent of the wind generator's.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
    }
}

/// Sample encodings for [`wav_bytes`].
#[derive(Clone, Copy, Debug)]
pub enum Enc {
    U8,
    S16,
    S24,
    S32,
    F32,
    F64,
    Alaw,
    Mulaw,
    /// 16-bit PCM in a WAVE_FORMAT_EXTENSIBLE header.
    Ext16,
}

/// A RIFF/WAVE file of interleaved `samples` (`channels` per frame).
pub fn wav_bytes(
    samples: &[f64],
    channels: u16,
    rate: u32,
    enc: Enc,
    extra_chunks: bool,
) -> Vec<u8> {
    let (tag, bits): (u16, u16) = match enc {
        Enc::U8 => (1, 8),
        Enc::S16 => (1, 16),
        Enc::S24 => (1, 24),
        Enc::S32 => (1, 32),
        Enc::F32 => (3, 32),
        Enc::F64 => (3, 64),
        Enc::Alaw => (6, 8),
        Enc::Mulaw => (7, 8),
        Enc::Ext16 => (0xFFFE, 16),
    };
    let mut data = Vec::new();
    for &s in samples {
        let c = s.clamp(-1.0, 1.0);
        match enc {
            Enc::U8 => data.push(((c * 127.0).round() + 128.0) as u8),
            Enc::S16 | Enc::Ext16 => {
                data.extend_from_slice(&((c * 32767.0).round() as i16).to_le_bytes())
            }
            Enc::S24 => {
                let v = (c * 8_388_607.0).round() as i32;
                data.extend_from_slice(&v.to_le_bytes()[..3]);
            }
            Enc::S32 => {
                data.extend_from_slice(&((c * 2_147_483_647.0).round() as i32).to_le_bytes())
            }
            Enc::F32 => data.extend_from_slice(&(s as f32).to_le_bytes()),
            Enc::F64 => data.extend_from_slice(&s.to_le_bytes()),
            Enc::Alaw | Enc::Mulaw => data.push(((c * 127.0).round() as i32 + 128) as u8),
        }
    }
    let block_align = channels * bits / 8;
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&tag.to_le_bytes());
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * block_align as u32).to_le_bytes());
    fmt.extend_from_slice(&block_align.to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());
    if let Enc::Ext16 = enc {
        fmt.extend_from_slice(&22u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        fmt.extend_from_slice(&0x3u32.to_le_bytes());
        fmt.extend_from_slice(&[
            1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71,
        ]);
    }
    let mut body = Vec::new();
    body.extend_from_slice(b"WAVE");
    if extra_chunks {
        // An odd-sized chunk before fmt (padding byte) and a LIST after data.
        body.extend_from_slice(b"junk");
        body.extend_from_slice(&3u32.to_le_bytes());
        body.extend_from_slice(&[1, 2, 3, 0]);
    }
    body.extend_from_slice(b"fmt ");
    body.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    body.extend_from_slice(&fmt);
    body.extend_from_slice(b"data");
    body.extend_from_slice(&(data.len() as u32).to_le_bytes());
    body.extend_from_slice(&data);
    if data.len() % 2 == 1 {
        body.push(0);
    }
    if extra_chunks {
        body.extend_from_slice(b"LIST");
        body.extend_from_slice(&4u32.to_le_bytes());
        body.extend_from_slice(b"INFO");
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// A test signal: harmonics of `freq` plus some noise, `channels`
/// interleaved, each channel slightly detuned.
pub fn signal(
    frames: usize,
    channels: usize,
    rate: f64,
    freq: f64,
    amp: f64,
    noise: f64,
    seed: u64,
) -> Vec<f64> {
    let mut r = Lcg(seed);
    let mut out = Vec::with_capacity(frames * channels);
    for i in 0..frames {
        let t = i as f64 / rate;
        for c in 0..channels {
            let f = freq * (1.0 + 0.013 * c as f64);
            let s = dsin(2.0 * std::f64::consts::PI * f * t) * 0.7
                + dsin(2.0 * std::f64::consts::PI * f * 2.0 * t) * 0.2
                + dsin(2.0 * std::f64::consts::PI * f * 3.1 * t) * 0.1;
            out.push(amp * (s * (1.0 - noise) + noise * r.next()));
        }
    }
    out
}

/// The scene fixtures (decoded-cache style mono 44.1 kHz 16-bit, plus
/// other formats for 2-D sounds). Returns (id, path).
pub fn write_fixtures(dir: &Path) -> std::io::Result<Vec<(u32, PathBuf)>> {
    fs::create_dir_all(dir)?;
    let mut list = Vec::new();
    let mut add = |id: u32, name: &str, bytes: Vec<u8>| -> std::io::Result<()> {
        let p = dir.join(name);
        fs::write(&p, bytes)?;
        list.push((id, p));
        Ok(())
    };
    add(
        1,
        "tone.wav",
        wav_bytes(
            &signal(57_330, 1, 44100.0, 440.0, 0.6, 0.05, 1),
            1,
            44100,
            Enc::S16,
            false,
        ),
    )?;
    add(
        2,
        "click.wav",
        wav_bytes(
            &signal(200, 1, 44100.0, 2000.0, 0.8, 0.3, 2),
            1,
            44100,
            Enc::S16,
            false,
        ),
    )?;
    add(
        3,
        "noise.wav",
        wav_bytes(
            &signal(30_870, 1, 44100.0, 120.0, 0.95, 0.8, 3),
            1,
            44100,
            Enc::S16,
            false,
        ),
    )?;
    add(
        4,
        "loop.wav",
        wav_bytes(
            &signal(22_050, 1, 44100.0, 330.0, 0.5, 0.0, 4),
            1,
            44100,
            Enc::S16,
            false,
        ),
    )?;
    add(
        5,
        "stereo48.wav",
        wav_bytes(
            &signal(48_000, 2, 48000.0, 550.0, 0.5, 0.1, 5),
            2,
            48000,
            Enc::S16,
            true,
        ),
    )?;
    add(
        6,
        "float22.wav",
        wav_bytes(
            &signal(17_640, 1, 22050.0, 300.0, 0.7, 0.02, 6),
            1,
            22050,
            Enc::F32,
            false,
        ),
    )?;
    add(
        7,
        "u8_11k.wav",
        wav_bytes(
            &signal(6_615, 1, 11025.0, 220.0, 0.8, 0.1, 7),
            1,
            11025,
            Enc::U8,
            false,
        ),
    )?;
    add(
        8,
        "s24_96k.wav",
        wav_bytes(
            &signal(38_400, 2, 96000.0, 880.0, 0.6, 0.05, 8),
            2,
            96000,
            Enc::S24,
            false,
        ),
    )?;
    add(
        9,
        "six.wav",
        wav_bytes(
            &signal(22_050, 6, 44100.0, 200.0, 0.4, 0.1, 9),
            6,
            44100,
            Enc::S16,
            false,
        ),
    )?;
    add(
        10,
        "eight.wav",
        wav_bytes(
            &signal(22_050, 8, 44100.0, 260.0, 0.4, 0.1, 10),
            8,
            44100,
            Enc::S16,
            false,
        ),
    )?;
    add(
        11,
        "quad.wav",
        wav_bytes(
            &signal(22_050, 4, 44100.0, 310.0, 0.4, 0.1, 11),
            4,
            44100,
            Enc::S16,
            false,
        ),
    )?;
    // A stereo asset as LLVorbisDecodeState writes it: mono header, stereo data.
    add(
        12,
        "cache_stereo.wav",
        wav_bytes(
            &signal(44_100, 2, 44100.0, 500.0, 0.5, 0.05, 12),
            1,
            44100,
            Enc::S16,
            false,
        ),
    )?;
    Ok(list)
}

/// A scene under construction.
struct Scene {
    t: String,
    period: u32,
}

impl Scene {
    fn new(rate: u32, ch: u32, fixtures: &[(u32, PathBuf)]) -> Self {
        let mut t = String::new();
        let _ = writeln!(t, "init {rate} {ch}");
        for (id, p) in fixtures {
            let _ = writeln!(t, "wav {id} {}", p.display().to_string().replace('\\', "/"));
        }
        let _ = writeln!(t, "listener 0 0 0 0 0 -1 0 1 0 0 0 0");
        // The miniaudio period (40 ms) the viewer's callback receives.
        Scene {
            t,
            period: rate * 40 / 1000,
        }
    }
    fn line(&mut self, s: String) {
        self.t.push_str(&s);
        self.t.push('\n');
    }
    fn period(&mut self) {
        let p = self.period;
        self.line(format!("mix {p} 1024"));
    }
}

fn f(v: f32) -> String {
    // Shortest round-trip text: both runners parse it to the same float.
    format!("{v:?}")
}

fn p3(v: [f32; 3]) -> String {
    format!("{} {} {}", f(v[0]), f(v[1]), f(v[2]))
}

/// The scene names, in report order.
pub const SCENES: &[&str] = &[
    "moving",
    "listener",
    "attenuation",
    "rolloff",
    "doppler",
    "wind",
    "voices",
    "declick",
    "synced",
    "looping",
    "ui2d",
    "global",
    "inaudible",
    "chunks",
    "winddecay",
];

/// Builds scene `name` for one output format.
pub fn scene(name: &str, rate: u32, ch: u32, fixtures: &[(u32, PathBuf)]) -> String {
    let mut s = Scene::new(rate, ch, fixtures);
    s.line("global 0.8".into());
    match name {
        "moving" => {
            s.line("play3d 1 1 0.9 1 -40 0 -5 40 0 0 1 1".into());
            s.line("play3d 2 3 0.5 1 0 2 40 0 0 -30 1 1".into());
            s.line("play3d 3 5 0.7 1 10 -3 0 0 0 0 1 1".into());
            for i in 0..50 {
                let t = i as f32 * 0.04;
                s.line(format!("pos 1 {} 0 -5 40 0 0 1 1", f(-40.0 + 40.0 * t)));
                s.line(format!("pos 2 0 2 {} 0 0 -30 1 1", f(40.0 - 30.0 * t)));
                s.line(format!(
                    "pos 3 {} -3 {} {} 0 {} 1 1",
                    f(10.0 * cosf(t * 3.0)),
                    f(10.0 * sinf(t * 3.0)),
                    f(-30.0 * sinf(t * 3.0)),
                    f(30.0 * cosf(t * 3.0))
                ));
                s.line("commit".into());
                s.period();
            }
        }
        "listener" => {
            s.line("play3d 1 1 0.9 1 5 0 0 0 0 0 1 1".into());
            s.line("play3d 2 4 0.8 1 -3 1 -4 0 0 0 1 1".into());
            s.line("play3d 3 3 0.4 1 0 0 8 0 0 0 1 1".into());
            for i in 0..45 {
                let a = i as f32 * 0.14;
                let pos = [i as f32 * 0.3, 0.0, -(i as f32) * 0.1];
                let at = [sinf(a), 0.0, -cosf(a)];
                let up = [0.1 * sinf(a * 0.5), 1.0, 0.0];
                s.line(format!(
                    "listener {} {} {} 7.5 0 -2.5",
                    p3(pos),
                    p3(at),
                    p3(up)
                ));
                s.period();
            }
        }
        "attenuation" => {
            let d = [
                0.5f32, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 400.0, 512.0, 600.0,
            ];
            for (i, &dist) in d.iter().enumerate() {
                let a = i as f32 * 0.55;
                s.line(format!(
                    "play3d {} 1 0.9 1 {} {} {} 0 0 0 1 1",
                    i + 1,
                    f(dist * cosf(a)),
                    f(0.3 * dist),
                    f(dist * sinf(a))
                ));
            }
            for _ in 0..25 {
                s.period();
            }
        }
        "rolloff" => {
            let d = [0.5f32, 3.0, 12.0, 40.0, 150.0, 600.0];
            let r = [0.4f32, 1.0, 19.0];
            let mut id = 1;
            for (k, &ro) in r.iter().enumerate() {
                for &dist in &d {
                    s.line(format!(
                        "play3d {id} 4 0.7 1 {} 0 {} 0 0 0 1 {}",
                        f(dist),
                        f(-(k as f32)),
                        f(ro)
                    ));
                    id += 1;
                }
            }
            for i in 0..30 {
                if i == 10 {
                    // A live rolloff change, as update3DPosition sends every frame.
                    let mut id = 1;
                    for (k, &ro) in r.iter().enumerate() {
                        for &dist in &d {
                            s.line(format!(
                                "pos {id} {} 0 {} 0 0 0 1 {}",
                                f(dist),
                                f(-(k as f32)),
                                f(r[(k + 1) % 3] * 0.5 + ro * 0.5)
                            ));
                            id += 1;
                        }
                    }
                    s.line("commit".into());
                }
                s.period();
            }
        }
        "doppler" => {
            let factors = [0.0f32, 1.0, 4.0];
            let mut id = 1;
            for &fac in &factors {
                s.line(format!("play3d {id} 1 0.6 1 -30 0 -2 30 0 0 {} 1", f(fac)));
                id += 1;
                s.line(format!("play3d {id} 4 0.6 1 20 0 3 15 0 -2 {} 1", f(fac)));
                id += 1;
            }
            s.line("listener 0 0 0 0 0 -1 0 1 0 10 0 0".into());
            for i in 0..40 {
                let t = i as f32 * 0.04;
                let mut id = 1;
                for &fac in &factors {
                    s.line(format!(
                        "pos {id} {} 0 -2 30 0 0 {} 1",
                        f(-30.0 + 30.0 * t),
                        f(fac)
                    ));
                    id += 1;
                    s.line(format!(
                        "pos {id} {} 0 {} 15 0 -2 {} 1",
                        f(20.0 + 15.0 * t),
                        f(3.0 - 2.0 * t),
                        f(fac)
                    ));
                    id += 1;
                }
                s.line(format!("listener {} 0 0 0 0 -1 0 1 0 10 0 0", f(10.0 * t)));
            }
            for _ in 0..3 {
                s.period();
            }
            for i in 0..40 {
                let t = i as f32 * 0.04;
                s.line(format!(
                    "listener {} 0 0 0 0 -1 0 1 0 -25 0 4",
                    f(-25.0 * t)
                ));
                s.period();
            }
        }
        "wind" => {
            s.line("wind_on 12345".into());
            for i in 0..75 {
                let t = i as f32 * 0.04;
                let x = 1.0 + 0.5 * sinf(t * 1.7);
                let freq = 80.0 * x * x * x.sqrt(); // x^2.5 without powf
                let gain = if (30..55).contains(&i) {
                    0.0
                } else {
                    0.3 + 0.25 * sinf(t * 2.3)
                };
                let pan = 0.5 + 0.45 * sinf(t * 0.9);
                s.line(format!("wind {} {} {}", f(freq), f(gain), f(pan)));
                s.period();
            }
            s.line("wind_off".into());
            s.period();
        }
        "winddecay" => {
            // Six seconds at zero gain: the generator's gain decays
            // geometrically (x 0.999 per synthesized sample) into the
            // denormal range, where SoLoud's FTZ/DAZ mixing thread flushes it
            // to zero (without FTZ it would stick at the smallest denormal).
            s.line("wind_on 99".into());
            s.line("wind 150 0.5 0.4".into());
            for i in 0..190 {
                if i == 25 {
                    s.line("wind 150 0 0.4".into());
                }
                if i == 175 {
                    s.line("wind 300 0.3 0.6".into());
                }
                s.period();
            }
        }
        "voices" => {
            s.line("wind_on 777".into());
            s.line("wind 120 0.4 0.3".into());
            for i in 0..80 {
                let a = i as f32 * 0.37;
                let dist = 1.0 + (i as f32 * 7.3) % 90.0;
                let fx = [1, 3, 4, 5][i % 4];
                s.line(format!(
                    "play3d {} {fx} {} 1 {} 0 {} 0 0 0 1 1",
                    i + 1,
                    f(0.3 + (i % 7) as f32 * 0.1),
                    f(dist * cosf(a)),
                    f(dist * sinf(a))
                ));
                // Most voices stop ticking when inaudible so that SoLoud's
                // volume sort runs: with more than 64 ticking voices SoLoud
                // crashes (mixer_behaviour.rs, more_than_64_ticking_voices_do_not_crash,
                // covers that case).
                if i % 8 != 0 {
                    s.line(format!("inaudible {} 0 0", i + 1));
                }
                if i % 10 == 9 {
                    s.period();
                }
            }
            s.line("count".into());
            for i in 0..15 {
                for k in 0..5 {
                    let id = 1 + (i * 5 + k) % 80;
                    let a = (i * 5 + k) as f32 * 0.21;
                    let dist = 0.5 + ((i * 13 + k * 7) as f32 % 60.0);
                    s.line(format!(
                        "pos {id} {} 0 {} 0 0 0 1 1",
                        f(dist * cosf(a)),
                        f(dist * sinf(a))
                    ));
                }
                s.line("commit".into());
                if i == 6 {
                    for id in (1..80).step_by(3) {
                        s.line(format!("stop {id}"));
                    }
                    s.line("count".into());
                }
                s.period();
            }
            s.line("query 2".into());
            s.line("query 80".into());
        }
        "declick" => {
            for i in 0..6 {
                s.line(format!(
                    "play3d {} {} 0.8 1 {} 0 -2 0 0 0 1 1",
                    i + 1,
                    [1, 3, 4][i % 3],
                    f(i as f32 - 2.5)
                ));
            }
            s.line("play2d 7 5 0.6 0".into());
            for i in 0..20 {
                if i == 3 {
                    s.line("declick 1".into());
                    s.line("declick 7".into());
                }
                if i == 5 {
                    s.line("declick 2".into());
                    s.line("stop 3".into());
                    s.line("query 1".into());
                }
                if i == 8 {
                    s.line("declick 4".into());
                    s.line("play3d 8 1 0.9 0 1 0 -1 0 0 0 1 1".into());
                }
                s.period();
                if i == 4 {
                    s.line("query 1".into());
                    s.line("query 7".into());
                }
            }
            s.line("query 2".into());
            s.line("query 4".into());
            s.line("count".into());
        }
        "synced" => {
            s.line("play3d 1 4 0.8 1 2 0 -3 0 0 0 1 1".into());
            for i in 0..40 {
                if i == 8 {
                    s.line("sync 2 1 4 0.6 1 -2 0 -3 0 0 0 1 1".into());
                }
                if i == 19 {
                    s.line("sync 3 1 1 0.7 1 0 1 -6 0 0 0 1 1".into());
                }
                if i == 30 {
                    s.line("sync 4 99 4 0.5 1 0 0 -1 0 0 0 1 1".into());
                }
                s.period();
                if i % 6 == 0 {
                    s.line("query 1".into());
                    s.line("query 2".into());
                }
            }
        }
        "looping" => {
            s.line("play3d 1 4 0.7 1 1 0 -1 0 0 0 1 1".into());
            s.line("play2d 2 2 0.5 1".into());
            s.line("play3d 3 1 0.6 0 -1 0 -1 0 0 0 1 1".into());
            for i in 0..45 {
                s.period();
                s.line("query 1".into());
                s.line("query 2".into());
                s.line("query 3".into());
                if i == 25 {
                    s.line("loop 1 0".into());
                }
                if i == 30 {
                    s.line("loop 2 0".into());
                }
            }
        }
        "ui2d" => {
            let list = [
                (1, 0.7),
                (5, 0.6),
                (6, 0.8),
                (7, 0.5),
                (8, 0.6),
                (9, 0.5),
                (10, 0.5),
                (11, 0.6),
                (12, 0.6),
                (2, 1.0),
                (3, 0.9),
            ];
            for (k, (fx, g)) in list.iter().enumerate() {
                s.line(format!("play2d {} {fx} {} 0", k + 1, f(*g)));
                s.period();
                if k == 5 {
                    s.line("gain 2 0.2".into());
                }
            }
            for _ in 0..20 {
                s.period();
            }
        }
        "global" => {
            s.line("play2d 1 1 0.9 1".into());
            s.line("play3d 2 3 0.9 1 0 0 -1 0 0 0 1 1".into());
            let g = [1.0f32, 0.5, 0.0, 0.8, 2.0, 0.3];
            for i in 0..30 {
                if i % 5 == 0 {
                    s.line(format!("global {}", f(g[i / 5])));
                }
                s.period();
            }
        }
        "inaudible" => {
            s.line("play3d 1 4 0.9 1 600 0 0 0 0 0 1 19".into());
            s.line("play3d 2 1 0.001 1 3 0 0 0 0 0 1 1".into());
            s.line("play3d 3 2 0.8 0 400 0 0 0 0 0 1 19".into());
            for i in 0..40 {
                if i == 15 {
                    s.line("pos 1 2 0 0 0 0 0 1 1".into());
                    s.line("gain 2 0.9".into());
                    s.line("commit".into());
                }
                if i == 28 {
                    s.line("pos 1 300 0 0 0 0 0 1 19".into());
                    s.line("commit".into());
                }
                s.period();
                if i % 5 == 0 {
                    s.line("query 1".into());
                    s.line("query 2".into());
                    s.line("query 3".into());
                }
            }
        }
        "chunks" => {
            s.line("play2d 1 5 0.7 1".into());
            s.line("play3d 2 1 0.9 1 3 0 -2 0 0 0 1 1".into());
            s.line("play2d 3 1 0.6 1".into());
            s.line("wind_on 4242".into());
            s.line("wind 200 0.3 0.7".into());
            for (k, c) in [37u32, 333, 512, 1000, 1023, 1024, 441, 7]
                .iter()
                .enumerate()
            {
                s.line(format!("gain 1 {}", f(0.3 + 0.08 * k as f32)));
                s.line(format!("mix {} {c}", c * 3 + 5));
            }
        }
        _ => {}
    }
    s.t
}

/// Writes every scene for every (rate, channels) into `dir`; returns the
/// scene files.
pub fn write_scenes(dir: &Path, fixtures: &[(u32, PathBuf)]) -> std::io::Result<Vec<PathBuf>> {
    fs::create_dir_all(dir)?;
    let mut out = Vec::new();
    for &rate in &[44100u32, 48000] {
        for &ch in &[1u32, 2, 4, 6, 8] {
            for name in SCENES {
                let p = dir.join(format!("{name}-{rate}-{ch}.txt"));
                fs::write(&p, scene(name, rate, ch, fixtures))?;
                out.push(p);
            }
        }
    }
    Ok(out)
}

/// The timing scene: 64 moving mono 44.1 kHz 3D voices for `periods`
/// device periods of 1024 frames.
pub fn bench_scene(rate: u32, ch: u32, fixtures: &[(u32, PathBuf)], periods: u32) -> String {
    let mut s = Scene::new(rate, ch, fixtures);
    s.line("global 0.8".into());
    for i in 0..64 {
        let a = i as f32 * 0.41;
        let dist = 2.0 + (i as f32 * 3.7) % 60.0;
        let fx = [1, 3, 4][i % 3];
        s.line(format!(
            "play3d {} {fx} 0.5 1 {} 0 {} 0 0 0 1 1",
            i + 1,
            f(dist * cosf(a)),
            f(dist * sinf(a))
        ));
    }
    for p in 0..periods {
        for i in 0..64 {
            let a = i as f32 * 0.41 + p as f32 * 0.02;
            let dist = 2.0 + (i as f32 * 3.7) % 60.0;
            s.line(format!(
                "pos {} {} 0 {} {} 0 {} 1 1",
                i + 1,
                f(dist * cosf(a)),
                f(dist * sinf(a)),
                f(-dist * sinf(a)),
                f(dist * cosf(a))
            ));
        }
        s.line("commit".into());
        s.line("mix 1024 1024".into());
    }
    s.t
}
