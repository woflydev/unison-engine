//! Live smoke test: connect to the QR login gateway until a QR code is issued, then stop.
//! Does not log in. Run: cargo run -p unison-core --example qr_probe
use std::sync::Arc;
use unison_core::{fidelity::*, privacy::Policy, profile::IdentityProfile, remote_auth::*, rest::Rest, transport::Transport};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let profile = IdentityProfile::bundled();
    let transport = Arc::new(Transport::new(None).unwrap());
    let identity = Arc::new(ClientIdentity::new(profile, HostInfo::detect()));
    let rest = Arc::new(Rest::new(transport.clone(), identity, Arc::new(parking_lot::RwLock::new(Policy::default()))));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let (_ctx, crx) = tokio::sync::mpsc::unbounded_channel();
    let task = start(rest, tx, crx);
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(20));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Some(RemoteAuthEvent::Qr { url }) => { println!("QR issued: {}… ({} chars)", &url[..26], url.len()); break }
                Some(other) => { println!("event: {other:?}"); if matches!(other, RemoteAuthEvent::Failed{..}) { break } }
                None => break,
            },
            _ = &mut deadline => { println!("timeout"); break }
        }
    }
    task.abort();
    println!("cookies set by discord.com: {}", transport.saved_cookies().split("; ").filter(|s| !s.is_empty()).map(|c| c.split('=').next().unwrap()).collect::<Vec<_>>().join(", "));
}
