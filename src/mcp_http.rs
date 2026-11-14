//! MCP over HTTP, for AI apps that connect to a server instead of starting one (Open WebUI,
//! LM Studio, AnythingLLM, Jan, n8n, LibreChat, your own agent...). Two transports:
//!
//! - Streamable HTTP: `POST /mcp` with one JSON-RPC message, answered as JSON; the session id
//!   comes back in `Mcp-Session-Id`. `DELETE /mcp` ends the session.
//! - HTTP + SSE (older clients): `GET /sse` opens an event stream whose first event names the
//!   address to `POST` messages to (`/messages?sessionId=...`); answers arrive on the stream.
//!
//! It listens on 127.0.0.1 only, refuses requests addressed to other host names or coming from
//! web pages on other sites (DNS rebinding), and can require a password (a bearer token).

use crate::mcp::Session;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_BODY: usize = 32 * 1024 * 1024;
const MAX_HEADERS: usize = 64 * 1024;
const MAX_CONNECTIONS: usize = 64;
const SESSION_IDLE: Duration = Duration::from_secs(6 * 3600);

struct Sse {
    tx: Sender<String>,
}

#[derive(Default)]
struct State {
    sessions: Mutex<HashMap<String, (Arc<Session>, Instant)>>,
    sse: Mutex<HashMap<String, Sse>>,
    token: String,
    port: u16,
    stop: AtomicBool,
    active: AtomicUsize,
}

/// One of the `MAX_CONNECTIONS` places, given back when dropped: also when the connection's
/// thread panics or can't be started, so a few broken requests can't use up every place and
/// leave the server answering 503 for ever.
struct Slot(Arc<State>);

