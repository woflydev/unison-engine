//! Zero-copy access to the top level of a gateway frame (`{op, d, s, t}`),
//! so the dispatcher can route on `t` and decode `d` straight into the right type.

use crate::{Error, Result, VERSION, tag};

/// A term's raw bytes (no version prefix), decodable with [`crate::from_term`].
#[derive(Clone, Copy, Debug)]
pub struct RawTerm<'a>(pub &'a [u8]);

pub struct Frame<'a> {
    pub op: u8,
    pub seq: Option<u64>,
    pub event: Option<&'a str>,
    pub data: Option<RawTerm<'a>>,
}

/// Decode a term without the leading version byte.
pub fn from_term<'de, T: serde::Deserialize<'de>>(term: RawTerm<'de>) -> Result<T> {
    let mut de = crate::Deserializer::from_raw(term.0);
    T::deserialize(&mut de)
}

pub fn split_frame(input: &[u8]) -> Result<Frame<'_>> {
    let body = match input.first() {
        Some(&VERSION) => &input[1..],
        Some(&v) => return Err(Error::Version(v)),
        None => return Err(Error::Eof),
    };
    if body.first() != Some(&tag::MAP) {
        return Err(Error::Custom("gateway frame is not a map".into()));
    }
    let n = u32::from_be_bytes(body.get(1..5).ok_or(Error::Eof)?.try_into().unwrap()) as usize;
    let mut pos = 5;
    let mut f = Frame { op: 255, seq: None, event: None, data: None };
    for _ in 0..n {
        let klen = term_len(&body[pos..])?;
        let key = key_name(&body[pos..pos + klen])?;
        pos += klen;
        let vlen = term_len(&body[pos..])?;
        let v = &body[pos..pos + vlen];
        match key {
            "op" => f.op = from_term::<u8>(RawTerm(v))?,
            "s" => f.seq = from_term::<Option<u64>>(RawTerm(v))?,
            "t" => f.event = from_term::<Option<&str>>(RawTerm(v))?,
            "d" => f.data = Some(RawTerm(v)),
            _ => {}
        }
        pos += vlen;
    }
    Ok(f)
}

fn key_name(b: &[u8]) -> Result<&str> {
    let s = match b[0] {
        tag::ATOM | tag::ATOM_UTF8 => &b[3..],
        tag::SMALL_ATOM | tag::SMALL_ATOM_UTF8 => &b[2..],
        tag::BINARY => &b[5..],
        _ => return Ok(""),
    };
    std::str::from_utf8(s).map_err(|_| Error::Utf8)
}

fn be32(b: &[u8], at: usize) -> Result<usize> {
    Ok(u32::from_be_bytes(b.get(at..at + 4).ok_or(Error::Eof)?.try_into().unwrap()) as usize)
}

fn be16(b: &[u8], at: usize) -> Result<usize> {
    Ok(u16::from_be_bytes(b.get(at..at + 2).ok_or(Error::Eof)?.try_into().unwrap()) as usize)
}

/// Byte length of the term at the start of `b`.
pub fn term_len(b: &[u8]) -> Result<usize> {
    let t = *b.first().ok_or(Error::Eof)?;
    let len = match t {
        tag::SMALL_INTEGER => 2,
        tag::INTEGER => 5,
        tag::NEW_FLOAT => 9,
        tag::FLOAT => 32,
        tag::NIL => 1,
        tag::ATOM | tag::ATOM_UTF8 => 3 + be16(b, 1)?,
        tag::STRING => 3 + be16(b, 1)?,
        tag::SMALL_ATOM | tag::SMALL_ATOM_UTF8 => 2 + *b.get(1).ok_or(Error::Eof)? as usize,
        tag::BINARY => 5 + be32(b, 1)?,
        tag::SMALL_BIG => 3 + *b.get(1).ok_or(Error::Eof)? as usize,
        tag::LARGE_BIG => 6 + be32(b, 1)?,
        tag::SMALL_TUPLE | tag::LARGE_TUPLE | tag::LIST | tag::MAP => {
            let (count, mut pos) = match t {
                tag::SMALL_TUPLE => (*b.get(1).ok_or(Error::Eof)? as usize, 2),
                _ => (be32(b, 1)?, 5),
            };
            let items = if t == tag::MAP { count * 2 } else { count };
            for _ in 0..items {
                pos += term_len(b.get(pos..).ok_or(Error::Eof)?)?;
            }
            if t == tag::LIST {
                pos += term_len(b.get(pos..).ok_or(Error::Eof)?)?;
            }
            pos
        }
        other => return Err(Error::Tag(other)),
    };
    if len > b.len() { Err(Error::Eof) } else { Ok(len) }
}
