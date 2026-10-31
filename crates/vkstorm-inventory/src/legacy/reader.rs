//! The line reader the legacy importers share: `std::istream::getline` into a
//! fixed buffer, with the stream-state bits it leaves (`llinventory.cpp` and others).
//!
//! [`Stream`] is an `std::istringstream` over a byte slice: a get position
//! and the `eofbit`, `failbit` and `badbit` state. The legacy readers take
//! one and leave it where the C++ leaves its stream, so that a caller (the
//! notecard and task-inventory readers) continues from the same byte.

/// An input stream over bytes, as `std::istringstream` behaves for the
/// operations the legacy readers use.
#[derive(Clone, Debug)]
pub struct Stream<'a> {
    data: &'a [u8],
    pos: usize,
    eof: bool,
    fail: bool,
    bad: bool,
}

impl<'a> Stream<'a> {
    /// A good stream at the start of `data`.
    pub fn new(data: &'a [u8]) -> Stream<'a> {
        Stream {
            data,
            pos: 0,
            eof: false,
            fail: false,
            bad: false,
        }
    }

    /// A stream at the start of `data` with the given state bits: a caller's
    /// stream as it is handed to a reader, which may already be past its
    /// end or failed (`fail` here is the `failbit` alone).
    pub fn with_state(data: &'a [u8], eof: bool, fail: bool, bad: bool) -> Stream<'a> {
        Stream { data, pos: 0, eof, fail, bad }
    }

    /// `rdstate()` as its three bits: `eofbit`, `failbit` and `badbit`.
    pub fn state_bits(&self) -> (bool, bool, bool) {
        (self.eof, self.fail, self.bad)
    }

    /// `good()`: no state bit set.
    pub fn good(&self) -> bool {
        !(self.eof || self.fail || self.bad)
    }

    /// `eof()`.
    pub fn eof(&self) -> bool {
        self.eof
    }

    /// `fail()`: `failbit` or `badbit`.
    pub fn fail(&self) -> bool {
        self.fail || self.bad
    }

    /// `bad()`.
    pub fn bad(&self) -> bool {
        self.bad
    }

    /// `setstate(failbit)`.
    pub fn set_fail(&mut self) {
        self.fail = true;
    }

    /// The get position of the stream buffer (`pubseekoff(0, cur, in)`),
    /// which stays meaningful after a failure.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// The whole input, read or not.
    pub fn input(&self) -> &'a [u8] {
        self.data
    }

    /// The input not yet read.
    pub fn rest(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }

    /// The state as the recorder writes it: `good`, or the letters of the
    /// bits set, in the order `e` (eof), `f` (fail), `b` (bad).
    pub fn state_letters(&self) -> String {
        if self.good() {
            return "good".to_owned();
        }
        let mut s = String::with_capacity(3);
        if self.eof {
            s.push('e');
        }
        if self.fail {
            s.push('f');
        }
        if self.bad {
            s.push('b');
        }
        s
    }

    /// `peek()` without changing state: the next byte, if any.
    pub fn peek_byte(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    /// `getline(buffer, size)` with the delimiter `'\n'`: the bytes it
    /// stores, without the terminating NUL it writes after them.
    ///
    /// As the C++ (both the MSVC runtime and libstdc++):
    /// - a stream that is not good stores nothing and sets `failbit`;
    /// - bytes are stored until the delimiter, which is extracted but not
    ///   stored, or the end of input, which sets `eofbit`;
    /// - after `size - 1` stored bytes a following delimiter is still
    ///   extracted and the end of input still sets `eofbit`; any other byte
    ///   is left unread and sets `failbit`;
    /// - extracting nothing at all (not even a delimiter) sets `failbit`.
    ///
    /// The stored bytes may contain NUL; a reader that `sscanf`s the buffer
    /// sees it as a C string ending at the first one.
    pub fn getline(&mut self, size: usize) -> &'a [u8] {
        let start = self.pos;
        if !self.good() {
            self.fail = true;
            return &self.data[start..start];
        }
        let limit = size.saturating_sub(1);
        let mut extracted = 0usize;
        let end;
        loop {
            match self.data.get(self.pos) {
                None => {
                    self.eof = true;
                    end = self.pos;
                    break;
                }
                Some(b'\n') => {
                    end = self.pos;
                    self.pos += 1;
                    extracted += 1;
                    break;
                }
                Some(_) if self.pos - start >= limit => {
                    self.fail = true;
                    end = self.pos;
                    break;
                }
                Some(_) => {
                    self.pos += 1;
                    extracted += 1;
                }
            }
        }
        if extracted == 0 {
            self.fail = true;
        }
        &self.data[start..end]
    }
}

impl<'a> Stream<'a> {
    /// `clear()`: resets every state bit.
    pub fn clear(&mut self) {
        self.eof = false;
        self.fail = false;
        self.bad = false;
    }

    /// The `sentry` of the unformatted and `noskipws` operations: a stream
    /// that is not good sets `failbit` and does nothing.
    fn sentry(&mut self) -> bool {
        if self.good() {
            true
        } else {
            self.fail = true;
            false
        }
    }

    /// `get(c)`: the next byte. A stream that is not good sets `failbit`;
    /// the end of input sets `eofbit` and `failbit`. Either way nothing is
    /// extracted and `None` is returned (`c` keeps its value).
    pub fn get(&mut self) -> Option<u8> {
        if !self.sentry() {
            return None;
        }
        match self.data.get(self.pos) {
            Some(&b) => {
                self.pos += 1;
                Some(b)
            }
            None => {
                self.eof = true;
                self.fail = true;
                None
            }
        }
    }

