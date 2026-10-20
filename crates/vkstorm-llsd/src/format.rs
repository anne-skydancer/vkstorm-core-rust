//! The formatters, as one push-style writer: the caller walks its own value
//! tree (the viewer's `LLSD`, or a [`Value`](crate::Value)) in document order
//! and calls one method per node. The output is byte-identical to the
//! viewer's `LLSDNotationFormatter`, `LLSDBinaryFormatter` and
//! `LLSDXMLFormatter` (whose `format()` wraps the document in `<llsd>`).

use crate::text::{self, FloatField};

/// The three serialization formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Notation,
    Binary,
    Xml,
}

/// Formatter options: the `EFormatterOptions` flags, `boolalpha`, and the
/// destination stream's real-number settings.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// `OPTIONS_PRETTY`: indentation and line breaks.
    pub pretty: bool,
    /// `OPTIONS_PRETTY_BINARY`: notation binary as `b16"..."` instead of raw bytes.
    pub pretty_binary: bool,
    /// Booleans as `true`/`false` instead of `1`/`0` (the formatter's or the stream's flag).
    pub boolalpha: bool,
    /// The stream's precision for reals (notation only; XML always uses 25).
    pub precision: usize,
    /// The stream's floatfield.
    pub floatfield: FloatField,
}

impl Default for Options {
    /// The viewer's defaults: `OPTIONS_PRETTY_BINARY`, numeric booleans, a
    /// stream with precision 6 in general notation.
    fn default() -> Self {
        Options {
            pretty: false,
            pretty_binary: true,
            boolalpha: false,
            precision: 6,
            floatfield: FloatField::General,
        }
    }
}

/// Receives the formatted bytes.
pub trait Out {
    fn write(&mut self, data: &[u8]);
}

impl Out for Vec<u8> {
    fn write(&mut self, data: &[u8]) {
        self.extend_from_slice(data);
    }
}

impl Out for &mut Vec<u8> {
    fn write(&mut self, data: &[u8]) {
        self.extend_from_slice(data);
    }
}

const FLUSH_AT: usize = 64 * 1024;

struct Frame {
    map: bool,
    /// The container's own indentation level.
    level: u32,
    /// No element written yet (notation comma placement).
    first: bool,
    /// XML: an empty container was written as a single tag; nothing to close.
    empty: bool,
}

/// A push-style formatter for one document, owning its output sink.
pub struct Formatter<O: Out> {
    format: Format,
    opts: Options,
    out: O,
    buf: Vec<u8>,
    stack: Vec<Frame>,
    count: i32,
}

impl<O: Out> Formatter<O> {
    /// Starts a document. The XML formatter writes its `<llsd>` opener here.
    pub fn new(format: Format, opts: Options, out: O) -> Self {
        let mut f = Formatter {
            format,
            opts,
            out,
            buf: Vec::with_capacity(4096),
            stack: Vec::new(),
            count: 0,
        };
        if format == Format::Xml {
            f.buf.extend_from_slice(b"<llsd>");
            f.post();
        }
        f
    }

    /// Ends the document, flushes, and returns the number of values written
    /// (the C++ `format()` return value) together with the output sink.
    pub fn finish(mut self) -> (i32, O) {
        if self.format == Format::Xml {
            self.buf.extend_from_slice(b"</llsd>\n");
        }
        self.out.write(&self.buf);
        (self.count, self.out)
    }

    #[inline]
    fn maybe_flush(&mut self) {
        if self.buf.len() >= FLUSH_AT {
            self.out.write(&self.buf);
            self.buf.clear();
        }
    }

    #[inline]
    fn post(&mut self) {
        if self.opts.pretty {
            self.buf.push(b'\n');
        }
    }

    #[inline]
    fn pre(&mut self, level: u32) {
        if self.opts.pretty {
            for _ in 0..level {
                self.buf.extend_from_slice(b"    ");
            }
        }
    }

    /// The level at which the next value is written.
    fn level(&self) -> u32 {
        match (self.format, self.stack.last()) {
            (Format::Xml, None) => 1,
            (_, None) => 0,
            (Format::Notation, Some(f)) => f.level + if f.map { 2 } else { 1 },
            (_, Some(f)) => f.level + 1,
        }
    }

    /// Bookkeeping before any value: counting, and the notation array comma.
    fn begin_value(&mut self) -> u32 {
        self.count += 1;
        if self.format == Format::Notation {
            if let Some(f) = self.stack.last_mut() {
                if !f.map {
                    if !f.first {
                        self.buf.push(b',');
                    }
                    f.first = false;
                }
            }
        }
        self.level()
    }

    fn be_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    fn xml_scalar(
        &mut self,
        level: u32,
        open: &[u8],
        body: impl FnOnce(&mut Vec<u8>),
        close: &[u8],
    ) {
        self.pre(level);
        self.buf.extend_from_slice(open);
        body(&mut self.buf);
        self.buf.extend_from_slice(close);
        self.post();
    }

