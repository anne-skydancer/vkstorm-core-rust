//! Zero-coding: runs of zero bytes after the packet id are sent as `0`
//! followed by a one-byte count (`lltemplatemessagebuilder.cpp`'s
//! `zero_code`, `LLMessageSystem::zeroCodeExpand` and
//! `zeroCodeAdjustCurrentSendTotal`).
//!
//! The encoder counts a run up to 255 and starts a new `0 n` pair for the
//! rest; the decoder also understands the `0 0 n` wrap form (a zero count
//! byte means 256 zeros) that the encoder never produces. A packet is only
//! sent coded when coding makes it smaller; the flags byte then carries
//! [`ZERO_CODE_FLAG`](crate::ZERO_CODE_FLAG).

use crate::{PACKET_ID_SIZE, ZERO_CODE_FLAG};

/// The net size change zero-coding `packet` would make (`zeroCodeAdjustCurrentSendTotal`
/// without the build step): negative when coding saves bytes, else 0.
pub fn zero_code_gain(packet: &[u8]) -> i32 {
    let mut net_gain: i32 = 0;
    let mut num_zeroes: u8 = 0;
    for &b in packet.iter().skip(PACKET_ID_SIZE) {
        if b == 0 {
            if num_zeroes != 0 {
                num_zeroes = num_zeroes.wrapping_add(1);
                if num_zeroes > 254 {
                    num_zeroes = 0;
                }
                net_gain -= 1;
            } else {
                net_gain += 1;
                num_zeroes = 1;
            }
        } else {
            num_zeroes = 0;
        }
    }
    if net_gain < 0 { net_gain } else { 0 }
}

/// `zero_code`: encodes `packet` into `out` (which needs `packet.len() + 1`
/// bytes at most; the viewer gives it twice the packet buffer). Returns the
/// coded length when coding made the packet smaller, with the zero-code flag
/// set in `out[0]`; `None` when it would not, in which case `packet` is sent
/// as it is and `out` holds nothing useful.
///
/// Packets shorter than the packet id are copied through the id loop as the
/// C++ did (its count went negative and the main loop then ran over the
/// buffer); here they are treated as uncodable and return `None`.
pub fn zero_code(packet: &[u8], out: &mut [u8]) -> Option<usize> {
    if packet.len() < PACKET_ID_SIZE || out.len() < packet.len() + 1 {
        return None;
    }
    out[..PACKET_ID_SIZE].copy_from_slice(&packet[..PACKET_ID_SIZE]);
    let mut o = PACKET_ID_SIZE;
    let mut net_gain: i32 = 0;
    let mut num_zeroes: u8 = 0;
    for &b in &packet[PACKET_ID_SIZE..] {
        if b == 0 {
            if num_zeroes != 0 {
                num_zeroes += 1;
                if num_zeroes > 254 {
                    out[o] = num_zeroes;
                    o += 1;
                    num_zeroes = 0;
                }
                net_gain -= 1;
            } else {
                out[o] = 0;
                o += 1;
                net_gain += 1;
                num_zeroes = 1;
            }
        } else {
            if num_zeroes != 0 {
                out[o] = num_zeroes;
                o += 1;
                num_zeroes = 0;
            }
            out[o] = b;
            o += 1;
        }
    }
    if num_zeroes != 0 {
        out[o] = num_zeroes;
        o += 1;
    }
    if net_gain < 0 {
        out[0] |= ZERO_CODE_FLAG;
        debug_assert_eq!(o as i32, packet.len() as i32 + net_gain);
        Some(o)
    } else {
        None
    }
}

/// Why a zero-coded packet could not be expanded into the receive buffer
/// (`MX_WROTE_PAST_BUFFER_SIZE`); the number is the C++ log message's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpandError {
    /// A literal byte would land past the end of the buffer.
    PastEnd1,
    /// A 256-zero wrap would land past the buffer's last 256 bytes.
    PastEnd2,
}

/// The result of [`zero_code_expand`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expanded {
    /// Bytes now in `out` (0 after an error).
    pub len: usize,
    /// The overruns the C++ reported through `callExceptionFunc` while
    /// still producing output: case 3, where a zero run would pass the end
    /// of the buffer and the C++ reset its write position to the start of
    /// the buffer and carried on. The output is then what it wrote.
    pub overruns: u32,
    /// The overrun that stopped decoding, when one did; the C++ then
    /// reported it too and handed on an empty packet.
    pub error: Option<ExpandError>,
}

/// `LLMessageSystem::zeroCodeExpand` for a packet whose flags byte has the
/// zero-code flag set: expands `packet` into `out` (the viewer's
/// `MAX_BUFFER_SIZE` receive buffer; `out.len()` plays that constant's role
/// in the bounds checks) and clears the flag in the output. The C++ loop,
/// including its reactions to packets that expand past the buffer, is kept
/// so that malformed input yields the same bytes.
///
/// A packet whose flag is not set is not for this function: the caller
/// passes it on unchanged.
pub fn zero_code_expand(packet: &[u8], out: &mut [u8]) -> Expanded {
    let mut overruns = 0u32;
    match expand(packet, out, &mut overruns) {
        Ok(len) => Expanded {
            len,
            overruns,
            error: None,
        },
        Err(e) => Expanded {
            len: 0,
            overruns,
            error: Some(e),
        },
    }
}

