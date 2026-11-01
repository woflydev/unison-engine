//! Voice messages: mono Opus in an Ogg file, with the duration and the 8-bit waveform Discord
//! shows in the player. The shell captures microphone audio (48 kHz, mono, `f32`) and pushes it
//! here; this module encodes, builds the file and the waveform, and enforces the length limit.
//!
//! A voice message is an ordinary attachment named `voice-message.ogg` carrying `duration_secs`
//! and `waveform` (base64, at most 256 samples), sent with message flag `1 << 13` and no text.

use audiopus_sys as ffi;

pub const SAMPLE_RATE: usize = 48_000;
/// Samples in one 20 ms frame.
const FRAME: usize = 960;
/// Longest voice message the official client records.
pub const MAX_SECS: f64 = 300.0;
/// Shorter than this is a mis-click, not a message.
pub const MIN_SECS: f64 = 0.5;
/// Message flag `IS_VOICE_MESSAGE`.
pub const FLAG: u64 = 1 << 13;
/// Send Voice Messages (`1 << 46`).
pub const SEND_VOICE_MESSAGES: u64 = 1 << 46;
const BITRATE: i32 = 32_000;
const MAX_PACKET: usize = 1276;
/// Packets per Ogg page (half a second): keeps the page overhead near 1 kbps.
const PACKETS_PER_PAGE: usize = 25;
const WAVEFORM_MAX: usize = 256;
/// What libopus drops at the start (`OPUS_GET_LOOKAHEAD` at 48 kHz, 20 ms VoIP: 312 samples).
const PRE_SKIP: u16 = 312;

/// Voice messages need Send Messages and Send Voice Messages in a server; DMs always allow them.
pub fn can_record(in_guild: bool, perms: u64) -> bool {
    use crate::store::perm;
    if !in_guild {
        return true;
    }
    perms & perm::ADMINISTRATOR != 0 || perms & perm::SEND_MESSAGES != 0 && perms & SEND_VOICE_MESSAGES != 0
}

pub struct VoiceNote {
    raw: *mut ffi::OpusEncoder,
    pending: Vec<f32>,
    out: [u8; MAX_PACKET],
    packets: Vec<Vec<u8>>,
    /// Loudest sample of each 20 ms frame, 0...1.
    peaks: Vec<f32>,
    frames: usize,
}

// The encoder is used from one thread at a time (the recorder holds it behind a mutex).
unsafe impl Send for VoiceNote {}

pub struct Finished {
    pub bytes: Vec<u8>,
    pub duration_secs: f64,
    /// Base64 of one byte per sample.
    pub waveform: String,
}

impl VoiceNote {
    pub fn new() -> Option<Self> {
        let mut err = 0;
        let raw = unsafe { ffi::opus_encoder_create(SAMPLE_RATE as i32, 1, ffi::OPUS_APPLICATION_VOIP, &mut err) };
        if raw.is_null() || err != 0 {
            return None;
        }
        unsafe {
            ffi::opus_encoder_ctl(raw, ffi::OPUS_SET_BITRATE_REQUEST, BITRATE);
            ffi::opus_encoder_ctl(raw, ffi::OPUS_SET_SIGNAL_REQUEST, ffi::OPUS_SIGNAL_VOICE);
            ffi::opus_encoder_ctl(raw, ffi::OPUS_SET_COMPLEXITY_REQUEST, 8i32);
        }
        Some(Self { raw, pending: Vec::new(), out: [0; MAX_PACKET], packets: Vec::new(), peaks: Vec::new(), frames: 0 })
    }

    pub fn elapsed_secs(&self) -> f64 {
        (self.frames * FRAME) as f64 / SAMPLE_RATE as f64
    }

    pub fn full(&self) -> bool {
        self.elapsed_secs() >= MAX_SECS
    }

    /// Add microphone samples; returns the loudness (0...1) of what was added, for the live meter.
    /// Samples past the length limit are dropped.
    pub fn push(&mut self, samples: &[f32]) -> f32 {
        let level = samples.iter().fold(0f32, |m, s| m.max(s.abs())).min(1.0);
        if self.full() {
            return level;
        }
        self.pending.extend_from_slice(samples);
        while self.pending.len() >= FRAME && !self.full() {
            let frame: Vec<f32> = self.pending.drain(..FRAME).collect();
            self.peaks.push(frame.iter().fold(0f32, |m, s| m.max(s.abs())).min(1.0));
            let n = unsafe { ffi::opus_encode_float(self.raw, frame.as_ptr(), FRAME as i32, self.out.as_mut_ptr(), MAX_PACKET as i32) };
            if n > 0 {
                self.packets.push(self.out[..n as usize].to_vec());
                self.frames += 1;
            }
        }
        level
    }

