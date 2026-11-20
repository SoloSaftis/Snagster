//! The streaming engine: a picture thread (frames → video encoder), a sound thread (samples →
//! AAC encoder) and a network thread (queue → RTMP server, reconnecting when the connection
//! drops). The window reads `Streamer::stats` every frame; it never waits on the threads.

use super::flv::{self, Meta, ParamSets};
use super::queue::{Packet, SendQueue};
use super::rtmp::Client;
use super::services::{Endpoint, StreamTarget, mask};
use crate::encode::{AudioEncoder, Codec, EncoderName, VideoConfig, VideoEncoder};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Fills one frame, RGBA, `width * height * 4` bytes, top row first. Returns false to stop.
pub type RgbaFeed = Arc<dyn Fn(&mut [u8]) -> bool + Send + Sync>;

/// Where the pictures come from.
#[derive(Clone)]
pub enum FrameSource {
    /// The recorder's feed (BGRA), e.g. `studio::Engine::feed()`.
    #[cfg(test)]
    Bgra(crate::video::FrameFeed),
    Rgba(RgbaFeed),
    /// Pictures in graphics memory, already NV12 at the stream's size (zero-copy); the encoder
    /// must have been opened for them (`encode::open_video_for_surfaces`).
    #[cfg(zero_copy)]
    Surfaces(SurfaceFeedFn),
}

/// The newest picture in graphics memory: None when the source has ended, Some(None) before
/// the first picture.
#[cfg(zero_copy)]
pub type SurfaceFeedFn = Arc<dyn Fn() -> Option<Option<crate::encode::Surface>> + Send + Sync>;

/// The picture the video thread hands the encoder next.
enum Picture {
    Memory(Vec<u8>),
    #[cfg(zero_copy)]
    Surface(Option<crate::encode::Surface>),
}

impl FrameSource {
    /// Where this source's pictures go: an RGBA buffer, or a surface.
    fn picture(&self, w: u32, h: u32) -> Picture {
        match self {
            #[cfg(zero_copy)]
            FrameSource::Surfaces(_) => Picture::Surface(None),
            _ => Picture::Memory(vec![0u8; w as usize * h as usize * 4]),
        }
    }

    /// Bring `p` up to date; false: the source has ended.
    fn take(&self, p: &mut Picture) -> bool {
        match (self, p) {
            #[cfg(zero_copy)]
            (FrameSource::Surfaces(f), Picture::Surface(s)) => match f() {
                None => false,
                Some(new) => {
                    if new.is_some() {
                        *s = new;
                    }
                    true
                }
            },
            (_, Picture::Memory(buf)) => self.fill(buf),
            #[cfg(zero_copy)]
            _ => false,
        }
    }

    /// Fill `rgba`; false: the source has ended.
    fn fill(&self, rgba: &mut [u8]) -> bool {
        match self {
            FrameSource::Rgba(f) => f(rgba),
            #[cfg(zero_copy)]
            FrameSource::Surfaces(_) => false,
            #[cfg(test)]
            FrameSource::Bgra(f) => {
                let go = f(rgba);
                for p in rgba.chunks_exact_mut(4) {
                    p.swap(0, 2);
                }
                go
            }
        }
    }
}

/// Takes `frames` stereo 48 kHz frames (interleaved i16) from wherever the sound comes from.
pub type AudioFeed = Box<dyn FnMut(usize) -> Vec<i16> + Send>;

/// Where the sound comes from.
pub enum SoundSource {
    /// No audio track at all.
    None,
    /// A silent audio track: services expect one, and some warn without it.
    Silence,
    /// System sound and/or microphone, with Studio's mixer.
    Capture(crate::audio::Capture),
    #[cfg(test)]
    Feed(AudioFeed),
}

impl SoundSource {
    fn into_feed(self) -> Option<AudioFeed> {
        match self {
            SoundSource::None => None,
            SoundSource::Silence => Some(Box::new(|n| vec![0; n * 2])),
            #[cfg(test)]
            SoundSource::Feed(f) => Some(f),
            SoundSource::Capture(c) => Some(capture_feed(c)),
        }
    }
}

/// The capture's mix (computer sound and microphones on Windows; sound files everywhere).
fn capture_feed(c: crate::audio::Capture) -> AudioFeed {
    Box::new(move |n| c.take(n))
}

pub type VideoFactory = Arc<dyn Fn(&VideoConfig) -> Result<Box<dyn VideoEncoder>, String> + Send + Sync>;
pub type AudioFactory = Arc<dyn Fn(u32) -> Result<Box<dyn AudioEncoder>, String> + Send + Sync>;
/// Another encoder, any but those named (they failed while streaming).
pub type FallbackFactory = Arc<dyn Fn(&VideoConfig, &[String]) -> Result<Box<dyn VideoEncoder>, String> + Send + Sync>;

/// How encoders are made (the system's, or fakes in tests).
#[derive(Clone)]
pub struct Encoders {
    pub video: VideoFactory,
    pub audio: AudioFactory,
    /// When the video encoder fails while streaming: another one, so the stream goes on.
    pub fallback: Option<FallbackFactory>,
    /// These are the system's encoders, so a zero-copy source may open its own on its device
    /// (`with_surfaces`); fakes in the tests take pictures from memory.
    pub zero_copy: bool,
    /// The encoder the person chose ("" automatic), for the zero-copy encoders opened on the
    /// screen copy's device (memory encoders get it inside `video`).
    #[cfg(zero_copy)]
    pub pick: String,
    /// Set by `video` when the pick couldn't start and another was used (for the status line).
    pub note: Arc<Mutex<Option<String>>>,
}

impl Encoders {
    /// This computer's encoders (graphics chip first, then software).
    pub fn system() -> Encoders {
        Encoders::system_picked("")
    }

    /// This computer's encoders, `pick` first ("" automatic; see `encode::open_video_picked`).
    pub fn system_picked(pick: &str) -> Encoders {
        let note: Arc<Mutex<Option<String>>> = Arc::default();
        let (n, p) = (note.clone(), pick.trim().to_string());
        Encoders {
            video: Arc::new(move |c: &VideoConfig| {
                let (e, why) = crate::encode::open_video_picked(c, &p)?;
                if let Ok(mut g) = n.lock() {
                    *g = why;
                }
                Ok(e)
            }),
            audio: Arc::new(crate::encode::open_audio),
            fallback: Some(Arc::new(crate::encode::open_video_avoiding)),
            zero_copy: true,
            #[cfg(zero_copy)]
            pick: pick.trim().to_string(),
            note,
        }
    }

    /// What `video` noted about the pick, once.
    fn take_note(&self) -> Option<String> {
        self.note.lock().ok().and_then(|mut g| g.take())
    }

    /// Video encoders on `dev` for pictures in graphics memory, starting with `first`, already
    /// opened for `cfg` (so checking that zero-copy works doesn't cost a second encoder).
    #[cfg(zero_copy)]
    pub fn with_surfaces(&self, dev: crate::encode::SurfaceDevice, first: Box<dyn VideoEncoder>, cfg: VideoConfig) -> Encoders {
        let first = Mutex::new(Some(first));
        let (d, pick) = (dev.clone(), self.pick.clone());
        Encoders {
            video: Arc::new(move |c: &VideoConfig| {
                if c == &cfg
                    && let Some(e) = first.lock().ok().and_then(|mut f| f.take())
                {
                    return Ok(e);
                }
                crate::encode::open_video_for_surfaces(c, &d, &pick, &[])
            }),
            audio: self.audio.clone(),
            // Another graphics-chip encoder on the same device (the pictures are there).
            fallback: Some(Arc::new(move |c: &VideoConfig, avoid: &[String]| crate::encode::open_video_for_surfaces(c, &dev, "", avoid))),
            zero_copy: false,
            pick: self.pick.clone(),
            note: self.note.clone(),
        }
    }
}

