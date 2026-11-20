//! The streaming engine against a small RTMP server made for these tests, on 127.0.0.1 only,
//! with fake encoders (no screen, sound card, graphics chip or internet needed).

use super::amf::{self, Amf};
use super::engine::*;
use super::rtmp::{self, ChunkReader, Message};
use super::services::{Service, StreamTarget};
use crate::encode::{AudioEncoder, AudioPacket, Codec, EncoderName, VideoEncoder, VideoPacket};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---- the test server ----

#[derive(Default, Debug)]
struct Conn {
    app: String,
    tc_url: String,
    publish_name: String,
    metadata: Vec<Amf>,
    video: Vec<(u32, Vec<u8>)>,
    audio: Vec<(u32, Vec<u8>)>,
    commands: Vec<String>,
    ping_answered: bool,
    acks: u32,
}

#[derive(Clone, Default)]
struct ServerOpts {
    /// Refuse other stream names (with the name in the error, to check it gets masked).
    expected_key: Option<String>,
    /// Close the first connection after this many audio/video messages.
    drop_first_after_media: Option<usize>,
    /// Close every connection at its first audio/video message (takes the stream, then drops it).
    drop_all_at_media: bool,
    /// Stop reading after publishing starts (a network that can't keep up).
    stall: bool,
}

struct Server {
    port: u16,
    conns: Arc<Mutex<Vec<Conn>>>,
}

impl Server {
    fn start(opts: ServerOpts) -> Server {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let conns: Arc<Mutex<Vec<Conn>>> = Arc::default();
        let cs = conns.clone();
        std::thread::spawn(move || {
            for (i, s) in l.incoming().enumerate() {
                let Ok(s) = s else { break };
                let idx = {
                    let mut g = cs.lock().unwrap();
                    g.push(Conn::default());
                    g.len() - 1
                };
                let (cs, opts) = (cs.clone(), opts.clone());
                std::thread::spawn(move || {
                    let _ = serve(s, idx, i == 0, &opts, &cs);
                });
            }
        });
        Server { port, conns }
    }

    fn url(&self) -> String {
        format!("rtmp://127.0.0.1:{}/live", self.port)
    }

