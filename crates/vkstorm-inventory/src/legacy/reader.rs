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
        Stream { data, pos: 0, eof: false, fail: false, bad: false }
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

/// The C string of a buffer: its bytes up to the first NUL.
pub fn c_str(buf: &[u8]) -> &[u8] {
    match buf.iter().position(|&b| b == 0) {
        Some(n) => &buf[..n],
        None => buf,
    }
}
