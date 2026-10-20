//! The notation parser: `LLSDNotationParser::doParse` and its helpers,
//! transcribed with an explicit container stack instead of recursion.
//!
//! The grammar, from the viewer:
//!
//! ```text
//! map:     { string:object, string:object }
//! array:   [ object, object, object ]
//! undef:   !
//! boolean: true | false | 1 | 0 | T | F | t | f | TRUE | FALSE
//! integer: i####
//! real:    r####
//! uuid:    u####
//! string:  "g'day" | 'have a "nice" day' | s(size)"raw data"
//! uri:     l"escaped"
//! date:    d"YYYY-MM-DDTHH:MM:SS.FFZ"
//! binary:  b##"ff3120ab1" | b(size)"raw data"
//! ```
//!
//! Every quirk of the C++ parser that affects results is kept: junk before a
//! map key is skipped, a key without a value is dropped, a lone `t`/`f` is a
//! boolean, numbers stop at the first non-numeric byte, a UUID is 36 bytes
//! read with whitespace skipped, `b64""` fails (an `istream::get` with no
//! characters sets failbit) and the size limit is applied where the C++
//! applies it, with the bytes the C++ never counted left uncounted.

use crate::reader::Reader;
use crate::sink::Sink;
use crate::text::{self, is_alpha, is_space};
use crate::{Limits, PARSE_FAILURE};

enum Outcome {
    Scalar,
    Zero,
    OpenMap,
    OpenArray,
}

enum Step {
    Need(i32),
    Done(i32),
}

enum Frame {
    Map {
        depth: i32,
        count: i32,
        found_name: bool,
        name: Vec<u8>,
    },
    Array {
        depth: i32,
        count: i32,
    },
}

struct Parser<'r, 'a, S: Sink> {
    r: &'r mut Reader<'a>,
    sink: &'r mut S,
    max_bytes: Option<i64>,
    /// Bytes consumed by `istream >>` extractions, which the C++ parser never
    /// subtracted from its byte budget.
    unaccounted: usize,
    scratch: Vec<u8>,
}

type R<T> = Result<T, ()>;

/// `deserialize_string_delim`: reads to the delimiter, decoding `\xHH`,
/// `\a \b \f \n \r \t \v`, and `\c` for any other `c`. Fails at end of input.
/// The binary parser shares it for its quoted strings and keys.
pub(crate) fn string_delim(r: &mut Reader<'_>, delim: u8, out: &mut Vec<u8>) -> R<()> {
    loop {
        // Fast path over the buffered window: copy up to the next delimiter
        // or backslash.
        let w = r.window();
        if !w.is_empty() {
            let n = w
                .iter()
                .position(|&b| b == delim || b == b'\\')
                .unwrap_or(w.len());
            out.extend_from_slice(&w[..n]);
            r.advance(n);
        }
        let c = r.get().ok_or(())?;
        if c == delim {
            return Ok(());
        }
        if c != b'\\' {
            out.push(c);
            continue;
        }
        let e = r.get().ok_or(())?;
        match e {
            b'x' => {
                let hi = r.get().ok_or(())?;
                let lo = r.get().ok_or(())?;
                out.push(text::hex_nybble(hi) << 4 | text::hex_nybble(lo));
            }
            b'a' => out.push(7),
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'v' => out.push(11),
            other => out.push(other),
        }
    }
}

/// Parses one notation value. Returns the number of values parsed, 0 when
/// the input holds nothing but whitespace, or [`PARSE_FAILURE`].
pub fn parse<S: Sink>(r: &mut Reader<'_>, sink: &mut S, limits: Limits) -> i32 {
    let mut p = Parser {
        r,
        sink,
        max_bytes: limits.max_bytes,
        unaccounted: 0,
        scratch: Vec::new(),
    };
    p.run(limits.max_depth).unwrap_or(PARSE_FAILURE)
}

impl<S: Sink> Parser<'_, '_, S> {
    /// `mMaxBytesLeft` as the C++ parser would have it: -1 when unlimited.
    fn bytes_left_cpp(&self) -> i64 {
        match self.max_bytes {
            None => -1,
            Some(m) => m - (self.r.consumed() - self.unaccounted) as i64,
        }
    }

