//! A small JSON reader for the Icecast `status-json.xsl` document (the only
//! JSON the stream module reads; a dependency would be larger than this).
//! Strict RFC 8259 syntax, depth-limited.

#![forbid(unsafe_code)]

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    /// The number's text as written (converted as LLSD would on read).
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    pub(crate) fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The value as the viewer's `LLSD::asString` gave it after its JSON
    /// parse: strings as is, integers in decimal, reals in shortest form,
    /// booleans as "true"/"false", everything else empty.
    pub(crate) fn as_string(&self) -> String {
        match self {
            Json::String(s) => s.clone(),
            Json::Number(n) => match n.parse::<i64>() {
                Ok(i) => i.to_string(),
                Err(_) => n.parse::<f64>().map(|f| f.to_string()).unwrap_or_default(),
            },
            Json::Bool(b) => b.to_string(),
            _ => String::new(),
        }
    }
}

const MAX_DEPTH: usize = 64;

pub(crate) fn parse(text: &[u8]) -> Option<Json> {
    let mut p = Parser { b: text, i: 0 };
    p.ws();
    // A UTF-8 BOM is tolerated.
    if p.b[p.i..].starts_with(b"\xEF\xBB\xBF") {
        p.i += 3;
        p.ws();
    }
    let v = p.value(0)?;
    p.ws();
    (p.i == p.b.len()).then_some(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.b.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn lit(&mut self, s: &[u8], v: Json) -> Option<Json> {
        if self.b[self.i..].starts_with(s) {
            self.i += s.len();
            Some(v)
        } else {
            None
        }
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.ws();
        match *self.b.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut m = Vec::new();
                if self.eat(b'}') {
                    return Some(Json::Object(m));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    if !self.eat(b':') {
                        return None;
                    }
                    let v = self.value(depth + 1)?;
                    m.push((k, v));
                    if self.eat(b',') {
                        continue;
                    }
                    return self.eat(b'}').then_some(Json::Object(m));
                }
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                if self.eat(b']') {
                    return Some(Json::Array(a));
                }
                loop {
                    a.push(self.value(depth + 1)?);
                    if self.eat(b',') {
                        continue;
                    }
                    return self.eat(b']').then_some(Json::Array(a));
                }
            }
            b'"' => self.string().map(Json::String),
            b't' => self.lit(b"true", Json::Bool(true)),
            b'f' => self.lit(b"false", Json::Bool(false)),
            b'n' => self.lit(b"null", Json::Null),
            b'-' | b'0'..=b'9' => {
                let s = self.i;
                if self.b[self.i] == b'-' {
                    self.i += 1;
                }
                let digits = |p: &mut Self| {
                    let st = p.i;
                    while p.i < p.b.len() && p.b[p.i].is_ascii_digit() {
                        p.i += 1;
                    }
                    p.i > st
                };
                if !digits(self) {
                    return None;
                }
                if self.b.get(self.i) == Some(&b'.') {
                    self.i += 1;
                    if !digits(self) {
                        return None;
                    }
                }
                if matches!(self.b.get(self.i), Some(b'e' | b'E')) {
                    self.i += 1;
                    if matches!(self.b.get(self.i), Some(b'+' | b'-')) {
                        self.i += 1;
                    }
                    if !digits(self) {
                        return None;
                    }
                }
                Some(Json::Number(String::from_utf8_lossy(&self.b[s..self.i]).into_owned()))
            }
            _ => None,
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let h = self.b.get(self.i..self.i + 4)?;
        self.i += 4;
        u32::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()
    }

    fn string(&mut self) -> Option<String> {
        if self.b.get(self.i) != Some(&b'"') {
            return None;
        }
        self.i += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let mut u = self.hex4()?;
                            if (0xD800..0xDC00).contains(&u) && self.b[self.i..].starts_with(b"\\u") {
                                self.i += 2;
                                let lo = self.hex4()?;
                                if (0xDC00..0xE000).contains(&lo) {
                                    u = 0x10000 + ((u - 0xD800) << 10) + (lo - 0xDC00);
                                }
                            }
                            char::from_u32(u).unwrap_or('\u{FFFD}')
                        }
                        _ => return None,
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                0x00..=0x1F => return None,
                _ => out.push(c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icestats() {
        let doc = br#"{"icestats":{"admin":"x","source":[{"listenurl":"http://h:8000/a","title":"A - B","bitrate":128},{"listenurl":"http://h:8000/b","artist":"Art","title":"T\u00e9 \"q\""}]}}"#;
        let j = parse(doc).unwrap();
        let src = j.get("icestats").unwrap().get("source").unwrap();
        let Json::Array(a) = src else { panic!() };
        assert_eq!(a[0].get("bitrate").unwrap().as_string(), "128");
        assert_eq!(a[1].get("title").unwrap().as_string(), "Té \"q\"");
        assert!(parse(b"{\"a\":1,}").is_none());
        assert!(parse(b"[1 2]").is_none());
    }
}
