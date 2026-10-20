//! The viewer's `llsdserialize_test.cpp` (TUT) cases, as Rust tests: the XML
//! formatter expectations, the round trips through every format, and the
//! notation, binary and XML parsing cases with their counts and failures.

use std::collections::BTreeMap;

use vkstorm_llsd::{Format, Limits, Options, PARSE_FAILURE, Value, format, parse};

fn xml(v: &Value) -> String {
    String::from_utf8(format(Format::Xml, v, Options::default()).0).unwrap()
}

fn map(entries: &[(&str, Value)]) -> Value {
    let mut m = BTreeMap::new();
    for (k, v) in entries {
        m.insert(k.as_bytes().to_vec(), v.clone());
    }
    Value::Map(m)
}

fn uuid(s: &str) -> Value {
    Value::Uuid(vkstorm_llsd::text::uuid_set(s.as_bytes()))
}

fn date(s: &str) -> Value {
    Value::Date(vkstorm_llsd::text::date_parse(s.as_bytes()).unwrap())
}

fn uri(s: &str) -> Value {
    Value::Uri(s.as_bytes().to_vec())
}

fn binary(s: &[u8]) -> Value {
    Value::Binary(s.to_vec())
}

// ---- LLSDXMLFormatter -------------------------------------------------------

#[test]
fn xml_formatter_atoms() {
    assert_eq!(xml(&Value::Undefined), "<llsd><undef /></llsd>\n");
    assert_eq!(
        xml(&Value::Integer(3463)),
        "<llsd><integer>3463</integer></llsd>\n"
    );
    assert_eq!(xml(&Value::string("")), "<llsd><string /></llsd>\n");
    assert_eq!(
        xml(&Value::string("foobar")),
        "<llsd><string>foobar</string></llsd>\n"
    );
    assert_eq!(xml(&Value::Uuid([0; 16])), "<llsd><uuid /></llsd>\n");
    assert_eq!(
        xml(&uuid("c96f9b1e-f589-4100-9774-d98643ce0bed")),
        "<llsd><uuid>c96f9b1e-f589-4100-9774-d98643ce0bed</uuid></llsd>\n"
    );
    assert_eq!(
        xml(&uri("https://secondlife.com/login")),
        "<llsd><uri>https://secondlife.com/login</uri></llsd>\n"
    );
    assert_eq!(
        xml(&date("2006-04-24T16:11:33Z")),
        "<llsd><date>2006-04-24T16:11:33Z</date></llsd>\n"
    );
    assert_eq!(
        xml(&binary(b"hello")),
        "<llsd><binary encoding=\"base64\">aGVsbG8=</binary></llsd>\n"
    );
}

#[test]
fn xml_formatter_booleans() {
    let alpha = Options {
        boolalpha: true,
        ..Options::default()
    };
    let s = |v: bool, o: Options| {
        String::from_utf8(format(Format::Xml, &Value::Boolean(v), o).0).unwrap()
    };
    assert_eq!(s(true, alpha), "<llsd><boolean>true</boolean></llsd>\n");
    assert_eq!(s(false, alpha), "<llsd><boolean>false</boolean></llsd>\n");
    assert_eq!(
        s(true, Options::default()),
        "<llsd><boolean>1</boolean></llsd>\n"
    );
    assert_eq!(
        s(false, Options::default()),
        "<llsd><boolean>0</boolean></llsd>\n"
    );
}

#[test]
fn xml_formatter_real_format() {
    // realFormat("%.2f") etc.: the caller pre-formats; here through the
    // formatter's real_text path.
    let mut out = Vec::new();
    {
        let mut f = vkstorm_llsd::Formatter::new(Format::Xml, Options::default(), &mut out);
        f.real_text(1.0, b"1.00");
        f.finish();
    }
    assert_eq!(out, b"<llsd><real>1.00</real></llsd>\n");
    assert_eq!(xml(&Value::Real(1.0)), "<llsd><real>1</real></llsd>\n");
    assert_eq!(
        xml(&Value::Real(-34379.0438)),
        "<llsd><real>-34379.04379999999946448952</real></llsd>\n"
    );
}

