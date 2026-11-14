//! RTMP: the handshake, the chunk stream and the publishing client. Written from Adobe's public
//! "Real-Time Messaging Protocol (RTMP) specification" (2012). The client publishes only: it
//! connects, creates a stream, publishes under the stream key and then sends metadata, audio and
//! video messages, answering the server's pings and acknowledging what it receives.

use super::amf::{self, Amf};
use super::services::{Endpoint, mask};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const MSG_SET_CHUNK_SIZE: u8 = 1;
pub const MSG_ABORT: u8 = 2;
pub const MSG_ACK: u8 = 3;
pub const MSG_USER_CONTROL: u8 = 4;
pub const MSG_WINDOW_ACK_SIZE: u8 = 5;
pub const MSG_SET_PEER_BANDWIDTH: u8 = 6;
pub const MSG_AUDIO: u8 = 8;
pub const MSG_VIDEO: u8 = 9;
pub const MSG_DATA_AMF0: u8 = 18;
pub const MSG_COMMAND_AMF0: u8 = 20;

/// Chunk size we send with (servers default to 128 until told).
pub const OUT_CHUNK_SIZE: u32 = 4096;
const HANDSHAKE_LEN: usize = 1536;
/// Biggest message we accept from the server (it only sends small ones).
const MAX_IN_MESSAGE: usize = 4 << 20;

/// One RTMP message.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub csid: u32,
    pub type_id: u8,
    pub stream_id: u32,
    /// Milliseconds.
    pub timestamp: u32,
    pub payload: Vec<u8>,
}

impl Message {
    pub fn new(csid: u32, type_id: u8, stream_id: u32, timestamp: u32, payload: Vec<u8>) -> Message {
        Message { csid, type_id, stream_id, timestamp, payload }
    }

    pub fn command(csid: u32, stream_id: u32, values: &[Amf]) -> Message {
        Message::new(csid, MSG_COMMAND_AMF0, stream_id, 0, amf::encode(values))
    }

    pub fn control(type_id: u8, payload: Vec<u8>) -> Message {
        Message::new(2, type_id, 0, 0, payload)
    }
}

fn basic_header(out: &mut Vec<u8>, fmt: u8, csid: u32) {
    match csid {
        2..=63 => out.push((fmt << 6) | csid as u8),
        64..=319 => {
            out.push(fmt << 6);
            out.push((csid - 64) as u8);
        }
        _ => {
            let c = csid.clamp(320, 65599) - 64;
            out.push((fmt << 6) | 1);
            out.push(c as u8);
            out.push((c >> 8) as u8);
        }
    }
}

/// A message as chunks: a type-0 header, then type-3 continuation headers every `chunk_size`
/// bytes. Timestamps of 0xFFFFFF and above use the extended timestamp field (repeated on every
/// continuation chunk, as the specification's readers expect).
pub fn encode_chunks(m: &Message, chunk_size: u32) -> Vec<u8> {
    let cs = chunk_size.max(1) as usize;
    let ext = m.timestamp >= 0xFF_FFFF;
    let mut out = Vec::with_capacity(m.payload.len() + 16 + m.payload.len() / cs * 5);
    basic_header(&mut out, 0, m.csid);
    let ts = if ext { 0xFF_FFFF } else { m.timestamp };
    out.extend_from_slice(&ts.to_be_bytes()[1..]);
    out.extend_from_slice(&(m.payload.len() as u32).to_be_bytes()[1..]);
    out.push(m.type_id);
    out.extend_from_slice(&m.stream_id.to_le_bytes());
    if ext {
        out.extend_from_slice(&m.timestamp.to_be_bytes());
    }
    for (i, part) in m.payload.chunks(cs).enumerate() {
        if i > 0 {
            basic_header(&mut out, 3, m.csid);
            if ext {
                out.extend_from_slice(&m.timestamp.to_be_bytes());
            }
        }
        out.extend_from_slice(part);
    }
    out
}

#[derive(Clone, Default)]
struct InStream {
    timestamp: u32,
    delta: u32,
    length: usize,
    type_id: u8,
    stream_id: u32,
    ext: bool,
    partial: Vec<u8>,
}

/// Reads chunks into messages. Bytes are fed in as they arrive (any amount); finished messages
/// come out of `next`.
pub struct ChunkReader {
    buf: Vec<u8>,
    pos: usize,
    chunk_size: usize,
    streams: std::collections::HashMap<u32, InStream>,
}

