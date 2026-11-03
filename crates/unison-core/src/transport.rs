//! HTTP/2 and WebSocket transport.
//!
//! Two stacks sit behind one [`Transport`]:
//! - **Chrome** (default): BoringSSL with the TLS ClientHello and HTTP/2 settings of the Chromium
//!   build Discord's desktop client ships (Electron's network stack *is* Chromium's), so the
//!   handshake looks like the official client's instead of rustls'. Only the TLS and HTTP/2
//!   parameters of the profile are used; every header still comes from [`crate::fidelity`]. The
//!   gateway WebSocket is HTTP/1.1 only (ALPN `http/1.1`), as in Chromium.
//! - **rustls**: `UNISON_TLS=rustls`, the plain stack (kept as an escape hatch).
//!
//! One account, one stack, for the life of the process: never mix fingerprints on one session.

use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::{Error, Result};

pub use reqwest::Method;
pub use tokio_tungstenite::tungstenite::Message as WsMessage;

/// The Chromium profile whose TLS/HTTP2 shape we present. Discord 0.0.420 ships Chrome 152; the
/// newest profile the library carries is used (ClientHello shape is stable across these).
const CHROME_PROFILE: wreq_util::Profile = wreq_util::Profile::Chrome149;

enum Engine {
    /// REST client, and the WebSocket client (no ALPS: Chromium sends it only when h2 is offered).
    Chrome(wreq::Client, wreq::Client),
    Rustls(reqwest::Client),
}

/// Which stack this process uses (`UNISON_TLS=rustls` selects the plain one).
pub fn uses_chrome_tls() -> bool {
    !std::env::var("UNISON_TLS").is_ok_and(|v| v.eq_ignore_ascii_case("rustls"))
}

/// One account's connection pool and cookies.
pub struct Transport {
    client: Engine,
    cookies: Mutex<CookieJar>,
}

pub struct Response {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: bytes::Bytes,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Network(e.to_string())
    }
}

impl From<wreq::Error> for Error {
    fn from(e: wreq::Error) -> Self {
        Error::Network(e.to_string())
    }
}

impl From<tokio_tungstenite::tungstenite::Error> for Error {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        use tokio_tungstenite::tungstenite::Error as E;
        match e {
            E::Http(resp) => Error::UpgradeRefused(resp.status().as_u16()),
            other => Error::Network(other.to_string()),
        }
    }
}

fn install_crypto() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// The profile's TLS and HTTP/2 parameters only. Its default headers (a stock Chrome UA and
/// client hints) are dropped: Unison sends Discord's own set, in Discord's order.
///
/// The builder insists on a platform; it only selects header values, which are discarded below,
/// so it has no effect on the wire and is not a client-identity claim (that is `fidelity.rs`).
fn chrome_emulation(websocket: bool) -> wreq::Emulation {
    use wreq::IntoEmulation;
    let mut emu = wreq_util::Emulation::builder().profile(CHROME_PROFILE).platform(wreq_util::Platform::MacOS).build().into_emulation();
    emu.headers.clear();
    emu.orig_headers = wreq::header::OrigHeaderMap::new();
    if websocket {
        if let Some(tls) = emu.tls_options.as_mut() {
            tls.alps_protocols = None;
        }
    }
    emu
}

/// Chromium's HTTP/1.1 WebSocket handshake: Title-Case names in this order. Names we don't carry
/// (the library fills in `Host`, `Upgrade`, `Connection` and the `Sec-WebSocket-*` pair) still
/// take their place; any extra header of ours follows `Accept-Language`.
fn chromium_ws_order(ours: &[(String, String)]) -> wreq::header::OrigHeaderMap {
    const ORDER: [&str; 12] = [
        "Host", "Connection", "Pragma", "Cache-Control", "User-Agent", "Upgrade", "Origin", "Sec-WebSocket-Version", "Accept-Encoding", "Accept-Language",
        "Sec-WebSocket-Key", "Sec-WebSocket-Extensions",
    ];
    let mut order = wreq::header::OrigHeaderMap::with_capacity(ORDER.len() + ours.len());
    for name in ORDER {
        order.insert(name.to_string());
    }
    for (k, _) in ours {
        if !ORDER.iter().any(|o| o.eq_ignore_ascii_case(k)) {
            order.insert(title_case(k));
        }
    }
    order
}

