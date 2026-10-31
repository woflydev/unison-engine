//! A minimal RFC 6455 client over an already-upgraded stream, with permessage-deflate (RFC 7692).
//!
//! The Chrome transport does its own upgrade so every header of the handshake is ours (casing,
//! order, `Sec-WebSocket-Extensions`); this reads what comes back. Receiving handles fragments,
//! ping/pong, close and deflated messages. Sending is never compressed (allowed by the RFC).
//! `recv` keeps partial frames in `buf`, so dropping its future (a `select!` branch losing) loses
//! nothing.

use bytes::{Buf, BytesMut};
use flate2::{Decompress, FlushDecompress};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_tungstenite::tungstenite::protocol::{frame::coding::CloseCode, CloseFrame};
use tokio_tungstenite::tungstenite::Message;

use crate::{Error, Result};

const MAX_MESSAGE: usize = 64 << 20;

pub struct RawWs<S> {
    io: S,
    buf: BytesMut,
    /// Opcode, compressed flag and bytes of the data message being reassembled.
    partial: Option<(u8, bool, Vec<u8>)>,
    inflate: Option<(Decompress, bool)>,
    closed: bool,
}

impl<S: AsyncRead + AsyncWrite + Unpin> RawWs<S> {
    /// `extensions` is the server's `Sec-WebSocket-Extensions` answer, if any.
    pub fn new(io: S, leftover: &[u8], extensions: Option<&str>) -> Self {
        let ext = extensions.unwrap_or("").to_ascii_lowercase();
        let inflate = ext.contains("permessage-deflate").then(|| (Decompress::new(false), ext.contains("server_no_context_takeover")));
        Self { io, buf: BytesMut::from(leftover), partial: None, inflate, closed: false }
    }

    pub async fn recv(&mut self) -> Option<Result<Message>> {
        loop {
            match self.parse() {
                Ok(Some(m)) => return Some(Ok(m)),
                Ok(None) => {}
                Err(e) => return Some(Err(e)),
            }
            if self.closed {
                return None;
            }
            match self.io.read_buf(&mut self.buf).await {
                Ok(0) => return None,
                Ok(_) => {}
                Err(e) => return Some(Err(Error::Network(e.to_string()))),
            }
        }
    }

    /// One complete message from `buf`, answering pings on the way. `None` = need more bytes.
    fn parse(&mut self) -> Result<Option<Message>> {
        loop {
            let b = &self.buf[..];
            if b.len() < 2 {
                return Ok(None);
            }
            let (fin, rsv1, op) = (b[0] & 0x80 != 0, b[0] & 0x40 != 0, b[0] & 0x0f);
            if b[0] & 0x30 != 0 {
                return Err(Error::Network("websocket: reserved bits set".into()));
            }
            let masked = b[1] & 0x80 != 0;
            let (len, head) = match b[1] & 0x7f {
                126 if b.len() >= 4 => (u16::from_be_bytes([b[2], b[3]]) as usize, 4),
                127 if b.len() >= 10 => (u64::from_be_bytes(b[2..10].try_into().unwrap()) as usize, 10),
                126 | 127 => return Ok(None),
                n => (n as usize, 2),
            };
            if len > MAX_MESSAGE {
                return Err(Error::Network("websocket: frame too large".into()));
            }
            let head = head + if masked { 4 } else { 0 };
            if b.len() < head + len {
                return Ok(None);
            }
            let mut payload = b[head..head + len].to_vec();
            if masked {
                let k = [b[head - 4], b[head - 3], b[head - 2], b[head - 1]];
                payload.iter_mut().enumerate().for_each(|(i, p)| *p ^= k[i % 4]);
            }
            self.buf.advance(head + len);

            match op {
                0x8 => {
                    self.closed = true;
                    let frame = (payload.len() >= 2).then(|| CloseFrame {
                        code: CloseCode::from(u16::from_be_bytes([payload[0], payload[1]])),
                        reason: String::from_utf8_lossy(&payload[2..]).into_owned().into(),
                    });
                    return Ok(Some(Message::Close(frame)));
                }
                0x9 => return Ok(Some(Message::Ping(payload.into()))),
                0xA => return Ok(Some(Message::Pong(payload.into()))),
                0x1 | 0x2 => {
                    if self.partial.is_some() {
                        return Err(Error::Network("websocket: new message inside a fragmented one".into()));
                    }
                    self.partial = Some((op, rsv1, payload));
                }
                0x0 => match self.partial.as_mut() {
                    Some((_, _, data)) => {
                        data.extend_from_slice(&payload);
                        if data.len() > MAX_MESSAGE {
                            return Err(Error::Network("websocket: message too large".into()));
                        }
                    }
                    None => return Err(Error::Network("websocket: stray continuation".into())),
                },
                _ => return Err(Error::Network("websocket: unknown opcode".into())),
            }
            if fin {
                let (op, compressed, mut data) = self.partial.take().unwrap();
                if compressed {
                    data = self.inflate_message(&data)?;
                }
                return Ok(Some(if op == 0x1 {
                    Message::Text(String::from_utf8(data).map_err(|_| Error::Network("websocket: invalid utf-8".into()))?.into())
                } else {
                    Message::Binary(data.into())
                }));
            }
        }
    }