impl Default for ChunkReader {
    fn default() -> Self {
        ChunkReader { buf: Vec::new(), pos: 0, chunk_size: 128, streams: Default::default() }
    }
}

impl ChunkReader {
    pub fn feed(&mut self, data: &[u8]) {
        if self.pos > 0 && self.pos == self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        } else if self.pos > 64 * 1024 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        self.buf.extend_from_slice(data);
    }

    pub fn set_chunk_size(&mut self, n: u32) {
        self.chunk_size = (n & 0x7FFF_FFFF).clamp(1, 16 << 20) as usize;
    }

    /// The next whole message, None until enough bytes have arrived.
    pub fn next(&mut self) -> Result<Option<Message>, String> {
        loop {
            match self.one_chunk()? {
                None => return Ok(None),
                Some(Some(m)) => {
                    if m.type_id == MSG_SET_CHUNK_SIZE && m.payload.len() >= 4 {
                        self.set_chunk_size(u32::from_be_bytes([m.payload[0], m.payload[1], m.payload[2], m.payload[3]]));
                    }
                    if m.type_id == MSG_ABORT && m.payload.len() >= 4 {
                        let c = u32::from_be_bytes([m.payload[0], m.payload[1], m.payload[2], m.payload[3]]);
                        if let Some(s) = self.streams.get_mut(&c) {
                            s.partial.clear();
                        }
                    }
                    return Ok(Some(m));
                }
                Some(None) => continue,
            }
        }
    }

    /// Parse one chunk. None: not enough bytes yet (nothing consumed). Some(None): a chunk of a
    /// message that isn't finished.
    fn one_chunk(&mut self) -> Result<Option<Option<Message>>, String> {
        let b = &self.buf[self.pos..];
        let mut at = 0usize;
        let need = |n: usize, at: usize| at + n <= b.len();
        if !need(1, at) {
            return Ok(None);
        }
        let fmt = b[0] >> 6;
        let mut csid = (b[0] & 0x3F) as u32;
        at += 1;
        if csid == 0 {
            if !need(1, at) {
                return Ok(None);
            }
            csid = b[at] as u32 + 64;
            at += 1;
        } else if csid == 1 {
            if !need(2, at) {
                return Ok(None);
            }
            csid = b[at] as u32 + ((b[at + 1] as u32) << 8) + 64;
            at += 2;
        }
        let hlen = [11, 7, 3, 0][fmt as usize];
        if !need(hlen, at) {
            return Ok(None);
        }
        let u24 = |s: &[u8]| ((s[0] as u32) << 16) | ((s[1] as u32) << 8) | s[2] as u32;
        let mut st = self.streams.get(&csid).cloned().unwrap_or_default();
        if fmt < 3 && !st.partial.is_empty() {
            return Err("the server started a new message before finishing the last one".into());
        }
        let starting = st.partial.is_empty();
        let mut ts_field = 0u32;
        if fmt <= 2 {
            ts_field = u24(&b[at..]);
        }
        if fmt <= 1 {
            st.length = u24(&b[at + 3..]) as usize;
            st.type_id = b[at + 6];
            if st.length > MAX_IN_MESSAGE {
                return Err("the server sent a message that is far too big".into());
            }
        }
        if fmt == 0 {
            st.stream_id = u32::from_le_bytes([b[at + 7], b[at + 8], b[at + 9], b[at + 10]]);
        }
        at += hlen;
        if fmt <= 2 {
            st.ext = ts_field == 0xFF_FFFF;
        }
        let mut ext_val = None;
        if st.ext {
            if !need(4, at) {
                return Ok(None);
            }
            ext_val = Some(u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]));
            at += 4;
        }
        let field = ext_val.unwrap_or(ts_field);
        let take = (st.length - st.partial.len()).min(self.chunk_size);
        if !need(take, at) {
            return Ok(None);
        }
        // Only now, with the whole chunk here, does the state change.
        if starting {
            match fmt {
                0 => {
                    st.timestamp = field;
                    st.delta = 0;
                }
                1 | 2 => {
                    st.delta = field;
                    st.timestamp = st.timestamp.wrapping_add(field);
                }
                _ => st.timestamp = st.timestamp.wrapping_add(st.delta),
            }
        }
        st.partial.extend_from_slice(&b[at..at + take]);
        at += take;
        self.pos += at;
        let done = st.partial.len() >= st.length;
        let msg = if done {
            let payload = std::mem::take(&mut st.partial);
            Some(Message { csid, type_id: st.type_id, stream_id: st.stream_id, timestamp: st.timestamp, payload })
        } else {
            None
        };
        self.streams.insert(csid, st);
        Ok(Some(msg))
    }
}

