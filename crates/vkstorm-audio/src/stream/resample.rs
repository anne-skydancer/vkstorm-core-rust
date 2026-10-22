//! Sample rate conversion from the source rate to the device rate: a
//! polyphase windowed-sinc filter with exact rational position accounting.
//!
//! Output frame `k` sits at input time `k * in_rate / out_rate`, kept as an
//! integer position plus a numerator over `L = out_rate / gcd`, so there is
//! no accumulated drift: over a segment of `N` input frames the converter
//! emits exactly `ceil(N * out_rate / in_rate)` frames (output frame 0 is
//! aligned with input frame 0, input beyond the ends is taken as zero). The
//! same input always gives the same output (no time-varying state, fixed
//! summation order).
//!
//! The kernel is a Kaiser-windowed sinc (beta 8.0, about -80 dB stopband)
//! with 64 taps per phase when upsampling and proportionally more when
//! downsampling; the -6 dB point is at 0.46 of the lower of the two rates,
//! so the passband is flat to about 0.42 (18.5 kHz at 44.1 kHz) and the
//! stopband starts at the lower Nyquist frequency. Each phase is normalised
//! to unit DC gain. For ratios with more than [`MAX_PHASES`] phases (none
//! of the standard audio rates) the coefficients are interpolated linearly
//! between [`MAX_PHASES`] tabulated phases; the position stays exact.
//!
//! Equal rates pass samples through untouched.

#![forbid(unsafe_code)]

/// The most phases tabulated exactly. 44.1 kHz to 48 kHz needs 160, 11.025
/// kHz to 48 kHz 640, 8 kHz to 44.1 kHz 441.
pub const MAX_PHASES: usize = 1024;
/// Half the taps per phase when upsampling (taps = 2 * HALF_TAPS).
const HALF_TAPS: usize = 32;
/// The upper bound on half the taps (extreme downsampling ratios).
const MAX_HALF_TAPS: usize = 512;
/// -6 dB point as a fraction of the lower rate.
const CUTOFF: f64 = 0.46;
const KAISER_BETA: f64 = 8.0;
/// History kept in front of the read position before it is compacted.
const COMPACT_AT: usize = 8192;

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// Zeroth-order modified Bessel function of the first kind (series).
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    let mut k = 1.0;
    while term > sum * 1e-17 {
        term *= q / (k * k);
        sum += term;
        k += 1.0;
    }
    sum
}

/// The filter of one rate pair: `phases` rows of `taps` coefficients, row
/// `p` for the fractional position `p / phases` of an input sample.
#[derive(Debug)]
struct Filter {
    l: u64,
    m: u64,
    half: usize,
    taps: usize,
    /// Tabulated phases (`l` when exact, else MAX_PHASES); one extra row
    /// (phase `phases`, i.e. the next input sample) for interpolation.
    phases: usize,
    exact: bool,
    table: Vec<f32>,
}

impl Filter {
    fn new(in_rate: u32, out_rate: u32) -> Filter {
        let g = gcd(u64::from(in_rate), u64::from(out_rate));
        let l = u64::from(out_rate) / g;
        let m = u64::from(in_rate) / g;
        // Downsampling lowers the cutoff (relative to the input rate) and
        // widens the kernel by the same factor.
        let scale = if m > l { l as f64 / m as f64 } else { 1.0 };
        let half = ((HALF_TAPS as f64 / scale).ceil() as usize).clamp(HALF_TAPS, MAX_HALF_TAPS);
        let taps = 2 * half;
        let exact = l as usize <= MAX_PHASES;
        let phases = if exact { l as usize } else { MAX_PHASES };
        let fc = CUTOFF * scale; // cycles per input sample
        let i0b = bessel_i0(KAISER_BETA);
        let mut table = vec![0f32; (phases + 1) * taps];
        let mut row = vec![0f64; taps];
        for p in 0..=phases {
            let frac = p as f64 / phases as f64;
            let mut sum = 0.0;
            for (t, c) in row.iter_mut().enumerate() {
                // Tap t multiplies input sample (pos - half + 1 + t).
                let x = (t as f64 - (half as f64 - 1.0)) - frac;
                let r = x / half as f64;
                let w = if r.abs() >= 1.0 { 0.0 } else { bessel_i0(KAISER_BETA * (1.0 - r * r).sqrt()) / i0b };
                let arg = 2.0 * fc * x;
                let sinc = if arg.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * arg).sin() / (std::f64::consts::PI * arg) };
                *c = 2.0 * fc * sinc * w;
                sum += *c;
            }
            for (t, c) in row.iter().enumerate() {
                table[p * taps + t] = (*c / sum) as f32;
            }
        }
        Filter { l, m, half, taps, phases, exact, table }
    }

    fn row(&self, p: usize) -> &[f32] {
        &self.table[p * self.taps..(p + 1) * self.taps]
    }
}

