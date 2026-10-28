//! C2 normalisers: regenerates every line of `fixtures/model/c2_normalisers.txt`
//! from the crate, as `c2_normalisers()` in `llinventory_tool.cpp` writes it
//! from the frozen C++, and compares them line by line.

mod common;

use common::*;
use vkstorm_inventory::text;

const FIXTURE: &str = "c2_normalisers.txt";

/// `c2_inputs()`, byte for byte.
fn c2_inputs() -> Vec<Vec<u8>> {
    let cat = |parts: &[&[u8]]| parts.concat();
    let mut v: Vec<Vec<u8>> = Vec::new();
    for b in 0..=255u8 {
        let c: &[u8] = &[b];
        v.push(c.to_vec());
        v.push(cat(&[c, b"abc"]));
        v.push(cat(&[b"abc", c]));
        v.push(cat(&[b"ab", c, b"c"]));
    }
    let pattern = b"Abcdefghij klmnopqrst UVWXYZ0123 456789-_+= ";
    let patterned = |n: usize| -> Vec<u8> { (0..n).map(|i| pattern[i % pattern.len()]).collect() };
    for n in (0..=70).chain(120..=130).chain(250..=260) {
        v.push(patterned(n));
    }
    let ws: [&[u8]; 6] = [b" ", b"\t", b"\n", b"\x0b", b"\x0c", b"\r"];
    for a in ws {
        for b in ws {
            v.push(cat(&[a, b"x y", b]));
            v.push(cat(&[a, b, b"x", b, a]));
        }
    }
    // 63/64 bytes with spaces around the cut.
    v.push(cat(&[&[b'a'; 62], b" b"]));
    v.push(cat(&[&[b'a'; 63], b" "]));
    v.push(cat(&[b" ", &[b'a'; 63]]));
    v.push(cat(&[&[b'a'; 60], b"   ", b"bbbb"]));
    v.push(vec![b' '; 64]);
    v.push(vec![b'|'; 70]);
    // UTF-8 and friends.
    let utf: [&[u8]; 13] = [
        b"caf\xc3\xa9",
        b"\xc3\xa9t\xc3\xa9",
        b"\xe2\x82\xac5",
        b"\xe6\x97\xa5\xe6\x9c\xac",
        b"\xf0\x9f\x98\x80 smile",
        b"a\xf0\x9f\x8f\xb3\xef\xb8\x8f\xe2\x80\x8d\xf0\x9f\x8c\x88z",
        b"\xc3",
        b"\xe2\x82",
        b"\xff\xfe",
        b"\xed\xa0\x80",
        b"\xc0\xaf",
        b"\x80",
        b"\xbf\xbf",
    ];
    v.extend(utf.iter().map(|x| x.to_vec()));
    v.push(vec![0xc3; 30]);
    v.push(b"a\0b".to_vec());
    v.push(b"\0".to_vec());
    v.push(b"name\0".to_vec());
    for i in 0..=10 {
        let mut s = b"0123456789".to_vec();
        s.insert(i, b'|');
        v.push(s);
    }
    for s in [&b"|"[..], b"||", b" | ", b"line1\r\nline2", b"line1\nline2\n", b"\r\n", b"del\x7f", b"\x7f", b"tab\tinside"] {
        v.push(s.to_vec());
    }
    v.push(cat(&[&[0xc3; 63], b"x"]));
    v.push(vec![0x80; 70]);
    v
}

/// `c2_normalisers<I>()` over the crate.
fn c2_lines() -> Lines {
    let mut s = Lines::default();
    for (i, input) in c2_inputs().iter().enumerate() {
        let cs = format!("c2.{i}");

        let name = text::correct_inventory_name(input);
        let desc = text::correct_inventory_description(input);
        let mut rna = input.clone();
        text::replace_nonstandard_ascii(&mut rna, b' ');
        let mut rna_pipe = rna.clone();
        text::replace_char(&mut rna_pipe, b'|', b' ');
        // rename() on an object named "prev".
        let prev: &[u8] = b"prev";
        let renamed = text::rename(prev, input).unwrap_or_else(|| prev.to_vec());
        // The item constructor: the object constructor's name, and the
        // description's two replacements.
        let ctor_name = text::correct_inventory_name(input);
        let ctor_desc = text::correct_inventory_description(input);
        let prev_desc: &[u8] = b"prev desc";
        let set_desc = text::set_description(prev_desc, input).unwrap_or_else(|| prev_desc.to_vec());
        // The category constructor: the object constructor's name.
        let cat_name = text::correct_inventory_name(input);

        s.emit(&cs, "in", &[esc(input)]);
        s.emit(
            &cs,
            "out",
            &[
                esc(&name),
                esc(&desc),
                esc(&rna_pipe),
                esc(&rna),
                esc(&renamed),
                esc(&ctor_name),
                esc(&ctor_desc),
                esc(&set_desc),
                esc(&cat_name),
            ],
        );
    }
    s
}

#[test]
fn c2_normalisers_replay() {
    let expected = read_fixture(FIXTURE);
    assert_eq!(expected.len(), 2 * 1234, "{FIXTURE}: data line count");
    let actual = c2_lines();
    assert_replay(FIXTURE, &expected, &actual.0);
}
