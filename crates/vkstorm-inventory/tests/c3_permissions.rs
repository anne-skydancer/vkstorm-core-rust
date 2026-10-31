//! C3 permissions: regenerates every line of `fixtures/model/c3_permissions.txt`
//! and of the `c3r` random-stream sample from the crate, as `c3_permissions()`
//! and `c3r_case()` in `llinventory_tool.cpp` write them from the frozen C++,
//! and checks the `c3r` stream and the `c3x` exhaustive sweeps against their
//! chunked SHA-256 digests in `digests.txt`.

mod common;

use std::time::Instant;

use common::*;
use vkstorm_inventory::aggregate::AggregatePermissions;
use vkstorm_inventory::legacy::reader::Stream;
use vkstorm_inventory::perms::{
    NULL_ID, PERM_ALL, PERM_COPY, PERM_EXPORT, PERM_MODIFY, PERM_MOVE, PERM_TRANSFER, Permissions,
    Uuid,
};
use vkstorm_inventory::types::InventoryType;
use vkstorm_llsd::Value;

const T_: u32 = PERM_TRANSFER;
const M_: u32 = PERM_MODIFY;
const C_: u32 = PERM_COPY;
const X_: u32 = PERM_EXPORT;
const V_: u32 = PERM_MOVE;

/// `PermF` defaults: null ids, every mask `PERM_ALL`, not group owned.
/// `make_perm` stores every field raw, so the record is the `PermF`.
const PERM_F: Permissions = Permissions::DEFAULT;

/// `c3_states()`.
fn c3_states() -> Vec<Permissions> {
    let mut v = Vec::new();
    // 0: owned by A, created by C, group G, typical masks.
    let mut p = Permissions {
        creator: id_c(),
        owner: id_a(),
        last_owner: id_b(),
        group: id_g(),
        base: PERM_ALL,
        owner_mask: PERM_ALL,
        group_mask: 0,
        everyone: 0,
        next: T_ | V_,
        ..PERM_F
    };
    v.push(p);
    // 1: no-transfer owner mask.
    p.owner_mask = PERM_ALL & !T_;
    p.base = PERM_ALL & !T_;
    p.next = C_ | M_ | V_;
    v.push(p);
    // 2: group owned (owner null, group G).
    v.push(Permissions {
        creator: id_c(),
        owner: NUL,
        last_owner: id_a(),
        group: id_g(),
        group_owned: true,
        base: PERM_ALL,
        owner_mask: PERM_ALL,
        group_mask: V_ | C_,
        everyone: C_,
        next: PERM_ALL,
        ..PERM_F
    });
    // 3: unowned, no group.
    v.push(PERM_F);
    // 4: owner A, last owner null, group null, no-copy base.
    v.push(Permissions {
        creator: id_a(),
        owner: id_a(),
        base: PERM_ALL & !C_,
        owner_mask: PERM_ALL & !C_,
        group_mask: 0,
        everyone: 0,
        next: 0,
        ..PERM_F
    });
    // 5: raw everything (unfixed masks, flag inconsistent with ids).
    v.push(Permissions {
        creator: id_b(),
        owner: id_a(),
        last_owner: id_a(),
        group: id_h(),
        group_owned: true,
        base: 0xffffffff,
        owner_mask: 0x7fff1fff,
        group_mask: 0x80000001,
        everyone: 0x0000e000,
        next: 0,
    });
    // 6: group G set, owner A, group mask with modify, everyone copy.
    let mut p = Permissions {
        creator: id_c(),
        owner: id_a(),
        group: id_g(),
        base: PERM_ALL,
        owner_mask: PERM_ALL,
        group_mask: M_ | C_ | V_,
        everyone: C_ | V_,
        next: PERM_ALL,
        ..PERM_F
    };
    v.push(p);
    // 7: owner A, base no-transfer, group not owned, everyone copy.
    p.base = PERM_ALL & !T_;
    p.owner_mask = PERM_ALL & !T_;
    v.push(p);
    // 8: owner A whose next-owner mask has neither copy nor transfer.
    v.push(Permissions {
        creator: id_c(),
        owner: id_a(),
        last_owner: id_b(),
        group: id_g(),
        base: PERM_ALL,
        owner_mask: PERM_ALL,
        group_mask: 0,
        everyone: 0,
        next: M_ | V_,
        ..PERM_F
    });
    v
}

