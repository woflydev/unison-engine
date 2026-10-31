//! Sign an appcast for the update channel.
//!
//! cargo run -p unison-core --example sign_appcast -- keygen            # prints a new seed and public key
//! cargo run -p unison-core --example sign_appcast -- sign <seed-b64> <appcast.json>   # prints the signed JSON
//!
//! Put the public key in `crates/unison-core/update_pubkey.txt` (this is what enables the update
//! channel in a build) and keep the seed offline. `appcast.json` is
//! `{"version","build","url","sha256","notes","min_os"}` plus the optional `"channel"` and `"assets"`
//! (see `unison_core::update::Appcast`); it is signed byte-for-byte, so do not reformat it.

use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};

fn main() {
    let b64 = base64::engine::general_purpose::STANDARD;
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keygen") => {
            let mut seed = [0u8; 32];
            for b in seed.iter_mut() {
                *b = fastrand::u8(..);
            }
            let sk = SigningKey::from_bytes(&seed);
            println!("seed (secret): {}", b64.encode(seed));
            println!("public key:    {}", b64.encode(sk.verifying_key().to_bytes()));
        }
        Some("sign") if args.len() == 3 => {
            let seed: [u8; 32] = b64.decode(&args[1]).expect("seed base64").try_into().expect("seed must be 32 bytes");
            let raw = std::fs::read_to_string(&args[2]).expect("read appcast");
            let raw = raw.trim();
            serde_json::from_str::<unison_core::update::Appcast>(raw).expect("appcast does not parse");
            let sig = SigningKey::from_bytes(&seed).sign(raw.as_bytes());
            println!("{{\"appcast\":{raw},\"signature\":\"{}\"}}", b64.encode(sig.to_bytes()));
        }
        _ => eprintln!("usage: sign_appcast keygen | sign <seed-b64> <appcast.json>"),
    }
}