/// TCP, or TCP with TLS (RTMPS).
pub enum Transport {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Transport {
    fn tcp(&self) -> &TcpStream {
        match self {
            Transport::Plain(s) => s,
            Transport::Tls(s) => &s.sock,
        }
    }

    pub fn set_read_timeout(&self, d: Duration) -> std::io::Result<()> {
        self.tcp().set_read_timeout(Some(d))
    }

    pub fn set_write_timeout(&self, d: Duration) -> std::io::Result<()> {
        self.tcp().set_write_timeout(Some(d))
    }

    pub fn set_nonblocking(&self) -> std::io::Result<()> {
        self.tcp().set_nonblocking(true)
    }

    pub fn shutdown(&mut self) {
        if let Transport::Tls(s) = self {
            s.conn.send_close_notify();
            let _ = s.flush();
        }
        let _ = self.tcp().shutdown(std::net::Shutdown::Both);
    }
}

impl Read for Transport {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Transport::Plain(s) => s.read(b),
            Transport::Tls(s) => s.read(b),
        }
    }
}

impl Write for Transport {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        match self {
            Transport::Plain(s) => s.write(b),
            Transport::Tls(s) => s.write(b),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Transport::Plain(s) => s.flush(),
            Transport::Tls(s) => s.flush(),
        }
    }
}

fn tls_config() -> Arc<rustls::ClientConfig> {
    static CFG: std::sync::OnceLock<Arc<rustls::ClientConfig>> = std::sync::OnceLock::new();
    CFG.get_or_init(|| {
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map(|b| b.with_root_certificates(roots).with_no_client_auth());
        Arc::new(cfg.unwrap_or_else(|_| {
            // Can't happen with ring's defaults; an empty config fails every connection clearly.
            rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(&[&rustls::version::TLS13])
                .expect("TLS 1.3 is always available")
                .with_root_certificates(rustls::RootCertStore::empty())
                .with_no_client_auth()
        }))
    })
    .clone()
}

fn is_timeout(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)
}

/// Open the TCP (and TLS) connection.
pub fn open(ep: &Endpoint, timeout: Duration) -> Result<Transport, String> {
    let addrs: Vec<_> = (ep.host.as_str(), ep.port).to_socket_addrs().map_err(|e| format!("Can't find the server {}: {e}", ep.host))?.collect();
    let mut last = format!("Can't find the server {}", ep.host);
    let mut tcp = None;
    for a in addrs {
        match TcpStream::connect_timeout(&a, timeout) {
            Ok(s) => {
                tcp = Some(s);
                break;
            }
            Err(e) => last = format!("Can't connect to {}:{}: {e}", ep.host, ep.port),
        }
    }
    let tcp = tcp.ok_or(last)?;
    let _ = tcp.set_nodelay(true);
    tcp.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    tcp.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    if !ep.tls {
        return Ok(Transport::Plain(tcp));
    }
    let name = rustls::pki_types::ServerName::try_from(ep.host.clone()).map_err(|_| format!("{} isn't a valid server name", ep.host))?;
    let conn = rustls::ClientConnection::new(tls_config(), name).map_err(|e| format!("Secure connection failed: {e}"))?;
    let mut s = rustls::StreamOwned::new(conn, tcp);
    // Finish the TLS handshake now so certificate errors show as such.
    while s.conn.is_handshaking() {
        s.conn.complete_io(&mut s.sock).map_err(|e| format!("Secure connection to {} failed: {e}", ep.host))?;
    }
    Ok(Transport::Tls(Box::new(s)))
}