const OPS: [u32; 7] = [T_, M_, C_, V_, X_, 0, PERM_ALL];

/// `perm_queries`.
fn perm_queries(p: &Permissions) -> Vec<String> {
    let ch = |b: bool| if b { '1' } else { '0' };
    let req = [NUL, id_a(), id_b(), id_g(), id_h()];
    let grp = [NUL, id_g(), id_h()];
    let mut allow = String::with_capacity(105);
    for r in &req {
        for g in &grp {
            for op in OPS {
                allow.push(ch(p.allow_operation_by(op, r, g)));
            }
        }
    }
    let mut helpers = String::with_capacity(40);
    for r in &req {
        helpers.push(ch(p.allow_modify_by(r, &NULL_ID)));
        helpers.push(ch(p.allow_copy_by(r, &NULL_ID)));
        helpers.push(ch(p.allow_move_by(r, &NULL_ID)));
        helpers.push(ch(p.allow_modify_by(r, &id_g())));
        helpers.push(ch(p.allow_copy_by(r, &id_g())));
        helpers.push(ch(p.allow_move_by(r, &id_g())));
        helpers.push(ch(p.allow_transfer_to(r)));
        helpers.push(ch(p.allow_export_by(r)));
    }
    // getOwnership starts from U(ee)/true and leaves them when unowned.
    let (owned, (owner_id, is_group)) = match p.ownership() {
        Some(o) => (true, o),
        None => (false, (u(0xee), true)),
    };
    vec![
        allow,
        helpers,
        x8(p.crc32()),
        bl(owned),
        uuid(&owner_id),
        bl(is_group),
        bl(p.is_owned()),
        bl(p.is_group_owned()),
    ]
}

/// The legacy round trip of the recorder: export, then import the text
/// after the `permissions 0` line into `into`.
fn legacy_round_trip(p: &Permissions, into: &mut Permissions) -> (Vec<u8>, bool, Vec<String>) {
    let mut text = Vec::new();
    p.export_legacy_stream(&mut text);
    let body = after_first_line(&text);
    let mut is = Stream::new(body);
    let ok = into.import_legacy_stream(&mut is);
    let stream = stream_cols(&is, body);
    (text, ok, stream)
}

