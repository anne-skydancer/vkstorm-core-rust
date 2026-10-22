//! The Rust scene runner: the same scripts as `c/soloud_oracle.cpp`, driven
//! through vkstorm-audio's mixer with the adapter's call sequences.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Instant;

use vkstorm_audio::mixer::{Attenuation, Handle, Mixer, MixerConfig, Wav, WindNoise, WindTargets};

#[derive(Default, Clone, Copy)]
struct Voice {
    handle: Handle,
    wav: u32,
}

/// Output of a run.
#[derive(Default)]
pub struct Output {
    pub samples: Vec<f32>,
    pub log: String,
    pub mix_ns: f64,
    pub mix_calls: u64,
}

struct Runner {
    mixer: Option<Mixer>,
    wavs: HashMap<u32, Arc<Wav>>,
    voices: HashMap<i64, Voice>,
    wind: Option<Arc<WindTargets>>,
    wind_handle: Handle,
    channels: u32,
    keep: bool,
    out: Output,
    buffer: Vec<f32>,
}

fn nums<T: std::str::FromStr>(
    it: &mut std::str::SplitWhitespace,
    n: usize,
) -> Result<Vec<T>, String> {
    (0..n)
        .map(|_| {
            it.next()
                .and_then(|t| t.parse().ok())
                .ok_or_else(|| "bad argument".to_string())
        })
        .collect()
}

impl Runner {
    fn m(&mut self) -> Result<&mut Mixer, String> {
        self.mixer.as_mut().ok_or_else(|| "no init".to_string())
    }

    /// `LLAudioChannelSoLoud::startVoicePaused`.
    #[allow(clippy::too_many_arguments)]
    fn start_voice_paused(
        &mut self,
        wav: u32,
        gain: f32,
        looping: bool,
        positional: bool,
        p: &[f32],
        doppler: f32,
        rolloff: f32,
    ) -> Result<Handle, String> {
        let w = self.wavs.get(&wav).cloned().ok_or("unknown wav")?;
        let m = self.m()?;
        let h = if !positional {
            m.play(&w, gain, 0.0, true)
        } else {
            let h = m.play_3d(&w, [p[0], p[1], p[2]], [p[3], p[4], p[5]], gain, true);
            m.set_3d_source_doppler_factor(h, doppler);
            if rolloff != 1.0 {
                m.set_3d_source_attenuation(h, Attenuation::InverseDistance, rolloff);
            }
            h
        };
        m.set_looping(h, looping);
        m.set_inaudible_behavior(h, true, false);
        if positional {
            m.update_3d_audio();
        }
        Ok(h)
    }

    fn mix(&mut self, frames: u32, chunk: u32) -> Result<(), String> {
        let ch = self.channels as usize;
        self.buffer.resize(chunk as usize * ch, 0.0);
        let mut done = 0u32;
        while done < frames {
            let n = (frames - done).min(chunk) as usize;
            let mixer = self.mixer.as_mut().ok_or("no init")?;
            let t0 = Instant::now();
            mixer.mix(&mut self.buffer[..n * ch], n);
            let dt = t0.elapsed();
            self.out.mix_ns += dt.as_nanos() as f64;
            self.out.mix_calls += 1;
            if self.keep {
                self.out.samples.extend_from_slice(&self.buffer[..n * ch]);
            }
            done += n as u32;
        }
        Ok(())
    }

