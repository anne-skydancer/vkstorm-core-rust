//! C4 sale info: regenerates every line of `fixtures/model/c4_sale.txt` and of
//! the `c4r` random-stream sample from the crate, as `c4_sale()` and
//! `c4r_case()` in `llinventory_tool.cpp` write them from the frozen C++, and
//! checks the `c4r` stream against its chunked SHA-256 digests.

mod common;

use std::time::Instant;

use common::*;
use vkstorm_inventory::legacy::reader::Stream;
use vkstorm_inventory::sale::{SaleInfo, SaleType};
use vkstorm_llsd::Value;

/// `make_sale`: the raw fields.
fn raw(t: i32, p: i32) -> SaleInfo {
    SaleInfo { sale_type: SaleType(t), price: p }
}

const SALE_TYPES: [i32; 8] = [0, 1, 2, 3, -1, 4, 200, 255];
const PRICES: [i32; 5] = [i32::MIN, -1, 0, 10, i32::MAX];

/// `sale_cols` + ok + has + perm_mask.
fn sale_pm_cols(s: &SaleInfo, ok: Option<bool>, has: bool, pm: u32) -> Vec<String> {
    let mut v = sale_cols(s);
    if let Some(ok) = ok {
        v.push(bl(ok));
    }
    v.push(bl(has));
    v.push(x8(pm));
    v
}

fn map(entries: &[(&str, Value)]) -> Value {
    let mut sd = Value::map();
    if let Value::Map(m) = &mut sd {
        for (k, v) in entries {
            m.insert(k.as_bytes().to_vec(), v.clone());
        }
    }
    sd
}