    fn wait(&self, what: &str, secs: u64, f: impl Fn(&[Conn]) -> bool) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(secs) {
            if f(&self.conns.lock().unwrap()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {what}: {:?}", self.conns.lock().unwrap());
    }
}

fn send(s: &mut TcpStream, m: &Message, chunk: u32) -> std::io::Result<()> {
    s.write_all(&rtmp::encode_chunks(m, chunk))
}

fn status(level: &str, code: &str, desc: &str) -> Amf {
    Amf::Object(vec![("level".into(), Amf::str(level)), ("code".into(), Amf::str(code)), ("description".into(), Amf::str(desc))])
}

fn serve(mut s: TcpStream, idx: usize, first: bool, opts: &ServerOpts, conns: &Mutex<Vec<Conn>>) -> Result<(), String> {
    let io = |e: std::io::Error| e.to_string();
    rtmp::server_handshake(&mut s)?;
    send(&mut s, &Message::control(rtmp::MSG_WINDOW_ACK_SIZE, 100u32.to_be_bytes().to_vec()), 128).map_err(io)?;
    send(&mut s, &Message::control(rtmp::MSG_SET_PEER_BANDWIDTH, vec![0, 0x26, 0x25, 0xA0, 2]), 128).map_err(io)?;
    send(&mut s, &Message::control(rtmp::MSG_SET_CHUNK_SIZE, 4096u32.to_be_bytes().to_vec()), 128).map_err(io)?;
    let mut r = ChunkReader::default();
    let mut buf = vec![0u8; 64 * 1024];
    let mut media = 0usize;
    loop {
        let n = s.read(&mut buf).map_err(io)?;
        if n == 0 {
            return Ok(());
        }
        r.feed(&buf[..n]);
        while let Some(m) = r.next()? {
            let mut g = conns.lock().unwrap();
            let c = &mut g[idx];
            match m.type_id {
                rtmp::MSG_COMMAND_AMF0 => {
                    let v = amf::decode_all(&m.payload)?;
                    let name = v[0].as_str().unwrap_or("").to_string();
                    let txn = v.get(1).cloned().unwrap_or(Amf::Number(0.0));
                    c.commands.push(name.clone());
                    match name.as_str() {
                        "connect" => {
                            c.app = v[2].get("app").and_then(Amf::as_str).unwrap_or("").into();
                            c.tc_url = v[2].get("tcUrl").and_then(Amf::as_str).unwrap_or("").into();
                            drop(g);
                            let ok = Amf::Object(vec![("fmsVer".into(), Amf::str("FMS/3,0,1,123"))]);
                            send(&mut s, &Message::command(3, 0, &[Amf::str("_result"), txn, ok, status("status", "NetConnection.Connect.Success", "")]), 4096).map_err(io)?;
                        }
                        "createStream" => {
                            drop(g);
                            send(&mut s, &Message::command(3, 0, &[Amf::str("_result"), txn, Amf::Null, Amf::Number(1.0)]), 4096).map_err(io)?;
                        }
                        "publish" => {
                            let key = v.get(3).and_then(Amf::as_str).unwrap_or("").to_string();
                            c.publish_name = key.clone();
                            drop(g);
                            if opts.expected_key.as_ref().is_some_and(|k| *k != key) {
                                let st = status("error", "NetStream.Publish.BadName", &format!("no stream called {key}"));
                                send(&mut s, &Message::command(5, 1, &[Amf::str("onStatus"), Amf::Number(0.0), Amf::Null, st]), 4096).map_err(io)?;
                                continue;
                            }
                            let st = status("status", "NetStream.Publish.Start", "publishing");
                            send(&mut s, &Message::command(5, 1, &[Amf::str("onStatus"), Amf::Number(0.0), Amf::Null, st]), 4096).map_err(io)?;
                            // A ping the client must answer.
                            send(&mut s, &Message::control(rtmp::MSG_USER_CONTROL, vec![0, 6, 0, 0, 4, 0xD2]), 4096).map_err(io)?;
                            if opts.stall {
                                std::thread::sleep(Duration::from_secs(30));
                                return Ok(());
                            }
                        }
                        _ => {}
                    }
                }
                rtmp::MSG_DATA_AMF0 => c.metadata = amf::decode_all(&m.payload)?,
                rtmp::MSG_VIDEO | rtmp::MSG_AUDIO => {
                    if m.type_id == rtmp::MSG_VIDEO {
                        c.video.push((m.timestamp, m.payload));
                    } else {
                        c.audio.push((m.timestamp, m.payload));
                    }
                    media += 1;
                    if opts.drop_all_at_media || (first && opts.drop_first_after_media.is_some_and(|n| media >= n)) {
                        let _ = s.shutdown(std::net::Shutdown::Both);
                        return Ok(());
                    }
                }
                rtmp::MSG_USER_CONTROL if m.payload.get(..6) == Some(&[0, 7, 0, 0, 4, 0xD2]) => c.ping_answered = true,
                rtmp::MSG_ACK => c.acks += 1,
                _ => {}
            }
        }
    }
}

// ---- fake encoders ----

const SPS: &[u8] = &[0x67, 0x42, 0xC0, 0x1F, 0xDA];
const PPS: &[u8] = &[0x68, 0xCE, 0x3C, 0x80];

struct FakeVideo {
    n: u64,
    gop: u64,
    pad: usize,
}

impl VideoEncoder for FakeVideo {
    fn encode(&mut self, rgba: &[u8], pts_us: i64, force_keyframe: bool) -> Result<Vec<VideoPacket>, String> {
        assert_eq!(rgba.len(), 64 * 64 * 4);
        let key = force_keyframe || self.n % self.gop == 0;
        self.n += 1;
        let mut d = Vec::new();
        if key {
            for n in [SPS, PPS] {
                d.extend([0, 0, 0, 1]);
                d.extend(n);
            }
            d.extend([0, 0, 0, 1, 0x65]);
        } else {
            d.extend([0, 0, 0, 1, 0x41]);
        }
        d.extend((self.n as u32).to_be_bytes());
        d.resize(d.len() + self.pad, 0x55);
        Ok(vec![VideoPacket { data: d, pts_us, dts_us: pts_us, keyframe: key }])
    }
    fn finish(&mut self) -> Result<Vec<VideoPacket>, String> {
        Ok(Vec::new())
    }
    fn name(&self) -> EncoderName {
        EncoderName { name: "fake video".into(), hardware: false }
    }
}

#[derive(Default)]
struct FakeAudio {
    frames: u64,
    made: u64,
}

impl AudioEncoder for FakeAudio {
    fn encode(&mut self, pcm: &[i16], _pts_us: i64) -> Result<Vec<AudioPacket>, String> {
        self.frames += pcm.len() as u64 / 2;
        let mut out = Vec::new();
        while (self.made + 1) * 1024 <= self.frames {
            out.push(AudioPacket { data: vec![0x21, 0x10, self.made as u8], pts_us: (self.made * 1024 * 1_000_000 / 48_000) as i64 });
            self.made += 1;
        }
        Ok(out)
    }
    fn finish(&mut self) -> Result<Vec<AudioPacket>, String> {
        Ok(Vec::new())
    }
    fn audio_specific_config(&self) -> Vec<u8> {
        vec![0x11, 0x90]
    }
    fn name(&self) -> EncoderName {
        EncoderName { name: "fake aac".into(), hardware: false }
    }
}

pub(crate) fn fakes(gop: u64, pad: usize) -> Encoders {
    Encoders {
        zero_copy: false,
        video: Arc::new(move |c| {
            assert!(c.low_latency);
            Ok(Box::new(FakeVideo { n: 0, gop, pad }) as Box<dyn VideoEncoder>)
        }),
        audio: Arc::new(|_| Ok(Box::new(FakeAudio::default()) as Box<dyn AudioEncoder>)),
        fallback: None,
        #[cfg(zero_copy)]
        pick: String::new(),
        note: Arc::default(),
    }
}

/// A fake graphics-chip encoder that stops working after `after` pictures (a driver reset).
struct Breaks {
    inner: FakeVideo,
    after: u64,
    name: String,
}

impl VideoEncoder for Breaks {
    fn encode(&mut self, rgba: &[u8], pts_us: i64, key: bool) -> Result<Vec<VideoPacket>, String> {
        if self.inner.n >= self.after {
            return Err("the graphics chip was reset".into());
        }
        self.inner.encode(rgba, pts_us, key)
    }
    fn finish(&mut self) -> Result<Vec<VideoPacket>, String> {
        Ok(Vec::new())
    }
    fn name(&self) -> EncoderName {
        EncoderName { name: self.name.clone(), hardware: true }
    }
    /// It threw a few pictures away before it broke (the count must outlive it).
    fn dropped(&self) -> u64 {
        7
    }
}

fn breaking(after: u64, name: &str) -> Box<dyn VideoEncoder> {
    Box::new(Breaks { inner: FakeVideo { n: 0, gop: 30, pad: 0 }, after, name: name.into() })
}

const KEY: &str = "live_secret_4242";

fn config(url: String, key: &str) -> StreamConfig {
    let target = StreamTarget { service: Service::Custom, url, key: key.into() };
    let mut c = StreamConfig::for_target(target, 64, 64, 30, Codec::H264);
    c.video_kbps = 1000;
    c.connect_timeout = Duration::from_secs(5);
    c.backoff = Duration::from_millis(50);
    c
}

fn grey() -> (FrameSource, Arc<AtomicU64>) {
    let calls = Arc::new(AtomicU64::new(0));
    let c = calls.clone();
    (
        FrameSource::Rgba(Arc::new(move |b: &mut [u8]| {
            c.fetch_add(1, Ordering::Relaxed);
            b.fill(128);
            true
        })),
        calls,
    )
}

fn silence() -> SoundSource {
    SoundSource::Feed(Box::new(|n| vec![0; n * 2]))
}

/// Timestamps 1/30 s apart (or whole numbers of frames when one was skipped).
fn assert_frame_spacing(ts: &[u32]) {
    for w in ts.windows(2) {
        let d = w[1] as f64 - w[0] as f64;
        let k = (d / 33.333).round();
        assert!(k >= 1.0 && (d - k * 33.333).abs() <= 1.0, "timestamps {ts:?}");
    }
}

// ---- tests ----

/// The picture and network threads' counters fill in while streaming.
#[test]
fn a_stream_counts_its_encoding_and_sending_time() {
    let srv = Server::start(ServerOpts { expected_key: Some(KEY.into()), ..Default::default() });
    let (frames, calls) = grey();
    let s = Streamer::start(config(srv.url(), KEY), frames, silence(), fakes(30, 0)).unwrap();
    srv.wait("10 video frames", 15, |c| c.first().is_some_and(|c| c.video.len() >= 10));
    let tm = s.stop().timing;
    assert!(tm.frames_in >= 10 && calls.load(Ordering::Relaxed) > 0, "{tm:?}");
    assert!(tm.packets_out >= 10 && tm.sends >= 10, "{tm:?}");
    assert!(tm.encode_us >= tm.encode_max_us, "{tm:?}");
    assert_eq!(tm.encoder_dropped, 0, "the fake encoder drops nothing");
    assert!(tm.summary(1.0).starts_with("encoder took "));
}

#[test]
fn publishes_metadata_sequence_headers_and_frames_with_timestamps() {
    let srv = Server::start(ServerOpts { expected_key: Some(KEY.into()), ..Default::default() });
    let (frames, _) = grey();
    let s = Streamer::start(config(srv.url(), KEY), frames, silence(), fakes(30, 0)).unwrap();
    srv.wait("20 video frames", 15, |c| c.first().is_some_and(|c| c.video.len() >= 20 && c.audio.len() >= 5));
    let live = s.stats();
    assert_eq!(live.state, State::Live);
    assert!(live.frames_sent > 0);
    assert_eq!(live.video_encoder.as_ref().map(|e| e.name.as_str()), Some("fake video"));
    let last = s.stop();
    assert_eq!(last.state, State::Stopped);
    assert!(last.bytes_sent > 0);
    // The server reads on its own thread: give it time to see the goodbye (slow CI machines).
    srv.wait("deleteStream at the server", 10, |c| c.first().is_some_and(|c| c.commands.iter().any(|x| x == "deleteStream")));

    let g = srv.conns.lock().unwrap();
    let c = &g[0];
    assert_eq!(c.app, "live");
    assert_eq!(c.tc_url, srv.url());
    assert_eq!(c.publish_name, KEY);
    for cmd in ["connect", "releaseStream", "FCPublish", "createStream", "publish"] {
        assert!(c.commands.iter().any(|x| x == cmd), "{cmd} missing: {:?}", c.commands);
    }
    // Metadata.
    assert_eq!(c.metadata[0].as_str(), Some("@setDataFrame"));
    assert_eq!(c.metadata[2].get("width").and_then(Amf::as_number), Some(64.0));
    assert_eq!(c.metadata[2].get("audiocodecid").and_then(Amf::as_number), Some(10.0));
    // Sequence headers first.
    assert_eq!(&c.audio[0].1, &[0xAF, 0, 0x11, 0x90]);
    assert_eq!(&c.video[0].1[..2], &[0x17, 0]);
    assert_eq!(&c.video[0].1[5..9], &[1, 0x42, 0xC0, 0x1F]);
    assert_eq!(&c.video[1].1[..5], &[0x17, 1, 0, 0, 0]);
    assert_eq!(c.video[0].0, c.video[1].0);
    // Frames as length-prefixed NAL units, 1/30 s apart.
    let p = c.video.iter().skip(2).find(|v| v.1[0] == 0x27).expect("an inter frame");
    assert_eq!(&p.1[5..10], &[0, 0, 0, 5, 0x41]);
    let ts: Vec<u32> = c.video[1..].iter().filter(|v| v.1[1] == 1).map(|v| v.0).collect();
    assert_frame_spacing(&ts);
    // Sound 1024 samples (21.3 ms) apart.
    for w in c.audio[1..].windows(2) {
        assert!((21..=22).contains(&(w[1].0 - w[0].0)), "{:?}", c.audio);
    }
    // Upkeep and a clean end.
    assert!(c.ping_answered);
    assert!(c.acks > 0);
    assert!(c.commands.iter().any(|x| x == "deleteStream"), "{:?}", c.commands);
}

#[test]
fn reconnects_after_the_server_drops_the_connection() {
    let srv = Server::start(ServerOpts { drop_first_after_media: Some(12), ..Default::default() });
    let (frames, _) = grey();
    let mut cfg = config(srv.url(), KEY);
    cfg.max_reconnects = 3;
    // Keyframes only when asked: the second connection's keyframe proves the engine asked.
    let s = Streamer::start(cfg, frames, silence(), fakes(100_000, 0)).unwrap();
    srv.wait("a second connection with frames", 15, |c| c.len() >= 2 && c[1].video.len() >= 6);
    let st = s.stats();
    assert!(st.reconnects >= 1, "{st:?}");
    assert_eq!(st.state, State::Live);
    // The health counts the reconnect, and the connection time starts again with it.
    let h = &st.health.outputs[0];
    assert_eq!(h.reconnects, st.reconnects);
    assert!(h.connected_s > 0.0 && h.connected_s < st.uptime_s, "{h:?} after {} s", st.uptime_s);
    // The new connection's bytes keep adding up: measured once it has run five seconds.
    let t = Instant::now();
    while !s.stats().health.outputs[0].measured && t.elapsed() < Duration::from_secs(15) {
        std::thread::sleep(Duration::from_millis(100));
    }
    let h = s.stats().health.outputs[0].clone();
    assert!(h.measured && h.sent_kbps > 0, "{h:?}");
    drop(s);
    let g = srv.conns.lock().unwrap();
    let c = &g[1];
    assert_eq!(c.publish_name, KEY);
    assert!(!c.metadata.is_empty());
    assert_eq!(&c.audio[0].1, &[0xAF, 0, 0x11, 0x90]);
    assert_eq!(&c.video[0].1[..2], &[0x17, 0]);
    assert_eq!(&c.video[1].1[..2], &[0x17, 1]);
    // Timestamps start again near zero on the new connection.
    assert!(c.video[1].0 < 1000, "{:?}", c.video[1].0);
    let ts: Vec<u32> = c.video[1..].iter().filter(|v| v.1[1] == 1).map(|v| v.0).collect();
    assert_frame_spacing(&ts);
}

/// A server that takes the stream and drops it at the first packet, every time: the engine
/// waits between tries and gives up after `max_reconnects`, instead of reconnecting at once
/// forever (each connection used to reset the count).
#[test]
fn a_server_that_drops_every_stream_at_once_is_given_up_on() {
    let srv = Server::start(ServerOpts { drop_all_at_media: true, ..Default::default() });
    let (frames, _) = grey();
    let mut cfg = config(srv.url(), KEY);
    cfg.max_reconnects = 2;
    let t = Instant::now();
    let s = Streamer::start(cfg, frames, silence(), fakes(30, 0)).unwrap();
    while s.stats().state != State::Failed && t.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let st = s.stats();
    assert_eq!(st.state, State::Failed, "{st:?}");
    // Waited 50 ms, then 100 ms, between the three connections.
    assert!(t.elapsed() >= Duration::from_millis(150));
    drop(s);
    let n = srv.conns.lock().unwrap().len();
    assert_eq!(n, 3, "the first connection and two more, no more");
}

#[test]
fn a_wrong_key_fails_without_showing_the_key() {
    let srv = Server::start(ServerOpts { expected_key: Some("the_right_one".into()), ..Default::default() });
    let (frames, _) = grey();
    let mut cfg = config(srv.url(), KEY);
    cfg.max_reconnects = 0;
    let s = Streamer::start(cfg, frames, SoundSource::None, fakes(30, 0)).unwrap();
    let t = Instant::now();
    while s.stats().state != State::Failed && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let st = s.stats();
    assert_eq!(st.state, State::Failed);
    let e = st.error.unwrap();
    assert!(e.contains("BadName"), "{e}");
    assert!(!e.contains(KEY), "{e}");
    assert!(!s.is_running());
    assert_eq!(s.stop().state, State::Failed);
}

#[test]
fn a_slow_network_drops_frames_and_never_holds_up_the_capture() {
    let srv = Server::start(ServerOpts { stall: true, ..Default::default() });
    let (frames, calls) = grey();
    let mut cfg = config(srv.url(), KEY);
    cfg.max_queue_ms = 300;
    cfg.write_timeout = Duration::from_secs(60);
    // 400 KB frames at 30 fps: far more than the stalled server takes.
    let s = Streamer::start(cfg, frames, silence(), fakes(30, 400_000)).unwrap();
    srv.wait("publishing", 10, |c| c.first().is_some_and(|c| !c.publish_name.is_empty()));
    let t = Instant::now();
    let c0 = calls.load(Ordering::Relaxed);
    while s.stats().frames_dropped < 10 && t.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let st = s.stats();
    assert!(st.frames_dropped >= 10, "{st:?}");
    assert!(st.queue_ms >= 300 || st.frames_dropped > 0);
    // The health counts them as the network's, not the computer's (the fake encoder keeps up).
    let h = &st.health;
    assert_eq!(h.outputs.len(), 1);
    // (Read a moment after the other numbers, while frames keep being dropped.)
    assert!(h.outputs[0].dropped >= st.outputs[0].frames_dropped, "{h:?}");
    assert!(h.encoder_lost >= st.timing.encoder_dropped + st.timing.late, "{h:?}");
    assert!(h.set_kbps >= st.outputs.len() as u32 * 300, "{h:?}");
    // The capture kept going meanwhile. Held up by the stuck network it would make next to
    // nothing; a quarter of the frames it was due tells that apart even on a 3-core CI Mac
    // running every test at once (it made 13 a second there once, against 30 alone).
    let secs = t.elapsed().as_secs_f64();
    let made = calls.load(Ordering::Relaxed) - c0;
    assert!(made as f64 >= secs * 30.0 * 0.25, "{made} frames in {secs:.1} s");
    // Stopping doesn't hang on the stuck connection for long.
    let t = Instant::now();
    drop(s);
    assert!(t.elapsed() < Duration::from_secs(8), "{:?}", t.elapsed());
}

#[test]
fn stopping_while_waiting_to_reconnect_is_quick() {
    // A server that hangs up at once. (Not a closed port: the system may hand that port to
    // another test's server meanwhile, and this stream would then go live there.)
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming() {
            drop(s);
        }
    });
    let (frames, _) = grey();
    let mut cfg = config(format!("rtmp://127.0.0.1:{port}/live"), KEY);
    cfg.max_reconnects = 5;
    cfg.backoff = Duration::from_secs(20);
    let s = Streamer::start(cfg, frames, SoundSource::None, fakes(30, 0)).unwrap();
    let t = Instant::now();
    while s.stats().error.is_none() && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!s.stats().error.unwrap().contains(KEY));
    let t = Instant::now();
    assert_eq!(s.stop().state, State::Stopped);
    assert!(t.elapsed() < Duration::from_secs(3));
}

