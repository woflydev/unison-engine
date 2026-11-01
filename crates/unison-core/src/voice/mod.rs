//! Voice: the voice gateway, the UDP media path and audio, in one process.
//!
//! Layering, bottom to top: [`rtp`] framing and IP discovery, [`crypto`] transport AEAD,
//! [`codec`] Opus, [`dave`] end-to-end encryption, [`protocol`] voice gateway payloads,
//! [`audio`] the voice-processing audio unit, and [`connection`] tying them together.
pub mod audio;
pub mod codec;
pub mod connection;
pub mod crypto;
pub mod dave;
pub mod params;
pub mod h264;
pub mod protocol;
pub mod recv;
pub mod rtcp;
pub mod rtp;
pub mod stream;
pub mod video;
