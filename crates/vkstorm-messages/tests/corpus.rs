//! Parity with the viewer's C++ message system over the recorded corpus
//! (`tests/fixtures/llmessage_corpus.vksm`, recorded by the viewer's
//! `INTEGRATION_TEST_llmessagetemplate` from the C++ implementation before
//! the port): the parsed template, every message packed at four sizes with
//! its zero-coded form, every packet read back complete, cut and corrupted,
//! zero-code expansion of coded and malformed packets, and the data packer
//! scripts in the binary, text buffer and text file forms.
//!
//! The fixture carries the stated product behaviour where the C++ read
//! past its buffers (see the crate documentation); those entries are
//! flagged and the test checks them against the statement.

use std::collections::HashMap;
use std::sync::Arc;
use vkstorm_messages::datapacker::{self, AsciiBuffer, BinaryBuffer, TextKind};
use vkstorm_messages::reader::decode_number;
use vkstorm_messages::{
    BlockType, Builder, MAX_BUFFER_SIZE, Reader, Template, Tokenizer, VarType, parse_template,
    zerocode,
};

const MAGIC: u32 = 0x4D534B56;
const VERSION: u32 = 1;

struct R<'a> {
    b: &'a [u8],
    p: usize,
}

impl R<'_> {
    fn u8(&mut self) -> u8 {
        let v = self.b[self.p];
        self.p += 1;
        v
    }
    fn u32(&mut self) -> u32 {
        let v = u32::from_le_bytes(self.b[self.p..self.p + 4].try_into().unwrap());
        self.p += 4;
        v
    }
    fn i32(&mut self) -> i32 {
        self.u32() as i32
    }
    fn f32(&mut self) -> f32 {
        f32::from_bits(self.u32())
    }
    fn bytes(&mut self) -> Vec<u8> {
        let n = self.u32() as usize;
        let v = self.b[self.p..self.p + n].to_vec();
        self.p += n;
        v
    }
    fn str(&mut self) -> String {
        String::from_utf8(self.bytes()).unwrap()
    }
}

struct VarDump {
    name: String,
    ty: i32,
    size: i32,
}
struct BlockDump {
    name: String,
    ty: i32,
    number: i32,
    total_size: i32,
    vars: Vec<VarDump>,
}
struct MessageDump {
    name: String,
    number: u32,
    frequency: i32,
    trust: i32,
    encoding: i32,
    deprecation: i32,
    total_size: i32,
    blocks: Vec<BlockDump>,
}
struct VarInput {
    data: Vec<u8>,
    as_string: bool,
}
struct BuildCase {
    message: String,
    variant: u8,
    offset: u8,
    repeats: Vec<i32>,
    inputs: Vec<Vec<VarInput>>,
    packet: Vec<u8>,
    send_total: i32,
    full: bool,
    coded: Vec<u8>,
}
struct VarRead {
    raw: Vec<u8>,
    stated: bool,
}
struct ReadCase {
    message: String,
    kind: String,
    packet: Vec<u8>,
    valid: bool,
    read: bool,
    ran_off_end: i32,
    message_size: i32,
    counts: Vec<i32>,
    vars: Vec<Vec<VarRead>>,
}
struct ExpandCase {
    kind: String,
    input: Vec<u8>,
    result: i32,
    wrote_past: i32,
    output: Vec<u8>,
}
struct DpOp {
    kind: u8,
    name: String,
    value: Vec<u8>,
    size: i32,
}
struct DpOpResult {
    ok: bool,
    cursor: i32,
    value: Vec<u8>,
    /// The stated behaviour rather than the oracle (informational).
    _stated: bool,
}
struct DpCase {
    kind: String,
    ops: Vec<DpOp>,
    buffer_size: i32,
    packed: Vec<u8>,
    pack_results: Vec<DpOpResult>,
    measured: i32,
    unpack_size: i32,
    unpack_results: Vec<DpOpResult>,
}
struct Corpus {
    version: f32,
    dump: Vec<MessageDump>,
    builds: Vec<BuildCase>,
    reads: Vec<ReadCase>,
    expands: Vec<ExpandCase>,
    packers: Vec<DpCase>,
}

fn read_result(r: &mut R) -> DpOpResult {
    let ok = r.u8() != 0;
    let cursor = r.i32();
    let value = r.bytes();
    let stated = r.u8() != 0;
    DpOpResult {
        ok,
        cursor,
        value,
        _stated: stated,
    }
}