/// The simple RTMP handshake (C0+C1, S0+S1+S2, C2).
pub fn handshake<S: Read + Write>(s: &mut S) -> Result<(), String> {
    let io = |e: std::io::Error| format!("RTMP handshake failed: {e}");
    let mut c0c1 = vec![0u8; 1 + HANDSHAKE_LEN];
    c0c1[0] = 3;
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u32).unwrap_or(0);
    c0c1[1..5].copy_from_slice(&t.to_be_bytes());
    let mut x = t as u64 ^ 0x9E37_79B9_7F4A_7C15;
    for b in &mut c0c1[9..] {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = x as u8;
    }
    s.write_all(&c0c1).map_err(io)?;
    s.flush().map_err(io)?;
    let mut s0s1 = vec![0u8; 1 + HANDSHAKE_LEN];
    s.read_exact(&mut s0s1).map_err(io)?;
    if s0s1[0] != 3 {
        return Err(format!("The server speaks RTMP version {}, not 3", s0s1[0]));
    }
    let mut s2 = vec![0u8; HANDSHAKE_LEN];
    s.read_exact(&mut s2).map_err(io)?;
    // C2 echoes S1.
    s.write_all(&s0s1[1..]).map_err(io)?;
    s.flush().map_err(io)
}

/// The server's side of the handshake (for the tests' server).
#[cfg(test)]
pub fn server_handshake<S: Read + Write>(s: &mut S) -> Result<(), String> {
    let io = |e: std::io::Error| e.to_string();
    let mut c0c1 = vec![0u8; 1 + HANDSHAKE_LEN];
    s.read_exact(&mut c0c1).map_err(io)?;
    let mut out = vec![3u8];
    out.extend(std::iter::repeat_n(7u8, HANDSHAKE_LEN));
    out.extend_from_slice(&c0c1[1..]);
    s.write_all(&out).map_err(io)?;
    let mut c2 = vec![0u8; HANDSHAKE_LEN];
    s.read_exact(&mut c2).map_err(io)?;
    if c2[..] != out[1..1 + HANDSHAKE_LEN] {
        return Err("C2 doesn't echo S1".into());
    }
    Ok(())
}

/// A publishing connection, after `publish` succeeded.
pub struct Client {
    io: Transport,
    reader: ChunkReader,
    pub stream_id: u32,
    /// Bytes received, for acknowledgements.
    received: u64,
    acked: u64,
    window: u64,
    /// Bytes written, for the bitrate.
    pub sent: u64,
    key: String,
    next_txn: f64,
    /// Once publishing: the socket doesn't block, and a send fails when nothing went out for
    /// this long (a dead or far too slow network).
    pub stall: Duration,
    /// Set from another thread to give up a send that is waiting on the network.
    pub abort: Option<Arc<std::sync::atomic::AtomicBool>>,
    nonblocking: bool,
}

const CSID_COMMAND: u32 = 3;
const CSID_AUDIO: u32 = 4;
const CSID_VIDEO: u32 = 6;
const CSID_DATA: u32 = 5;

/// What a received message meant to the client.
enum Event {
    Result(f64, Vec<Amf>),
    Error(f64, String),
    Status(String, String, String),
    Other,
}

fn describe(info: Option<&Amf>) -> (String, String, String) {
    let g = |k: &str| info.and_then(|i| i.get(k)).and_then(Amf::as_str).unwrap_or("").to_string();
    (g("level"), g("code"), g("description"))
}

impl Client {
    /// Connect, handshake, `connect`, `createStream` and `publish`. Every error has the key
    /// masked.
    pub fn connect(ep: &Endpoint, timeout: Duration) -> Result<Client, String> {
        let key = ep.stream.clone();
        Client::connect_inner(ep, timeout).map_err(|e| mask(&e, &key))
    }

