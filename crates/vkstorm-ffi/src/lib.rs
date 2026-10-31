//! C ABI for the C++ viewer (vulkanstorm). Each function is `extern "C"`,
//! catches panics at the boundary, and uses only plain types. The matching
//! header is `include/vkstorm_ffi.h`; keep the two in step.
//!
//! Built as a static library and shipped to the viewer as an autobuild
//! package (see `packaging/`), so viewer builds need no Rust toolchain. Each
//! rung of the port is a Cargo feature; a viewer line links a build with the
//! rungs it has adopted, and `full` enables them all:
//!
//! - rung one, `j2c`: JPEG 2000 decoding and encoding via `vkstorm-j2c`;
//! - rung two, `llsd`: LLSD serialization via `vkstorm-llsd` (`vks_llsd_*`),
//!   `volume`: the parametric prim mesher via `vkstorm-volume`, and `image`:
//!   PNG, TGA, BMP and DXT via `vkstorm-image`;
//! - rung three, `messages`: the message template system and the data packers
//!   via `vkstorm-messages`;
//! - rung four, `http`: the HTTP layer via `vkstorm-http`;
//! - rung five, `assets`: the asset pipeline via `vkstorm-assets`;
//! - rung six, `audio`: the audio engine, sound asset decoding and parcel
//!   music via `vkstorm-audio`;
//! - rung seven, `inventory`: the inventory data model (sub-rung R1:
//!   permissions, sale info, item, category and object records, their
//!   legacy text and message block forms, notecards; sub-rung R3a: AIS
//!   replies, from the raw HTTP response to the parse `doUpdate()` applies)
//!   via `vkstorm-inventory` (`vks_inv_*`, `vks_inv_ais_*`).
//!
//! The header declares every rung's functions; a build without a rung simply
//! does not define them, so a viewer must not call a rung it has not adopted.

#[cfg(feature = "assets")]
mod assets;
#[cfg(feature = "audio")]
mod audio;
#[cfg(feature = "http")]
mod http;
#[cfg(feature = "image")]
mod image;
#[cfg(feature = "inventory")]
mod inventory;
#[cfg(feature = "inventory")]
mod inventory_r3a;
#[cfg(feature = "j2c")]
mod j2c;
#[cfg(feature = "llsd")]
mod llsd;
#[cfg(feature = "messages")]
mod messages;
#[cfg(feature = "volume")]
mod volume;
