//! The binary parser: `LLSDBinaryParser::doParse`, transcribed with an
//! explicit container stack.
//!
//! ```text
//! Undefined: '!'
//! Boolean:   '1' true, '0' false
//! Integer:   'i' + 4 bytes network order
//! Real:      'r' + 8 bytes IEEE double, network order
//! UUID:      'u' + 16 bytes
//! String:    's' + 4 byte size + bytes, or a notation-style quoted string
//! Date:      'd' + 8 byte IEEE double, host order (as the C++ writes it)
//! URI:       'l' + 4 byte size + bytes
//! Binary:    'b' + 4 byte size + bytes
//! Array:     '[' + 4 byte count + values + ']'
//! Map:       '{' + 4 byte count + (key + value)* + '}'
//!            keys are 'k' + 4 byte size + bytes, or quoted
//! ```
//!
//! Kept quirks: a short integer, real or UUID is zero-padded and accepted
//! (the C++ only logs); a map key with an unknown tag byte is the empty key;
//! a negative binary size is an empty blob; counts are checked against the
//! declared sizes exactly as the C++ loops do.

use crate::reader::Reader;
use crate::sink::Sink;
use crate::{Limits, PARSE_FAILURE};

enum Outcome {
    Scalar,
    Zero,
    OpenMap(i32),
    OpenArray(i32),
}

enum Step {
    Need(i32),
    Done(i32),
}

enum Frame {
    Map {
        depth: i32,
        size: i32,
        count: i32,
        parsed: i32,
    },
    Array {
        depth: i32,
        size: i32,
        count: i32,
        parsed: i32,
    },
}

type R<T> = Result<T, ()>;

struct Parser<'r, 'a, S: Sink> {
    r: &'r mut Reader<'a>,
    sink: &'r mut S,
    max_bytes: Option<i64>,
    scratch: Vec<u8>,
}

/// Parses one binary value. Returns the number of values parsed, 0 at end of
/// input, or [`PARSE_FAILURE`].
pub fn parse<S: Sink>(r: &mut Reader<'_>, sink: &mut S, limits: Limits) -> i32 {
    let mut p = Parser {
        r,
        sink,
        max_bytes: limits.max_bytes,
        scratch: Vec::new(),
    };
    p.run(limits.max_depth).unwrap_or(PARSE_FAILURE)
}

fn read_padded<const N: usize>(r: &mut Reader<'_>) -> ([u8; N], bool) {
    let mut b = [0u8; N];
    let mut tmp = Vec::with_capacity(N);
    let n = r.read_exact(N, &mut tmp);
    b[..n].copy_from_slice(&tmp);
    (b, n == N)
}

fn read_i32(r: &mut Reader<'_>) -> R<i32> {
    let (b, full) = read_padded::<4>(r);
    if full {
        Ok(i32::from_be_bytes(b))
    } else {
        Err(())
    }
}

impl<S: Sink> Parser<'_, '_, S> {
    fn bytes_left(&self) -> Option<i64> {
        self.max_bytes.map(|m| m - self.r.consumed() as i64)
    }