    fn connect_inner(ep: &Endpoint, timeout: Duration) -> Result<Client, String> {
        let mut io = open(ep, timeout)?;
        handshake(&mut io)?;
        let mut c = Client { io, reader: ChunkReader::default(), stream_id: 0, received: 0, acked: 0, window: 2_500_000, sent: 0, key: ep.stream.clone(), next_txn: 1.0, stall: timeout, abort: None, nonblocking: false };
        c.send(&Message::control(MSG_SET_CHUNK_SIZE, OUT_CHUNK_SIZE.to_be_bytes().to_vec()))?;
        let txn = c.txn();
        c.send(&Message::command(
            CSID_COMMAND,
            0,
            &[
                Amf::str("connect"),
                Amf::Number(txn),
                Amf::Object(vec![
                    ("app".into(), Amf::Str(ep.app.clone())),
                    ("type".into(), Amf::str("nonprivate")),
                    ("flashVer".into(), Amf::str("FMLE/3.0 (compatible; Snagster)")),
                    ("tcUrl".into(), Amf::Str(ep.tc_url.clone())),
                ]),
            ],
        ))?;
        let deadline = Instant::now() + timeout;
        c.wait_result(txn, deadline).map_err(|e| format!("The server refused the connection: {e}"))?;
        let key = Amf::Str(ep.stream.clone());
        let t = c.txn();
        c.send(&Message::command(CSID_COMMAND, 0, &[Amf::str("releaseStream"), Amf::Number(t), Amf::Null, key.clone()]))?;
        let t = c.txn();
        c.send(&Message::command(CSID_COMMAND, 0, &[Amf::str("FCPublish"), Amf::Number(t), Amf::Null, key.clone()]))?;
        let txn = c.txn();
        c.send(&Message::command(CSID_COMMAND, 0, &[Amf::str("createStream"), Amf::Number(txn), Amf::Null]))?;
        let r = c.wait_result(txn, deadline).map_err(|e| format!("The server refused to create a stream: {e}"))?;
        c.stream_id = r.iter().rev().find_map(Amf::as_number).map(|n| n as u32).ok_or("The server didn't give a stream number")?;
        let t = c.txn();
        c.send(&Message::command(CSID_COMMAND, c.stream_id, &[Amf::str("publish"), Amf::Number(t), Amf::Null, key, Amf::str("live")]))?;
        loop {
            match c.wait_event(deadline)? {
                Event::Status(level, code, desc) => {
                    if code == "NetStream.Publish.Start" {
                        break;
                    }
                    if level == "error" || code.contains("Failed") || code.contains("BadName") || code.contains("Rejected") {
                        return Err(format!("The server refused the stream key ({code}{}{desc})", if desc.is_empty() { "" } else { ": " }));
                    }
                }
                Event::Error(_, e) => return Err(format!("The server refused the stream: {e}")),
                _ => {}
            }
        }
        c.io.set_nonblocking().map_err(|e| e.to_string())?;
        c.nonblocking = true;
        Ok(c)
    }

    fn txn(&mut self) -> f64 {
        let t = self.next_txn;
        self.next_txn += 1.0;
        t
    }

    fn wait_result(&mut self, txn: f64, deadline: Instant) -> Result<Vec<Amf>, String> {
        loop {
            match self.wait_event(deadline)? {
                Event::Result(t, v) if t == txn => return Ok(v),
                Event::Error(t, e) if t == txn => return Err(e),
                Event::Status(level, code, desc) if level == "error" => return Err(format!("{code} {desc}")),
                _ => {}
            }
        }
    }

    fn wait_event(&mut self, deadline: Instant) -> Result<Event, String> {
        loop {
            if let Some(m) = self.reader.next()? {
                return self.handle(m);
            }
            if Instant::now() > deadline {
                return Err("The server didn't answer in time".into());
            }
            self.read_some()?;
        }
    }

    /// Read what has arrived, waiting at most the socket's read timeout.
    fn read_some(&mut self) -> Result<(), String> {
        let mut buf = [0u8; 16 * 1024];
        match self.io.read(&mut buf) {
            Ok(0) => Err("The server closed the connection".into()),
            Ok(n) => {
                self.received += n as u64;
                self.reader.feed(&buf[..n]);
                Ok(())
            }
            Err(e) if is_timeout(&e) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => Ok(()),
            Err(e) => Err(format!("Connection lost: {e}")),
        }
    }

