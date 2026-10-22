//! Playlist resolution: M3U/M3U8 as plain URL lists and PLS (`FileN=`), the
//! formats parcel music URLs point at besides the streams themselves. The
//! entries are tried in order by the session.
//!
//! HLS (an M3U8 with `#EXT-X-` segment or variant tags) is not a URL list:
//! VLC played it, but parcel streams practically never use it, and it needs
//! a segment scheduler and an MPEG-TS/fMP4 demuxer; it is rejected with a
//! clear reason. XSPF and ASX are not supported either.

#![forbid(unsafe_code)]

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Playlist {
    Entries(Vec<String>),
    Hls,
}

/// Parses a playlist body (`pls` selects the PLS syntax; otherwise M3U).
pub(crate) fn parse(body: &[u8], pls: bool, base: &str) -> Playlist {
    let text = super::icy::decode_text(body);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    if pls { parse_pls(text, base) } else { parse_m3u(text, base) }
}

fn parse_m3u(text: &str, base: &str) -> Playlist {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(tag) = line.strip_prefix('#') {
            let tag = tag.to_ascii_uppercase();
            // Segment and variant playlists (RFC 8216).
            if tag.starts_with("EXT-X-TARGETDURATION")
                || tag.starts_with("EXT-X-MEDIA-SEQUENCE")
                || tag.starts_with("EXT-X-STREAM-INF")
                || tag.starts_with("EXT-X-KEY")
                || tag.starts_with("EXT-X-MAP")
                || tag.starts_with("EXT-X-PLAYLIST-TYPE")
            {
                return Playlist::Hls;
            }
            continue;
        }
        out.push(resolve(base, line));
    }
    Playlist::Entries(out)
}

fn parse_pls(text: &str, base: &str) -> Playlist {
    let mut files: Vec<(u32, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        if key.len() > 4
            && key[..4].eq_ignore_ascii_case("file")
            && let Ok(n) = key[4..].parse::<u32>()
        {
            let v = value.trim();
            if !v.is_empty() {
                files.push((n, resolve(base, v)));
            }
        }
    }
    // Stable: equal numbers keep their order in the file.
    files.sort_by_key(|(n, _)| *n);
    Playlist::Entries(files.into_iter().map(|(_, u)| u).collect())
}

/// Resolves a playlist entry against the playlist's URL.
pub(crate) fn resolve(base: &str, entry: &str) -> String {
    if entry.contains("://") {
        return entry.to_owned();
    }
    let Some(scheme_end) = base.find("://") else { return entry.to_owned() };
    let scheme = &base[..scheme_end];
    let after = &base[scheme_end + 3..];
    let host_end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let origin = &base[..scheme_end + 3 + host_end];
    if let Some(rest) = entry.strip_prefix("//") {
        return format!("{scheme}://{rest}");
    }
    if entry.starts_with('/') {
        return format!("{origin}{entry}");
    }
    let path = &after[host_end..];
    let path = &path[..path.find(['?', '#']).unwrap_or(path.len())];
    let dir = match path.rfind('/') {
        Some(i) => &path[..=i],
        None => "/",
    };
    format!("{origin}{dir}{entry}")
}

/// The content types that announce a playlist (and whether it is PLS).
pub(crate) fn content_type_kind(ct: &str) -> Option<bool> {
    match ct {
        "audio/x-scpls" | "audio/scpls" | "application/pls+xml" | "application/pls" => Some(true),
        "audio/x-mpegurl" | "audio/mpegurl" | "application/x-mpegurl" | "application/vnd.apple.mpegurl"
        | "application/mpegurl" | "audio/m3u" | "audio/x-m3u" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pls() {
        let body = b"[playlist]\nNumberOfEntries=2\nFile2=http://b/2\nTitle1=x\nfile1=http://a/1\nLength1=-1\nVersion=2\n";
        assert_eq!(parse(body, true, "http://h/p.pls"), Playlist::Entries(vec!["http://a/1".into(), "http://b/2".into()]));
    }

    #[test]
    fn m3u() {
        let body = b"#EXTM3U\r\n#EXTINF:-1,Radio\r\nhttp://a:8000/live\r\n\r\nrel.mp3\r\n/abs.ogg\r\n";
        assert_eq!(
            parse(body, false, "https://h:1/dir/list.m3u?x=1"),
            Playlist::Entries(vec!["http://a:8000/live".into(), "https://h:1/dir/rel.mp3".into(), "https://h:1/abs.ogg".into()])
        );
        assert_eq!(parse(b"#EXTM3U\n#EXT-X-TARGETDURATION:10\nseg0.ts\n", false, "http://h/"), Playlist::Hls);
        assert_eq!(parse(b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1\nv.m3u8\n", false, "http://h/"), Playlist::Hls);
    }

    #[test]
    fn resolution() {
        assert_eq!(resolve("http://h", "x"), "http://h/x");
        assert_eq!(resolve("http://h/a/b", "//o/p"), "http://o/p");
        assert_eq!(resolve("http://h/a/b?q=/z", "c"), "http://h/a/c");
    }
}
