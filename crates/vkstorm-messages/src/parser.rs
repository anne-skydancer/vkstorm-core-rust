//! The template language: `LLTemplateTokenizer` and `LLTemplateParser`
//! (`llmessagetemplateparser.cpp`).
//!
//! The tokenizer keeps the C++ cursor semantics exactly (a "not started"
//! state before the first token, `want` leaving the cursor at the end once it
//! has run off it), since the viewer's parser tests drive it directly. Where
//! the C++ aborted (`LL_ERRS`) this returns an error carrying the same text,
//! and the viewer aborts with it.
//!
//! Number conversion is settled as one behaviour on every platform: the
//! message number follows `strtoul(s, 0, 0)` (base prefix `0x`, leading-zero
//! octal, a `-` sign negating modulo 2^32) and saturates at 0xFFFFFFFF, as a
//! 32-bit `unsigned long` did on Windows (glibc's 64-bit `unsigned long`
//! would have truncated instead); block repeat counts and variable sizes
//! follow `atoi` and saturate at `i32::MAX` (MSVC's result; glibc's `(int)`
//! of a saturated 64-bit `long` is -1). The template never exercises either.

use crate::template::{
    Block, BlockType, Deprecation, Encoding, Frequency, Template, Trust, VarType,
};
use std::fmt;

/// A parse or tokenizer failure, with the C++ message text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ParseError {}

fn err<T>(message: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError {
        message: message.into(),
    })
}

#[derive(Clone, Debug)]
struct Token {
    text: String,
    line: u32,
}

/// `LLTemplateTokenizer`.
#[derive(Clone, Debug)]
pub struct Tokenizer {
    tokens: Vec<Token>,
    started: bool,
    current: usize,
}

impl Tokenizer {
    /// Splits `contents` as the C++ did: lines on every `\r` or `\n` (each
    /// one counts as a line, so CRLF input numbers lines by two), words on
    /// spaces and tabs, and a word starting with `/` ends the line.
    pub fn new(contents: &str) -> Tokenizer {
        let mut tokens = Vec::new();
        for (i, line) in contents.split(['\r', '\n']).enumerate() {
            let line_counter = i as u32 + 1;
            for word in line.split([' ', '\t']).filter(|w| !w.is_empty()) {
                if word.starts_with('/') {
                    break;
                }
                tokens.push(Token {
                    text: word.to_owned(),
                    line: line_counter,
                });
            }
        }
        Tokenizer {
            tokens,
            started: false,
            current: 0,
        }
    }

    fn error<T>(&self, message: &str) -> Result<T, ParseError> {
        if self.at_eof() {
            err(format!("Unexpected end of file: {message}"))
        } else {
            err(format!(
                "Problem parsing message template at line {}, with token '{}' : {}",
                self.line(),
                self.tokens[self.current].text,
                message
            ))
        }
    }

    fn inc(&mut self) -> Result<(), ParseError> {
        if self.at_eof() {
            self.error("trying to increment token of EOF")
        } else if self.started {
            self.current += 1;
            Ok(())
        } else {
            self.started = true;
            self.current = 0;
            Ok(())
        }
    }

    fn dec(&mut self) -> Result<(), ParseError> {
        if self.current == 0 {
            if self.started {
                self.started = false;
                Ok(())
            } else {
                self.error("trying to decrement past beginning of file")
            }
        } else {
            self.current -= 1;
            Ok(())
        }
    }

    fn get(&self) -> Result<&str, ParseError> {
        if self.at_eof() {
            return self.error("trying to get EOF");
        }
        Ok(&self.tokens[self.current].text)
    }

    /// The line of the current token, 0 at the end.
    pub fn line(&self) -> u32 {
        if self.at_eof() {
            0
        } else {
            self.tokens[self.current].line
        }
    }

    pub fn at_eof(&self) -> bool {
        self.current >= self.tokens.len()
    }