/// `c3_permissions<I>()` over the crate.
fn c3_lines() -> Lines {
    let mut s = Lines::default();
    let p = Permissions::default();
    s.emit("c3.default", "perm", &perm_cols(&p));
    s.emit("c3.default", "static", &perm_cols(&Permissions::DEFAULT));

    // fix() on structured masks.
    for &b in &MASK_EDGES {
        for &x in &MASK_EDGES {
            for go in 0..2 {
                let mut p = Permissions {
                    base: b,
                    owner_mask: x,
                    group_mask: x,
                    everyone: x,
                    next: x,
                    group_owned: go != 0,
                    ..PERM_F
                };
                p.fix();
                s.emit(
                    &format!("c3.fix.{}.{}.{go}", x8(b), x8(x)),
                    "perm",
                    &perm_cols(&p),
                );
            }
        }
    }
    // initMasks(5 args).
    for &b in &MASK_EDGES {
        for &n in &MASK_EDGES {
            let mut p = Permissions::default();
            p.init_masks(b, PERM_ALL, PERM_ALL, PERM_ALL, n);
            s.emit(
                &format!("c3.initmasks.{}.{}", x8(b), x8(n)),
                "perm",
                &perm_cols(&p),
            );
        }
    }
    // initMasks(inventory type).
    let states = c3_states();
    for t in -1..=30 {
        for si in [0, 5] {
            let mut p = states[si];
            p.init_masks_for_type(InventoryType(t));
            s.emit(
                &format!("c3.initmasks_type.{t}.{si}"),
                "perm",
                &perm_cols(&p),
            );
        }
    }
    {
        let mut p = states[5];
        p.init_masks_for_type(InventoryType(255));
        s.emit("c3.initmasks_type.255.5", "perm", &perm_cols(&p));
    }
    // accumulate over all pairs.
    for (a, sa) in states.iter().enumerate() {
        for (b, sb) in states.iter().enumerate() {
            let mut p = *sa;
            p.accumulate(sb);
            s.emit(&format!("c3.accumulate.{a}.{b}"), "perm", &perm_cols(&p));
        }
    }
    let ids = [NUL, id_a(), id_b(), id_g()];
    // setOwnerAndGroup.
    for si in [0usize, 1, 2, 3, 8] {
        for agent in &ids {
            for owner in &ids {
                for group in &ids {
                    for atomic in 0..2 {
                        let mut p = states[si];
                        let ret = p.set_owner_and_group(agent, owner, group, atomic != 0);
                        let cs = format!(
                            "c3.soag.{si}.{}.{}.{}.{atomic}",
                            idname(agent),
                            idname(owner),
                            idname(group)
                        );
                        s.emit(&cs, "ret+perm", &ret_perm_cols(ret, &p));
                    }
                }
            }
        }
    }
    // deedToGroup.
    for (si, st) in states.iter().enumerate() {
        for agent in &ids {
            for group in &[NUL, id_g(), id_h()] {
                let mut p = *st;
                let ret = p.deed_to_group(agent, group);
                let cs = format!("c3.deed.{si}.{}.{}", idname(agent), idname(group));
                s.emit(&cs, "ret+perm", &ret_perm_cols(ret, &p));
            }
        }
    }
    // set*Bits.
    let bits = [T_, M_, C_, V_, X_, PERM_ALL];
    let agents = [NUL, id_a(), id_b()];
    let groups = [NUL, id_g(), id_h()];
    for si in [0usize, 1, 2] {
        for agent in &agents {
            for set in [false, true] {
                for &bit in &bits {
                    let tail = format!("{si}.{}.{}.{}", idname(agent), u8::from(set), x8(bit));
                    let mut p = states[si];
                    let ret = p.set_base_bits(agent, set, bit);
                    s.emit(
                        &format!("c3.setbase.{tail}"),
                        "ret+perm",
                        &ret_perm_cols(ret, &p),
                    );
                    let mut p = states[si];
                    let ret = p.set_owner_bits(agent, set, bit);
                    s.emit(
                        &format!("c3.setowner.{tail}"),
                        "ret+perm",
                        &ret_perm_cols(ret, &p),
                    );
                    for group in &groups {
                        let gt = format!("{tail}.{}", idname(group));
                        let mut p = states[si];
                        let ret = p.set_group_bits(agent, group, set, bit);
                        s.emit(
                            &format!("c3.setgroup.{gt}"),
                            "ret+perm",
                            &ret_perm_cols(ret, &p),
                        );
                        let mut p = states[si];
                        let ret = p.set_everyone_bits(agent, group, set, bit);
                        s.emit(
                            &format!("c3.seteveryone.{gt}"),
                            "ret+perm",
                            &ret_perm_cols(ret, &p),
                        );
                        let mut p = states[si];
                        let ret = p.set_next_owner_bits(agent, group, set, bit);
                        s.emit(
                            &format!("c3.setnext.{gt}"),
                            "ret+perm",
                            &ret_perm_cols(ret, &p),
                        );
                    }
                }
            }
        }
    }
    // Every state: queries, setLastOwner, set(), operators, the serialized forms.
    for (si, p) in states.iter().enumerate() {
        let cs = format!("c3.state.{si}");
        s.emit(&cs, "in", &perm_cols(p));
        s.emit(&cs, "queries", &perm_queries(p));
        if let Some(owner) = p.safe_owner() {
            s.emit(&cs, "safe_owner", &[uuid(&owner)]);
        }
        let mut q = *p;
        q.set_last_owner(&id_h());
        s.emit(&cs, "set_last_owner", &perm_cols(&q));
        let mut r = Permissions::default();
        r.set(p);
        s.emit(&cs, "set", &perm_cols(&r));
        s.emit(
            &cs,
            "eq",
            &[bl(r == *p), bl(r != *p), bl(q == *p), bl(q != *p)],
        );
        // Legacy text.
        let mut q = states[0];
        let (text, ok, stream) = legacy_round_trip(p, &mut q);
        s.emit(&cs, "legacy", &[esc(&text)]);
        s.emit(&cs, "legacy.reimport", &ret_perm_cols(ok, &q));
        s.emit(&cs, "legacy.reimport.stream", &stream);
        // LLSD.
        let sd = p.to_llsd();
        s.emit_llsd(&cs, "llsd", &sd);
        s.emit(
            &cs,
            "llsd.reimport",
            &perm_cols(&Permissions::from_llsd(&sd)),
        );
        let mut filled = Value::map();
        if let Value::Map(m) = &mut filled {
            m.insert(b"preexisting".to_vec(), Value::Integer(1));
        }
        p.fill_llsd(&mut filled);
        s.emit(&cs, "llsd.fill.bin", &[llsd_bin(&filled)]);
        let mut q = states[5];
        q.import_llsd(&sd);
        s.emit(&cs, "llsd.import_over5", &perm_cols(&q));
        // The hyphenated LLSD message forms.
        let pm = p.pack_message_llsd();
        s.emit(&cs, "packmessage.bin", &[llsd_bin(&pm)]);
        let mut q = states[3];
        q.unpack_message_llsd(&pm);
        s.emit(&cs, "unpackmessage", &perm_cols(&q));
    }
    // importLLSD of non-maps.
    let non_maps = [
        Value::Undefined,
        Value::Integer(5),
        Value::string("x"),
        Value::array(),
    ];
    for (i, v) in non_maps.iter().enumerate() {
        let mut q = states[5];
        q.import_llsd(v);
        s.emit(&format!("c3.importllsd.nonmap.{i}"), "perm", &perm_cols(&q));
    }
    // LLAggregatePermissions::aggregate(mask) sequences.
    let agg_masks = [0, T_, M_, C_, T_ | M_ | C_, PERM_ALL];
    let agg_cols = |a: &AggregatePermissions| {
        vec![
            x2(u32::from(a.to_u8())),
            d(a.value(PERM_COPY)),
            d(a.value(PERM_MODIFY)),
            d(a.value(PERM_TRANSFER)),
            d(a.value(PERM_MOVE)),
            bl(a.is_empty()),
        ]
    };
    s.emit(
        "c3.agg.seq",
        "agg",
        &agg_cols(&AggregatePermissions::default()),
    );
    s.emit(
        "c3.agg.empty",
        "agg",
        &agg_cols(&AggregatePermissions::EMPTY),
    );
    for &m1 in &agg_masks {
        let mut a1 = AggregatePermissions::default();
        a1.aggregate_mask(m1);
        s.emit(&format!("c3.agg.seq.{}", x8(m1)), "agg", &agg_cols(&a1));
        for &m2 in &agg_masks {
            let mut a2 = a1;
            a2.aggregate_mask(m2);
            s.emit(
                &format!("c3.agg.seq.{}.{}", x8(m1), x8(m2)),
                "agg",
                &agg_cols(&a2),
            );
            for &m3 in &agg_masks {
                let mut a3 = a2;
                a3.aggregate_mask(m3);
                s.emit(
                    &format!("c3.agg.seq.{}.{}.{}", x8(m1), x8(m2), x8(m3)),
                    "agg",
                    &agg_cols(&a3),
                );
            }
        }
    }
    s
}