#[test]
fn start_reports_bad_addresses_and_missing_encoders_at_once() {
    let (frames, _) = grey();
    let e = Streamer::start(config("http://x/live".into(), KEY), frames.clone(), SoundSource::None, fakes(30, 0)).err().unwrap();
    assert!(!e.contains(KEY));
    let mut enc = fakes(30, 0);
    enc.video = Arc::new(|_| Err(format!("no encoder for {KEY}")));
    let e = Streamer::start(config("rtmp://127.0.0.1:1/live".into(), KEY), frames, SoundSource::None, enc).err().unwrap();
    assert_eq!(e, "no encoder for ****");
}

#[test]
fn a_silent_track_goes_out_with_snagsters_own_aac_encoder() {
    // No sound source (Linux and macOS today): the stream still has an AAC track, all silence.
    let srv = Server::start(ServerOpts::default());
    let (frames, _) = grey();
    let mut enc = fakes(30, 0);
    enc.audio = Arc::new(|bps| crate::encode::OwnAac::open(bps).map(|a| Box::new(a) as Box<dyn AudioEncoder>));
    let s = Streamer::start(config(srv.url(), KEY), frames, SoundSource::Silence, enc).unwrap();
    srv.wait("sound", 15, |c| c.first().is_some_and(|c| c.video.len() >= 5 && c.audio.len() >= 5));
    let st = s.stop();
    assert!(st.notice.is_none(), "{:?}", st.notice);
    assert!(st.audio_encoder.is_some());
    let g = srv.conns.lock().unwrap();
    assert_eq!(g[0].metadata[2].get("audiocodecid").and_then(Amf::as_number), Some(10.0), "AAC");
    assert_eq!(&g[0].audio[0].1[..2], &[0xAF, 0], "the AAC sequence header first");
    // Silence costs next to nothing: a few bytes a frame.
    let frames: Vec<usize> = g[0].audio[1..].iter().map(|(_, p)| p.len()).collect();
    assert!(frames.iter().all(|&n| n < 64), "{frames:?}");
}