#[test]
fn xml_formatter_containers() {
    assert_eq!(xml(&Value::array()), "<llsd><array /></llsd>\n");
    assert_eq!(
        xml(&Value::Array(vec![Value::Undefined])),
        "<llsd><array><undef /></array></llsd>\n"
    );
    assert_eq!(
        xml(&Value::Array(vec![Value::Undefined, Value::Integer(1)])),
        "<llsd><array><undef /><integer>1</integer></array></llsd>\n"
    );
    assert_eq!(xml(&Value::map()), "<llsd><map /></llsd>\n");
    assert_eq!(
        xml(&map(&[("foo", Value::string("bar"))])),
        "<llsd><map><key>foo</key><string>bar</string></map></llsd>\n"
    );
    assert_eq!(
        xml(&map(&[
            ("foo", Value::string("bar")),
            ("baz", Value::Undefined)
        ])),
        "<llsd><map><key>baz</key><undef /><key>foo</key><string>bar</string></map></llsd>\n"
    );
}

#[test]
fn xml_formatter_pretty() {
    let v = map(&[("a", Value::Array(vec![Value::Integer(1)])), ("b", Value::string("c"))]);
    let out = format(Format::Xml, &v, Options { pretty: true, ..Options::default() }).0;
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "<llsd>
    <map>
    <key>a</key>
        <array>
            <integer>1</integer>
        </array>
    <key>b</key>
        <string>c</string>
    </map>
</llsd>
"
    );
}

#[test]
fn xml_formatter_binary_blob() {
    let blob = b"6|6|asdfhappybox|60e44ec5-305c-43c2-9a19-b4b89b1ae2a6|60e44ec5-305c-43c2-9a19-b4b89b1ae2a6|60e44ec5-305c-43c2-9a19-b4b89b1ae2a6|00000000-0000-0000-0000-000000000000|7fffffff|7fffffff|0|0|82000|450fe394-2904-c9ad-214c-a07eb7feec29|(No Description)|0|10|0";
    assert_eq!(
        xml(&binary(blob)),
        "<llsd><binary encoding=\"base64\">Nnw2fGFzZGZoYXBweWJveHw2MGU0NGVjNS0zMDVjLTQzYzItOWExOS1iNGI4OWIxYWUyYTZ8NjBlNDRlYzUtMzA1Yy00M2MyLTlhMTktYjRiODliMWFlMmE2fDYwZTQ0ZWM1LTMwNWMtNDNjMi05YTE5LWI0Yjg5YjFhZTJhNnwwMDAwMDAwMC0wMDAwLTAwMDAtMDAwMC0wMDAwMDAwMDAwMDB8N2ZmZmZmZmZ8N2ZmZmZmZmZ8MHwwfDgyMDAwfDQ1MGZlMzk0LTI5MDQtYzlhZC0yMTRjLWEwN2ViN2ZlZWMyOXwoTm8gRGVzY3JpcHRpb24pfDB8MTB8MA==</binary></llsd>\n"
    );
}

// ---- round trips ---------------------------------------------------------

fn unicode_blocks() -> Vec<Value> {
    let mut out = Vec::new();
    let block_size = 0x20u32;
    let mut block = 0u32;
    while block <= 0x10ffff {
        let mut s = Vec::new();
        for c in block..block + block_size {
            if c <= 0x1f && c != 0x9 && c != 0xa {
                continue;
            }
            if (0xd800..=0xdfff).contains(&c)
                || (0xfdd0..=0xfdef).contains(&c)
                || (c & 0xfffe) == 0xfffe
            {
                continue;
            }
            vkstorm_llsd_push_utf8(c, &mut s);
        }
        out.push(Value::String(s));
        block += block_size;
    }
    out
}

fn vkstorm_llsd_push_utf8(c: u32, out: &mut Vec<u8>) {
    if c <= 0x7f {
        out.push(c as u8);
    } else if c <= 0x7ff {
        out.push(0xc0 | ((c >> 6) & 0x1f) as u8);
        out.push(0x80 | (c & 0x3f) as u8);
    } else if c <= 0xffff {
        out.push(0xe0 | ((c >> 12) & 0x0f) as u8);
        out.push(0x80 | ((c >> 6) & 0x3f) as u8);
        out.push(0x80 | (c & 0x3f) as u8);
    } else {
        out.push(0xf0 | ((c >> 18) & 0x07) as u8);
        out.push(0x80 | ((c >> 12) & 0x3f) as u8);
        out.push(0x80 | ((c >> 6) & 0x3f) as u8);
        out.push(0x80 | (c & 0x3f) as u8);
    }
}

