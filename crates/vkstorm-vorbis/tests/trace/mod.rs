//! Decode traces in the format of `tools/vorbis-oracle/c/vorbis_oracle.c`:
//! the viewer's `ov_read` loop (`file` modes) and a stream player's packet
//! loop (`packets` modes), with every return value (run-length coded) and
//! an FNV-1a 64 hash of the output. Shared by the crate's parity tests and
//! the `vorbis-oracle` tool (which includes this file by path), so both
//! produce byte-identical traces to the C oracle's.

#![allow(dead_code)]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use vkstorm_vorbis::ogg::{PacketOut, Page, StreamState, SyncState};
use vkstorm_vorbis::{Decoder, OV_HOLE, VorbisFile};

/// FNV-1a 64 with a byte count.
pub struct Fnv {
    pub h: u64,
    pub n: u64,
}

impl Fnv {
    pub fn new() -> Self {
        Self {
            h: 0xcbf2_9ce4_8422_2325,
            n: 0,
        }
    }
    pub fn bytes(&mut self, p: &[u8]) {
        for &b in p {
            self.h ^= b as u64;
            self.h = self.h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.n += p.len() as u64;
    }
}

fn hex(p: &[u8]) -> String {
    let mut s = String::with_capacity(p.len() * 2);
    for b in p {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Run-length coded `<tag> <a> <b> x<count>` lines (`rle_t` in the C).
struct Rle<'a> {
    out: &'a mut String,
    cur: Option<(&'static str, i64, i64)>,
    count: u64,
}

impl<'a> Rle<'a> {
    fn new(out: &'a mut String) -> Self {
        Self {
            out,
            cur: None,
            count: 0,
        }
    }
    fn flush(&mut self) {
        if let Some((t, a, b)) = self.cur
            && self.count > 0
        {
            let _ = writeln!(self.out, "{t} {a} {b} x{}", self.count);
        }
        self.count = 0;
        self.cur = None;
    }
    fn add(&mut self, tag: &'static str, a: i64, b: i64) {
        if self.count > 0 && self.cur == Some((tag, a, b)) {
            self.count += 1;
            return;
        }
        self.flush();
        self.cur = Some((tag, a, b));
        self.count = 1;
    }
    fn line(&mut self, s: &str) {
        self.out.push_str(s);
        self.out.push('\n');
    }
}

/// `ov_read` parameters and the loop policy of a `file` trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileOpts {
    /// Continue after `OV_HOLE` (the viewer stops at any error).
    pub keep_going: bool,
    pub word: usize,
    pub bigendian: bool,
    pub signed: bool,
    /// The buffer length passed to `ov_read` (the viewer's is 4096).
    pub len: usize,
}

impl Default for FileOpts {
    fn default() -> Self {
        Self {
            keep_going: false,
            word: 2,
            bigendian: false,
            signed: true,
            len: 4096,
        }
    }
}

impl FileOpts {
    /// The C oracle's command-line options for these settings.
    pub fn c_args(&self) -> Vec<String> {
        let mut a = Vec::new();
        if self.keep_going {
            a.push("--keep-going".into());
        }
        if self.word != 2 {
            a.push("--word".into());
            a.push(self.word.to_string());
        }
        if self.bigendian {
            a.push("--be".into());
        }
        if !self.signed {
            a.push("--unsigned".into());
        }
        if self.len != 4096 {
            a.push("--len".into());
            a.push(self.len.to_string());
        }
        a
    }
}

/// A trace mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    File(FileOpts),
    Packets(usize),
}

impl Mode {
    /// The modes compared by the harness, by name.
    pub fn all() -> Vec<Mode> {
        [
            "file",
            "file-keep",
            "file-u8",
            "file-be16u-1001",
            "packets-4096",
            "packets-37",
        ]
        .iter()
        .filter_map(|n| Mode::parse(n))
        .collect()
    }

    /// A short stable name (`file`, `file-keep`, `file-u8`,
    /// `file-be16u-1001`, `packets-<chunk>`).
    pub fn name(&self) -> String {
        match self {
            Mode::File(o) if *o == FileOpts::default() => "file".into(),
            Mode::File(o)
                if *o
                    == FileOpts {
                        keep_going: true,
                        ..Default::default()
                    } =>
            {
                "file-keep".into()
            }
            Mode::File(o)
                if *o
                    == FileOpts {
                        keep_going: true,
                        word: 1,
                        signed: false,
                        ..Default::default()
                    } =>
            {
                "file-u8".into()
            }
            Mode::File(o)
                if *o
                    == FileOpts {
                        keep_going: true,
                        bigendian: true,
                        signed: false,
                        len: 1001,
                        ..Default::default()
                    } =>
            {
                "file-be16u-1001".into()
            }
            Mode::File(o) => format!("file{}", o.c_args().join("")),
            Mode::Packets(c) => format!("packets-{c}"),
        }
    }

