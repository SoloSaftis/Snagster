//! Going live from the window or from an AI app: the settings turned into a stream (size,
//! bitrate, source, sound, a copy on disk), and the one stream this Snagster runs at a time.
//!
//! Starting and stopping happen on their own threads (`begin`, `end`): the window asks
//! `snapshot` every frame and never waits on the encoders or the network.

use super::engine::{Encoders, FrameSource, SoundSource, Stats, StreamConfig, Streamer};
use super::flv::Meta;
use super::flvfile::{Copy, Recorder};
use super::services::{StreamTarget, mask};
use super::source::{Grabber, Source, studio_frames};
use crate::encode::Codec;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Everything one stream needs.
#[derive(Clone, Debug)]
pub struct LiveOptions {
    pub target: StreamTarget,
    pub source: Source,
    /// The tallest picture (0: the source's size).
    pub max_height: u32,
    pub fps: u32,
    /// kbit/s; 0: recommended.
    pub kbps: u32,
    pub hardware: bool,
    /// Send HEVC where the service takes it (Enhanced RTMP) and this computer can encode it.
    pub hevc: bool,
    pub keyframe_s: u32,
    pub audio: crate::audio::AudioSource,
    pub mic: String,
    pub cursor: bool,
    /// Write what is streamed to this FLV file too.
    pub copy_to: Option<PathBuf>,
    pub max_reconnects: u32,
}

impl LiveOptions {
    /// The options the settings give, with this stream key.
    pub fn from_settings(s: &crate::settings::Settings, key: &str) -> LiveOptions {
        let st = s.stream.clamped();
        LiveOptions {
            target: StreamTarget { service: st.service, url: st.url.clone(), key: key.trim().to_string() },
            source: st.source.clone(),
            max_height: st.max_height,
            fps: st.fps,
            kbps: st.kbps,
            hardware: st.hardware,
            hevc: st.hevc,
            keyframe_s: st.keyframe_s,
            audio: if crate::audio::AudioSource::supported() { st.audio } else { crate::audio::AudioSource::None },
            mic: s.audio_mic.clone(),
            cursor: st.cursor,
            copy_to: st.record_copy.then(|| crate::video::recording_path(&s.video_folder, "Stream", "flv")),
            max_reconnects: st.reconnects,
        }
    }
}

/// Where Studio's scene comes from.
pub enum StudioFrames {
    /// The Studio window's compositor (its canvas size).
    Running(Arc<crate::studio::Shared>, u32, u32),
    /// No Studio window: draw the current scene here.
    Start(crate::studio::StudioSettings),
}

/// The video codec to stream: HEVC only when the person asked for it, the service takes it
/// (Enhanced RTMP: YouTube, custom servers) and this system has an HEVC encoder; else H.264,
/// which every service takes.
pub fn pick_codec(want_hevc: bool, service_takes_hevc: bool, hevc_encoder: bool) -> Codec {
    if want_hevc && service_takes_hevc && hevc_encoder { Codec::Hevc } else { Codec::H264 }
}

/// Whether this system has an HEVC encoder at all (Linux has none yet).
fn hevc_encoder_here(hardware: bool) -> bool {
    !crate::encode::plan(crate::encode::Os::current(), Codec::Hevc, hardware).is_empty()
}

/// The stream's size and rate for a source of `sw` × `sh` (pure, so it can be tested).
pub fn plan(o: &LiveOptions, sw: u32, sh: u32) -> StreamConfig {
    let (mut w, mut h) = (sw.max(2), sh.max(2));
    if o.max_height > 0 && h > o.max_height {
        w = ((w as u64 * o.max_height as u64) / h as u64).max(2) as u32;
        h = o.max_height;
    }
    let p = o.target.service.preset();
    let codec = pick_codec(o.hevc, p.hevc, hevc_encoder_here(o.hardware));
    let mut c = StreamConfig::for_target(o.target.clone(), w, h, o.fps, codec);
    if o.kbps > 0 {
        c.video_kbps = o.kbps.clamp(300, p.max_video_kbps);
    }
    c.hardware = o.hardware;
    c.keyframe_s = o.keyframe_s.clamp(1, 4);
    c.max_reconnects = o.max_reconnects;
    c
}

