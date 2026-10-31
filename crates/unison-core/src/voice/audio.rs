//! Microphone and speakers through Apple's voice-processing I/O unit (echo cancellation,
//! automatic gain and noise suppression in one hardware-tuned unit).
//!
//! The two real-time callbacks never allocate, lock or block: capture writes mono samples into
//! a lock-free ring and wakes the media thread; playout mixes per-speaker rings that the receive
//! thread fills. Everything else happens on ordinary threads.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::Thread;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::voice::codec::{CHANNELS, FRAME};

pub const SAMPLE_RATE: f64 = 48_000.0;
/// Remote speakers mixed at once.
pub const SLOTS: usize = 16;
/// About 1 s of mono capture; the media thread drains it every 20 ms.
const CAPTURE_RING: usize = 48_000;
/// About 500 ms of stereo per speaker.
const SLOT_RING: usize = FRAME * CHANNELS * 25;
/// Audio buffered for a speaker before their playout starts (and after an underrun): 60 ms.
const PRIME: usize = FRAME * CHANNELS * 3;

/// Flags shared with the callbacks.
#[derive(Default)]
pub struct Controls {
    pub deaf: AtomicBool,
    /// Per-slot gain as `f32` bits (1.0 = 100 %).
    pub gain: [AtomicU32; SLOTS],
}

impl Controls {
    pub fn new() -> Arc<Self> {
        let c = Self::default();
        for g in &c.gain {
            g.store(1.0f32.to_bits(), Ordering::Relaxed);
        }
        Arc::new(c)
    }
}

struct Slot {
    rx: Consumer<f32>,
    primed: bool,
}

/// The playout side: owned by the render callback.
pub struct Mixer {
    slots: Vec<Slot>,
    controls: Arc<Controls>,
}

/// The decode side: owned by the receive thread, one producer per speaker slot.
pub struct Feeds {
    pub slots: Vec<Producer<f32>>,
}

pub fn mixer(controls: Arc<Controls>) -> (Mixer, Feeds) {
    let mut slots = Vec::with_capacity(SLOTS);
    let mut feeds = Vec::with_capacity(SLOTS);
    for _ in 0..SLOTS {
        let (tx, rx) = RingBuffer::new(SLOT_RING);
        slots.push(Slot { rx, primed: false });
        feeds.push(tx);
    }
    (Mixer { slots, controls }, Feeds { slots: feeds })
}

impl Feeds {
    /// Queue decoded stereo for a slot; drops what doesn't fit rather than blocking.
    pub fn push(&mut self, slot: usize, pcm: &[f32]) {
        let Some(tx) = self.slots.get_mut(slot) else { return };
        let n = pcm.len().min(tx.slots());
        if let Ok(mut chunk) = tx.write_chunk_uninit(n) {
            let (a, b) = chunk.as_mut_slices();
            for (d, s) in a.iter_mut().chain(b.iter_mut()).zip(pcm) {
                d.write(*s);
            }
            unsafe { chunk.commit_all() };
        }
    }
}

impl Mixer {
    /// Fill interleaved stereo `out`. Real-time safe.
    pub fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let deaf = self.controls.deaf.load(Ordering::Relaxed);
        let master = crate::voice::params::params().output_gain();
        for (i, slot) in self.slots.iter_mut().enumerate() {
            let have = slot.rx.slots();
            if !slot.primed {
                if have < PRIME {
                    continue;
                }
                slot.primed = true;
            }
            if have < out.len() {
                // Underrun: play what's left, then wait to re-prime.
                slot.primed = false;
            }
            let n = have.min(out.len());
            let gain = f32::from_bits(self.controls.gain[i].load(Ordering::Relaxed)) * master;
            if let Ok(chunk) = slot.rx.read_chunk(n) {
                if !deaf {
                    let (a, b) = chunk.as_slices();
                    for (o, s) in out.iter_mut().zip(a.iter().chain(b)) {
                        *o += s * gain;
                    }
                }
                chunk.commit_all();
            }
        }
        // Soft clip the sum so several loud speakers don't wrap.
        for s in out.iter_mut() {
            if s.abs() > 0.9 {
                *s = s.signum() * (0.9 + (s.abs() - 0.9).tanh() * 0.1);
            }
        }
    }
}

/// The capture side: the media thread reads 20 ms of mono at a time.
pub struct Capture {
    pub rx: Consumer<f32>,
}

impl Capture {
    /// One 20 ms mono frame, if a whole one is buffered.
    pub fn frame(&mut self, out: &mut [f32; FRAME]) -> bool {
        let Ok(chunk) = self.rx.read_chunk(FRAME) else { return false };
        let (a, b) = chunk.as_slices();
        out[..a.len()].copy_from_slice(a);
        out[a.len()..].copy_from_slice(b);
        chunk.commit_all();
        true
    }
}

/// The running voice-processing unit; dropping it stops the microphone and speakers.
#[cfg(target_os = "macos")]
pub struct Device {
    unit: coreaudio::audio_unit::AudioUnit,
}

#[cfg(target_os = "macos")]
impl Drop for Device {
    fn drop(&mut self) {
        let _ = self.unit.stop();
    }
}

/// No audio backend for this OS yet: voice connects but stays silent and reports the error.
/// A new platform implements `open` (and `Device`) here, nothing else in the crate changes.
#[cfg(not(target_os = "macos"))]
pub struct Device;

#[cfg(not(target_os = "macos"))]
pub fn open(_mix: Mixer, _wake: Thread) -> Result<(Device, Capture), String> {
    Err("no audio backend for this platform".into())
}