    /// Advances and returns the token there.
    #[allow(clippy::should_implement_trait)] // the C++ name; not an iterator
    pub fn next(&mut self) -> Result<String, ParseError> {
        self.inc()?;
        Ok(self.get()?.to_owned())
    }

    /// Consumes the next token if it is `token`.
    pub fn want(&mut self, token: &str) -> Result<bool, ParseError> {
        if self.at_eof() {
            return Ok(false);
        }
        self.inc()?;
        if self.at_eof() {
            return Ok(false);
        }
        if self.get()? != token {
            self.dec()?;
            return Ok(false);
        }
        Ok(true)
    }

    /// Whether the next token is the end (consumed when it is).
    pub fn want_eof(&mut self) -> Result<bool, ParseError> {
        if self.at_eof() {
            return Ok(true);
        }
        self.inc()?;
        if !self.at_eof() {
            self.dec()?;
            return Ok(false);
        }
        Ok(true)
    }
}

// ---- the token checkers ("fv*", "sd*", "d*") -----------------------------

fn first_variable_ok(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn variable_ok_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// `b_variable_ok`: a C identifier.
pub fn variable_ok(token: &str) -> bool {
    let b = token.as_bytes();
    match b.split_first() {
        None => true,
        Some((first, rest)) => {
            first_variable_ok(*first) && rest.iter().all(|c| variable_ok_char(*c))
        }
    }
}

/// `b_positive_integer_ok`: digits only.
pub fn positive_integer_ok(token: &str) -> bool {
    token.bytes().all(|c| c.is_ascii_digit())
}

/// `strtoul(s, NULL, 0)` into a `U32`, saturating at `u32::MAX`.
pub fn strtoul_u32(s: &str) -> u32 {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && (b[i] == b' ' || (b'\t'..=b'\r').contains(&b[i])) {
        i += 1;
    }
    let mut negative = false;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        negative = b[i] == b'-';
        i += 1;
    }
    let mut base: u64 = 10;
    if i + 1 < b.len()
        && b[i] == b'0'
        && (b[i + 1] == b'x' || b[i + 1] == b'X')
        && b.get(i + 2).is_some_and(|c| c.is_ascii_hexdigit())
    {
        base = 16;
        i += 2;
    } else if i < b.len() && b[i] == b'0' {
        base = 8;
    }
    let mut value: u64 = 0;
    let mut saturated = false;
    while i < b.len() {
        let d = match b[i] {
            c @ b'0'..=b'9' => (c - b'0') as u64,
            c @ b'a'..=b'f' => (c - b'a' + 10) as u64,
            c @ b'A'..=b'F' => (c - b'A' + 10) as u64,
            _ => break,
        };
        if d >= base {
            break;
        }
        match value.checked_mul(base).and_then(|v| v.checked_add(d)) {
            Some(v) => value = v,
            None => saturated = true,
        }
        i += 1;
    }
    let value: u32 = if saturated || value > u32::MAX as u64 {
        u32::MAX
    } else {
        value as u32
    };
    if negative {
        value.wrapping_neg()
    } else {
        value
    }
}

/// `atoi` on a digits-only token, saturating at `i32::MAX`.
pub fn atoi_positive(s: &str) -> i32 {
    let mut value: i32 = 0;
    for c in s.bytes() {
        if !c.is_ascii_digit() {
            break;
        }
        value = value.saturating_mul(10).saturating_add((c - b'0') as i32);
    }
    value
}