fn load() -> Corpus {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/llmessage_corpus.vksm"
    );
    let b = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut r = R { b: &b, p: 0 };
    assert_eq!(r.u32(), MAGIC);
    assert_eq!(r.u32(), VERSION);
    let version = r.f32();
    let mut dump = Vec::new();
    for _ in 0..r.u32() {
        let name = r.str();
        let number = r.u32();
        let frequency = r.i32();
        let trust = r.i32();
        let encoding = r.i32();
        let deprecation = r.i32();
        let total_size = r.i32();
        let mut blocks = Vec::new();
        for _ in 0..r.u32() {
            let name = r.str();
            let ty = r.i32();
            let number = r.i32();
            let total_size = r.i32();
            let mut vars = Vec::new();
            for _ in 0..r.u32() {
                let name = r.str();
                let ty = r.i32();
                let size = r.i32();
                vars.push(VarDump { name, ty, size });
            }
            blocks.push(BlockDump {
                name,
                ty,
                number,
                total_size,
                vars,
            });
        }
        dump.push(MessageDump {
            name,
            number,
            frequency,
            trust,
            encoding,
            deprecation,
            total_size,
            blocks,
        });
    }
    let mut builds = Vec::new();
    for _ in 0..r.u32() {
        let message = r.str();
        let variant = r.u8();
        let offset = r.u8();
        let mut repeats = Vec::new();
        let mut inputs = Vec::new();
        for _ in 0..r.u32() {
            repeats.push(r.i32());
            let mut v = Vec::new();
            for _ in 0..r.u32() {
                let data = r.bytes();
                let as_string = r.u8() != 0;
                v.push(VarInput { data, as_string });
            }
            inputs.push(v);
        }
        let packet = r.bytes();
        let send_total = r.i32();
        let full = r.u8() != 0;
        let coded = r.bytes();
        builds.push(BuildCase {
            message,
            variant,
            offset,
            repeats,
            inputs,
            packet,
            send_total,
            full,
            coded,
        });
    }
    let mut reads = Vec::new();
    for _ in 0..r.u32() {
        let message = r.str();
        let kind = r.str();
        let packet = r.bytes();
        let valid = r.u8() != 0;
        let read = r.u8() != 0;
        let ran_off_end = r.i32();
        let message_size = r.i32();
        let mut counts = Vec::new();
        let mut vars = Vec::new();
        for _ in 0..r.u32() {
            counts.push(r.i32());
            let mut v = Vec::new();
            for _ in 0..r.u32() {
                let raw = r.bytes();
                let _typed = r.bytes();
                let stated = r.u8() != 0;
                v.push(VarRead { raw, stated });
            }
            vars.push(v);
        }
        reads.push(ReadCase {
            message,
            kind,
            packet,
            valid,
            read,
            ran_off_end,
            message_size,
            counts,
            vars,
        });
    }
    let mut expands = Vec::new();
    for _ in 0..r.u32() {
        let kind = r.str();
        let input = r.bytes();
        let result = r.i32();
        let wrote_past = r.i32();
        let output = r.bytes();
        expands.push(ExpandCase {
            kind,
            input,
            result,
            wrote_past,
            output,
        });
    }
    let mut packers = Vec::new();
    for _ in 0..r.u32() {
        let kind = r.str();
        let mut ops = Vec::new();
        for _ in 0..r.u32() {
            let kind = r.u8();
            let name = r.str();
            let value = r.bytes();
            let size = r.i32();
            ops.push(DpOp {
                kind,
                name,
                value,
                size,
            });
        }
        let buffer_size = r.i32();
        let packed = r.bytes();
        let mut pack_results = Vec::new();
        for _ in 0..r.u32() {
            pack_results.push(read_result(&mut r));
        }
        let measured = r.i32();
        let unpack_size = r.i32();
        let mut unpack_results = Vec::new();
        for _ in 0..r.u32() {
            unpack_results.push(read_result(&mut r));
        }
        packers.push(DpCase {
            kind,
            ops,
            buffer_size,
            packed,
            pack_results,
            measured,
            unpack_size,
            unpack_results,
        });
    }
    assert_eq!(r.p, b.len(), "fixture read completely");
    Corpus {
        version,
        dump,
        builds,
        reads,
        expands,
        packers,
    }
}

type Templates = (
    f32,
    Vec<Arc<Template>>,
    HashMap<String, Arc<Template>>,
    HashMap<u32, Arc<Template>>,
);

fn templates() -> Templates {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/message_template.msg"
    ))
    .unwrap();
    let file = parse_template(&mut Tokenizer::new(&text)).unwrap();
    let list: Vec<Arc<Template>> = file
        .messages
        .into_iter()
        .map(|m| Arc::new(m.with_index_keys()))
        .collect();
    let by_name = list.iter().map(|t| (t.name.clone(), t.clone())).collect();
    let by_number = list.iter().map(|t| (t.number, t.clone())).collect();
    (file.version, list, by_name, by_number)
}