fn title_case(name: &str) -> String {
    name.split('-').map(|p| { let mut c = p.chars(); c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default() }).collect::<Vec<_>>().join("-")
}

fn wreq_headers(headers: &[(String, String)]) -> Result<(wreq::header::HeaderMap, wreq::header::OrigHeaderMap)> {
    let mut map = wreq::header::HeaderMap::with_capacity(headers.len());
    let mut order = wreq::header::OrigHeaderMap::with_capacity(headers.len());
    for (k, v) in headers {
        let name = wreq::header::HeaderName::from_bytes(k.as_bytes()).map_err(|e| Error::Other(e.to_string()))?;
        let value = wreq::header::HeaderValue::from_str(v).map_err(|e| Error::Other(e.to_string()))?;
        order.insert(name.clone());
        map.append(name, value);
    }
    Ok((map, order))
}

impl Transport {
    pub fn new(saved_cookies: Option<&str>) -> Result<Self> {
        install_crypto();
        let client = if uses_chrome_tls() {
            let build = |ws: bool| {
                wreq::Client::builder()
                    .emulation(chrome_emulation(ws))
                    .redirect(wreq::redirect::Policy::none())
                    .pool_idle_timeout(Duration::from_secs(90))
                    .tcp_keepalive(Duration::from_secs(60))
                    .connect_timeout(Duration::from_secs(15))
                    .build()
            };
            Engine::Chrome(build(false)?, build(true)?)
        } else {
            Engine::Rustls(
                reqwest::Client::builder()
                    .use_rustls_tls()
                    // The default 64 KiB window caps one upload stream at window / round-trip time.
                    .http2_adaptive_window(true)
                    .redirect(reqwest::redirect::Policy::none())
                    .pool_idle_timeout(Duration::from_secs(90))
                    .tcp_keepalive(Duration::from_secs(60))
                    .connect_timeout(Duration::from_secs(15))
                    .build()?,
            )
        };
        let jar = saved_cookies.map(CookieJar::from_saved).unwrap_or_default();
        Ok(Self { client, cookies: Mutex::new(jar) })
    }

    pub fn cookie_header(&self) -> String {
        self.cookies.lock().save()
    }

    pub fn saved_cookies(&self) -> String {
        self.cookies.lock().save()
    }

    pub async fn send(
        &self,
        method: Method,
        url: &str,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
    ) -> Result<Response> {
        let (status, headers, body) = match &self.client {
            Engine::Chrome(c, _) => {
                let (map, order) = wreq_headers(&headers)?;
                let mut req = c.request(method, url).headers(map).orig_headers(order);
                if let Some(b) = body {
                    req = req.body(b);
                }
                let resp = req.send().await?;
                (resp.status().as_u16(), resp.headers().clone(), resp.bytes().await?)
            }
            Engine::Rustls(c) => {
                let mut req = c.request(method, url).headers(header_map(headers)?);
                if let Some(b) = body {
                    req = req.body(b);
                }
                let resp = req.send().await?;
                (resp.status().as_u16(), resp.headers().clone(), resp.bytes().await?)
            }
        };
        self.cookies.lock().absorb(&headers);
        Ok(Response { status, headers, body })
    }

    /// Plain request to a third-party host (attachment upload URLs) with no Discord headers.
    pub async fn put_raw(&self, url: &str, content_type: &str, body: Vec<u8>) -> Result<u16> {
        Ok(match &self.client {
            Engine::Chrome(c, _) => c.put(url).header("content-type", content_type).body(body).send().await?.status().as_u16(),
            Engine::Rustls(c) => c.put(url).header("content-type", content_type).body(body).send().await?.status().as_u16(),
        })
    }

