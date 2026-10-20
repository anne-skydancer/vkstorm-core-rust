//! The XML parser: `LLSDXMLParser::Impl` on expat, as one tokenizer with the
//! same well-formedness rules plus the viewer's element handlers.
//!
//! What the tokenizer enforces, as expat did: UTF-8 input (the viewer forced
//! the encoding regardless of the declaration), XML characters only (no C0
//! controls other than tab, LF and CR, no U+FFFE/U+FFFF, no surrogates),
//! matching end tags, quoted attributes without duplicates, the five
//! predefined entities and character references only, no `]]>` in text, no
//! `--` in comments, nothing but whitespace outside the root element, and the
//! XML declaration only at the very start. Line ends are normalised to LF.
//!
//! What the handlers do, as the viewer did: parsing stops at `</llsd>` (the
//! rest of that line is consumed), anything outside `<llsd>` is ignored, a
//! `<key>` must be a map's child, a value in a map needs a preceding
//! non-empty key, a value inside a scalar element is ignored, unknown elements
//! are undefined values, `<binary>` with an encoding other than base64 is
//! ignored, integers parse as `sscanf("%d")` with `asInteger()` as the
//! fallback, reals as `asReal()` (whole-string `istream` extraction), booleans
//! are `true` or `1`, base64 has its whitespace stripped, and a repeated key
//! keeps the last value.
//!
//! `XmlMode::Parse` (the C++ `parse()`) fails any document that ends before
//! `</llsd>`: the C++ appended a stray byte at EOF, which expat rejected.
//! `XmlMode::Lines` (`parseLines()`) returns the count for a complete
//! document that never closed `<llsd>`.

use crate::reader::Reader;
use crate::sink::Sink;
use crate::text::{self, is_space};
use crate::{PARSE_FAILURE, XmlMode};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Element {
    Llsd,
    Undef,
    Bool,
    Integer,
    Real,
    String,
    Uuid,
    Date,
    Uri,
    Binary,
    Map,
    Array,
    Key,
    Unknown,
}