/// Dot product with eight independent accumulators, in a fixed order (the
/// compiler vectorises it; the result does not depend on the target).
#[inline]
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);
    let mut acc = [0f32; 8];
    let (ca, ra) = a.as_chunks::<8>();
    let (cb, rb) = b.as_chunks::<8>();
    for (x, y) in ca.iter().zip(cb) {
        for i in 0..8 {
            acc[i] += x[i] * y[i];
        }
    }
    let mut tail = 0f32;
    for (x, y) in ra.iter().zip(rb) {
        tail += x * y;
    }
    ((acc[0] + acc[4]) + (acc[1] + acc[5])) + ((acc[2] + acc[6]) + (acc[3] + acc[7])) + tail
}

/// A streaming converter for one channel layout and one source rate.
#[derive(Debug)]
pub struct Resampler {
    in_rate: u32,
    out_rate: u32,
    channels: usize,
    filter: Option<Filter>,
    /// Input history per channel; `hist[c][pos]` is the input sample at the
    /// current integer position.
    hist: Vec<Vec<f32>>,
    pos: usize,
    /// The `hist` index of input frame 0 (negative once compaction dropped
    /// it): `pos - origin` is the input frame at the position.
    origin: i64,
    /// Fractional position, numerator over `filter.l`.
    frac: u64,
    /// Interpolated-mode scratch row.
    scratch: Vec<f32>,
    in_total: u64,
    out_total: u64,
    /// Input frames appended since the start (incl. the zero prefix).
    flushed: bool,
}

impl Resampler {
    /// A converter from `in_rate` to `out_rate` for `channels` channels.
    pub fn new(in_rate: u32, out_rate: u32, channels: usize) -> Resampler {
        let mut r = Resampler {
            in_rate,
            out_rate,
            channels,
            filter: None,
            hist: vec![Vec::new(); channels],
            pos: 0,
            origin: 0,
            frac: 0,
            scratch: Vec::new(),
            in_total: 0,
            out_total: 0,
            flushed: false,
        };
        r.configure();
        let prefix = r.prefix();
        for h in &mut r.hist {
            h.resize(prefix, 0.0);
        }
        r.pos = prefix;
        r.origin = prefix as i64;
        r
    }

    /// The zero history in front of input frame 0.
    fn prefix(&self) -> usize {
        self.filter.as_ref().map_or(0, |f| f.half - 1)
    }

    fn configure(&mut self) {
        self.filter = if self.in_rate == self.out_rate { None } else { Some(Filter::new(self.in_rate, self.out_rate)) };
        if let Some(f) = &self.filter {
            self.scratch.resize(f.taps, 0.0);
        }
    }

    pub fn in_rate(&self) -> u32 {
        self.in_rate
    }