    /// Inverse of [`Mode::name`] for the named modes.
    pub fn parse(name: &str) -> Option<Mode> {
        Some(match name {
            "file" => Mode::File(FileOpts::default()),
            "file-keep" => Mode::File(FileOpts {
                keep_going: true,
                ..Default::default()
            }),
            "file-u8" => Mode::File(FileOpts {
                keep_going: true,
                word: 1,
                signed: false,
                ..Default::default()
            }),
            "file-be16u-1001" => Mode::File(FileOpts {
                keep_going: true,
                bigendian: true,
                signed: false,
                len: 1001,
                ..Default::default()
            }),
            p => Mode::Packets(p.strip_prefix("packets-")?.parse().ok()?),
        })
    }

    /// The C oracle's arguments after the input and trace paths.
    pub fn c_command(&self) -> (&'static str, Vec<String>) {
        match self {
            Mode::File(o) => ("file", o.c_args()),
            Mode::Packets(c) => ("packets", vec!["--chunk".into(), c.to_string()]),
        }
    }

    /// The Rust trace of `data` in this mode.
    pub fn trace(&self, data: &[u8]) -> String {
        match self {
            Mode::File(o) => run_file(data, o, None),
            Mode::Packets(c) => run_packets(data, *c),
        }
    }
}

fn trace_link(o: &mut String, vf: &VorbisFile, link: i32) {
    let _ = writeln!(o, "pcm_total {link} {}", vf.pcm_total(link));
    match vf.info(link) {
        Some(vi) => {
            let _ = writeln!(
                o,
                "info {link} {} {} {} {} {} {}",
                vi.version,
                vi.channels,
                vi.rate,
                vi.bitrate_upper,
                vi.bitrate_nominal,
                vi.bitrate_lower
            );
        }
        None => {
            let _ = writeln!(o, "info {link} none");
        }
    }
    let _ = writeln!(o, "serial {link} {}", vf.serialnumber(link));
    match vf.comment(link) {
        Some(vc) => {
            match &vc.vendor {
                Some(v) => {
                    // the C prints the vendor as a C string
                    let end = v.iter().position(|&b| b == 0).unwrap_or(v.len());
                    let _ = writeln!(o, "vendor {link} {}", hex(&v[..end]));
                }
                None => {
                    let _ = writeln!(o, "vendor {link} none");
                }
            }
            let _ = writeln!(o, "comments {link} {}", vc.user_comments.len());
            for (i, c) in vc.user_comments.iter().enumerate() {
                let _ = writeln!(o, "comment {link} {i} {}", hex(c));
            }
        }
        None => {
            let _ = writeln!(o, "vendor {link} none\ncomments {link} 0");
        }
    }
}

/// The viewer's decode (`run_file` in the C oracle); the PCM is appended
/// to `pcm` when given.
pub fn run_file(data: &[u8], opt: &FileOpts, mut pcm: Option<&mut Vec<u8>>) -> String {
    let mut o = String::new();
    let mut vf = match VorbisFile::open(data) {
        Ok(vf) => {
            o.push_str("open 0\n");
            vf
        }
        Err(e) => {
            let _ = writeln!(o, "open {}", e.code());
            return o;
        }
    };
    let _ = writeln!(o, "streams {}", vf.streams());
    trace_link(&mut o, &vf, -1);
    for link in 0..vf.streams() {
        trace_link(&mut o, &vf, link);
    }
    let mut h = Fnv::new();
    let mut buf = vec![0u8; opt.len.min(65536)];
    let mut section = 0i32;
    let mut calls = 0u64;
    {
        let mut rl = Rle::new(&mut o);
        loop {
            let ret = vf.read(&mut buf, opt.bigendian, opt.word, opt.signed, &mut section);
            calls += 1;
            rl.add("read", ret as i64, section as i64);
            if ret > 0 {
                h.bytes(&buf[..ret as usize]);
                if let Some(p) = pcm.as_deref_mut() {
                    p.extend_from_slice(&buf[..ret as usize]);
                }
                continue;
            }
            if ret == 0 || !opt.keep_going || ret != OV_HOLE as isize || calls > 10_000_000 {
                break;
            }
        }
        rl.flush();
    }
    let _ = writeln!(o, "pcm_tell {}", vf.pcm_tell());
    let _ = writeln!(o, "pcm {} {:016x}", h.n, h.h);
    o
}

/// The packet API (`run_packets` in the C oracle): `chunk`-byte reads, a
/// new decoder at every BOS page.
pub fn run_packets(data: &[u8], chunk: usize) -> String {
    // debugging aid: VORBIS_ORACLE_DUMP=<file> receives the raw samples
    let mut dump: Option<Vec<u8>> = std::env::var_os("VORBIS_ORACLE_DUMP").map(|_| Vec::new());
    let mut o = String::new();
    let mut oy = SyncState::new();
    let mut os: Option<StreamState> = None;
    let mut dec = Decoder::new();
    let mut state = 0;
    let mut nheaders = 0;
    let mut pos = 0usize;
    let mut h = Fnv::new();
    let mut samples: u64 = 0;
    let mut page = Page::new();
    let chunk = chunk.max(1);
    {
        let mut rl = Rle::new(&mut o);
        loop {
            if pos < data.len() {
                let n = (data.len() - pos).min(chunk);
                oy.write(&data[pos..pos + n]);
                pos += n;
            }
            loop {
                let r = oy.pageout(&mut page);
                if r == 0 {
                    break;
                }
                if r < 0 {
                    rl.add("sync", r as i64, 0);
                    continue;
                }
                if page.bos() {
                    rl.flush();
                    rl.line(&format!("bos {}", page.serialno()));
                    os = Some(StreamState::new(page.serialno()));
                    dec = Decoder::new();
                    state = 1;
                    nheaders = 0;
                }
                let Some(st) = os.as_mut() else { continue };
                if st.pagein(&page) < 0 {
                    continue;
                }
                loop {
                    let p = match st.packetout() {
                        PacketOut::None => break,
                        PacketOut::Hole => {
                            rl.add("hole", 0, 0);
                            continue;
                        }
                        PacketOut::Packet(p) => p,
                    };
                    if state == 1 {
                        let hr = match dec.headerin_packet(&p) {
                            Ok(()) => 0,
                            Err(e) => e.code(),
                        };
                        rl.add("hdr", hr as i64, 0);
                        if hr < 0 {
                            state = 3;
                            continue;
                        }
                        nheaders += 1;
                        if nheaders == 3 {
                            rl.flush();
                            if dec.synthesis_init().is_err() {
                                rl.line("init fail");
                                state = 3;
                            } else {
                                rl.line(&format!(
                                    "init {} {}",
                                    dec.info().channels,
                                    dec.info().rate
                                ));
                                state = 2;
                            }
                        }
                        continue;
                    }
                    if state == 2 {
                        let s = match dec.synthesis(&p) {
                            Ok(()) => 0,
                            Err(e) => e.code(),
                        };
                        let mut total: i64 = 0;
                        loop {
                            let out = dec.pcmout();
                            let n = out.len();
                            if n == 0 {
                                break;
                            }
                            let ch = out.channels();
                            for i in 0..n {
                                for c in 0..ch {
                                    // NaN samples hash as one canonical NaN: their
                                    // sign bit is not reproducible (crate docs)
                                    let v = out.channel(c)[i];
                                    let v = if v.is_nan() {
                                        f32::from_bits(0x7fc0_0000)
                                    } else {
                                        v
                                    };
                                    h.bytes(&v.to_le_bytes());
                                    if let Some(d) = dump.as_mut() {
                                        d.extend_from_slice(&out.channel(c)[i].to_le_bytes());
                                    }
                                }
                            }
                            let _ = dec.read(n);
                            total += n as i64;
                        }
                        samples += total as u64;
                        rl.add("pkt", s as i64, total);
                    }
                }
            }
            if pos >= data.len() {
                break;
            }
        }
        rl.flush();
    }
    let _ = writeln!(o, "samples {} {:016x}", samples, h.h);
    if let (Some(d), Some(path)) = (dump, std::env::var_os("VORBIS_ORACLE_DUMP")) {
        let _ = fs::write(path, d);
    }
    o
}

/// The expected-trace file of the committed fixtures for this platform:
/// the Windows (MSVC) and Linux (glibc) C libraries decode differently in
/// the last bit (see the crate docs), so each has its own.
pub fn platform_expected_name() -> Option<&'static str> {
    if cfg!(all(windows, target_env = "msvc", target_arch = "x86_64")) {
        Some("expected-windows-msvc.txt")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("expected-linux-x86_64.txt")
    } else {
        None
    }
}