fn expand(packet: &[u8], out: &mut [u8], overruns: &mut u32) -> Result<usize, ExpandError> {
    let max = out.len();
    let n = packet.len();
    let mut count: i64 = n as i64;
    let mut i = 0usize;
    let mut o = 0usize;
    // the packet id field
    let id = PACKET_ID_SIZE.min(n).min(max);
    while i < id {
        out[o] = packet[i];
        o += 1;
        i += 1;
        count -= 1;
    }
    if o > 0 {
        out[0] &= !ZERO_CODE_FLAG;
    }
    if i < PACKET_ID_SIZE {
        // shorter than the packet id: the C++ copied `data_size` bytes and
        // then ran its loop with a negative count over the whole buffer;
        // what it produced is not meaningful, and the caller never sends
        // such a packet here (it is rejected as too short beforehand)
        return Ok(o);
    }
    // The C++ `while (count--)` tests the old value and decrements before
    // the body, so count is -1 after the last byte.
    loop {
        if count <= 0 {
            break;
        }
        // a run of literal bytes, copied up to the next zero (the C++ copied
        // them one by one through the same count); case 1 when it would
        // pass the end of the buffer
        let remaining = (count as usize).min(n - i);
        let room = remaining.min(max - o);
        let src = &packet[i..i + room];
        let dst = &mut out[o..o + room];
        let mut k = 0;
        while k < room && src[k] != 0 {
            dst[k] = src[k];
            k += 1;
        }
        o += k;
        i += k;
        count -= k as i64;
        if count <= 0 {
            break;
        }
        if k == room && i < n && packet[i] != 0 {
            // out of room in the middle of a literal run
            return Err(ExpandError::PastEnd1);
        }
        if o > max - 1 || i >= n {
            // case 1 (o past the end), or input exhausted on a malformed
            // count: the C++ read past its buffer here; treated as the
            // same overrun
            return Err(ExpandError::PastEnd1);
        }
        // the zero byte of a run
        count -= 1;
        out[o] = 0;
        o += 1;
        i += 1;
        {
            // `while ((count--) && (!(*inptr)))`
            loop {
                let c = count;
                count -= 1;
                if c == 0 {
                    break;
                }
                if i >= n {
                    return Err(ExpandError::PastEnd1);
                }
                if packet[i] != 0 {
                    break;
                }
                // a 0 count byte: 256 zeros (the copied zero plus 255)
                if o >= max {
                    return Err(ExpandError::PastEnd2);
                }
                out[o] = packet[i];
                o += 1;
                i += 1;
                if o > max - 256 {
                    return Err(ExpandError::PastEnd2);
                }
                out[o..o + 255].fill(0);
                o += 255;
            }
            if count < 0 {
                break;
            }
            // the count byte
            let run = packet[i] as usize;
            if o > max - run {
                // case 3: reported, write position reset, decoding continues
                *overruns += 1;
                o = 0;
            }
            let zeros = run.saturating_sub(1);
            if o + zeros > max {
                return Err(ExpandError::PastEnd2);
            }
            out[o..o + zeros].fill(0);
            o += zeros;
            i += 1;
        }
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MAX_BUFFER_SIZE;

    fn roundtrip(data: &[u8]) {
        let mut coded = vec![0u8; data.len() * 2 + 2];
        let gain = zero_code_gain(data);
        match zero_code(data, &mut coded) {
            Some(len) => {
                assert_eq!(len as i32, data.len() as i32 + gain);
                assert!(coded[0] & ZERO_CODE_FLAG != 0);
                let mut out = vec![0u8; MAX_BUFFER_SIZE];
                let e = zero_code_expand(&coded[..len], &mut out);
                assert_eq!((e.overruns, e.error), (0, None));
                assert_eq!(&out[..e.len], data);
            }
            None => assert_eq!(gain, 0),
        }
    }

    #[test]
    fn runs() {
        let mut p = vec![1u8, 0, 0, 0, 0, 0, 7];
        roundtrip(&p);
        p.extend_from_slice(&[0; 3]);
        roundtrip(&p);
        p.extend_from_slice(&[9]);
        p.extend_from_slice(&[0; 255]);
        p.push(1);
        roundtrip(&p);
        p.extend_from_slice(&[0; 600]);
        roundtrip(&p);
        p.extend_from_slice(&[0; 1]);
        roundtrip(&p);
        let mut q = vec![0u8; 6];
        q.extend_from_slice(&[0; 300]);
        roundtrip(&q);
    }

    #[test]
    fn not_worth_it() {
        let p = [1u8, 2, 3, 4, 5, 6, 0, 7, 0, 8];
        let mut coded = vec![0u8; 32];
        assert_eq!(zero_code(&p, &mut coded), None);
        assert_eq!(zero_code_gain(&p), 0);
    }

    #[test]
    fn wrap_form() {
        // 0 0 3: one zero, a 256-zero wrap, then two more zeros
        let coded = [0x80u8, 0, 0, 0, 0, 0, 0, 0, 3, 5];
        let mut out = vec![0u8; MAX_BUFFER_SIZE];
        let e = zero_code_expand(&coded, &mut out);
        assert_eq!(e.error, None);
        assert_eq!(e.len, 6 + 1 + 256 + 2 + 1);
        assert_eq!(out[e.len - 1], 5);
        assert!(out[6..e.len - 1].iter().all(|&b| b == 0));
    }

    #[test]
    fn overruns() {
        let mut coded = vec![0x80u8, 0, 0, 0, 0, 0];
        for _ in 0..40 {
            coded.extend_from_slice(&[0, 255]);
        }
        let mut out = vec![0u8; MAX_BUFFER_SIZE];
        let e = zero_code_expand(&coded, &mut out);
        assert!(e.overruns > 0 && e.error.is_none());
        let mut coded = vec![0x80u8, 0, 0, 0, 0, 0];
        coded.extend(std::iter::repeat_n(0u8, 40));
        let e = zero_code_expand(&coded, &mut out);
        assert_eq!((e.len, e.error), (0, Some(ExpandError::PastEnd2)));
    }
}
