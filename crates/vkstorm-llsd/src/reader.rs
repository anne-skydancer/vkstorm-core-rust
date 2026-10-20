//! Byte input for the parsers.
//!
//! A [`Source`] supplies bytes; the [`Reader`] buffers them and offers the
//! `peek`/`get`/`unget`/`read_exact` operations the viewer's parsers were
//! written against (`std::istream::peek()`, `get()`, `putback()`, `read()`),
//! plus bulk access to the buffered window for the scanners.
//!
//! A source may be *exact*: then the reader never pulls more than the parser
//! asks for, so an underlying stream is left positioned right after the parsed
//! object, as the C++ parsers left it. Otherwise the reader pulls chunks and
//! reports how many buffered bytes went unconsumed, for the caller to seek back.

/// Supplies bytes to a [`Reader`].
pub trait Source {
    /// Reads up to `buf.len()` bytes; 0 means end of input.
    fn read(&mut self, buf: &mut [u8]) -> usize;

    /// Returns the next byte without consuming it. Only called on exact
    /// sources; the default is "end of input".
    fn peek(&mut self) -> Option<u8> {
        None
    }

    /// True when reads must never run ahead of what the parser needs.
    fn exact(&self) -> bool {
        false
    }
}

/// A source over a byte slice.
pub struct SliceSource<'a> {
    data: &'a [u8],
}

impl<'a> SliceSource<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        SliceSource { data }
    }
}

impl Source for SliceSource<'_> {
    fn read(&mut self, buf: &mut [u8]) -> usize {
        let n = buf.len().min(self.data.len());
        buf[..n].copy_from_slice(&self.data[..n]);
        self.data = &self.data[n..];
        n
    }
}

const INITIAL_BUFFER: usize = 16 * 1024;
const READ_CHUNK: usize = 64 * 1024;

/// Buffered access to a [`Source`] with the position bookkeeping the parsers need.
pub struct Reader<'a> {
    src: &'a mut dyn Source,
    exact: bool,
    buf: Vec<u8>,
    pos: usize,
    end: usize,
    eof: bool,
    /// Bytes handed to the parser so far (prefix bytes included).
    consumed: usize,
}

impl<'a> Reader<'a> {
    pub fn new(src: &'a mut dyn Source) -> Self {
        let exact = src.exact();
        Reader {
            src,
            exact,
            buf: Vec::new(),
            pos: 0,
            end: 0,
            eof: false,
            consumed: 0,
        }
    }

    /// A reader whose first bytes are `prefix`, followed by the source.
    pub fn with_prefix(prefix: &[u8], src: &'a mut dyn Source) -> Self {
        let mut r = Reader::new(src);
        r.buf = prefix.to_vec();
        r.end = prefix.len();
        r
    }

    /// Total bytes consumed so far.
    #[inline]
    pub fn consumed(&self) -> usize {
        self.consumed
    }

    /// Bytes pulled from the source but not consumed by the parser.
    #[inline]
    pub fn unconsumed(&self) -> usize {
        self.end - self.pos
    }

    /// True once the source reported end of input (and the buffer is drained).
    #[inline]
    pub fn at_eof(&self) -> bool {
        self.eof && self.pos >= self.end
    }

    /// The buffered, unconsumed bytes.
    #[inline]
    pub fn window(&self) -> &[u8] {
        &self.buf[self.pos..self.end]
    }

    /// Consumes `n` buffered bytes (`n <= window().len()`).
    #[inline]
    pub fn advance(&mut self, n: usize) {
        debug_assert!(n <= self.end - self.pos);
        self.pos += n;
        self.consumed += n;
    }

    /// Makes room and pulls more bytes. Returns the number pulled (0 at EOF).
    fn refill(&mut self, want: usize) -> usize {
        if self.eof {
            return 0;
        }
        if self.pos == self.end {
            self.pos = 0;
            self.end = 0;
        } else if self.pos > 0 && self.end + want > self.buf.len() {
            self.buf.copy_within(self.pos..self.end, 0);
            self.end -= self.pos;
            self.pos = 0;
        }
        let need = self.end
            + want.max(if self.exact {
                1
            } else {
                READ_CHUNK.min(INITIAL_BUFFER.max(want))
            });
        if self.buf.len() < need {
            let grow = if self.buf.is_empty() {
                INITIAL_BUFFER.max(need)
            } else {
                need.max(self.buf.len() * 2)
            };
            self.buf.resize(grow, 0);
        }
        let cap = if self.exact {
            want
        } else {
            self.buf.len() - self.end
        };
        let n = self.src.read(&mut self.buf[self.end..self.end + cap]);
        if n == 0 {
            self.eof = true;
        }
        self.end += n;
        n
    }