/// Parses an expected-trace file: `== <file> <mode>` headers, each
/// followed by its trace lines.
pub fn parse_expected(text: &str) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = Vec::new();
    for line in text.lines() {
        if let Some(h) = line.strip_prefix("== ") {
            let mut it = h.rsplitn(2, ' ');
            let mode = it.next().unwrap_or_default().to_string();
            let file = it.next().unwrap_or_default().to_string();
            out.push((file, mode, String::new()));
        } else if let Some(last) = out.last_mut() {
            last.2.push_str(line);
            last.2.push('\n');
        }
    }
    out
}

// ------------------------------------------------------------------
// comparison with the C oracle

/// Every `.ogg` under `dir`, sorted.
pub fn collect_ogg(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect_ogg(&p, out);
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ogg")) {
            out.push(p);
        }
    }
}

/// Runs the C oracle on `file` in `mode`; `Err` when it fails (a crash of
/// the C library on a malicious input).
pub fn c_trace(
    oracle: &str,
    file: &Path,
    mode: &Mode,
    trace_path: &Path,
) -> Result<String, String> {
    let _ = fs::remove_file(trace_path);
    let (cmd, args) = mode.c_command();
    let status = Command::new(oracle)
        .arg(cmd)
        .arg(file)
        .arg(trace_path)
        .args(args)
        .status();
    match status {
        Ok(s) if s.success() => fs::read_to_string(trace_path).map_err(|e| e.to_string()),
        Ok(s) => Err(format!("c-failed({s})")),
        Err(e) => Err(format!("c-error({e})")),
    }
}