/// Called with every packet the encoders make, before it is queued for the network: the hook
/// for recording the stream to a file at the same time. It runs on the encoding threads, so it
/// must be quick (hand the packet to a channel).
pub type PacketHook = Arc<dyn Fn(&Packet) + Send + Sync>;

/// What to stream and how.
#[derive(Clone)]
pub struct StreamConfig {
    pub target: StreamTarget,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub video_kbps: u32,
    pub audio_kbps: u32,
    pub codec: Codec,
    pub hardware: bool,
    pub keyframe_s: u32,
    /// Reconnect this many times in a row before giving up (0: don't).
    pub max_reconnects: u32,
    /// Video is dropped when more than this is waiting to be sent.
    pub max_queue_ms: u32,
    pub connect_timeout: Duration,
    /// A send that takes longer than this counts as a lost connection.
    pub write_timeout: Duration,
    /// First wait before reconnecting; doubles each try up to 30 s.
    pub backoff: Duration,
    pub on_packet: Option<PacketHook>,
    /// More services fed from the same encode (multistreaming): each gets its own connection,
    /// queue, reconnects and numbers, and one that fails leaves the others live.
    pub also: Vec<StreamTarget>,
}

impl std::fmt::Debug for StreamConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamConfig")
            .field("target", &self.target)
            .field("also", &self.also)
            .field("size", &(self.width, self.height, self.fps))
            .field("kbps", &(self.video_kbps, self.audio_kbps))
            .field("codec", &self.codec)
            .finish_non_exhaustive()
    }
}

impl StreamConfig {
    /// Settings for a service: its recommended bitrate for the size, frame rate and codec,
    /// keyframes every 2 s, the size and frame rate brought within what it takes (its largest
    /// picture, its highest frame rate and its pixel rate: 4K at 60 at most), HEVC only where it
    /// takes HEVC.
    pub fn for_target(target: StreamTarget, width: u32, height: u32, fps: u32, codec: Codec) -> StreamConfig {
        let p = target.service.preset();
        let (w, h) = p.fit_size(width, height);
        let fps = p.fps_for(w, h, fps);
        let codec = if p.hevc { codec } else { Codec::H264 };
        StreamConfig {
            width: w,
            height: h,
            fps,
            video_kbps: p.recommended_kbps(w, h, fps, codec),
            audio_kbps: p.audio_kbps,
            codec,
            hardware: true,
            keyframe_s: p.keyframe_s,
            max_reconnects: 5,
            max_queue_ms: 2000,
            connect_timeout: Duration::from_secs(10),
            write_timeout: Duration::from_secs(10),
            backoff: Duration::from_secs(1),
            on_packet: None,
            target,
            also: Vec::new(),
        }
    }

    /// Every service this stream goes to, the main one first.
    pub fn targets(&self) -> impl Iterator<Item = &StreamTarget> {
        std::iter::once(&self.target).chain(&self.also)
    }

    /// Bring the picture within what every service takes, since one encode feeds them all: the
    /// smallest size and frame rate any of them takes, H.264 unless all take HEVC, and a
    /// bitrate none of them refuses. (Size and rate only ever come down, so the main service's
    /// limits still hold.)
    pub fn fit_all(&mut self) {
        let presets: Vec<_> = self.targets().map(|t| t.service.preset()).collect();
        for p in &presets {
            (self.width, self.height) = p.fit_size(self.width, self.height);
        }
        for p in &presets {
            self.fps = p.fps_for(self.width, self.height, self.fps);
            if !p.hevc {
                self.codec = Codec::H264;
            }
            self.video_kbps = self.video_kbps.min(p.max_video_kbps);
            self.audio_kbps = self.audio_kbps.min(p.audio_kbps);
        }
    }

    /// The bitrate every service recommends or takes for this picture: the lowest of them.
    pub fn recommended_for_all(&self) -> u32 {
        self.targets().map(|t| t.service.preset().recommended_kbps(self.width, self.height, self.fps, self.codec)).min().unwrap_or(500)
    }
}

/// A sensible H.264 bitrate for a size and frame rate (about 0.1 bit per pixel at 30 fps).
pub fn recommended_kbps(w: u32, h: u32, fps: u32) -> u32 {
    let px = w as f64 * h as f64 * fps.clamp(1, 120) as f64;
    ((px * 0.1 / 1000.0) as u32).clamp(500, 50_000)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Connecting,
    Live,
    Reconnecting,
    Stopping,
    Stopped,
    /// Gave up (see `Stats::error`).
    Failed,
}

impl State {
    fn from_u8(v: u8) -> State {
        [State::Connecting, State::Live, State::Reconnecting, State::Stopping, State::Stopped, State::Failed].get(v as usize).copied().unwrap_or(State::Failed)
    }
}

/// A snapshot for the window or MCP.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub state: State,
    /// Bits per second going out (last second).
    pub bitrate_bps: u64,
    /// Video frames per second going out (last second).
    pub fps: f32,
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub bytes_sent: u64,
    /// How much is waiting to be sent.
    pub queue_ms: u32,
    /// Seconds since the stream first went live.
    pub uptime_s: f64,
    pub reconnects: u32,
    /// What went wrong last (the key masked).
    pub error: Option<String>,
    /// Something the person should know (e.g. streaming without sound).
    pub notice: Option<String>,
    pub video_encoder: Option<EncoderName>,
    pub audio_encoder: Option<EncoderName>,
    /// Where the picture and network threads spend their time (for `--stream-test`).
    pub timing: Timing,
    /// Each service on its own, the main one first (one entry when not multistreaming). The
    /// numbers above add them up: live while any is live, failed only when all have failed.
    pub outputs: Vec<OutputStats>,
}

/// One service of a stream.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutputStats {
    /// The service's name ("Twitch").
    pub service: String,
    /// The server address with the key masked.
    pub address: String,
    pub state: State,
    pub bitrate_bps: u64,
    pub fps: f32,
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub bytes_sent: u64,
    pub queue_ms: u32,
    pub reconnects: u32,
    pub error: Option<String>,
}

/// The state of a stream going to several services: live while any of them is live (one that
/// failed doesn't take the others down), failed only when every one has failed.
pub fn overall_state(states: &[State]) -> State {
    let any = |s: State| states.contains(&s);
    if states.is_empty() || states.iter().all(|&s| s == State::Failed) {
        State::Failed
    } else if any(State::Live) {
        State::Live
    } else if any(State::Reconnecting) {
        State::Reconnecting
    } else if any(State::Connecting) {
        State::Connecting
    } else if any(State::Stopping) {
        State::Stopping
    } else {
        State::Stopped
    }
}

/// The numbers of the whole stream from its services' (see `Stats::outputs`).
fn add_up(outs: &[OutputStats]) -> (State, u64, f32, u64, u64, u64, u32, u32, Option<String>) {
    let states: Vec<State> = outs.iter().map(|o| o.state).collect();
    let error = if outs.len() == 1 {
        outs[0].error.clone()
    } else {
        // Name the service when there are several.
        outs.iter().find_map(|o| o.error.as_ref().map(|e| format!("{}: {e}", o.service)))
    };
    (
        overall_state(&states),
        outs.iter().map(|o| o.bitrate_bps).sum(),
        outs.iter().map(|o| o.fps).fold(0.0, f32::max),
        outs.iter().map(|o| o.frames_sent).max().unwrap_or(0),
        outs.iter().map(|o| o.frames_dropped).max().unwrap_or(0),
        outs.iter().map(|o| o.bytes_sent).sum(),
        outs.iter().map(|o| o.queue_ms).max().unwrap_or(0),
        outs.iter().map(|o| o.reconnects).sum(),
        error,
    )
}

