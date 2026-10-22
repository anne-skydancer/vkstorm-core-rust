//! The stream metadata policy, as the fork's C++ had it across the VLC
//! plugin (`updateStreamMetadata`) and the streaming-audio adapter
//! (`updateMetadata`, `emitMetadata`, `resetMetadata`, `onStreamStatus`):
//!
//! - In-band ICY `StreamTitle` text is split on the first " - " into artist
//!   and title (no separator: all title). In-band tags (Ogg comments, FLAC
//!   comments, ID3v2) give artist and title directly; a title with no
//!   artist that is part of the stream URL is a placeholder and ignored
//!   (VLC put the URL's file name there before real tags arrived).
//! - Empty pairs are ignored; a pair equal to the last in-band pair is not
//!   reported again.
//! - The Icecast status side channel wins once it delivered track data for
//!   the current stream: in-band changes are then no longer emitted.
//! - Every emitted pair differs from the previous one; the viewer receives
//!   it as the LLSD map `{ARTIST, TITLE}` (both keys always present).
//! - Start and stop reset everything and, if anything had been emitted,
//!   tell the viewer the old information no longer applies (an empty map).

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::time::Instant;

use super::PlayerEvent;
use super::codec::InBand;
use super::icy::split_now_playing;

/// Events kept for a viewer that does not poll (oldest dropped first).
const MAX_EVENTS: usize = 64;

pub(crate) struct Meta {
    generation: u64,
    url: String,
    artist: String,
    title: String,
    has_metadata: bool,
    inband: Option<(String, String)>,
    status_valid: bool,
    pub(crate) status_url: String,
    pub(crate) probe_fails: u32,
    pub(crate) poll_done: bool,
    pub(crate) poll_timer: Instant,
    events: VecDeque<PlayerEvent>,
}

impl Meta {
    pub(crate) fn new() -> Meta {
        Meta {
            generation: 0,
            url: String::new(),
            artist: String::new(),
            title: String::new(),
            has_metadata: false,
            inband: None,
            status_valid: false,
            status_url: String::new(),
            probe_fails: 0,
            poll_done: false,
            poll_timer: Instant::now(),
            events: VecDeque::new(),
        }
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// `resetMetadata` (start of a stream, or stop).
    pub(crate) fn reset(&mut self, generation: u64, url: &str) {
        let had = self.has_metadata;
        self.generation = generation;
        self.url = url.to_owned();
        self.artist.clear();
        self.title.clear();
        self.has_metadata = false;
        self.inband = None;
        self.status_valid = false;
        self.status_url.clear();
        self.probe_fails = 0;
        self.poll_done = false;
        self.poll_timer = Instant::now();
        if had {
            self.push(PlayerEvent::MetadataCleared);
        }
    }

    fn push(&mut self, e: PlayerEvent) {
        if self.events.len() >= MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(e);
    }

    pub(crate) fn pop(&mut self) -> Option<PlayerEvent> {
        self.events.pop_front()
    }

    /// The last emitted pair (the viewer's `getCurrentMetadata`); None
    /// when the map is empty.
    pub(crate) fn current(&self) -> Option<(String, String)> {
        self.has_metadata.then(|| (self.artist.clone(), self.title.clone()))
    }

    /// In-band metadata of the stream of `generation`.
    pub(crate) fn in_band(&mut self, generation: u64, m: InBand) {
        if generation != self.generation {
            return;
        }
        let (artist, title) = match m {
            InBand::NowPlaying(text) => split_now_playing(&text),
            InBand::Tags { artist, title } => {
                if artist.is_empty() && !title.is_empty() && self.url.contains(title.as_str()) {
                    return;
                }
                (artist, title)
            }
        };
        if artist.is_empty() && title.is_empty() {
            return;
        }
        if self.inband.as_ref().is_some_and(|(a, t)| *a == artist && *t == title) {
            return;
        }
        self.inband = Some((artist.clone(), title.clone()));
        if !self.status_valid {
            self.emit(artist, title);
        }
    }

    /// `emitMetadata`.
    fn emit(&mut self, artist: String, title: String) {
        if self.has_metadata && artist == self.artist && title == self.title {
            return;
        }
        // The C++ compared against its (initially empty) last pair, so an
        // empty pair is never emitted; callers already filter those.
        if !self.has_metadata && artist.is_empty() && title.is_empty() {
            return;
        }
        self.artist = artist.clone();
        self.title = title.clone();
        self.has_metadata = true;
        self.push(PlayerEvent::Metadata { artist, title });
    }

    /// `onStreamStatus`: a status document was read for `generation`.
    pub(crate) fn status_ok(&mut self, generation: u64, artist: String, title: String, status_url: &str) {
        if generation != self.generation {
            return;
        }
        if self.status_url != status_url {
            self.status_url = status_url.to_owned();
        }
        self.probe_fails = 0;
        if artist.is_empty() && title.is_empty() {
            return;
        }
        self.status_valid = true;
        self.emit(artist, title);
    }

    /// `onStreamStatusFailed`: every candidate failed.
    pub(crate) fn status_failed(&mut self, generation: u64) {
        if generation != self.generation {
            return;
        }
        self.status_url.clear();
        self.probe_fails += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(m: &mut Meta) -> Vec<PlayerEvent> {
        std::iter::from_fn(|| m.pop()).collect()
    }

    fn md(a: &str, t: &str) -> PlayerEvent {
        PlayerEvent::Metadata { artist: a.into(), title: t.into() }
    }

    #[test]
    fn policy() {
        let mut m = Meta::new();
        m.reset(1, "http://h/live/radio.ogg");
        m.in_band(1, InBand::NowPlaying("A - B".into()));
        m.in_band(1, InBand::NowPlaying("A - B".into()));
        m.in_band(1, InBand::Tags { artist: String::new(), title: "radio.ogg".into() });
        m.in_band(1, InBand::Tags { artist: String::new(), title: String::new() });
        m.in_band(0, InBand::NowPlaying("Old - X".into()));
        m.in_band(1, InBand::NowPlaying("Solo".into()));
        assert_eq!(events(&mut m), vec![md("A", "B"), md("", "Solo")]);
        // The side channel takes over.
        m.status_ok(1, "S".into(), "T".into(), "http://h/status-json.xsl");
        m.in_band(1, InBand::NowPlaying("C - D".into()));
        assert_eq!(events(&mut m), vec![md("S", "T")]);
        m.reset(2, "http://h/other");
        assert_eq!(events(&mut m), vec![PlayerEvent::MetadataCleared]);
        m.reset(3, "http://h/other");
        assert_eq!(events(&mut m), vec![]);
    }
}
