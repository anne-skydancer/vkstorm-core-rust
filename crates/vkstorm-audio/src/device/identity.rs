//! Output device identity: the UUID the viewer stores in its settings.
//!
//! The SoLoud backend (`LLAudioEngine_SoLoud::getDevices`) derived each
//! endpoint's `LLUUID` as `MD5(ma_get_backend_name(backend) ++ raw id)`,
//! with the 16 digest bytes taken in order as `LLUUID::mData`. For WASAPI
//! (the only backend it ever enumerated on Windows) the backend name is
//! miniaudio 0.10.42's `"WASAPI"` and the raw id is the endpoint id string
//! from `IMMDevice::GetId` as its UTF-16LE `wchar_t` bytes without the
//! terminator (`wcslen * sizeof(wchar_t)`). Reproducing that exactly keeps
//! every user's saved output device selected after the switch.

#![forbid(unsafe_code)]

use std::fmt;

use super::md5::Md5;

/// The backend name the WASAPI identities are hashed with
/// (`ma_get_backend_name(ma_backend_wasapi)` in miniaudio 0.10.42).
pub const WASAPI_IDENTITY_NAME: &str = "WASAPI";

/// A device identity in the viewer's `LLUUID` byte order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct DeviceUuid(pub [u8; 16]);

impl DeviceUuid {
    pub const NULL: DeviceUuid = DeviceUuid([0; 16]);

    pub fn is_null(&self) -> bool {
        self.0 == [0; 16]
    }

    /// `MD5(backend_name ++ raw_id)`, as the SoLoud backend computed it.
    pub fn derive(backend_name: &str, raw_id: &[u8]) -> DeviceUuid {
        let mut h = Md5::new();
        h.update(backend_name.as_bytes());
        h.update(raw_id);
        DeviceUuid(h.finalize())
    }

    /// The identity of a WASAPI endpoint from its `IMMDevice::GetId`
    /// string (UTF-16 code units, without the terminating zero).
    pub fn from_wasapi_endpoint_id(id: &[u16]) -> DeviceUuid {
        let mut bytes = Vec::with_capacity(id.len() * 2);
        for unit in id {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        DeviceUuid::derive(WASAPI_IDENTITY_NAME, &bytes)
    }
}

impl fmt::Display for DeviceUuid {
    /// `LLUUID::toString` form: lowercase, 8-4-4-4-12.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, b) in self.0.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for DeviceUuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceUuid({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// Reference values: Python `hashlib.md5(b"WASAPI" +
    /// id.encode("utf-16-le"))`, which is what `LLMD5` computes over the
    /// same bytes (`update(std::string)` then `update(wchar_t bytes)`).
    #[test]
    fn wasapi_identity_matches_the_soloud_backend() {
        let id = "{0.0.0.00000000}.{8e5b2b74-6d2e-4c9a-9b5f-3a1d2c4e5f60}";
        assert_eq!(
            DeviceUuid::from_wasapi_endpoint_id(&wide(id)).to_string(),
            "fe887044-c442-545a-8762-a161afe9cd8d"
        );
        let id = "{0.0.0.00000000}.{00000000-0000-0000-0000-000000000000}";
        assert_eq!(
            DeviceUuid::from_wasapi_endpoint_id(&wide(id)).to_string(),
            "ed3e6fc6-c53d-2e4e-ae4d-b6ecdfd33f99"
        );
    }

    #[test]
    fn derive_hashes_name_then_id() {
        let a = DeviceUuid::derive("WASAPI", b"\x7b\x00");
        let mut joined = b"WASAPI".to_vec();
        joined.extend_from_slice(b"\x7b\x00");
        assert_eq!(a.0, super::super::md5::md5(&joined));
        assert!(!a.is_null());
        assert!(DeviceUuid::NULL.is_null());
    }
}