fn fillmap(width: u32, depth: u32) -> Value {
    if depth == 0 {
        return map(&[("foo", Value::string("bar"))]);
    }
    let mut m = BTreeMap::new();
    for i in 0..width {
        m.insert(format!("child {i}").into_bytes(), fillmap(width, depth - 1));
    }
    Value::Map(m)
}

fn round_trip_values() -> Vec<(&'static str, Value)> {
    let long = "Second Life is a 3-D virtual world entirely built and owned by its residents. Since opening to the public in 2003, it has grown explosively and today is inhabited by nearly 100,000 people from around the globe.\n\nFrom the moment you enter the World you'll discover a vast digital continent, teeming with people, entertainment, experiences and opportunity. Once you've explored a bit, perhaps you'll find a perfect parcel of land to build your house or business.\n\nYou'll also be surrounded by the Creations of your fellow residents. Because residents retain the rights to their digital creations, they can buy, sell and trade with other residents.\n\nThe Marketplace currently supports millions of US dollars in monthly transactions. This commerce is handled with the in-world currency, the Linden dollar, which can be converted to US dollars at several thriving online currency exchanges.\n\nWelcome to Second Life. We look forward to seeing you in-world!\n";
    let mut v: Vec<(&str, Value)> = vec![
        ("undefined", Value::Undefined),
        ("true bool", Value::Boolean(true)),
        ("false bool", Value::Boolean(false)),
        ("positive int", Value::Integer(1)),
        ("zero int", Value::Integer(0)),
        ("negative int", Value::Integer(-1)),
        ("positive float", Value::Real(1234.5)),
        ("zero float", Value::Real(0.0)),
        ("negative float", Value::Real(-1234.5)),
        ("null uuid", Value::Uuid([0; 16])),
        ("new uuid", uuid("1e6a4e1b-2c3d-4e5f-8a9b-0c1d2e3f4a5b")),
        ("empty string", Value::string("")),
        ("non-empty string", Value::string("some string")),
        ("long string", Value::string(long)),
        ("epoch date", Value::Date(0.0)),
        ("date", date("2002-12-07T05:07:15.00Z")),
        (
            "url",
            uri("http://slurl.com/secondlife/Ambleside/57/104/26/"),
        ),
        (
            "binary",
            Value::Binary(b"it must be a blue moon again\0".to_vec()),
        ),
        ("empty map", Value::map()),
        (
            "map",
            map(&[("name", Value::string("luke")), ("age", Value::Integer(3))]),
        ),
        (
            "nested maps",
            map(&[
                ("a", map(&[("1", Value::Boolean(true))])),
                ("b", map(&[("0", Value::Boolean(false))])),
            ]),
        ),
        ("empty array", Value::array()),
        (
            "array",
            Value::Array(vec![Value::string("ali"), Value::Integer(28)]),
        ),
        (
            "nested arrays",
            Value::Array(vec![
                Value::Array(vec![Value::Boolean(true)]),
                Value::Array(vec![Value::Boolean(false)]),
            ]),
        ),
        ("many nested maps", fillmap(10, 3)),
    ];
    for s in unicode_blocks() {
        v.push(("unicode block", s));
    }
    v
}

fn check_round_trip(fmt: Format, opts: Options, header: bool) {
    for (name, v) in round_trip_values() {
        let (mut bytes, count) = format(fmt, &v, opts);
        assert!(count >= 1, "{name}: count");
        if header {
            let h = match fmt {
                Format::Binary => "<? LLSD/Binary ?>\n",
                Format::Xml => "<? LLSD/XML ?>\n",
                Format::Notation => "<? llsd/notation ?>\n",
            };
            let mut with = h.as_bytes().to_vec();
            with.append(&mut bytes);
            bytes = with;
            // deserialize() strips the header; emulate that here.
            let nl = bytes.iter().position(|&b| b == b'\n').unwrap();
            bytes.drain(..=nl);
        }
        let (n, parsed) = parse(fmt, &bytes, Limits::for_len(bytes.len()));
        assert_eq!(
            n,
            count,
            "{name}: parse count ({:?})",
            String::from_utf8_lossy(&bytes[..bytes.len().min(80)])
        );
        assert_eq!(parsed.as_ref(), Some(&v), "{name}: value");
    }
}

#[test]
fn round_trip_pretty_binary_notation() {
    check_round_trip(
        Format::Notation,
        Options {
            pretty_binary: true,
            ..Options::default()
        },
        false,
    );
}

#[test]
fn round_trip_raw_binary_notation() {
    check_round_trip(
        Format::Notation,
        Options {
            pretty_binary: false,
            ..Options::default()
        },
        false,
    );
}