    /// `>> std::ws`: skips white space (C `isspace`). A stream that is not
    /// good sets `failbit`; reaching the end of input sets `eofbit` only.
    pub fn skip_ws(&mut self) {
        if !self.sentry() {
            return;
        }
        while let Some(&b) = self.data.get(self.pos) {
            if !super::scanf::is_space(b) {
                return;
            }
            self.pos += 1;
        }
        self.eof = true;
    }

    /// `operator>>(std::istream&, const char*)` (`llstreamtools.cpp`): reads
    /// the literal byte by byte with `get(c)`, and sets `failbit` at the
    /// first byte that differs (that byte is extracted) or that cannot be
    /// read.
    ///
    /// The C++ compares a `c` that a failed `get` left unchanged and stops
    /// only on `badbit`; the stream has `failbit` set from then on and no
    /// later `get` extracts anything, so the state and position are the
    /// ones returned here.
    pub fn expect(&mut self, literal: &[u8]) {
        for &want in literal {
            if self.bad {
                return;
            }
            match self.get() {
                Some(c) if c == want => {}
                _ => {
                    self.fail = true;
                    return;
                }
            }
        }
    }

    /// The digits of a formatted integer extraction (`>> S32`, `>> U32`,
    /// the MSVC runtime's `num_get`): skips white space (`eofbit` and
    /// `failbit` when the input ends there), then an optional sign and
    /// decimal digits; reaching the end sets `eofbit`. Returns the sign and
    /// the magnitude (saturated), or `None` with `failbit` when nothing was
    /// converted. A stream that is not good sets `failbit` and returns
    /// `None` without touching the target.
    fn extract_number(&mut self) -> Result<(bool, u64, bool), bool> {
        if !self.good() {
            self.fail = true;
            return Err(false);
        }
        while let Some(&b) = self.data.get(self.pos) {
            if !super::scanf::is_space(b) {
                break;
            }
            self.pos += 1;
        }
        if self.pos >= self.data.len() {
            self.eof = true;
            self.fail = true;
            return Err(false);
        }
        let mut negative = false;
        if let Some(&s @ (b'+' | b'-')) = self.data.get(self.pos) {
            negative = s == b'-';
            self.pos += 1;
        }
        let mut digits = 0usize;
        let mut acc: u64 = 0;
        let mut overflow = false;
        while let Some(&b) = self.data.get(self.pos) {
            if !b.is_ascii_digit() {
                break;
            }
            match acc
                .checked_mul(10)
                .and_then(|v| v.checked_add(u64::from(b - b'0')))
            {
                Some(v) if v <= u64::from(u32::MAX) + 1 => acc = v,
                _ => overflow = true,
            }
            digits += 1;
            self.pos += 1;
        }
        if self.pos >= self.data.len() {
            self.eof = true;
        }
        if digits == 0 {
            self.fail = true;
            return Err(true);
        }
        Ok((negative, acc, overflow))
    }

    /// `>> S32`: the value stored into `v`. A conversion that fails stores
    /// 0, an out-of-range value the nearest limit, both with `failbit`; a
    /// stream that is not good leaves `v` as it is.
    pub fn extract_i32(&mut self, v: &mut i32) {
        match self.extract_number() {
            Err(true) => *v = 0,
            Err(false) => {}
            Ok((negative, acc, overflow)) => {
                let limit = if negative {
                    1u64 << 31
                } else {
                    (1u64 << 31) - 1
                };
                if overflow || acc > limit {
                    self.fail = true;
                    *v = if negative { i32::MIN } else { i32::MAX };
                } else if negative {
                    *v = (acc as i64).wrapping_neg() as i32;
                } else {
                    *v = acc as i32;
                }
            }
        }
    }

    /// `>> U32`: as [`extract_i32`](Self::extract_i32), with a negative
    /// value negated modulo 2^32 (`strtoul`) and `U32_MAX` on overflow.
    pub fn extract_u32(&mut self, v: &mut u32) {
        match self.extract_number() {
            Err(true) => *v = 0,
            Err(false) => {}
            Ok((negative, acc, overflow)) => {
                if overflow || acc > u64::from(u32::MAX) {
                    self.fail = true;
                    *v = u32::MAX;
                } else if negative {
                    *v = (acc as u32).wrapping_neg();
                } else {
                    *v = acc as u32;
                }
            }
        }
    }

    /// `read(buf, n)`: up to `n` bytes. A stream that is not good sets
    /// `failbit` and reads nothing; fewer than `n` bytes set `eofbit` and
    /// `failbit`. Returns the bytes read (`gcount()` of them).
    pub fn read(&mut self, n: usize) -> &'a [u8] {
        let start = self.pos;
        if !self.sentry() {
            return &self.data[start..start];
        }
        let end = start.saturating_add(n).min(self.data.len());
        self.pos = end;
        if end - start < n {
            self.eof = true;
            self.fail = true;
        }
        &self.data[start..end]
    }
}

/// The C string of a buffer: its bytes up to the first NUL.
pub fn c_str(buf: &[u8]) -> &[u8] {
    match buf.iter().position(|&b| b == 0) {
        Some(n) => &buf[..n],
        None => buf,
    }
}