    /// Close the file. `None` when nothing worth sending was recorded.
    pub fn finish(mut self) -> Option<Finished> {
        if !self.pending.is_empty() && !self.full() {
            let mut last = std::mem::take(&mut self.pending);
            last.resize(FRAME, 0.0);
            self.push(&last);
        }
        let duration = self.elapsed_secs();
        if duration < MIN_SECS || self.packets.is_empty() {
            return None;
        }
        let waveform = waveform(&self.peaks);
        let bytes = ogg_opus(&self.packets, self.frames * FRAME);
        Some(Finished { bytes, duration_secs: duration, waveform: base64(&waveform) })
    }
}

impl Drop for VoiceNote {
    fn drop(&mut self) {
        unsafe { ffi::opus_encoder_destroy(self.raw) }
    }
}

/// Up to 256 samples: the loudest frame in each equal slice, on a gentle curve so quiet speech
/// still shows, scaled so the loudest slice is full height.
pub fn waveform(peaks: &[f32]) -> Vec<u8> {
    if peaks.is_empty() {
        return vec![];
    }
    let n = peaks.len().min(WAVEFORM_MAX);
    let mut cells: Vec<f32> = (0..n)
        .map(|i| {
            let lo = i * peaks.len() / n;
            let hi = ((i + 1) * peaks.len() / n).max(lo + 1);
            peaks[lo..hi].iter().cloned().fold(0.0, f32::max).sqrt()
        })
        .collect();
    let top = cells.iter().cloned().fold(0.0, f32::max);
    if top > 0.0 {
        for c in &mut cells {
            *c /= top;
        }
    }
    cells.into_iter().map(|c| (c * 255.0).round() as u8).collect()
}

fn base64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

// MARK: Ogg

fn crc_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (i, slot) in t.iter_mut().enumerate() {
        let mut r = (i as u32) << 24;
        for _ in 0..8 {
            r = if r & 0x8000_0000 != 0 { (r << 1) ^ 0x04C1_1DB7 } else { r << 1 };
        }
        *slot = r;
    }
    t
}

struct OggWriter {
    out: Vec<u8>,
    serial: u32,
    seq: u32,
    table: [u32; 256],
}

impl OggWriter {
    /// `kind`: 0x02 first page, 0x04 last page. `packets` are laced into the segment table.
    fn page(&mut self, kind: u8, granule: u64, packets: &[&[u8]]) {
        let mut segs: Vec<u8> = Vec::new();
        for p in packets {
            let mut left = p.len();
            while left >= 255 {
                segs.push(255);
                left -= 255;
            }
            segs.push(left as u8);
        }
        debug_assert!(segs.len() <= 255);
        let start = self.out.len();
        self.out.extend_from_slice(b"OggS");
        self.out.push(0);
        self.out.push(kind);
        self.out.extend_from_slice(&granule.to_le_bytes());
        self.out.extend_from_slice(&self.serial.to_le_bytes());
        self.out.extend_from_slice(&self.seq.to_le_bytes());
        self.out.extend_from_slice(&[0; 4]);
        self.out.push(segs.len() as u8);
        self.out.extend_from_slice(&segs);
        for p in packets {
            self.out.extend_from_slice(p);
        }
        let mut crc = 0u32;
        for b in &self.out[start..] {
            crc = (crc << 8) ^ self.table[((crc >> 24) as u8 ^ b) as usize];
        }
        self.out[start + 22..start + 26].copy_from_slice(&crc.to_le_bytes());
        self.seq += 1;
    }
}

