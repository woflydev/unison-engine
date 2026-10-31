use serde::Deserialize;
use serde_json::{Value, json};

use crate::{encode_value, from_slice};

#[test]
fn round_trips_json_shapes() {
    let v = json!({
        "op": 2,
        "d": {
            "token": "abc",
            "capabilities": 1734653,
            "big": 1_300_000_000_000_000_000u64,
            "neg": -5,
            "f": 1.5,
            "flag": true,
            "none": null,
            "list": [1, "two", {"three": 3}],
            "empty": []
        }
    });
    let bytes = encode_value(&v);
    let back: Value = from_slice(&bytes).unwrap();
    assert_eq!(back, v);
}

#[test]
fn snowflakes_decode_from_small_big_into_strings_or_ints() {
    #[derive(Deserialize)]
    struct M {
        #[serde(deserialize_with = "snow")]
        id: u64,
    }
    fn snow<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum S<'a> {
            N(u64),
            S(&'a str),
        }
        match S::deserialize(d)? {
            S::N(n) => Ok(n),
            S::S(s) => s.parse().map_err(serde::de::Error::custom),
        }
    }
    let bytes = encode_value(&json!({"id": 1_134_872_000_000_000_000u64}));
    assert_eq!(from_slice::<M>(&bytes).unwrap().id, 1_134_872_000_000_000_000);
    let bytes = encode_value(&json!({"id": "1134872000000000000"}));
    assert_eq!(from_slice::<M>(&bytes).unwrap().id, 1_134_872_000_000_000_000);
}

#[test]
fn nil_is_none_and_atom_keys_work() {
    // {seq: nil, t: nil} with atom keys, as Discord sends for HELLO.
    let mut b = vec![131, 116, 0, 0, 0, 2];
    for key in ["t", "s"] {
        b.extend_from_slice(&[100, 0, key.len() as u8]);
        b.extend_from_slice(key.as_bytes());
        b.extend_from_slice(&[100, 0, 3, b'n', b'i', b'l']);
    }
    #[derive(Deserialize)]
    struct F {
        t: Option<String>,
        s: Option<u64>,
    }
    let f: F = from_slice(&b).unwrap();
    assert!(f.t.is_none() && f.s.is_none());
}

#[test]
fn integer_map_keys_become_strings() {
    // {123: 1}
    let b = vec![131, 116, 0, 0, 0, 1, 97, 123, 97, 1];
    let m: std::collections::HashMap<String, u8> = from_slice(&b).unwrap();
    assert_eq!(m["123"], 1);
}

#[test]
fn splits_gateway_frame_without_decoding_d() {
    let bytes = encode_value(&json!({
        "t": "MESSAGE_CREATE",
        "s": 42,
        "op": 0,
        "d": {"id": 1_300_000_000_000_000_000u64, "content": "hi", "nested": [1, [2, 3]]}
    }));
    let f = crate::split_frame(&bytes).unwrap();
    assert_eq!((f.op, f.seq, f.event), (0, Some(42), Some("MESSAGE_CREATE")));
    let d: Value = crate::from_term(f.data.unwrap()).unwrap();
    assert_eq!(d["content"], "hi");
    assert_eq!(d["nested"][1][1], 3);
}

#[test]
fn tuples_and_partial_maps_keep_stream_aligned() {
    #[derive(Deserialize)]
    struct S {
        range: (u32, u32),
        after: String,
    }
    let bytes = encode_value(&json!({"range": [0, 99, 7], "after": "ok"}));
    let s: S = from_slice(&bytes).unwrap();
    assert_eq!((s.range, s.after.as_str()), ((0, 99), "ok"));
}

#[test]
fn rejects_bad_version() {
    assert!(from_slice::<Value>(&[130, 97, 1]).is_err());
}