#[test]
fn round_trip_pretty_notation() {
    check_round_trip(
        Format::Notation,
        Options {
            pretty: true,
            pretty_binary: true,
            ..Options::default()
        },
        false,
    );
}

#[test]
fn round_trip_xml() {
    check_round_trip(Format::Xml, Options::default(), false);
    check_round_trip(
        Format::Xml,
        Options {
            pretty: true,
            ..Options::default()
        },
        false,
    );
}

#[test]
fn round_trip_binary() {
    check_round_trip(Format::Binary, Options::default(), false);
}

#[test]
fn round_trip_with_headers() {
    check_round_trip(Format::Binary, Options::default(), true);
    check_round_trip(Format::Xml, Options::default(), true);
    check_round_trip(Format::Notation, Options::default(), true);
}

// ---- XML parsing -------------------------------------------------------------

fn ensure_parse(
    fmt: Format,
    msg: &str,
    input: &[u8],
    expected: Option<Value>,
    count: i32,
    depth: i32,
) {
    let (n, v) = parse(
        fmt,
        input,
        Limits {
            max_bytes: Some(input.len() as i64),
            max_depth: depth,
        },
    );
    assert_eq!(n, count, "{msg} (count)");
    assert_eq!(v, expected, "{msg}");
}

fn xml_case(msg: &str, input: &str, expected: Option<Value>, count: i32) {
    ensure_parse(Format::Xml, msg, input.as_bytes(), expected, count, -1);
}

#[test]
fn xml_parse_not_llsd() {
    xml_case(
        "malformed xml",
        "<llsd><string>ha ha</string>",
        None,
        PARSE_FAILURE,
    );
    xml_case(
        "not llsd",
        "<html><body><p>ha ha</p></body></html>",
        None,
        PARSE_FAILURE,
    );
    xml_case(
        "value without llsd",
        "<string>ha ha</string>",
        None,
        PARSE_FAILURE,
    );
    xml_case("key without llsd", "<key>ha ha</key>", None, PARSE_FAILURE);
}

#[test]
fn xml_parse_unknown_type() {
    let v = map(&[
        ("amy", Value::Integer(23)),
        ("bob", Value::Undefined),
        ("cam", Value::Real(1.23)),
    ]);
    xml_case(
        "unknown data type",
        "<llsd><map><key>amy</key><integer>23</integer><key>bob</key><bigint>99999999999999999</bigint><key>cam</key><real>1.23</real></map></llsd>",
        Some(v),
        4,
    );
}

#[test]
fn xml_parse_nested_bad_data() {
    let v = map(&[("amy", Value::Integer(23)), ("cam", Value::Real(1.23))]);
    xml_case(
        "map with html",
        "<llsd><map><key>amy</key><integer>23</integer><html><body>ha ha</body></html><key>cam</key><real>1.23</real></map></llsd>",
        Some(v.clone()),
        3,
    );
    xml_case(
        "map with value for key",
        "<llsd><map><key>amy</key><integer>23</integer><string>ha ha</string><key>cam</key><real>1.23</real></map></llsd>",
        Some(v),
        3,
    );
    let v = map(&[
        ("amy", Value::Integer(23)),
        ("bob", Value::map()),
        ("cam", Value::Real(1.23)),
    ]);
    xml_case(
        "map with map of html",
        "<llsd><map><key>amy</key><integer>23</integer><key>bob</key><map><html><body>ha ha</body></html></map><key>cam</key><real>1.23</real></map></llsd>",
        Some(v),
        4,
    );
    let v = Value::Array(vec![
        Value::Integer(23),
        Value::Undefined,
        Value::Real(1.23),
    ]);
    xml_case(
        "array value of html",
        "<llsd><array><integer>23</integer><html><body>ha ha</body></html><real>1.23</real></array></llsd>",
        Some(v),
        4,
    );
    let v = Value::Array(vec![Value::Integer(23), Value::map(), Value::Real(1.23)]);
    xml_case(
        "array with map of html",
        "<llsd><array><integer>23</integer><map><html><body>ha ha</body></html></map><real>1.23</real></array></llsd>",
        Some(v),
        4,
    );
}

