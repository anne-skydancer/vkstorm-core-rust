//! The hot paths, timed in the crate alone: unpacking an `ObjectUpdate`
//! (`tests/fixtures/objectupdate.bin`, 1274 bytes: `RegionData` and four
//! `ObjectData` repeats of a typical prim, one with hover text and name
//! values, one with two textures), reading it as the viewer's handler does,
//! expanding its zero-coded form (`objectupdate_coded.bin`, 600 bytes) and
//! packing an `AgentUpdate` with a moving agent (`agentupdate.bin`, 121
//! bytes). The packets were constructed by the viewer's own builder
//! (`INTEGRATION_TEST_llmessagetemplate` with `VKS_MSG_PACKETS_OUT`): no
//! real session was captured, since nothing reaches UDP without a login.
//! The viewer-side harness (the same test with `VKS_MSG_BENCH`) times the
//! same work through the C++ classes and the C ABI; the difference is the
//! boundary cost.
//!
//! Usage: `cargo bench --bench hot_paths [-- iterations]`.

use std::sync::Arc;
use std::time::Instant;
use vkstorm_messages::reader::decode_number;
use vkstorm_messages::{Builder, Reader, Tokenizer, VarType, parse_template, zerocode};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn template_file() -> String {
    String::from_utf8(fixture("message_template.msg")).unwrap()
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let iterations: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(20000);
    let text = template_file();
    let file = parse_template(&mut Tokenizer::new(&text)).unwrap();
    let find = |name: &str| {
        Arc::new(
            file.messages
                .iter()
                .find(|m| m.name == name)
                .unwrap()
                .clone()
                .with_index_keys(),
        )
    };
    let object_update = find("ObjectUpdate");
    let agent_update = find("AgentUpdate");

    let packet = fixture("objectupdate.bin");
    let coded = fixture("objectupdate_coded.bin");
    let agent_packet = fixture("agentupdate.bin");
    let len = packet.len();
    println!(
        "ObjectUpdate packet: {len} bytes, zero-coded {}; AgentUpdate {} bytes",
        coded.len(),
        agent_packet.len()
    );

    let mut reader = Reader::new();
    let mut rounds = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        for _ in 0..iterations {
            let num = decode_number(&packet).unwrap();
            assert_eq!(num, object_update.number);
            reader.set_message(object_update.clone(), len as i32);
            let out = reader.decode_data(&packet);
            assert!(out.has_blocks);
        }
        rounds.push(start.elapsed().as_secs_f64() * 1e6 / iterations as f64);
    }
    println!(
        "unpack ObjectUpdate (decode only): {:.3} us",
        median(rounds)
    );

    // the getters the viewer's handler issues for a full prim update:
    // every variable of every repeat except Data (empty here), read by key
    let region = &object_update.blocks[0];
    let objects = &object_update.blocks[1];
    let mut rounds = Vec::new();
    let mut sink = 0u64;
    for _ in 0..7 {
        let start = Instant::now();
        for _ in 0..iterations {
            reader.set_message(object_update.clone(), len as i32);
            reader.decode_data(&packet);
            let n = reader.number_of_blocks(objects.key).unwrap();
            for var in &region.variables {
                sink += reader.get(region.key, 0, var.key, 0).unwrap().len() as u64;
            }
            for r in 0..n {
                for var in &objects.variables {
                    let d = reader.get(objects.key, r, var.key, 0).unwrap();
                    sink =
                        sink.wrapping_add(d.len() as u64 + d.first().copied().unwrap_or(0) as u64);
                }
            }
        }
        rounds.push(start.elapsed().as_secs_f64() * 1e6 / iterations as f64);
    }
    println!(
        "unpack ObjectUpdate (decode + every getter): {:.3} us (sink {sink})",
        median(rounds)
    );

    let mut rounds = Vec::new();
    let mut expanded = vec![0u8; 8192];
    for _ in 0..7 {
        let start = Instant::now();
        for _ in 0..iterations {
            let e = zerocode::zero_code_expand(&coded, &mut expanded);
            assert_eq!(e.len, len);
        }
        rounds.push(start.elapsed().as_secs_f64() * 1e6 / iterations as f64);
    }
    println!("zero-code expand ObjectUpdate: {:.3} us", median(rounds));

    // pack the AgentUpdate: the fixture's field values added one by one
    let mut r2 = Reader::new();
    r2.set_message(agent_update.clone(), agent_packet.len() as i32);
    r2.decode_data(&agent_packet);
    let block = &agent_update.blocks[0];
    let fields: Vec<(u64, Vec<u8>, VarType)> = block
        .variables
        .iter()
        .map(|v| {
            (
                v.key,
                r2.get(block.key, 0, v.key, 0).unwrap().to_vec(),
                v.ty,
            )
        })
        .collect();
    let mut builder = Builder::new();
    let mut rounds = Vec::new();
    let mut out = vec![0u8; 8192];
    let mut coded_out = vec![0u8; 16384];
    for _ in 0..7 {
        let start = Instant::now();
        for _ in 0..iterations {
            builder.new_message(agent_update.clone());
            builder.next_block(block.key).unwrap();
            for (key, data, ty) in &fields {
                builder.add_data(*key, data, *ty).unwrap();
            }
            let n = builder.build_message(&mut out, 0).unwrap();
            let _ = zerocode::zero_code(&out[..n], &mut coded_out);
        }
        rounds.push(start.elapsed().as_secs_f64() * 1e6 / iterations as f64);
    }
    println!(
        "pack AgentUpdate (new message, add every field, build, zero-code): {:.3} us",
        median(rounds)
    );
}
