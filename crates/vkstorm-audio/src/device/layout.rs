//! Channel layouts: WAVEFORMATEXTENSIBLE channel masks and the mapping
//! between the mixer's interleave order and each backend's channel order.
//!
//! The mixer always writes a frame in ascending mask-bit order (FL FR FC
//! LFE BL BR FLC FRC BC SL SR ...), which is how WASAPI interleaves a
//! WAVEFORMATEXTENSIBLE stream, so on Windows the device order IS the
//! mixer order and nothing is permuted. PulseAudio and ALSA describe each
//! device channel by a position (`pa_channel_map`, `snd_pcm_chmap_t`); the
//! device layer converts those positions to mask bits and permutes each
//! frame from mask order into the device's order.
//!
//! What reached each speaker before this port (SoLoud + miniaudio 0.10.42,
//! verified in the vendored source): SoLoud was opened with
//! `channels = 0`, so miniaudio copied the device's own channel map into
//! the client map and its channel converter was a passthrough. Mixer slot
//! `i` went to device channel `i` on every backend, whatever the device's
//! map said, while SoLoud panned for the Windows orders (4: FL FR BL BR;
//! 6: FL FR FC LFE BL BR; 8, with the adapter's remap: FL FR FC LFE BL BR
//! SL SR).
//!
//! WASAPI: device channel `i` is the `i`-th set bit of the mix format's
//! mask, so slot `i` reached that speaker; with a 5.1 side mask (0x60F)
//! slots 4/5 reached SL/SR. The port is identical (no permutation).
//!
//! | ch | mask  | slot -> speaker, today and in the port |
//! |----|-------|----------------------------------------|
//! | 1  | 0x4   | 0 mono                                 |
//! | 2  | 0x3   | 0 FL, 1 FR                             |
//! | 4  | 0x33  | 0 FL, 1 FR, 2 BL, 3 BR                 |
//! | 6  | 0x3F  | 0 FL, 1 FR, 2 FC, 3 LFE, 4 BL, 5 BR    |
//! | 6  | 0x60F | 0 FL, 1 FR, 2 FC, 3 LFE, 4 SL, 5 SR    |
//! | 8  | 0x63F | 0 FL, 1 FR, 2 FC, 3 LFE, 4 BL, 5 BR, 6 SL, 7 SR |
//!
//! PulseAudio (default sink, its own map) and ALSA (`snd_pcm_get_chmap`):
//! sinks in WAVEFORMATEXTENSIBLE order (PipeWire's usual 5.1 "FL FR FC LFE
//! RL RR", every stereo/quad sink) behaved as on Windows. Sinks in ALSA
//! order (PulseAudio's `module-alsa-card` surround profiles: FL FR RL RR FC
//! LFE [SL SR]) were misrouted today; the port routes by position:
//!
//! | ch | device order       | today: slot -> speaker            | port: device channel <- slot |
//! |----|--------------------|-----------------------------------|------------------------------|
//! | 6  | FL FR RL RR FC LFE | 2 (FC) -> RL, 3 (LFE) -> RR, 4 (BL) -> FC, 5 (BR) -> LFE | RL<-4, RR<-5, FC<-2, LFE<-3 |
//! | 8  | FL FR RL RR FC LFE SL SR | as 6, plus 6 SL, 7 SR       | as 6, SL<-6, SR<-7           |
//!
//! That routing is a correction (centre and LFE no longer land on the rear
//! speakers), flagged in the rung report. miniaudio's ALSA backend asked
//! for 2 channels (`MA_DEFAULT_CHANNELS`, see `alsa.rs`), so it never ran
//! surround; only PulseAudio surround sinks were affected. A map with an
//! unknown or repeated position keeps today's passthrough.

#![forbid(unsafe_code)]

use crate::music::{
    SPEAKER_BACK_LEFT, SPEAKER_BACK_RIGHT, SPEAKER_FRONT_CENTER, SPEAKER_FRONT_LEFT,
    SPEAKER_FRONT_RIGHT, SPEAKER_LOW_FREQUENCY, SPEAKER_SIDE_LEFT, SPEAKER_SIDE_RIGHT,
};

pub const SPEAKER_FRONT_LEFT_OF_CENTER: u32 = 0x40;
pub const SPEAKER_FRONT_RIGHT_OF_CENTER: u32 = 0x80;
pub const SPEAKER_BACK_CENTER: u32 = 0x100;
pub const SPEAKER_TOP_CENTER: u32 = 0x800;
pub const SPEAKER_TOP_FRONT_LEFT: u32 = 0x1000;
pub const SPEAKER_TOP_FRONT_CENTER: u32 = 0x2000;
pub const SPEAKER_TOP_FRONT_RIGHT: u32 = 0x4000;
pub const SPEAKER_TOP_BACK_LEFT: u32 = 0x8000;
pub const SPEAKER_TOP_BACK_CENTER: u32 = 0x10000;
pub const SPEAKER_TOP_BACK_RIGHT: u32 = 0x20000;