struct Mismatches(Vec<String>, usize);

impl Mismatches {
    fn add(&mut self, s: String) {
        self.1 += 1;
        if self.0.len() < 40 {
            self.0.push(s);
        }
    }
    fn eq<T: PartialEq + std::fmt::Debug>(&mut self, what: &str, expected: T, actual: T) {
        if expected != actual {
            self.add(format!("{what}: expected {expected:?} got {actual:?}"));
        }
    }
    fn bytes(&mut self, what: &str, expected: &[u8], actual: &[u8]) {
        if expected != actual {
            let first = expected.iter().zip(actual).position(|(a, b)| a != b);
            self.add(format!(
                "{what}: expected {} bytes got {}, first difference at {:?}",
                expected.len(),
                actual.len(),
                first
            ));
        }
    }
    fn report(self, what: &str) {
        assert!(
            self.1 == 0,
            "{what}: {} mismatches\n  {}",
            self.1,
            self.0.join("\n  ")
        );
    }
}

#[test]
fn template_parser_parity() {
    let c = load();
    let (version, list, _, _) = templates();
    let mut m = Mismatches(Vec::new(), 0);
    m.eq("version", c.version, version);
    m.eq("message count", c.dump.len(), list.len());
    for (d, t) in c.dump.iter().zip(&list) {
        m.eq("name", &d.name, &t.name);
        m.eq(&format!("{} number", d.name), d.number, t.number);
        m.eq(
            &format!("{} frequency", d.name),
            d.frequency,
            t.frequency as i32,
        );
        m.eq(&format!("{} trust", d.name), d.trust, t.trust as i32);
        m.eq(
            &format!("{} encoding", d.name),
            d.encoding,
            t.encoding as i32,
        );
        m.eq(
            &format!("{} deprecation", d.name),
            d.deprecation,
            t.deprecation as i32,
        );
        m.eq(
            &format!("{} total size", d.name),
            d.total_size,
            t.total_size,
        );
        m.eq(
            &format!("{} blocks", d.name),
            d.blocks.len(),
            t.blocks.len(),
        );
        for (bd, b) in d.blocks.iter().zip(&t.blocks) {
            let tag = format!("{}.{}", d.name, bd.name);
            m.eq(&format!("{tag} name"), &bd.name, &b.name);
            m.eq(&format!("{tag} type"), bd.ty, b.ty as i32);
            m.eq(&format!("{tag} number"), bd.number, b.number);
            m.eq(&format!("{tag} total size"), bd.total_size, b.total_size);
            m.eq(
                &format!("{tag} variables"),
                bd.vars.len(),
                b.variables.len(),
            );
            for (vd, v) in bd.vars.iter().zip(&b.variables) {
                m.eq(&format!("{tag}.{} name", vd.name), &vd.name, &v.name);
                m.eq(&format!("{tag}.{} type", vd.name), vd.ty, v.ty as i32);
                m.eq(&format!("{tag}.{} size", vd.name), vd.size, v.size);
            }
        }
    }
    m.report("template parser");
}

#[test]
fn builder_parity() {
    let c = load();
    let (_, _, by_name, _) = templates();
    let mut m = Mismatches(Vec::new(), 0);
    let mut builder = Builder::new();
    let mut buffer = vec![0u8; MAX_BUFFER_SIZE];
    let mut coded = vec![0u8; 2 * MAX_BUFFER_SIZE];
    for bc in &c.builds {
        let tag = format!("{}/{}", bc.message, bc.variant);
        let t = by_name[&bc.message].clone();
        builder.new_message(t.clone());
        for (bi, block) in t.blocks.iter().enumerate() {
            let mut k = 0;
            for _ in 0..bc.repeats[bi] {
                builder.next_block(block.key).unwrap();
                for var in &block.variables {
                    let input = &bc.inputs[bi][k];
                    k += 1;
                    let ty = if input.as_string {
                        VarType::Variable
                    } else {
                        VarType::Fixed
                    };
                    builder.add_data(var.key, &input.data, ty).unwrap();
                }
            }
        }
        buffer.fill(0);
        let n = builder.build_message(&mut buffer, bc.offset).unwrap();
        m.bytes(&format!("{tag} packet"), &bc.packet, &buffer[..n]);
        m.eq(
            &format!("{tag} send total"),
            bc.send_total,
            builder.message_size(),
        );
        m.eq(
            &format!("{tag} full"),
            bc.full,
            builder.is_message_full(None).unwrap(),
        );
        assert!(builder.is_built());
        let coded_len = if t.encoding == vkstorm_messages::Encoding::ZeroCoded {
            zerocode::zero_code(&buffer[..n], &mut coded).unwrap_or(0)
        } else {
            0
        };
        m.bytes(&format!("{tag} coded"), &bc.coded, &coded[..coded_len]);
    }
    m.report("builder");
}