    /// Ensures at least `n` bytes are buffered unless the input ends first.
    /// Returns true when `n` bytes are available.
    pub fn ensure(&mut self, n: usize) -> bool {
        while self.end - self.pos < n {
            if self.refill(n - (self.end - self.pos)) == 0 {
                return false;
            }
        }
        true
    }

    /// The next byte, not consumed.
    #[inline]
    pub fn peek(&mut self) -> Option<u8> {
        if self.pos < self.end {
            Some(self.buf[self.pos])
        } else {
            self.peek_slow()
        }
    }

    #[cold]
    fn peek_slow(&mut self) -> Option<u8> {
        if self.exact {
            return self.src.peek();
        }
        if self.refill(1) == 0 {
            return None;
        }
        Some(self.buf[self.pos])
    }

    /// The next byte, consumed.
    #[inline]
    pub fn get(&mut self) -> Option<u8> {
        if self.pos < self.end {
            let b = self.buf[self.pos];
            self.pos += 1;
            self.consumed += 1;
            Some(b)
        } else {
            self.get_slow()
        }
    }

    #[cold]
    fn get_slow(&mut self) -> Option<u8> {
        if self.refill(1) == 0 {
            return None;
        }
        let b = self.buf[self.pos];
        self.pos += 1;
        self.consumed += 1;
        Some(b)
    }

    /// Puts the byte just read back (`std::istream::putback`).
    pub fn unget(&mut self, b: u8) {
        if self.pos > 0 {
            self.pos -= 1;
            self.buf[self.pos] = b;
        } else {
            if self.end == self.buf.len() {
                self.buf.push(0);
            }
            self.buf.copy_within(self.pos..self.end, 1);
            self.buf[0] = b;
            self.end += 1;
        }
        self.consumed -= 1;
    }

    /// Appends up to `n` bytes to `out`, returning how many were read. Short
    /// only at end of input. Memory grows with the data actually present, so
    /// a bogus length cannot force a large allocation.
    pub fn read_exact(&mut self, n: usize, out: &mut Vec<u8>) -> usize {
        let mut got = 0;
        while got < n {
            if self.pos < self.end {
                let take = (self.end - self.pos).min(n - got);
                out.extend_from_slice(&self.buf[self.pos..self.pos + take]);
                self.pos += take;
                self.consumed += take;
                got += take;
                continue;
            }
            if self.eof {
                break;
            }
            let want = (n - got).min(READ_CHUNK);
            if self.exact && want > 0 {
                // Pull straight into the output: never more than asked for.
                let start = out.len();
                out.resize(start + want, 0);
                let r = self.src.read(&mut out[start..]);
                out.truncate(start + r);
                if r == 0 {
                    self.eof = true;
                    break;
                }
                self.consumed += r;
                got += r;
            } else if self.refill(want) == 0 {
                break;
            }
        }
        got
    }

    /// Reads bytes up to but not including `delim`, or up to `max` bytes,
    /// appending to `out`. Returns true when the delimiter was reached (and left
    /// unconsumed). This is `std::istream::get(buf, n, delim)` without its
    /// failbit side effect; the caller checks the count.
    pub fn read_until(&mut self, delim: u8, max: usize, out: &mut Vec<u8>) -> bool {
        let mut got = 0;
        loop {
            if self.pos < self.end {
                let w = &self.buf[self.pos..self.end];
                let lim = w.len().min(max - got);
                match w[..lim].iter().position(|&b| b == delim) {
                    Some(i) => {
                        out.extend_from_slice(&w[..i]);
                        self.advance(i);
                        return true;
                    }
                    None => {
                        out.extend_from_slice(&w[..lim]);
                        self.advance(lim);
                        got += lim;
                        if got >= max {
                            return self.peek() == Some(delim);
                        }
                    }
                }
            }
            if self.exact {
                match self.src.peek() {
                    None => return false,
                    Some(b) if b == delim => return true,
                    Some(b) => {
                        self.src.read(&mut [0u8; 1]);
                        self.consumed += 1;
                        out.push(b);
                        got += 1;
                        if got >= max {
                            return self.src.peek() == Some(delim);
                        }
                    }
                }
            } else if self.refill(1) == 0 {
                return false;
            }
        }
    }
}