/// Running totals of the picture and network threads' work, so a slow stream shows which step
/// is slow: taking the picture, encoding it or sending it. Two snapshots give the numbers for
/// the time between them (`Timing::since`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Timing {
    /// Pictures handed to the video encoder.
    pub frames_in: u64,
    /// Video packets the encoder gave back.
    pub packets_out: u64,
    /// Pictures the encoder itself threw away (it couldn't keep up).
    pub encoder_dropped: u64,
    /// Frame slots skipped because the picture thread fell behind its timeline.
    pub late: u64,
    /// Microseconds taking pictures from the source.
    pub fill_us: u64,
    /// Microseconds inside the encoder, and the longest single call since the start.
    pub encode_us: u64,
    pub encode_max_us: u64,
    /// Messages written to the server and the microseconds that took.
    pub sends: u64,
    pub send_us: u64,
}

impl Timing {
    /// The work done between `earlier` and `self` (the longest call stays the longest since
    /// the start).
    pub fn since(&self, earlier: &Timing) -> Timing {
        Timing {
            frames_in: self.frames_in.saturating_sub(earlier.frames_in),
            packets_out: self.packets_out.saturating_sub(earlier.packets_out),
            encoder_dropped: self.encoder_dropped.saturating_sub(earlier.encoder_dropped),
            late: self.late.saturating_sub(earlier.late),
            fill_us: self.fill_us.saturating_sub(earlier.fill_us),
            encode_us: self.encode_us.saturating_sub(earlier.encode_us),
            encode_max_us: self.encode_max_us,
            sends: self.sends.saturating_sub(earlier.sends),
            send_us: self.send_us.saturating_sub(earlier.send_us),
        }
    }

    /// One line for a person: rates over `secs` seconds and the average time of each step.
    pub fn summary(&self, secs: f64) -> String {
        let secs = secs.max(1e-6);
        let per = |us: u64, n: u64| if n == 0 { 0.0 } else { us as f64 / n as f64 / 1000.0 };
        format!(
            "encoder took {:.1} pictures a second and gave back {:.1} ({} thrown away by it, {} frame slots late); per picture: taking it {:.2} ms, encoding {:.2} ms (longest {:.1} ms); per message sent {:.2} ms",
            self.frames_in as f64 / secs,
            self.packets_out as f64 / secs,
            self.encoder_dropped,
            self.late,
            per(self.fill_us, self.frames_in),
            per(self.encode_us, self.frames_in),
            self.encode_max_us as f64 / 1000.0,
            per(self.send_us, self.sends),
        )
    }
}

/// The live side of `Timing`, written by the stream's threads.
#[derive(Default)]
struct TimingCounters {
    frames_in: AtomicU64,
    packets_out: AtomicU64,
    encoder_dropped: AtomicU64,
    late: AtomicU64,
    fill_us: AtomicU64,
    encode_us: AtomicU64,
    encode_max_us: AtomicU64,
    sends: AtomicU64,
    send_us: AtomicU64,
}

impl TimingCounters {
    fn snapshot(&self) -> Timing {
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        Timing {
            frames_in: g(&self.frames_in),
            packets_out: g(&self.packets_out),
            encoder_dropped: g(&self.encoder_dropped),
            late: g(&self.late),
            fill_us: g(&self.fill_us),
            encode_us: g(&self.encode_us),
            encode_max_us: g(&self.encode_max_us),
            sends: g(&self.sends),
            send_us: g(&self.send_us),
        }
    }
}

/// Add the microseconds since `since` to `a`; returns them.
fn add_us(a: &AtomicU64, since: Instant) -> u64 {
    let us = since.elapsed().as_micros() as u64;
    a.fetch_add(us, Ordering::Relaxed);
    us
}

/// One service's connection: its own queue (a slow one drops its own frames and never holds up
/// the others), state, reconnects and numbers.
#[derive(Default)]
struct Output {
    service: String,
    address: String,
    /// Masked out of this connection's errors.
    key: String,
    state: AtomicU8,
    bitrate: AtomicU64,
    fps_x100: AtomicU32,
    frames_sent: AtomicU64,
    frames_dropped: AtomicU64,
    bytes_sent: AtomicU64,
    queue_ms: AtomicU32,
    reconnects: AtomicU32,
    error: Mutex<Option<String>>,
    queue: Mutex<Option<SendQueue>>,
    wake: Condvar,
}

impl Output {
    fn state(&self) -> State {
        State::from_u8(self.state.load(Ordering::Relaxed))
    }

    fn set_state(&self, s: State) {
        self.state.store(s as u8, Ordering::Relaxed);
    }

    fn set_error(&self, e: String) {
        if let Ok(mut g) = self.error.lock() {
            *g = Some(mask(&e, &self.key));
        }
    }

    fn with_queue<R>(&self, f: impl FnOnce(&mut SendQueue) -> R) -> Option<R> {
        let mut g = self.queue.lock().ok()?;
        let r = g.as_mut().map(f);
        self.wake.notify_all();
        r
    }

    fn stats(&self) -> OutputStats {
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        OutputStats {
            service: self.service.clone(),
            address: self.address.clone(),
            state: self.state(),
            bitrate_bps: g(&self.bitrate),
            fps: self.fps_x100.load(Ordering::Relaxed) as f32 / 100.0,
            frames_sent: g(&self.frames_sent),
            frames_dropped: g(&self.frames_dropped),
            bytes_sent: g(&self.bytes_sent),
            queue_ms: self.queue_ms.load(Ordering::Relaxed),
            reconnects: self.reconnects.load(Ordering::Relaxed),
            error: self.error.try_lock().ok().and_then(|e| e.clone()),
        }
    }
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    /// Stopping or stopped by `Streamer::stop` (0: not yet).
    ending: AtomicU8,
    /// The picture thread should make the next frame a keyframe.
    want_key: AtomicBool,
    /// When the first service went live (ms since the start, + 1; 0: not yet).
    live_since_ms: AtomicU64,
    /// What stopped the encoders (every service then ends).
    error: Mutex<Option<String>>,
    notice: Mutex<Option<String>>,
    names: Mutex<(Option<EncoderName>, Option<EncoderName>)>,
    outputs: Vec<Output>,
    /// Give up sending at once (stopping took too long).
    abort: Arc<AtomicBool>,
    timing: TimingCounters,
}

impl Shared {
    fn set_error(&self, e: String) {
        if let Ok(mut g) = self.error.lock() {
            let e = self.outputs.iter().fold(e, |e, o| mask(&e, &o.key));
            *g = Some(e);
        }
    }

    /// Every service's queue.
    fn each_queue(&self, mut f: impl FnMut(&Output, &mut SendQueue)) {
        for o in &self.outputs {
            o.with_queue(|q| f(o, q));
        }
    }
}

/// A running stream. Dropping it stops it.
pub struct Streamer {
    shared: Arc<Shared>,
    started: Instant,
    /// The encoders' threads, then one network thread per service.
    threads: Vec<JoinHandle<()>>,
    net_threads: Vec<JoinHandle<()>>,
    /// Keeps macOS from napping Snagster or stretching its timers while streaming.
    _timely: crate::video::TimelyActivity,
}

/// What the video encoder of a stream with `cfg` is asked for.
pub fn video_config(cfg: &StreamConfig) -> Result<VideoConfig, String> {
    let (w, h) = crate::video::even_size(cfg.width, cfg.height)?;
    Ok(VideoConfig {
        width: w,
        height: h,
        fps: cfg.fps.clamp(1, 120),
        bitrate_bps: cfg.video_kbps.saturating_mul(1000),
        keyframe_every_s: cfg.keyframe_s.max(1),
        codec: cfg.codec,
        prefer_hardware: cfg.hardware,
        low_latency: true,
    })
}