#[test]
fn reader_parity() {
    let c = load();
    let (_, _, _, by_number) = templates();
    let mut m = Mismatches(Vec::new(), 0);
    let mut reader = Reader::new();
    let mut stated = 0usize;
    for (i, rc) in c.reads.iter().enumerate() {
        let tag = format!("{}/{}/{}#{i}", rc.message, rc.kind, rc.packet.len());
        reader.clear();
        let number = decode_number(&rc.packet).ok();
        // validateMessage: the number must be registered and the message
        // not UDP-blacklisted (the ban flags are the viewer's, all clear here)
        let t = number
            .and_then(|n| by_number.get(&n).cloned())
            .filter(|t| t.deprecation != vkstorm_messages::Deprecation::UdpBlacklisted);
        m.eq(&format!("{tag} valid"), rc.valid, t.is_some());
        let Some(t) = t else { continue };
        if !rc.valid {
            continue;
        }
        m.eq(&format!("{tag} name"), &rc.message, &t.name);
        reader.set_message(t.clone(), rc.packet.len() as i32);
        m.eq(
            &format!("{tag} message size"),
            rc.message_size,
            reader.message_size(),
        );
        let out = reader.decode_data(&rc.packet);
        m.eq(&format!("{tag} read"), rc.read, out.has_blocks);
        m.eq(
            &format!("{tag} ran off end"),
            rc.ran_off_end,
            out.ran_off_end.len() as i32,
        );
        for (bi, block) in t.blocks.iter().enumerate() {
            let count = reader.number_of_blocks(block.key).unwrap();
            m.eq(&format!("{tag} {} count", block.name), rc.counts[bi], count);
            if count != rc.counts[bi] {
                continue;
            }
            let mut k = 0;
            for r in 0..count {
                for var in &block.variables {
                    let vr = &rc.vars[bi][k];
                    k += 1;
                    let vtag = format!("{tag} {}[{r}].{}", block.name, var.name);
                    let size = reader.size(block.key, r, var.key).unwrap();
                    m.eq(&format!("{vtag} size"), vr.raw.len() as i32, size);
                    let raw = reader.get(block.key, r, var.key, 0).unwrap();
                    m.bytes(&format!("{vtag} raw"), &vr.raw, raw);
                    if vr.stated {
                        stated += 1;
                        assert!(vr.raw.is_empty(), "{vtag}: a stated field is empty");
                    }
                }
            }
        }
    }
    println!(
        "{} read cases, {stated} fields with the stated past-the-end behaviour",
        c.reads.len()
    );
    m.report("reader");
}

#[test]
fn zero_code_expansion_parity() {
    let c = load();
    let mut m = Mismatches(Vec::new(), 0);
    let mut out = vec![0u8; MAX_BUFFER_SIZE];
    for (i, e) in c.expands.iter().enumerate() {
        let tag = format!("{}#{i}", e.kind);
        if e.input[0] & vkstorm_messages::ZERO_CODE_FLAG == 0 {
            // not coded: passed through unchanged, result 0
            m.eq(&format!("{tag} result"), e.result, 0);
            m.bytes(&format!("{tag} output"), &e.output, &e.input);
            continue;
        }
        m.eq(&format!("{tag} result"), e.result, e.input.len() as i32);
        out.fill(0);
        let x = zerocode::zero_code_expand(&e.input, &mut out);
        m.eq(
            &format!("{tag} wrote past"),
            e.wrote_past,
            x.overruns as i32 + x.error.is_some() as i32,
        );
        m.bytes(&format!("{tag} output"), &e.output, &out[..x.len]);
    }
    m.report("zero-code expansion");
}

const DP_STRING: u8 = 0;
const DP_BINARY: u8 = 1;
const DP_BINARY_FIXED: u8 = 2;
const DP_U8: u8 = 3;
const DP_U16: u8 = 4;
const DP_S16: u8 = 5;
const DP_U32: u8 = 6;
const DP_S32: u8 = 7;
const DP_F32: u8 = 8;
const DP_COLOR4: u8 = 9;
const DP_COLOR4U: u8 = 10;
const DP_VECTOR2: u8 = 11;
const DP_VECTOR3: u8 = 12;
const DP_VECTOR4: u8 = 13;
const DP_UUID: u8 = 14;
const DP_FIXED: u8 = 15;