    /// Like [`put_raw`](Self::put_raw), with the caller's whole header list (it carries `content-length`), but the body is fed in 64 KiB pieces: `on_bytes` hears about
    /// each 256 KiB piece as the connection takes it, and setting `cancel` ends the request with
    /// [`Error::Cancelled`]. Costs nothing while idle: no timers, only the pulls the socket makes.
    pub async fn put_progress(
        &self,
        url: &str,
        headers: Vec<(String, String)>,
        body: bytes::Bytes,
        on_bytes: std::sync::Arc<dyn Fn(u64) + Send + Sync>,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<u16> {
        use std::sync::atomic::Ordering;
        const CHUNK: usize = 256 * 1024;
        let data = body;
        let flag = cancel.clone();
        let stream = futures_util::stream::unfold(0usize, move |at| {
            let (data, on_bytes, flag) = (data.clone(), on_bytes.clone(), flag.clone());
            async move {
                if at >= data.len() {
                    return None;
                }
                if flag.load(Ordering::Relaxed) {
                    return Some((Err(std::io::Error::other("cancelled")), data.len()));
                }
                let end = (at + CHUNK).min(data.len());
                on_bytes((end - at) as u64);
                Some((Ok::<_, std::io::Error>(data.slice(at..end)), end))
            }
        });
        let result: Result<u16> = match &self.client {
            Engine::Chrome(c, _) => {
                let (map, order) = wreq_headers(&headers)?;
                c.put(url)
                    .headers(map)
                    .orig_headers(order)
                    .body(wreq::Body::wrap_stream(stream))
                    .send()
                    .await
                    .map(|r| r.status().as_u16())
                    .map_err(Error::from)
            }
            Engine::Rustls(c) => c
                .put(url)
                .headers(header_map(headers)?)
                .body(reqwest::Body::wrap_stream(stream))
                .send()
                .await
                .map(|r| r.status().as_u16())
                .map_err(Error::from),
        };
        match result {
            Err(_) if cancel.load(Ordering::Relaxed) => Err(Error::Cancelled),
            other => other,
        }
    }

    pub async fn websocket(&self, url: &str, headers: Vec<(String, String)>) -> Result<WebSocket> {
        install_crypto();
        if let Engine::Chrome(_, c) = &self.client {
            use tokio_tungstenite::tungstenite::handshake::{client::generate_key, derive_accept_key};
            let key = generate_key();
            let mut all = headers;
            for (k, v) in [
                ("connection", "Upgrade"),
                ("upgrade", "websocket"),
                ("sec-websocket-version", "13"),
                ("sec-websocket-key", key.as_str()),
                ("sec-websocket-extensions", "permessage-deflate; client_max_window_bits"),
            ] {
                all.push((k.into(), v.into()));
            }
            let (map, _) = wreq_headers(&all)?;
            let order = chromium_ws_order(&all);
            let http_url = url.replacen("wss://", "https://", 1).replacen("ws://", "http://", 1);
            let resp = c.get(http_url).version(wreq::Version::HTTP_11).headers(map).orig_headers(order).send().await?;
            if resp.status().as_u16() != 101 {
                return Err(Error::UpgradeRefused(resp.status().as_u16()));
            }
            let accept = resp.headers().get("sec-websocket-accept").and_then(|v| v.to_str().ok()).map(str::to_owned);
            if accept.as_deref() != Some(derive_accept_key(key.as_bytes()).as_str()) {
                return Err(Error::Network("websocket: bad Sec-WebSocket-Accept".into()));
            }
            let ext = resp.headers().get("sec-websocket-extensions").and_then(|v| v.to_str().ok()).map(str::to_owned);
            let io = resp.upgrade().await?;
            return Ok(WebSocket { inner: Inner::Chrome(crate::wsraw::RawWs::new(io, &[], ext.as_deref())) });
        }
        let mut req = url.into_client_request()?;
        for (k, v) in headers {
            let name = HeaderName::from_bytes(k.as_bytes()).map_err(|e| Error::Other(e.to_string()))?;
            let value = HeaderValue::from_str(&v).map_err(|e| Error::Other(e.to_string()))?;
            req.headers_mut().insert(name, value);
        }
        let config = WebSocketConfig::default().max_message_size(Some(64 << 20)).max_frame_size(Some(16 << 20));
        let (inner, _resp) = tokio_tungstenite::connect_async_with_config(req, Some(config), true).await?;
        Ok(WebSocket { inner: Inner::Rustls(inner) })
    }
}

type RustlsWs = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

enum Inner {
    Chrome(crate::wsraw::RawWs<wreq::Upgraded>),
    Rustls(RustlsWs),
}

pub struct WebSocket {
    inner: Inner,
}

impl WebSocket {
    pub async fn recv(&mut self) -> Option<Result<WsMessage>> {
        match &mut self.inner {
            Inner::Chrome(ws) => {
                let m = ws.recv().await;
                if let Some(Ok(WsMessage::Ping(p))) = &m {
                    let _ = ws.pong(p.clone()).await;
                }
                m
            }
            Inner::Rustls(ws) => ws.next().await.map(|r| r.map_err(Error::from)),
        }
    }