/// Ogg Opus (RFC 7845): the OpusHead page, the OpusTags page, then audio. `samples` is the audio
/// length at 48 kHz, used for the final granule position.
pub fn ogg_opus(packets: &[Vec<u8>], samples: usize) -> Vec<u8> {
    let mut w = OggWriter { out: Vec::new(), serial: 0x554e_4953, seq: 0, table: crc_table() };
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1);
    head.push(1); // mono
    head.extend_from_slice(&PRE_SKIP.to_le_bytes());
    head.extend_from_slice(&(SAMPLE_RATE as u32).to_le_bytes());
    head.extend_from_slice(&0u16.to_le_bytes());
    head.push(0);
    w.page(0x02, 0, &[&head]);
    let vendor = b"Unison";
    let mut tags = Vec::new();
    tags.extend_from_slice(b"OpusTags");
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes());
    w.page(0, 0, &[&tags]);
    let chunks: Vec<&[Vec<u8>]> = packets.chunks(PACKETS_PER_PAGE).collect();
    let last = chunks.len().saturating_sub(1);
    let mut done = 0usize;
    for (i, chunk) in chunks.iter().enumerate() {
        done += chunk.len();
        let granule = if i == last { samples as u64 + PRE_SKIP as u64 } else { (done * FRAME) as u64 + PRE_SKIP as u64 };
        let refs: Vec<&[u8]> = chunk.iter().map(|p| p.as_slice()).collect();
        w.page(if i == last { 0x04 } else { 0 }, granule, &refs);
    }
    w.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(secs: f32) -> Vec<f32> {
        (0..(secs * SAMPLE_RATE as f32) as usize).map(|i| (i as f32 * 0.05).sin() * 0.5).collect()
    }

    #[test]
    fn records_a_valid_ogg_opus_file() {
        let mut n = VoiceNote::new().unwrap();
        // Pushed in odd-sized pieces, as a microphone callback would.
        for piece in tone(2.0).chunks(1111) {
            n.push(piece);
        }
        let f = n.finish().unwrap();
        assert!((f.duration_secs - 2.0).abs() < 0.05, "{}", f.duration_secs);
        assert_eq!(&f.bytes[..4], b"OggS");
        assert_eq!(&f.bytes[28..36], b"OpusHead");
        // The waveform decodes to between 1 and 256 samples and peaks at full scale.
        use base64::Engine;
        let w = base64::engine::general_purpose::STANDARD.decode(&f.waveform).unwrap();
        assert!(!w.is_empty() && w.len() <= 256);
        assert_eq!(*w.iter().max().unwrap(), 255);
    }

    #[test]
    fn page_checksums_are_valid() {
        let mut n = VoiceNote::new().unwrap();
        n.push(&tone(1.0));
        let f = n.finish().unwrap();
        let table = crc_table();
        let mut at = 0;
        let mut pages = 0;
        while at < f.bytes.len() {
            let nsegs = f.bytes[at + 26] as usize;
            let body: usize = f.bytes[at + 27..at + 27 + nsegs].iter().map(|b| *b as usize).sum();
            let end = at + 27 + nsegs + body;
            let mut page = f.bytes[at..end].to_vec();
            let want = u32::from_le_bytes(page[22..26].try_into().unwrap());
            page[22..26].copy_from_slice(&[0; 4]);
            let mut crc = 0u32;
            for b in &page {
                crc = (crc << 8) ^ table[((crc >> 24) as u8 ^ b) as usize];
            }
            assert_eq!(crc, want);
            at = end;
            pages += 1;
        }
        assert!(pages >= 3);
    }

    #[test]
    fn too_short_is_nothing() {
        let mut n = VoiceNote::new().unwrap();
        n.push(&tone(0.2));
        assert!(n.finish().is_none());
    }

    #[test]
    fn stops_at_the_limit() {
        let mut n = VoiceNote::new().unwrap();
        for _ in 0..31 {
            n.push(&tone(10.0));
        }
        assert!(n.full());
        assert!(n.elapsed_secs() <= MAX_SECS + 0.02);
    }

    #[test]
    fn silence_has_a_flat_waveform() {
        assert!(waveform(&[0.0; 40]).iter().all(|b| *b == 0));
        assert!(waveform(&[]).is_empty());
    }

    #[test]
    fn permission_rule() {
        use crate::store::perm;
        assert!(can_record(false, 0));
        assert!(!can_record(true, perm::SEND_MESSAGES));
        assert!(can_record(true, perm::SEND_MESSAGES | SEND_VOICE_MESSAGES));
    }
}