#[test]
fn xml_parse_binary() {
    xml_case(
        "hello",
        "<llsd><binary encoding=\"base64\">aGVsbG8=</binary></llsd>\n",
        Some(binary(b"hello")),
        1,
    );
    let blob = b"6|6|asdfhappybox|60e44ec5-305c-43c2-9a19-b4b89b1ae2a6|60e44ec5-305c-43c2-9a19-b4b89b1ae2a6|60e44ec5-305c-43c2-9a19-b4b89b1ae2a6|00000000-0000-0000-0000-000000000000|7fffffff|7fffffff|0|0|82000|450fe394-2904-c9ad-214c-a07eb7feec29|(No Description)|0|10|0";
    xml_case(
        "blob",
        "<llsd><binary encoding=\"base64\">Nnw2fGFzZGZoYXBweWJveHw2MGU0NGVjNS0zMDVjLTQzYzItOWExOS1iNGI4OWIxYWUyYTZ8NjBlNDRlYzUtMzA1Yy00M2MyLTlhMTktYjRiODliMWFlMmE2fDYwZTQ0ZWM1LTMwNWMtNDNjMi05YTE5LWI0Yjg5YjFhZTJhNnwwMDAwMDAwMC0wMDAwLTAwMDAtMDAwMC0wMDAwMDAwMDAwMDB8N2ZmZmZmZmZ8N2ZmZmZmZmZ8MHwwfDgyMDAwfDQ1MGZlMzk0LTI5MDQtYzlhZC0yMTRjLWEwN2ViN2ZlZWMyOXwoTm8gRGVzY3JpcHRpb24pfDB8MTB8MA==</binary></llsd>\n",
        Some(binary(blob)),
        1,
    );
    xml_case(
        "blob with line breaks",
        "<llsd><binary encoding=\"base64\">Nnw2fGFzZGZoYXBweWJveHw2MGU0NGVjNS0zMDVjLTQzYzItOWExOS1iNGI4OWIxYWUyYTZ8NjBl\nNDRlYzUtMzA1Yy00M2MyLTlhMTktYjRiODliMWFlMmE2fDYwZTQ0ZWM1LTMwNWMtNDNjMi05YTE5\nLWI0Yjg5YjFhZTJhNnwwMDAwMDAwMC0wMDAwLTAwMDAtMDAwMC0wMDAwMDAwMDAwMDB8N2ZmZmZm\nZmZ8N2ZmZmZmZmZ8MHwwfDgyMDAwfDQ1MGZlMzk0LTI5MDQtYzlhZC0yMTRjLWEwN2ViN2ZlZWMy\nOXwoTm8gRGVzY3JpcHRpb24pfDB8MTB8MA==</binary></llsd>\n",
        Some(binary(blob)),
        1,
    );
}

#[test]
fn xml_parse_deep() {
    let mut v = map(&[("level_5", Value::Real(42.0))]);
    for i in (0..5).rev() {
        v = map(&[(Box::leak(format!("level_{i}").into_boxed_str()), v)]);
    }
    let v = map(&[("deep", v)]);
    xml_case(
        "deep llsd xml map",
        "<llsd><map><key>deep</key><map><key>level_0</key><map><key>level_1</key><map><key>level_2</key><map><key>level_3</key><map><key>level_4</key><map><key>level_5</key><real>42.0</real></map></map></map></map></map></map></map></llsd>",
        Some(v),
        8,
    );
}

// ---- notation parsing ------------------------------------------------------------

fn notation_case(msg: &str, input: &str, expected: Option<Value>, count: i32) {
    ensure_parse(Format::Notation, msg, input.as_bytes(), expected, count, -1);
}

#[test]
fn notation_parse_malformed() {
    notation_case("malformed notation map", "{'ha ha'", None, PARSE_FAILURE);
    notation_case("malformed notation array", "['ha ha'", None, PARSE_FAILURE);
    notation_case("malformed notation string", "'ha ha", None, PARSE_FAILURE);
    notation_case("bad notation noise", "g48ejlnfr", None, PARSE_FAILURE);
}