impl Streamer {
    /// Check the address, open the encoders and start streaming in the background. Errors (bad
    /// address, no video encoder) come back at once; connection problems show in `stats`.
    pub fn start(cfg: StreamConfig, frames: FrameSource, sound: SoundSource, encoders: Encoders) -> Result<Streamer, String> {
        let keys: Vec<String> = cfg.targets().map(|t| t.key.clone()).collect();
        let mask_all = |e: String| keys.iter().fold(e, |e, k| mask(&e, k));
        let mut outputs = Vec::new();
        let mut endpoints = Vec::new();
        for (i, t) in cfg.targets().enumerate() {
            let ep = t.endpoint().map_err(|e| if i == 0 { e } else { format!("{}: {e}", t.service.preset().name) }).map_err(mask_all)?;
            if endpoints.iter().any(|e: &Endpoint| e.tc_url == ep.tc_url && e.stream == ep.stream) {
                return Err(format!("{} is in the list twice.", t.service.preset().name));
            }
            endpoints.push(ep);
            outputs.push(Output { service: t.service.preset().name.to_string(), address: t.display(), key: t.key.clone(), queue: Mutex::new(Some(SendQueue::new(cfg.max_queue_ms))), ..Default::default() });
        }
        Streamer::launch(cfg, outputs, endpoints, frames, sound, encoders, "Live streaming", &mask_all)
    }

    /// Encode only, for no service: every packet goes to `cfg.on_packet` (the replay buffer
    /// keeps them); `cfg.target` isn't used. `why` names the work for macOS's activity.
    pub fn start_local(cfg: StreamConfig, frames: FrameSource, sound: SoundSource, encoders: Encoders, why: &'static str) -> Result<Streamer, String> {
        if cfg.on_packet.is_none() {
            return Err("nothing takes the packets".into());
        }
        Streamer::launch(cfg, Vec::new(), Vec::new(), frames, sound, encoders, why, &|e| e)
    }

    #[allow(clippy::too_many_arguments)]
    fn launch(cfg: StreamConfig, outputs: Vec<Output>, endpoints: Vec<Endpoint>, frames: FrameSource, sound: SoundSource, encoders: Encoders, why: &'static str, mask_all: &dyn Fn(String) -> String) -> Result<Streamer, String> {
        let vcfg = video_config(&cfg)?;
        let (w, h, fps) = (vcfg.width, vcfg.height, vcfg.fps);
        let venc = (encoders.video)(&vcfg).map_err(mask_all)?;
        let shared = Arc::new(Shared { outputs, ..Default::default() });
        // The chosen encoder couldn't start and another one is used: say so.
        let mut notes: Vec<String> = encoders.take_note().into_iter().collect();
        let mut sound = sound.into_feed();
        let mut aenc = None;
        if sound.is_some() {
            match (encoders.audio)(cfg.audio_kbps.saturating_mul(1000)) {
                Ok(a) => aenc = Some(a),
                Err(e) => {
                    sound = None;
                    notes.push(if why == "Live streaming" { format!("Streaming without sound: {e}") } else { format!("Kept without sound: {e}") });
                }
            }
        }
        if !notes.is_empty() {
            *shared.notice.lock().unwrap_or_else(|p| p.into_inner()) = Some(notes.join(" "));
        }
        *shared.names.lock().unwrap_or_else(|p| p.into_inner()) = (Some(venc.name()), aenc.as_ref().map(|a| a.name()));
        let asc = aenc.as_ref().map(|a| a.audio_specific_config());
        let meta = Meta { width: w, height: h, fps, video_kbps: cfg.video_kbps, codec: cfg.codec, audio_kbps: asc.as_ref().map(|_| cfg.audio_kbps) };
        // How this machine's sleeps behave, found before the clock starts (it takes a few ms).
        let pacer = crate::video::Pacer::calibrated();
        let started = Instant::now();
        let mut threads = Vec::new();
        let hook = cfg.on_packet.clone();
        let sh = shared.clone();
        let swap = Swap { cfg: vcfg.clone(), fallback: encoders.fallback.clone() };
        threads.push(spawn("snagster-stream-video", move || video_thread(sh, frames, venc, swap, started, pacer, hook)).map_err(|e| e.to_string())?);
        if let (Some(feed), Some(enc)) = (sound, aenc) {
            let sh = shared.clone();
            let hook = cfg.on_packet.clone();
            threads.push(spawn("snagster-stream-audio", move || audio_thread(sh, feed, enc, started, hook)).map_err(|e| e.to_string())?);
        }
        let mut net_threads = Vec::new();
        for (i, ep) in endpoints.into_iter().enumerate() {
            let sh = shared.clone();
            let net = NetCfg { ep, codec: cfg.codec, meta: meta.clone(), asc: asc.clone(), max_reconnects: cfg.max_reconnects, connect_timeout: cfg.connect_timeout, write_timeout: cfg.write_timeout, backoff: cfg.backoff };
            net_threads.push(spawn(&format!("snagster-stream-net-{i}"), move || net_thread(sh, i, net, started)).map_err(|e| e.to_string())?);
        }
        Ok(Streamer { shared, started, threads, net_threads, _timely: crate::video::TimelyActivity::begin(why) })
    }

    /// Where the stream stands; never waits on the stream's threads.
    pub fn stats(&self) -> Stats {
        let s = &self.shared;
        let live = s.live_since_ms.load(Ordering::Relaxed);
        let names = s.names.try_lock().map(|n| n.clone()).unwrap_or_default();
        let outputs: Vec<OutputStats> = s.outputs.iter().map(Output::stats).collect();
        let (mut state, bitrate_bps, fps, frames_sent, frames_dropped, bytes_sent, queue_ms, reconnects, out_error) = add_up(&outputs);
        let ending = s.ending.load(Ordering::Relaxed);
        if ending != 0 && state != State::Failed {
            state = State::from_u8(ending);
        }
        Stats {
            state,
            bitrate_bps,
            fps,
            frames_sent,
            frames_dropped,
            bytes_sent,
            queue_ms,
            uptime_s: if live == 0 { 0.0 } else { (self.started.elapsed().as_millis() as u64).saturating_sub(live - 1) as f64 / 1000.0 },
            reconnects,
            error: s.error.try_lock().ok().and_then(|e| e.clone()).or(out_error),
            notice: s.notice.try_lock().ok().and_then(|e| e.clone()),
            video_encoder: names.0,
            audio_encoder: names.1,
            timing: s.timing.snapshot(),
            outputs,
        }
    }

    /// Still streaming or trying to (not stopped, not failed).
    #[cfg(test)]
    pub fn is_running(&self) -> bool {
        !matches!(self.stats().state, State::Stopped | State::Failed)
    }

    /// Stop: the encoders are emptied, what is queued is sent (for up to a few seconds), the
    /// stream is unpublished and the connection closed. Returns the final numbers.
    pub fn stop(mut self) -> Stats {
        self.shutdown();
        self.stats()
    }