    fn run(&mut self, max_depth: i32) -> R<i32> {
        let mut stack: Vec<Frame> = Vec::new();
        let mut depth = max_depth;
        loop {
            let mut result = match self.value(depth)? {
                Outcome::Scalar => 1,
                Outcome::Zero => 0,
                Outcome::OpenMap => {
                    stack.push(Frame::Map {
                        depth: depth - 1,
                        count: 0,
                        found_name: false,
                        name: Vec::new(),
                    });
                    -2
                }
                Outcome::OpenArray => {
                    stack.push(Frame::Array {
                        depth: depth - 1,
                        count: 0,
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
                        count,
                        found_name,
                        name,
                    } => {
                        if result != -2 {
                            if result > 0 {
                                *count += result;
                            } else {
                                return Err(());
                            }
                            *found_name = false;
                        }
                        let (d, c) = (*depth, *count);
                        Self::map_loop(
                            self.r,
                            self.sink,
                            &mut self.scratch,
                            self.max_bytes,
                            &mut self.unaccounted,
                            d,
                            c,
                            found_name,
                            name,
                        )?
                    }
                    Frame::Array { depth, count } => {
                        if result != -2 {
                            *count += result;
                        }
                        Self::array_loop(self.r, self.sink, *depth, *count)?
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
        unaccounted: &mut usize,
        depth: i32,
        count: i32,
        found_name: &mut bool,
        name: &mut Vec<u8>,
    ) -> R<Step> {
        let mut c = r.get();
        loop {
            let Some(b) = c else { return Err(()) };
            if b == b'}' {
                break;
            }
            if !*found_name {
                if b == b'"' || b == b'\'' || b == b's' {
                    r.unget(b);
                    *found_name = true;
                    name.clear();
                    let left = match max_bytes {
                        None => -1,
                        Some(m) => m - (r.consumed() - *unaccounted) as i64,
                    };
                    Self::deserialize_string(r, scratch, left, name)?;
                }
                c = r.get();
            } else {
                if is_space(b) || b == b':' {
                    c = r.get();
                    continue;
                }
                r.unget(b);
                sink.key(name);
                return Ok(Step::Need(depth));
            }
        }
        sink.end_map();
        Ok(Step::Done(count))
    }

    fn array_loop(r: &mut Reader<'_>, sink: &mut S, depth: i32, count: i32) -> R<Step> {
        let mut c = r.get();
        loop {
            let Some(b) = c else { return Err(()) };
            if b == b']' {
                break;
            }
            if is_space(b) || b == b',' {
                c = r.get();
                continue;
            }
            r.unget(b);
            return Ok(Step::Need(depth));
        }
        sink.end_array();
        Ok(Step::Done(count))
    }

    /// One value at the current position, without its children.
    fn value(&mut self, depth: i32) -> R<Outcome> {
        if depth == 0 {
            return Err(());
        }
        while let Some(b) = self.r.peek() {
            if !is_space(b) {
                break;
            }
            self.r.get();
        }
        let Some(c) = self.r.peek() else {
            return Ok(Outcome::Zero);
        };
        match c {
            b'{' => {
                self.r.get();
                self.sink.begin_map();
                return Ok(Outcome::OpenMap);
            }
            b'[' => {
                self.r.get();
                self.sink.begin_array();
                return Ok(Outcome::OpenArray);
            }
            b'!' => {
                self.r.get();
                self.sink.undef();
            }
            b'0' => {
                self.r.get();
                self.sink.boolean(false);
            }
            b'1' => {
                self.r.get();
                self.sink.boolean(true);
            }
            b'F' | b'f' => {
                self.r.get();
                if matches!(self.r.peek(), Some(n) if is_alpha(n)) {
                    self.boolean_word(b"false")?;
                }
                self.sink.boolean(false);
            }
            b'T' | b't' => {
                self.r.get();
                if matches!(self.r.peek(), Some(n) if is_alpha(n)) {
                    self.boolean_word(b"true")?;
                }
                self.sink.boolean(true);
            }
            b'i' => {
                self.r.get();
                let before = self.r.consumed();
                let v = text::extract_int(self.r);
                self.unaccounted += self.r.consumed() - before;
                self.sink.integer(v.ok_or(())?);
            }
            b'r' => {
                self.r.get();
                let before = self.r.consumed();
                let v = text::extract_real(self.r);
                self.unaccounted += self.r.consumed() - before;
                self.sink.real(v.ok_or(())?);
            }
            b'u' => {
                self.r.get();
                let before = self.r.consumed();
                let mut s = [0u8; 36];
                for slot in s.iter_mut() {
                    loop {
                        match self.r.get() {
                            None => return Err(()),
                            Some(b) if is_space(b) => continue,
                            Some(b) => {
                                *slot = b;
                                break;
                            }
                        }
                    }
                }
                self.unaccounted += self.r.consumed() - before;
                self.sink.uuid(&text::uuid_set(&s));
            }
            b'"' | b'\'' | b's' => {
                let left = self.bytes_left_cpp();
                let mut value = std::mem::take(&mut self.scratch);
                value.clear();
                let mut tmp = Vec::new();
                let res = Self::deserialize_string(self.r, &mut tmp, left, &mut value);
                if res.is_ok() {
                    self.sink.string(&value);
                }
                self.scratch = value;
                res?;
            }
            b'l' => {
                self.r.get();
                let delim = self.r.get().ok_or(())?;
                let mut value = std::mem::take(&mut self.scratch);
                value.clear();
                let res = Self::string_delim(self.r, delim, &mut value);
                if res.is_ok() {
                    self.sink.uri(&value);
                }
                self.scratch = value;
                res?;
            }
            b'd' => {
                self.r.get();
                let delim = self.r.get().ok_or(())?;
                let mut value = std::mem::take(&mut self.scratch);
                value.clear();
                let res = Self::string_delim(self.r, delim, &mut value);
                if res.is_ok() {
                    self.sink.date(text::date_parse(&value).unwrap_or(0.0));
                }
                self.scratch = value;
                res?;
            }
            b'b' => self.binary()?,
            _ => return Err(()),
        }
        Ok(Outcome::Scalar)
    }

    /// `deserialize_boolean`: the leading letter is consumed; the rest of
    /// `word` must follow, case-insensitively.
    fn boolean_word(&mut self, word: &[u8]) -> R<()> {
        let mut ii = 0;
        let mut c = self.r.peek();
        loop {
            ii += 1;
            if ii >= word.len() {
                break;
            }
            match c {
                Some(b) if b.to_ascii_lowercase() == word[ii] => {
                    self.r.get();
                    c = self.r.peek();
                }
                _ => break,
            }
        }
        if ii != word.len() { Err(()) } else { Ok(()) }
    }

    /// `deserialize_string`: a delimited or raw (`s(len)"..."`) string. `left`
    /// is the byte budget as the C++ parser saw it on entry.
    fn deserialize_string(
        r: &mut Reader<'_>,
        scratch: &mut Vec<u8>,
        left: i64,
        out: &mut Vec<u8>,
    ) -> R<()> {
        let c = r.get().ok_or(())?;
        match c {
            b'"' | b'\'' => Self::string_delim(r, c, out),
            b's' => Self::string_raw(r, scratch, left, out),
            _ => Err(()),
        }
    }

    fn string_delim(r: &mut Reader<'_>, delim: u8, out: &mut Vec<u8>) -> R<()> {
        string_delim(r, delim, out)
    }

    /// `deserialize_string_raw`: `(len)"bytes"` after the `s`.
    fn string_raw(
        r: &mut Reader<'_>,
        scratch: &mut Vec<u8>,
        left: i64,
        out: &mut Vec<u8>,
    ) -> R<()> {
        scratch.clear();
        r.read_until(b')', 18, scratch);
        if scratch.is_empty() {
            return Err(());
        }
        let _close_paren = r.get();
        let quote = r.get().ok_or(())?;
        if !((quote == b'"' || quote == b'\'') && scratch[0] == b'(') {
            return Err(());
        }
        let len = text::strtol0(&scratch[1..]);
        if len < 0 {
            return Err(());
        }
        if left > 0 && len > left {
            return Err(());
        }
        let len = len as usize;
        if len > 0 && r.read_exact(len, out) < len {
            return Err(());
        }
        let end = r.get().ok_or(())?;
        if end == b'"' || end == b'\'' {
            Ok(())
        } else {
            Err(())
        }
    }

    /// `parseBinary`: `b(len)"raw"`, `b64"..."` or `b16"..."`.
    fn binary(&mut self) -> R<()> {
        let mut head = std::mem::take(&mut self.scratch);
        head.clear();
        self.r.read_until(b'"', 254, &mut head);
        let res = self.binary_body(&head);
        self.scratch = head;
        res
    }

    fn binary_body(&mut self, head: &[u8]) -> R<()> {
        if head.is_empty() {
            return Err(());
        }
        if self.r.get() != Some(b'"') {
            return Err(());
        }
        let mut data = Vec::new();
        if head.starts_with(b"b(") {
            let len = text::strtol0(&head[2..]);
            if len < 0 {
                return Err(());
            }
            if self.max_bytes.is_some() && len > self.bytes_left_cpp() {
                return Err(());
            }
            let len = len as usize;
            if len > 0 && self.r.read_exact(len, &mut data) < len {
                return Err(());
            }
            // The trailing quote is consumed but not checked; end of input fails.
            self.r.get().ok_or(())?;
        } else if head.starts_with(b"b64") {
            let mut coded = Vec::new();
            self.r.read_until(b'"', usize::MAX, &mut coded);
            if coded.is_empty() {
                return Err(());
            }
            self.r.get().ok_or(())?;
            text::base64_decode_apr(&coded, &mut data);
        } else if head.starts_with(b"b16") {
            let mut hex = Vec::new();
            let mut c = self.r.get().ok_or(())?;
            while c != b'"' {
                self.r.unget(c);
                hex.clear();
                self.r.read_until(b'"', 254, &mut hex);
                c = self.r.get().ok_or(())?;
                for pair in hex.chunks(2) {
                    let hi = text::hex_nybble(pair[0]);
                    let lo = pair.get(1).map_or(0, |&b| text::hex_nybble(b));
                    data.push(hi << 4 | lo);
                }
            }
        } else {
            return Err(());
        }
        self.sink.binary(&data);
        Ok(())
    }
}