    fn inflate_message(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        let Some((d, reset)) = self.inflate.as_mut() else {
            return Err(Error::Network("websocket: compressed frame without the extension".into()));
        };
        let mut input = data.to_vec();
        input.extend_from_slice(&[0, 0, 0xff, 0xff]);
        let mut out = Vec::with_capacity(data.len() * 4);
        let start_in = d.total_in();
        loop {
            let consumed = (d.total_in() - start_in) as usize;
            if consumed >= input.len() {
                break;
            }
            out.reserve(16 * 1024);
            d.decompress_vec(&input[consumed..], &mut out, FlushDecompress::Sync).map_err(|e| Error::Network(format!("websocket: inflate: {e}")))?;
            if out.len() > MAX_MESSAGE {
                return Err(Error::Network("websocket: message too large".into()));
            }
        }
        if *reset {
            d.reset(false);
        }
        Ok(out)
    }

    pub async fn send(&mut self, msg: Message) -> Result<()> {
        let (op, payload): (u8, Vec<u8>) = match msg {
            Message::Text(t) => (0x1, t.as_bytes().to_vec()),
            Message::Binary(b) => (0x2, b.to_vec()),
            Message::Ping(b) => (0x9, b.to_vec()),
            Message::Pong(b) => (0xA, b.to_vec()),
            Message::Close(f) => (0x8, f.map(|f| [u16::from(f.code).to_be_bytes().to_vec(), f.reason.as_bytes().to_vec()].concat()).unwrap_or_default()),
            Message::Frame(_) => return Ok(()),
        };
        let mut out = Vec::with_capacity(payload.len() + 14);
        out.push(0x80 | op);
        match payload.len() {
            n if n < 126 => out.push(0x80 | n as u8),
            n if n <= u16::MAX as usize => {
                out.push(0x80 | 126);
                out.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                out.push(0x80 | 127);
                out.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        let key: [u8; 4] = fastrand::u32(..).to_be_bytes();
        out.extend_from_slice(&key);
        out.extend(payload.iter().enumerate().map(|(i, p)| p ^ key[i % 4]));
        self.io.write_all(&out).await.map_err(|e| Error::Network(e.to_string()))?;
        self.io.flush().await.map_err(|e| Error::Network(e.to_string()))
    }

    /// Answer a ping (called by the owner when `recv` yields one).
    pub async fn pong(&mut self, payload: bytes::Bytes) -> Result<()> {
        self.send(Message::Pong(payload)).await
    }
}

#[cfg(test)]
mod tests {
    use flate2::{Compress, Compression, FlushCompress};
    use tokio::io::duplex;

    use super::*;

    fn frame(op: u8, fin: bool, rsv1: bool, payload: &[u8]) -> Vec<u8> {
        let mut f = vec![(fin as u8) << 7 | (rsv1 as u8) << 6 | op];
        assert!(payload.len() < 126);
        f.push(payload.len() as u8);
        f.extend_from_slice(payload);
        f
    }

    fn deflate(c: &mut Compress, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() + 64);
        c.compress_vec(data, &mut out, FlushCompress::Sync).unwrap();
        out.truncate(out.len() - 4); // RFC 7692: drop the 00 00 ff ff tail
        out
    }

    #[tokio::test]
    async fn plain_fragmented_and_control_frames() {
        let (a, mut b) = duplex(4096);
        let mut ws = RawWs::new(a, &[], None);
        b.write_all(&[frame(0x1, false, false, b"hel"), frame(0x9, true, false, b"p"), frame(0x0, true, false, b"lo"), frame(0x2, true, false, &[1, 2])].concat()).await.unwrap();
        assert!(matches!(ws.recv().await, Some(Ok(Message::Ping(p))) if &p[..] == b"p"));
        assert!(matches!(ws.recv().await, Some(Ok(Message::Text(t))) if t.as_str() == "hello"));
        assert!(matches!(ws.recv().await, Some(Ok(Message::Binary(d))) if &d[..] == [1, 2]));
    }

    #[tokio::test]
    async fn deflated_messages_with_context_takeover() {
        let (a, mut b) = duplex(4096);
        let mut ws = RawWs::new(a, &[], Some("permessage-deflate; client_max_window_bits=15"));
        let mut c = Compress::new(Compression::default(), false);
        for text in ["{\"op\":10,\"d\":{\"heartbeat_interval\":41250}}", "{\"op\":10,\"d\":{\"heartbeat_interval\":41250}}"] {
            b.write_all(&frame(0x1, true, true, &deflate(&mut c, text.as_bytes()))).await.unwrap();
            assert!(matches!(ws.recv().await, Some(Ok(Message::Text(t))) if t.as_str() == text));
        }
    }

    #[tokio::test]
    async fn a_partial_frame_survives_a_dropped_recv() {
        let (a, mut b) = duplex(4096);
        let mut ws = RawWs::new(a, &[], None);
        let f = frame(0x2, true, false, &[9, 8, 7]);
        b.write_all(&f[..3]).await.unwrap();
        let _ = tokio::time::timeout(std::time::Duration::from_millis(20), ws.recv()).await; // dropped mid-frame
        b.write_all(&f[3..]).await.unwrap();
        assert!(matches!(ws.recv().await, Some(Ok(Message::Binary(d))) if &d[..] == [9, 8, 7]));
    }

    #[tokio::test]
    async fn sent_frames_are_masked_and_decodable() {
        let (a, mut b) = duplex(4096);
        let mut ws = RawWs::new(a, &[], None);
        ws.send(Message::Binary(vec![5u8; 300].into())).await.unwrap();
        let mut head = [0u8; 8];
        b.read_exact(&mut head).await.unwrap();
        assert_eq!((head[0], head[1]), (0x82, 0x80 | 126));
        assert_eq!(u16::from_be_bytes([head[2], head[3]]), 300);
    }
}