impl Slot {
    fn take(st: Arc<State>) -> Slot {
        st.active.fetch_add(1, Ordering::SeqCst);
        Slot(st)
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A running server; stops when dropped.
pub struct Server {
    pub port: u16,
    state: Arc<State>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop.
        let _ = TcpStream::connect_timeout(&([127, 0, 0, 1], self.port).into(), Duration::from_millis(200));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Start serving on 127.0.0.1:`port` (0 picks a free port).
pub fn start(port: u16, token: &str) -> Result<Server, String> {
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("port {port} is taken or not allowed: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let state = Arc::new(State { token: token.trim().to_string(), port, ..Default::default() });
    let st = state.clone();
    let thread = std::thread::Builder::new()
        .name("snagster-mcp-http".into())
        .spawn(move || {
            for conn in listener.incoming() {
                if st.stop.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = conn else { continue };
                if st.active.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                    let mut s = stream;
                    let _ = s.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    continue;
                }
                let st2 = st.clone();
                let slot = Slot::take(st.clone());
                let _ = std::thread::Builder::new().name("snagster-mcp-conn".into()).spawn(move || {
                    let _slot = slot;
                    let _ = serve(stream, &st2);
                });
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Server { port, state, thread: Some(thread) })
}

/// Serve until the process is stopped (`snagster mcp-http`).
pub fn serve_forever(port: u16, token: &str) -> Result<(), String> {
    let server = start(port, token)?;
    eprintln!("Snagster MCP server: {}", server.url());
    eprintln!("Older apps (HTTP + SSE): http://127.0.0.1:{}/sse", server.port);
    if !token.is_empty() {
        eprintln!("Apps must send the header  Authorization: Bearer <your token>");
    }
    eprintln!("Press Ctrl+C to stop.");
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

struct Request {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> Result<Option<Request>, String> {
    stream.set_read_timeout(Some(Duration::from_secs(30))).ok();
    parse_request(BufReader::new(stream.try_clone().map_err(|e| e.to_string())?))
}

/// One line of at most `room` bytes (more is an error: a line that never ends mustn't fill
/// the memory). Empty at the end of the input.
fn read_line_within(reader: &mut impl BufRead, room: usize) -> Result<String, String> {
    let mut raw = Vec::new();
    reader.by_ref().take(room as u64 + 1).read_until(b'\n', &mut raw).map_err(|e| e.to_string())?;
    if raw.len() > room {
        return Err("headers too large".into());
    }
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

/// The body: `len` bytes, kept only as they arrive (a Content-Length of 32 MB in front of a
/// few bytes doesn't reserve 32 MB).
fn read_body(reader: &mut impl Read, len: usize) -> Result<Vec<u8>, String> {
    let mut body = Vec::with_capacity(len.min(64 * 1024));
    reader.take(len as u64).read_to_end(&mut body).map_err(|e| e.to_string())?;
    if body.len() < len {
        return Err("the body ended early".into());
    }
    Ok(body)
}

fn parse_request(mut reader: impl BufRead) -> Result<Option<Request>, String> {
    let line = read_line_within(&mut reader, MAX_HEADERS)?;
    if line.is_empty() {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_ascii_uppercase();
    let target = parts.next().unwrap_or("/").to_string();
    let mut headers = HashMap::new();
    let mut total = line.len();
    loop {
        let h = read_line_within(&mut reader, MAX_HEADERS - total)?;
        let n = h.len();
        total += n;
        if n == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return Err("body too large".into());
    }
    let body = read_body(&mut reader, len)?;
    let (path, q) = target.split_once('?').unwrap_or((&target, ""));
    let query = q
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), percent_decode(v)))
        .collect();
    Ok(Some(Request { method, path: path.to_string(), query, headers, body }))
}

pub(crate) fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Only this computer's own names (blocks DNS rebinding).
fn local_host(h: &str) -> bool {
    let host = if h.starts_with('[') { h.split(']').next().unwrap_or("").trim_start_matches('[') } else { h.split(':').next().unwrap_or("") };
    matches!(host.to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost" | "::1" | "")
}

/// Web pages may call only from this computer's own addresses.
fn origin_ok(o: &str) -> bool {
    if o.is_empty() || o == "null" {
        return true;
    }
    let rest = o.split("://").nth(1).unwrap_or("");
    local_host(rest.split('/').next().unwrap_or(""))
}

fn respond(stream: &mut TcpStream, status: &str, headers: &[(&str, String)], body: &[u8]) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n", body.len());
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// A tool call that asks for progress (`_meta.progressToken`): its answer goes out as an event
/// stream so progress notifications can come first.
fn wants_progress(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .is_some_and(|v| v.get("method").and_then(|m| m.as_str()) == Some("tools/call") && v.pointer("/params/_meta/progressToken").is_some_and(|t| !t.is_null()))
}

/// One server-sent event carrying a JSON-RPC message.
fn sse_event(msg: &str) -> String {
    format!("event: message\ndata: {msg}\n\n")
}

/// Start a response whose body runs until the connection closes (an event stream).
fn respond_open(stream: &mut TcpStream, status: &str, headers: &[(&str, String)]) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {status}\r\nConnection: close\r\nCache-Control: no-store\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.flush()
}

fn cors(req: &Request) -> Vec<(&'static str, String)> {
    let origin = req.headers.get("origin").cloned().unwrap_or_default();
    if origin.is_empty() {
        return Vec::new();
    }
    vec![
        ("Access-Control-Allow-Origin", origin),
        ("Access-Control-Allow-Methods", "GET, POST, DELETE, OPTIONS".into()),
        ("Access-Control-Allow-Headers", "Content-Type, Authorization, Mcp-Session-Id, Mcp-Protocol-Version, Last-Event-ID".into()),
        ("Access-Control-Expose-Headers", "Mcp-Session-Id".into()),
        ("Vary", "Origin".into()),
    ]
}

fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    for _ in 0..2 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

impl State {
    fn session(&self, id: &str) -> Option<Arc<Session>> {
        let mut map = self.sessions.lock().ok()?;
        map.retain(|_, (_, t)| t.elapsed() < SESSION_IDLE);
        let entry = map.get_mut(id)?;
        entry.1 = Instant::now();
        Some(entry.0.clone())
    }

    fn create(&self) -> (String, Arc<Session>) {
        let id = new_id();
        let s = Arc::new(Session::default());
        if let Ok(mut map) = self.sessions.lock() {
            map.insert(id.clone(), (s.clone(), Instant::now()));
        }
        (id, s)
    }
}

fn serve(mut stream: TcpStream, st: &State) -> Result<(), String> {
    let Some(req) = read_request(&mut stream)? else { return Ok(()) };
    let mut extra = cors(&req);
    let host = req.headers.get("host").cloned().unwrap_or_default();
    if !local_host(&host) || !origin_ok(req.headers.get("origin").map(String::as_str).unwrap_or("")) {
        let _ = respond(&mut stream, "403 Forbidden", &extra, b"Only apps on this computer may connect.");
        return Ok(());
    }
    if req.method == "OPTIONS" {
        let _ = respond(&mut stream, "204 No Content", &extra, b"");
        return Ok(());
    }
    if !st.token.is_empty() {
        let bearer = req.headers.get("authorization").and_then(|a| a.strip_prefix("Bearer ").or_else(|| a.strip_prefix("bearer "))).map(str::trim);
        let given = bearer.map(String::from).or_else(|| req.query.get("token").cloned()).unwrap_or_default();
        if given != st.token {
            extra.push(("WWW-Authenticate", "Bearer".into()));
            let _ = respond(&mut stream, "401 Unauthorized", &extra, b"Send the token set in Snagster: Authorization: Bearer <token>");
            return Ok(());
        }
    }
    let json = ("Content-Type", "application/json".to_string());
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") | ("GET", "/health") => {
            let info = serde_json::json!({
                "name": "snagster",
                "version": env!("CARGO_PKG_VERSION"),
                "mcp": format!("http://127.0.0.1:{}/mcp", st.port),
                "sse": format!("http://127.0.0.1:{}/sse", st.port),
                "tools": crate::mcp::tool_names().len(),
            });
            extra.push(json);
            let _ = respond(&mut stream, "200 OK", &extra, info.to_string().as_bytes());
        }
        ("POST", "/mcp") => {
            let text = String::from_utf8_lossy(&req.body).to_string();
            let is_init = text.contains("\"initialize\"");
            let sid = req.headers.get("mcp-session-id").cloned();
            let (sid, session) = match sid {
                Some(id) => match st.session(&id) {
                    Some(s) => (id, s),
                    None if is_init => st.create(),
                    None => {
                        let _ = respond(&mut stream, "404 Not Found", &extra, b"Unknown session; initialize again.");
                        return Ok(());
                    }
                },
                // Clients that don't keep the id get one session per initialize.
                None => st.create(),
            };
            extra.push(("Mcp-Session-Id", sid));
            let accepts_sse = req.headers.get("accept").is_some_and(|a| a.contains("text/event-stream"));
            if accepts_sse && wants_progress(&text) {
                // Progress notifications, then the answer, on one event stream.
                extra.push(("Content-Type", "text/event-stream".into()));
                if respond_open(&mut stream, "200 OK", &extra).is_err() {
                    return Ok(());
                }
                let out = Arc::new(Mutex::new(stream.try_clone().map_err(|e| e.to_string())?));
                let o = out.clone();
                let notify: crate::mcp::Notify = Arc::new(move |msg: String| {
                    if let Ok(mut s) = o.lock() {
                        let _ = s.write_all(sse_event(&msg).as_bytes());
                        let _ = s.flush();
                    }
                });
                if let Some(reply) = session.handle_with(&text, Some(notify))
                    && let Ok(mut s) = out.lock()
                {
                    let _ = s.write_all(sse_event(&reply).as_bytes());
                    let _ = s.flush();
                }
                return Ok(());
            }
            match session.handle(&text) {
                Some(reply) => {
                    let wants_sse = accepts_sse && req.headers.get("accept").is_some_and(|a| !a.contains("application/json") && !a.contains("*/*"));
                    if wants_sse {
                        extra.push(("Content-Type", "text/event-stream".into()));
                        let body = sse_event(&reply);
                        let _ = respond(&mut stream, "200 OK", &extra, body.as_bytes());
                    } else {
                        extra.push(json);
                        let _ = respond(&mut stream, "200 OK", &extra, reply.as_bytes());
                    }
                }
                None => {
                    let _ = respond(&mut stream, "202 Accepted", &extra, b"");
                }
            }
        }
        ("GET", "/mcp") => {
            extra.push(("Allow", "POST, DELETE".into()));
            let _ = respond(&mut stream, "405 Method Not Allowed", &extra, b"");
        }
        ("DELETE", "/mcp") => {
            if let Some(id) = req.headers.get("mcp-session-id")
                && let Ok(mut map) = st.sessions.lock()
            {
                map.remove(id);
            }
            let _ = respond(&mut stream, "200 OK", &extra, b"");
        }
        ("GET", "/sse") => {
            let (id, _session) = st.create();
            let (tx, rx): (Sender<String>, Receiver<String>) = channel();
            if let Ok(mut m) = st.sse.lock() {
                m.insert(id.clone(), Sse { tx });
            }
            let mut head = String::from("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: keep-alive\r\n");
            for (k, v) in &extra {
                head.push_str(&format!("{k}: {v}\r\n"));
            }
            head.push_str("\r\n");
            let ok = stream.write_all(head.as_bytes()).is_ok() && stream.write_all(format!("event: endpoint\ndata: /messages?sessionId={id}\n\n").as_bytes()).is_ok() && stream.flush().is_ok();
            if ok {
                loop {
                    if st.stop.load(Ordering::SeqCst) {
                        break;
                    }
                    match rx.recv_timeout(Duration::from_secs(15)) {
                        Ok(msg) => {
                            if stream.write_all(format!("event: message\ndata: {msg}\n\n").as_bytes()).is_err() || stream.flush().is_err() {
                                break;
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            if stream.write_all(b": keep-alive\n\n").is_err() || stream.flush().is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            if let Ok(mut m) = st.sse.lock() {
                m.remove(&id);
            }
            if let Ok(mut map) = st.sessions.lock() {
                map.remove(&id);
            }
        }
        ("POST", "/messages") | ("POST", "/message") => {
            let id = req.query.get("sessionId").or_else(|| req.query.get("session_id")).cloned().unwrap_or_default();
            let (Some(session), Some(tx)) = (st.session(&id), st.sse.lock().ok().and_then(|m| m.get(&id).map(|s| s.tx.clone()))) else {
                let _ = respond(&mut stream, "404 Not Found", &extra, b"Unknown session; open /sse first.");
                return Ok(());
            };
            // Answer at once; the reply travels on the event stream when the tool is done.
            let _ = respond(&mut stream, "202 Accepted", &extra, b"Accepted");
            let text = String::from_utf8_lossy(&req.body).to_string();
            let progress = tx.clone();
            let notify: crate::mcp::Notify = Arc::new(move |msg: String| {
                let _ = progress.send(msg);
            });
            if let Some(reply) = session.handle_with(&text, Some(notify)) {
                let _ = tx.send(reply);
            }
        }
        _ => {
            let _ = respond(&mut stream, "404 Not Found", &extra, b"Snagster's MCP server: POST /mcp (or GET /sse).");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_and_origins() {
        assert!(local_host("127.0.0.1:45831"));
        assert!(local_host("localhost"));
        assert!(local_host("[::1]:80"));
        assert!(!local_host("evil.example:45831"));
        assert!(origin_ok(""));
        assert!(origin_ok("http://localhost:3000"));
        assert!(!origin_ok("https://evil.example"));
        assert_eq!(percent_decode("a%20b+c"), "a b c");
    }

    // ---- pure helpers ----

    #[test]
    fn percent_decoding_handles_odd_input() {
        assert_eq!(percent_decode(""), "");
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode("%41%62"), "Ab");
        assert_eq!(percent_decode("caf%C3%A9"), "café", "UTF-8 bytes join up");
        assert_eq!(percent_decode("100%"), "100%", "a lone % at the end stays");
        assert_eq!(percent_decode("a%2"), "a%2", "too short to be an escape");
        assert_eq!(percent_decode("%zz"), "%zz", "not hex");
        assert_eq!(percent_decode("%2B"), "+", "an escaped plus is a plus, not a space");
        assert_eq!(percent_decode("%FF"), "\u{FFFD}", "bad UTF-8 is replaced, not a panic");
        assert_eq!(percent_decode("%e2%82%ac"), "€", "lower-case hex");
    }

    #[test]
    fn only_this_computers_names_are_local() {
        for ok in ["127.0.0.1", "127.0.0.1:1", "LOCALHOST:45831", "localhost", "[::1]", "[::1]:45831", ""] {
            assert!(local_host(ok), "{ok}");
        }
        for bad in ["127.0.0.2", "0.0.0.0", "localhost.evil.example", "evil.localhost", "[::2]:80", "192.168.1.5:45831", "127.0.0.1.nip.io"] {
            assert!(!local_host(bad), "{bad}");
        }
    }

    #[test]
    fn only_local_web_pages_may_call() {
        for ok in ["null", "http://127.0.0.1:5173", "https://localhost", "http://[::1]:3000/app", "http://localhost:8080/some/page"] {
            assert!(origin_ok(ok), "{ok}");
        }
        for bad in ["http://evil.example", "https://localhost.evil.example", "http://127.0.0.1@evil.example", "http://10.0.0.1:45831"] {
            assert!(!origin_ok(bad), "{bad}");
        }
    }

    fn request(headers: &[(&str, &str)]) -> Request {
        Request {
            method: "GET".into(),
            path: "/".into(),
            query: HashMap::new(),
            headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            body: Vec::new(),
        }
    }

    #[test]
    fn cors_headers_only_for_web_pages() {
        assert!(cors(&request(&[])).is_empty());
        let h = cors(&request(&[("origin", "http://localhost:3000")]));
        let get = |k: &str| h.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
        assert_eq!(get("Access-Control-Allow-Origin").as_deref(), Some("http://localhost:3000"));
        assert!(get("Access-Control-Allow-Headers").unwrap().contains("Mcp-Session-Id"));
        assert!(get("Access-Control-Allow-Methods").unwrap().contains("DELETE"));
        assert_eq!(get("Access-Control-Expose-Headers").as_deref(), Some("Mcp-Session-Id"));
        assert_eq!(get("Vary").as_deref(), Some("Origin"));
    }

    #[test]
    fn session_ids_are_long_hex_and_differ() {
        let ids: Vec<String> = (0..20).map(|_| new_id()).collect();
        for id in &ids {
            assert_eq!(id.len(), 32);
            assert!(id.chars().all(|c| c.is_ascii_hexdigit()), "{id}");
        }
        let mut uniq = ids.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), ids.len());
    }

    /// Bug: a connection's place was given back only when its thread ended normally; a panic
    /// (or a thread that couldn't start) kept it, and after 64 the server refused everyone.
    #[test]
    fn a_connection_gives_its_place_back_even_when_it_panics() {
        let st = Arc::new(State::default());
        let slot = Slot::take(st.clone());
        assert_eq!(st.active.load(Ordering::SeqCst), 1);
        let t = std::thread::spawn(move || {
            let _slot = slot;
            panic!("a broken request (expected in this test)");
        });
        assert!(t.join().is_err());
        assert_eq!(st.active.load(Ordering::SeqCst), 0);
        drop(Slot::take(st.clone()));
        assert_eq!(st.active.load(Ordering::SeqCst), 0, "a place whose thread never ran comes back too");
    }

    #[test]
    fn sessions_are_kept_found_and_forgotten() {
        let st = State::default();
        assert!(st.session("nope").is_none());
        let (id, s) = st.create();
        assert!(Arc::ptr_eq(&st.session(&id).unwrap(), &s));
        st.sessions.lock().unwrap().remove(&id);
        assert!(st.session(&id).is_none());
    }

    // ---- reading requests over a loopback socket (127.0.0.1, no outside network) ----

    /// Send `raw` from one end of a fresh 127.0.0.1 connection and parse it at the other.
    fn parse_raw(raw: &[u8]) -> Result<Option<Request>, String> {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let raw = raw.to_vec();
        let client = std::thread::spawn(move || {
            let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let _ = c.write_all(&raw);
            let _ = c.shutdown(std::net::Shutdown::Write);
            // Keep the socket until the server side has read.
            std::thread::sleep(Duration::from_millis(200));
        });
        let (mut s, _) = l.accept().unwrap();
        let r = read_request(&mut s);
        // Close first: a client still writing (oversized headers) then stops instead of waiting.
        drop(s);
        let _ = client.join();
        r
    }

    #[test]
    fn a_request_is_split_into_method_path_query_headers_and_body() {
        let raw = b"post /messages?sessionId=ab%20c&x=1&flag HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nX-Spaced :  v a l  \r\nContent-Length: 5\r\n\r\nhelloEXTRA";
        let r = parse_raw(raw).unwrap().unwrap();
        assert_eq!(r.method, "POST", "upper-cased");
        assert_eq!(r.path, "/messages");
        assert_eq!(r.query.get("sessionId").map(String::as_str), Some("ab c"));
        assert_eq!(r.query.get("x").map(String::as_str), Some("1"));
        assert!(!r.query.contains_key("flag"), "a key without = is skipped");
        assert_eq!(r.headers.get("content-type").map(String::as_str), Some("application/json"), "names are lower-cased");
        assert_eq!(r.headers.get("x-spaced").map(String::as_str), Some("v a l"));
        assert_eq!(r.body, b"hello", "only Content-Length bytes are read");
    }

    #[test]
    fn requests_with_bare_newlines_and_no_body_parse() {
        let r = parse_raw(b"GET /health HTTP/1.1\nHost: localhost\n\n").unwrap().unwrap();
        assert_eq!((r.method.as_str(), r.path.as_str()), ("GET", "/health"));
        assert!(r.body.is_empty() && r.query.is_empty());
        assert_eq!(r.headers.get("host").map(String::as_str), Some("localhost"));
    }

    #[test]
    fn empty_oversized_and_short_requests() {
        assert!(parse_raw(b"").unwrap().is_none(), "a closed connection is no request");
        let big = format!("POST /mcp HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert_eq!(parse_raw(big.as_bytes()).err().as_deref(), Some("body too large"));
        let mut huge = b"GET / HTTP/1.1\r\n".to_vec();
        for i in 0..2000 {
            huge.extend_from_slice(format!("X-Filler-{i}: {}\r\n", "y".repeat(40)).as_bytes());
        }
        huge.extend_from_slice(b"\r\n");
        assert_eq!(parse_raw(&huge).err().as_deref(), Some("headers too large"));
        assert!(parse_raw(b"POST /mcp HTTP/1.1\r\nContent-Length: 50\r\n\r\nshort").is_err(), "the body ended early");
        // A junk Content-Length reads as no body.
        let r = parse_raw(b"POST /mcp HTTP/1.1\r\nContent-Length: lots\r\n\r\n").unwrap().unwrap();
        assert!(r.body.is_empty());
    }

    // ---- broken and hostile requests (in memory, no sockets) ----

    fn parse_bytes(raw: &[u8]) -> Result<Option<Request>, String> {
        parse_request(std::io::Cursor::new(raw))
    }

    #[test]
    fn random_and_broken_requests_never_panic_or_reserve_much() {
        let good = format!("POST /messages?sessionId=a%20b HTTP/1.1\r\nHost: 127.0.0.1:45831\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{PING}", PING.len());
        assert_eq!(parse_bytes(good.as_bytes()).unwrap().unwrap().body, PING.as_bytes());
        let mut g = crate::testalloc::Bytes::new(3);
        for i in 0..5_000 {
            let input = match i % 3 {
                0 => g.mutate(good.as_bytes()),
                1 => g.noise(300),
                // Big numbers where the length goes.
                _ => format!("POST /mcp HTTP/1.1\r\nContent-Length: {}\r\n\r\nxyz", g.next() % (MAX_BODY as u64 * 2)).into_bytes(),
            };
            let (r, largest, _) = crate::testalloc::measure(|| parse_bytes(&input));
            if let Ok(Some(r)) = r {
                assert!(r.body.len() <= MAX_BODY && r.headers.len() <= input.len());
            }
            assert!(largest <= 256 * 1024 + 4 * input.len(), "input {i} ({} bytes) reserved {largest} bytes", input.len());
        }
    }

    /// Bug: the body's buffer was made as big as Content-Length said before anything was read,
    /// so 64 connections each claiming 32 MB took 2 GB without sending it.
    #[test]
    fn a_claimed_body_size_isnt_reserved_before_it_arrives() {
        let raw = format!("POST /mcp HTTP/1.1\r\nContent-Length: {MAX_BODY}\r\n\r\nshort");
        let (r, largest, _) = crate::testalloc::measure(|| parse_bytes(raw.as_bytes()));
        assert_eq!(r.err().as_deref(), Some("the body ended early"));
        assert!(largest < 1 << 20, "reserved {largest} bytes for 5");
        // A body that is all there reads in full.
        let body = "x".repeat(300_000);
        let r = parse_bytes(format!("POST /mcp HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).unwrap().unwrap();
        assert_eq!(r.body.len(), 300_000);
    }

    /// Bug: the request line had no length limit (only header lines were counted), so a
    /// client sending one endless line filled the memory.
    #[test]
    fn an_endless_request_line_is_refused() {
        let mut raw = b"GET /".to_vec();
        raw.extend(std::iter::repeat_n(b'a', 1 << 20));
        assert_eq!(parse_raw(&raw).err().as_deref(), Some("headers too large"));
        let (r, largest, _) = crate::testalloc::measure(|| parse_bytes(&raw));
        assert!(r.is_err());
        assert!(largest < 1 << 20, "{largest}");
        // One endless header line likewise.
        let mut raw = b"GET / HTTP/1.1\r\nX-Long: ".to_vec();
        raw.extend(std::iter::repeat_n(b'b', 1 << 20));
        let (r, largest, _) = crate::testalloc::measure(|| parse_bytes(&raw));
        assert_eq!(r.err().as_deref(), Some("headers too large"));
        assert!(largest < 1 << 20, "{largest}");
        // Headers that just fit are fine; bytes that aren't UTF-8 are no error either.
        let fits = format!("GET / HTTP/1.1\r\nX-A: {}\r\nX-B: caf\u{e9}\r\n\r\n", "y".repeat(MAX_HEADERS - 100));
        assert!(parse_bytes(fits.as_bytes()).unwrap().is_some());
        assert!(parse_bytes(b"GET /\xff\xfe HTTP/1.1\r\n\r\n").unwrap().is_some());
    }

    // ---- the whole server on 127.0.0.1 with a free port ----

    struct Reply {
        status: u16,
        headers: HashMap<String, String>,
        body: String,
    }

    impl Reply {
        fn header(&self, k: &str) -> Option<&str> {
            self.headers.get(&k.to_ascii_lowercase()).map(String::as_str)
        }
    }

    fn split_reply(raw: &str) -> Reply {
        let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
        let mut lines = head.lines();
        let status = lines.next().and_then(|l| l.split_whitespace().nth(1)).and_then(|s| s.parse().ok()).unwrap_or(0);
        let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
        Reply { status, headers, body: body.to_string() }
    }

    /// One HTTP exchange: `head` lines (without Content-Length) and a body.
    fn http(port: u16, method: &str, target: &str, headers: &[(&str, &str)], body: &str) -> Reply {
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let mut req = format!("{method} {target} HTTP/1.1\r\n");
        if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
            req.push_str(&format!("Host: 127.0.0.1:{port}\r\n"));
        }
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        c.write_all(req.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = c.read_to_string(&mut out);
        split_reply(&out)
    }

    const INIT: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#;
    const PING: &str = r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#;

    #[test]
    fn health_and_unknown_paths() {
        let srv = start(0, "").unwrap();
        assert!(srv.port > 0);
        assert_eq!(srv.url(), format!("http://127.0.0.1:{}/mcp", srv.port));
        let r = http(srv.port, "GET", "/health", &[], "");
        assert_eq!(r.status, 200);
        assert_eq!(r.header("content-type"), Some("application/json"));
        assert_eq!(r.header("cache-control"), Some("no-store"));
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["name"], "snagster");
        assert_eq!(v["tools"], crate::mcp::tool_names().len());
        assert_eq!(v["mcp"], srv.url());
        assert_eq!(http(srv.port, "GET", "/", &[], "").status, 200);
        assert_eq!(http(srv.port, "GET", "/elsewhere", &[], "").status, 404);
        let r = http(srv.port, "GET", "/mcp", &[], "");
        assert_eq!(r.status, 405);
        assert_eq!(r.header("allow"), Some("POST, DELETE"));
    }

    #[test]
    fn other_hosts_and_sites_are_refused() {
        let srv = start(0, "").unwrap();
        assert_eq!(http(srv.port, "GET", "/health", &[("Host", "evil.example")], "").status, 403);
        assert_eq!(http(srv.port, "POST", "/mcp", &[("Origin", "https://evil.example")], INIT).status, 403);
        let r = http(srv.port, "OPTIONS", "/mcp", &[("Origin", "http://localhost:3000")], "");
        assert_eq!(r.status, 204);
        assert_eq!(r.header("access-control-allow-origin"), Some("http://localhost:3000"));
    }

    #[test]
    fn a_token_is_required_when_set() {
        let srv = start(0, "  s3cret ").unwrap();
        let r = http(srv.port, "GET", "/health", &[], "");
        assert_eq!(r.status, 401);
        assert_eq!(r.header("www-authenticate"), Some("Bearer"));
        assert_eq!(http(srv.port, "GET", "/health", &[("Authorization", "Bearer wrong")], "").status, 401);
        assert_eq!(http(srv.port, "GET", "/health", &[("Authorization", "Bearer s3cret")], "").status, 200, "the token is trimmed");
        assert_eq!(http(srv.port, "GET", "/health", &[("Authorization", "bearer  s3cret ")], "").status, 200);
        assert_eq!(http(srv.port, "GET", "/health?token=s3cret", &[], "").status, 200);
        assert_eq!(http(srv.port, "GET", "/health", &[("Authorization", "Basic s3cret")], "").status, 401);
        // Preflight requests carry no token and still get through.
        assert_eq!(http(srv.port, "OPTIONS", "/mcp", &[("Origin", "http://localhost")], "").status, 204);
    }

    #[test]
    fn streamable_http_keeps_a_session() {
        let srv = start(0, "").unwrap();
        let p = srv.port;
        let r = http(p, "POST", "/mcp", &[("Content-Type", "application/json"), ("Accept", "application/json, text/event-stream")], INIT);
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(r.header("content-type"), Some("application/json"));
        let sid = r.header("mcp-session-id").expect("a session id").to_string();
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["result"]["protocolVersion"], "2025-06-18");

        let r = http(p, "POST", "/mcp", &[("Mcp-Session-Id", &sid)], PING);
        assert_eq!(r.status, 200);
        assert_eq!(r.header("mcp-session-id"), Some(sid.as_str()));
        // A notification gets 202 and no body.
        let r = http(p, "POST", "/mcp", &[("Mcp-Session-Id", &sid)], r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        assert_eq!(r.status, 202);
        assert!(r.body.is_empty());
        // Clients asking only for an event stream get the answer framed as one event.
        let r = http(p, "POST", "/mcp", &[("Mcp-Session-Id", &sid), ("Accept", "text/event-stream")], PING);
        assert_eq!(r.header("content-type"), Some("text/event-stream"));
        assert!(r.body.starts_with("event: message\ndata: {") && r.body.ends_with("}\n\n"), "{:?}", r.body);

        // Unknown sessions must initialize again; an initialize with a stale id starts afresh.
        assert_eq!(http(p, "POST", "/mcp", &[("Mcp-Session-Id", "feedface")], PING).status, 404);
        let r = http(p, "POST", "/mcp", &[("Mcp-Session-Id", "feedface")], INIT);
        assert_eq!(r.status, 200);
        assert_ne!(r.header("mcp-session-id"), Some("feedface"));

        // DELETE ends the session.
        assert_eq!(http(p, "DELETE", "/mcp", &[("Mcp-Session-Id", &sid)], "").status, 200);
        assert_eq!(http(p, "POST", "/mcp", &[("Mcp-Session-Id", &sid)], PING).status, 404);
    }

    #[test]
    fn sessions_keep_their_own_pictures() {
        let srv = start(0, "").unwrap();
        let p = srv.port;
        let a = http(p, "POST", "/mcp", &[], INIT).header("mcp-session-id").unwrap().to_string();
        let b = http(p, "POST", "/mcp", &[], INIT).header("mcp-session-id").unwrap().to_string();
        assert_ne!(a, b);
        let list = r#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#;
        let count = |sid: &str| {
            let r = http(p, "POST", "/mcp", &[("Mcp-Session-Id", sid)], list);
            let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
            v["result"]["resources"].as_array().unwrap().len()
        };
        let dir = std::env::temp_dir().join(format!("snagster-http-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("p.png");
        image::RgbaImage::new(8, 8).save(&img).unwrap();
        let open = serde_json::json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "open_image", "arguments": {"path": img}}}).to_string();
        let r = http(p, "POST", "/mcp", &[("Mcp-Session-Id", &a)], &open);
        assert!(r.body.contains("\"isError\":false"), "{}", r.body);
        assert_eq!(count(&a), 3, "a has a current picture");
        assert_eq!(count(&b), 2, "b does not");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_tool_calls_with_a_progress_token_stream() {
        assert!(wants_progress(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"x","_meta":{"progressToken":"t"}}}"#));
        assert!(wants_progress(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"_meta":{"progressToken":0}}}"#));
        assert!(!wants_progress(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"_meta":{"progressToken":null}}}"#));
        assert!(!wants_progress(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"x"}}"#));
        assert!(!wants_progress(r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{"_meta":{"progressToken":"t"}}}"#));
        assert!(!wants_progress("not json"));
        assert_eq!(sse_event("{}"), "event: message\ndata: {}\n\n");
    }

    /// A small guide to make: two steps from a picture file, saved as a web page.
    fn guide_call(dir: &std::path::Path, id: u64, token: Option<&str>) -> String {
        std::fs::create_dir_all(dir).unwrap();
        let pic = dir.join("p.png");
        image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255])).save(&pic).unwrap();
        let mut params = serde_json::json!({"name": "make_guide", "arguments": {"path": dir.join(format!("g{id}.html")), "steps": [{"path": pic}, {"path": pic}]}});
        if let Some(t) = token {
            params["_meta"] = serde_json::json!({ "progressToken": t });
        }
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": params}).to_string()
    }

    /// The data lines of an event stream, as JSON.
    fn events(body: &str) -> Vec<serde_json::Value> {
        body.lines().filter_map(|l| l.strip_prefix("data: ")).map(|d| serde_json::from_str(d).unwrap()).collect()
    }

    #[test]
    fn streamable_http_sends_progress_before_the_answer() {
        let srv = start(0, "").unwrap();
        let p = srv.port;
        let dir = std::env::temp_dir().join(format!("snagster-http-progress-{}", std::process::id()));
        let both = ("Accept", "application/json, text/event-stream");
        let sid = http(p, "POST", "/mcp", &[], INIT).header("mcp-session-id").unwrap().to_string();
        let r = http(p, "POST", "/mcp", &[("Mcp-Session-Id", &sid), both], &guide_call(&dir, 5, Some("tok")));
        assert_eq!(r.status, 200);
        assert_eq!(r.header("content-type"), Some("text/event-stream"));
        assert!(r.header("content-length").is_none(), "the stream runs until the answer");
        let ev = events(&r.body);
        assert!(ev.len() >= 3, "{:?}", r.body);
        let (last, notes) = ev.split_last().unwrap();
        assert_eq!(last["id"], 5);
        assert_eq!(last["result"]["isError"], false, "{last}");
        assert!(notes.iter().all(|n| n["method"] == "notifications/progress" && n["params"]["progressToken"] == "tok"));
        // Without a token (or without accepting a stream) it's plain JSON as before.
        let r = http(p, "POST", "/mcp", &[("Mcp-Session-Id", &sid), both], &guide_call(&dir, 6, None));
        assert_eq!(r.header("content-type"), Some("application/json"));
        let r = http(p, "POST", "/mcp", &[("Mcp-Session-Id", &sid), ("Accept", "application/json")], &guide_call(&dir, 7, Some("tok")));
        assert_eq!(r.header("content-type"), Some("application/json"));
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["id"], 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_older_sse_transport_answers_on_the_stream() {
        let srv = start(0, "").unwrap();
        let p = srv.port;
        let mut sse = TcpStream::connect(("127.0.0.1", p)).unwrap();
        sse.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        sse.write_all(format!("GET /sse HTTP/1.1\r\nHost: 127.0.0.1:{p}\r\nAccept: text/event-stream\r\n\r\n").as_bytes()).unwrap();
        let mut reader = BufReader::new(sse.try_clone().unwrap());
        // Read lines until the endpoint event's data line.
        let mut status = String::new();
        reader.read_line(&mut status).unwrap();
        assert!(status.starts_with("HTTP/1.1 200"), "{status}");
        let mut saw_type = false;
        // The data line of the next event, noting the stream's content type on the way.
        let mut next_data = |saw_type: &mut bool| loop {
            let mut l = String::new();
            assert!(reader.read_line(&mut l).unwrap() > 0, "the stream ended early");
            if l.to_ascii_lowercase().starts_with("content-type: text/event-stream") {
                *saw_type = true;
            }
            if let Some(d) = l.strip_prefix("data: ") {
                break d.trim().to_string();
            }
        };
        let endpoint = next_data(&mut saw_type);
        assert!(saw_type);
        assert!(endpoint.starts_with("/messages?sessionId="), "{endpoint}");

        let r = http(p, "POST", &endpoint, &[], PING);
        assert_eq!(r.status, 202);
        // The answer arrives as a message event.
        let event = next_data(&mut saw_type);
        let v: serde_json::Value = serde_json::from_str(&event).unwrap();
        assert_eq!(v["id"], 2);
        assert_eq!(v["result"], serde_json::json!({}));

        // Progress travels on the stream too, before the answer.
        let dir = std::env::temp_dir().join(format!("snagster-sse-progress-{}", std::process::id()));
        assert_eq!(http(p, "POST", &endpoint, &[], &guide_call(&dir, 3, Some("sse-tok"))).status, 202);
        let mut seen = Vec::new();
        loop {
            let v: serde_json::Value = serde_json::from_str(&next_data(&mut saw_type)).unwrap();
            let done = v["id"] == 3;
            seen.push(v);
            if done {
                break;
            }
        }
        assert!(seen.len() >= 3, "{seen:?}");
        assert!(seen[..seen.len() - 1].iter().all(|n| n["method"] == "notifications/progress" && n["params"]["progressToken"] == "sse-tok"));
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(http(p, "POST", "/messages?sessionId=nope", &[], PING).status, 404);
        let id = endpoint.split('=').nth(1).unwrap().to_string();
        assert_eq!(http(p, "POST", &format!("/message?session_id={id}"), &[], PING).status, 202, "the other spelling works too");
        drop(reader);
        drop(sse);
    }
}
