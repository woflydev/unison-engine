use serde::de::{self, DeserializeSeed, IntoDeserializer, MapAccess, SeqAccess, Visitor};

use crate::{Error, Result, VERSION, tag};

/// Deserialize a value from a complete ETF payload (leading version byte included).
pub fn from_slice<'de, T: de::Deserialize<'de>>(input: &'de [u8]) -> Result<T> {
    let mut de = Deserializer::new(input)?;
    T::deserialize(&mut de)
}

pub struct Deserializer<'de> {
    input: &'de [u8],
}

impl<'de> Deserializer<'de> {
    pub fn new(input: &'de [u8]) -> Result<Self> {
        match input.first() {
            Some(&VERSION) => Ok(Self { input: &input[1..] }),
            Some(&v) => Err(Error::Version(v)),
            None => Err(Error::Eof),
        }
    }

    /// Deserializer over a term with no version prefix (a sub-term of a frame).
    pub fn from_raw(input: &'de [u8]) -> Self {
        Self { input }
    }

    fn take(&mut self, n: usize) -> Result<&'de [u8]> {
        if self.input.len() < n {
            return Err(Error::Eof);
        }
        let (head, tail) = self.input.split_at(n);
        self.input = tail;
        Ok(head)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn peek(&self) -> Result<u8> {
        self.input.first().copied().ok_or(Error::Eof)
    }

    fn str(bytes: &[u8]) -> Result<&str> {
        std::str::from_utf8(bytes).map_err(|_| Error::Utf8)
    }

    /// Reads a bignum. Discord snowflakes arrive as SMALL_BIG_EXT (u64).
    fn big(&mut self, n: usize) -> Result<Big> {
        let sign = self.u8()?;
        let digits = self.take(n)?;
        if n > 8 && digits[8..].iter().any(|&b| b != 0) {
            return Err(Error::BigInt);
        }
        let mut v: u64 = 0;
        for (i, &b) in digits.iter().take(8).enumerate() {
            v |= (b as u64) << (8 * i);
        }
        Ok(if sign == 0 { Big::Pos(v) } else { Big::Neg(v) })
    }

    fn atom<V: Visitor<'de>>(name: &'de str, visitor: V) -> Result<V::Value> {
        match name {
            "nil" | "null" => visitor.visit_unit(),
            "true" => visitor.visit_bool(true),
            "false" => visitor.visit_bool(false),
            other => visitor.visit_borrowed_str(other),
        }
    }
}

enum Big {
    Pos(u64),
    Neg(u64),
}

impl<'de> de::Deserializer<'de> for &mut Deserializer<'de> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.u8()? {
            tag::SMALL_INTEGER => visitor.visit_u64(self.u8()? as u64),
            tag::INTEGER => visitor.visit_i64(self.u32()? as i32 as i64),
            tag::NEW_FLOAT => visitor.visit_f64(f64::from_be_bytes(self.take(8)?.try_into().unwrap())),
            tag::FLOAT => {
                let raw = Deserializer::str(self.take(31)?)?;
                let f: f64 = raw.trim_end_matches('\0').trim().parse().map_err(de::Error::custom)?;
                visitor.visit_f64(f)
            }
            tag::ATOM | tag::ATOM_UTF8 => {
                let n = self.u16()? as usize;
                let s = Deserializer::str(self.take(n)?)?;
                Deserializer::atom(s, visitor)
            }
            tag::SMALL_ATOM | tag::SMALL_ATOM_UTF8 => {
                let n = self.u8()? as usize;
                let s = Deserializer::str(self.take(n)?)?;
                Deserializer::atom(s, visitor)
            }
            tag::BINARY => {
                let n = self.u32()? as usize;
                let bytes = self.take(n)?;
                match std::str::from_utf8(bytes) {
                    Ok(s) => visitor.visit_borrowed_str(s),
                    Err(_) => visitor.visit_borrowed_bytes(bytes),
                }
            }
            tag::STRING => {
                // Erlang "string" is a list of bytes; erlpack emits it for short byte lists.
                let n = self.u16()? as usize;
                let bytes = self.take(n)?;
                visitor.visit_seq(ByteSeq { bytes, pos: 0 })
            }
            tag::NIL => visitor.visit_seq(Seq { de: self, left: 0, tail: false }),
            tag::LIST => {
                let n = self.u32()? as usize;
                seq(Seq { de: self, left: n, tail: true }, visitor)
            }
            tag::SMALL_TUPLE => {
                let n = self.u8()? as usize;
                seq(Seq { de: self, left: n, tail: false }, visitor)
            }
            tag::LARGE_TUPLE => {
                let n = self.u32()? as usize;
                seq(Seq { de: self, left: n, tail: false }, visitor)
            }
            tag::MAP => {
                let n = self.u32()? as usize;
                let mut map = Map { de: self, left: n };
                let v = visitor.visit_map(&mut map)?;
                // Visitors may stop early; skip any unread entries so the stream stays aligned.
                while map.next_key::<de::IgnoredAny>()?.is_some() {
                    map.next_value::<de::IgnoredAny>()?;
                }
                Ok(v)
            }
            tag::SMALL_BIG => {
                let n = self.u8()? as usize;
                match self.big(n)? {
                    Big::Pos(v) => visitor.visit_u64(v),
                    Big::Neg(v) => visitor.visit_i64(-(v as i64)),
                }
            }
            tag::LARGE_BIG => {
                let n = self.u32()? as usize;
                match self.big(n)? {
                    Big::Pos(v) => visitor.visit_u64(v),
                    Big::Neg(v) => visitor.visit_i64(-(v as i64)),
                }
            }
            t => Err(Error::Tag(t)),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        // `nil` atom is `None`; anything else is `Some`.
        let is_nil = match self.peek()? {
            tag::SMALL_ATOM | tag::SMALL_ATOM_UTF8 => self.input.get(1..5) == Some(&[3, b'n', b'i', b'l']),
            tag::ATOM | tag::ATOM_UTF8 => self.input.get(1..6) == Some(&[0, 3, b'n', b'i', b'l']),
            _ => false,
        };
        if is_nil {
            de::Deserializer::deserialize_any(self, de::IgnoredAny)?;
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        // Unit variants only (string-tagged enums); data-carrying enums use #[serde(tag)] maps.
        let s: String = de::Deserialize::deserialize(&mut *self)?;
        visitor.visit_enum(s.into_deserializer())
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value> {
        visitor.visit_newtype_struct(self)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct seq tuple
        tuple_struct map struct identifier ignored_any
    }
}

