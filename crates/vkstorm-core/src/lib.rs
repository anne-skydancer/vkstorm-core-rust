//! Vulkanstorm core types shared by every viewer track.
//!
//! This crate is the bottom of the dependency graph: no I/O, no rendering, no UI.
//! It will hold LLSD and its three codecs, UUIDs, the region/global/agent
//! coordinate frames as distinct types, asset and inventory enums, permissions
//! and the binary packers used by the message and asset codecs.
//!
//! The `opensim` feature enables OpenSim grid support. Without it the build is
//! Second Life only: no OpenSim grid profile exists and none of the legacy
//! UDP protocol paths are compiled. With it, the grid profile is chosen at
//! login and fixed for the whole session; Second Life and OpenSim are never
//! mixed in one session.

/// Whether this build includes OpenSim grid support.
pub const OPENSIM_SUPPORT: bool = cfg!(feature = "opensim");