/// Open the default microphone and speakers through VoiceProcessingIO. `wake` is unparked
/// after each capture callback so the media thread can encode without polling.
#[cfg(target_os = "macos")]
pub fn open(mut mix: Mixer, wake: Thread) -> Result<(Device, Capture), String> {
    use coreaudio::audio_unit::audio_format::LinearPcmFlags;
    use coreaudio::audio_unit::render_callback::{self, data};
    use coreaudio::audio_unit::{AudioUnit, Element, IOType, SampleFormat, Scope, StreamFormat};

    const ENABLE_IO: u32 = 2003; // kAudioOutputUnitProperty_EnableIO
    const STREAM_FORMAT: u32 = 8; // kAudioUnitProperty_StreamFormat

    let err = |e: coreaudio::Error| e.to_string();
    let mut unit = AudioUnit::new(IOType::VoiceProcessingIO).map_err(err)?;
    let _ = unit.uninitialize();
    // User choices (read at open, so they apply from the next join).
    let pr = crate::voice::params::params();
    let (input_dev, output_dev) = pr.devices();
    // VoiceProcessingIO drives microphone and speakers as one device, so a chosen device has to
    // support input. The microphone choice wins; an output-only pick falls back to the system default.
    let dev = [input_dev, output_dev].into_iter().find(|d| {
        *d != 0 && coreaudio::audio_unit::macos_helpers::get_audio_device_supports_scope(*d, Scope::Input).unwrap_or(false)
    });
    if let Some(d) = dev {
        const CURRENT_DEVICE: u32 = 2000; // kAudioOutputUnitProperty_CurrentDevice
        let _ = unit.set_property(CURRENT_DEVICE, Scope::Global, Element::Output, Some(&d));
    }
    // Apple exposes no separate echo / noise switches: voice processing is on when either is wanted,
    // and bypassed only when both are off. AGC has its own switch.
    let (echo, noise, agc) = pr.processing();
    const BYPASS_VP: u32 = 2100; // kAUVoiceIOProperty_BypassVoiceProcessing
    const ENABLE_AGC: u32 = 2101; // kAUVoiceIOProperty_VoiceProcessingEnableAGC
    let _ = unit.set_property(BYPASS_VP, Scope::Global, Element::Output, Some(&u32::from(!(echo || noise))));
    let _ = unit.set_property(ENABLE_AGC, Scope::Global, Element::Output, Some(&u32::from(agc)));
    unit.set_property(ENABLE_IO, Scope::Input, Element::Input, Some(&1u32)).map_err(err)?;
    unit.set_property(ENABLE_IO, Scope::Output, Element::Output, Some(&1u32)).map_err(err)?;
    let flags = LinearPcmFlags::IS_FLOAT | LinearPcmFlags::IS_PACKED;
    let mic = StreamFormat { sample_rate: SAMPLE_RATE, sample_format: SampleFormat::F32, flags, channels: 1 };
    let spk = StreamFormat { sample_rate: SAMPLE_RATE, sample_format: SampleFormat::F32, flags, channels: CHANNELS as u32 };
    unit.set_property(STREAM_FORMAT, Scope::Output, Element::Input, Some(&mic.to_asbd())).map_err(err)?;
    unit.set_property(STREAM_FORMAT, Scope::Input, Element::Output, Some(&spk.to_asbd())).map_err(err)?;

    let (mut tx, rx) = RingBuffer::<f32>::new(CAPTURE_RING);
    type Args = render_callback::Args<data::Interleaved<f32>>;
    unit.set_input_callback(move |args: Args| {
        let samples = &args.data.buffer[..args.num_frames * args.data.channels];
        let n = samples.len().min(tx.slots());
        if let Ok(mut chunk) = tx.write_chunk_uninit(n) {
            let (a, b) = chunk.as_mut_slices();
            for (d, s) in a.iter_mut().chain(b.iter_mut()).zip(samples.iter().step_by(args.data.channels.max(1))) {
                d.write(*s);
            }
            unsafe { chunk.commit_all() };
        }
        wake.unpark();
        Ok(())
    })
    .map_err(err)?;
    unit.set_render_callback(move |args: Args| {
        let n = args.num_frames * args.data.channels;
        mix.render(&mut args.data.buffer[..n]);
        Ok(())
    })
    .map_err(err)?;
    unit.initialize().map_err(err)?;
    unit.start().map_err(err)?;
    Ok((Device { unit }, Capture { rx }))
}

/// RMS level of a frame in dBFS.
pub fn level_db(frame: &[f32]) -> f32 {
    let mean = frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32;
    10.0 * mean.max(1e-10).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixer_primes_mixes_and_respects_deafen() {
        let controls = Controls::new();
        let (mut mix, mut feeds) = mixer(controls.clone());
        let mut out = vec![0f32; 480 * 2];
        feeds.push(0, &vec![0.1; FRAME * 2]);
        mix.render(&mut out);
        assert!(out.iter().all(|s| *s == 0.0), "not primed yet");
        feeds.push(0, &vec![0.1; FRAME * 2 * 2]);
        feeds.push(1, &vec![0.2; FRAME * 2 * 3]);
        mix.render(&mut out);
        assert!((out[0] - 0.3).abs() < 1e-6, "two speakers summed: {}", out[0]);
        controls.gain[1].store(0.5f32.to_bits(), Ordering::Relaxed);
        mix.render(&mut out);
        assert!((out[0] - 0.2).abs() < 1e-6);
        controls.deaf.store(true, Ordering::Relaxed);
        mix.render(&mut out);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn levels() {
        assert!(level_db(&[0.0; 960]) < -90.0);
        assert!((level_db(&[1.0; 960])).abs() < 0.01);
    }
}