    fn shutdown(&mut self) {
        let sh = &self.shared;
        sh.ending.store(State::Stopping as u8, Ordering::Relaxed);
        for o in &sh.outputs {
            if o.state() != State::Failed {
                o.set_state(State::Stopping);
            }
        }
        sh.stop.store(true, Ordering::Relaxed);
        sh.each_queue(|_, _| {});
        // The network threads are last: they send what the encoders still hand over (all of
        // them at once, within one wait).
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        let t = Instant::now();
        while self.net_threads.iter().any(|n| !n.is_finished()) && t.elapsed() < STOP_WAIT {
            std::thread::sleep(Duration::from_millis(10));
        }
        sh.abort.store(true, Ordering::Relaxed);
        for n in self.net_threads.drain(..) {
            let _ = n.join();
        }
        for o in &sh.outputs {
            if o.state() != State::Failed {
                o.set_state(State::Stopped);
            }
        }
        sh.ending.store(State::Stopped as u8, Ordering::Relaxed);
    }
}

impl Drop for Streamer {
    fn drop(&mut self) {
        if !self.threads.is_empty() || !self.net_threads.is_empty() {
            self.shutdown();
        }
    }
}

/// How long stopping waits for the last packets to go out before cutting the connection.
const STOP_WAIT: Duration = Duration::from_secs(4);

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new().name(name.into()).spawn(f)
}

/// Hand a packet to the hook and to every service's queue (the last one gets it without a copy).
fn queue_packet(sh: &Shared, hook: &Option<PacketHook>, p: Packet) {
    if let Some(h) = hook {
        h(&p);
    }
    let mut p = Some(p);
    let n = sh.outputs.len();
    for (i, o) in sh.outputs.iter().enumerate() {
        let one = if i + 1 == n { p.take() } else { p.clone() };
        let Some(one) = one else { break };
        o.with_queue(|q| match one {
            Packet::Video(v) => {
                if !q.push_video(v) {
                    o.frames_dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
            Packet::Audio(a) => q.push_audio(a),
        });
    }
}

/// At most this many due frame slots are filled at once when the picture thread wakes late;
/// slots older than that are skipped (counted as late) rather than sent in a burst.
const CATCH_UP: u64 = 3;

/// What the picture thread does at `now_us` (since the stream started) with frame slots
/// `frame_us` apart and `next` the first slot not yet filled.
#[derive(Clone, Debug, PartialEq)]
struct SlotPlan {
    /// Not due yet: wait this many microseconds (and plan again).
    wait_us: Option<u64>,
    /// The slots to fill now with the newest picture.
    emit: std::ops::Range<u64>,
    /// Slots too old to send, skipped.
    skipped: u64,
}

fn plan_slots(now_us: u64, frame_us: f64, next: u64, catch_up: u64) -> SlotPlan {
    let due_us = |slot: u64| (slot as f64 * frame_us) as u64;
    if now_us < due_us(next) {
        return SlotPlan { wait_us: Some(due_us(next) - now_us), emit: next..next, skipped: 0 };
    }
    // The newest slot that is due, and up to `catch_up` slots ending with it.
    let newest = ((now_us as f64 / frame_us) as u64).max(next);
    let first = newest.saturating_sub(catch_up.max(1) - 1).max(next);
    SlotPlan { wait_us: None, emit: first..newest + 1, skipped: first - next }
}

/// How the picture thread replaces a video encoder that stops working.
struct Swap {
    cfg: VideoConfig,
    fallback: Option<FallbackFactory>,
}

/// At most this many encoder switches in one stream: past that the stream stops, rather than
/// trying encoders that keep failing for ever.
const MAX_SWITCHES: usize = 3;

impl Swap {
    /// Another encoder after `failed` stopped working with `why`: the new one and the note for
    /// the status line, or None when there is none (or too many have failed already).
    fn replace(&self, avoid: &mut Vec<String>, failed: &str, why: &str) -> Option<(Box<dyn VideoEncoder>, String)> {
        avoid.push(failed.to_string());
        if avoid.len() > MAX_SWITCHES {
            return None;
        }
        let e = (self.fallback.as_ref()?)(&self.cfg, avoid).ok()?;
        let note = crate::encode::switched_note(failed, why, &e.name().name);
        Some((e, note))
    }
}

fn video_thread(sh: Arc<Shared>, frames: FrameSource, mut enc: Box<dyn VideoEncoder>, swap: Swap, started: Instant, mut pacer: crate::video::Pacer, hook: Option<PacketHook>) {
    crate::video::timely_thread();
    let (w, h, fps) = (swap.cfg.width, swap.cfg.height, swap.cfg.fps);
    let mut avoid: Vec<String> = Vec::new();
    let mut pic = frames.picture(w, h);
    let frame_us = 1_000_000.0 / fps as f64;
    // The next frame slot to fill: slot n is due at `started + n frames` (fixed deadlines, so
    // nothing drifts however the waits go).
    let mut n: u64 = 0;
    'frames: while !sh.stop.load(Ordering::Relaxed) {
        let mut plan = plan_slots(started.elapsed().as_micros() as u64, frame_us, n, CATCH_UP);
        if let Some(wait) = plan.wait_us {
            // Wait for the slot, then fill it (also when told to stop meanwhile: the slot was
            // due, and its picture goes out with the rest).
            pacer.wait_until(Instant::now() + Duration::from_micros(wait));
            plan = plan_slots(started.elapsed().as_micros() as u64, frame_us, n, CATCH_UP);
            if plan.wait_us.is_some() {
                continue;
            }
        }
        sh.timing.late.fetch_add(plan.skipped, Ordering::Relaxed);
        let t = Instant::now();
        if !frames.take(&mut pic) {
            sh.stop.store(true, Ordering::Relaxed);
            break;
        }
        add_us(&sh.timing.fill_us, t);
        // The newest picture for every slot that is due (a slot missed is filled with a repeat,
        // so the service sees a steady frame rate).
        for slot in plan.emit.clone() {
            let pts = (slot as f64 * frame_us) as i64;
            #[cfg(zero_copy)]
            if matches!(pic, Picture::Surface(None)) {
                // No picture in graphics memory yet: nothing to send for this slot.
                continue;
            }
            let key = sh.want_key.swap(false, Ordering::Relaxed);
            let t = Instant::now();
            let r = match &pic {
                Picture::Memory(buf) => enc.encode(buf, pts, key),
                #[cfg(zero_copy)]
                Picture::Surface(Some(s)) => enc.encode_surface(s, pts, key),
                #[cfg(zero_copy)]
                Picture::Surface(None) => continue,
            };
            let took = add_us(&sh.timing.encode_us, t);
            sh.timing.encode_max_us.fetch_max(took, Ordering::Relaxed);
            sh.timing.frames_in.fetch_add(1, Ordering::Relaxed);
            sh.timing.encoder_dropped.store(enc.dropped(), Ordering::Relaxed);
            match r {
                Ok(ps) => {
                    sh.timing.packets_out.fetch_add(ps.len() as u64, Ordering::Relaxed);
                    ps.into_iter().for_each(|p| queue_packet(&sh, &hook, Packet::Video(p)))
                }
                Err(e) => {
                    // Another encoder takes over (from the next keyframe); the stream goes on.
                    let failed = enc.name().name;
                    if let Some((new, note)) = swap.replace(&mut avoid, &failed, &e) {
                        enc = new;
                        if let Ok(mut n) = sh.names.lock() {
                            n.0 = Some(enc.name());
                        }
                        if let Ok(mut n) = sh.notice.lock() {
                            *n = Some(note);
                        }
                        sh.want_key.store(true, Ordering::Relaxed);
                        continue;
                    }
                    sh.set_error(format!("Video encoder: {e}"));
                    sh.stop.store(true, Ordering::Relaxed);
                    break 'frames;
                }
            }
        }
        n = plan.emit.end;
    }
    if let Ok(ps) = enc.finish() {
        ps.into_iter().for_each(|p| queue_packet(&sh, &hook, Packet::Video(p)));
    }
    sh.each_queue(|_, q| q.ended = true);
}

fn audio_thread(sh: Arc<Shared>, mut feed: AudioFeed, mut enc: Box<dyn AudioEncoder>, started: Instant, hook: Option<PacketHook>) {
    let mut taken: u64 = 0;
    loop {
        let stopping = sh.stop.load(Ordering::Relaxed);
        let due = (started.elapsed().as_micros() as u64 * 48 / 1000).saturating_sub(taken) as usize;
        if due > 0 {
            let pcm = feed(due);
            let pts = (taken * 1_000_000 / 48_000) as i64;
            taken += due as u64;
            match enc.encode(&pcm, pts) {
                Ok(ps) => ps.into_iter().for_each(|p| queue_packet(&sh, &hook, Packet::Audio(p))),
                Err(e) => {
                    sh.set_error(format!("Sound encoder: {e}"));
                    break;
                }
            }
        }
        if stopping {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if let Ok(ps) = enc.finish() {
        ps.into_iter().for_each(|p| queue_packet(&sh, &hook, Packet::Audio(p)));
    }
}

struct NetCfg {
    ep: Endpoint,
    codec: Codec,
    meta: Meta,
    asc: Option<Vec<u8>>,
    max_reconnects: u32,
    connect_timeout: Duration,
    write_timeout: Duration,
    backoff: Duration,
}

/// Sleep up to `d`, waking early when the stream is stopped.
fn sleep_unless_stopped(sh: &Shared, d: Duration) {
    let end = Instant::now() + d;
    while !sh.stop.load(Ordering::Relaxed) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10).min(end - Instant::now()));
    }
}