fn f32s(v: &[u8]) -> Vec<f32> {
    v.chunks(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

fn uuid_text(bytes: &[u8]) -> String {
    let h: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}{}{}{}-{}{}-{}{}-{}{}-{}{}{}{}{}{}",
        h[0],
        h[1],
        h[2],
        h[3],
        h[4],
        h[5],
        h[6],
        h[7],
        h[8],
        h[9],
        h[10],
        h[11],
        h[12],
        h[13],
        h[14],
        h[15]
    )
}

/// `LLUUID::set` on the text form: 16 bytes, or null on a malformed one.
fn uuid_parse(text: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    if text.len() < 36 {
        return out;
    }
    let mut i = 0;
    let mut cur = 0usize;
    let mut hi = true;
    for (pos, &c) in text[..36].iter().enumerate() {
        if matches!(pos, 8 | 13 | 18 | 23) {
            if c != b'-' {
                return [0; 16];
            }
            continue;
        }
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => return [0; 16],
        };
        if hi {
            cur = (d as usize) << 4;
        } else {
            out[i] = (cur | d as usize) as u8;
            i += 1;
        }
        hi = !hi;
    }
    out
}

/// One pack op on the binary form, as the C++ class does it (the typed
/// `pack*` are fixed-size byte copies; `packFixed` goes through U8/U16/U32).
fn bin_pack(b: &mut BinaryBuffer, op: &DpOp) -> bool {
    match op.kind {
        DP_STRING => b.pack_string(&op.value),
        DP_BINARY => b.pack_binary(&op.value),
        DP_S16 => b.pack_s16(i16::from_le_bytes([op.value[0], op.value[1]])),
        DP_FIXED => {
            // packFixed reports success whatever the inner pack said
            let v = f32::from_le_bytes(op.value[..4].try_into().unwrap());
            let (bits, fixed) =
                datapacker::pack_fixed(v, op.value[4] != 0, op.value[5] as u32, op.value[6] as u32);
            if bits <= 8 {
                b.pack_bytes(&[fixed as u8]);
            } else if bits <= 16 {
                b.pack_bytes(&(fixed as u16).to_le_bytes());
            } else {
                b.pack_bytes(&fixed.to_le_bytes());
            }
            true
        }
        _ => b.pack_bytes(&op.value),
    }
}

fn bin_unpack(b: &mut BinaryBuffer, op: &DpOp) -> (bool, Vec<u8>) {
    let sentinel = |n: usize| vec![0xA5u8; n];
    match op.kind {
        DP_STRING => match b.unpack_string() {
            Some(s) => (true, s.to_vec()),
            None => (false, Vec::new()),
        },
        DP_BINARY => {
            // the size field reaches the caller even when the data cannot
            let c = b.cur.max(0) as usize;
            let size_bytes: Vec<u8> = if c + 4 <= b.data.len() {
                b.data[c..c + 4].to_vec()
            } else {
                0xA5A5A5A5u32.to_le_bytes().to_vec()
            };
            match b.unpack_binary() {
                Some(d) => {
                    let mut v = (d.len() as i32).to_le_bytes().to_vec();
                    v.extend_from_slice(d);
                    (true, v)
                }
                None => (false, size_bytes),
            }
        }
        DP_S16 => match b.unpack_s16() {
            Some(v) => (true, v.to_le_bytes().to_vec()),
            None => (false, sentinel(2)),
        },
        DP_FIXED => {
            let bits = datapacker::fixed_total_bits(
                op.value[4] != 0,
                op.value[5] as u32,
                op.value[6] as u32,
            );
            let n = if bits <= 8 {
                1
            } else if bits <= 16 {
                2
            } else {
                4
            };
            match b.unpack_bytes(n) {
                Some(d) => {
                    let fixed = match n {
                        1 => d[0] as u32,
                        2 => u16::from_le_bytes([d[0], d[1]]) as u32,
                        _ => u32::from_le_bytes(d.try_into().unwrap()),
                    };
                    let v = datapacker::unpack_fixed(
                        fixed,
                        op.value[4] != 0,
                        op.value[5] as u32,
                        op.value[6] as u32,
                    );
                    (true, v.to_le_bytes().to_vec())
                }
                None => {
                    // unpackFixed on failure: fixed_val is the (unassigned) local
                    // converted and scaled; the C++ wrapper leaves the value as
                    // it was, which the fixture records as the sentinel
                    (false, sentinel(4))
                }
            }
        }
        _ => {
            let n = match op.kind {
                DP_BINARY_FIXED => op.size as usize,
                DP_U8 => 1,
                DP_U16 => 2,
                DP_U32 | DP_S32 | DP_F32 | DP_COLOR4U => 4,
                DP_VECTOR2 => 8,
                DP_VECTOR3 => 12,
                DP_COLOR4 | DP_VECTOR4 | DP_UUID => 16,
                _ => 0,
            };
            match b.unpack_bytes(n) {
                Some(d) => (true, d.to_vec()),
                None => (false, sentinel(n)),
            }
        }
    }
}

