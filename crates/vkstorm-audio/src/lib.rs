//! The audio engine of Vulkanstorm (vkstorm-rust rung six).
//!
//! - [`mixer`]: the sound-effect mixer: voices, 3D positioning, attenuation,
//!   Doppler, faders, wind, the music bus with its fade envelope and speaker
//!   fill, mixed into the output device's channel layout.
//! - [`device`]: the output device: WASAPI on Windows, PulseAudio/ALSA on
//!   Linux, and a null device that renders on demand (tests, offline).
//! - [`engine`]: the mixer behind its mutex, the output device that drives
//!   it, restarts and device selection; the layer the FFI calls.
//! - [`asset`]: sound asset decoding (Ogg Vorbis to the decoded WAV cache file).
//! - [`wav`]: the WAV reader the buffers load decoded and local sounds with.
//! - [`music`]: the contract between music decoding and the mixer.
//! - [`stream`]: parcel music: HTTP/ICY streaming, playlists, demuxing and
//!   decoding, metadata, reconnects.

pub mod asset;
pub mod device;
pub mod engine;
pub mod mixer;
pub mod music;
pub mod stream;
pub mod wav;

/// The SoLoud the mixer is a port of: release 202002 at upstream commit
/// e82fd32c1f (https://github.com/jarikomppa/soloud) with the viewer
/// package's patches (3p-soloud, `VERSION.txt` 202002.e82fd32c1f.vulkanstorm3:
/// bounded 1024-frame mixing, device selection and native format
/// negotiation). The mixer reproduces that SoLoud's output bit for bit; to
/// follow a newer one, port its changes, re-run `tools/soloud-oracle` against
/// it and update this constant and [`ENGINE_INFO`].
pub const UPSTREAM_BASELINE: UpstreamBaseline = UpstreamBaseline {
    upstream: "e82fd32c1f62183922f08c14c814a02b58db1873",
    version: "202002",
    patch_level: "vulkanstorm3",
};

/// See [`UPSTREAM_BASELINE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpstreamBaseline {
    /// The upstream SoLoud commit.
    pub upstream: &'static str,
    /// The SoLoud release that commit is (`SOLOUD_VERSION`).
    pub version: &'static str,
    /// The 3p-soloud package revision whose patches the port follows.
    pub patch_level: &'static str,
}

/// The engine's name for the viewer's "Audio Driver Version" (the J2C codec
/// reports itself the same way). A test keeps it in step with
/// [`UPSTREAM_BASELINE`].
pub const ENGINE_INFO: &str = "SoLoud 202002 (Rust)";

#[cfg(test)]
mod baseline_tests {
    #[test]
    fn engine_info_names_the_baseline() {
        let expected = format!("SoLoud {} (Rust)", super::UPSTREAM_BASELINE.version);
        assert_eq!(super::ENGINE_INFO, expected);
        assert_eq!(super::UPSTREAM_BASELINE.upstream.len(), 40);
    }
}
