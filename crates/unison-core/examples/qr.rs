//! Dev check: run the QR login handshake and print each event with a timestamp.
use std::{sync::Arc, time::Instant};
use parking_lot::RwLock;
use unison_core::{fidelity::{ClientIdentity, HostInfo}, privacy::Policy, profile::IdentityProfile, remote_auth, rest::Rest, transport::Transport};

#[tokio::main]
async fn main() {
    let profile = IdentityProfile::bundled();
    let identity = Arc::new(ClientIdentity::new(profile, HostInfo::detect()));
    let rest = Arc::new(Rest::new(Arc::new(Transport::new(None).unwrap()), identity, Arc::new(RwLock::new(Policy::default()))));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let t = Instant::now();
    let (_ctx, crx) = tokio::sync::mpsc::unbounded_channel();
    remote_auth::start(rest, tx, crx);
    while let Some(ev) = rx.recv().await {
        println!("{:>6.1}s {:?}", t.elapsed().as_secs_f32(), ev);
    }
    println!("{:>6.1}s channel closed", t.elapsed().as_secs_f32());
}