#[test]
fn notation_parse_scalars() {
    notation_case("valid undef", "!", Some(Value::Undefined), 1);
    for s in ["false", "f", "0", "F", "FALSE"] {
        notation_case("valid boolean false", s, Some(Value::Boolean(false)), 1);
    }
    for s in ["true", "t", "1", "T", "TRUE"] {
        notation_case("valid boolean true", s, Some(Value::Boolean(true)), 1);
    }
    notation_case("invalid true", "TR", None, PARSE_FAILURE);
    notation_case("invalid false", "FAL", None, PARSE_FAILURE);
    notation_case("valid integer", "i123", Some(Value::Integer(123)), 1);
    notation_case("invalid integer", "421", None, PARSE_FAILURE);
    notation_case("valid real", "r456.7", Some(Value::Real(456.7)), 1);
    notation_case("invalid real", "456.7", None, PARSE_FAILURE);
    notation_case("unparseable uuid", "u123", None, PARSE_FAILURE);
    notation_case(
        "valid uuid",
        "u1e6a4e1b-2c3d-4e5f-8a9b-0c1d2e3f4a5b",
        Some(uuid("1e6a4e1b-2c3d-4e5f-8a9b-0c1d2e3f4a5b")),
        1,
    );
    notation_case(
        "valid string 1",
        "\"foolish\"",
        Some(Value::string("foolish")),
        1,
    );
    notation_case(
        "valid string 2",
        "\"g'day\"",
        Some(Value::string("g'day")),
        1,
    );
    notation_case(
        "valid string 3",
        "'have a \"nice\" day'",
        Some(Value::string("have a \"nice\" day")),
        1,
    );
    notation_case(
        "valid string 4",
        "s(8)\"whatever\"",
        Some(Value::string("whatever")),
        1,
    );
    notation_case("invalid string 1", "s(7)\"whatever\"", None, PARSE_FAILURE);
    notation_case("invalid string 2", "s(9)\"whatever\"", None, PARSE_FAILURE);
    notation_case(
        "valid uri",
        "l\"http://www.google.com\"",
        Some(uri("http://www.google.com")),
        1,
    );
    notation_case(
        "valid date",
        "d\"2007-12-28T09:22:53.10Z\"",
        Some(date("2007-12-28T09:22:53.10Z")),
        1,
    );
    notation_case(
        "valid binary b64",
        "b64\"YWJjMzIx\"",
        Some(binary(b"abc321")),
        1,
    );
    notation_case(
        "valid binary b16",
        "b16\"616263333231\"",
        Some(binary(b"abc321")),
        1,
    );
    notation_case(
        "valid binary raw",
        "b(6)\"abc321\"",
        Some(binary(b"abc321")),
        1,
    );
    notation_case(
        "binary length too long",
        "b(7)\"abc321\"",
        None,
        PARSE_FAILURE,
    );
    notation_case(
        "binary length way too long",
        "b(1000000)\"abc321\"",
        None,
        PARSE_FAILURE,
    );
    notation_case(
        "size longer than bytes left",
        "s(5)\"hi\"",
        None,
        PARSE_FAILURE,
    );
    notation_case(
        "binary size longer than bytes left",
        "b(5)\"hi\"",
        None,
        PARSE_FAILURE,
    );
    notation_case("empty input", "", None, 0);
    notation_case("whitespace input", "  \n", None, 0);
}

#[test]
fn notation_parse_containers() {
    let v = map(&[
        ("amy", Value::Integer(23)),
        ("bob", Value::Undefined),
        ("cam", Value::Real(1.23)),
    ]);
    notation_case("simple map", "{'amy':i23,'bob':!,'cam':r1.23}", Some(v), 4);
    let v = map(&[
        ("amy", Value::Integer(23)),
        ("bob", map(&[("vehicle", Value::string("bicycle"))])),
        ("cam", Value::Real(1.23)),
    ]);
    notation_case(
        "nested map",
        "{'amy':i23,'bob':{'vehicle':'bicycle'},'cam':r1.23}",
        Some(v),
        5,
    );
    let v = Value::Array(vec![
        Value::Integer(23),
        Value::Undefined,
        Value::Real(1.23),
    ]);
    notation_case("simple array", "[i23,!,r1.23]", Some(v), 4);
    let v = Value::Array(vec![
        Value::Integer(23),
        Value::Array(vec![Value::string("bicycle")]),
        Value::Real(1.23),
    ]);
    notation_case("nested array", "[i23,['bicycle'],r1.23]", Some(v), 5);
    let v = map(&[
        ("amy", Value::Integer(23)),
        (
            "bob",
            map(&[(
                "dogs",
                Value::Array(vec![
                    map(&[
                        ("name", Value::string("groove")),
                        ("breed", Value::string("samoyed")),
                    ]),
                    map(&[
                        ("name", Value::string("greyley")),
                        ("breed", Value::string("chow/husky")),
                    ]),
                ]),
            )]),
        ),
        ("cam", Value::Real(1.23)),
    ]);
    notation_case(
        "nested notation",
        "{'amy':i23, 'bob':{'dogs':[{'name':'groove', 'breed':'samoyed'},{'name':'greyley', 'breed':'chow/husky'}]}, 'cam':r1.23}",
        Some(v),
        11,
    );
}