#[test]
fn c3_permissions_replay() {
    let fixture = "c3_permissions.txt";
    let expected = read_fixture(fixture);
    assert_eq!(expected.len(), 3463, "{fixture}: data line count");
    assert_replay(fixture, &expected, &c3_lines().0);
}

// ---------------------------------------------------------------------------
// c3r: the random stream
// ---------------------------------------------------------------------------

const C3R_SEED: u64 = 0x7633_0000_0000_0001;
const C3R_CASES: u64 = 200_000;

fn draw_id(r: &mut Rng) -> Uuid {
    match r.below(4) {
        0 => NUL,
        1 => id_a(),
        2 => id_b(),
        _ => id_g(),
    }
}

fn draw_mask(r: &mut Rng) -> u32 {
    let v = r.next();
    if v & 1 != 0 {
        MASK_EDGES[((v >> 1) % MASK_EDGES.len() as u64) as usize]
    } else {
        (v >> 32) as u32
    }
}

fn draw_perm(r: &mut Rng) -> Permissions {
    let creator = draw_id(r);
    let owner = draw_id(r);
    let last_owner = draw_id(r);
    let group = draw_id(r);
    let base = draw_mask(r);
    let owner_mask = draw_mask(r);
    let group_mask = draw_mask(r);
    let everyone = draw_mask(r);
    let next = draw_mask(r);
    let group_owned = r.coin();
    Permissions {
        creator,
        owner,
        last_owner,
        group,
        base,
        owner_mask,
        group_mask,
        everyone,
        next,
        group_owned,
    }
}

