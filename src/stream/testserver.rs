//! A tiny RTMP server on 127.0.0.1 that accepts one publisher and counts what arrives: for the
//! button test (`--ui-test`) and the unit tests, so going live is tested without the internet.
//! It answers connect, createStream and publish, and keeps nothing but counts.

use super::amf::{self, Amf};
use super::rtmp::{self, ChunkReader, Message};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Seen {
    video: AtomicU64,
    publish_name: Mutex<String>,
    stop: AtomicBool,
}

pub struct TestServer {
    pub port: u16,
    seen: Arc<Seen>,
}

impl TestServer {
    pub fn start() -> Result<TestServer, String> {
        let l = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let port = l.local_addr().map_err(|e| e.to_string())?.port();
        l.set_nonblocking(true).map_err(|e| e.to_string())?;
        let seen = Arc::new(Seen::default());
        let s = seen.clone();
        std::thread::Builder::new()
            .name("snagster-test-rtmp".into())
            .spawn(move || {
                while !s.stop.load(Ordering::Relaxed) {
                    match l.accept() {
                        Ok((c, _)) => {
                            let s = s.clone();
                            std::thread::spawn(move || {
                                let _ = c.set_nonblocking(false);
                                let _ = c.set_read_timeout(Some(Duration::from_millis(200)));
                                let _ = serve(c, &s);
                            });
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(20)),
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(TestServer { port, seen })
    }

    /// The address to stream to (the key is the stream name).
    pub fn url(&self) -> String {
        format!("rtmp://127.0.0.1:{}/live", self.port)
    }

    pub fn video_messages(&self) -> u64 {
        self.seen.video.load(Ordering::Relaxed)
    }

    pub fn publish_name(&self) -> String {
        self.seen.publish_name.lock().map(|n| n.clone()).unwrap_or_default()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.seen.stop.store(true, Ordering::Relaxed);
    }
}

fn send(s: &mut TcpStream, m: &Message) -> Result<(), String> {
    s.write_all(&rtmp::encode_chunks(m, 4096)).map_err(|e| e.to_string())
}

fn status(code: &str) -> Amf {
    Amf::Object(vec![("level".into(), Amf::str("status")), ("code".into(), Amf::str(code)), ("description".into(), Amf::str(""))])
}

fn serve(mut s: TcpStream, seen: &Seen) -> Result<(), String> {
    rtmp::server_handshake(&mut s)?;
    let io = |e: std::io::Error| e.to_string();
    s.write_all(&rtmp::encode_chunks(&Message::control(rtmp::MSG_WINDOW_ACK_SIZE, 2_500_000u32.to_be_bytes().to_vec()), 128)).map_err(io)?;
    s.write_all(&rtmp::encode_chunks(&Message::control(rtmp::MSG_SET_PEER_BANDWIDTH, vec![0, 0x26, 0x25, 0xA0, 2]), 128)).map_err(io)?;
    s.write_all(&rtmp::encode_chunks(&Message::control(rtmp::MSG_SET_CHUNK_SIZE, 4096u32.to_be_bytes().to_vec()), 128)).map_err(|e| e.to_string())?;
    let mut r = ChunkReader::default();
    let mut buf = vec![0u8; 64 * 1024];
    while !seen.stop.load(Ordering::Relaxed) {
        let n = match s.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(e) => return Err(e.to_string()),
        };
        r.feed(&buf[..n]);
        while let Some(m) = r.next()? {
            match m.type_id {
                rtmp::MSG_COMMAND_AMF0 => {
                    let v = amf::decode_all(&m.payload)?;
                    let txn = v.get(1).cloned().unwrap_or(Amf::Number(0.0));
                    match v.first().and_then(Amf::as_str).unwrap_or("") {
                        "connect" => {
                            let ok = Amf::Object(vec![("fmsVer".into(), Amf::str("FMS/3,0,1,123"))]);
                            send(&mut s, &Message::command(3, 0, &[Amf::str("_result"), txn, ok, status("NetConnection.Connect.Success")]))?;
                        }
                        "createStream" => send(&mut s, &Message::command(3, 0, &[Amf::str("_result"), txn, Amf::Null, Amf::Number(1.0)]))?,
                        "publish" => {
                            if let Ok(mut p) = seen.publish_name.lock() {
                                *p = v.get(3).and_then(Amf::as_str).unwrap_or("").to_string();
                            }
                            send(&mut s, &Message::command(5, 1, &[Amf::str("onStatus"), Amf::Number(0.0), Amf::Null, status("NetStream.Publish.Start")]))?;
                        }
                        _ => {}
                    }
                }
                rtmp::MSG_VIDEO => {
                    seen.video.fetch_add(1, Ordering::Relaxed);
                }
                _ => {}
            }
        }
    }
    Ok(())
}