#[test]
fn without_an_aac_encoder_it_streams_without_sound_and_says_so() {
    let srv = Server::start(ServerOpts::default());
    let (frames, _) = grey();
    let mut enc = fakes(30, 0);
    enc.audio = Arc::new(|_| Err("no AAC encoder on this system".into()));
    let hooked = Arc::new(AtomicU64::new(0));
    let h = hooked.clone();
    let mut cfg = config(srv.url(), KEY);
    cfg.on_packet = Some(Arc::new(move |_p| {
        h.fetch_add(1, Ordering::Relaxed);
    }));
    let s = Streamer::start(cfg, frames, silence(), enc).unwrap();
    srv.wait("frames", 15, |c| c.first().is_some_and(|c| c.video.len() >= 5));
    let st = s.stop();
    assert!(st.notice.unwrap().contains("without sound"));
    assert!(st.audio_encoder.is_none());
    assert!(hooked.load(Ordering::Relaxed) >= 5, "the recording hook sees every packet");
    let g = srv.conns.lock().unwrap();
    assert!(g[0].audio.is_empty());
    assert!(g[0].metadata[2].get("audiocodecid").is_none());
}

#[test]
fn bgra_sources_are_turned_into_rgba() {
    let feed: crate::video::FrameFeed = Arc::new(|b: &mut [u8]| {
        for p in b.chunks_exact_mut(4) {
            p.copy_from_slice(&[1, 2, 3, 255]);
        }
        true
    });
    let src = FrameSource::Bgra(feed);
    let srv = Server::start(ServerOpts::default());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sn = seen.clone();
    let enc = Encoders {
        zero_copy: false,
        video: Arc::new(move |_| {
            struct Spy(Arc<Mutex<Vec<u8>>>, FakeVideo);
            impl VideoEncoder for Spy {
                fn encode(&mut self, rgba: &[u8], pts: i64, k: bool) -> Result<Vec<VideoPacket>, String> {
                    *self.0.lock().unwrap() = rgba[..4].to_vec();
                    self.1.encode(rgba, pts, k)
                }
                fn finish(&mut self) -> Result<Vec<VideoPacket>, String> {
                    Ok(Vec::new())
                }
                fn name(&self) -> EncoderName {
                    self.1.name()
                }
            }
            Ok(Box::new(Spy(sn.clone(), FakeVideo { n: 0, gop: 30, pad: 0 })) as Box<dyn VideoEncoder>)
        }),
        audio: fakes(30, 0).audio,
        fallback: None,
        #[cfg(zero_copy)]
        pick: String::new(),
        note: Arc::default(),
    };
    let s = Streamer::start(config(srv.url(), KEY), src, SoundSource::None, enc).unwrap();
    srv.wait("frames", 15, |c| c.first().is_some_and(|c| c.video.len() >= 2));
    drop(s);
    assert_eq!(*seen.lock().unwrap(), vec![3, 2, 1, 255]);
}