/// What is going out, for the window and AI apps (no secret in it).
#[derive(Clone, Debug, PartialEq)]
pub struct Info {
    pub service: &'static str,
    /// The server address with the key masked.
    pub address: String,
    pub source: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub kbps: u32,
    pub copy: Option<PathBuf>,
    /// Something to know, e.g. going out without sound.
    pub notice: Option<String>,
}

/// A running stream with its capture, compositor and copy.
pub struct Live {
    streamer: Streamer,
    // Dropped after the streamer, so the picture thread never outlives its source.
    _grabber: Option<Grabber>,
    _engine: Option<crate::studio::Engine>,
    copy: Option<Recorder>,
    pub info: Info,
    key: String,
}

/// How a stream ended.
#[derive(Clone, Debug, PartialEq)]
pub struct Ended {
    pub info: Info,
    pub stats: Stats,
    pub copy: Option<Result<Copy, String>>,
}

impl Live {
    pub fn start(o: LiveOptions, studio: Option<StudioFrames>, encoders: Encoders) -> Result<Live, String> {
        let key = o.target.key.clone();
        let masked = |e: String| mask(&e, &key);
        o.target.endpoint().map_err(masked)?;
        let mut grabber = None;
        let mut engine = None;
        let mut notice = None;
        let (rect, title) = match &o.source {
            Source::Screen => (Some(crate::capture::monitor_under_cursor()?), None),
            Source::Window(t) if t.trim().is_empty() => return Err("Pick the window to stream first.".into()),
            Source::Window(t) => {
                let low = t.to_lowercase();
                let w = crate::capture::list_windows().into_iter().find(|w| w.label.to_lowercase().contains(&low)).ok_or_else(|| format!("There's no window called \"{t}\" now."))?;
                (Some((w.x, w.y, w.width, w.height)), Some(t.clone()))
            }
            Source::Area(_, _, w, h) if *w < 16 || *h < 16 => return Err("Pick the area to stream first.".into()),
            Source::Area(x, y, w, h) => (Some((*x, *y, *w, *h)), None),
            Source::Studio => (None, None),
        };
        let (frames, cfg): (FrameSource, StreamConfig) = match rect {
            Some(r) => {
                let cfg = plan(&o, r.2, r.3);
                let g = Grabber::start(r, title, cfg.width, cfg.height, cfg.fps, o.cursor);
                // The first picture, so the stream doesn't start black.
                let t = Instant::now();
                while !g.ready() && t.elapsed() < Duration::from_secs(2) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                let f = g.frames();
                grabber = Some(g);
                (f, cfg)
            }
            None => {
                let (sh, w, h) = match studio {
                    Some(StudioFrames::Running(sh, w, h)) => (sh, w, h),
                    Some(StudioFrames::Start(s)) => start_engine(&s, &mut engine),
                    None => start_engine(&crate::settings::Settings::load(false).studio, &mut engine),
                };
                let cfg = plan(&o, w, h);
                if let Ok(mut f) = sh.recording_fps.lock()
                    && f.is_none_or(|v| v < cfg.fps)
                {
                    *f = Some(cfg.fps);
                }
                (studio_frames(sh, cfg.width, cfg.height), cfg)
            }
        };
        let mut cfg = cfg;
        // A computer that has no HEVC encoder after all streams H.264 rather than not at all.
        if cfg.codec == Codec::Hevc && super::engine::video_config(&cfg).and_then(|v| (encoders.video)(&v)).is_err() {
            cfg.codec = Codec::H264;
            notice = Some("This computer couldn't start an HEVC encoder, so the stream is H.264.".to_string());
        }
        let (capture, sound_notice) = sound_for(o.audio, || crate::audio::Capture::start(o.audio, &o.mic));
        let sound = match capture {
            Some(c) => SoundSource::Capture(c),
            None => SoundSource::Silence,
        };
        notice = match (notice, sound_notice) {
            (Some(a), Some(b)) => Some(format!("{a} {b}")),
            (a, b) => a.or(b),
        };
        let has_sound = !matches!(sound, SoundSource::None);
        let copy = match &o.copy_to {
            Some(p) => {
                let meta = Meta { width: cfg.width, height: cfg.height, fps: cfg.fps, video_kbps: cfg.video_kbps, codec: cfg.codec, audio_kbps: has_sound.then_some(cfg.audio_kbps) };
                let r = Recorder::start(p, meta, has_sound.then(|| crate::encode::aac_lc_config(48_000, 2)))?;
                cfg.on_packet = Some(r.hook());
                Some(r)
            }
            None => None,
        };
        let info = Info {
            service: o.target.service.preset().name,
            address: o.target.display(),
            source: o.source.label().to_string(),
            width: cfg.width,
            height: cfg.height,
            fps: cfg.fps,
            kbps: cfg.video_kbps,
            copy: o.copy_to.clone(),
            notice,
        };
        let streamer = Streamer::start(cfg, frames, sound, encoders).map_err(masked)?;
        Ok(Live { streamer, _grabber: grabber, _engine: engine, copy, info, key })
    }