    /// Protocol upkeep for one received message.
    fn handle(&mut self, m: Message) -> Result<Event, String> {
        if self.received - self.acked >= self.window {
            self.acked = self.received;
            self.send(&Message::control(MSG_ACK, (self.received as u32).to_be_bytes().to_vec()))?;
        }
        match m.type_id {
            MSG_WINDOW_ACK_SIZE if m.payload.len() >= 4 => {
                self.window = u32::from_be_bytes([m.payload[0], m.payload[1], m.payload[2], m.payload[3]]).max(1) as u64;
            }
            MSG_USER_CONTROL if m.payload.len() >= 6 => {
                let ev = u16::from_be_bytes([m.payload[0], m.payload[1]]);
                if ev == 6 {
                    // Ping request: answer with the same timestamp.
                    let mut p = vec![0, 7];
                    p.extend_from_slice(&m.payload[2..6]);
                    self.send(&Message::control(MSG_USER_CONTROL, p))?;
                }
            }
            MSG_COMMAND_AMF0 => {
                let v = amf::decode_all(&m.payload)?;
                let name = v.first().and_then(Amf::as_str).unwrap_or("");
                let txn = v.get(1).and_then(Amf::as_number).unwrap_or(0.0);
                return Ok(match name {
                    "_result" => Event::Result(txn, v[2.min(v.len())..].to_vec()),
                    "_error" => {
                        let (_, code, desc) = describe(v.get(3));
                        Event::Error(txn, format!("{code} {desc}").trim().to_string())
                    }
                    "onStatus" => {
                        let (l, c, d) = describe(v.get(3));
                        Event::Status(l, c, d)
                    }
                    _ => Event::Other,
                });
            }
            _ => {}
        }
        Ok(Event::Other)
    }