    fn line(&mut self, line: &str) -> Result<(), String> {
        let mut it = line.split_whitespace();
        let Some(cmd) = it.next() else { return Ok(()) };
        match cmd {
            "init" => {
                let v: Vec<u32> = nums(&mut it, 2)?;
                self.channels = v[1];
                self.voices.clear();
                self.wind = None;
                self.mixer =
                    Some(Mixer::new(MixerConfig::viewer(v[0], v[1])).map_err(|e| e.to_string())?);
            }
            "global" => {
                let g: Vec<f32> = nums(&mut it, 1)?;
                self.m()?.set_global_volume(g[0]);
            }
            "wav" => {
                let id: u32 = it.next().and_then(|t| t.parse().ok()).ok_or("wav id")?;
                // The rest of the line (paths may contain spaces).
                let rest = line.trim_start();
                let rest = rest[3..].trim_start();
                let path = rest[rest.find(char::is_whitespace).ok_or("wav path")?..].trim();
                let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
                let w = vkstorm_audio::wav::load_viewer_buffer(&bytes)
                    .map_err(|e| format!("{path}: {e}"))?;
                self.wavs.insert(id, w);
            }
            "listener" => {
                let v: Vec<f32> = nums(&mut it, 12)?;
                let m = self.m()?;
                m.set_3d_listener_parameters(
                    [v[0], v[1], v[2]],
                    [v[3], v[4], v[5]],
                    [v[6], v[7], v[8]],
                    [v[9], v[10], v[11]],
                );
                m.update_3d_audio();
            }
            "play2d" | "play3d" => {
                let id: i64 = it.next().and_then(|t| t.parse().ok()).ok_or("id")?;
                let wav: u32 = it.next().and_then(|t| t.parse().ok()).ok_or("wav")?;
                let gain: f32 = it.next().and_then(|t| t.parse().ok()).ok_or("gain")?;
                let looping: i32 = it.next().and_then(|t| t.parse().ok()).ok_or("loop")?;
                let positional = cmd == "play3d";
                let (p, doppler, rolloff) = if positional {
                    let p: Vec<f32> = nums(&mut it, 8)?;
                    (p[..6].to_vec(), p[6], p[7])
                } else {
                    (vec![0.0; 6], 1.0, 1.0)
                };
                let h = self.start_voice_paused(
                    wav,
                    gain,
                    looping != 0,
                    positional,
                    &p,
                    doppler,
                    rolloff,
                )?;
                if h != 0 {
                    self.m()?.set_pause(h, false);
                }
                self.voices.insert(id, Voice { handle: h, wav });
            }
            "sync" => {
                let id: i64 = it.next().and_then(|t| t.parse().ok()).ok_or("id")?;
                let master: i64 = it.next().and_then(|t| t.parse().ok()).ok_or("master")?;
                let wav: u32 = it.next().and_then(|t| t.parse().ok()).ok_or("wav")?;
                let gain: f32 = it.next().and_then(|t| t.parse().ok()).ok_or("gain")?;
                let looping: i32 = it.next().and_then(|t| t.parse().ok()).ok_or("loop")?;
                let p: Vec<f32> = nums(&mut it, 8)?;
                let mv = self.voices.get(&master).copied().unwrap_or_default();
                let valid = mv.handle != 0 && self.m()?.is_valid_voice_handle(mv.handle);
                let h;
                if valid {
                    let mut master_offset = self.m()?.stream_position(mv.handle);
                    let master_length = self.wavs.get(&mv.wav).map_or(0.0, |w| w.length());
                    if master_length > 0.0 {
                        master_offset %= master_length;
                    }
                    h = self.start_voice_paused(
                        wav,
                        gain,
                        looping != 0,
                        true,
                        &p[..6],
                        p[6],
                        p[7],
                    )?;
                    if h != 0 {
                        let m = self.m()?;
                        m.set_volume(h, 0.0);
                        m.seek(h, master_offset).map_err(|e| e.to_string())?;
                        m.fade_volume(h, gain, 0.02);
                        m.set_pause(h, false);
                    }
                } else {
                    h = self.start_voice_paused(
                        wav,
                        gain,
                        looping != 0,
                        true,
                        &p[..6],
                        p[6],
                        p[7],
                    )?;
                    if h != 0 {
                        self.m()?.set_pause(h, false);
                    }
                }
                self.voices.insert(id, Voice { handle: h, wav });
            }
            "pos" => {
                let id: i64 = it.next().and_then(|t| t.parse().ok()).ok_or("id")?;
                let p: Vec<f32> = nums(&mut it, 8)?;
                let h = self.voices.get(&id).map_or(0, |v| v.handle);
                let m = self.m()?;
                m.set_3d_source_parameters(h, [p[0], p[1], p[2]], [p[3], p[4], p[5]]);
                m.set_3d_source_doppler_factor(h, p[6]);
                m.set_3d_source_attenuation(h, Attenuation::InverseDistance, p[7]);
            }
            "gain" => {
                let id: i64 = it.next().and_then(|t| t.parse().ok()).ok_or("id")?;
                let g: Vec<f32> = nums(&mut it, 1)?;
                let h = self.voices.get(&id).map_or(0, |v| v.handle);
                self.m()?.set_volume(h, g[0]);
            }
            "loop" => {
                let id: i64 = it.next().and_then(|t| t.parse().ok()).ok_or("id")?;
                let l: Vec<i32> = nums(&mut it, 1)?;
                let h = self.voices.get(&id).map_or(0, |v| v.handle);
                self.m()?.set_looping(h, l[0] != 0);
            }
            "inaudible" => {
                let id: i64 = it.next().and_then(|t| t.parse().ok()).ok_or("id")?;
                let v: Vec<i32> = nums(&mut it, 2)?;
                let h = self.voices.get(&id).map_or(0, |v| v.handle);
                self.m()?.set_inaudible_behavior(h, v[0] != 0, v[1] != 0);
            }
            "commit" => self.m()?.update_3d_audio(),
            "declick" => {
                let id: i64 = it.next().and_then(|t| t.parse().ok()).ok_or("id")?;
                let h = self.voices.get(&id).map_or(0, |v| v.handle);
                let m = self.m()?;
                m.fade_volume(h, 0.0, 0.02);
                m.schedule_stop(h, 0.02);
            }
            "stop" => {
                let id: i64 = it.next().and_then(|t| t.parse().ok()).ok_or("id")?;
                let h = self.voices.get(&id).map_or(0, |v| v.handle);
                self.m()?.stop(h);
            }
            "wind_on" => {
                let seed: u64 = it.next().and_then(|t| t.parse().ok()).ok_or("seed")?;
                let targets = WindTargets::new();
                let m = self.mixer.as_mut().ok_or("no init")?;
                let h = m.play_wind(&targets, WindNoise::new(seed), 1.0);
                m.set_protect_voice(h, true);
                self.wind = Some(targets);
                self.wind_handle = h;
            }
            "wind" => {
                let v: Vec<f32> = nums(&mut it, 3)?;
                if let Some(w) = &self.wind {
                    w.set(v[0], v[1], v[2]);
                }
            }
            "wind_off" => {
                if self.wind_handle != 0 {
                    let h = self.wind_handle;
                    self.m()?.stop(h);
                }
                self.wind_handle = 0;
                self.wind = None;
            }
            "mix" => {
                let v: Vec<u32> = nums(&mut it, 2)?;
                self.mix(v[0], v[1])?;
            }
            "query" => {
                let id: i64 = it.next().and_then(|t| t.parse().ok()).ok_or("id")?;
                let h = self.voices.get(&id).map_or(0, |v| v.handle);
                let m = self.mixer.as_ref().ok_or("no init")?;
                let _ = writeln!(
                    self.out.log,
                    "query {id} valid={} loops={} pos={:016x} vol={:08x} ovol={:08x}",
                    m.is_valid_voice_handle(h) as i32,
                    m.loop_count(h),
                    m.stream_position(h).to_bits(),
                    m.volume(h).to_bits(),
                    m.overall_volume(h).to_bits()
                );
            }
            "count" => {
                let c = self.mixer.as_ref().ok_or("no init")?.voice_count();
                let _ = writeln!(self.out.log, "count voices={c}");
            }
            other => return Err(format!("unknown command {other}")),
        }
        Ok(())
    }
}

/// Runs a scene script; `keep` retains the samples.
pub fn run(script: &str, keep: bool) -> Result<Output, String> {
    let mut r = Runner {
        mixer: None,
        wavs: HashMap::new(),
        voices: HashMap::new(),
        wind: None,
        wind_handle: 0,
        channels: 2,
        keep,
        out: Output::default(),
        buffer: Vec::new(),
    };
    for (n, line) in script.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        r.line(line).map_err(|e| format!("line {}: {e}", n + 1))?;
    }
    Ok(r.out)
}