    pub fn out_rate(&self) -> u32 {
        self.out_rate
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Input frames consumed so far.
    pub fn in_frames(&self) -> u64 {
        self.in_total
    }

    /// Output frames produced so far.
    pub fn out_frames(&self) -> u64 {
        self.out_total
    }

    /// The taps per output sample (0 when passing through).
    pub fn taps(&self) -> usize {
        self.filter.as_ref().map_or(0, |f| f.taps)
    }

    /// Changes the output rate, keeping the input history and the current
    /// input time (rounded to the nearest position of the new rate, less
    /// than one output frame away). The exact-count property then holds
    /// from the change on.
    pub fn set_out_rate(&mut self, out_rate: u32) {
        if out_rate == self.out_rate {
            return;
        }
        let old_l = self.filter.as_ref().map_or(1, |f| f.l);
        self.out_rate = out_rate;
        self.configure();
        let new_l = self.filter.as_ref().map_or(1, |f| f.l);
        let mut frac = (self.frac * new_l + old_l / 2) / old_l;
        if frac >= new_l {
            frac -= new_l;
            self.pos += 1;
        }
        self.frac = frac;
        // A wider kernel needs more history in front of the position.
        let need = self.prefix();
        if self.pos < need {
            let add = need - self.pos;
            for h in &mut self.hist {
                h.splice(0..0, std::iter::repeat_n(0.0, add));
            }
            self.pos += add;
            self.origin += add as i64;
        }
    }

    /// Appends planar input (`planes[c][..frames]`) and writes the output
    /// that is now complete, interleaved, to `out` (appended).
    pub fn process(&mut self, planes: &[&[f32]], frames: usize, out: &mut Vec<f32>) {
        debug_assert_eq!(planes.len(), self.channels);
        self.in_total += frames as u64;
        if self.filter.is_none() {
            out.reserve(frames * self.channels);
            for i in 0..frames {
                for p in planes {
                    out.push(p[i]);
                }
            }
            self.out_total += frames as u64;
            return;
        }
        for (h, p) in self.hist.iter_mut().zip(planes) {
            h.extend_from_slice(&p[..frames]);
        }
        self.generate(out, None);
        self.compact();
    }

    /// Ends the segment: emits the remaining output (the input after the
    /// end taken as zero) so that the total is `ceil(in * out / in_rate)`.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        if self.flushed {
            return;
        }
        self.flushed = true;
        let Some(f) = &self.filter else { return };
        let pad = f.half + 1;
        for h in &mut self.hist {
            h.extend(std::iter::repeat_n(0.0, pad));
        }
        // Emit while the input frame at the position (pos - origin) is
        // below the input total.
        let limit = self.in_total;
        self.generate(out, Some(limit));
    }

    fn generate(&mut self, out: &mut Vec<f32>, limit: Option<u64>) {
        let Some(f) = &self.filter else { return };
        let len = self.hist.first().map_or(0, Vec::len);
        loop {
            // Needs hist[pos - half + 1 ..= pos + half].
            if self.pos + f.half >= len {
                break;
            }
            if let Some(limit) = limit
                && self.pos as i64 - self.origin >= limit as i64
            {
                break;
            }
            let start = self.pos + 1 - f.half;
            let row: &[f32] = if f.exact {
                f.row(self.frac as usize)
            } else {
                // Interpolate between the two neighbouring tabulated phases.
                let x = self.frac as f64 * f.phases as f64 / f.l as f64;
                let p0 = (x.floor() as usize).min(f.phases - 1);
                let t = (x - p0 as f64) as f32;
                let (r0, r1) = (f.row(p0), f.row(p0 + 1));
                for ((s, a), b) in self.scratch.iter_mut().zip(r0).zip(r1) {
                    *s = a + (b - a) * t;
                }
                &self.scratch
            };
            for h in &self.hist {
                out.push(dot(&h[start..start + f.taps], row));
            }
            self.out_total += 1;
            self.frac += f.m;
            self.pos += (self.frac / f.l) as usize;
            self.frac %= f.l;
        }
    }

    fn compact(&mut self) {
        let Some(f) = &self.filter else { return };
        let keep_from = self.pos + 1 - f.half;
        if keep_from >= COMPACT_AT {
            for h in &mut self.hist {
                h.drain(..keep_from);
            }
            self.pos -= keep_from;
            self.origin -= keep_from as i64;
        }
    }
}