/// Wait at most `d` for a packet.
fn next_packet(o: &Output, d: Duration) -> (Option<Packet>, bool, u32) {
    let Ok(mut g) = o.queue.lock() else { return (None, true, 0) };
    if g.as_ref().is_some_and(|q| q.len() == 0 && !q.ended) {
        match o.wake.wait_timeout(g, d) {
            Ok((g2, _)) => g = g2,
            Err(_) => return (None, true, 0),
        }
    }
    match g.as_mut() {
        Some(q) => {
            let p = q.pop();
            (p, q.ended && q.len() == 0, q.duration_ms())
        }
        None => (None, true, 0),
    }
}

const CLOCK_SLACK_US: i64 = 500_000;

/// One RTMP media message to send.
#[derive(Debug, PartialEq)]
struct Out {
    video: bool,
    /// A video frame (not a header): counted in the stats.
    frame: bool,
    ts: u32,
    body: Vec<u8>,
}

/// What one connection sends, kept apart from the socket so the tests can check it.
struct Session {
    codec: Codec,
    header_sent: bool,
    base_us: Option<i64>,
    last_ts: u32,
    params: ParamSets,
}

impl Session {
    fn new(codec: Codec, params: ParamSets) -> Session {
        Session { codec, header_sent: false, base_us: None, last_ts: 0, params }
    }

    /// Milliseconds on this connection's clock. It starts a little before the first packet
    /// (never below the stream's own zero), because sound and pictures reach the queue from
    /// different threads and a packet slightly older than the first one must not go negative.
    fn ts(&mut self, t_us: i64) -> u32 {
        let base = *self.base_us.get_or_insert((t_us - CLOCK_SLACK_US).max(0));
        let t = ((t_us - base).max(0) / 1000) as u32;
        self.last_ts = self.last_ts.max(t);
        t
    }

    /// The messages for one packet. Video before the first keyframe is skipped; the first
    /// keyframe brings the sequence header (from its own SPS/PPS, else the last ones seen).
    fn messages(&mut self, p: &Packet) -> Vec<Out> {
        match p {
            Packet::Audio(a) => vec![Out { video: false, frame: false, ts: self.ts(a.pts_us), body: flv::audio_frame(&a.data) }],
            Packet::Video(v) => {
                let mut out = Vec::new();
                // A keyframe with other parameter sets (another encoder took over): the player
                // needs the new decoder configuration before it.
                if self.header_sent && v.keyframe {
                    let found = ParamSets::find(self.codec, &v.data);
                    if found.complete(self.codec) && found != self.params {
                        self.header_sent = false;
                    }
                }
                if !self.header_sent {
                    if !v.keyframe {
                        return out;
                    }
                    let found = ParamSets::find(self.codec, &v.data);
                    if found.complete(self.codec) {
                        self.params = found;
                    }
                    match flv::video_sequence_header(self.codec, &self.params) {
                        Ok(h) => {
                            let ts = self.ts(v.dts_us);
                            out.push(Out { video: true, frame: false, ts, body: h });
                            self.header_sent = true;
                        }
                        Err(_) => return out,
                    }
                }
                let ts = self.ts(v.dts_us);
                let cts = ((v.pts_us - v.dts_us) / 1000) as i32;
                out.push(Out { video: true, frame: true, ts, body: flv::video_frame(self.codec, &v.data, v.keyframe, cts) });
                out
            }
        }
    }
}

/// A connection counts as working (and the reconnect count starts again) once it has carried
/// pictures for this long.
const HEALTHY_AFTER: Duration = Duration::from_secs(5);

fn connection_is_healthy(live_for: Duration, frames_sent: u64) -> bool {
    live_for >= HEALTHY_AFTER && frames_sent > 0
}

/// The wait before reconnect try number `tries` (from 0): `backoff`, doubling, at most 30 s.
fn backoff_wait(backoff: Duration, tries: u32) -> Duration {
    backoff.saturating_mul(1u32 << tries.min(5)).min(Duration::from_secs(30))
}

/// After a connection failed (or broke): false when giving up (stopping, or out of tries: the
/// state is then Failed); else waits the backoff and counts the try.
fn retry_after_failure(sh: &Shared, o: &Output, cfg: &NetCfg, tries: &mut u32) -> bool {
    if sh.stop.load(Ordering::Relaxed) {
        return false;
    }
    if *tries >= cfg.max_reconnects {
        o.set_state(State::Failed);
        // Nobody sends this queue any more: let it go.
        if let Ok(mut q) = o.queue.lock() {
            *q = None;
        }
        return false;
    }
    sleep_unless_stopped(sh, backoff_wait(cfg.backoff, *tries));
    *tries += 1;
    !sh.stop.load(Ordering::Relaxed)
}