/// `LLTemplateParser::parseVariable`: `None` when the next token is not `{`.
pub fn parse_variable(
    tokens: &mut Tokenizer,
) -> Result<Option<(String, VarType, i32)>, ParseError> {
    if !tokens.want("{")? {
        return Ok(None);
    }
    let var_name = tokens.next()?;
    if !variable_ok(&var_name) {
        return err(format!(
            "Not a legit variable name: {var_name} at {}",
            tokens.line()
        ));
    }
    let var_type = tokens.next()?;
    let (ty, size) = match var_type.as_str() {
        "U8" => (VarType::U8, 1),
        "U16" => (VarType::U16, 2),
        "U32" => (VarType::U32, 4),
        "U64" => (VarType::U64, 8),
        "S8" => (VarType::S8, 1),
        "S16" => (VarType::S16, 2),
        "S32" => (VarType::S32, 4),
        "S64" => (VarType::S64, 8),
        "F32" => (VarType::F32, 4),
        "F64" => (VarType::F64, 8),
        "LLVector3" => (VarType::LLVector3, 12),
        "LLVector3d" => (VarType::LLVector3d, 24),
        "LLVector4" => (VarType::LLVector4, 16),
        "LLQuaternion" => (VarType::LLQuaternion, 12),
        "LLUUID" => (VarType::LLUUID, 16),
        "BOOL" => (VarType::Bool, 1),
        "IPADDR" => (VarType::IpAddr, 4),
        "IPPORT" => (VarType::IpPort, 2),
        "Fixed" | "Variable" => {
            let variable_size = tokens.next()?;
            if !positive_integer_ok(&variable_size) {
                return err(format!(
                    "not a legal integer variable size: {variable_size} at {}",
                    tokens.line()
                ));
            }
            let ty = if var_type == "Variable" {
                VarType::Variable
            } else {
                VarType::Fixed
            };
            (ty, atoi_positive(&variable_size))
        }
        _ => return err(format!("bad variable type:{var_type} at {}", tokens.line())),
    };
    if !tokens.want("}")? {
        return err(format!(
            "Expecting closing }} for variable {var_name} at {}",
            tokens.line()
        ));
    }
    Ok(Some((var_name, ty, size)))
}

/// `LLTemplateParser::parseBlock`: `None` when the next token is not `{`.
pub fn parse_block(tokens: &mut Tokenizer) -> Result<Option<Block>, ParseError> {
    if !tokens.want("{")? {
        return Ok(None);
    }
    let block_name = tokens.next()?;
    if !variable_ok(&block_name) {
        return err(format!(
            "not a legal block name: {block_name} at {}",
            tokens.line()
        ));
    }
    let block_type = tokens.next()?;
    let mut block = match block_type.as_str() {
        "Single" => Block::new(block_name.clone(), 0, BlockType::Single, 1),
        "Multiple" => {
            let repeats = tokens.next()?;
            if !positive_integer_ok(&repeats) {
                return err(format!(
                    "not a legal integer for block multiple count: {repeats} at {}",
                    tokens.line()
                ));
            }
            Block::new(
                block_name.clone(),
                0,
                BlockType::Multiple,
                atoi_positive(&repeats),
            )
        }
        "Variable" => Block::new(block_name.clone(), 0, BlockType::Variable, 1),
        _ => return err(format!("bad block type: {block_type} at {}", tokens.line())),
    };
    while let Some((name, ty, size)) = parse_variable(tokens)? {
        block
            .add_variable(name, 0, ty, size)
            .map_err(|message| ParseError { message })?;
    }
    if !tokens.want("}")? {
        return err(format!(
            "Expecting closing }} for block {block_name} at {}",
            tokens.line()
        ));
    }
    Ok(Some(block))
}

