//! Opens the call audio unit (VoiceProcessingIO) with each microphone choice and prints the capture level
//! for two seconds: digital silence (-100 dB) means the choice delivers nothing.
//! `cargo run -p unison-core --example mic_probe` (macOS; needs microphone access for the terminal).
#[cfg(target_os = "macos")]
fn main() {
    use coreaudio::audio_unit::Scope;
    use coreaudio::audio_unit::macos_helpers as h;
    
    use unison_core::voice::audio::{self, Controls};

    let ids = h::get_audio_device_ids().unwrap_or_default();
    let mut cases: Vec<(String, u32)> = vec![("system default".into(), 0)];
    for id in ids {
        let name = h::get_device_name(id).unwrap_or_default();
        let i = h::get_audio_device_supports_scope(id, Scope::Input).unwrap_or(false);
        let o = h::get_audio_device_supports_scope(id, Scope::Output).unwrap_or(false);
        println!("device {id}: {name} in={i} out={o}");
        if i { cases.push((format!("{name} (id {id}, out={o})"), id)); }
    }
    let secs: f32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(2.0);
    for (label, dev) in cases {
        unison_core::voice::params::params().set_devices(dev, 0);
        let (mix, _feeds) = audio::mixer(Controls::new());
        let t = std::thread::current();
        match audio::open(mix, t) {
            Err(e) => println!("{label}: open failed: {e}"),
            Ok((_device, mut cap)) => {
                let mut worst = -200f32; let mut sum = 0f64; let mut n = 0u32;
                let mut f = [0f32; 960];
                let t0 = std::time::Instant::now();
                while t0.elapsed().as_secs_f32() < secs {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    while cap.frame(&mut f) { let l = audio::level_db(&f); worst = worst.max(l); sum += l as f64; n += 1; }
                }
                println!("{label}: frames={n} mean={:.1} dB peak={:.1} dB", if n > 0 { sum / n as f64 } else { -200.0 }, worst);
            }
        }
    }
}
#[cfg(not(target_os = "macos"))]
fn main() {}