/// One service's connection: connect, send its queue, reconnect when it drops, until the
/// stream stops or this service is given up on (the others carry on either way).
fn net_thread(sh: Arc<Shared>, idx: usize, cfg: NetCfg, started: Instant) {
    let o = &sh.outputs[idx];
    let mut tries: u32 = 0;
    let mut params = ParamSets::default();
    let mut first = true;
    let mut stop_deadline: Option<Instant> = None;
    'connect: loop {
        if sh.stop.load(Ordering::Relaxed) {
            break;
        }
        o.set_state(if first { State::Connecting } else { State::Reconnecting });
        let mut c = match Client::connect(&cfg.ep, cfg.connect_timeout) {
            Ok(c) => c,
            Err(e) => {
                o.set_error(e);
                if retry_after_failure(&sh, o, &cfg, &mut tries) {
                    continue;
                }
                break;
            }
        };
        if !first {
            o.reconnects.fetch_add(1, Ordering::Relaxed);
        }
        c.abort = Some(sh.abort.clone());
        c.stall = cfg.write_timeout;
        first = false;
        // `tries` is reset only once this connection has carried pictures for a while: a server
        // that takes the connection and then drops it at once must not be retried forever.
        if let Ok(mut e) = o.error.lock() {
            *e = None;
        }
        // Start this connection fresh: a keyframe soon, nothing stale.
        o.with_queue(|q| q.restart());
        sh.want_key.store(true, Ordering::Relaxed);
        let mut s = Session::new(cfg.codec, params.clone());
        let mut setup = c.send_metadata(flv::metadata(&cfg.meta));
        if let (Ok(()), Some(asc)) = (&setup, &cfg.asc) {
            setup = c.send_audio(0, flv::audio_sequence_header(asc));
        }
        if let Err(e) = setup {
            o.set_error(e);
            if retry_after_failure(&sh, o, &cfg, &mut tries) {
                continue;
            }
            break;
        }

        o.set_state(if sh.stop.load(Ordering::Relaxed) { State::Stopping } else { State::Live });
        let _ = sh.live_since_ms.compare_exchange(0, started.elapsed().as_millis() as u64 + 1, Ordering::Relaxed, Ordering::Relaxed);
        let mut last_poll = Instant::now();
        let mut window = (Instant::now(), c.sent, 0u64);
        let (connected, mut frames_here) = (Instant::now(), 0u64);
        loop {
            if sh.stop.load(Ordering::Relaxed) {
                let d = *stop_deadline.get_or_insert_with(|| Instant::now() + Duration::from_secs(3));
                if Instant::now() > d {
                    break;
                }
            }
            let (p, ended, queued) = next_packet(o, Duration::from_millis(20));
            o.queue_ms.store(queued, Ordering::Relaxed);
            let mut r = Ok(());
            if let Some(p) = &p {
                for m in s.messages(p) {
                    let t = Instant::now();
                    r = if m.video { c.send_video(m.ts, m.body) } else { c.send_audio(m.ts, m.body) };
                    add_us(&sh.timing.send_us, t);
                    sh.timing.sends.fetch_add(1, Ordering::Relaxed);
                    if r.is_err() {
                        break;
                    }
                    if m.frame {
                        o.frames_sent.fetch_add(1, Ordering::Relaxed);
                        window.2 += 1;
                        frames_here += 1;
                    }
                }
            }
            if tries > 0 && connection_is_healthy(connected.elapsed(), frames_here) {
                tries = 0;
            }
            if r.is_ok() && last_poll.elapsed() > Duration::from_millis(50) {
                last_poll = Instant::now();
                r = c.poll();
            }
            let dt = window.0.elapsed();
            if dt >= Duration::from_secs(1) {
                let bytes = c.sent - window.1;
                o.bitrate.store((bytes as f64 * 8.0 / dt.as_secs_f64()) as u64, Ordering::Relaxed);
                o.fps_x100.store((window.2 as f64 * 100.0 / dt.as_secs_f64()) as u32, Ordering::Relaxed);
                o.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
                window = (Instant::now(), c.sent, 0);
            }
            if let Err(e) = r {
                o.bytes_sent.fetch_add(c.sent - window.1, Ordering::Relaxed);
                o.set_error(e);
                params = s.params.clone();
                o.bitrate.store(0, Ordering::Relaxed);
                o.fps_x100.store(0, Ordering::Relaxed);
                if retry_after_failure(&sh, o, &cfg, &mut tries) {
                    continue 'connect;
                }
                break 'connect;
            }
            if ended {
                break;
            }
        }
        o.bytes_sent.fetch_add(c.sent - window.1, Ordering::Relaxed);
        if s.header_sent {
            let _ = c.send_video(s.last_ts, flv::video_end(cfg.codec));
        }
        c.close();
        break;
    }
    o.bitrate.store(0, Ordering::Relaxed);
    o.fps_x100.store(0, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::{AudioPacket, VideoPacket};
    use crate::stream::services::Service;

    fn key_au() -> Vec<u8> {
        vec![0, 0, 0, 1, 0x67, 0x42, 0xC0, 0x1F, 0, 0, 0, 1, 0x68, 0xCE, 0, 0, 0, 1, 0x65, 0xAA]
    }

    fn vid(ms: i64, key: bool) -> Packet {
        let data = if key { key_au() } else { vec![0, 0, 1, 0x41, 0xBB] };
        Packet::Video(VideoPacket { data, pts_us: ms * 1000 + 40_000, dts_us: ms * 1000, keyframe: key })
    }

    #[test]
    fn frame_slots_wait_emit_and_catch_up() {
        let f = 1_000_000.0 / 30.0;
        // Before slot 1 is due: wait for it.
        assert_eq!(plan_slots(10_000, f, 1, 3), SlotPlan { wait_us: Some(23_333), emit: 1..1, skipped: 0 });
        // On time: just that slot.
        assert_eq!(plan_slots(33_400, f, 1, 3), SlotPlan { wait_us: None, emit: 1..2, skipped: 0 });
        // Woke 2 slots late: slots 1, 2 and 3 all get the newest picture.
        assert_eq!(plan_slots(100_100, f, 1, 3), SlotPlan { wait_us: None, emit: 1..4, skipped: 0 });
        // Woke 10 slots late: the last 3 are sent, the rest are skipped.
        assert_eq!(plan_slots(366_700, f, 1, 3), SlotPlan { wait_us: None, emit: 9..12, skipped: 8 });
        // The first slot at the start.
        assert_eq!(plan_slots(0, f, 0, 3), SlotPlan { wait_us: None, emit: 0..1, skipped: 0 });
    }

    /// A machine whose sleeps all overrun by 30 ms (the CI Mac): with fixed deadlines and
    /// catch-up, every slot still goes out, even with no allowance in the wait at all.
    #[test]
    fn a_coarse_timer_still_gets_every_frame_out() {
        let f = 1_000_000.0 / 30.0;
        let (mut now, mut next, mut sent, mut skipped) = (0u64, 0u64, 0u64, 0u64);
        while now < 3_000_000 {
            let p = plan_slots(now, f, next, CATCH_UP);
            match p.wait_us {
                // A plain sleep that runs 30 ms long, then the encode (3 ms).
                Some(w) => now += w + 30_000,
                None => {
                    sent += p.emit.end - p.emit.start;
                    skipped += p.skipped;
                    next = p.emit.end;
                    now += 3_000 * (p.emit.end - p.emit.start);
                }
            }
        }
        let due = (3_000_000.0 / f) as u64;
        assert!(sent as f64 >= 0.9 * due as f64, "{sent} of {due} sent, {skipped} skipped");
        assert_eq!(skipped, 0);
        // Timestamps stay on the slots: none twice, none missing.
        assert_eq!(next, sent);
    }

    #[test]
    fn timing_between_two_snapshots_and_its_summary() {
        let a = Timing { frames_in: 10, packets_out: 9, encoder_dropped: 1, late: 2, fill_us: 1000, encode_us: 20_000, encode_max_us: 5000, sends: 10, send_us: 500 };
        let b = Timing { frames_in: 70, packets_out: 66, encoder_dropped: 3, late: 2, fill_us: 7000, encode_us: 140_000, encode_max_us: 9000, sends: 130, send_us: 3500 };
        let d = b.since(&a);
        assert_eq!(d, Timing { frames_in: 60, packets_out: 57, encoder_dropped: 2, late: 0, fill_us: 6000, encode_us: 120_000, encode_max_us: 9000, sends: 120, send_us: 3000 });
        assert_eq!(
            d.summary(2.0),
            "encoder took 30.0 pictures a second and gave back 28.5 (2 thrown away by it, 0 frame slots late); per picture: taking it 0.10 ms, encoding 2.00 ms (longest 9.0 ms); per message sent 0.03 ms"
        );
        // Nothing happened (and counters that went backwards don't wrap round).
        let z = a.since(&b);
        assert_eq!((z.frames_in, z.send_us), (0, 0));
        assert!(Timing::default().summary(0.0).contains("encoding 0.00 ms"));
    }

    #[test]
    fn a_session_skips_video_until_a_keyframe_and_then_sends_the_header() {
        let mut s = Session::new(Codec::H264, ParamSets::default());
        assert!(s.messages(&vid(1000, false)).is_empty());
        let a = s.messages(&Packet::Audio(AudioPacket { data: vec![1], pts_us: 1_010_000 }));
        assert_eq!(a, vec![Out { video: false, frame: false, ts: 500, body: vec![0xAF, 1, 1] }]);
        let k = s.messages(&vid(1033, true));
        assert_eq!(k.len(), 2);
        assert!(!k[0].frame && k[1].frame);
        assert_eq!(&k[0].body[..2], &[0x17, 0]);
        assert_eq!((k[0].ts, k[1].ts), (523, 523));
        // Composition time 40 ms.
        assert_eq!(&k[1].body[..5], &[0x17, 1, 0, 0, 40]);
        let p = s.messages(&vid(1066, false));
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].ts, 556);
        assert_eq!(s.last_ts, 556);
    }

    /// After another encoder took over, its first keyframe has other parameter sets: the
    /// player gets the new decoder configuration first. The same ones again send no header.
    #[test]
    fn a_keyframe_with_new_parameter_sets_brings_a_new_header() {
        let mut s = Session::new(Codec::H264, ParamSets::default());
        assert_eq!(s.messages(&vid(0, true)).len(), 2, "header and frame");
        assert_eq!(s.messages(&vid(33, true)).len(), 1, "same parameter sets: just the frame");
        let other = vec![0, 0, 0, 1, 0x67, 0x64, 0x00, 0x28, 0, 0, 0, 1, 0x68, 0xEE, 0, 0, 0, 1, 0x65, 0xAB];
        let m = s.messages(&Packet::Video(VideoPacket { data: other, pts_us: 66_000, dts_us: 66_000, keyframe: true }));
        assert_eq!(m.len(), 2, "a new header, then the frame");
        assert_eq!(&m[0].body[..2], &[0x17, 0]);
        assert_eq!(&m[0].body[5..9], &[1, 0x64, 0x00, 0x28], "the new SPS's profile and level");
        assert_eq!(s.messages(&vid(99, false)).len(), 1);
    }

    #[test]
    fn a_keyframe_without_parameter_sets_uses_the_last_ones() {
        let known = ParamSets::find(Codec::H264, &key_au());
        let mut s = Session::new(Codec::H264, known);
        let bare = Packet::Video(VideoPacket { data: vec![0, 0, 1, 0x65, 1], pts_us: 0, dts_us: 0, keyframe: true });
        let m = s.messages(&bare);
        assert_eq!(m.len(), 2);
        let mut none = Session::new(Codec::H264, ParamSets::default());
        assert!(none.messages(&bare).is_empty(), "no header can be made, so nothing is sent");
    }

    #[test]
    fn service_settings_respect_the_services_limits() {
        let t = StreamTarget { service: Service::Twitch, url: String::new(), key: "k".into() };
        let c = StreamConfig::for_target(t, 3840, 2160, 144, Codec::Hevc);
        assert_eq!((c.width, c.height, c.fps), (1920, 1080, 60));
        assert_eq!(c.codec, Codec::H264, "Twitch takes no HEVC over RTMP");
        assert!(c.video_kbps <= 6000);
        assert_eq!(c.keyframe_s, 2);
        let t = StreamTarget { service: Service::YouTube, url: String::new(), key: "k".into() };
        let c = StreamConfig::for_target(t, 1281, 721, 30, Codec::Hevc);
        assert_eq!((c.width, c.height, c.codec), (1280, 720, Codec::Hevc));
        assert!(!format!("{c:?}").contains("\"k\""));
        assert_eq!(c.video_kbps, 6000, "YouTube's 720p HEVC row");
        // YouTube: 4K60, 50 Mbit/s H.264 or 35 HEVC; 120 fps comes down to 60.
        let yt = |w, h, fps, codec| StreamConfig::for_target(StreamTarget { service: Service::YouTube, url: String::new(), key: "k".into() }, w, h, fps, codec);
        let c = yt(3840, 2160, 60, Codec::H264);
        assert_eq!((c.width, c.height, c.fps, c.video_kbps), (3840, 2160, 60, 50_000));
        assert_eq!(yt(3840, 2160, 120, Codec::Hevc).fps, 60);
        assert_eq!(yt(3840, 2160, 60, Codec::Hevc).video_kbps, 35_000);
        assert_eq!(yt(1920, 1080, 120, Codec::H264).fps, 60);
        // A custom server: 1080p and 1440p at 120 fps, 4K at 60.
        let cu = |w, h, fps| StreamConfig::for_target(StreamTarget { service: Service::Custom, url: "rtmp://h/app".into(), key: "k".into() }, w, h, fps, Codec::H264);
        assert_eq!((cu(1920, 1080, 120).fps, cu(1920, 1080, 120).video_kbps), (120, 24_000));
        assert_eq!(cu(2560, 1440, 120).fps, 120);
        let c = cu(3840, 2160, 120);
        assert_eq!((c.width, c.height, c.fps), (3840, 2160, 60), "4K caps at 60 fps (the pixel rate)");
        assert_eq!(cu(7680, 4320, 30).height, 2160);
        assert_eq!(recommended_kbps(1920, 1080, 30), 6220);
        assert_eq!(recommended_kbps(16, 16, 1), 500);
    }

    #[test]
    fn the_first_connection_keeps_the_streams_own_clock() {
        let mut s = Session::new(Codec::H264, ParamSets::default());
        assert_eq!(s.ts(40_000), 40);
        assert_eq!(s.ts(0), 0);
    }

    /// Needs this computer's real encoders: `cargo test -- --ignored real_encoders`.
    #[test]
    #[ignore]
    fn real_encoders_open_for_streaming() {
        let e = Encoders::system();
        let cfg = VideoConfig { width: 640, height: 360, fps: 30, bitrate_bps: 1_500_000, keyframe_every_s: 2, codec: Codec::H264, prefer_hardware: true, low_latency: true };
        let mut v = (e.video)(&cfg).unwrap();
        let mut packets = Vec::new();
        for i in 0..10 {
            packets.extend(v.encode(&vec![100u8; 640 * 360 * 4], i * 33_333, i == 0).unwrap());
        }
        packets.extend(v.finish().unwrap());
        let first = packets.iter().find(|p| p.keyframe).expect("a keyframe");
        assert!(ParamSets::find(Codec::H264, &first.data).complete(Codec::H264));
        if let Ok(a) = (e.audio)(128_000) {
            assert_eq!(a.audio_specific_config().len(), 2);
        }
    }

    #[test]
    fn reconnects_wait_longer_each_time_and_count_again_only_after_pictures_flowed() {
        let b = Duration::from_millis(100);
        assert_eq!(backoff_wait(b, 0), b);
        assert_eq!(backoff_wait(b, 3), Duration::from_millis(800));
        assert_eq!(backoff_wait(Duration::from_secs(5), 4), Duration::from_secs(30), "at most 30 s");
        assert_eq!(backoff_wait(b, 40), Duration::from_millis(3200), "no overflow on many tries");
        assert!(!connection_is_healthy(Duration::from_secs(60), 0), "no pictures, not working");
        assert!(!connection_is_healthy(Duration::from_millis(200), 50), "dropped at once, not working");
        assert!(connection_is_healthy(HEALTHY_AFTER, 1));
    }

    #[test]
    fn states_round_trip_through_their_numbers() {
        for s in [State::Connecting, State::Live, State::Reconnecting, State::Stopping, State::Stopped, State::Failed] {
            assert_eq!(State::from_u8(s as u8), s);
        }
        assert_eq!(State::from_u8(200), State::Failed);
    }
}