/// The first differing line of two traces.
pub fn first_diff(a: &str, b: &str) -> String {
    for (i, (x, y)) in a.lines().zip(b.lines()).enumerate() {
        if x != y {
            let cut = |s: &str| {
                if s.len() > 160 {
                    format!("{}...", &s[..160])
                } else {
                    s.to_string()
                }
            };
            return format!("line {}: C `{}` / Rust `{}`", i + 1, cut(x), cut(y));
        }
    }
    format!(
        "length: C {} lines / Rust {} lines",
        a.lines().count(),
        b.lines().count()
    )
}

pub type Verdicts = Vec<(String, String, String)>;

/// Compares the C oracle with the port over a corpus (see the tool's docs);
/// prints every difference and a summary, `Ok(true)` when all compared
/// traces are identical.
pub fn run_compare(
    oracle: &str,
    corpus: &str,
    work: &str,
    jobs: usize,
    quick: bool,
) -> Result<bool, String> {
    let mut files = Vec::new();
    collect_ogg(Path::new(corpus), &mut files);
    if files.is_empty() {
        return Err(format!("no .ogg files under {corpus}"));
    }
    fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let mut modes = Mode::all();
    if quick {
        modes.truncate(3);
    }
    let tasks: Vec<(PathBuf, Mode)> = files
        .iter()
        .flat_map(|f| modes.iter().map(move |m| (f.clone(), m.clone())))
        .collect();
    let next = Arc::new(AtomicUsize::new(0));
    let results: Arc<Mutex<Verdicts>> = Arc::new(Mutex::new(Vec::new()));
    let tasks = Arc::new(tasks);
    let mut handles = Vec::new();
    for t in 0..jobs.max(1) {
        let next = next.clone();
        let results = results.clone();
        let tasks = tasks.clone();
        let oracle = oracle.to_string();
        let work = PathBuf::from(work);
        handles.push(std::thread::spawn(move || {
            loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= tasks.len() {
                    break;
                }
                let (file, mode) = &tasks[i];
                let c = c_trace(&oracle, file, mode, &work.join(format!("t{t}.trace")));
                let name = format!("{} [{}]", file.display(), mode.name());
                let data = fs::read(file).unwrap_or_default();
                let rust = std::panic::catch_unwind(|| mode.trace(&data));
                let verdict = match (c, rust) {
                    (_, Err(_)) => ("rust-panic".to_string(), String::new()),
                    (Ok(c), Ok(r)) => {
                        if c == r {
                            ("same".into(), String::new())
                        } else {
                            ("DIFF".into(), first_diff(&c, &r))
                        }
                    }
                    (Err(e), Ok(_)) => (e, String::new()),
                };
                if let Ok(mut r) = results.lock() {
                    r.push((name, verdict.0, verdict.1));
                }
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    let mut results = results.lock().map_err(|_| "poisoned".to_string())?.clone();
    results.sort();
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    for (name, verdict, detail) in &results {
        let key = if verdict.starts_with("c-") {
            "C crashed or failed (not compared)".to_string()
        } else {
            verdict.clone()
        };
        *counts.entry(key).or_default() += 1;
        if verdict != "same" {
            println!("{verdict} {name} {detail}");
        }
    }
    println!(
        "files {} modes {} comparisons {}",
        files.len(),
        modes.len(),
        results.len()
    );
    for (k, v) in &counts {
        println!("  {k}: {v}");
    }
    Ok(results
        .iter()
        .all(|r| r.1 == "same" || r.1.starts_with("c-")))
}