    /// Never waits.
    pub fn stats(&self) -> Stats {
        let mut s = self.streamer.stats();
        s.error = s.error.map(|e| mask(&e, &self.key));
        if s.notice.is_none() {
            s.notice = self.info.notice.clone();
        }
        s
    }

    /// Stop and wait for the end (a few seconds at most): run it off the window's thread.
    pub fn stop(self) -> Ended {
        let Live { streamer, _grabber, _engine, copy, info, key } = self;
        let mut stats = streamer.stop();
        stats.error = stats.error.map(|e| mask(&e, &key));
        if stats.notice.is_none() {
            stats.notice = info.notice.clone();
        }
        drop(_grabber);
        drop(_engine);
        let copy = copy.map(Recorder::finish);
        Ended { info, stats, copy }
    }
}

/// Where a stream's sound comes from: the chosen capture, else a silent track. Services expect
/// an audio track (some warn without one), so a stream without sound chosen, on a system
/// without sound capture yet (Linux, macOS), or whose sound couldn't start sends silence, which
/// the AAC encoder makes for next to nothing. The note says why there is no sound.
fn sound_for<C>(audio: crate::audio::AudioSource, start: impl FnOnce() -> Result<C, String>) -> (Option<C>, Option<String>) {
    if audio == crate::audio::AudioSource::None {
        return (None, None);
    }
    match start() {
        Ok(c) => (Some(c), None),
        Err(e) => (None, Some(format!("Streaming without sound (a silent audio track): {e}"))),
    }
}

fn start_engine(s: &crate::studio::StudioSettings, slot: &mut Option<crate::studio::Engine>) -> (Arc<crate::studio::Shared>, u32, u32) {
    let scene = s.scene().cloned().unwrap_or_else(|| crate::studio::StudioSettings::default().scenes[0].clone());
    s.apply_mix();
    let e = crate::studio::Engine::start(scene, s.width, s.height, None);
    let t = Instant::now();
    while e.latest().is_none() && t.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let (w, h) = e.latest().map(|f| f.dimensions()).unwrap_or((s.width.max(16), s.height.max(16)));
    let sh = e.shared.clone();
    *slot = Some(e);
    (sh, w, h)
}

// ---- the one stream of this Snagster ----

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Idle,
    /// Opening the capture and the encoders.
    Starting,
    /// Streaming (see `Stats::state` for connecting, live or reconnecting).
    Running,
    /// Sending the last packets and closing.
    Stopping,
}

enum Slot {
    Idle,
    Starting(Arc<AtomicBool>),
    Running(Box<Live>),
    Stopping(Info),
}

