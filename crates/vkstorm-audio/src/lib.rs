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
pub mod wav;
