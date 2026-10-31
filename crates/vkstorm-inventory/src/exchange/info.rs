//! `LLOfferInfo`'s state and its LLSD form (`llviewermessage.cpp`), and the
//! `LLHost` text rules it relies on (`llhost.cpp`, `net.cpp`).

use vkstorm_llsd::Value;

use crate::llsd::coerce::{as_boolean, as_integer, as_string, as_uuid, get, insert};
use crate::perms::{NULL_ID, Uuid};
use crate::types::AssetType;

use super::im::IM_NOTHING_SPECIAL;
use super::names::atol;

/// `LLHost`: an IPv4 address (the four bytes in network order, as `mIP`
/// holds them in memory) and a port. The default is `0.0.0.0:0`
/// (`INVALID_HOST_IP_ADDRESS`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NetHost {
    pub ip: [u8; 4],
    pub port: u32,
}

impl NetHost {
    /// `LLHost(const std::string& ip_and_port)`: the text before the first
    /// `:` through `ip_string_to_u32`, the rest through `atol` into the
    /// `U32` port (so the platform's `long` decides an out-of-range port:
    /// see [`atol`]); without a colon, port 0.
    pub fn parse(s: &[u8]) -> NetHost {
        match s.iter().position(|&c| c == b':') {
            None => NetHost {
                ip: ip_string_to_u32(s),
                port: 0,
            },
            Some(i) => NetHost {
                ip: ip_string_to_u32(&s[..i]),
                port: atol(&s[i + 1..]) as u32,
            },
        }
    }

    /// `getIPandPort()`: `inet_ntoa` of the address, `:`, the port (`%u`).
    pub fn ip_and_port(&self) -> String {
        format!(
            "{}.{}.{}.{}:{}",
            self.ip[0], self.ip[1], self.ip[2], self.ip[3], self.port
        )
    }
}

/// The bytes before the first NUL (`c_str()` semantics).
fn c_str(s: &[u8]) -> &[u8] {
    &s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())]
}

/// `ip_string_to_u32`: `inet_addr`, where a failure gives
/// `INVALID_HOST_IP_ADDRESS` (0) unless the text is the broadcast address
/// `255.255.255.255` (whose `inet_addr` value is `INADDR_NONE`).
fn ip_string_to_u32(s: &[u8]) -> [u8; 4] {
    let s = c_str(s);
    inet_addr(s).unwrap_or(if s == b"255.255.255.255" {
        [255; 4]
    } else {
        [0; 4]
    })
}

/// `inet_addr`: one to four parts separated by dots, each decimal, octal
/// (leading `0`) or hexadecimal (`0x`); the last part fills the remaining
/// bytes. `None` is `INADDR_NONE` (including the all-ones address itself).
fn inet_addr(s: &[u8]) -> Option<[u8; 4]> {
    if s.is_empty() {
        return None;
    }
    let mut parts: Vec<u64> = Vec::new();
    for part in s.split(|&c| c == b'.') {
        if part.is_empty() || parts.len() == 4 {
            return None;
        }
        let (digits, radix) =
            if part.len() > 1 && part[0] == b'0' && (part[1] == b'x' || part[1] == b'X') {
                (&part[2..], 16)
            } else if part.len() > 1 && part[0] == b'0' {
                (&part[1..], 8)
            } else {
                (part, 10)
            };
        let mut v: u64 = 0;
        for &c in digits {
            let d = (c as char).to_digit(radix)? as u64;
            v = v.checked_mul(radix as u64)?.checked_add(d)?;
            if v > u32::MAX as u64 {
                return None;
            }
        }
        parts.push(v);
    }
    let n = parts.len();
    let mut addr: u64 = 0;
    for (i, &p) in parts.iter().enumerate() {
        if i + 1 < n {
            if p > 255 {
                return None;
            }
            addr |= p << (24 - 8 * i);
        } else {
            let max = if n == 1 {
                u32::MAX as u64
            } else {
                (1u64 << (8 * (4 - i))) - 1
            };
            if p > max {
                return None;
            }
            addr |= p;
        }
    }
    if addr == u32::MAX as u64 {
        return None;
    }
    Some((addr as u32).to_be_bytes())
}

/// `LLOfferInfo`'s fields. The dialog and the asset type are raw integers
/// (the C++ casts whatever arrives to its enums).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfferInfo {
    /// `mIM`.
    pub im: i32,
    /// `mFromID`.
    pub from_id: Uuid,
    /// `mFromGroup`.
    pub from_group: bool,
    /// `mFromObject`.
    pub from_object: bool,
    /// `mTransactionID`.
    pub transaction_id: Uuid,
    /// `mFolderID`.
    pub folder_id: Uuid,
    /// `mObjectID`.
    pub object_id: Uuid,
    /// `mType`.
    pub ty: AssetType,
    /// `mFromName`.
    pub from_name: Vec<u8>,
    /// `mDesc`.
    pub desc: Vec<u8>,
    /// `mHost`.
    pub host: NetHost,
    /// `mPersist`.
    pub persist: bool,
}

impl Default for OfferInfo {
    /// `LLOfferInfo()`: `IM_NOTHING_SPECIAL`, `AT_NONE`, null ids, empty
    /// names, the invalid host, not persistent.
    fn default() -> Self {
        OfferInfo {
            im: IM_NOTHING_SPECIAL,
            from_id: NULL_ID,
            from_group: false,
            from_object: false,
            transaction_id: NULL_ID,
            folder_id: NULL_ID,
            object_id: NULL_ID,
            ty: AssetType::NONE,
            from_name: Vec::new(),
            desc: Vec::new(),
            host: NetHost::default(),
            persist: false,
        }
    }
}