// ---- multistreaming ----

fn custom(url: String, key: &str) -> StreamTarget {
    StreamTarget { service: Service::Custom, url, key: key.into() }
}

/// One encode, two services: both get the whole stream (headers, frames, sound), each with its
/// own key and connection, and the numbers are kept for each.
#[test]
fn multistreaming_sends_the_same_encode_to_every_service() {
    let a = Server::start(ServerOpts { expected_key: Some(KEY.into()), ..Default::default() });
    let b = Server::start(ServerOpts { expected_key: Some("second_key_77".into()), ..Default::default() });
    let (frames, _) = grey();
    let made = Arc::new(AtomicU64::new(0));
    let mut enc = fakes(30, 0);
    let (m, inner) = (made.clone(), enc.video.clone());
    // Counts the encoders opened: one for both services.
    enc.video = Arc::new(move |c| {
        m.fetch_add(1, Ordering::Relaxed);
        inner(c)
    });
    let mut cfg = config(a.url(), KEY);
    cfg.also = vec![custom(b.url(), "second_key_77")];
    let s = Streamer::start(cfg, frames, silence(), enc).unwrap();
    for srv in [&a, &b] {
        srv.wait("15 frames and sound", 15, |c| c.first().is_some_and(|c| c.video.len() >= 15 && c.audio.len() >= 5));
    }
    let st = s.stats();
    assert_eq!(made.load(Ordering::Relaxed), 1, "encoded once");
    assert_eq!(st.state, State::Live);
    assert_eq!(st.outputs.len(), 2);
    assert!(st.outputs.iter().all(|o| o.state == State::Live && o.frames_sent > 0 && o.service == "Custom server"), "{:?}", st.outputs);
    assert!(!format!("{st:?}").contains("second_key_77") && !format!("{st:?}").contains(KEY));
    let last = s.stop();
    assert_eq!(last.state, State::Stopped);
    assert!(last.outputs.iter().all(|o| o.state == State::Stopped && o.bytes_sent > 0));
    assert_eq!(last.bytes_sent, last.outputs.iter().map(|o| o.bytes_sent).sum::<u64>());
    for (srv, key) in [(&a, KEY), (&b, "second_key_77")] {
        srv.wait("deleteStream", 10, |c| c.first().is_some_and(|c| c.commands.iter().any(|x| x == "deleteStream")));
        let g = srv.conns.lock().unwrap();
        assert_eq!(g[0].publish_name, key);
        assert_eq!(&g[0].video[0].1[..2], &[0x17, 0], "each starts with its own sequence header");
        assert_eq!(&g[0].audio[0].1, &[0xAF, 0, 0x11, 0x90]);
        let ts: Vec<u32> = g[0].video[1..].iter().filter(|v| v.1[1] == 1).map(|v| v.0).collect();
        assert_frame_spacing(&ts);
    }
}