fn text_kind(op: &DpOp) -> (TextKind, Vec<i64>, Vec<f32>, Vec<u8>, usize) {
    match op.kind {
        DP_STRING => (TextKind::String, vec![], vec![], op.value.clone(), 0),
        DP_BINARY => (TextKind::Binary, vec![], vec![], op.value.clone(), 0),
        DP_BINARY_FIXED => (
            TextKind::BinaryFixed,
            vec![],
            vec![],
            op.value.clone(),
            op.size as usize,
        ),
        DP_U8 => (TextKind::Int, vec![op.value[0] as i64], vec![], vec![], 1),
        DP_U16 => (
            TextKind::Int,
            vec![u16::from_le_bytes([op.value[0], op.value[1]]) as i64],
            vec![],
            vec![],
            1,
        ),
        DP_S16 => (
            TextKind::Int,
            vec![i16::from_le_bytes([op.value[0], op.value[1]]) as i64],
            vec![],
            vec![],
            1,
        ),
        DP_U32 => (
            TextKind::Uint,
            vec![u32::from_le_bytes(op.value[..4].try_into().unwrap()) as i64],
            vec![],
            vec![],
            1,
        ),
        DP_S32 => (
            TextKind::Int,
            vec![i32::from_le_bytes(op.value[..4].try_into().unwrap()) as i64],
            vec![],
            vec![],
            1,
        ),
        DP_F32 => (TextKind::Floats, vec![], f32s(&op.value), vec![], 1),
        DP_COLOR4 | DP_VECTOR4 => (TextKind::Floats, vec![], f32s(&op.value), vec![], 4),
        DP_VECTOR3 => (TextKind::Floats, vec![], f32s(&op.value), vec![], 3),
        DP_VECTOR2 => (TextKind::Floats, vec![], f32s(&op.value), vec![], 2),
        DP_COLOR4U => (TextKind::Bytes, vec![], vec![], op.value.clone(), 4),
        DP_UUID => (
            TextKind::Uuid,
            vec![],
            vec![],
            uuid_text(&op.value).into_bytes(),
            0,
        ),
        DP_FIXED => {
            let v = f32::from_le_bytes(op.value[..4].try_into().unwrap());
            let (bits, fixed) =
                datapacker::pack_fixed(v, op.value[4] != 0, op.value[5] as u32, op.value[6] as u32);
            if bits <= 16 {
                (TextKind::Int, vec![fixed as i64], vec![], vec![], 1)
            } else {
                (TextKind::Uint, vec![fixed as i64], vec![], vec![], 1)
            }
        }
        _ => unreachable!(),
    }
}

/// The C++ `unpack*` of the text form on a value line: the value bytes as
/// the C++ left them, starting from the sentinel.
fn text_unpack(op: &DpOp, line: &[u8]) -> Vec<u8> {
    let (kind, _, _, _, count) = text_kind(op);
    let p = datapacker::parse_value(kind, line, count);
    match op.kind {
        DP_STRING => p.bytes,
        DP_UUID => {
            if p.assigned == 1 {
                uuid_parse(&p.bytes).to_vec()
            } else {
                vec![0xA5; 16]
            }
        }
        DP_BINARY => {
            let mut v = if p.assigned == 1 {
                p.size.to_le_bytes().to_vec()
            } else {
                0xA5A5A5A5u32.to_le_bytes().to_vec()
            };
            let n = p.size.clamp(0, 70000) as usize;
            let mut data = vec![0xA5u8; n.min(167)];
            for (i, b) in p.bytes.iter().enumerate().take(data.len()) {
                data[i] = *b;
            }
            v.extend_from_slice(&data);
            v
        }
        DP_BINARY_FIXED => {
            let mut data = vec![0xA5u8; (op.size as usize).min(170)];
            for (i, b) in p.bytes.iter().enumerate().take(data.len()) {
                data[i] = *b;
            }
            data
        }
        DP_U8 => {
            if p.assigned == 1 {
                vec![p.ints[0] as u8]
            } else {
                vec![0xA5]
            }
        }
        DP_U16 | DP_S16 => {
            if p.assigned == 1 {
                (p.ints[0] as u16).to_le_bytes().to_vec()
            } else {
                vec![0xA5; 2]
            }
        }
        DP_U32 | DP_S32 => {
            if p.assigned == 1 {
                (p.ints[0] as u32).to_le_bytes().to_vec()
            } else {
                vec![0xA5; 4]
            }
        }
        DP_F32 | DP_COLOR4 | DP_VECTOR2 | DP_VECTOR3 | DP_VECTOR4 => {
            let mut v = vec![0xA5u8; count * 4];
            for (i, f) in p.floats.iter().enumerate() {
                v[i * 4..i * 4 + 4].copy_from_slice(&f.to_le_bytes());
            }
            v
        }
        DP_COLOR4U => {
            let mut v = vec![0xA5u8; 4];
            for (i, x) in p.ints.iter().enumerate() {
                v[i] = *x as u8;
            }
            v
        }
        DP_FIXED => {
            if p.assigned == 1 {
                let bits = datapacker::fixed_total_bits(
                    op.value[4] != 0,
                    op.value[5] as u32,
                    op.value[6] as u32,
                );
                let fixed = if bits <= 8 {
                    p.ints[0] as u8 as u32
                } else if bits <= 16 {
                    p.ints[0] as u16 as u32
                } else {
                    p.ints[0] as u32
                };
                datapacker::unpack_fixed(
                    fixed,
                    op.value[4] != 0,
                    op.value[5] as u32,
                    op.value[6] as u32,
                )
                .to_le_bytes()
                .to_vec()
            } else {
                vec![0xA5; 4]
            }
        }
        _ => unreachable!(),
    }
}