    pub fn send(&mut self, m: &Message) -> Result<(), String> {
        let b = encode_chunks(m, if m.type_id == MSG_SET_CHUNK_SIZE { 128 } else { OUT_CHUNK_SIZE });
        if !self.nonblocking {
            self.io.write_all(&b).map_err(|e| mask(&format!("Connection lost while sending: {e}"), &self.key))?;
            self.sent += b.len() as u64;
            return Ok(());
        }
        let mut off = 0;
        let mut progress = Instant::now();
        while off < b.len() {
            match self.io.write(&b[off..]) {
                Ok(0) => return Err("The server closed the connection".into()),
                Ok(n) => {
                    off += n;
                    self.sent += n as u64;
                    progress = Instant::now();
                }
                Err(e) if is_timeout(&e) || e.kind() == std::io::ErrorKind::Interrupted => {
                    if self.abort.as_ref().is_some_and(|a| a.load(std::sync::atomic::Ordering::Relaxed)) {
                        return Err("Stopped while the network was busy".into());
                    }
                    if progress.elapsed() > self.stall {
                        return Err(format!("The network is too slow: nothing could be sent for {} s", self.stall.as_secs()));
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(e) => return Err(mask(&format!("Connection lost while sending: {e}"), &self.key)),
            }
        }
        Ok(())
    }

    pub fn send_metadata(&mut self, body: Vec<u8>) -> Result<(), String> {
        self.send(&Message::new(CSID_DATA, MSG_DATA_AMF0, self.stream_id, 0, body))
    }

    pub fn send_audio(&mut self, ts: u32, body: Vec<u8>) -> Result<(), String> {
        self.send(&Message::new(CSID_AUDIO, MSG_AUDIO, self.stream_id, ts, body))
    }

    pub fn send_video(&mut self, ts: u32, body: Vec<u8>) -> Result<(), String> {
        self.send(&Message::new(CSID_VIDEO, MSG_VIDEO, self.stream_id, ts, body))
    }

    /// Handle whatever the server sent meanwhile (acks, pings, errors) without waiting.
    pub fn poll(&mut self) -> Result<(), String> {
        // TLS keeps what the socket couldn't take yet; push it on.
        if let Err(e) = self.io.flush()
            && !is_timeout(&e)
        {
            return Err(format!("Connection lost: {e}"));
        }
        for _ in 0..8 {
            self.read_some()?;
            while let Some(m) = self.reader.next()? {
                if let Event::Status(level, code, desc) = self.handle(m)?
                    && level == "error"
                {
                    return Err(mask(&format!("The server stopped the stream: {code} {desc}"), &self.key));
                }
            }
            if self.reader.pos >= self.reader.buf.len() {
                break;
            }
        }
        Ok(())
    }



    /// Unpublish and close.
    pub fn close(mut self) {
        self.stall = self.stall.min(Duration::from_secs(2));
        let key = Amf::Str(self.key.clone());
        let t = self.txn();
        let _ = self.send(&Message::command(CSID_COMMAND, self.stream_id, &[Amf::str("FCUnpublish"), Amf::Number(t), Amf::Null, key]));
        let t = self.txn();
        let _ = self.send(&Message::command(CSID_COMMAND, 0, &[Amf::str("deleteStream"), Amf::Number(t), Amf::Null, Amf::Number(self.stream_id as f64)]));
        let _ = self.io.flush();
        self.io.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_all(bytes: &[u8], chunk_size: u32, piece: usize) -> Vec<Message> {
        let mut r = ChunkReader::default();
        r.set_chunk_size(chunk_size);
        let mut out = Vec::new();
        for p in bytes.chunks(piece.max(1)) {
            r.feed(p);
            while let Some(m) = r.next().unwrap() {
                out.push(m);
            }
        }
        out
    }

    #[test]
    fn a_small_message_has_the_spec_header() {
        let m = Message::new(3, MSG_COMMAND_AMF0, 1, 0x010203, vec![9, 9]);
        let b = encode_chunks(&m, 128);
        assert_eq!(b, vec![0x03, 1, 2, 3, 0, 0, 2, 20, 1, 0, 0, 0, 9, 9]);
    }

    #[test]
    fn big_messages_split_into_continuation_chunks() {
        let payload: Vec<u8> = (0..300u32).map(|i| i as u8).collect();
        let m = Message::new(6, MSG_VIDEO, 1, 40, payload.clone());
        let b = encode_chunks(&m, 128);
        assert_eq!(b.len(), 12 + 300 + 2);
        assert_eq!(b[12 + 128], 0xC6);
        assert_eq!(b[12 + 128 + 1 + 128], 0xC6);
        for piece in [1, 7, 1000] {
            assert_eq!(read_all(&b, 128, piece), vec![m.clone()]);
        }
    }

    #[test]
    fn extended_timestamps_and_large_chunk_stream_ids_round_trip() {
        for csid in [2, 63, 64, 319, 320, 1000] {
            let m = Message::new(csid, MSG_AUDIO, 1, 0x0123_4567, vec![5; 200]);
            assert_eq!(read_all(&encode_chunks(&m, 64), 64, 3), vec![m]);
        }
    }

    #[test]
    fn type_1_2_3_headers_use_deltas() {
        // Hand-made: type 0 at ts 100, type 2 with delta 20, type 3 repeating the delta.
        let mut b = vec![0x04, 0, 0, 100, 0, 0, 1, 8, 1, 0, 0, 0, 0xAA];
        b.extend([0x84, 0, 0, 20, 0xBB]);
        b.extend([0xC4, 0xCC]);
        b.extend([0x44, 0, 0, 5, 0, 0, 2, 8, 0xDD, 0xEE]);
        let ms = read_all(&b, 128, 2);
        let ts: Vec<u32> = ms.iter().map(|m| m.timestamp).collect();
        assert_eq!(ts, vec![100, 120, 140, 145]);
        assert_eq!(ms[3].payload, vec![0xDD, 0xEE]);
        assert_eq!(ms[3].stream_id, 1);
    }

    #[test]
    fn set_chunk_size_changes_how_the_rest_is_read() {
        let mut b = encode_chunks(&Message::control(MSG_SET_CHUNK_SIZE, 4096u32.to_be_bytes().to_vec()), 128);
        let big = Message::new(6, MSG_VIDEO, 1, 0, vec![1; 3000]);
        b.extend(encode_chunks(&big, 4096));
        let ms = read_all(&b, 128, 500);
        assert_eq!(ms.len(), 2);
        assert_eq!(ms[1], big);
    }

    #[test]
    fn garbage_is_an_error() {
        let mut r = ChunkReader::default();
        r.feed(&[0x03, 0, 0, 0, 0xFF, 0xFF, 0xFF, 20, 0, 0, 0, 0]);
        assert!(r.next().is_err());
    }

    #[test]
    fn handshake_works_against_the_server_side() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let srv = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            server_handshake(&mut s)
        });
        let mut c = TcpStream::connect(addr).unwrap();
        handshake(&mut c).unwrap();
        srv.join().unwrap().unwrap();
    }

    #[test]
    fn rtmps_to_a_plain_server_fails_cleanly() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let srv = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut b = [0u8; 512];
            let _ = s.read(&mut b);
            let _ = s.write_all(b"HTTP/1.0 400 nope\r\n\r\n");
        });
        let ep = Endpoint { tls: true, host: "127.0.0.1".into(), port, app: "live".into(), tc_url: String::new(), stream: "secret_k".into() };
        let e = Client::connect(&ep, Duration::from_secs(3)).err().unwrap();
        assert!(e.contains("Secure connection"), "{e}");
        assert!(!e.contains("secret_k"));
        srv.join().unwrap();
    }
}