const DRAW_BITS: [u32; 10] = [
    T_,
    M_,
    C_,
    V_,
    X_,
    0x00000001,
    0x80000000,
    T_ | M_ | C_,
    PERM_ALL,
    0xffffffff,
];
const C3_OP_NAMES: [&str; 12] = [
    "accumulate",
    "setOwnerAndGroup",
    "deedToGroup",
    "setBaseBits",
    "setOwnerBits",
    "setGroupBits",
    "setEveryoneBits",
    "setNextOwnerBits",
    "initMasks5",
    "fix",
    "importLLSD",
    "legacy",
];

#[derive(Default)]
struct C3In {
    st: Permissions,
    op: u32,
    other: Permissions,
    a1: Uuid,
    a2: Uuid,
    a3: Uuid,
    flag: bool,
    bits: u32,
    masks: [u32; 5],
}

fn draw_c3(r: &mut Rng) -> C3In {
    let mut c = C3In {
        st: draw_perm(r),
        ..C3In::default()
    };
    c.op = r.below(12);
    match c.op {
        0 => c.other = draw_perm(r),
        1 => {
            c.a1 = draw_id(r);
            c.a2 = draw_id(r);
            c.a3 = draw_id(r);
            c.flag = r.coin();
        }
        2 => {
            c.a1 = draw_id(r);
            c.a2 = draw_id(r);
        }
        3 | 4 => {
            c.a1 = draw_id(r);
            c.flag = r.coin();
            c.bits = DRAW_BITS[r.below(10) as usize];
        }
        5..=7 => {
            c.a1 = draw_id(r);
            c.a2 = draw_id(r);
            c.flag = r.coin();
            c.bits = DRAW_BITS[r.below(10) as usize];
        }
        8 => {
            for m in c.masks.iter_mut() {
                *m = draw_mask(r);
            }
        }
        _ => {}
    }
    c
}