    pub fn undef(&mut self) {
        let level = self.begin_value();
        match self.format {
            Format::Notation | Format::Binary => self.buf.push(b'!'),
            Format::Xml => {
                self.pre(level);
                self.buf.extend_from_slice(b"<undef />");
                self.post();
            }
        }
        self.maybe_flush();
    }

    pub fn boolean(&mut self, v: bool) {
        let level = self.begin_value();
        match self.format {
            Format::Binary => self.buf.push(if v { b'1' } else { b'0' }),
            Format::Notation => {
                if self.opts.boolalpha {
                    self.buf
                        .extend_from_slice(if v { b"true" } else { b"false" });
                } else {
                    self.buf.push(if v { b'1' } else { b'0' });
                }
            }
            Format::Xml => {
                let alpha = self.opts.boolalpha;
                self.xml_scalar(
                    level,
                    b"<boolean>",
                    |b| {
                        if alpha {
                            b.extend_from_slice(if v { b"true" } else { b"false" })
                        } else {
                            b.push(if v { b'1' } else { b'0' })
                        }
                    },
                    b"</boolean>",
                )
            }
        }
        self.maybe_flush();
    }

    pub fn integer(&mut self, v: i32) {
        let level = self.begin_value();
        match self.format {
            Format::Binary => {
                self.buf.push(b'i');
                self.buf.extend_from_slice(&v.to_be_bytes());
            }
            Format::Notation => {
                self.buf.push(b'i');
                self.buf.extend_from_slice(v.to_string().as_bytes());
            }
            Format::Xml => self.xml_scalar(
                level,
                b"<integer>",
                |b| b.extend_from_slice(v.to_string().as_bytes()),
                b"</integer>",
            ),
        }
        self.maybe_flush();
    }

    pub fn real(&mut self, v: f64) {
        let level = self.begin_value();
        let (prec, ff) = (self.opts.precision, self.opts.floatfield);
        match self.format {
            Format::Binary => {
                self.buf.push(b'r');
                self.buf.extend_from_slice(&v.to_bits().to_be_bytes());
            }
            Format::Notation => {
                self.buf.push(b'r');
                text::fmt_real(v, prec, ff, &mut self.buf);
            }
            Format::Xml => self.xml_scalar(
                level,
                b"<real>",
                |b| text::fmt_real(v, 25, ff, b),
                b"</real>",
            ),
        }
        self.maybe_flush();
    }

    /// A real whose text form the caller already produced with the
    /// formatter's `realFormat` (the binary formatter ignores it).
    pub fn real_text(&mut self, v: f64, formatted: &[u8]) {
        if self.format == Format::Binary {
            return self.real(v);
        }
        let level = self.begin_value();
        match self.format {
            Format::Notation => {
                self.buf.push(b'r');
                self.buf.extend_from_slice(formatted);
            }
            _ => self.xml_scalar(
                level,
                b"<real>",
                |b| b.extend_from_slice(formatted),
                b"</real>",
            ),
        }
        self.maybe_flush();
    }

    pub fn uuid(&mut self, v: &[u8; 16]) {
        let level = self.begin_value();
        match self.format {
            Format::Binary => {
                self.buf.push(b'u');
                self.buf.extend_from_slice(v);
            }
            Format::Notation => {
                self.buf.push(b'u');
                text::uuid_to_hex(v, &mut self.buf);
            }
            Format::Xml => {
                if *v == text::NULL_UUID {
                    self.pre(level);
                    self.buf.extend_from_slice(b"<uuid />");
                    self.post();
                } else {
                    self.xml_scalar(level, b"<uuid>", |b| text::uuid_to_hex(v, b), b"</uuid>");
                }
            }
        }
        self.maybe_flush();
    }

    pub fn string(&mut self, v: &[u8]) {
        let level = self.begin_value();
        match self.format {
            Format::Binary => {
                self.buf.push(b's');
                self.be_u32(v.len() as u32);
                self.buf.extend_from_slice(v);
            }
            Format::Notation => {
                self.buf.push(b'\'');
                text::notation_escape(v, &mut self.buf);
                self.buf.push(b'\'');
            }
            Format::Xml => {
                if v.is_empty() {
                    self.pre(level);
                    self.buf.extend_from_slice(b"<string />");
                    self.post();
                } else {
                    self.xml_scalar(level, b"<string>", |b| text::xml_escape(v, b), b"</string>");
                }
            }
        }
        self.maybe_flush();
    }

    pub fn date(&mut self, seconds: f64) {
        let level = self.begin_value();
        match self.format {
            Format::Binary => {
                self.buf.push(b'd');
                self.buf.extend_from_slice(&seconds.to_ne_bytes());
            }
            Format::Notation => {
                self.buf.extend_from_slice(b"d\"");
                text::date_format(seconds, &mut self.buf);
                self.buf.push(b'"');
            }
            Format::Xml => self.xml_scalar(
                level,
                b"<date>",
                |b| text::date_format(seconds, b),
                b"</date>",
            ),
        }
        self.maybe_flush();
    }

