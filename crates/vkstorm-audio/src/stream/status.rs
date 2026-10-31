//! The Icecast `status-json.xsl` side channel, ported from the fork's
//! `LLStreamingAudio_MediaPlugins::pollStreamStatus`/`streamStatusCoro`:
//!
//! - polls only while the stream is active (connecting, buffering or
//!   playing: the viewer's `isPlaying() == 1`);
//! - the first poll 5 s after the start, then every 20 s (the timer runs
//!   from the start and from each poll);
//! - once a poll found the endpoint it is reused; otherwise the candidates
//!   walk the mount path upward (`/listen/station/radio.ogg` ->
//!   `/listen/station/status-json.xsl`, `/listen/status-json.xsl`,
//!   `/status-json.xsl`) for servers behind reverse proxies (AzuraCast);
//! - a candidate succeeds with an HTTP success and a JSON document with
//!   `icestats`; three consecutive polls where every candidate failed stop
//!   the polling for this stream (a failing known endpoint is forgotten and
//!   re-probed first);
//! - the source is the `icestats.source` entry whose `listenurl` ends with
//!   the mount path, or the only one; an empty artist with "Artist - Title"
//!   in the title is split;
//! - results apply only to the stream they were requested for.

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::Shared;
use super::curl::{self, NetConfig};
use super::json::{self, Json};

/// The poll timing (the C++ constants are the defaults).
#[derive(Clone, Copy, Debug)]
pub struct StatusPolicy {
    pub first_poll: Duration,
    pub interval: Duration,
    pub max_failures: u32,
}

impl Default for StatusPolicy {
    fn default() -> Self {
        StatusPolicy {
            first_poll: Duration::from_secs(5),
            interval: Duration::from_secs(20),
            max_failures: 3,
        }
    }
}

/// Splits a URL into (scheme://host[:port], path from the first '/').
fn split_url(url: &str) -> Option<(&str, &str)> {
    let scheme_end = url.find("://")?;
    Some(match url[scheme_end + 3..].find('/') {
        Some(i) => url.split_at(scheme_end + 3 + i),
        None => (url, ""),
    })
}

/// The status document candidates for a stream URL, most specific first.
pub(crate) fn candidates(url: &str) -> Vec<String> {
    let Some((base, mount)) = split_url(url) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut path = mount;
    while let Some(i) = path.rfind('/') {
        path = &path[..i];
        out.push(format!("{base}{path}/status-json.xsl"));
        if path.is_empty() {
            break;
        }
    }
    if out.is_empty() {
        out.push(format!("{base}/status-json.xsl"));
    }
    out
}

/// The (artist, title) of the source for `mount` in a status document;
/// None when no source matches.
pub(crate) fn pick(doc: &Json, mount: &str) -> Option<(String, String)> {
    let sources = doc.get("icestats")?.get("source")?;
    let source = match sources {
        Json::Array(a) => a
            .iter()
            .find(|s| {
                !mount.is_empty()
                    && s.get("listenurl")
                        .map(Json::as_string)
                        .is_some_and(|l| l.ends_with(mount))
            })
            .or(if a.len() == 1 { a.first() } else { None })?,
        Json::Object(_) => sources,
        _ => return None,
    };
    let mut artist = source
        .get("artist")
        .map(Json::as_string)
        .unwrap_or_default();
    let mut title = source.get("title").map(Json::as_string).unwrap_or_default();
    if artist.is_empty()
        && let Some(i) = title.find(" - ")
    {
        artist = title[..i].to_owned();
        title = title[i + 3..].to_owned();
    }
    Some((artist, title))
}

/// Polls for the stream of `generation` until it is replaced or `stop`.
pub(crate) fn run(
    shared: Arc<Shared>,
    generation: u64,
    url: String,
    net: NetConfig,
    policy: StatusPolicy,
    stop: Arc<AtomicBool>,
) {
    let mount = split_url(&url)
        .map(|(_, m)| m.to_owned())
        .unwrap_or_default();
    let gone = || stop.load(Ordering::Relaxed) || shared.generation() != generation;
    while !gone() {
        std::thread::sleep(Duration::from_millis(100));
        if shared.is_playing_code() != 1 {
            continue;
        }
        let list = {
            let mut m = shared.meta();
            if m.generation() != generation {
                return;
            }
            if m.probe_fails >= policy.max_failures {
                return; // no reachable status endpoint: stop asking
            }
            let threshold = if m.poll_done {
                policy.interval
            } else {
                policy.first_poll
            };
            if m.poll_timer.elapsed() < threshold {
                continue;
            }
            m.poll_done = true;
            m.poll_timer = std::time::Instant::now();
            if m.status_url.is_empty() {
                candidates(&url)
            } else {
                vec![m.status_url.clone()]
            }
        };
        let mut found = None;
        for cand in &list {
            if gone() {
                return;
            }
            let ok = curl::get(cand, &net, 1 << 20, Duration::from_secs(15), &gone);
            if let Ok((status, body)) = ok
                && (200..300).contains(&status)
                && let Some(doc) = json::parse(&body)
                && doc.get("icestats").is_some()
            {
                found = Some((cand.clone(), doc));
                break;
            }
        }
        let mut m = shared.meta();
        match found {
            None => m.status_failed(generation),
            Some((status_url, doc)) => {
                if let Some((artist, title)) = pick(&doc, &mount) {
                    m.status_ok(generation, artist, title, &status_url);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_walk() {
        assert_eq!(
            candidates("https://h:8000/listen/station/radio.ogg"),
            vec![
                "https://h:8000/listen/station/status-json.xsl",
                "https://h:8000/listen/status-json.xsl",
                "https://h:8000/status-json.xsl"
            ]
        );
        assert_eq!(
            candidates("http://h:8000"),
            vec!["http://h:8000/status-json.xsl"]
        );
        assert_eq!(candidates("http://h/"), vec!["http://h/status-json.xsl"]);
    }

    #[test]
    fn source_pick() {
        let doc = json::parse(br#"{"icestats":{"source":[{"listenurl":"http://h/a","title":"X - Y"},{"listenurl":"http://h/b","artist":"Z","title":"W"}]}}"#).unwrap();
        assert_eq!(pick(&doc, "/a"), Some(("X".into(), "Y".into())));
        assert_eq!(pick(&doc, "/b"), Some(("Z".into(), "W".into())));
        assert_eq!(pick(&doc, "/c"), None);
        let one = json::parse(br#"{"icestats":{"source":{"title":"Only"}}}"#).unwrap();
        assert_eq!(pick(&one, "/c"), Some(("".into(), "Only".into())));
    }
}
