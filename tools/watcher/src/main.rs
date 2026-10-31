//! Discord update watcher.
//!
//!   watcher snapshot [DIR]          fetch the live web client + desktop manifest, write DIR/<date>.json and DIR/latest.json
//!   watcher diff OLD.json NEW.json  report what changed; exit 2 if anything fidelity-relevant changed
//!   watcher keygen KEYFILE          create an Ed25519 signing key, print the public key for profile_pubkey.txt
//!   watcher sign KEYFILE SNAPSHOT OUT  build a signed identity profile from a snapshot
//!
//! A snapshot downloads exactly what opening discord.com/app downloads (the preloaded bundles),
//! once, unauthenticated. Run it daily, not in a loop.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use base64::Engine as _;
use futures_util::StreamExt;
use regex::Regex;
use serde::{Deserialize, Serialize};
use unison_core::profile::{IdentityProfile, sec_ch_ua_for};

#[derive(Debug, Default, Serialize, Deserialize)]
struct Snapshot {
    taken_at: String,
    build_number: Option<u64>,
    version_hash: Option<String>,
    api_version: Option<u64>,
    gateway_endpoint: Option<String>,
    host_version: Option<String>,
    asset_count: usize,
    asset_bytes: usize,
    gateway_capabilities: Vec<u64>,
    super_property_keys: Vec<String>,
    identify_keys: Vec<String>,
    rest_routes: BTreeSet<String>,
    dispatch_events: BTreeSet<String>,
    analytics_routes: BTreeSet<String>,
    monetisation: BTreeMap<String, usize>,
}

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36";

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> ExitCode {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("snapshot") => snapshot(Path::new(args.get(1).map_or("watch", String::as_str))).await,
        Some("diff") if args.len() == 3 => return diff(&args[1], &args[2]),
        Some("keygen") if args.len() == 2 => keygen(&args[1]),
        Some("sign") if args.len() == 4 => sign(&args[1], &args[2], &args[3]),
        _ => {
            eprintln!("usage: watcher snapshot [DIR] | diff OLD NEW | keygen KEYFILE | sign KEYFILE SNAPSHOT OUT");
            return ExitCode::from(64);
        }
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn snapshot(dir: &Path) -> Res<()> {
    std::fs::create_dir_all(dir)?;
    let client = reqwest::Client::builder().user_agent(UA).use_rustls_tls().build()?;
    let html = client.get("https://discord.com/app").send().await?.text().await?;

    let env = |key: &str| -> Option<String> {
        Regex::new(&format!(r#""{key}":"?([^",]+)"?"#)).ok()?.captures(&html).map(|c| c[1].to_string())
    };
    let mut snap = Snapshot {
        taken_at: now_iso(),
        build_number: env("BUILD_NUMBER").and_then(|v| v.parse().ok()),
        version_hash: env("VERSION_HASH"),
        api_version: env("API_VERSION").and_then(|v| v.parse().ok()),
        gateway_endpoint: env("GATEWAY_ENDPOINT"),
        ..Default::default()
    };

    let assets: BTreeSet<String> = Regex::new(r#"(?:src|href)="(/assets/[^"]+\.js)""#)?
        .captures_iter(&html)
        .map(|c| c[1].to_string())
        .collect();
    snap.asset_count = assets.len();
    let bodies: Vec<String> = futures_util::stream::iter(assets)
        .map(|path| {
            let client = client.clone();
            async move {
                let url = format!("https://discord.com{path}");
                match client.get(&url).send().await {
                    Ok(r) => r.text().await.unwrap_or_default(),
                    Err(_) => String::new(),
                }
            }
        })
        .buffer_unordered(6)
        .collect()
        .await;
    snap.asset_bytes = bodies.iter().map(String::len).sum();
    let js = bodies.join("\n");

    analyse(&js, &mut snap)?;
    snap.host_version = host_version(&client).await;

    let json = serde_json::to_string_pretty(&snap)?;
    let day = &snap.taken_at[..10];
    std::fs::write(dir.join(format!("{day}.json")), &json)?;
    std::fs::write(dir.join("latest.json"), &json)?;
    println!(
        "build {} · host {} · {} assets ({:.1} MB) · {} routes · {} events · capabilities {:?}",
        snap.build_number.map_or("?".into(), |b| b.to_string()),
        snap.host_version.as_deref().unwrap_or("?"),
        snap.asset_count,
        snap.asset_bytes as f64 / 1e6,
        snap.rest_routes.len(),
        snap.dispatch_events.len(),
        snap.gateway_capabilities
    );
    Ok(())
}

fn analyse(js: &str, snap: &mut Snapshot) -> Res<()> {
    // Gateway capabilities: `return t?1767421:1734653` inside the IDENTIFY builder.
    let caps = Regex::new(r"capabilities:function\(e\)\{let\{useChannelObfuscation:t\}=e;return t\?(\d+):(\d+)\}")?;
    if let Some(c) = caps.captures(js) {
        snap.gateway_capabilities = vec![c[2].parse()?, c[1].parse()?];
    }
    // Desktop super-properties object literal.
    let sp = Regex::new(r#"\{os:e,browser:"Discord Client",([^}]{0,600})\}"#)?;
    if let Some(c) = sp.captures(js) {
        let mut keys = vec!["os".to_string(), "browser".to_string()];
        keys.extend(Regex::new(r"(?:^|,)([a-z_]+):")?.captures_iter(&c[1]).map(|k| k[1].to_string()));
        snap.super_property_keys = keys;
    }
    for k in ["client_build_number", "native_build_number", "client_event_source", "launch_signature", "client_heartbeat_session_id", "client_app_state", "install_source"] {
        if js.contains(&format!(".{k}=")) || js.contains(&format!("{k}:")) {
            if !snap.super_property_keys.iter().any(|x| x == k) {
                snap.super_property_keys.push(k.into());
            }
        }
    }
    // IDENTIFY payload keys.
    if let Some(start) = js.find("let u={token:l,") {
        snap.identify_keys = top_level_keys(&js[start + "let u=".len()..]);
    }
    let route = Regex::new(r#"[A-Z][A-Z0-9_]{2,}:(?:"(/[a-z0-9@/_.\-]+)"|e=>`(/[^`]{1,120})`|\([a-z,]+\)=>`(/[^`]{1,120})`)"#)?;
    let param = Regex::new(r"\$\{[^}]+\}")?;
    for c in route.captures_iter(js) {
        let r = c.get(1).or(c.get(2)).or(c.get(3)).unwrap().as_str();
        let r = param.replace_all(r, ":p").split('?').next().unwrap_or("").to_string();
        if let Err(cat) = unison_core::privacy::Policy::default().check_route(&r) {
            snap.analytics_routes.insert(format!("{r} [{}]", cat.label()));
        }
        snap.rest_routes.insert(r);
    }
    let ev = Regex::new(r#"\b(?:case|type:)"((?:MESSAGE|GUILD|CHANNEL|THREAD|PRESENCE|TYPING|VOICE|READY|SESSIONS|USER|RELATIONSHIP|INTERACTION|CALL|STAGE|AUTO_MODERATION|ENTITLEMENT|PASSIVE|CONVERSATION|SOUNDBOARD|EMBEDDED|QUEST|RECENT_MENTION|SAVED_MESSAGE|LAST_MESSAGES)_[A-Z_]+)""#)?;
    snap.dispatch_events = ev.captures_iter(js).map(|c| c[1].to_string()).collect();
    for k in ["UPSELL", "QUEST", "ADS_", "PREMIUM_PROMOTION", "COACHMARK", "NEW_BADGE"] {
        snap.monetisation.insert(k.into(), js.matches(k).count());
    }
    Ok(())
}

/// Keys of the object literal starting at `s[0] == '{'`, ignoring nested objects/calls.
fn top_level_keys(s: &str) -> Vec<String> {
    let b = s.as_bytes();
    let (mut depth, mut i, mut keys, mut key_start) = (0i32, 0usize, Vec::new(), Some(1usize));
    while i < b.len() {
        match b[i] {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            b',' if depth == 1 => key_start = Some(i + 1),
            b':' if depth == 1 => {
                if let Some(k) = key_start.take() {
                    let name = &s[k..i];
                    if name.bytes().all(|c| c.is_ascii_lowercase() || c == b'_') && !name.is_empty() {
                        keys.push(name.to_string());
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    keys
}

/// Desktop host version from the official updater's manifest endpoint.
async fn host_version(client: &reqwest::Client) -> Option<String> {
    let url = "https://updates.discord.com/distributions/app/manifests/latest?channel=stable&platform=osx&arch=arm64";
    let v: serde_json::Value = client.get(url).send().await.ok()?.json().await.ok()?;
    let parts = v["full"]["host_version"].as_array()?;
    Some(parts.iter().filter_map(|p| p.as_u64()).map(|p| p.to_string()).collect::<Vec<_>>().join("."))
}

fn diff(old: &str, new: &str) -> ExitCode {
    let load = |p: &str| -> Option<Snapshot> { serde_json::from_slice(&std::fs::read(p).ok()?).ok() };
    let (Some(a), Some(b)) = (load(old), load(new)) else {
        eprintln!("could not read snapshots");
        return ExitCode::FAILURE;
    };
    let mut relevant = false;
    let mut out = String::new();
    let mut line = |s: String| {
        out.push_str(&s);
        out.push('\n');
    };
    line(format!("# Discord changes: {} → {}", &a.taken_at[..10], &b.taken_at[..10]));
    if a.build_number != b.build_number {
        line(format!("- build {:?} → {:?} (profile update needed)", a.build_number, b.build_number));
        relevant = true;
    }
    if a.host_version != b.host_version {
        line(format!("- desktop host {:?} → {:?} (profile update needed)", a.host_version, b.host_version));
        relevant = true;
    }
    for (name, x, y) in [
        ("gateway capabilities", format!("{:?}", a.gateway_capabilities), format!("{:?}", b.gateway_capabilities)),
        ("super-properties keys", a.super_property_keys.join(","), b.super_property_keys.join(",")),
        ("IDENTIFY keys", a.identify_keys.join(","), b.identify_keys.join(",")),
    ] {
        if x != y {
            line(format!("- **{name} changed**\n  - was: {x}\n  - now: {y}"));
            relevant = true;
        }
    }
    let set_diff = |label: &str, x: &BTreeSet<String>, y: &BTreeSet<String>, line: &mut dyn FnMut(String)| {
        let added: Vec<_> = y.difference(x).collect();
        let removed: Vec<_> = x.difference(y).collect();
        if !added.is_empty() || !removed.is_empty() {
            line(format!("- {label}: +{} −{}", added.len(), removed.len()));
            for r in added.iter().take(40) {
                line(format!("  - + `{r}`"));
            }
            for r in removed.iter().take(40) {
                line(format!("  - − `{r}`"));
            }
        }
    };
    set_diff("REST routes", &a.rest_routes, &b.rest_routes, &mut line);
    set_diff("dispatch events", &a.dispatch_events, &b.dispatch_events, &mut line);
    set_diff("telemetry/monetisation routes", &a.analytics_routes, &b.analytics_routes, &mut line);
    print!("{out}");
    if relevant { ExitCode::from(2) } else { ExitCode::SUCCESS }
}

fn keygen(path: &str) -> Res<()> {
    let p = PathBuf::from(path);
    if p.exists() {
        return Err(format!("{path} already exists; refusing to overwrite a signing key").into());
    }
    let sk = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    std::fs::write(&p, base64::engine::general_purpose::STANDARD.encode(sk.to_bytes()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600))?;
    }
    println!("{}", base64::engine::general_purpose::STANDARD.encode(sk.verifying_key().to_bytes()));
    eprintln!("Paste the line above into crates/unison-core/profile_pubkey.txt. Keep {path} private.");
    Ok(())
}

fn sign(key: &str, snapshot: &str, out: &str) -> Res<()> {
    use ed25519_dalek::Signer;
    let b64 = base64::engine::general_purpose::STANDARD;
    let bytes: [u8; 32] = b64.decode(std::fs::read_to_string(key)?.trim())?.try_into().map_err(|_| "bad key")?;
    let sk = ed25519_dalek::SigningKey::from_bytes(&bytes);
    let snap: Snapshot = serde_json::from_slice(&std::fs::read(snapshot)?)?;
    let mut p = IdentityProfile::bundled();
    p.serial = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    if let Some(b) = snap.build_number {
        p.client_build_number = b;
    }
    if let Some(h) = snap.host_version {
        p.client_version = h;
    }
    if let Some(c) = snap.gateway_capabilities.first() {
        p.gateway_capabilities = *c;
    }
    p.sec_ch_ua = sec_ch_ua_for(p.chrome_major());
    p.notes.insert("source".into(), format!("watcher snapshot {}", snap.taken_at));
    let raw = serde_json::to_string(&p)?;
    let sig = b64.encode(sk.sign(raw.as_bytes()).to_bytes());
    std::fs::write(out, format!("{{\"profile\":{raw},\"signature\":\"{sig}\"}}"))?;
    println!("signed profile serial {} → {out}", p.serial);
    Ok(())
}

fn now_iso() -> String {
    // UTC date-time without pulling in a date crate.
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