/// `ceil(frames * out_rate / in_rate)`: the output frames of a segment of
/// `frames` input frames.
pub fn expected_out_frames(frames: u64, in_rate: u32, out_rate: u32) -> u64 {
    let num = u128::from(frames) * u128::from(out_rate);
    let den = u128::from(in_rate);
    num.div_ceil(den) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f64, frames: usize, amp: f32) -> Vec<f32> {
        (0..frames)
            .map(|i| amp * (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin() as f32)
            .collect()
    }

    fn run(in_rate: u32, out_rate: u32, input: &[f32], chunk: usize) -> Vec<f32> {
        let mut r = Resampler::new(in_rate, out_rate, 1);
        let mut out = Vec::new();
        for c in input.chunks(chunk) {
            r.process(&[c], c.len(), &mut out);
        }
        r.flush(&mut out);
        assert_eq!(out.len() as u64, expected_out_frames(input.len() as u64, in_rate, out_rate));
        out
    }

    #[test]
    fn exact_counts() {
        for &(a, b) in &[(44100, 48000), (48000, 44100), (22050, 48000), (8000, 44100), (96000, 48000), (32000, 48000), (44100, 44100), (11025, 48000), (7350, 48000), (37800, 48000), (44100, 192000), (12345, 48000)] {
            for &n in &[0usize, 1, 2, 100, 1151, 4410, 10007] {
                let input = sine(a, 440.0, n, 0.5);
                for &chunk in &[1usize, 7, 1152, 100000] {
                    run(a, b, &input, chunk.max(1));
                }
            }
        }
    }

    #[test]
    fn chunking_does_not_change_output() {
        let input = sine(44100, 1000.0, 20000, 0.5);
        let a = run(44100, 48000, &input, 20000);
        let b = run(44100, 48000, &input, 333);
        assert_eq!(a, b);
    }

    /// Signal-to-error ratio of a converted sine against the ideal one,
    /// away from the ends.
    fn snr(in_rate: u32, out_rate: u32, freq: f64) -> f64 {
        let n = in_rate as usize;
        let input = sine(in_rate, freq, n, 0.5);
        let out = run(in_rate, out_rate, &input, 4096);
        let ideal = sine(out_rate, freq, out.len(), 0.5);
        let skip = out_rate as usize / 10;
        let (mut s, mut e) = (0.0f64, 0.0f64);
        for i in skip..out.len() - skip {
            s += f64::from(ideal[i]).powi(2);
            e += f64::from(out[i] - ideal[i]).powi(2);
        }
        10.0 * (s / e).log10()
    }

    #[test]
    fn passband_quality() {
        for &(a, b) in &[(44100, 48000), (48000, 44100), (22050, 48000), (32000, 48000)] {
            for &f in &[100.0, 1000.0, 5000.0] {
                let q = snr(a, b, f);
                assert!(q > 70.0, "{a}->{b} at {f} Hz: {q:.1} dB");
            }
        }
        let hi = snr(44100, 48000, 16000.0);
        assert!(hi > 60.0, "16 kHz: {hi:.1} dB");
    }

    #[test]
    fn stopband_rejection() {
        // A 23 kHz tone at 48 kHz has no place below 22.05 kHz.
        let input = sine(48000, 23000.0, 48000, 0.5);
        let out = run(48000, 44100, &input, 4096);
        let skip = 4410;
        let rms = (out[skip..out.len() - skip].iter().map(|x| f64::from(*x).powi(2)).sum::<f64>()
            / (out.len() - 2 * skip) as f64)
            .sqrt();
        let db = 20.0 * (rms / (0.5 / 2f64.sqrt())).log10();
        assert!(db < -60.0, "alias at {db:.1} dB");
    }

    /// Prints the converter's measured quality (run with --ignored
    /// --nocapture): signal-to-error ratio of sines, the passband gain and
    /// the rejection of a tone above the lower Nyquist frequency.
    #[test]
    #[ignore = "report"]
    fn quality_report() {
        for &(a, b) in &[(44100u32, 48000u32), (48000, 44100), (22050, 48000), (32000, 48000), (16000, 48000)] {
            let fs: Vec<String> = [100.0, 1000.0, 5000.0, 10000.0, 15000.0]
                .iter()
                .filter(|&&f| f < 0.42 * f64::from(a.min(b)))
                .map(|&f| format!("{f} Hz {:.1} dB", snr(a, b, f)))
                .collect();
            println!("{a}->{b}: SNR {}", fs.join(", "));
        }
        let gain = |a: u32, b: u32, f: f64| {
            let input = sine(a, f, a as usize, 0.5);
            let out = run(a, b, &input, 4096);
            let skip = b as usize / 10;
            let rms = (out[skip..out.len() - skip].iter().map(|x| f64::from(*x).powi(2)).sum::<f64>()
                / (out.len() - 2 * skip) as f64)
                .sqrt();
            20.0 * (rms / (0.5 / 2f64.sqrt())).log10()
        };
        for f in [1000.0, 10000.0, 16000.0, 18000.0, 19000.0, 20000.0, 21000.0] {
            println!("44100->48000 gain at {f} Hz: {:.3} dB", gain(44100, 48000, f));
        }
        for f in [22500.0, 23000.0, 23500.0] {
            println!("48000->44100 residue of {f} Hz: {:.1} dB", gain(48000, 44100, f));
        }
    }

    #[test]
    fn rate_change_keeps_time() {
        let input = sine(44100, 440.0, 44100, 0.5);
        let mut r = Resampler::new(44100, 48000, 1);
        let mut out = Vec::new();
        r.process(&[&input[..22050]], 22050, &mut out);
        let before = out.len();
        r.set_out_rate(96000);
        r.process(&[&input[22050..]], 22050, &mut out);
        r.flush(&mut out);
        // About half a second at each rate.
        let after = out.len() - before;
        assert!((before as i64 - 24000).abs() < 40, "{before}");
        assert!((after as i64 - 48000).abs() < 80, "{after}");
    }
}