/// The channel counts the engine accepts (the 3p-soloud device-selection
/// patch rejected anything else after negotiation).
pub fn is_supported_channel_count(channels: u32) -> bool {
    matches!(channels, 1 | 2 | 4 | 6 | 8)
}

/// The mask assumed for a channel count when the device does not give a
/// usable one: the layouts SoLoud panned for (mono centre, stereo,
/// KSAUDIO_SPEAKER_QUAD, KSAUDIO_SPEAKER_5POINT1, KSAUDIO_SPEAKER_7POINT1_SURROUND).
pub fn default_mask(channels: u32) -> u32 {
    match channels {
        1 => SPEAKER_FRONT_CENTER,
        2 => SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT,
        4 => SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT | SPEAKER_BACK_LEFT | SPEAKER_BACK_RIGHT,
        6 => {
            SPEAKER_FRONT_LEFT
                | SPEAKER_FRONT_RIGHT
                | SPEAKER_FRONT_CENTER
                | SPEAKER_LOW_FREQUENCY
                | SPEAKER_BACK_LEFT
                | SPEAKER_BACK_RIGHT
        }
        8 => {
            SPEAKER_FRONT_LEFT
                | SPEAKER_FRONT_RIGHT
                | SPEAKER_FRONT_CENTER
                | SPEAKER_LOW_FREQUENCY
                | SPEAKER_BACK_LEFT
                | SPEAKER_BACK_RIGHT
                | SPEAKER_SIDE_LEFT
                | SPEAKER_SIDE_RIGHT
        }
        _ => 0,
    }
}

/// The effective mask of a WASAPI mix format, following miniaudio's
/// `ma_channel_mask_to_channel_map__win32`: channel `i` is the `i`-th set
/// bit (bits beyond the channel count are ignored); a mono stream with
/// no mask or with the centre bit is mono (reported as front centre).
/// When the mask names fewer speakers than there are channels (including
/// a plain WAVEFORMATEX with no mask), miniaudio left the rest unnamed
/// and passed the mixer's slots straight through, so the layout the
/// mixer panned for, [`default_mask`], is what reached the speakers.
pub fn wasapi_effective_mask(channels: u32, dw_channel_mask: u32) -> u32 {
    if channels == 1 && (dw_channel_mask == 0 || dw_channel_mask & SPEAKER_FRONT_CENTER != 0) {
        return SPEAKER_FRONT_CENTER;
    }
    let mut mask = 0u32;
    let mut taken = 0u32;
    for bit in 0..32 {
        if taken == channels {
            break;
        }
        if dw_channel_mask & (1 << bit) != 0 {
            mask |= 1 << bit;
            taken += 1;
        }
    }
    if taken < channels {
        default_mask(channels)
    } else {
        mask
    }
}

/// PulseAudio channel positions (`pa_channel_position_t`, pulse/channelmap.h)
/// to mask bits; 0 for positions without a WAVEFORMATEXTENSIBLE speaker.
pub fn pulse_position_bit(position: i32) -> u32 {
    match position {
        0 => SPEAKER_FRONT_CENTER, // MONO
        1 => SPEAKER_FRONT_LEFT,
        2 => SPEAKER_FRONT_RIGHT,
        3 => SPEAKER_FRONT_CENTER,
        4 => SPEAKER_BACK_CENTER, // REAR_CENTER
        5 => SPEAKER_BACK_LEFT,   // REAR_LEFT
        6 => SPEAKER_BACK_RIGHT,  // REAR_RIGHT
        7 => SPEAKER_LOW_FREQUENCY,
        8 => SPEAKER_FRONT_LEFT_OF_CENTER,
        9 => SPEAKER_FRONT_RIGHT_OF_CENTER,
        10 => SPEAKER_SIDE_LEFT,
        11 => SPEAKER_SIDE_RIGHT,
        44 => SPEAKER_TOP_CENTER,
        45 => SPEAKER_TOP_FRONT_LEFT,
        46 => SPEAKER_TOP_FRONT_RIGHT,
        47 => SPEAKER_TOP_FRONT_CENTER,
        48 => SPEAKER_TOP_BACK_LEFT,
        49 => SPEAKER_TOP_BACK_RIGHT,
        50 => SPEAKER_TOP_BACK_CENTER,
        _ => 0, // INVALID, AUX0..31
    }
}

