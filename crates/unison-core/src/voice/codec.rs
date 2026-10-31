//! Opus, linked statically: 48 kHz stereo, 20 ms frames, in-band FEC.

use audiopus_sys as ffi;

pub const SAMPLE_RATE: i32 = 48_000;
pub const CHANNELS: usize = 2;
/// Samples per channel in one 20 ms frame.
pub const FRAME: usize = 960;
const MAX_PACKET: usize = 1276;

pub struct Encoder {
    raw: *mut ffi::OpusEncoder,
    out: [u8; MAX_PACKET],
}

// The encoder is used from one thread at a time.
unsafe impl Send for Encoder {}

impl Encoder {
    /// `bitrate` in bits per second (the channel's `bitrate`, 64 kbps by default).
    pub fn new(bitrate: i32) -> Option<Self> {
        let mut err = 0;
        let raw = unsafe { ffi::opus_encoder_create(SAMPLE_RATE, CHANNELS as i32, ffi::OPUS_APPLICATION_VOIP, &mut err) };
        if raw.is_null() || err != 0 {
            return None;
        }
        let e = Self { raw, out: [0; MAX_PACKET] };
        unsafe {
            ffi::opus_encoder_ctl(raw, ffi::OPUS_SET_BITRATE_REQUEST, bitrate);
            ffi::opus_encoder_ctl(raw, ffi::OPUS_SET_INBAND_FEC_REQUEST, 1i32);
            ffi::opus_encoder_ctl(raw, ffi::OPUS_SET_PACKET_LOSS_PERC_REQUEST, 0i32);
            ffi::opus_encoder_ctl(raw, ffi::OPUS_SET_SIGNAL_REQUEST, ffi::OPUS_SIGNAL_VOICE);
            ffi::opus_encoder_ctl(raw, ffi::OPUS_SET_COMPLEXITY_REQUEST, 10i32);
        }
        Some(e)
    }

    /// Raise FEC redundancy to match the loss the receiver reports.
    pub fn set_expected_loss(&mut self, percent: i32) {
        unsafe { ffi::opus_encoder_ctl(self.raw, ffi::OPUS_SET_PACKET_LOSS_PERC_REQUEST, percent.clamp(0, 30)) };
    }

    /// One frame of interleaved stereo (`FRAME * 2` samples) to an Opus packet.
    pub fn encode(&mut self, pcm: &[f32]) -> Option<&[u8]> {
        debug_assert_eq!(pcm.len(), FRAME * CHANNELS);
        let n = unsafe { ffi::opus_encode_float(self.raw, pcm.as_ptr(), FRAME as i32, self.out.as_mut_ptr(), MAX_PACKET as i32) };
        (n > 0).then(|| &self.out[..n as usize])
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { ffi::opus_encoder_destroy(self.raw) }
    }
}

pub struct Decoder {
    raw: *mut ffi::OpusDecoder,
}

unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new() -> Option<Self> {
        let mut err = 0;
        let raw = unsafe { ffi::opus_decoder_create(SAMPLE_RATE, CHANNELS as i32, &mut err) };
        (!raw.is_null() && err == 0).then_some(Self { raw })
    }

    /// Decode into `out` (interleaved stereo, `FRAME * 2`). `None` conceals a lost packet;
    /// `fec` recovers the previous frame from this packet's redundancy instead.
    pub fn decode(&mut self, packet: Option<&[u8]>, out: &mut [f32], fec: bool) -> usize {
        let (ptr, len) = packet.map_or((std::ptr::null(), 0), |p| (p.as_ptr(), p.len() as i32));
        let n = unsafe { ffi::opus_decode_float(self.raw, ptr, len, out.as_mut_ptr(), (out.len() / CHANNELS) as i32, fec as i32) };
        n.max(0) as usize
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { ffi::opus_decoder_destroy(self.raw) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_round_trips() {
        let mut enc = Encoder::new(64_000).unwrap();
        let mut dec = Decoder::new().unwrap();
        let mut energy_in = 0.0f32;
        let mut energy_out = 0.0f32;
        let mut out = vec![0f32; FRAME * CHANNELS];
        for f in 0..10 {
            let pcm: Vec<f32> = (0..FRAME)
                .flat_map(|i| {
                    let t = (f * FRAME + i) as f32 / SAMPLE_RATE as f32;
                    let s = (t * 440.0 * std::f32::consts::TAU).sin() * 0.3;
                    [s, s]
                })
                .collect();
            let packet = enc.encode(&pcm).unwrap().to_vec();
            assert!(packet.len() < 400);
            assert_eq!(dec.decode(Some(&packet), &mut out, false), FRAME);
            if f > 3 {
                energy_in += pcm.iter().map(|s| s * s).sum::<f32>();
                energy_out += out.iter().map(|s| s * s).sum::<f32>();
            }
        }
        let ratio = energy_out / energy_in;
        assert!((0.5..1.5).contains(&ratio), "energy ratio {ratio}");
        // Concealment of a lost frame still yields a full frame.
        assert_eq!(dec.decode(None, &mut out, false), FRAME);
    }
}