/// `LLTemplateParser::parseMessage`: `None` when the next token is not `{`.
pub fn parse_message(tokens: &mut Tokenizer) -> Result<Option<Template>, ParseError> {
    if !tokens.want("{")? {
        return Ok(None);
    }
    let template_name = tokens.next()?;
    if !variable_ok(&template_name) {
        return err(format!(
            "Not legit variable name: {template_name} at {}",
            tokens.line()
        ));
    }
    let freq_string = tokens.next()?;
    let frequency = match freq_string.as_str() {
        "High" => Frequency::High,
        "Medium" => Frequency::Medium,
        "Low" | "Fixed" => Frequency::Low,
        _ => {
            return err(format!(
                "Expected frequency, got {freq_string} at {}",
                tokens.line()
            ));
        }
    };
    let mut message_number = strtoul_u32(&tokens.next()?);
    match frequency {
        Frequency::High => {}
        Frequency::Medium => message_number |= 255 << 8,
        Frequency::Low => message_number |= (255 << 24) | (255 << 16),
        Frequency::Null => unreachable!(),
    }
    let mut template = Template::new(template_name.clone(), 0, message_number, frequency);
    let trust = tokens.next()?;
    template.trust = match trust.as_str() {
        "Trusted" => Trust::Trusted,
        "NotTrusted" => Trust::NotTrusted,
        _ => return err(format!("Bad trust {trust} at {}", tokens.line())),
    };
    let encoding = tokens.next()?;
    template.encoding = match encoding.as_str() {
        "Unencoded" => Encoding::Unencoded,
        "Zerocoded" => Encoding::ZeroCoded,
        _ => return err(format!("Bad encoding {encoding} at {}", tokens.line())),
    };
    template.deprecation = if tokens.want("Deprecated")? {
        Deprecation::Deprecated
    } else if tokens.want("UDPDeprecated")? {
        Deprecation::UdpDeprecated
    } else if tokens.want("UDPBlackListed")? {
        Deprecation::UdpBlacklisted
    } else {
        // the default, which may also be spelled out
        tokens.want("NotDeprecated")?;
        Deprecation::NotDeprecated
    };
    while let Some(block) = parse_block(tokens)? {
        template
            .add_block(block)
            .map_err(|message| ParseError { message })?;
    }
    if !tokens.want("}")? {
        return err(format!(
            "Expecting closing }} for message {template_name} at {}",
            tokens.line()
        ));
    }
    Ok(Some(template))
}

/// A parsed template file: the version and the messages, in file order,
/// without those marked `Deprecated` (dropped as the C++ did).
#[derive(Clone, Debug, PartialEq)]
pub struct TemplateFile {
    pub version: f32,
    pub messages: Vec<Template>,
}

/// `LLTemplateParser::LLTemplateParser`: the version line, then every
/// message, then the end of the file.
pub fn parse_template(tokens: &mut Tokenizer) -> Result<TemplateFile, ParseError> {
    let version = parse_version(tokens)?;
    let mut messages = Vec::new();
    while let Some(template) = parse_message(tokens)? {
        if template.deprecation != Deprecation::Deprecated {
            messages.push(template);
        }
    }
    if !tokens.want_eof()? {
        let next = tokens.next()?;
        return err(format!(
            "Expected end of template or a message, instead found: {next} at {}",
            tokens.line()
        ));
    }
    Ok(TemplateFile { version, messages })
}

/// The `version <number>` line that must open the file; `atof` of the
/// token (0 when it is not a number).
pub fn parse_version(tokens: &mut Tokenizer) -> Result<f32, ParseError> {
    if tokens.want("version")? {
        let vers_string = tokens.next()?;
        Ok(atof(&vers_string))
    } else {
        let next = tokens.next()?;
        err(format!(
            "Version must be first in the message template, found {next}"
        ))
    }
}