/// Visit a sequence, then consume whatever the visitor left (fixed-size tuples stop early,
/// leaving trailing elements and the list's NIL tail in the stream).
fn seq<'de, V: Visitor<'de>>(mut s: Seq<'_, 'de>, visitor: V) -> Result<V::Value> {
    let v = visitor.visit_seq(&mut s)?;
    while s.next_element::<de::IgnoredAny>()?.is_some() {}
    Ok(v)
}

struct Seq<'a, 'de> {
    de: &'a mut Deserializer<'de>,
    left: usize,
    /// Proper lists end in a NIL tail that must be consumed.
    tail: bool,
}

impl<'de> SeqAccess<'de> for Seq<'_, 'de> {
    type Error = Error;
    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        if self.left == 0 {
            if self.tail {
                self.tail = false;
                if self.de.peek()? == tag::NIL {
                    self.de.u8()?;
                } else {
                    de::Deserializer::deserialize_any(&mut *self.de, de::IgnoredAny)?;
                }
            }
            return Ok(None);
        }
        self.left -= 1;
        seed.deserialize(&mut *self.de).map(Some)
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.left)
    }
}

struct ByteSeq<'de> {
    bytes: &'de [u8],
    pos: usize,
}

impl<'de> SeqAccess<'de> for ByteSeq<'de> {
    type Error = Error;
    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        let Some(&b) = self.bytes.get(self.pos) else { return Ok(None) };
        self.pos += 1;
        seed.deserialize(b.into_deserializer()).map(Some)
    }
}

struct Map<'a, 'de> {
    de: &'a mut Deserializer<'de>,
    left: usize,
}

impl<'de> MapAccess<'de> for Map<'_, 'de> {
    type Error = Error;
    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        if self.left == 0 {
            return Ok(None);
        }
        self.left -= 1;
        seed.deserialize(KeyDeserializer { de: &mut *self.de }).map(Some)
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        seed.deserialize(&mut *self.de)
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.left)
    }
}

/// Map keys are atoms or binaries; integers (e.g. `guild_versions`) are stringified
/// so that `HashMap<String, _>` and struct field matching both work.
struct KeyDeserializer<'a, 'de> {
    de: &'a mut Deserializer<'de>,
}

impl<'de> de::Deserializer<'de> for KeyDeserializer<'_, 'de> {
    type Error = Error;
    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.de.peek()? {
            tag::SMALL_INTEGER | tag::INTEGER | tag::SMALL_BIG | tag::LARGE_BIG => {
                let v = de::Deserializer::deserialize_any(&mut *self.de, NumToString)?;
                visitor.visit_string(v)
            }
            tag::ATOM | tag::ATOM_UTF8 | tag::SMALL_ATOM | tag::SMALL_ATOM_UTF8 => {
                // Atom keys are names, never true/false/nil literals.
                let small = matches!(self.de.peek()?, tag::SMALL_ATOM | tag::SMALL_ATOM_UTF8);
                self.de.u8()?;
                let n = if small { self.de.u8()? as usize } else { self.de.u16()? as usize };
                visitor.visit_borrowed_str(Deserializer::str(self.de.take(n)?)?)
            }
            _ => de::Deserializer::deserialize_any(&mut *self.de, visitor),
        }
    }
    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map struct enum identifier ignored_any
    }
}

struct NumToString;
impl<'de> Visitor<'de> for NumToString {
    type Value = String;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("integer key")
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<String, E> {
        Ok(v.to_string())
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<String, E> {
        Ok(v.to_string())
    }
}