    pub fn uri(&mut self, v: &[u8]) {
        let level = self.begin_value();
        match self.format {
            Format::Binary => {
                self.buf.push(b'l');
                self.be_u32(v.len() as u32);
                self.buf.extend_from_slice(v);
            }
            Format::Notation => {
                self.buf.extend_from_slice(b"l\"");
                text::notation_escape(v, &mut self.buf);
                self.buf.push(b'"');
            }
            Format::Xml => self.xml_scalar(level, b"<uri>", |b| text::xml_escape(v, b), b"</uri>"),
        }
        self.maybe_flush();
    }

    pub fn binary(&mut self, v: &[u8]) {
        let level = self.begin_value();
        match self.format {
            Format::Binary => {
                self.buf.push(b'b');
                self.be_u32(v.len() as u32);
                self.buf.extend_from_slice(v);
            }
            Format::Notation => {
                if self.opts.pretty_binary {
                    self.buf.extend_from_slice(b"b16\"");
                    text::hex_upper(v, &mut self.buf);
                } else {
                    self.buf.extend_from_slice(b"b(");
                    self.buf.extend_from_slice(v.len().to_string().as_bytes());
                    self.buf.extend_from_slice(b")\"");
                    self.buf.extend_from_slice(v);
                }
                self.buf.push(b'"');
            }
            Format::Xml => {
                if v.is_empty() {
                    self.pre(level);
                    self.buf.extend_from_slice(b"<binary />");
                    self.post();
                } else {
                    self.xml_scalar(
                        level,
                        b"<binary encoding=\"base64\">",
                        |b| text::base64_encode(v, b),
                        b"</binary>",
                    );
                }
            }
        }
        self.maybe_flush();
    }

    /// Opens a map with `count` entries (the binary format writes the count).
    pub fn begin_map(&mut self, count: usize) {
        let level = self.begin_value();
        let mut empty = false;
        match self.format {
            Format::Binary => {
                self.buf.push(b'{');
                self.be_u32(count as u32);
            }
            Format::Notation => {
                if level != 0 {
                    self.post();
                    self.pre(level);
                }
                self.buf.push(b'{');
            }
            Format::Xml => {
                self.pre(level);
                if count == 0 {
                    self.buf.extend_from_slice(b"<map />");
                    empty = true;
                } else {
                    self.buf.extend_from_slice(b"<map>");
                }
                self.post();
            }
        }
        self.stack.push(Frame {
            map: true,
            level,
            first: true,
            empty,
        });
    }

    /// The key of the next map value.
    pub fn key(&mut self, k: &[u8]) {
        let Some(f) = self.stack.last_mut() else {
            return;
        };
        let level = f.level;
        let first = std::mem::replace(&mut f.first, false);
        match self.format {
            Format::Binary => {
                self.buf.push(b'k');
                self.be_u32(k.len() as u32);
                self.buf.extend_from_slice(k);
            }
            Format::Notation => {
                if !first {
                    self.buf.push(b',');
                }
                self.post();
                self.pre(level + 1);
                self.buf.push(b'\'');
                text::notation_escape(k, &mut self.buf);
                self.buf.extend_from_slice(b"':");
            }
            Format::Xml => {
                self.pre(level + 1);
                self.buf.extend_from_slice(b"<key>");
                text::xml_escape(k, &mut self.buf);
                self.buf.extend_from_slice(b"</key>");
                self.post();
            }
        }
        self.maybe_flush();
    }

    pub fn end_map(&mut self) {
        let Some(f) = self.stack.pop() else { return };
        match self.format {
            Format::Binary => self.buf.push(b'}'),
            Format::Notation => {
                self.post();
                self.pre(f.level);
                self.buf.push(b'}');
            }
            Format::Xml => {
                if !f.empty {
                    self.pre(f.level);
                    self.buf.extend_from_slice(b"</map>");
                    self.post();
                }
            }
        }
        self.maybe_flush();
    }

    /// Opens an array with `count` elements (the binary format writes the count).
    pub fn begin_array(&mut self, count: usize) {
        let level = self.begin_value();
        let mut empty = false;
        match self.format {
            Format::Binary => {
                self.buf.push(b'[');
                self.be_u32(count as u32);
            }
            Format::Notation => {
                self.post();
                self.pre(level);
                self.buf.push(b'[');
            }
            Format::Xml => {
                self.pre(level);
                if count == 0 {
                    self.buf.extend_from_slice(b"<array />");
                    empty = true;
                } else {
                    self.buf.extend_from_slice(b"<array>");
                }
                self.post();
            }
        }
        self.stack.push(Frame {
            map: false,
            level,
            first: true,
            empty,
        });
    }

    pub fn end_array(&mut self) {
        let Some(f) = self.stack.pop() else { return };
        match self.format {
            Format::Binary | Format::Notation => self.buf.push(b']'),
            Format::Xml => {
                if !f.empty {
                    self.pre(f.level);
                    self.buf.extend_from_slice(b"</array>");
                    self.post();
                }
            }
        }
        self.maybe_flush();
    }
}