/// A service that refuses the stream is given up on alone: the other stays live, and the error
/// names the service without either key.
#[test]
fn one_service_failing_leaves_the_others_live() {
    let good = Server::start(ServerOpts::default());
    let bad = Server::start(ServerOpts { expected_key: Some("the_right_one".into()), ..Default::default() });
    let (frames, _) = grey();
    let mut cfg = config(good.url(), KEY);
    cfg.max_reconnects = 0;
    cfg.also = vec![custom(bad.url(), "wrong_key_55")];
    let s = Streamer::start(cfg, frames, SoundSource::None, fakes(30, 0)).unwrap();
    let t = Instant::now();
    while s.stats().outputs[1].state != State::Failed && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let before = good.conns.lock().unwrap().first().map(|c| c.video.len()).unwrap_or(0);
    good.wait("frames after the other failed", 15, |c| c.first().is_some_and(|c| c.video.len() >= before + 10));
    let st = s.stats();
    assert_eq!(st.state, State::Live, "{st:?}");
    assert_eq!((st.outputs[0].state, st.outputs[1].state), (State::Live, State::Failed));
    let e = st.error.unwrap();
    assert!(e.starts_with("Custom server: ") && e.contains("BadName"), "{e}");
    assert!(!e.contains("wrong_key_55") && !e.contains(KEY), "{e}");
    assert!(st.outputs[0].error.is_none());
    assert_eq!(s.stop().state, State::Stopped, "the stream as a whole didn't fail");
}