/// `c3r_case<I>()`.
fn c3r_case(i: u64, c: &C3In, s: &mut Lines) {
    let cs = format!("c3r.{i}");
    let mut p = c.st;
    s.emit(&cs, "in", &perm_cols(&p));
    let mut ret = true;
    let flag = bl(c.flag);
    let mut args: Vec<String> = match c.op {
        0 => {
            p.accumulate(&c.other);
            perm_cols(&c.other)
        }
        1 => {
            ret = p.set_owner_and_group(&c.a1, &c.a2, &c.a3, c.flag);
            vec![idname(&c.a1), idname(&c.a2), idname(&c.a3), flag]
        }
        2 => {
            ret = p.deed_to_group(&c.a1, &c.a2);
            vec![idname(&c.a1), idname(&c.a2)]
        }
        3 | 4 => {
            ret = if c.op == 3 {
                p.set_base_bits(&c.a1, c.flag, c.bits)
            } else {
                p.set_owner_bits(&c.a1, c.flag, c.bits)
            };
            vec![idname(&c.a1), flag, x8(c.bits)]
        }
        5..=7 => {
            ret = match c.op {
                5 => p.set_group_bits(&c.a1, &c.a2, c.flag, c.bits),
                6 => p.set_everyone_bits(&c.a1, &c.a2, c.flag, c.bits),
                _ => p.set_next_owner_bits(&c.a1, &c.a2, c.flag, c.bits),
            };
            vec![idname(&c.a1), idname(&c.a2), flag, x8(c.bits)]
        }
        8 => {
            let m = c.masks;
            p.init_masks(m[0], m[1], m[2], m[3], m[4]);
            m.iter().map(|&v| x8(v)).collect()
        }
        9 => {
            p.fix();
            Vec::new()
        }
        10 => {
            let sd = p.to_llsd();
            p = Permissions::from_llsd(&sd);
            vec![llsd_bin(&sd)]
        }
        _ => {
            let mut q = Permissions::default();
            let (text, ok, _) = legacy_round_trip(&p, &mut q);
            ret = ok;
            p = q;
            vec![esc(&text)]
        }
    };
    args.insert(0, C3_OP_NAMES[c.op as usize].to_owned());
    s.emit(&cs, "op", &args);
    s.emit(&cs, "out", &ret_perm_cols(ret, &p));
    s.emit(&cs, "queries", &perm_queries(&p));
}

#[test]
fn c3_random_stream() {
    let start = Instant::now();
    let mut rng = Rng::new(C3R_SEED);
    let mut digest = ChunkedDigest::new("c3r", 1000);
    let mut sample = Vec::new();
    for i in 0..C3R_CASES {
        digest.begin_case(i);
        let c = draw_c3(&mut rng);
        let mut lines = Lines::default();
        c3r_case(i, &c, &mut lines);
        for line in &lines.0 {
            digest.line(line);
        }
        if sampled(i) {
            sample.extend(lines.0);
        }
    }
    let entries = digest.finish();
    let fixture = "c3_random_sample.txt";
    assert_replay(fixture, &read_fixture(fixture), &sample);
    assert_digests("c3r", &entries);
    println!("c3r: {C3R_CASES} cases in {:.1?}", start.elapsed());
}

// ---------------------------------------------------------------------------
// c3x: the exhaustive sweeps
// ---------------------------------------------------------------------------

fn alpha5(i: u32) -> u32 {
    [T_, M_, C_, V_, X_]
        .iter()
        .enumerate()
        .filter(|(k, _)| i & (1 << k) != 0)
        .fold(0, |m, (_, b)| m | b)
}

fn alpha4(i: u32) -> u32 {
    [T_, C_, 0x00000001, 0x80000000]
        .iter()
        .enumerate()
        .filter(|(k, _)| i & (1 << k) != 0)
        .fold(0, |m, (_, b)| m | b)
}

fn alpha7(i: u32) -> u32 {
    alpha5(i) | if i & 32 != 0 { 1 } else { 0 } | if i & 64 != 0 { 0x80000000 } else { 0 }
}

fn masks_le(p: &Permissions, out: &mut Vec<u8>) {
    for m in [p.base, p.owner_mask, p.group_mask, p.everyone, p.next] {
        out.extend_from_slice(&m.to_le_bytes());
    }
}

/// `c3x_fix(i, go, alphabet)`.
fn c3x_fix(i: u32, go: bool, alphabet: u32, table: &[u32], out: &mut Vec<u8>) {
    let bits = alphabet;
    let m = (1u32 << bits) - 1;
    let mut p = Permissions::default();
    p.base = table[(i & m) as usize];
    p.owner_mask = table[((i >> bits) & m) as usize];
    p.group_mask = table[((i >> (2 * bits)) & m) as usize];
    p.everyone = table[((i >> (3 * bits)) & m) as usize];
    p.next = table[((i >> (4 * bits)) & m) as usize];
    p.yes_really_set_owner(&NULL_ID, go);
    p.fix();
    masks_le(&p, out);
}