/// `c4_sale<I>()` over the crate.
fn c4_lines() -> Lines {
    let mut s = Lines::default();
    let def = SaleInfo::default();
    s.emit("c4.default", "sale", &sale_cols(&def));
    s.emit("c4.default", "static", &sale_cols(&SaleInfo::DEFAULT));
    s.emit("c4.default", "forsale", &[bl(def.is_for_sale())]);
    for t in SALE_TYPES {
        for p in PRICES {
            let cs = format!("c4.tp.{t}.{p}");
            let r = raw(t, p);
            s.emit(&cs, "raw", &sale_cols(&r));
            s.emit(&cs, "crc", &[x8(r.crc32()), bl(r.is_for_sale())]);
            s.emit(&cs, "ctor", &sale_cols(&SaleInfo::new(SaleType(t), p)));
            let mut setp = SaleInfo::default();
            setp.set_sale_type(SaleType(t));
            setp.set_sale_price(p);
            s.emit(&cs, "setters", &sale_cols(&setp));
            let mut text = Vec::new();
            r.export_legacy_stream(&mut text);
            s.emit(&cs, "legacy", &[esc(&text)]);
            if (0..=3).contains(&t) {
                let body = after_first_line(&text);
                let mut is = Stream::new(body);
                let mut q = raw(1, 77);
                let (mut has, mut pm) = (true, 0xa5a5a5a5u32);
                let ok = q.import_legacy_stream(&mut is, &mut has, &mut pm);
                s.emit(&cs, "legacy.reimport", &sale_pm_cols(&q, Some(ok), has, pm));
                s.emit(&cs, "legacy.reimport.stream", &stream_cols(&is, body));
            } else {
                // The writer never fails: the stream stays good (bug 1).
                s.emit(&cs, "legacy.outcome", &["good"]);
            }
            let sd = r.to_llsd();
            s.emit_llsd(&cs, "llsd", &sd);
            s.emit(&cs, "llsd.operator.bin", &[llsd_bin(&r.to_llsd())]);
            let mut q = raw(1, 77);
            let (mut has, mut pm) = (false, 0xa5a5a5a5u32);
            let ok = q.from_llsd(&sd, &mut has, &mut pm);
            s.emit(&cs, "llsd.reimport", &sale_pm_cols(&q, Some(ok), has, pm));
            let csd = r.to_llsd();
            s.emit(&cs, "sdcreate.bin", &[llsd_bin(&csd)]);
            s.emit(&cs, "sdcreate.reimport", &sale_cols(&SaleInfo::from_sd(&csd)));
            let pmsg = r.pack_message_llsd();
            s.emit(&cs, "packmessage.bin", &[llsd_bin(&pmsg)]);
            let mut q = raw(1, 77);
            q.unpack_message_llsd(&pmsg);
            s.emit(&cs, "unpackmessage", &sale_cols(&q));
        }
    }
    // accumulate, with the price overflow (runtime edge 4: wraps).
    let acc = [(0, 10), (1, 20), (1, 1), (2, 0), (3, i32::MAX), (200, 5), (1, i32::MAX), (0, i32::MIN), (0, -1)];
    for (a, &(ta, pa)) in acc.iter().enumerate() {
        for (b, &(tb, pb)) in acc.iter().enumerate() {
            let mut x = raw(ta, pa);
            x.accumulate(&raw(tb, pb));
            let y = raw(tb, pb);
            s.emit(&format!("c4.acc.{a}.{b}"), "sale", &[d(x.sale_type.0), d(x.price), bl(x == y), bl(x != y)]);
        }
    }
    // fromLLSD over value types.
    let b4 = Value::Binary(vec![0x80, 0, 0x40, 0x01]);
    let types: Vec<(&str, Option<Value>)> = vec![
        ("absent", None),
        ("undef", Some(Value::Undefined)),
        ("str.not", Some(Value::string("not"))),
        ("str.orig", Some(Value::string("orig"))),
        ("str.copy", Some(Value::string("copy"))),
        ("str.cntn", Some(Value::string("cntn"))),
        ("str.junk", Some(Value::string("junk"))),
        ("str.empty", Some(Value::string(""))),
        ("str.COPY", Some(Value::string("COPY"))),
        ("int.2", Some(Value::Integer(2))),
        ("int.-1", Some(Value::Integer(-1))),
        ("int.255", Some(Value::Integer(255))),
        ("int.256", Some(Value::Integer(256))),
        ("int.258", Some(Value::Integer(258))),
        ("real.2.7", Some(Value::Real(2.7))),
        ("bool.true", Some(Value::Boolean(true))),
        ("uuid", Some(Value::Uuid(u(0x55)))),
        ("bin4", Some(b4.clone())),
        ("map", Some(Value::map())),
        ("array", Some(Value::array())),
    ];
    let prices: Vec<(&str, Option<Value>)> = vec![
        ("absent", None),
        ("int.50", Some(Value::Integer(50))),
        ("int.-5", Some(Value::Integer(-5))),
        ("int.max", Some(Value::Integer(i32::MAX))),
        ("int.min", Some(Value::Integer(i32::MIN))),
        ("real.12.9", Some(Value::Real(12.9))),
        ("real.-3.5", Some(Value::Real(-3.5))),
        ("real.1e12", Some(Value::Real(1e12))),
        ("str.42", Some(Value::string("42"))),
        ("str.junk", Some(Value::string("junk"))),
        ("str.-7", Some(Value::string("-7"))),
        ("bool", Some(Value::Boolean(true))),
        ("bin4", Some(b4.clone())),
    ];
    for (tn, tv) in &types {
        for (pn, pv) in &prices {
            let mut entries = Vec::new();
            if let Some(v) = tv {
                entries.push(("sale_type", v.clone()));
            }
            if let Some(v) = pv {
                entries.push(("sale_price", v.clone()));
            }
            let sd = map(&entries);
            let mut q = raw(2, 77);
            let (mut has, mut pm) = (false, 0xa5a5a5a5u32);
            q.from_llsd(&sd, &mut has, &mut pm);
            let cs = format!("c4.fromllsd.{tn}.{pn}");
            s.emit(&cs, "sale+perm_mask", &sale_pm_cols(&q, None, has, pm));
            s.emit(&cs, "sdfrom", &sale_cols(&SaleInfo::from_sd(&sd)));
        }
    }
    let pms: [(&str, Option<Value>); 8] = [
        ("absent", None),
        ("bin0", Some(Value::Binary(vec![]))),
        ("bin3", Some(Value::Binary(vec![1, 2, 3]))),
        ("bin4", Some(b4)),
        ("bin5", Some(Value::Binary(vec![0x00, 0x00, 0xe0, 0x00, 0xff]))),
        ("int", Some(Value::Integer(0x8000))),
        ("str", Some(Value::string("8000"))),
        ("undef", Some(Value::Undefined)),
    ];
    for (name, v) in &pms {
        let mut entries = vec![("sale_type", Value::string("copy")), ("sale_price", Value::Integer(5))];
        if let Some(v) = v {
            entries.push(("perm_mask", v.clone()));
        }
        let sd = map(&entries);
        let mut q = raw(2, 77);
        let (mut has, mut pm) = (false, 0xa5a5a5a5u32);
        q.from_llsd(&sd, &mut has, &mut pm);
        s.emit(&format!("c4.permmask.{name}"), "sale+perm_mask", &sale_pm_cols(&q, None, has, pm));
    }
    let non_maps = [Value::Undefined, Value::Integer(3), Value::string("copy"), Value::array()];
    for (i, v) in non_maps.iter().enumerate() {
        let mut q = raw(2, 77);
        let (mut has, mut pm) = (false, 0xa5a5a5a5u32);
        q.from_llsd(v, &mut has, &mut pm);
        s.emit(&format!("c4.fromllsd.nonmap.{i}"), "sale+perm_mask", &sale_pm_cols(&q, None, has, pm));
    }
    s
}

