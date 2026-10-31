use serde_json::Value;

use crate::{VERSION, tag};

/// Serialize any `Serialize` type the way `erlpack.pack()` would after a JSON round-trip.
pub fn to_vec<T: serde::Serialize>(value: &T) -> serde_json::Result<Vec<u8>> {
    Ok(encode_value(&serde_json::to_value(value)?))
}

/// Encode a JSON value with erlpack's mapping:
/// null → `nil` atom, bool → `true`/`false` atoms, string → BINARY_EXT,
/// integers → SMALL_INTEGER / INTEGER / SMALL_BIG, floats → NEW_FLOAT,
/// arrays → LIST (empty → NIL), objects → MAP with binary keys.
pub fn encode_value(value: &Value) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    out.push(VERSION);
    write(&mut out, value);
    out
}

fn atom(out: &mut Vec<u8>, name: &str) {
    out.push(tag::SMALL_ATOM_UTF8);
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
}

fn write(out: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Null => atom(out, "nil"),
        Value::Bool(b) => atom(out, if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                if u <= u8::MAX as u64 {
                    out.push(tag::SMALL_INTEGER);
                    out.push(u as u8);
                } else if u <= i32::MAX as u64 {
                    out.push(tag::INTEGER);
                    out.extend_from_slice(&(u as i32).to_be_bytes());
                } else {
                    big(out, 0, u);
                }
            } else if let Some(i) = n.as_i64() {
                if i >= i32::MIN as i64 {
                    out.push(tag::INTEGER);
                    out.extend_from_slice(&(i as i32).to_be_bytes());
                } else {
                    big(out, 1, i.unsigned_abs());
                }
            } else {
                out.push(tag::NEW_FLOAT);
                out.extend_from_slice(&n.as_f64().unwrap_or(0.0).to_be_bytes());
            }
        }
        Value::String(s) => binary(out, s.as_bytes()),
        Value::Array(items) => {
            if items.is_empty() {
                out.push(tag::NIL);
            } else {
                out.push(tag::LIST);
                out.extend_from_slice(&(items.len() as u32).to_be_bytes());
                for item in items {
                    write(out, item);
                }
                out.push(tag::NIL);
            }
        }
        Value::Object(map) => {
            out.push(tag::MAP);
            out.extend_from_slice(&(map.len() as u32).to_be_bytes());
            for (k, item) in map {
                binary(out, k.as_bytes());
                write(out, item);
            }
        }
    }
}

fn binary(out: &mut Vec<u8>, bytes: &[u8]) {
    out.push(tag::BINARY);
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn big(out: &mut Vec<u8>, sign: u8, mut v: u64) {
    let mut digits = Vec::with_capacity(8);
    while v > 0 {
        digits.push((v & 0xff) as u8);
        v >>= 8;
    }
    out.push(tag::SMALL_BIG);
    out.push(digits.len() as u8);
    out.push(sign);
    out.extend_from_slice(&digits);
}
