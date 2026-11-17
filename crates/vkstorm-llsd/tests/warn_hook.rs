// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026, Anne Skydancer (Vulkanstorm)
//! The warning the node budget raises: once per failing parse, none for a
//! parse within budget. One test in its own binary: the hook is process-wide.

use std::sync::Arc;
use std::sync::Mutex;

use vkstorm_llsd::{Format, Limits, PARSE_FAILURE, parse, set_warn_hook};

#[test]
fn the_warning_fires_once_per_failing_parse() {
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = log.clone();
    set_warn_hook(Box::new(move |m| sink.lock().unwrap().push(m.to_string())));

    let limits = Limits {
        max_bytes: None,
        max_depth: -1,
        max_nodes: 10,
    };
    // Within budget: no warning.
    assert!(parse(Format::Notation, b"[!,!,!]", limits).0 > 0);
    assert!(log.lock().unwrap().is_empty());
    // Over budget, in each format: one warning per parse.
    let docs: [(Format, &[u8]); 3] = [
        (Format::Notation, b"[!,!,!,!,!,!,!,!,!,!,!,!,!,!,!]"),
        (Format::Xml, b"<llsd><array><undef/><undef/><undef/><undef/><undef/><undef/><undef/><undef/><undef/><undef/><undef/><undef/></array></llsd>"),
        (Format::Binary, b"[\x00\x00\x00\x0c!!!!!!!!!!!!]"),
    ];
    for (i, (format, doc)) in docs.iter().enumerate() {
        assert_eq!(parse(*format, doc, limits).0, PARSE_FAILURE);
        let log = log.lock().unwrap();
        assert_eq!(log.len(), i + 1, "{format:?}");
        assert!(log[i].contains("more than 10"), "{}", log[i]);
    }
}
