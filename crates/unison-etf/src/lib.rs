//! Erlang External Term Format (ETF), as spoken by Discord's gateway with
//! `encoding=etf`.
//!
//! Decoding goes straight from bytes into any `serde::Deserialize` type via
//! [`from_slice`], borrowing strings where possible. Encoding mirrors
//! `erlpack.pack()` from a `serde_json::Value`, which is what the official
//! client sends (JS strings become BINARY_EXT, `null` becomes the `nil` atom).

mod de;
mod enc;
mod frame;

pub use de::{Deserializer, from_slice};
pub use enc::{encode_value, to_vec};
pub use frame::{Frame, RawTerm, from_term, split_frame, term_len};

pub(crate) const VERSION: u8 = 131;

pub(crate) mod tag {
    pub const NEW_FLOAT: u8 = 70;
    pub const SMALL_INTEGER: u8 = 97;
    pub const INTEGER: u8 = 98;
    pub const FLOAT: u8 = 99;
    pub const ATOM: u8 = 100;
    pub const SMALL_TUPLE: u8 = 104;
    pub const LARGE_TUPLE: u8 = 105;
    pub const NIL: u8 = 106;
    pub const STRING: u8 = 107;
    pub const LIST: u8 = 108;
    pub const BINARY: u8 = 109;
    pub const SMALL_BIG: u8 = 110;
    pub const LARGE_BIG: u8 = 111;
    pub const MAP: u8 = 116;
    pub const ATOM_UTF8: u8 = 118;
    pub const SMALL_ATOM_UTF8: u8 = 119;
    pub const SMALL_ATOM: u8 = 115;
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unexpected end of input")]
    Eof,
    #[error("bad ETF version byte {0}")]
    Version(u8),
    #[error("unsupported ETF tag {0}")]
    Tag(u8),
    #[error("integer does not fit in 64 bits")]
    BigInt,
    #[error("invalid utf-8 in string")]
    Utf8,
    #[error("{0}")]
    Custom(String),
}

impl serde::de::Error for Error {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        Error::Custom(msg.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