    pub async fn send(&mut self, msg: WsMessage) -> Result<()> {
        match &mut self.inner {
            Inner::Chrome(ws) => ws.send(msg).await,
            Inner::Rustls(ws) => ws.send(msg).await.map_err(Error::from),
        }
    }

    pub async fn close(&mut self, code: u16, reason: &str) -> Result<()> {
        use tokio_tungstenite::tungstenite::protocol::{CloseFrame, frame::coding::CloseCode};
        let frame = CloseFrame { code: CloseCode::from(code), reason: reason.to_string().into() };
        self.send(WsMessage::Close(Some(frame))).await
    }
}

fn header_map(headers: Vec<(String, String)>) -> Result<HeaderMap> {
    let mut map = HeaderMap::with_capacity(headers.len());
    for (k, v) in headers {
        let name = HeaderName::from_bytes(k.as_bytes()).map_err(|e| Error::Other(e.to_string()))?;
        let value = HeaderValue::from_str(&v).map_err(|e| Error::Other(e.to_string()))?;
        map.append(name, value);
    }
    Ok(map)
}

/// Minimal jar for discord.com: name → value, persisted per account.
/// Discord's cookies (`__dcfduid`, `__sdcfduid`, `__cfruid`, `_cfuvid`) are host-wide.
#[derive(Default)]
struct CookieJar {
    cookies: BTreeMap<String, String>,
}

impl CookieJar {
    fn from_saved(s: &str) -> Self {
        let cookies = s
            .split("; ")
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Self { cookies }
    }

    fn absorb(&mut self, headers: &HeaderMap) {
        for v in headers.get_all("set-cookie") {
            let Ok(s) = v.to_str() else { continue };
            let pair = s.split(';').next().unwrap_or("");
            let Some((k, val)) = pair.split_once('=') else { continue };
            let expired = s.to_ascii_lowercase().contains("max-age=0");
            if expired || val.is_empty() {
                self.cookies.remove(k.trim());
            } else {
                self.cookies.insert(k.trim().to_string(), val.trim().to_string());
            }
        }
    }

    fn save(&self) -> String {
        self.cookies.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ")
    }
}

#[cfg(test)]
mod upload_tests {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;

    use super::*;