#[test]
fn notation_parse_depth() {
    let v = map(&[(
        "deep",
        map(&[(
            "level_0",
            map(&[("level_1", map(&[("level_2", Value::Integer(99))]))]),
        )]),
    )]);
    ensure_parse(
        Format::Notation,
        "nested notation 3 deep",
        b"{'deep' : {'level_0':{'level_1':{'level_2': i99} } } }",
        Some(v),
        5,
        5,
    );
    let mut v = map(&[("level_9", Value::Integer(99))]);
    for i in (0..9).rev() {
        v = map(&[(Box::leak(format!("level_{i}").into_boxed_str()), v)]);
    }
    let v = map(&[("deep", v)]);
    let ten = "{'deep' : {'level_0':{'level_1':{'level_2':{'level_3':{'level_4':{'level_5':{'level_6':{'level_7':{'level_8':{'level_9':i99}} } } } } } } } } }";
    ensure_parse(
        Format::Notation,
        "nested notation 10 deep",
        ten.as_bytes(),
        Some(v),
        12,
        15,
    );
    ensure_parse(
        Format::Notation,
        "nested notation 10 deep, limit 9",
        ten.as_bytes(),
        None,
        PARSE_FAILURE,
        9,
    );

    let mut v = map(&[("end", Value::Integer(99))]);
    for i in (0..50).rev() {
        v = map(&[(Box::leak(format!("level_{i}").into_boxed_str()), v)]);
    }
    let v = map(&[("deep", v)]);
    let mut text = String::from("{'deep':");
    for i in 0..50 {
        text.push_str(&format!("{{'level_{i}' :"));
    }
    text.push_str("{'end':i99}");
    for _ in 0..51 {
        text.push_str("} ");
    }
    ensure_parse(
        Format::Notation,
        "nested notation deep",
        text.as_bytes(),
        Some(v),
        53,
        -1,
    );
}

// ---- binary parsing --------------------------------------------------------

fn binary_case(msg: &str, input: &[u8], expected: Option<Value>, count: i32) {
    ensure_parse(Format::Binary, msg, input, expected, count, -1);
}

fn be(n: u32) -> [u8; 4] {
    n.to_be_bytes()
}

#[test]
fn binary_parse_strings_and_blobs() {
    let mut v = b"s".to_vec();
    v.extend_from_slice(&be(6));
    v.extend_from_slice(b"abc321");
    binary_case("correct string parse", &v, Some(Value::string("abc321")), 1);
    v[1..5].copy_from_slice(&be(7));
    binary_case("incorrect size string parse", &v, None, PARSE_FAILURE);
    v[1..5].copy_from_slice(&be(100000));
    binary_case("incorrect size string parse 2", &v, None, PARSE_FAILURE);

    let mut v = b"b".to_vec();
    v.extend_from_slice(&be(6));
    v.extend_from_slice(b"abc321");
    binary_case("correct binary parse", &v, Some(binary(b"abc321")), 1);
    v[1..5].copy_from_slice(&be(7));
    binary_case("incorrect size binary parse 1", &v, None, PARSE_FAILURE);
    v[1..5].copy_from_slice(&be(100000));
    binary_case("incorrect size binary parse 2", &v, None, PARSE_FAILURE);
}

#[test]
fn binary_parse_malformed_and_scalars() {
    binary_case("malformed binary map", b"{'ha ha'", None, PARSE_FAILURE);
    binary_case("malformed binary array", b"['ha ha'", None, PARSE_FAILURE);
    binary_case("malformed binary string", b"'ha ha", None, PARSE_FAILURE);
    binary_case("bad noise", b"g48ejlnfr", None, PARSE_FAILURE);
    binary_case("valid undef", b"!", Some(Value::Undefined), 1);
    binary_case("valid boolean false", b"0", Some(Value::Boolean(false)), 1);
    binary_case("valid boolean true", b"1", Some(Value::Boolean(true)), 1);
    binary_case("invalid true", b"t", None, PARSE_FAILURE);
    binary_case("invalid false", b"f", None, PARSE_FAILURE);
}

