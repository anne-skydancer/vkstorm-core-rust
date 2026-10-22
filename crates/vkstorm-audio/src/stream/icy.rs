//! SHOUTcast/Icecast in-band metadata: with `Icy-MetaData: 1` in the
//! request the server answers `icy-metaint: N` and inserts, after every N
//! audio bytes, a length byte L and L*16 bytes of metadata text
//! (`StreamTitle='Artist - Title';StreamUrl='...';`, NUL padded). The
//! stripper removes those blocks from the audio bytes and hands the
//! `StreamTitle` values on.
//!
//! The title is read as VLC's ICY reader did (the metadata path the viewer
//! used): find `StreamTitle=` (case-insensitive); a quoted value runs to the
//! first closing quote that is followed by `;` (so apostrophes inside a
//! title survive), else to the next `;`; an unquoted value runs to the next
//! `;`. An unchanged value is not reported again. The bytes are UTF-8 when
//! valid, else Latin-1 (what FMOD's `rawstr_to_utf8` did with SHOUTcast
//! tags; VLC replaced invalid bytes with `?`).

#![forbid(unsafe_code)]

/// Decodes metadata text: UTF-8 when valid, else Latin-1.
pub(crate) fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    }
}

/// The `StreamTitle` value of a metadata block, if it has one.
pub(crate) fn stream_title(block: &[u8]) -> Option<String> {
    // NUL padding ends the text.
    let end = block.iter().position(|&b| b == 0).unwrap_or(block.len());
    let text = &block[..end];
    const KEY: &[u8] = b"streamtitle=";
    let at = text.windows(KEY.len()).position(|w| w.eq_ignore_ascii_case(KEY))?;
    let rest = &text[at + KEY.len()..];
    let value = match rest.first() {
        Some(&q) if q == b'\'' || q == b'"' => {
            let body = &rest[1..];
            let close = body.windows(2).position(|w| w[0] == q && w[1] == b';');
            match close {
                Some(i) => &body[..i],
                None => match body.iter().position(|&b| b == b';') {
                    Some(i) => &body[..i],
                    None => body,
                },
            }
        }
        _ => match rest.iter().position(|&b| b == b';') {
            Some(i) => &rest[..i],
            None => rest,
        },
    };
    Some(decode_text(value))
}

#[derive(Clone, Copy, Debug)]
enum Phase {
    /// Audio bytes until the next metadata block.
    Audio(usize),
    /// The length byte.
    Length,
    /// Metadata bytes still to come.
    Meta(usize),
}

/// Splits a body with interleaved metadata into audio bytes and titles.
pub(crate) struct IcyStripper {
    metaint: usize,
    phase: Phase,
    block: Vec<u8>,
    last_title: Option<String>,
}

impl IcyStripper {
    pub(crate) fn new(metaint: usize) -> IcyStripper {
        IcyStripper { metaint, phase: Phase::Audio(metaint), block: Vec::new(), last_title: None }
    }

    /// Feeds body bytes; `audio` receives the audio runs, `title` each new
    /// `StreamTitle`. Returns false when `audio` asked to stop.
    pub(crate) fn feed(
        &mut self,
        mut data: &[u8],
        audio: &mut dyn FnMut(&[u8]) -> bool,
        title: &mut dyn FnMut(String),
    ) -> bool {
        while !data.is_empty() {
            match self.phase {
                Phase::Audio(left) => {
                    let n = left.min(data.len());
                    if !audio(&data[..n]) {
                        return false;
                    }
                    data = &data[n..];
                    self.phase = if n == left { Phase::Length } else { Phase::Audio(left - n) };
                }
                Phase::Length => {
                    let len = usize::from(data[0]) * 16;
                    data = &data[1..];
                    self.block.clear();
                    self.phase = if len == 0 { Phase::Audio(self.metaint) } else { Phase::Meta(len) };
                }
                Phase::Meta(left) => {
                    let n = left.min(data.len());
                    self.block.extend_from_slice(&data[..n]);
                    data = &data[n..];
                    if n == left {
                        if let Some(t) = stream_title(&self.block)
                            && self.last_title.as_deref() != Some(t.as_str())
                        {
                            self.last_title = Some(t.clone());
                            title(t);
                        }
                        self.phase = Phase::Audio(self.metaint);
                    } else {
                        self.phase = Phase::Meta(left - n);
                    }
                }
            }
        }
        true
    }
}

/// Splits `Artist - Title` on the first " - " (the plugin's NowPlaying
/// rule); without a separator the whole text is the title.
pub(crate) fn split_now_playing(text: &str) -> (String, String) {
    match text.find(" - ") {
        Some(i) => (text[..i].to_owned(), text[i + 3..].to_owned()),
        None => (String::new(), text.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert_eq!(stream_title(b"StreamTitle='A - B';StreamUrl='';\0\0").as_deref(), Some("A - B"));
        assert_eq!(stream_title(b"StreamTitle='Guns N' Roses - Don't Cry';").as_deref(), Some("Guns N' Roses - Don't Cry"));
        assert_eq!(stream_title(b"streamtitle=\"x\";").as_deref(), Some("x"));
        assert_eq!(stream_title(b"StreamTitle=plain;").as_deref(), Some("plain"));
        assert_eq!(stream_title(b"StreamTitle='unterminated").as_deref(), Some("unterminated"));
        assert_eq!(stream_title(b"StreamTitle='';").as_deref(), Some(""));
        assert_eq!(stream_title(b"StreamUrl='x';"), None);
        assert_eq!(stream_title(b"StreamTitle='Caf\xe9 - Cr\xe8me';").as_deref(), Some("Café - Crème"));
        assert_eq!(stream_title("StreamTitle='Café - Crème';".as_bytes()).as_deref(), Some("Café - Crème"));
    }

    #[test]
    fn split() {
        assert_eq!(split_now_playing("A - B - C"), ("A".into(), "B - C".into()));
        assert_eq!(split_now_playing("Only"), ("".into(), "Only".into()));
    }

    #[test]
    fn strips_across_chunk_boundaries() {
        let metaint = 10;
        let mut body = Vec::new();
        let mut audio_ref = Vec::new();
        let titles = ["StreamTitle='One - 1';", "StreamTitle='One - 1';", "StreamTitle='Two - 2';"];
        for (i, t) in titles.iter().enumerate() {
            let a: Vec<u8> = (0..metaint).map(|k| (i * 10 + k) as u8).collect();
            body.extend_from_slice(&a);
            audio_ref.extend_from_slice(&a);
            let mut m = t.as_bytes().to_vec();
            m.resize(m.len().div_ceil(16) * 16, 0);
            body.push((m.len() / 16) as u8);
            body.extend_from_slice(&m);
        }
        body.extend_from_slice(&[1, 2, 3]);
        audio_ref.extend_from_slice(&[1, 2, 3]);
        for chunk in [1usize, 3, 7, 1000] {
            let mut s = IcyStripper::new(metaint);
            let mut audio = Vec::new();
            let mut got = Vec::new();
            for c in body.chunks(chunk) {
                s.feed(c, &mut |a| { audio.extend_from_slice(a); true }, &mut |t| got.push(t));
            }
            assert_eq!(audio, audio_ref);
            assert_eq!(got, vec!["One - 1".to_owned(), "Two - 2".to_owned()]);
        }
    }
}