    fn run(&mut self, max_depth: i32) -> R<i32> {
        let mut stack: Vec<Frame> = Vec::new();
        let mut depth = max_depth;
        loop {
            let mut result = match self.value(depth)? {
                Outcome::Scalar => 1,
                Outcome::Zero => 0,
                Outcome::OpenMap(size) => {
                    stack.push(Frame::Map {
                        depth: depth - 1,
                        size,
                        count: 0,
                        parsed: 0,
                    });
                    -2
                }
                Outcome::OpenArray(size) => {
                    stack.push(Frame::Array {
                        depth: depth - 1,
                        size,
                        count: 0,
                        parsed: 0,
                    });
                    -2
                }
            };
            loop {
                let Some(top) = stack.last_mut() else {
                    return Ok(result);
                };
                let step = match top {
                    Frame::Map {
                        depth,
                        size,
                        count,
                        parsed,
                    } => {
                        if result != -2 {
                            if result > 0 {
                                *parsed += result;
                            } else {
                                return Err(());
                            }
                        }
                        Self::map_loop(
                            self.r,
                            self.sink,
                            &mut self.scratch,
                            self.max_bytes,
                            *depth,
                            *size,
                            count,
                            *parsed,
                        )?
                    }
                    Frame::Array {
                        depth,
                        size,
                        count,
                        parsed,
                    } => {
                        if result != -2 {
                            *parsed += result;
                        }
                        Self::array_loop(self.r, self.sink, *depth, *size, count, *parsed)?
                    }
                };
                match step {
                    Step::Need(d) => {
                        depth = d;
                        break;
                    }
                    Step::Done(children) => {
                        stack.pop();
                        result = 1 + children;
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn map_loop(
        r: &mut Reader<'_>,
        sink: &mut S,
        scratch: &mut Vec<u8>,
        max_bytes: Option<i64>,
        depth: i32,
        size: i32,
        count: &mut i32,
        parsed: i32,
    ) -> R<Step> {
        let Some(c) = r.get() else { return Err(()) };
        if c != b'}' {
            if *count >= size {
                return Err(());
            }
            scratch.clear();
            match c {
                b'k' => Self::parse_string(r, max_bytes, scratch)?,
                b'\'' | b'"' => crate::notation::string_delim(r, c, scratch)?,
                _ => {}
            }
            sink.key(scratch);
            *count += 1;
            return Ok(Step::Need(depth));
        }
        if *count < size {
            return Err(());
        }
        sink.end_map();
        Ok(Step::Done(parsed))
    }

    fn array_loop(
        r: &mut Reader<'_>,
        sink: &mut S,
        depth: i32,
        size: i32,
        count: &mut i32,
        parsed: i32,
    ) -> R<Step> {
        let c = r.peek();
        if c.is_some() && c != Some(b']') && *count < size {
            *count += 1;
            return Ok(Step::Need(depth));
        }
        if r.get() != Some(b']') || *count < size {
            return Err(());
        }
        sink.end_array();
        Ok(Step::Done(parsed))
    }

    /// `parseString`: a 4-byte size and that many bytes.
    fn parse_string(r: &mut Reader<'_>, max_bytes: Option<i64>, out: &mut Vec<u8>) -> R<()> {
        let size = read_i32(r)?;
        if let Some(m) = max_bytes {
            let left = m - r.consumed() as i64;
            if size as i64 > left {
                return Err(());
            }
        }
        if size < 0 {
            return Err(());
        }
        let size = size as usize;
        if size > 0 && r.read_exact(size, out) < size {
            return Err(());
        }
        Ok(())
    }

    fn value(&mut self, depth: i32) -> R<Outcome> {
        let Some(c) = self.r.get() else {
            return Ok(Outcome::Zero);
        };
        if depth == 0 {
            return Err(());
        }
        match c {
            b'{' => {
                let size = read_i32(self.r)?;
                self.sink.begin_map();
                return Ok(Outcome::OpenMap(size));
            }
            b'[' => {
                let size = read_i32(self.r)?;
                self.sink.begin_array();
                return Ok(Outcome::OpenArray(size));
            }
            b'!' => self.sink.undef(),
            b'0' => self.sink.boolean(false),
            b'1' => self.sink.boolean(true),
            b'i' => {
                let (b, _) = read_padded::<4>(self.r);
                self.sink.integer(i32::from_be_bytes(b));
            }
            b'r' => {
                let (b, _) = read_padded::<8>(self.r);
                self.sink.real(f64::from_bits(u64::from_be_bytes(b)));
            }
            b'u' => {
                let (b, _) = read_padded::<16>(self.r);
                self.sink.uuid(&b);
            }
            b'\'' | b'"' => {
                let mut s = std::mem::take(&mut self.scratch);
                s.clear();
                let res = crate::notation::string_delim(self.r, c, &mut s);
                if res.is_ok() {
                    self.sink.string(&s);
                }
                self.scratch = s;
                res?;
            }
            b's' | b'l' => {
                let mut s = std::mem::take(&mut self.scratch);
                s.clear();
                let res = Self::parse_string(self.r, self.max_bytes, &mut s);
                if res.is_ok() {
                    if c == b's' {
                        self.sink.string(&s);
                    } else {
                        self.sink.uri(&s);
                    }
                }
                self.scratch = s;
                res?;
            }
            b'd' => {
                let (b, full) = read_padded::<8>(self.r);
                if !full {
                    return Err(());
                }
                self.sink.date(f64::from_ne_bytes(b));
            }
            b'b' => {
                let size = read_i32(self.r)?;
                if let Some(left) = self.bytes_left()
                    && size as i64 > left
                {
                    return Err(());
                }
                let mut data = Vec::new();
                if size > 0 {
                    let size = size as usize;
                    if self.r.read_exact(size, &mut data) < size {
                        return Err(());
                    }
                }
                self.sink.binary(&data);
            }
            _ => return Err(()),
        }
        Ok(Outcome::Scalar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;
    use crate::reader::SliceSource;
    use crate::sink::{KeyPolicy, ValueBuilder};

    fn p(input: &[u8]) -> (i32, Option<Value>) {
        let mut src = SliceSource::new(input);
        let mut r = Reader::new(&mut src);
        let mut b = ValueBuilder::new(KeyPolicy::FirstWins);
        let n = parse(
            &mut r,
            &mut b,
            Limits {
                max_bytes: Some(input.len() as i64),
                max_depth: -1,
            },
        );
        (n, b.finish())
    }

    #[test]
    fn scalars() {
        assert_eq!(p(b"!"), (1, Some(Value::Undefined)));
        assert_eq!(p(b"1"), (1, Some(Value::Boolean(true))));
        assert_eq!(p(b"i\x00\x00\x00\x17"), (1, Some(Value::Integer(23))));
        assert_eq!(
            p(b"s\x00\x00\x00\x06abc321"),
            (1, Some(Value::string("abc321")))
        );
        assert_eq!(p(b"s\x00\x00\x00\x07abc321").0, PARSE_FAILURE);
        assert_eq!(p(b"t").0, PARSE_FAILURE);
        assert_eq!(p(b"").0, 0);
    }

    #[test]
    fn containers() {
        let mut m = Value::map();
        if let Value::Map(mm) = &mut m {
            mm.insert(b"amy".to_vec(), Value::Integer(23));
        }
        assert_eq!(
            p(b"{\x00\x00\x00\x01k\x00\x00\x00\x03amyi\x00\x00\x00\x17}"),
            (2, Some(m))
        );
        assert_eq!(
            p(b"{\x00\x00\x00\x01k\x00\x00\x00\x03amyi\x00\x00\x00\x17").0,
            PARSE_FAILURE
        );
        assert_eq!(
            p(b"{\x00\x00\x00\x00k\x00\x00\x00\x03amyi\x00\x00\x00\x17}").0,
            PARSE_FAILURE
        );
        assert_eq!(
            p(b"{\x00\x00\x00\x02k\x00\x00\x00\x03amyi\x00\x00\x00\x17}").0,
            PARSE_FAILURE
        );
        assert_eq!(p(b"{\x00\x00\x00\x00}"), (1, Some(Value::map())));
        assert_eq!(p(b"[\x00\x00\x00\x00]"), (1, Some(Value::array())));
        assert_eq!(
            p(b"[\x00\x00\x00\x02\"amy\"i\x00\x00\x00\x17]"),
            (
                3,
                Some(Value::Array(vec![Value::string("amy"), Value::Integer(23)]))
            )
        );
        assert_eq!(
            p(b"[\x00\x00\x00\x01\"amy\"i\x00\x00\x00\x17]").0,
            PARSE_FAILURE
        );
        assert_eq!(
            p(b"[\x00\x00\x00\x03\"amy\"i\x00\x00\x00\x17]").0,
            PARSE_FAILURE
        );
    }
}
