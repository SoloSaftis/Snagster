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
                st.active.fetch_add(1, Ordering::SeqCst);
                let st2 = st.clone();
                let _ = std::thread::Builder::new().name("snagster-mcp-conn".into()).spawn(move || {
                    let _ = serve(stream, &st2);
                    st2.active.fetch_sub(1, Ordering::SeqCst);
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
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut line = String::new();
    if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_ascii_uppercase();
    let target = parts.next().unwrap_or("/").to_string();
    let mut headers = HashMap::new();
    let mut total = line.len();
    loop {
        let mut h = String::new();
        let n = reader.read_line(&mut h).map_err(|e| e.to_string())?;
        total += n;
        if n == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if total > MAX_HEADERS {
            return Err("headers too large".into());
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return Err("body too large".into());
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).map_err(|e| e.to_string())?;
    let (path, q) = target.split_once('?').unwrap_or((&target, ""));
    let query = q
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), percent_decode(v)))
        .collect();
    Ok(Some(Request { method, path: path.to_string(), query, headers, body }))
}

fn percent_decode(s: &str) -> String {
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
            match session.handle(&text) {
                Some(reply) => {
                    let wants_sse = req.headers.get("accept").is_some_and(|a| a.contains("text/event-stream") && !a.contains("application/json") && !a.contains("*/*"));
                    if wants_sse {
                        extra.push(("Content-Type", "text/event-stream".into()));
                        let body = format!("event: message\ndata: {reply}\n\n");
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
            if let Some(reply) = session.handle(&text) {
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
}