struct Global {
    slot: Slot,
    /// Why the last start failed (masked).
    error: Option<String>,
    last: Option<Ended>,
}

static GLOBAL: Mutex<Global> = Mutex::new(Global { slot: Slot::Idle, error: None, last: None });

fn global() -> MutexGuard<'static, Global> {
    GLOBAL.lock().unwrap_or_else(|p| p.into_inner())
}

/// What the window and AI apps see.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub phase: Phase,
    pub stats: Option<Stats>,
    pub info: Option<Info>,
    /// Why the last start failed.
    pub error: Option<String>,
    /// The last stream that ended.
    pub last: Option<Ended>,
}

impl Snapshot {
    /// On the air or about to be (the red LIVE badge).
    pub fn on_air(&self) -> bool {
        matches!(self.phase, Phase::Starting | Phase::Running)
    }

    pub fn live(&self) -> bool {
        self.phase == Phase::Running && self.stats.as_ref().is_some_and(|s| s.state == super::State::Live)
    }
}

pub fn snapshot() -> Snapshot {
    let g = global();
    let (phase, stats, info) = match &g.slot {
        Slot::Idle => (Phase::Idle, None, None),
        Slot::Starting(_) => (Phase::Starting, None, None),
        Slot::Running(l) => (Phase::Running, Some(l.stats()), Some(l.info.clone())),
        Slot::Stopping(i) => (Phase::Stopping, None, Some(i.clone())),
    };
    Snapshot { phase, stats, info, error: g.error.clone(), last: g.last.clone() }
}

/// Start streaming in the background. Errors only when a stream is already running; what
/// goes wrong later shows in `snapshot().error` (or the stats).
pub fn begin(o: LiveOptions, studio: Option<StudioFrames>, encoders: Encoders) -> Result<(), String> {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut g = global();
        if !matches!(g.slot, Slot::Idle) {
            return Err("A stream is already running: stop it first.".into());
        }
        g.slot = Slot::Starting(cancel.clone());
        g.error = None;
    }
    let spawned = std::thread::Builder::new().name("snagster-stream-start".into()).spawn(move || {
        let r = Live::start(o, studio, encoders);
        let mut g = global();
        match r {
            Ok(l) if cancel.load(Ordering::Relaxed) => {
                g.slot = Slot::Stopping(l.info.clone());
                drop(g);
                let ended = l.stop();
                let mut g = global();
                g.slot = Slot::Idle;
                g.last = Some(ended);
            }
            Ok(l) => g.slot = Slot::Running(Box::new(l)),
            Err(e) => {
                g.slot = Slot::Idle;
                g.error = Some(e);
            }
        }
    });
    if let Err(e) = spawned {
        let mut g = global();
        g.slot = Slot::Idle;
        return Err(e.to_string());
    }
    Ok(())
}

/// Stop at once: the stream is closed in the background. False when nothing was running.
pub fn end() -> bool {
    let mut g = global();
    match std::mem::replace(&mut g.slot, Slot::Idle) {
        Slot::Idle => false,
        Slot::Starting(c) => {
            c.store(true, Ordering::Relaxed);
            g.slot = Slot::Starting(c);
            true
        }
        Slot::Stopping(i) => {
            g.slot = Slot::Stopping(i);
            true
        }
        Slot::Running(l) => {
            g.slot = Slot::Stopping(l.info.clone());
            drop(g);
            let _ = std::thread::Builder::new().name("snagster-stream-stop".into()).spawn(move || {
                let ended = l.stop();
                let mut g = global();
                g.slot = Slot::Idle;
                g.last = Some(ended);
            });
            true
        }
    }
}