    /// A one-shot HTTP server: reads the whole request (headers, then `content-length` bytes) and
    /// answers `status`. Returns its address and the number of body bytes it received.
    async fn server(status: u16) -> (String, tokio::task::JoinHandle<usize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/up", listener.local_addr().unwrap());
        let h = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 16384];
            let (mut head_end, mut want) = (None, 0usize);
            loop {
                let n = sock.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if head_end.is_none() {
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        head_end = Some(i + 4);
                        let head = String::from_utf8_lossy(&buf[..i]).to_lowercase();
                        want = head.lines().find_map(|l| l.strip_prefix("content-length: ")).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
                    }
                }
                if head_end.is_some_and(|h| buf.len() >= h + want) {
                    break;
                }
            }
            let got = head_end.map_or(0, |h| buf.len() - h);
            let _ = sock.write_all(format!("HTTP/1.1 {status} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").as_bytes()).await;
            got
        });
        (url, h)
    }

    #[tokio::test]
    async fn progress_reports_every_byte_of_the_body() {
        let (url, srv) = server(200).await;
        let t = Transport::new(None).unwrap();
        let seen = Arc::new(AtomicU64::new(0));
        let s = seen.clone();
        let body = bytes::Bytes::from(vec![7u8; 300_000]);
        let status = t.put_progress(&url, vec![("content-type".into(), "application/zip".into()), ("content-length".into(), "300000".into())], body, Arc::new(move |n| { s.fetch_add(n, Ordering::Relaxed); }), Arc::new(AtomicBool::new(false))).await.unwrap();
        assert_eq!(status, 200);
        assert_eq!(seen.load(Ordering::Relaxed), 300_000);
        assert_eq!(srv.await.unwrap(), 300_000, "the server received the whole body");
    }

    #[tokio::test]
    async fn a_refusal_status_is_returned_not_thrown() {
        let (url, _srv) = server(413).await;
        let t = Transport::new(None).unwrap();
        let status = t.put_progress(&url, vec![("content-type".into(), "x/y".into()), ("content-length".into(), "10".into())], bytes::Bytes::from(vec![1; 10]), Arc::new(|_| {}), Arc::new(AtomicBool::new(false))).await.unwrap();
        assert_eq!(status, 413);
    }

    #[tokio::test]
    async fn cancelling_stops_the_upload() {
        let (url, _srv) = server(200).await;
        let t = Transport::new(None).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        // Cancel as soon as the first piece has been taken.
        let r = t
            .put_progress(&url, vec![("content-type".into(), "x/y".into()), ("content-length".into(), "5000000".into())], bytes::Bytes::from(vec![0; 5_000_000]), Arc::new(move |_| flag.store(true, Ordering::Relaxed)), cancel)
            .await;
        assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
    }
}

/// Handshake shape, checked against a listener on 127.0.0.1: nothing here leaves the machine.
#[cfg(test)]
mod hello_tests {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncReadExt;

    use super::*;

    struct Hello {
        ciphers: Vec<u16>,
        exts: Vec<u16>,
        alpn: Vec<String>,
        sni: bool,
    }

    fn is_grease(v: u16) -> bool {
        v & 0x0f0f == 0x0a0a
    }

    fn parse(b: &[u8]) -> Hello {
        let u16at = |i: usize| u16::from_be_bytes([b[i], b[i + 1]]) as usize;
        let mut i = 5 + 4 + 2 + 32; // record, handshake header, version, random
        i += 1 + b[i] as usize; // session id
        let n = u16at(i);
        let ciphers = (0..n / 2).map(|k| u16at(i + 2 + k * 2) as u16).collect();
        i += 2 + n;
        i += 1 + b[i] as usize; // compression
        let end = i + 2 + u16at(i);
        i += 2;
        let (mut exts, mut alpn, mut sni) = (vec![], vec![], false);
        while i < end {
            let (t, l) = (u16at(i) as u16, u16at(i + 2));
            exts.push(t);
            sni |= t == 0;
            if t == 16 {
                let mut j = i + 6;
                while j < i + 4 + l {
                    alpn.push(String::from_utf8_lossy(&b[j + 1..j + 1 + b[j] as usize]).into_owned());
                    j += 1 + b[j] as usize;
                }
            }
            i += 4 + l;
        }
        Hello { ciphers, exts, alpn, sni }
    }

    /// JA4 `a_b_c` (c without the signature-algorithm suffix) of a parsed hello.
    fn ja4(h: &Hello) -> String {
        let mut c: Vec<u16> = h.ciphers.iter().copied().filter(|v| !is_grease(*v)).collect();
        let n_c = c.len();
        c.sort();
        let mut e: Vec<u16> = h.exts.iter().copied().filter(|v| !is_grease(*v) && *v != 0 && *v != 16).collect();
        let n_e = h.exts.iter().filter(|v| !is_grease(**v)).count();
        e.sort();
        let hex = |v: &[u16]| v.iter().map(|x| format!("{x:04x}")).collect::<Vec<_>>().join(",");
        let h12 = |s: String| Sha256::digest(s.as_bytes()).iter().take(6).map(|b| format!("{b:02x}")).collect::<String>();
        let alpn = h.alpn.first().map(|a| if a == "h2" { "h2".to_string() } else { "h1".to_string() }).unwrap_or_else(|| "00".into());
        format!("t13{}{n_c:02}{n_e:02}{alpn}_{}_{}", if h.sni { 'd' } else { 'i' }, h12(hex(&c)), h12(hex(&e)))
    }