/// ALSA channel-map positions (`enum snd_pcm_chmap_position`, alsa/pcm.h)
/// to mask bits; 0 for positions without a WAVEFORMATEXTENSIBLE speaker.
/// The phase-inverse and driver-specific flag bits are ignored.
pub fn alsa_position_bit(position: u32) -> u32 {
    match position & 0xffff {
        2 => SPEAKER_FRONT_CENTER, // MONO
        3 => SPEAKER_FRONT_LEFT,
        4 => SPEAKER_FRONT_RIGHT,
        5 => SPEAKER_BACK_LEFT,  // RL
        6 => SPEAKER_BACK_RIGHT, // RR
        7 => SPEAKER_FRONT_CENTER,
        8 => SPEAKER_LOW_FREQUENCY,
        9 => SPEAKER_SIDE_LEFT,
        10 => SPEAKER_SIDE_RIGHT,
        11 => SPEAKER_BACK_CENTER, // RC
        12 => SPEAKER_FRONT_LEFT_OF_CENTER,
        13 => SPEAKER_FRONT_RIGHT_OF_CENTER,
        21 => SPEAKER_TOP_CENTER,
        22 => SPEAKER_TOP_FRONT_LEFT,
        23 => SPEAKER_TOP_FRONT_RIGHT,
        24 => SPEAKER_TOP_FRONT_CENTER,
        25 => SPEAKER_TOP_BACK_LEFT,
        26 => SPEAKER_TOP_BACK_RIGHT,
        27 => SPEAKER_TOP_BACK_CENTER,
        _ => 0,
    }
}

/// miniaudio's `ma_get_standard_channel_map_alsa`, as mask bits per device
/// channel: what it assumed when a PCM had no channel map.
pub fn alsa_standard_positions(channels: u32) -> Vec<u32> {
    let all = [
        SPEAKER_FRONT_LEFT,
        SPEAKER_FRONT_RIGHT,
        SPEAKER_BACK_LEFT,
        SPEAKER_BACK_RIGHT,
        SPEAKER_FRONT_CENTER,
        SPEAKER_LOW_FREQUENCY,
        SPEAKER_SIDE_LEFT,
        SPEAKER_SIDE_RIGHT,
    ];
    match channels {
        1 => vec![SPEAKER_FRONT_CENTER],
        n if n as usize <= all.len() => all[..n as usize].to_vec(),
        n => vec![0; n as usize],
    }
}

/// How the mixer's mask-ordered frame maps onto the device's channels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceOrder {
    /// The layout reported to the mixer.
    pub mask: u32,
    /// `source[d]` = the mixer slot written to device channel `d`; `None`
    /// when the orders coincide (no permutation).
    pub source: Option<Vec<usize>>,
}

impl DeviceOrder {
    /// The mapping for a device whose channel `d` is the speaker
    /// `positions[d]` (a single mask bit, or 0 when unknown). A map with an
    /// unknown or repeated position cannot be described by a mask; it then
    /// gets the default layout for its channel count and no permutation,
    /// which is what the SoLoud passthrough did for every device.
    pub fn from_positions(positions: &[u32]) -> DeviceOrder {
        let channels = positions.len() as u32;
        let mut mask = 0u32;
        let mut valid = true;
        for &bit in positions {
            if bit == 0 || bit.count_ones() != 1 || mask & bit != 0 {
                valid = false;
                break;
            }
            mask |= bit;
        }
        if !valid {
            return DeviceOrder {
                mask: default_mask(channels),
                source: None,
            };
        }
        // The mixer slot of a speaker is its rank among the mask's bits.
        let source: Vec<usize> = positions
            .iter()
            .map(|&bit| (mask & (bit - 1)).count_ones() as usize)
            .collect();
        let identity = source.iter().enumerate().all(|(d, &s)| d == s);
        DeviceOrder {
            mask,
            source: if identity { None } else { Some(source) },
        }
    }