/// Wait until nothing runs (for AI apps and tests), at most `d`.
pub fn wait_idle(d: Duration) -> bool {
    let t = Instant::now();
    while t.elapsed() < d {
        if snapshot().phase == Phase::Idle {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// Tests that use the one stream take this first.
#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::Service;
    use crate::stream::testserver::TestServer;

    fn opts(url: &str, key: &str) -> LiveOptions {
        LiveOptions {
            target: StreamTarget { service: Service::Custom, url: url.into(), key: key.into() },
            source: Source::Studio,
            max_height: 0,
            fps: 30,
            kbps: 0,
            hardware: false,
            hevc: false,
            keyframe_s: 2,
            audio: crate::audio::AudioSource::None,
            mic: String::new(),
            cursor: false,
            copy_to: None,
            max_reconnects: 0,
        }
    }

    #[test]
    fn hevc_only_when_asked_taken_and_encodable() {
        assert_eq!(pick_codec(true, true, true), Codec::Hevc);
        assert_eq!(pick_codec(false, true, true), Codec::H264, "not asked for");
        assert_eq!(pick_codec(true, false, true), Codec::H264, "the service takes only H.264 (Twitch)");
        assert_eq!(pick_codec(true, true, false), Codec::H264, "no HEVC encoder here (Linux)");
        // Before, the plan always asked for H.264.
        let mut o = opts("rtmp://h/live", "k");
        o.hevc = true;
        let here = hevc_encoder_here(o.hardware);
        assert_eq!(plan(&o, 1280, 720).codec, if here { Codec::Hevc } else { Codec::H264 }, "a custom server takes HEVC");
        o.target.service = Service::YouTube;
        assert_eq!(plan(&o, 1280, 720).codec, if here { Codec::Hevc } else { Codec::H264 });
        o.target.service = Service::Twitch;
        assert_eq!(plan(&o, 1280, 720).codec, Codec::H264);
        o.target.service = Service::Custom;
        o.hevc = false;
        assert_eq!(plan(&o, 1280, 720).codec, Codec::H264);
        assert_eq!(hevc_encoder_here(true), !cfg!(all(unix, not(target_os = "macos"))), "Windows and macOS have HEVC encoders, Linux not yet");
    }

    #[test]
    fn a_stream_without_sound_gets_a_silent_track() {
        use crate::audio::AudioSource;
        let (c, n) = sound_for::<u8>(AudioSource::None, || panic!("nothing is started when no sound is chosen"));
        assert_eq!((c, n), (None, None));
        let (c, n) = sound_for::<u8>(AudioSource::System, || Err("no speakers".into()));
        assert_eq!(c, None);
        assert_eq!(n.as_deref(), Some("Streaming without sound (a silent audio track): no speakers"));
        let (c, n) = sound_for(AudioSource::Both, || Ok(7u8));
        assert_eq!((c, n), (Some(7), None));
    }

    #[test]
    fn the_plan_scales_down_to_the_height_and_the_services_limits() {
        let mut o = opts("rtmp://h/live", "k");
        o.max_height = 720;
        let c = plan(&o, 2560, 1440);
        assert_eq!((c.width, c.height), (1280, 720));
        o.max_height = 0;
        o.target.service = Service::Twitch;
        o.fps = 120;
        o.kbps = 99_000;
        let c = plan(&o, 3840, 2160);
        assert_eq!((c.width, c.height, c.fps, c.video_kbps), (1920, 1080, 60, 6000));
        assert_eq!(c.keyframe_s, 2);
        assert!(!c.hardware);
        // A small odd source stays its size, made even.
        let c = plan(&opts("rtmp://h/live", "k"), 641, 481);
        assert_eq!((c.width, c.height), (640, 480));
        assert_eq!(c.video_kbps, crate::stream::engine::recommended_kbps(640, 480, 30));
    }

    #[test]
    fn options_come_from_the_settings_without_the_key_in_them() {
        let mut s = crate::settings::Settings::default();
        s.stream.service = Service::YouTube;
        s.stream.fps = 500;
        s.stream.record_copy = true;
        let o = LiveOptions::from_settings(&s, " abc-123 ");
        assert_eq!(o.target.key, "abc-123");
        assert_eq!(o.fps, 60, "clamped to the service");
        assert!(o.copy_to.as_ref().is_some_and(|p| p.extension().is_some_and(|e| e == "flv")));
        assert!(!format!("{o:?}").contains("abc-123"), "the key never shows in Debug output");
    }

    #[test]
    fn bad_sources_and_addresses_fail_at_once_without_the_key() {
        let e = Live::start(opts("ftp://h/sk_live_777", "sk_live_777"), None, crate::stream::engine::Encoders::system()).err().unwrap();
        assert!(!e.contains("sk_live_777"), "{e}");
        let mut o = opts("rtmp://127.0.0.1:1/live", "k");
        o.source = Source::Window(String::new());
        assert!(Live::start(o.clone(), None, crate::stream::engine::Encoders::system()).err().unwrap().contains("window"));
        o.source = Source::Area(0, 0, 0, 0);
        assert!(Live::start(o, None, crate::stream::engine::Encoders::system()).err().unwrap().contains("area"));
    }

    fn green_studio() -> crate::studio::StudioSettings {
        let mut s = crate::studio::StudioSettings { width: 64, height: 64, ..Default::default() };
        s.scenes = vec![crate::studio::Scene {
            name: "t".into(),
            items: vec![crate::studio::Item { name: "c".into(), kind: crate::studio::SourceKind::Color { color: [0, 200, 0, 255] }, x: 0.0, y: 0.0, w: 64.0, h: 64.0, visible: true }],
        }];
        s.current = 0;
        s
    }

    #[test]
    fn going_live_streams_and_records_a_copy_then_stops_in_the_background() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let srv = TestServer::start().unwrap();
        let path = std::env::temp_dir().join(format!("snagster-live-test-{}.flv", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut o = opts(&srv.url(), "my_secret_key_1");
        o.copy_to = Some(path.clone());
        assert_eq!(snapshot().phase, Phase::Idle);
        begin(o.clone(), Some(StudioFrames::Start(green_studio())), crate::stream::tests::fakes(30, 0)).unwrap();
        assert!(begin(o, None, crate::stream::tests::fakes(30, 0)).is_err(), "one stream at a time");
        let t = Instant::now();
        while srv.video_messages() < 15 && t.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let s = snapshot();
        assert!(s.live(), "{s:?}");
        assert!(s.on_air());
        assert_eq!(srv.publish_name(), "my_secret_key_1");
        let info = s.info.unwrap();
        assert_eq!((info.width, info.height, info.service), (64, 64, "Custom server"));
        assert!(!info.address.contains("my_secret_key_1"));
        // Stopping returns at once; the stream closes behind.
        let t = Instant::now();
        assert!(end());
        assert!(t.elapsed() < Duration::from_millis(200), "{:?}", t.elapsed());
        assert!(matches!(snapshot().phase, Phase::Stopping | Phase::Idle));
        assert!(wait_idle(Duration::from_secs(15)));
        assert!(!end(), "nothing left to stop");
        let last = snapshot().last.unwrap();
        assert_eq!(last.stats.state, crate::stream::State::Stopped);
        assert!(last.stats.frames_sent >= 15);
        let copy = last.copy.unwrap().unwrap();
        assert!(copy.video_frames >= 15, "{copy:?}");
        let tags = crate::stream::flvfile::tests::tags(&std::fs::read(&path).unwrap());
        assert_eq!(&tags[1].2[..2], &[0x17, 0], "the copy starts with the decoder configuration");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_failed_start_is_reported_masked() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut o = opts("rtmp://127.0.0.1:1/live", "my_secret_key_2");
        o.source = Source::Area(0, 0, 4, 4);
        begin(o, None, crate::stream::tests::fakes(30, 0)).unwrap();
        assert!(wait_idle(Duration::from_secs(10)));
        let e = snapshot().error.unwrap();
        assert!(e.contains("area") && !e.contains("my_secret_key_2"), "{e}");
    }
}