    async fn capture(ws: bool) -> Hello {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let srv = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = s.read(&mut buf).await.unwrap();
            buf.truncate(n);
            buf // dropping the socket makes the client fail; we only wanted the hello
        });
        let t = Transport::new(None).unwrap();
        let url = format!("https://localhost:{port}/");
        if ws {
            let _ = t.websocket(&url.replace("https", "wss"), vec![]).await;
        } else {
            let _ = t.send(Method::GET, &url, vec![], None).await;
        }
        parse(&srv.await.unwrap())
    }

    #[tokio::test]
    async fn rest_hello_looks_like_chrome() {
        if !uses_chrome_tls() {
            return;
        }
        let h = capture(false).await;
        println!("REST ja4 {} ciphers {:04x?} exts {:04x?} alpn {:?}", ja4(&h), h.ciphers, h.exts, h.alpn);
        assert!(h.ciphers.iter().any(|c| is_grease(*c)), "GREASE cipher");
        assert_eq!(h.alpn, ["h2", "http/1.1"]);
        assert!(h.exts.contains(&0x001b), "certificate compression");
        assert!(h.exts.contains(&0x0023), "session ticket");
    }

    #[tokio::test]
    async fn websocket_hello_offers_http1_only() {
        if !uses_chrome_tls() {
            return;
        }
        let h = capture(true).await;
        println!("WS ja4 {} alpn {:?} exts {:04x?}", ja4(&h), h.alpn, h.exts);
        assert_eq!(h.alpn, ["http/1.1"]);
        assert!(!h.exts.contains(&0x44cd), "Chromium sends ALPS only when h2 is offered");
    }

    /// The gateway upgrade as bytes (plain ws:// to a local socket): Chromium's order and casing.
    #[tokio::test]
    async fn websocket_request_is_chromium_ordered_and_title_cased() {
        if !uses_chrome_tls() {
            return;
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let srv = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = s.read(&mut buf).await.unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        let t = Transport::new(None).unwrap();
        let headers = [("pragma", "no-cache"), ("cache-control", "no-cache"), ("user-agent", "UA"), ("origin", "https://discord.com"), ("accept-encoding", "gzip, deflate, br, zstd"), ("accept-language", "en-AU,en;q=0.9")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let _ = t.websocket(&format!("ws://127.0.0.1:{port}/?v=9&encoding=etf"), headers).await;
        let req = srv.await.unwrap();
        println!("{req}");
        let names: Vec<&str> = req.lines().skip(1).filter_map(|l| l.split(':').next()).filter(|n| !n.is_empty()).collect();
        assert_eq!(
            names,
            ["Host", "Connection", "Pragma", "Cache-Control", "User-Agent", "Upgrade", "Origin", "Sec-WebSocket-Version", "Accept-Encoding", "Accept-Language", "Sec-WebSocket-Key", "Sec-WebSocket-Extensions"]
        );
    }

    /// The whole path on loopback: our handshake, a standard server, messages both ways.
    #[tokio::test]
    async fn websocket_round_trip_through_the_chrome_handshake() {
        if !uses_chrome_tls() {
            return;
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let srv = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            ws.send(WsMessage::Text("hello".into())).await.unwrap();
            ws.next().await.unwrap().unwrap()
        });
        let t = Transport::new(None).unwrap();
        let mut ws = t.websocket(&format!("ws://127.0.0.1:{port}/?v=9"), vec![("user-agent".into(), "UA".into())]).await.unwrap();
        assert!(matches!(ws.recv().await, Some(Ok(WsMessage::Text(t))) if t.as_str() == "hello"));
        ws.send(WsMessage::Binary(vec![1, 2, 3].into())).await.unwrap();
        assert!(matches!(srv.await.unwrap(), WsMessage::Binary(b) if &b[..] == [1, 2, 3]));
    }
}