fn read_element(name: &[u8]) -> Element {
    match name {
        b"key" => Element::Key,
        b"real" => Element::Real,
        b"integer" => Element::Integer,
        b"array" => Element::Array,
        b"map" => Element::Map,
        b"uuid" => Element::Uuid,
        b"undef" => Element::Undef,
        b"uri" => Element::Uri,
        b"binary" => Element::Binary,
        b"boolean" => Element::Bool,
        b"string" => Element::String,
        b"llsd" => Element::Llsd,
        b"date" => Element::Date,
        _ => Element::Unknown,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Container {
    Map,
    Array,
    Other,
}

struct Open {
    name_start: usize,
    name_len: usize,
    /// The handler's element for a tag it accepted; `None` for one it skipped.
    element: Option<Element>,
}

type R<T> = Result<T, ()>;

struct Parser<'r, 'a, S: Sink> {
    r: &'r mut Reader<'a>,
    sink: &'r mut S,
    // tokenizer
    open: Vec<Open>,
    names: Vec<u8>,
    seen_root: bool,
    root_done: bool,
    doc_start: usize,
    attrs: Vec<(Vec<u8>, Vec<u8>)>,
    // handlers
    in_llsd: bool,
    graceful_stop: bool,
    depth: i32,
    skipping: bool,
    skip_through: i32,
    cstack: Vec<Container>,
    content: Vec<u8>,
    current_key: Vec<u8>,
    parse_count: i32,
}

/// Parses one XML document from `r` into `sink`.
pub fn parse<S: Sink>(r: &mut Reader<'_>, sink: &mut S, mode: XmlMode) -> i32 {
    let mut p = Parser {
        r,
        sink,
        open: Vec::new(),
        names: Vec::new(),
        seen_root: false,
        root_done: false,
        doc_start: 0,
        attrs: Vec::new(),
        in_llsd: false,
        graceful_stop: false,
        depth: 0,
        skipping: false,
        skip_through: 0,
        cstack: Vec::new(),
        content: Vec::new(),
        current_key: Vec::new(),
        parse_count: 0,
    };
    match p.run(mode) {
        Ok(n) => n,
        Err(()) => PARSE_FAILURE,
    }
}

#[inline]
fn is_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b':' || b >= 0x80
}

#[inline]
fn is_name_char(b: u8) -> bool {
    is_name_start(b) || b.is_ascii_digit() || b == b'-' || b == b'.'
}

/// Length of the UTF-8 sequence led by `b0`, when it is a valid lead byte.
fn utf8_len(b0: u8) -> Option<usize> {
    match b0 {
        0x00..=0x7f => Some(1),
        0xc2..=0xdf => Some(2),
        0xe0..=0xef => Some(3),
        0xf0..=0xf4 => Some(4),
        _ => None,
    }
}

/// Validates one multi-byte sequence as a UTF-8 encoded XML character.
fn valid_sequence(s: &[u8]) -> bool {
    match s.len() {
        2 => s[1] & 0xc0 == 0x80,
        3 => {
            if s[1] & 0xc0 != 0x80 || s[2] & 0xc0 != 0x80 {
                return false;
            }
            match s[0] {
                0xe0 => s[1] >= 0xa0,
                0xed => s[1] < 0xa0,
                0xef => !(s[1] == 0xbf && (s[2] == 0xbe || s[2] == 0xbf)),
                _ => true,
            }
        }
        4 => {
            if s[1] & 0xc0 != 0x80 || s[2] & 0xc0 != 0x80 || s[3] & 0xc0 != 0x80 {
                return false;
            }
            match s[0] {
                0xf0 => s[1] >= 0x90,
                0xf4 => s[1] < 0x90,
                _ => true,
            }
        }
        _ => false,
    }
}

fn push_utf8(cp: u32, out: &mut Vec<u8>) {
    match char::from_u32(cp) {
        Some(c) => {
            let mut b = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
        }
        None => out.push(b'?'),
    }
}

fn valid_xml_char(cp: u32) -> bool {
    matches!(cp, 0x9 | 0xa | 0xd | 0x20..=0xd7ff | 0xe000..=0xfffd | 0x10000..=0x10ffff)
}

impl<S: Sink> Parser<'_, '_, S> {
    fn run(&mut self, mode: XmlMode) -> R<i32> {
        if self.r.ensure(3) && self.r.window().starts_with(b"\xef\xbb\xbf") {
            self.r.advance(3);
            self.doc_start = 3;
        }
        while !self.graceful_stop {
            let Some(b) = self.r.peek() else { break };
            if b == b'<' {
                self.markup()?;
            } else {
                self.text()?;
            }
        }
        if self.graceful_stop {
            self.discard_rest_of_line();
            return Ok(self.parse_count);
        }
        match mode {
            XmlMode::Parse => Err(()),
            XmlMode::Lines => {
                if !self.seen_root || !self.open.is_empty() {
                    Err(())
                } else {
                    Ok(self.parse_count)
                }
            }
        }
    }

    /// After `</llsd>`: the C++ parser's chunk ended at the line end, and
    /// whatever followed the end tag on that line was consumed with it.
    fn discard_rest_of_line(&mut self) {
        loop {
            let w = self.r.window();
            if w.is_empty() {
                return;
            }
            match w.iter().position(|&b| b == b'\n') {
                Some(i) => {
                    self.r.advance(i + 1);
                    return;
                }
                None => {
                    let n = w.len();
                    self.r.advance(n);
                    if self.r.at_eof() {
                        return;
                    }
                }
            }
        }
    }

    // ----- tokenizer -------------------------------------------------------

    /// Character data up to the next `<`.
    fn text(&mut self) -> R<()> {
        let inside = !self.open.is_empty();
        loop {
            let w = self.r.window();
            if w.is_empty() {
                if !self.r.ensure(1) {
                    return Ok(());
                }
                continue;
            }
            // Copy the run of plain bytes.
            let n = w
                .iter()
                .position(|&b| {
                    b == b'<'
                        || b == b'&'
                        || b == b'\r'
                        || b == b']'
                        || b < 0x20 && b != b'\t' && b != b'\n'
                        || b >= 0x80
                })
                .unwrap_or(w.len());
            if n > 0 {
                if inside {
                    self.content.extend_from_slice(&w[..n]);
                } else if w[..n].iter().any(|&b| !is_space(b)) {
                    return Err(());
                }
                self.r.advance(n);
                continue;
            }
            let b = w[0];
            match b {
                b'<' => return Ok(()),
                b'&' => {
                    let cp = self.reference()?;
                    if !inside {
                        return Err(());
                    }
                    push_utf8(cp, &mut self.content);
                }
                b'\r' => {
                    self.r.advance(1);
                    if self.r.peek() == Some(b'\n') {
                        self.r.advance(1);
                    }
                    if inside {
                        self.content.push(b'\n');
                    }
                }
                b']' => {
                    if self.r.ensure(3) && self.r.window().starts_with(b"]]>") {
                        return Err(());
                    }
                    self.r.advance(1);
                    if inside {
                        self.content.push(b']');
                    } else {
                        return Err(());
                    }
                }
                _ if b >= 0x80 => {
                    let len = self.utf8_char()?;
                    if inside {
                        let w = self.r.window();
                        self.content.extend_from_slice(&w[..len]);
                    } else {
                        return Err(());
                    }
                    self.r.advance(len);
                }
                _ => return Err(()),
            }
        }
    }

    /// Validates the UTF-8 sequence at the window start; returns its length.
    fn utf8_char(&mut self) -> R<usize> {
        let b0 = self.r.peek().ok_or(())?;
        let len = utf8_len(b0).ok_or(())?;
        if !self.r.ensure(len) {
            return Err(());
        }
        if !valid_sequence(&self.r.window()[..len]) {
            return Err(());
        }
        Ok(len)
    }

    /// An entity or character reference at `&`; returns the code point.
    fn reference(&mut self) -> R<u32> {
        self.r.advance(1);
        let mut name = Vec::with_capacity(8);
        loop {
            let c = self.r.get().ok_or(())?;
            if c == b';' {
                break;
            }
            if name.len() > 12 {
                return Err(());
            }
            name.push(c);
        }
        if let Some(num) = name.strip_prefix(b"#") {
            let cp = if let Some(hex) = num.strip_prefix(b"x") {
                if hex.is_empty() || !hex.iter().all(|b| b.is_ascii_hexdigit()) {
                    return Err(());
                }
                u32::from_str_radix(std::str::from_utf8(hex).map_err(|_| ())?, 16)
                    .map_err(|_| ())?
            } else {
                if num.is_empty() || !num.iter().all(|b| b.is_ascii_digit()) {
                    return Err(());
                }
                std::str::from_utf8(num)
                    .map_err(|_| ())?
                    .parse::<u32>()
                    .map_err(|_| ())?
            };
            if !valid_xml_char(cp) {
                return Err(());
            }
            return Ok(cp);
        }
        match name.as_slice() {
            b"lt" => Ok('<' as u32),
            b"gt" => Ok('>' as u32),
            b"amp" => Ok('&' as u32),
            b"apos" => Ok('\'' as u32),
            b"quot" => Ok('"' as u32),
            _ => Err(()),
        }
    }

    /// Reads a name at the window start into `self.names`; returns (start, len).
    fn name(&mut self) -> R<(usize, usize)> {
        let start = self.names.len();
        let first = self.r.peek().ok_or(())?;
        if !is_name_start(first) {
            return Err(());
        }
        loop {
            let Some(b) = self.r.peek() else { break };
            if b >= 0x80 {
                let len = self.utf8_char()?;
                let w = self.r.window();
                self.names.extend_from_slice(&w[..len]);
                self.r.advance(len);
            } else if is_name_char(b) {
                self.names.push(b);
                self.r.advance(1);
            } else {
                break;
            }
        }
        Ok((start, self.names.len() - start))
    }

    fn skip_ws(&mut self) -> usize {
        let mut n = 0;
        while let Some(b) = self.r.peek() {
            if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
                self.r.advance(1);
                n += 1;
            } else {
                break;
            }
        }
        n
    }

    /// Consumes bytes through the first occurrence of `end`.
    fn skip_through(&mut self, end: &[u8], forbid_double_dash: bool) -> R<()> {
        loop {
            if !self.r.ensure(end.len()) {
                return Err(());
            }
            let w = self.r.window();
            if w.starts_with(end) {
                self.r.advance(end.len());
                return Ok(());
            }
            if forbid_double_dash && w.starts_with(b"--") {
                return Err(());
            }
            let b = w[0];
            if b >= 0x80 {
                let len = self.utf8_char()?;
                self.r.advance(len);
            } else if b < 0x20 && b != b'\t' && b != b'\n' && b != b'\r' {
                return Err(());
            } else {
                self.r.advance(1);
            }
        }
    }

    fn markup(&mut self) -> R<()> {
        let at = self.r.consumed();
        self.r.ensure(9);
        let w = self.r.window();
        if w.starts_with(b"<?") {
            self.r.advance(2);
            let (s, l) = self.name()?;
            let target = &self.names[s..s + l];
            if target.eq_ignore_ascii_case(b"xml") {
                if target != b"xml" || at != self.doc_start {
                    return Err(());
                }
            }
            self.names.truncate(s);
            if self.skip_ws() == 0 {
                if !self.r.ensure(2) || !self.r.window().starts_with(b"?>") {
                    return Err(());
                }
            }
            return self.skip_through(b"?>", false);
        }
        if w.starts_with(b"<!--") {
            self.r.advance(4);
            self.skip_through(b"-->", true)?;
            return Ok(());
        }
        if w.starts_with(b"<![CDATA[") {
            if self.open.is_empty() {
                return Err(());
            }
            self.r.advance(9);
            return self.cdata();
        }
        if w.starts_with(b"<!DOCTYPE") {
            if self.seen_root {
                return Err(());
            }
            self.r.advance(9);
            return self.doctype();
        }
        if w.starts_with(b"</") {
            self.r.advance(2);
            let (s, l) = self.name()?;
            self.skip_ws();
            if self.r.get() != Some(b'>') {
                return Err(());
            }
            let top = self.open.pop().ok_or(())?;
            if self.names[top.name_start..top.name_start + top.name_len] != self.names[s..s + l] {
                return Err(());
            }
            self.names.truncate(top.name_start);
            if self.open.is_empty() {
                self.root_done = true;
            }
            self.end_element(top.element);
            return Ok(());
        }
        self.r.advance(1);
        if self.root_done {
            return Err(());
        }
        let (s, l) = self.name()?;
        // Attributes.
        self.attrs.clear();
        let mut self_closing = false;
        loop {
            let ws = self.skip_ws();
            match self.r.peek().ok_or(())? {
                b'>' => {
                    self.r.advance(1);
                    break;
                }
                b'/' => {
                    self.r.advance(1);
                    if self.r.get() != Some(b'>') {
                        return Err(());
                    }
                    self_closing = true;
                    break;
                }
                _ => {
                    if ws == 0 {
                        return Err(());
                    }
                    let (a, al) = self.name()?;
                    let aname = self.names[a..a + al].to_vec();
                    self.names.truncate(a);
                    self.skip_ws();
                    if self.r.get() != Some(b'=') {
                        return Err(());
                    }
                    self.skip_ws();
                    let q = self.r.get().ok_or(())?;
                    if q != b'"' && q != b'\'' {
                        return Err(());
                    }
                    let value = self.attribute_value(q)?;
                    if self.attrs.iter().any(|(n, _)| *n == aname) {
                        return Err(());
                    }
                    self.attrs.push((aname, value));
                }
            }
        }
        self.seen_root = true;
        let element = self.start_element(&self.names[s..s + l].to_vec());
        if self_closing {
            self.names.truncate(s);
            if self.open.is_empty() {
                self.root_done = true;
            }
            self.end_element(element);
        } else {
            self.open.push(Open {
                name_start: s,
                name_len: l,
                element,
            });
        }
        Ok(())
    }

    fn attribute_value(&mut self, quote: u8) -> R<Vec<u8>> {
        let mut v = Vec::new();
        loop {
            let b = self.r.peek().ok_or(())?;
            match b {
                _ if b == quote => {
                    self.r.advance(1);
                    return Ok(v);
                }
                b'<' => return Err(()),
                b'&' => {
                    let cp = self.reference()?;
                    push_utf8(cp, &mut v);
                }
                b'\r' => {
                    self.r.advance(1);
                    if self.r.peek() == Some(b'\n') {
                        self.r.advance(1);
                    }
                    v.push(b' ');
                }
                b'\n' | b'\t' => {
                    self.r.advance(1);
                    v.push(b' ');
                }
                _ if b < 0x20 => return Err(()),
                _ if b >= 0x80 => {
                    let len = self.utf8_char()?;
                    v.extend_from_slice(&self.r.window()[..len]);
                    self.r.advance(len);
                }
                _ => {
                    v.push(b);
                    self.r.advance(1);
                }
            }
        }
    }

    fn cdata(&mut self) -> R<()> {
        loop {
            if !self.r.ensure(3) {
                return Err(());
            }
            let w = self.r.window();
            if w.starts_with(b"]]>") {
                self.r.advance(3);
                return Ok(());
            }
            let b = w[0];
            match b {
                b'\r' => {
                    self.r.advance(1);
                    if self.r.peek() == Some(b'\n') {
                        self.r.advance(1);
                    }
                    self.content.push(b'\n');
                }
                _ if b >= 0x80 => {
                    let len = self.utf8_char()?;
                    self.content.extend_from_slice(&self.r.window()[..len]);
                    self.r.advance(len);
                }
                _ if b < 0x20 && b != b'\t' && b != b'\n' => return Err(()),
                _ => {
                    self.content.push(b);
                    self.r.advance(1);
                }
            }
        }
    }

    /// Skips a document type declaration, including an internal subset.
    fn doctype(&mut self) -> R<()> {
        let mut bracket = 0i32;
        let mut quote: Option<u8> = None;
        loop {
            let b = self.r.get().ok_or(())?;
            match quote {
                Some(q) => {
                    if b == q {
                        quote = None;
                    }
                }
                None => match b {
                    b'"' | b'\'' => quote = Some(b),
                    b'[' => bracket += 1,
                    b']' => bracket -= 1,
                    b'>' if bracket <= 0 => return Ok(()),
                    _ => {}
                },
            }
        }
    }

    // ----- LLSD handlers -----------------------------------------------------

    fn start_skipping(&mut self) {
        self.skipping = true;
        self.skip_through = self.depth;
    }

    fn start_element(&mut self, name: &[u8]) -> Option<Element> {
        self.depth += 1;
        if self.skipping {
            return None;
        }
        let element = read_element(name);
        self.content.clear();
        match element {
            Element::Llsd => {
                if self.in_llsd {
                    self.start_skipping();
                    return None;
                }
                self.in_llsd = true;
                return Some(element);
            }
            Element::Key => {
                if self.cstack.last() != Some(&Container::Map) {
                    self.start_skipping();
                    return None;
                }
                return Some(element);
            }
            Element::Binary => {
                if let Some((_, enc)) = self.attrs.iter().find(|(n, _)| n == b"encoding") {
                    if enc != b"base64" {
                        self.start_skipping();
                        return None;
                    }
                }
            }
            _ => {}
        }
        if !self.in_llsd {
            self.start_skipping();
            return None;
        }
        match self.cstack.last() {
            None => {}
            Some(Container::Map) => {
                if self.current_key.is_empty() {
                    self.start_skipping();
                    return None;
                }
                self.sink.key(&self.current_key);
                self.current_key.clear();
            }
            Some(Container::Array) => {}
            Some(Container::Other) => {
                self.start_skipping();
                return None;
            }
        }
        self.parse_count += 1;
        match element {
            Element::Map => {
                self.sink.begin_map();
                self.cstack.push(Container::Map);
            }
            Element::Array => {
                self.sink.begin_array();
                self.cstack.push(Container::Array);
            }
            _ => self.cstack.push(Container::Other),
        }
        Some(element)
    }

    fn end_element(&mut self, element: Option<Element>) {
        self.depth -= 1;
        if self.skipping {
            if self.depth < self.skip_through {
                self.skipping = false;
            }
            return;
        }
        let Some(element) = element else { return };
        match element {
            Element::Llsd => {
                if self.in_llsd {
                    self.in_llsd = false;
                    self.graceful_stop = true;
                }
                return;
            }
            Element::Key => {
                self.current_key.clear();
                self.current_key.extend_from_slice(&self.content);
                return;
            }
            _ => {}
        }
        if !self.in_llsd {
            return;
        }
        self.cstack.pop();
        let content = &self.content;
        match element {
            Element::Undef | Element::Unknown => self.sink.undef(),
            Element::Bool => self.sink.boolean(content == b"true" || content == b"1"),
            Element::Integer => {
                let v = match text::sscanf_int(content) {
                    Some(i) => i,
                    None => text::real_as_integer(text::string_as_real(content)),
                };
                self.sink.integer(v);
            }
            Element::Real => self.sink.real(text::string_as_real(content)),
            Element::String => self.sink.string(content),
            Element::Uuid => self.sink.uuid(&text::uuid_set(content)),
            Element::Date => self.sink.date(text::date_parse(content).unwrap_or(0.0)),
            Element::Uri => self.sink.uri(content),
            Element::Binary => {
                let stripped: Vec<u8> = content.iter().copied().filter(|&b| !is_space(b)).collect();
                let mut data = Vec::new();
                text::base64_decode_apr(&stripped, &mut data);
                self.sink.binary(&data);
            }
            Element::Map => self.sink.end_map(),
            Element::Array => self.sink.end_array(),
            Element::Llsd | Element::Key => {}
        }
        self.content.clear();
    }
}