#[test]
fn data_packer_parity() {
    let c = load();
    let mut m = Mismatches(Vec::new(), 0);
    for (i, d) in c.packers.iter().enumerate() {
        let tag = format!("{}#{i}", d.kind);
        match d.kind.as_str() {
            "binary" => {
                let mut data = vec![0xA5u8; d.buffer_size as usize + 16];
                let mut b = BinaryBuffer {
                    data: &mut data,
                    cur: 0,
                    size: d.buffer_size,
                    write_enabled: true,
                };
                for (k, op) in d.ops.iter().enumerate() {
                    let ok = bin_pack(&mut b, op);
                    m.eq(
                        &format!("{tag} op {k} {} pack ok", op.name),
                        d.pack_results[k].ok,
                        ok,
                    );
                    m.eq(
                        &format!("{tag} op {k} {} pack cursor", op.name),
                        d.pack_results[k].cursor,
                        b.cur,
                    );
                }
                let n = b.cur.clamp(0, d.buffer_size) as usize;
                m.bytes(&format!("{tag} packed"), &d.packed, &data[..n]);
                let mut measure = BinaryBuffer {
                    data: &mut [],
                    cur: 0,
                    size: 0,
                    write_enabled: false,
                };
                for op in &d.ops {
                    bin_pack(&mut measure, op);
                }
                m.eq(&format!("{tag} measured"), d.measured, measure.cur);
                let mut input = data[..n].to_vec();
                input.resize(n.max(d.unpack_size as usize) + 16, 0xA5);
                let size = d.unpack_size as usize;
                let mut u = BinaryBuffer {
                    data: &mut input[..size],
                    cur: 0,
                    size: d.unpack_size,
                    write_enabled: true,
                };
                for (k, op) in d.ops.iter().enumerate() {
                    let (ok, value) = bin_unpack(&mut u, op);
                    let otag = format!("{tag} op {k} {}", op.name);
                    m.eq(&format!("{otag} unpack ok"), d.unpack_results[k].ok, ok);
                    m.eq(
                        &format!("{otag} unpack cursor"),
                        d.unpack_results[k].cursor,
                        u.cur,
                    );
                    m.bytes(
                        &format!("{otag} unpack value"),
                        &d.unpack_results[k].value,
                        &value,
                    );
                }
            }
            "ascii" | "ascii_names" => {
                let names = d.kind == "ascii_names";
                let mut data = vec![0xA5u8; d.buffer_size as usize + 16];
                let mut a = AsciiBuffer {
                    data: &mut data,
                    cur: 0,
                    size: d.buffer_size,
                    write_enabled: true,
                    include_names: names,
                };
                for (k, op) in d.ops.iter().enumerate() {
                    let (kind, ints, floats, bytes, _) = text_kind(op);
                    let (ok, _) = a.pack(kind, op.name.as_bytes(), &ints, &floats, &bytes);
                    m.eq(
                        &format!("{tag} op {k} {} pack ok", op.name),
                        d.pack_results[k].ok,
                        ok,
                    );
                    m.eq(
                        &format!("{tag} op {k} {} pack cursor", op.name),
                        d.pack_results[k].cursor,
                        a.cur + 1,
                    );
                }
                let n = (a.cur + 1).clamp(0, d.buffer_size) as usize;
                m.bytes(&format!("{tag} packed"), &d.packed, &data[..n]);
                let mut measure = AsciiBuffer {
                    data: &mut [],
                    cur: 0,
                    size: d.buffer_size,
                    write_enabled: false,
                    include_names: names,
                };
                for op in &d.ops {
                    let (kind, ints, floats, bytes, _) = text_kind(op);
                    measure.pack(kind, op.name.as_bytes(), &ints, &floats, &bytes);
                }
                m.eq(&format!("{tag} measured"), d.measured, measure.cur + 1);
                let mut input = data[..n].to_vec();
                input.resize(n.max(d.unpack_size as usize) + 16, 0);
                let cut = d.unpack_size as usize - 1;
                if (d.unpack_size as usize) < n + 1 && cut < input.len() {
                    input[cut] = 0;
                }
                let size = d.unpack_size as usize;
                let mut u = AsciiBuffer {
                    data: &mut input[..size],
                    cur: 0,
                    size: d.unpack_size,
                    write_enabled: true,
                    include_names: names,
                };
                for (k, op) in d.ops.iter().enumerate() {
                    let otag = format!("{tag} op {k} {}", op.name);
                    let line = u.next_value(op.name.as_bytes());
                    m.eq(
                        &format!("{otag} unpack ok"),
                        d.unpack_results[k].ok,
                        line.is_some(),
                    );
                    m.eq(
                        &format!("{otag} unpack cursor"),
                        d.unpack_results[k].cursor,
                        u.cur + 1,
                    );
                    if let Some(line) = line {
                        let value = text_unpack(op, &line);
                        m.bytes(
                            &format!("{otag} unpack value"),
                            &d.unpack_results[k].value,
                            &value,
                        );
                    }
                }
            }
            "file" => {
                // the file form: indent, name, tab, the value line
                let mut text = Vec::new();
                for op in &d.ops {
                    let (kind, ints, floats, bytes, _) = text_kind(op);
                    let kind = if kind == TextKind::Floats {
                        TextKind::FloatsShort
                    } else {
                        kind
                    };
                    text.extend_from_slice(b"\t\t");
                    text.extend_from_slice(op.name.as_bytes());
                    text.push(b'\t');
                    datapacker::format_value(kind, &ints, &floats, &bytes, &mut text);
                }
                m.bytes(&format!("{tag} packed"), &d.packed, &text);
                // the text the reader was given (a case may cut it)
                text.truncate(d.unpack_size as usize);
                // reading: one getline of at most 511 bytes per value;
                // a longer line leaves the stream failed
                let mut pos = 0usize;
                let mut failed = false;
                for (k, op) in d.ops.iter().enumerate() {
                    let otag = format!("{tag} op {k} {}", op.name);
                    let mut ok = false;
                    let mut value: Vec<u8> = Vec::new();
                    if !failed && pos <= text.len() {
                        let rest = &text[pos..];
                        let nl = rest.iter().position(|&b| b == b'\n');
                        let line_len = nl.unwrap_or(rest.len());
                        if line_len > 511 {
                            // getline stops at 511 bytes with failbit set
                            failed = true;
                            pos += 511;
                            let line = &rest[..511];
                            let (key, v) = vkstorm_messages::ascii::split_keyword_value(line);
                            if key == op.name.as_bytes() {
                                ok = true;
                                value = text_unpack(op, v);
                            }
                        } else {
                            let line = &rest[..line_len];
                            pos += line_len + 1;
                            let (key, v) = vkstorm_messages::ascii::split_keyword_value(line);
                            if !key.is_empty() && key == op.name.as_bytes() {
                                ok = true;
                                value = text_unpack(op, v);
                            }
                        }
                    }
                    m.eq(&format!("{otag} unpack ok"), d.unpack_results[k].ok, ok);
                    if ok {
                        m.bytes(
                            &format!("{otag} unpack value"),
                            &d.unpack_results[k].value,
                            &value,
                        );
                    }
                }
            }
            other => panic!("unknown packer kind {other}"),
        }
    }
    m.report("data packers");
}

#[test]
fn block_types_cover_the_template() {
    let (_, list, _, _) = templates();
    let mut kinds = [0usize; 3];
    for t in &list {
        for b in &t.blocks {
            match b.ty {
                BlockType::Single => kinds[0] += 1,
                BlockType::Multiple => kinds[1] += 1,
                BlockType::Variable => kinds[2] += 1,
                _ => {}
            }
        }
    }
    assert!(kinds.iter().all(|&k| k > 0), "{kinds:?}");
}