    /// Reorders interleaved frames in place from mixer order to device
    /// order. `channels` is at most 8 for every accepted layout; larger
    /// layouts are never built (they fail to open).
    pub fn apply(&self, samples: &mut [f32], channels: usize) {
        let Some(source) = &self.source else { return };
        debug_assert_eq!(source.len(), channels);
        let mut frame = [0f32; 32];
        if channels == 0 || channels > frame.len() {
            return;
        }
        for chunk in samples.chunks_exact_mut(channels) {
            frame[..channels].copy_from_slice(chunk);
            for (d, out) in chunk.iter_mut().enumerate() {
                *out = frame[source[d]];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wasapi_masks() {
        // Common mix formats pass through.
        assert_eq!(wasapi_effective_mask(2, 0x3), 0x3);
        assert_eq!(wasapi_effective_mask(6, 0x3f), 0x3f);
        assert_eq!(wasapi_effective_mask(6, 0x60f), 0x60f);
        assert_eq!(wasapi_effective_mask(8, 0x63f), 0x63f);
        assert_eq!(wasapi_effective_mask(4, 0x33), 0x33);
        // Mono: no mask or centre -> centre; a single other bit is kept.
        assert_eq!(wasapi_effective_mask(1, 0), SPEAKER_FRONT_CENTER);
        assert_eq!(wasapi_effective_mask(1, 0x4), SPEAKER_FRONT_CENTER);
        assert_eq!(wasapi_effective_mask(1, 0x1), 0x1);
        // Stereo with no mask (plain WAVEFORMATEX) is FL FR.
        assert_eq!(wasapi_effective_mask(2, 0), 0x3);
        // Excess bits are ignored, as miniaudio stopped at the count.
        assert_eq!(wasapi_effective_mask(2, 0x3f), 0x3);
        // Too few bits: the layout SoLoud panned for.
        assert_eq!(wasapi_effective_mask(6, 0), 0x3f);
        assert_eq!(wasapi_effective_mask(8, 0x3f), 0x63f);
    }

    #[test]
    fn default_masks() {
        assert_eq!(default_mask(1), 0x4);
        assert_eq!(default_mask(2), 0x3);
        assert_eq!(default_mask(4), 0x33);
        assert_eq!(default_mask(6), 0x3f);
        assert_eq!(default_mask(8), 0x63f);
        for c in 0..=9 {
            assert_eq!(
                is_supported_channel_count(c),
                matches!(c, 1 | 2 | 4 | 6 | 8)
            );
        }
    }

    fn pulse_order(positions: &[i32]) -> DeviceOrder {
        let bits: Vec<u32> = positions.iter().map(|&p| pulse_position_bit(p)).collect();
        DeviceOrder::from_positions(&bits)
    }

    #[test]
    fn pulse_alsa_ordered_51_is_permuted() {
        // front-left,front-right,rear-left,rear-right,front-center,lfe
        let order = pulse_order(&[1, 2, 5, 6, 3, 7]);
        assert_eq!(order.mask, 0x3f);
        // Device channel d takes mixer slot source[d]
        // (mixer: 0 FL 1 FR 2 FC 3 LFE 4 BL 5 BR).
        assert_eq!(order.source, Some(vec![0, 1, 4, 5, 2, 3]));
        let mut frame = [10.0, 11.0, 12.0, 13.0, 14.0, 15.0];
        order.apply(&mut frame, 6);
        assert_eq!(frame, [10.0, 11.0, 14.0, 15.0, 12.0, 13.0]);
    }

    #[test]
    fn pulse_alsa_ordered_71_is_permuted() {
        // FL FR RL RR FC LFE SL SR
        let order = pulse_order(&[1, 2, 5, 6, 3, 7, 10, 11]);
        assert_eq!(order.mask, 0x63f);
        assert_eq!(order.source, Some(vec![0, 1, 4, 5, 2, 3, 6, 7]));
    }

    #[test]
    fn pulse_wave_ordered_maps_are_identity() {
        assert_eq!(pulse_order(&[1, 2]).source, None);
        assert_eq!(pulse_order(&[1, 2]).mask, 0x3);
        assert_eq!(pulse_order(&[0]).mask, SPEAKER_FRONT_CENTER);
        let o = pulse_order(&[1, 2, 3, 7, 5, 6]);
        assert_eq!((o.mask, o.source), (0x3f, None));
        let o = pulse_order(&[1, 2, 3, 7, 10, 11]);
        assert_eq!((o.mask, o.source), (0x60f, None));
        let o = pulse_order(&[1, 2, 5, 6]);
        assert_eq!((o.mask, o.source), (0x33, None));
    }

    #[test]
    fn unknown_or_duplicate_positions_fall_back_to_passthrough() {
        // aux0 has no speaker bit.
        let o = pulse_order(&[1, 2, 12, 13, 3, 7]);
        assert_eq!((o.mask, o.source), (0x3f, None));
        // Duplicate front-left.
        let o = pulse_order(&[1, 1]);
        assert_eq!((o.mask, o.source), (0x3, None));
    }

    #[test]
    fn alsa_positions() {
        let bits: Vec<u32> = [3u32, 4, 5, 6, 7, 8]
            .iter()
            .map(|&p| alsa_position_bit(p))
            .collect();
        let o = DeviceOrder::from_positions(&bits);
        assert_eq!(o.mask, 0x3f);
        assert_eq!(o.source, Some(vec![0, 1, 4, 5, 2, 3]));
        // Phase-inverse flag is ignored.
        assert_eq!(alsa_position_bit(0x10000 | 3), SPEAKER_FRONT_LEFT);
        assert_eq!(alsa_standard_positions(2), vec![0x1, 0x2]);
        assert_eq!(
            DeviceOrder::from_positions(&alsa_standard_positions(8)).source,
            Some(vec![0, 1, 4, 5, 2, 3, 6, 7])
        );
    }
}