#[test]
fn c4_sale_replay() {
    let fixture = "c4_sale.txt";
    let expected = read_fixture(fixture);
    assert_eq!(expected.len(), 1196, "{fixture}: data line count");
    assert_replay(fixture, &expected, &c4_lines().0);
}

// ---------------------------------------------------------------------------
// c4r: the random stream
// ---------------------------------------------------------------------------

const C4R_SEED: u64 = 0x7634_0000_0000_0001;
const C4R_CASES: u64 = 100_000;

fn draw_sale_type(r: &mut Rng) -> i32 {
    const T: [i32; 14] = [0, 1, 2, 3, 0, 1, 2, 3, -1, 4, 200, 255, 127, -128];
    T[r.below(14) as usize]
}

fn draw_price(r: &mut Rng) -> i32 {
    let v = r.next();
    if v & 1 != 0 {
        PRICES[((v >> 1) % 5) as usize]
    } else if v & 2 != 0 {
        ((v >> 8) % 100_000) as i32
    } else {
        (v >> 32) as u32 as i32
    }
}

struct C4In {
    type1: i32,
    price1: i32,
    op: u32,
    type2: i32,
    price2: i32,
    sd_kind: u32,
}

fn draw_c4(r: &mut Rng) -> C4In {
    let type1 = draw_sale_type(r);
    let price1 = draw_price(r);
    let op = r.below(4);
    let type2 = draw_sale_type(r);
    let price2 = draw_price(r);
    let sd_kind = r.below(3);
    C4In { type1, price1, op, type2, price2, sd_kind }
}

/// `c4r_case<I>()`.
fn c4r_case(i: u64, c: &C4In, s: &mut Lines) {
    let cs = format!("c4r.{i}");
    let mut a = raw(c.type1, c.price1);
    s.emit(
        &cs,
        "in",
        &[d(c.type1), d(c.price1), d(c.op), d(c.type2), d(c.price2), d(c.sd_kind)],
    );
    s.emit(&cs, "crc", &[x8(a.crc32()), bl(a.is_for_sale())]);
    match c.op {
        0 => {
            // A sum outside S32 is not drawn as a value (runtime edge 4).
            if i32::try_from(i64::from(c.price1) + i64::from(c.price2)).is_ok() {
                a.accumulate(&raw(c.type2, c.price2));
                s.emit(&cs, "accumulate", &sale_cols(&a));
            } else {
                s.emit(&cs, "accumulate", &["overflow"]);
            }
        }
        1 => {
            let sd = a.to_llsd();
            s.emit(&cs, "llsd.bin", &[llsd_bin(&sd)]);
            let mut q = raw(c.type2, c.price2);
            let (mut has, mut pm) = (false, 0u32);
            q.from_llsd(&sd, &mut has, &mut pm);
            s.emit(&cs, "llsd.reimport", &sale_cols(&q));
        }
        2 => {
            let mut entries = Vec::new();
            match c.sd_kind {
                0 => entries.push(("sale_type", Value::Integer(c.type2))),
                1 => {
                    let name = match c.type2 {
                        1 => "orig",
                        2 => "copy",
                        3 => "cntn",
                        _ => "x",
                    };
                    entries.push(("sale_type", Value::string(name)));
                }
                _ => {}
            }
            entries.push(("sale_price", Value::Integer(c.price2)));
            let sd = map(&entries);
            let mut q = raw(c.type1, c.price1);
            let (mut has, mut pm) = (false, 0u32);
            q.from_llsd(&sd, &mut has, &mut pm);
            s.emit(&cs, "fromllsd", &sale_cols(&q));
        }
        _ => {
            let mut q = SaleInfo::default();
            q.set_sale_type(SaleType(c.type2));
            q.set_sale_price(c.price2);
            s.emit(
                &cs,
                "setters",
                &[d(q.sale_type.0), d(q.price), bl(q == a), bl(q != a), x8(q.crc32())],
            );
        }
    }
}

#[test]
fn c4_random_stream() {
    let start = Instant::now();
    let mut rng = Rng::new(C4R_SEED);
    let mut digest = ChunkedDigest::new("c4r", 1000);
    let mut sample = Vec::new();
    for i in 0..C4R_CASES {
        digest.begin_case(i);
        let c = draw_c4(&mut rng);
        let mut lines = Lines::default();
        c4r_case(i, &c, &mut lines);
        for line in &lines.0 {
            digest.line(line);
        }
        if sampled(i) {
            sample.extend(lines.0);
        }
    }
    let entries = digest.finish();
    let fixture = "c4_random_sample.txt";
    assert_replay(fixture, &read_fixture(fixture), &sample);
    assert_digests("c4r", &entries);
    println!("c4r: {C4R_CASES} cases in {:.1?}", start.elapsed());
}
