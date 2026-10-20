//! The two hot paths, timed in the crate alone: unpacking a 1 KB
//! `ObjectUpdate` (the template's largest high-frequency message, three
//! object blocks) and packing an `AgentUpdate` (sent every frame), plus the
//! same reader's getters over every field of the decoded message. The
//! viewer-side harness (`INTEGRATION_TEST_llmessagetemplate` with
//! `VKS_MSG_BENCH`) times the same work through the C++ classes and the C
//! ABI; the difference is the boundary cost.
//!
//! Usage: `cargo bench --bench hot_paths [-- iterations]`, with the message
//! template at `VKS_MESSAGE_TEMPLATE` (default: the viewer's
//! `scripts/messages/message_template.msg` next to this workspace).

use std::sync::Arc;
use std::time::Instant;
use vkstorm_messages::reader::decode_number;
use vkstorm_messages::{Builder, Reader, Template, Tokenizer, VarType, parse_template, zerocode};

fn template_file() -> String {
    let path = std::env::var("VKS_MESSAGE_TEMPLATE").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/message_template.msg"
        )
        .to_owned()
    });
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn fill(builder: &mut Builder, t: &Arc<Template>, repeats: &[(usize, usize)], var_len: usize) {
    builder.new_message(t.clone());
    for &(b, n) in repeats {
        let block = &t.blocks[b];
        for r in 0..n {
            builder.next_block(block.key).unwrap();
            for (v, var) in block.variables.iter().enumerate() {
                let data: Vec<u8> = if var.ty == VarType::Variable {
                    (0..var_len).map(|i| (i + v + r) as u8 | 1).collect()
                } else {
                    (0..var.size as usize)
                        .map(|i| {
                            if (i + v) % 8 < 4 {
                                0
                            } else {
                                (i * 7 + v + 3 * r) as u8
                            }
                        })
                        .collect()
                };
                builder.add_data(var.key, &data, var.ty).unwrap();
            }
        }
    }
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

    // a ~1 KB ObjectUpdate: RegionData plus five ObjectData repeats with
    // modest variable fields
    let mut builder = Builder::new();
    fill(&mut builder, &object_update, &[(0, 1), (1, 3)], 24);
    let mut packet = vec![0u8; 8192];
    let len = builder.build_message(&mut packet, 0).unwrap();
    let packet = packet[..len].to_vec();
    let mut coded = vec![0u8; 16384];
    let coded_len = zerocode::zero_code(&packet, &mut coded).unwrap();
    let coded = coded[..coded_len].to_vec();
    println!("ObjectUpdate packet: {len} bytes, zero-coded {coded_len}");

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

    let mut rounds = Vec::new();
    let mut sink = 0u64;
    for _ in 0..7 {
        let start = Instant::now();
        for _ in 0..iterations {
            reader.set_message(object_update.clone(), len as i32);
            reader.decode_data(&packet);
            for (b, block) in object_update.blocks.iter().enumerate() {
                let n = reader.number_of_blocks(block.key).unwrap();
                for r in 0..n {
                    for var in &block.variables {
                        let d = reader.get(block.key, r, var.key, 0).unwrap();
                        sink = sink
                            .wrapping_add(d.len() as u64 + d.first().copied().unwrap_or(0) as u64);
                    }
                }
                let _ = b;
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

    let mut rounds = Vec::new();
    let mut out = vec![0u8; 8192];
    let mut coded = vec![0u8; 16384];
    for _ in 0..7 {
        let start = Instant::now();
        for _ in 0..iterations {
            fill(&mut builder, &agent_update, &[(0, 1)], 0);
            let n = builder.build_message(&mut out, 0).unwrap();
            let _ = zerocode::zero_code(&out[..n], &mut coded);
        }
        rounds.push(start.elapsed().as_secs_f64() * 1e6 / iterations as f64);
    }
    println!(
        "pack AgentUpdate (new message, add every field, build, zero-code): {:.3} us",
        median(rounds)
    );
}