/// The health's numbers come from what the stream really did: after five seconds to a server
/// that takes everything, the rates are measured and the bytes it counted are about what the
/// connection sent. Nothing is dropped: the frames queued while the first connection was made
/// don't count (nothing was live yet).
#[test]
fn a_healthy_stream_measures_what_it_sends() {
    let srv = Server::start(ServerOpts::default());
    let (frames, _) = grey();
    let s = Streamer::start(config(srv.url(), KEY), frames, silence(), fakes(30, 2_000)).unwrap();
    srv.wait("frames", 15, |c| c.first().is_some_and(|c| c.video.len() >= 10));
    let t = Instant::now();
    while !s.stats().health.outputs.first().is_some_and(|o| o.measured) && t.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(100));
    }
    let st = s.stats();
    let h = &st.health;
    assert!(h.window_s >= 5.0 && h.outputs[0].measured, "{h:?}");
    // 2 KB pictures at 30 fps: about 480 kbit/s of video, plus the sound and the headers.
    assert!(h.outputs[0].sent_kbps > 200 && h.outputs[0].sent_kbps < 2000, "{h:?}");
    assert_eq!((h.outputs[0].dropped, h.outputs[0].reconnects), (0, 0));
    assert!(h.outputs[0].connected_s >= 5.0, "{h:?}");
    assert!(h.encode_ms >= 0.0 && h.frame_budget_ms > 33.0 && h.frame_budget_ms < 34.0, "{h:?}");
    drop(s);
}

/// A service on a slow network drops its own frames; the others get every one.
#[test]
fn a_slow_service_never_holds_up_the_others() {
    let fast = Server::start(ServerOpts::default());
    let slow = Server::start(ServerOpts { stall: true, ..Default::default() });
    let (frames, _) = grey();
    let mut cfg = config(slow.url(), KEY);
    cfg.max_queue_ms = 300;
    cfg.write_timeout = Duration::from_secs(60);
    cfg.also = vec![custom(fast.url(), "fast_key")];
    let s = Streamer::start(cfg, frames, SoundSource::None, fakes(30, 200_000)).unwrap();
    slow.wait("publishing", 10, |c| c.first().is_some_and(|c| !c.publish_name.is_empty()));
    let t = Instant::now();
    while s.stats().outputs[0].frames_dropped < 10 && t.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let st = s.stats();
    assert!(st.outputs[0].frames_dropped >= 10, "{:?}", st.outputs);
    assert_eq!(st.outputs[1].frames_dropped, 0, "{:?}", st.outputs);
    fast.wait("frames on the fast one", 15, |c| c.first().is_some_and(|c| c.video.len() >= 20));
    let t = Instant::now();
    drop(s);
    assert!(t.elapsed() < Duration::from_secs(8), "{:?}", t.elapsed());
}