#[test]
fn binary_parse_map() {
    let mut v = b"{".to_vec();
    v.extend_from_slice(&be(1));
    v.push(b'k');
    let key_size_loc = v.len();
    v.extend_from_slice(&be(1));
    v.extend_from_slice(b"amy");
    v.push(b'i');
    v.extend_from_slice(&be(23));
    binary_case("invalid key size", &v, None, PARSE_FAILURE);
    v[key_size_loc..key_size_loc + 4].copy_from_slice(&be(3));
    binary_case("valid key size, unterminated map", &v, None, PARSE_FAILURE);
    v.push(b'}');
    binary_case(
        "valid map",
        &v,
        Some(map(&[("amy", Value::Integer(23))])),
        2,
    );
    v[1..5].copy_from_slice(&be(0));
    binary_case("invalid map too long", &v, None, PARSE_FAILURE);
    v[1..5].copy_from_slice(&be(2));
    binary_case("invalid map too short", &v, None, PARSE_FAILURE);
}

#[test]
fn binary_parse_array() {
    let mut v = b"[".to_vec();
    v.extend_from_slice(&be(1));
    v.extend_from_slice(b"\"amy\"i");
    v.extend_from_slice(&be(23));
    binary_case("invalid array size", &v, None, PARSE_FAILURE);
    v[1..5].copy_from_slice(&be(2));
    binary_case("unterminated array", &v, None, PARSE_FAILURE);
    v.push(b']');
    binary_case(
        "valid array",
        &v,
        Some(Value::Array(vec![Value::string("amy"), Value::Integer(23)])),
        3,
    );
    v[1..5].copy_from_slice(&be(3));
    binary_case("array too short", &v, None, PARSE_FAILURE);
}

#[test]
fn binary_parse_empty_containers_and_uri() {
    let mut v = b"{".to_vec();
    v.extend_from_slice(&be(0));
    v.push(b'}');
    binary_case("empty map", &v, Some(Value::map()), 1);
    let mut v = b"[".to_vec();
    v.extend_from_slice(&be(0));
    v.push(b']');
    binary_case("empty array", &v, Some(Value::array()), 1);
    let mut v = b"l".to_vec();
    v.extend_from_slice(&be(14));
    v.extend_from_slice(b"http://sl.com");
    binary_case("invalid uri length size", &v, None, PARSE_FAILURE);
    v[1..5].copy_from_slice(&be(13));
    binary_case("valid uri", &v, Some(uri("http://sl.com")), 1);
}

// ---- cross-format compatibility --------------------------------------------

#[test]
fn cross_compatible() {
    let mut blob = Vec::new();
    let mut seed = 1u32;
    for _ in 0..100 {
        let size = (seed % 100 + 10) as usize;
        for _ in 0..size {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            blob.push((seed >> 16) as u8);
        }
    }
    let cases = vec![
        Value::Undefined,
        Value::Boolean(true),
        Value::Boolean(false),
        Value::Integer(0),
        Value::Integer(1),
        Value::Integer(-234567),
        Value::Real(0.0),
        Value::Real(1.0),
        Value::Real(-1.0),
        Value::string("foobar"),
        uuid("1e6a4e1b-2c3d-4e5f-8a9b-0c1d2e3f4a5b"),
        Value::Date(12345.0),
        uri("http://www.secondlife.com/"),
        Value::Binary(blob),
        Value::Array(vec![Value::Integer(1), Value::string("hello")]),
        map(&[("foo", Value::string("bar")), ("baz", Value::Integer(100))]),
    ];
    for v in cases {
        let (bin, c1) = format(Format::Binary, &v, Options::default());
        let (c2, from_bin) = parse(Format::Binary, &bin, Limits::UNLIMITED);
        assert_eq!(c1, c2);
        let from_bin = from_bin.unwrap();
        let (notation, c3) = format(
            Format::Notation,
            &from_bin,
            Options {
                pretty_binary: false,
                ..Options::default()
            },
        );
        assert_eq!(c3, c2);
        let (c4, from_notation) = parse(Format::Notation, &notation, Limits::UNLIMITED);
        assert_eq!(c4, c3);
        assert_eq!(from_notation.unwrap(), v);
        let (x, c5) = format(Format::Xml, &from_bin, Options::default());
        assert_eq!(c5, c2);
        let (c6, from_xml) = parse(Format::Xml, &x, Limits::UNLIMITED);
        assert_eq!(c6, c5);
        assert_eq!(from_xml.unwrap(), v);
    }
}