/// `c3x_fair(i)`.
fn c3x_fair(i: u32, table: &[u32], out: &mut Vec<u8>) {
    const OTHERS: [u32; 5] = [PERM_ALL, 0, T_ | C_, M_ | V_, 0xffffffff];
    let k = (i >> 14) as usize;
    let mut p = Permissions::default();
    p.init_masks(
        table[(i & 127) as usize],
        OTHERS[k % 5],
        OTHERS[(k / 5) % 5],
        OTHERS[(k / 25) % 5],
        table[((i >> 7) & 127) as usize],
    );
    masks_le(&p, out);
}

const EXHAUSTIVE_CHUNK: u64 = 1 << 20;
const C3X_FAIR_CASES: u64 = 128 * 128 * 125;

/// One sweep: its records hashed whole and per chunk of 2^20 cases, as the
/// recorder's `ChunkedDigest` (one record of 20 bytes per case). The work is
/// pipelined over three threads: while this one extends the whole-stream
/// digest with a chunk, one thread hashes the same chunk alone and another
/// builds the next chunk's records.
fn sweep(name: &str, cases: u64, case: &(dyn Fn(u32, &mut Vec<u8>) + Sync)) -> Vec<DigestEntry> {
    let build = |start: u64| {
        let end = (start + EXHAUSTIVE_CHUNK).min(cases);
        let mut buf = Vec::with_capacity(((end - start) * 20) as usize);
        for i in start..end {
            case(i as u32, &mut buf);
        }
        buf
    };
    let mut total = Sha256::default();
    let mut entries = Vec::new();
    let mut buf = build(0);
    let mut start = 0u64;
    while start < cases {
        let end = (start + EXHAUSTIVE_CHUNK).min(cases);
        let (part, next) = std::thread::scope(|scope| {
            let part = scope.spawn(|| sha256_hex(&buf));
            let next = (end < cases).then(|| scope.spawn(move || build(end)));
            total.update(&buf);
            (
                part.join().expect("chunk digest thread"),
                next.map(|h| h.join().expect("chunk build thread")),
            )
        });
        entries.push([
            format!("{name}@{start}-{end}"),
            (end - start).to_string(),
            (end - start).to_string(),
            part,
        ]);
        buf = next.unwrap_or_default();
        start = end;
    }
    entries.insert(
        0,
        [
            name.to_owned(),
            cases.to_string(),
            cases.to_string(),
            total.hex(),
        ],
    );
    entries
}

#[test]
fn c3_exhaustive_sweeps() {
    let start = Instant::now();
    let t5: Vec<u32> = (0..32).map(alpha5).collect();
    let t4: Vec<u32> = (0..16).map(alpha4).collect();
    let t7: Vec<u32> = (0..128).map(alpha7).collect();
    let results: Vec<(String, Vec<DigestEntry>)> = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for go in [false, true] {
            let (t5, t4) = (&t5, &t4);
            let n5 = format!("c3x.fix.alpha5.go{}", u8::from(go));
            handles.push(scope.spawn(move || {
                let e = sweep(&n5, 1 << 25, &|i, out| c3x_fix(i, go, 5, t5, out));
                (n5, e)
            }));
            let n4 = format!("c3x.fix.alpha4.go{}", u8::from(go));
            handles.push(scope.spawn(move || {
                let e = sweep(&n4, 1 << 20, &|i, out| c3x_fix(i, go, 4, t4, out));
                (n4, e)
            }));
        }
        let t7 = &t7;
        handles.push(scope.spawn(move || {
            let e = sweep("c3x.initmasks5", C3X_FAIR_CASES, &|i, out| {
                c3x_fair(i, t7, out)
            });
            ("c3x.initmasks5".to_owned(), e)
        }));
        handles
            .into_iter()
            .map(|h| h.join().expect("sweep thread"))
            .collect()
    });
    let mut report = String::new();
    let (mut ok, mut n) = (0, 0);
    for (name, entries) in &results {
        let (k, r) = check_digests(name, entries);
        println!("{name}: {k}/{} digest entries match", entries.len());
        ok += k;
        n += entries.len();
        report.push_str(&r);
    }
    println!(
        "c3x: {ok}/{n} digest entries match in {:.1?}",
        start.elapsed()
    );
    assert!(report.is_empty(), "{report}");
}