#[test]
fn the_same_service_twice_and_bad_extra_addresses_are_refused_at_once() {
    let (frames, _) = grey();
    let mut cfg = config("rtmp://127.0.0.1:1/live".into(), KEY);
    cfg.also = vec![custom("rtmp://127.0.0.1:1/live".into(), KEY)];
    let e = Streamer::start(cfg, frames.clone(), SoundSource::None, fakes(30, 0)).err().unwrap();
    assert!(e.contains("twice") && !e.contains(KEY), "{e}");
    let mut cfg = config("rtmp://127.0.0.1:1/live".into(), KEY);
    cfg.also = vec![custom("ftp://127.0.0.1/also_secret_9".into(), "also_secret_9")];
    let e = Streamer::start(cfg, frames, SoundSource::None, fakes(30, 0)).err().unwrap();
    assert!(e.starts_with("Custom server: ") && !e.contains("also_secret_9"), "{e}");
}

// ---- an encoder that stops working ----

/// The graphics chip's encoder fails in the middle of a stream (a driver reset, a laptop
/// switching chips): another encoder takes over at a keyframe and the stream goes on, with a
/// note saying so. It used to end the stream.
#[test]
fn a_failing_encoder_is_replaced_and_the_stream_goes_on() {
    let srv = Server::start(ServerOpts::default());
    let (frames, _) = grey();
    let mut enc = fakes(30, 0);
    enc.video = Arc::new(|_| Ok(breaking(10, "fake chip")));
    let asked = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let a = asked.clone();
    enc.fallback = Some(Arc::new(move |_c, avoid| {
        a.lock().unwrap().push(avoid.to_vec());
        Ok(Box::new(FakeVideo { n: 0, gop: 30, pad: 0 }) as Box<dyn VideoEncoder>)
    }));
    let s = Streamer::start(config(srv.url(), KEY), frames, SoundSource::None, enc).unwrap();
    srv.wait("frames after the switch", 15, |c| c.first().is_some_and(|c| c.video.len() >= 30));
    let st = s.stats();
    assert_eq!(st.state, State::Live, "{st:?}");
    assert_eq!(st.video_encoder.as_ref().map(|e| e.name.as_str()), Some("fake video"));
    assert_eq!(st.notice.as_deref(), Some("fake chip stopped working (the graphics chip was reset); switched to fake video."));
    assert!(st.error.is_none(), "{:?}", st.error);
    assert_eq!(*asked.lock().unwrap(), vec![vec!["fake chip".to_string()]], "asked once, avoiding the one that failed");
    // The pictures the broken encoder threw away still count after the switch.
    assert_eq!(st.timing.encoder_dropped, 7, "{:?}", st.timing);
    assert!(st.health.encoder_lost >= 7, "{:?}", st.health);
    drop(s);
    // The new encoder started with a keyframe; its frames follow the old ones on the same clock.
    let g = srv.conns.lock().unwrap();
    let keys = g[0].video.iter().filter(|v| v.1[0] == 0x17 && v.1[1] == 1).count();
    assert!(keys >= 2, "a keyframe from each encoder: {keys}");
    let ts: Vec<u32> = g[0].video[1..].iter().filter(|v| v.1[1] == 1).map(|v| v.0).collect();
    assert!(ts.windows(2).all(|w| w[1] > w[0]), "timestamps keep going up: {ts:?}");
}

/// Without another encoder (or when every one fails) the stream stops and says why, as before;
/// it never tries for ever.
#[test]
fn without_a_working_encoder_the_stream_stops_and_says_why() {
    let (frames, _) = grey();
    let srv = Server::start(ServerOpts::default());
    let mut enc = fakes(30, 0);
    enc.video = Arc::new(|_| Ok(breaking(5, "fake chip")));
    let n = Arc::new(AtomicU64::new(0));
    let k = n.clone();
    // Every replacement fails at once too.
    enc.fallback = Some(Arc::new(move |_c, _avoid| Ok(breaking(0, &format!("fake chip {}", k.fetch_add(1, Ordering::SeqCst))))));
    let s = Streamer::start(config(srv.url(), KEY), frames, SoundSource::None, enc).unwrap();
    let t = Instant::now();
    while s.stats().error.is_none() && t.elapsed() < Duration::from_secs(15) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let st = s.stop();
    assert!(st.error.as_deref().is_some_and(|e| e.starts_with("Video encoder: ")), "{st:?}");
    assert_eq!(n.load(Ordering::SeqCst), 3, "three switches at most");
}