/// `(F32)atof(s)`: the longest decimal prefix, 0 when there is none.
fn atof(s: &str) -> f32 {
    let b = s.as_bytes();
    let mut end = 0;
    if end < b.len() && (b[end] == b'+' || b[end] == b'-') {
        end += 1;
    }
    let mut digits = 0;
    while end < b.len() && b[end].is_ascii_digit() {
        end += 1;
        digits += 1;
    }
    if end < b.len() && b[end] == b'.' {
        end += 1;
        while end < b.len() && b[end].is_ascii_digit() {
            end += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return 0.0;
    }
    if end < b.len() && (b[end] == b'e' || b[end] == b'E') {
        let mut e = end + 1;
        if e < b.len() && (b[e] == b'+' || b[e] == b'-') {
            e += 1;
        }
        if e < b.len() && b[e].is_ascii_digit() {
            while e < b.len() && b[e].is_ascii_digit() {
                e += 1;
            }
            end = e;
        }
    }
    s[..end].parse::<f64>().unwrap_or(0.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_lines_and_want() {
        let mut t = Tokenizer::new("first line\nnext\t line\n\nfourth");
        assert_eq!(t.next().unwrap(), "first");
        assert_eq!(t.line(), 1);
        assert_eq!(t.next().unwrap(), "line");
        assert_eq!(t.next().unwrap(), "next");
        assert_eq!(t.line(), 2);
        assert_eq!(t.next().unwrap(), "line");
        assert_eq!(t.next().unwrap(), "fourth");
        assert_eq!(t.line(), 4);

        let mut t = Tokenizer::new("\n\t{ \t   Test1 Fixed \n 523 }\n\n");
        assert!(t.want("{").unwrap());
        assert_eq!(t.next().unwrap(), "Test1");
        assert_eq!(t.line(), 2);
        assert_eq!(t.next().unwrap(), "Fixed");
        assert_eq!(t.next().unwrap(), "523");
        assert_eq!(t.line(), 3);
        assert!(t.want("}").unwrap());

        let mut t = Tokenizer::new("first line\nnext\t line\n\nfourth");
        assert!(t.want("first").unwrap());
        assert!(!t.want("blar").unwrap());
        assert!(t.want("line").unwrap());

        let mut t = Tokenizer::new("single\n\n");
        assert!(!t.at_eof());
        assert!(!t.want_eof().unwrap());
        assert!(t.want("single").unwrap());
        assert!(!t.at_eof());
        assert!(t.want_eof().unwrap());
        assert!(t.at_eof());
    }

    #[test]
    fn comments_and_crlf() {
        let t = Tokenizer::new("a // b c\r\nd");
        assert_eq!(
            t.tokens
                .iter()
                .map(|t| (t.text.as_str(), t.line))
                .collect::<Vec<_>>(),
            vec![("a", 1), ("d", 3)]
        );
    }

    #[test]
    fn numbers() {
        assert_eq!(strtoul_u32("0xFFFFFFFB"), 0xFFFFFFFB);
        assert_eq!(strtoul_u32("12"), 12);
        assert_eq!(strtoul_u32("010"), 8);
        assert_eq!(strtoul_u32("-1"), 0xFFFFFFFF);
        assert_eq!(strtoul_u32("99999999999"), u32::MAX);
        assert_eq!(strtoul_u32("0x"), 0);
        assert_eq!(atoi_positive("523"), 523);
        assert_eq!(atoi_positive("99999999999"), i32::MAX);
        assert_eq!(atof("2.0"), 2.0);
        assert_eq!(atof("x"), 0.0);
    }

    #[test]
    fn parse_simple_message() {
        let text = "{\nTestMessage Low 1 NotTrusted Zerocoded\n// comment \n  {\nTestBlock1      Single\n      {   Test1       U32 }\n  }\n  {\n      NeighborBlock       Multiple        4\n      {   Test0       U32 }\n      {   Test1       U32 }\n      {   Test2       U32 }\n  }\n}";
        let mut t = Tokenizer::new(text);
        let m = parse_message(&mut t).unwrap().unwrap();
        assert_eq!(m.name, "TestMessage");
        assert_eq!(m.frequency, Frequency::Low);
        assert_eq!(m.number, 0xFFFF0001);
        assert_eq!(m.blocks.len(), 2);
        assert_eq!(m.blocks[1].number, 4);
        assert_eq!(m.blocks[1].total_size, 12);
        assert_eq!(m.total_size, 4 + 48);
        assert!(m.block_by_name("Nonexistant").is_none());
    }

    #[test]
    fn parse_errors() {
        let mut t = Tokenizer::new("{ 9bad Low 1 NotTrusted Zerocoded }");
        assert!(parse_message(&mut t).is_err());
        let mut t = Tokenizer::new("{ A Single { X U32 } { X U8 } }");
        assert!(parse_block(&mut t).is_err());
        let mut t = Tokenizer::new("nope 1");
        assert!(parse_template(&mut t).is_err());
    }
}