/// `LLOfferInfo::mResponderType`.
pub const RESPONDER_TYPE: &str = "offer_info";

impl OfferInfo {
    /// `LLOfferInfo(const LLSD&)`, and `fromLLSD` (`*this = params`, through
    /// that constructor): every field from its key with the LLSD
    /// conversions; the type by name (`LLAssetType::lookup(const char*)`, so
    /// an unknown or empty name is `AT_UNKNOWN`), the host by `LLHost`'s text
    /// rules.
    pub fn from_llsd(sd: &Value) -> OfferInfo {
        let ty_name = as_string(get(sd, "type"));
        OfferInfo {
            im: as_integer(get(sd, "im_type")),
            from_id: as_uuid(get(sd, "from_id")),
            from_group: as_boolean(get(sd, "from_group")),
            from_object: as_boolean(get(sd, "from_object")),
            transaction_id: as_uuid(get(sd, "transaction_id")),
            folder_id: as_uuid(get(sd, "folder_id")),
            object_id: as_uuid(get(sd, "object_id")),
            ty: AssetType::from_name_cstr(Some(&ty_name)),
            from_name: as_string(get(sd, "from_name")),
            desc: as_string(get(sd, "description")),
            host: NetHost::parse(&as_string(get(sd, "sender"))),
            persist: as_boolean(get(sd, "persist")),
        }
    }

    /// `asLLSD()`: the map the notification store persists.
    pub fn as_llsd(&self) -> Value {
        let mut sd = Value::Undefined;
        insert(&mut sd, "responder_type", Value::string(RESPONDER_TYPE));
        insert(&mut sd, "im_type", Value::Integer(self.im));
        insert(&mut sd, "from_id", Value::Uuid(self.from_id));
        insert(&mut sd, "from_group", Value::Boolean(self.from_group));
        insert(&mut sd, "from_object", Value::Boolean(self.from_object));
        insert(&mut sd, "transaction_id", Value::Uuid(self.transaction_id));
        insert(&mut sd, "folder_id", Value::Uuid(self.folder_id));
        insert(&mut sd, "object_id", Value::Uuid(self.object_id));
        insert(&mut sd, "type", Value::string(self.ty.lookup()));
        insert(&mut sd, "from_name", Value::String(self.from_name.clone()));
        insert(&mut sd, "description", Value::String(self.desc.clone()));
        insert(
            &mut sd,
            "sender",
            Value::String(self.host.ip_and_port().into_bytes()),
        );
        insert(&mut sd, "persist", Value::Boolean(self.persist));
        sd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        assert_eq!(
            NetHost::parse(b"10.0.0.9:13005").ip_and_port(),
            "10.0.0.9:13005"
        );
        assert_eq!(NetHost::parse(b"10.0.0.9").ip_and_port(), "10.0.0.9:0");
        assert_eq!(NetHost::parse(b"garbage:xx").ip_and_port(), "0.0.0.0:0");
        assert_eq!(
            NetHost::parse(b"localhost:13000").ip_and_port(),
            "0.0.0.0:13000"
        );
        assert_eq!(
            NetHost::parse(b"255.255.255.255:65535").ip_and_port(),
            "255.255.255.255:65535"
        );
        assert_eq!(NetHost::parse(b"").ip_and_port(), "0.0.0.0:0");
    }

    fn port(s: &[u8]) -> u32 {
        NetHost::parse(s).port
    }

    /// `atol`'s text rules, the same on both platforms.
    #[test]
    fn port_text_rules() {
        assert_eq!(port(b"1.2.3.4: \t+13000x"), 13000);
        assert_eq!(port(b"1.2.3.4:x13000"), 0);
        assert_eq!(port(b"1.2.3.4:- 1"), 0);
        assert_eq!(port(b"1.2.3.4:-1"), u32::MAX);
        assert_eq!(port(b"1.2.3.4:2147483647"), 2147483647);
        assert_eq!(port(b"1.2.3.4:-2147483648"), 2147483648);
        assert_eq!(port(b"1.2.3.4:1:2"), 1);
    }

    /// MSVC: a 32-bit `long`, saturated, then `U32`.
    #[cfg(windows)]
    #[test]
    fn port_overflow_msvc() {
        assert_eq!(port(b"1.2.3.4:2147483648"), 2147483647);
        assert_eq!(port(b"1.2.3.4:4294967297"), 2147483647);
        assert_eq!(port(b"1.2.3.4:99999999999999999999"), 2147483647);
        assert_eq!(port(b"1.2.3.4:-2147483649"), 2147483648);
        assert_eq!(port(b"1.2.3.4:-99999999999999999999"), 2147483648);
    }

    /// glibc: a 64-bit `long`, saturated, then its low 32 bits.
    #[cfg(not(windows))]
    #[test]
    fn port_overflow_glibc() {
        assert_eq!(port(b"1.2.3.4:2147483648"), 2147483648);
        assert_eq!(port(b"1.2.3.4:4294967297"), 1);
        assert_eq!(port(b"1.2.3.4:99999999999999999999"), u32::MAX);
        assert_eq!(port(b"1.2.3.4:-2147483649"), 2147483647);
        assert_eq!(port(b"1.2.3.4:-99999999999999999999"), 0);
        assert_eq!(port(b"1.2.3.4:-9223372036854775808"), 0);
    }
}
