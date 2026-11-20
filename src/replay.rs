//! The replay buffer: while it is on, the screen is encoded all the time (the graphics chip's
//! encoder where there is one, the same capture and encoder path live streaming uses) and the
//! last 30-120 seconds are kept in memory, so a moment nobody planned to record can still be
//! saved as an MP4 afterwards.
//!
//! What is kept is a ring of keyframe groups (GOPs): the encoder makes a keyframe every 2 s
//! and every keyframe carries its parameter sets, so each group plays on its own and the ring
//! is only ever cut between groups. The oldest group goes as soon as the ones after it still
//! cover the length asked for, or when the ring would grow past its memory cap (the bitrate is
//! also chosen to fit the cap). Saving takes the groups that cover the length (closed groups
//! are shared, not copied) and writes them, with the sound kept alongside, as an MP4.
//!
//! It is off by default and only starts when the person (or an AI app) turns it on: on
//! Wayland that is the moment the desktop may ask which screen to share. At launch it comes
//! back only when the person asked for that, and on Wayland only when the remembered share
//! can be reused without a dialog (`can_resume_quietly`).
//!
//! Our own design from the behaviour wanted; no other program's code was read.

use crate::encode::{AudioPacket, Codec, VideoPacket};
use crate::stream::engine::{Encoders, FrameSource, SoundSource, StreamConfig, Streamer};
use crate::stream::queue::Packet;
use crate::stream::source::Grabber;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The lengths a person can pick, in seconds.
pub const LENGTHS: [u32; 4] = [30, 60, 90, 120];
/// The length when nothing is set.
pub const DEFAULT_S: u32 = 60;
/// The most memory the ring holds (video and sound).
pub const MAX_BYTES: usize = 512 << 20;
/// Less than this held and there is nothing worth saving yet.
pub const MIN_SAVE_US: i64 = 2_000_000;
/// The encoder's keyframe interval.
const KEYFRAME_S: u32 = 2;

/// A length brought within 30-120 s.
pub fn clamp_seconds(s: u32) -> u32 {
    s.clamp(LENGTHS[0], LENGTHS[LENGTHS.len() - 1])
}

/// The bitrate for a replay: the recording's own for this size, rate and quality, but never so
/// high that `seconds` (and the group being filled) would pass `max_bytes`.
pub fn bitrate(w: u32, h: u32, fps: u32, quality: u8, seconds: u32, max_bytes: usize) -> u32 {
    let wanted = crate::video::bitrate(w, h, fps, crate::video::VideoFormat::Mp4, quality);
    // Room for one extra group and the sound (about 160 kbit/s).
    let span = (clamp_seconds(seconds) + 2 * KEYFRAME_S) as u64;
    let budget = (max_bytes as u64 * 8 / span).saturating_sub(160_000);
    (wanted as u64).min(budget).max(250_000) as u32
}

/// About how many megabytes `seconds` take at `bitrate_bps` (for the settings page).
pub fn memory_mb(bitrate_bps: u32, seconds: u32) -> u32 {
    let bits = (bitrate_bps as u64 + 160_000) * (clamp_seconds(seconds) + KEYFRAME_S) as u64;
    (bits / 8 / 1_000_000).max(1) as u32
}

// ---------------------------------------------------------------------------------------------
// The ring
// ---------------------------------------------------------------------------------------------

/// One keyframe and the frames up to the next one, with the sound that came meanwhile.
#[derive(Clone, Debug, Default)]
pub struct Gop {
    pub start_us: i64,
    pub video: Vec<VideoPacket>,
    pub audio: Vec<AudioPacket>,
    /// The parameter sets the group decodes with (its keyframe's, or the last ones seen).
    pub sps: Vec<u8>,
    pub pps: Vec<u8>,
    bytes: usize,
}

/// The newest keyframe groups, cut at keyframes, within a length and a memory cap.
pub struct Ring {
    closed: VecDeque<Arc<Gop>>,
    open: Option<Gop>,
    keep_us: i64,
    max_bytes: usize,
    frame_us: i64,
    bytes: usize,
    /// The newest frame's time.
    last_us: Option<i64>,
    sps: Vec<u8>,
    pps: Vec<u8>,
    /// Groups thrown away because one group alone passed the memory cap.
    pub lost: u64,
}

impl Ring {
    pub fn new(seconds: u32, max_bytes: usize, fps: u32) -> Ring {
        Ring {
            closed: VecDeque::new(),
            open: None,
            keep_us: clamp_seconds(seconds) as i64 * 1_000_000,
            max_bytes,
            frame_us: 1_000_000 / fps.clamp(1, 240) as i64,
            bytes: 0,
            last_us: None,
            sps: Vec::new(),
            pps: Vec::new(),
            lost: 0,
        }
    }

    /// Keep this many seconds from now on (30-120).
    pub fn set_seconds(&mut self, seconds: u32) {
        self.keep_us = clamp_seconds(seconds) as i64 * 1_000_000;
        self.trim();
    }

    #[cfg(test)]
    pub fn seconds(&self) -> u32 {
        (self.keep_us / 1_000_000) as u32
    }

    pub fn push(&mut self, p: &Packet) {
        match p {
            Packet::Video(v) => self.push_video(v.clone()),
            Packet::Audio(a) => self.push_audio(a.clone()),
        }
    }

    pub fn push_video(&mut self, v: VideoPacket) {
        if v.keyframe {
            let mut changed = false;
            if let Some((s, p)) = crate::encode::sps_pps(&v.data) {
                changed = !self.sps.is_empty() && (s != self.sps || p != self.pps);
                (self.sps, self.pps) = (s, p);
            }
            if let Some(g) = self.open.take() {
                self.closed.push_back(Arc::new(g));
            }
            if changed {
                // Another encoder took over (the first one failed): one MP4 track has one set of
                // parameter sets, so what the old encoder made can't be saved with the new.
                self.closed.clear();
                self.bytes = 0;
            }
            if self.sps.is_empty() {
                // Nothing decodes without the parameter sets: wait for a keyframe with them.
                return;
            }
            self.open = Some(Gop { start_us: v.pts_us, sps: self.sps.clone(), pps: self.pps.clone(), ..Default::default() });
        }
        // Frames before the first keyframe can't be decoded: they are skipped.
        let Some(g) = self.open.as_mut() else { return };
        let n = v.data.len();
        self.last_us = Some(self.last_us.map_or(v.pts_us, |l| l.max(v.pts_us)));
        g.video.push(v);
        g.bytes += n;
        self.bytes += n;
        self.trim();
    }

    pub fn push_audio(&mut self, a: AudioPacket) {
        let Some(g) = self.open.as_mut() else { return };
        let n = a.data.len();
        g.audio.push(a);
        g.bytes += n;
        self.bytes += n;
        self.trim();
    }

    /// Where the newest frame ends.
    fn end_us(&self) -> Option<i64> {
        self.last_us.map(|l| l + self.frame_us)
    }

    fn first_start(&self) -> Option<i64> {
        self.closed.front().map(|g| g.start_us).or(self.open.as_ref().map(|g| g.start_us))
    }

    /// How much is held now, in microseconds.
    pub fn held_us(&self) -> i64 {
        match (self.first_start(), self.end_us()) {
            (Some(s), Some(e)) => (e - s).max(0),
            _ => 0,
        }
    }

    /// Bytes held (video and sound).
    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Groups held.
    #[cfg(test)]
    pub fn groups(&self) -> usize {
        self.closed.len() + usize::from(self.open.is_some())
    }

    /// Drop the oldest groups the newer ones don't need: while the groups after the oldest
    /// still cover the length, or the ring is over its memory cap.
    fn trim(&mut self) {
        let Some(end) = self.end_us() else { return };
        while let Some(first) = self.closed.front() {
            let second = self.closed.get(1).map(|g| g.start_us).or(self.open.as_ref().map(|g| g.start_us));
            let Some(second) = second else { break };
            if end - second >= self.keep_us || self.bytes > self.max_bytes {
                self.bytes -= first.bytes;
                self.closed.pop_front();
            } else {
                break;
            }
        }
        // One group alone over the cap (an encoder that never makes keyframes): start again
        // from the next keyframe rather than grow without end.
        if self.closed.is_empty()
            && self.bytes > self.max_bytes
            && let Some(g) = self.open.take()
        {
            self.bytes -= g.bytes;
            self.lost += 1;
            self.last_us = None;
        }
    }

    /// The last `seconds` (None: all that is kept), from the newest keyframe at or before that
    /// point, so the clip may be up to one group longer than asked. None when nothing is held.
    pub fn clip(&self, seconds: Option<u32>) -> Option<Clip> {
        let end = self.end_us()?;
        let mut gops: Vec<Arc<Gop>> = self.closed.iter().cloned().collect();
        if let Some(g) = &self.open {
            gops.push(Arc::new(g.clone()));
        }
        let want = seconds.map_or(self.keep_us, |s| s.max(1) as i64 * 1_000_000);
        let from = end - want;
        let first = gops.iter().rposition(|g| g.start_us <= from).unwrap_or(0);
        let mut gops = gops;
        let kept = gops.split_off(first);
        let start_us = kept.first()?.start_us;
        // Sound timed from the first keyframe on that came in before it (the video encoder may
        // hand its packets over later than the sound one) sits in the group before.
        let lead_audio = gops.last().map(|g| g.audio.iter().filter(|a| a.pts_us >= start_us).cloned().collect()).unwrap_or_default();
        Some(Clip { gops: kept, lead_audio, start_us, end_us: end, frame_us: self.frame_us })
    }
}

/// Keyframe groups taken from the ring, ready to be written.
pub struct Clip {
    pub gops: Vec<Arc<Gop>>,
    /// Sound from the group before the first one that belongs to the clip.
    pub lead_audio: Vec<AudioPacket>,
    pub start_us: i64,
    pub end_us: i64,
    frame_us: i64,
}

impl Clip {
    pub fn seconds(&self) -> f64 {
        (self.end_us - self.start_us) as f64 / 1e6
    }

    #[cfg(test)]
    pub fn frames(&self) -> usize {
        self.gops.iter().map(|g| g.video.len()).sum()
    }
}

/// What a written clip holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Written {
    pub frames: u64,
    pub seconds: f64,
    pub audio: bool,
}

/// Write `clip` as an MP4: H.264 video and, when `asc` (the AAC configuration) is given and
/// there is sound, an AAC track. Both start at the clip's first keyframe.
pub fn write_mp4<W: std::io::Write + std::io::Seek>(out: W, clip: &Clip, width: u32, height: u32, asc: Option<&[u8]>) -> Result<Written, String> {
    let first = clip.gops.first().ok_or("nothing to save")?;
    if first.sps.is_empty() || first.pps.is_empty() {
        return Err("the video has no parameter sets".into());
    }
    let base = clip.start_us;
    let video: Vec<&VideoPacket> = clip.gops.iter().flat_map(|g| g.video.iter()).collect();
    let mut audio: Vec<&AudioPacket> = clip.lead_audio.iter().chain(clip.gops.iter().flat_map(|g| g.audio.iter())).filter(|a| a.pts_us >= base && a.pts_us < clip.end_us).collect();
    audio.sort_by_key(|a| a.pts_us);
    let sound = asc.is_some() && !audio.is_empty();
    let config = mp4::Mp4Config {
        major_brand: mp4::FourCC::from(*b"isom"),
        minor_version: 512,
        compatible_brands: [*b"isom", *b"iso2", *b"avc1", *b"mp41"].into_iter().map(mp4::FourCC::from).collect(),
        timescale: 1000,
    };
    let mut m = mp4::Mp4Writer::write_start(out, &config).map_err(|e| e.to_string())?;
    m.add_track(&mp4::TrackConfig {
        track_type: mp4::TrackType::Video,
        timescale: VIDEO_TIMESCALE,
        language: "und".into(),
        media_conf: mp4::MediaConfig::AvcConfig(mp4::AvcConfig { width: width as u16, height: height as u16, seq_param_set: first.sps.clone(), pic_param_set: first.pps.clone() }),
    })
    .map_err(|e| e.to_string())?;
    if sound {
        m.add_track(&mp4::TrackConfig {
            track_type: mp4::TrackType::Audio,
            timescale: 48_000,
            language: "und".into(),
            media_conf: mp4::MediaConfig::AacConfig(mp4::AacConfig::default()),
        })
        .map_err(|e| e.to_string())?;
    }
    let ticks = |us: i64| (us.max(0) as u64 * VIDEO_TIMESCALE as u64 / 1_000_000) as u32;
    let mut frames = 0u64;
    for (i, v) in video.iter().enumerate() {
        let next = video.get(i + 1).map_or(v.pts_us + clip.frame_us, |n| n.pts_us);
        let (data, _, _) = crate::video::avcc_sample(&v.data);
        if data.is_empty() {
            continue;
        }
        let s = mp4::Mp4Sample { start_time: ticks(v.pts_us - base) as u64, duration: ticks(next - v.pts_us).max(1), rendering_offset: 0, is_sync: v.keyframe, bytes: data.into() };
        m.write_sample(1, &s).map_err(|e| e.to_string())?;
        frames += 1;
    }
    if sound {
        for a in &audio {
            let s = mp4::Mp4Sample { start_time: ((a.pts_us - base) as u64 * 48_000 / 1_000_000), duration: crate::encode::AAC_FRAME, rendering_offset: 0, is_sync: true, bytes: a.data.clone().into() };
            m.write_sample(2, &s).map_err(|e| e.to_string())?;
        }
    }
    m.write_end().map_err(|e| e.to_string())?;
    // The last bytes go out here: a full disk must fail the save, not leave a file that won't play.
    m.into_writer().flush().map_err(|e| e.to_string())?;
    Ok(Written { frames, seconds: clip.seconds(), audio: sound })
}

/// The video track's clock: 90 kHz, the usual one for video.
const VIDEO_TIMESCALE: u32 = 90_000;

// ---------------------------------------------------------------------------------------------
// Running it
// ---------------------------------------------------------------------------------------------

/// What one replay buffer needs.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplayOptions {
    pub seconds: u32,
    pub fps: u32,
    pub quality: u8,
    pub hardware: bool,
    /// The chosen encoder ("" automatic).
    pub encoder: String,
    pub cursor: bool,
    pub audio: crate::audio::AudioSource,
    pub mic: String,
    /// Where saved replays go.
    pub folder: String,
}

impl ReplayOptions {
    /// The Recording settings: the video's frame rate (60 at most: the ring holds seconds,
    /// not frames), quality, encoder and pointer, the replay's own length and sound, and the
    /// recordings folder.
    pub fn from_settings(s: &crate::settings::Settings) -> ReplayOptions {
        ReplayOptions {
            seconds: clamp_seconds(s.replay_s),
            fps: s.video_fps.clamp(1, 60),
            quality: s.video_quality.clamp(1, 4),
            hardware: s.video_hardware,
            encoder: s.video_encoder.clone(),
            cursor: s.gif_cursor,
            // Linux and macOS record no sound yet: no note about it every time.
            audio: if crate::audio::AudioSource::supported() { s.replay_audio } else { crate::audio::AudioSource::None },
            mic: s.audio_mic.clone(),
            folder: s.video_folder.clone(),
        }
    }
}

/// What a running replay buffer is.
#[derive(Clone, Debug, PartialEq)]
pub struct Info {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u32,
    pub encoder: String,
    pub hardware: bool,
    /// Whether sound is kept.
    pub sound: bool,
    /// Something the person should know (no sound, another encoder was used).
    pub notice: Option<String>,
}

/// A saved replay.
#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub path: PathBuf,
    pub seconds: f64,
    pub frames: u64,
    pub audio: bool,
}

/// A running replay buffer: the screen copy, the encoders and the ring. Stops when dropped.
pub struct Replay {
    streamer: Option<Streamer>,
    _grabber: Option<Grabber>,
    ring: Arc<Mutex<Ring>>,
    info: Info,
    asc: Option<Vec<u8>>,
    folder: String,
}

fn lock_ring(r: &Mutex<Ring>) -> MutexGuard<'_, Ring> {
    r.lock().unwrap_or_else(|p| p.into_inner())
}

impl Replay {
    /// Copy the screen the mouse is on and keep encoding it. On Wayland this waits for the
    /// person in the desktop's dialog (up to two minutes) unless the share is remembered;
    /// `cancel` gives up the wait.
    pub fn start(o: &ReplayOptions, cancel: &AtomicBool) -> Result<Replay, String> {
        let rect = crate::capture::monitor_under_cursor()?;
        let (w, h) = crate::video::even_size(rect.2, rect.3)?;
        let fps = o.fps.clamp(1, 60);
        let mut cfg = Replay::config(w, h, fps, o);
        let encoders = Encoders::system_picked(&o.encoder);
        let feed = crate::video::FeedOptions { fps, cursor: o.cursor, fit: Some((w, h)), every_frame: false };
        let (grabber, encoders) = match crate::stream::live::zero_copy(o.hardware, rect, None, feed, &cfg, &encoders) {
            Ok(both) => both,
            Err(_) => (Grabber::start(rect, None, w, h, fps, o.cursor), encoders),
        };
        let t = Instant::now();
        while !grabber.ready() && t.elapsed() < Duration::from_secs(120) && !cancel.load(Ordering::Relaxed) {
            if let Some(e) = grabber.error() {
                return Err(e);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if !grabber.ready() {
            return Err(grabber.error().unwrap_or_else(|| "The screen couldn't be copied.".into()));
        }
        let (sound, sound_note) = match crate::audio::Capture::start(o.audio, &o.mic) {
            Ok(c) if o.audio == crate::audio::AudioSource::None || c.is_silent() => (SoundSource::None, None),
            Ok(c) => (SoundSource::Capture(c), None),
            Err(e) => (SoundSource::None, Some(crate::i18n::tf("Replays are kept without sound: {reason}", &[("reason", &e)]))),
        };
        let frames = grabber.frames();
        let ring = Arc::new(Mutex::new(Ring::new(o.seconds, MAX_BYTES, fps)));
        let mut r = Replay::launch(&mut cfg, frames, sound, encoders, ring)?;
        r._grabber = Some(grabber);
        r.folder = o.folder.clone();
        if r.info.notice.is_none() {
            r.info.notice = sound_note;
        }
        Ok(r)
    }

    /// The encoder settings for a replay of `w` × `h` at `fps`.
    fn config(w: u32, h: u32, fps: u32, o: &ReplayOptions) -> StreamConfig {
        let mut cfg = StreamConfig::for_target(Default::default(), w, h, fps, Codec::H264);
        cfg.width = w;
        cfg.height = h;
        cfg.fps = fps;
        cfg.codec = Codec::H264;
        cfg.hardware = o.hardware;
        cfg.keyframe_s = KEYFRAME_S;
        cfg.video_kbps = bitrate(w, h, fps, o.quality, o.seconds, MAX_BYTES) / 1000;
        cfg.audio_kbps = 128;
        cfg
    }

    /// Start the encoders on `frames`, every packet into `ring`.
    fn launch(cfg: &mut StreamConfig, frames: FrameSource, sound: SoundSource, encoders: Encoders, ring: Arc<Mutex<Ring>>) -> Result<Replay, String> {
        let has_sound = !matches!(sound, SoundSource::None);
        let r = ring.clone();
        cfg.on_packet = Some(Arc::new(move |p: &Packet| lock_ring(&r).push(p)));
        let streamer = Streamer::start_local(cfg.clone(), frames, sound, encoders, "Replay buffer")?;
        let st = streamer.stats();
        let enc = st.video_encoder.clone().unwrap_or(crate::encode::EncoderName { name: String::new(), hardware: false });
        let info = Info {
            width: cfg.width,
            height: cfg.height,
            fps: cfg.fps,
            bitrate_bps: cfg.video_kbps * 1000,
            encoder: enc.name,
            hardware: enc.hardware,
            sound: has_sound && st.audio_encoder.is_some(),
            notice: st.notice,
        };
        let asc = info.sound.then(|| crate::encode::aac_lc_config(48_000, 2));
        Ok(Replay { streamer: Some(streamer), _grabber: None, ring, info, asc, folder: String::new() })
    }

    /// Seconds held now.
    pub fn held_s(&self) -> f32 {
        lock_ring(&self.ring).held_us() as f32 / 1e6
    }

    /// What stopped the encoders, if anything did.
    pub fn error(&self) -> Option<String> {
        self.streamer.as_ref().and_then(|s| s.stats().error)
    }

    pub fn set_seconds(&self, seconds: u32) {
        lock_ring(&self.ring).set_seconds(seconds);
    }

    /// The last `seconds` (None: all of it) as the clip to write, taken at once.
    pub fn clip(&self, seconds: Option<u32>) -> Result<Clip, String> {
        let ring = lock_ring(&self.ring);
        if ring.held_us() < MIN_SAVE_US {
            return Err(crate::i18n::t("Nothing to save yet: replay has only just started.").to_string());
        }
        ring.clip(seconds).ok_or_else(|| crate::i18n::t("Nothing to save yet: replay has only just started.").to_string())
    }

    /// A new file in the recordings folder.
    pub fn new_path(&self) -> PathBuf {
        crate::video::recording_path(&self.folder, "Replay", "mp4")
    }

    /// Stop the encoders and the screen copy (waits for their threads).
    pub fn stop(mut self) {
        if let Some(s) = self.streamer.take() {
            s.stop();
        }
    }
}

/// `write_mp4` into a new file at `path`.
fn write_clip(clip: &Clip, path: &std::path::Path, w: u32, h: u32, asc: Option<&[u8]>) -> Result<Saved, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let f = std::fs::File::create(path).map_err(|e| format!("Couldn't make {}: {e}", path.display()))?;
    let r = write_mp4(std::io::BufWriter::new(f), clip, w, h, asc);
    match r {
        Ok(wr) => Ok(Saved { path: path.to_path_buf(), seconds: wr.seconds, frames: wr.frames, audio: wr.audio }),
        Err(e) => {
            let _ = std::fs::remove_file(path);
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The one replay buffer of this Snagster (the window's, or an AI app's in the MCP server)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Off,
    /// Opening the screen and the encoders (on Wayland: maybe waiting in the share dialog).
    Starting,
    On,
}

enum Slot {
    Off,
    Starting(Arc<AtomicBool>),
    On(Box<Replay>),
}

struct Global {
    slot: Slot,
    seconds: u32,
    /// Why the last start failed.
    error: Option<String>,
    saving: bool,
    /// Counts saves that ended, with the last one's result.
    saved_no: u64,
    saved: Option<Result<Saved, String>>,
}

static GLOBAL: Mutex<Global> = Mutex::new(Global { slot: Slot::Off, seconds: DEFAULT_S, error: None, saving: false, saved_no: 0, saved: None });

fn global() -> MutexGuard<'static, Global> {
    GLOBAL.lock().unwrap_or_else(|p| p.into_inner())
}

/// What the window and AI apps see; never waits on the encoders.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub phase: Phase,
    /// The length asked for.
    pub seconds: u32,
    /// Seconds held now.
    pub held_s: f32,
    pub info: Option<Info>,
    /// Why the last start failed, or what stopped the encoders.
    pub error: Option<String>,
    pub saving: bool,
    pub saved_no: u64,
    pub saved: Option<Result<Saved, String>>,
}

impl Status {
    pub fn on(&self) -> bool {
        self.phase != Phase::Off
    }
}

pub fn status() -> Status {
    let g = global();
    let (phase, held_s, info, run_error) = match &g.slot {
        Slot::Off => (Phase::Off, 0.0, None, None),
        Slot::Starting(_) => (Phase::Starting, 0.0, None, None),
        Slot::On(r) => (Phase::On, r.held_s(), Some(r.info.clone()), r.error()),
    };
    Status { phase, seconds: g.seconds, held_s, info, error: run_error.or_else(|| g.error.clone()), saving: g.saving, saved_no: g.saved_no, saved: g.saved.clone() }
}

/// Turn the replay buffer on in the background. Errors only when it is on already; what goes
/// wrong while starting shows in `status().error`.
pub fn begin(o: ReplayOptions) -> Result<(), String> {
    begin_with(o, |o, cancel| Replay::start(o, cancel))
}

fn begin_with(o: ReplayOptions, start: impl FnOnce(&ReplayOptions, &AtomicBool) -> Result<Replay, String> + Send + 'static) -> Result<(), String> {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut g = global();
        if !matches!(g.slot, Slot::Off) {
            return Err(crate::i18n::t("Replay is already on.").to_string());
        }
        g.slot = Slot::Starting(cancel.clone());
        g.seconds = clamp_seconds(o.seconds);
        g.error = None;
    }
    let c = cancel.clone();
    let spawned = std::thread::Builder::new().name("snagster-replay-start".into()).spawn(move || {
        let r = start(&o, &c);
        let mut g = global();
        // Only this start's own slot is touched: turned off meanwhile (and maybe on again by a
        // newer start), it neither reports an error nor takes the newer start's place.
        let ours = matches!(&g.slot, Slot::Starting(x) if Arc::ptr_eq(x, &c)) && !c.load(Ordering::Relaxed);
        match r {
            Ok(r) if !ours => {
                drop(g);
                r.stop();
            }
            Ok(r) => {
                r.set_seconds(g.seconds);
                g.slot = Slot::On(Box::new(r));
            }
            Err(_) if !ours => {}
            Err(e) => {
                g.slot = Slot::Off;
                g.error = Some(e);
            }
        }
    });
    if let Err(e) = spawned {
        global().slot = Slot::Off;
        return Err(e.to_string());
    }
    Ok(())
}

/// Turn it off (nothing is saved); stops in the background. False when it was off.
pub fn end() -> bool {
    let mut g = global();
    match std::mem::replace(&mut g.slot, Slot::Off) {
        Slot::Off => false,
        Slot::Starting(c) => {
            c.store(true, Ordering::Relaxed);
            true
        }
        Slot::On(r) => {
            g.error = None;
            drop(g);
            let _ = std::thread::Builder::new().name("snagster-replay-stop".into()).spawn(move || r.stop());
            true
        }
    }
}

/// Keep this many seconds (also while on).
pub fn set_seconds(seconds: u32) {
    let mut g = global();
    g.seconds = clamp_seconds(seconds);
    if let Slot::On(r) = &g.slot {
        r.set_seconds(g.seconds);
    }
}

/// Save the last `seconds` (None: all kept) now, on this thread. The clip is taken at once;
/// writing it doesn't hold anything up, and the buffer keeps going.
pub fn save(seconds: Option<u32>) -> Result<Saved, String> {
    let (clip, path, w, h, asc) = {
        let g = global();
        let r = match &g.slot {
            Slot::On(r) => r,
            Slot::Starting(_) => return Err(crate::i18n::t("Replay is still starting.").to_string()),
            Slot::Off => return Err(crate::i18n::t("Replay is off. Turn it on in Record > Replay first.").to_string()),
        };
        (r.clip(seconds)?, r.new_path(), r.info.width, r.info.height, r.asc.clone())
    };
    write_clip(&clip, &path, w, h, asc.as_deref())
}

/// `save` on a thread of its own; the result shows in `status().saved` (with `saved_no` one
/// higher). `done` runs after (the window repaints).
pub fn save_in_background(seconds: Option<u32>, done: impl FnOnce() + Send + 'static) -> Result<(), String> {
    {
        let mut g = global();
        if g.saving {
            return Err(crate::i18n::t("Still saving the last replay.").to_string());
        }
        if !matches!(g.slot, Slot::On(_)) {
            drop(g);
            return save(seconds).map(|_| ());
        }
        g.saving = true;
    }
    std::thread::Builder::new()
        .name("snagster-replay-save".into())
        .spawn(move || {
            let r = save(seconds);
            let mut g = global();
            g.saving = false;
            g.saved_no += 1;
            g.saved = Some(r);
            drop(g);
            done();
        })
        .map(|_| ())
        .map_err(|e| {
            global().saving = false;
            e.to_string()
        })
}

/// Wait until it is no longer starting (for AI apps and tests), at most `d`.
pub fn wait_started(d: Duration) -> Status {
    let t = Instant::now();
    loop {
        let s = status();
        if s.phase != Phase::Starting || t.elapsed() >= d {
            return s;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Whether replay may come back by itself at launch without the desktop asking anything: on
/// Linux through the screen-share portal only when a share is remembered (its restore token
/// is saved); everywhere else always.
pub fn can_resume_quietly() -> bool {
    quietly(uses_screen_share(), || remembered_share())
}

/// Through the desktop's screen share only with a remembered share (asked only then).
fn quietly(share: bool, remembered: impl FnOnce() -> bool) -> bool {
    !share || remembered()
}

/// Whether the screen is copied through the desktop's screen share here (Linux on Wayland or
/// in a Flatpak), which may ask the person which screen to share.
pub fn uses_screen_share() -> bool {
    #[cfg(target_os = "linux")]
    {
        crate::video::copies_through_screen_share()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

fn remembered_share() -> bool {
    #[cfg(target_os = "linux")]
    {
        crate::portal::screencast_remembered()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

/// The status line after turning it on.
pub fn on_message(seconds: u32, hotkey: &str) -> String {
    let s = clamp_seconds(seconds).to_string();
    if hotkey.is_empty() {
        crate::i18n::tf("Replay on: keeping the last {seconds} s. Record > Save replay saves it.", &[("seconds", &s)])
    } else {
        crate::i18n::tf("Replay on: keeping the last {seconds} s. {hotkey} saves it.", &[("seconds", &s), ("hotkey", hotkey)])
    }
}

/// The status line after a save.
pub fn saved_message(s: &Saved) -> String {
    let name = s.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    crate::i18n::tf("Saved {name}: the last {seconds} s. Record > Play last video.", &[("name", &name), ("seconds", &format!("{:.0}", s.seconds))])
}

#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    const SPS: &[u8] = &[0x67, 0x42, 0xC0, 0x1F, 0xDA, 0x01, 0x40, 0x16, 0xE8];
    const PPS: &[u8] = &[0x68, 0xCE, 0x3C, 0x80];

    /// A fake H.264 frame at `ms`: a keyframe with its parameter sets, or a P frame.
    fn frame(ms: i64, key: bool, pad: usize) -> VideoPacket {
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
        d.resize(d.len() + pad, 0x55);
        VideoPacket { data: d, pts_us: ms * 1000, dts_us: ms * 1000, keyframe: key }
    }

    /// `seconds` of 10 fps video, a keyframe every 2 s, and AAC packets every ~21 ms.
    fn fill(ring: &mut Ring, seconds: i64, pad: usize) {
        let mut audio_ms = 0.0;
        for i in 0..seconds * 10 {
            let ms = i * 100;
            ring.push(&Packet::Video(frame(ms, i % 20 == 0, pad)));
            while audio_ms < (ms + 100) as f64 {
                ring.push(&Packet::Audio(AudioPacket { data: vec![0x21, 0x10, 7], pts_us: (audio_ms * 1000.0) as i64 }));
                audio_ms += 1024.0 * 1000.0 / 48_000.0;
            }
        }
    }

    #[test]
    fn the_ring_is_cut_only_at_keyframes_and_keeps_at_least_the_length() {
        let mut r = Ring::new(30, MAX_BYTES, 10);
        fill(&mut r, 100, 10);
        // 30 s asked, groups of 2 s: between 30 and 32 s are kept, from a keyframe.
        let held = r.held_us();
        assert!((30_000_000..32_000_000).contains(&held), "{held}");
        assert!(r.closed.iter().all(|g| g.video[0].keyframe && g.start_us == g.video[0].pts_us));
        assert_eq!(r.groups(), 15);
        let c = r.clip(None).unwrap();
        assert!(c.gops[0].video[0].keyframe);
        assert_eq!(c.end_us, 100_000_000);
    }

    #[test]
    fn frames_before_the_first_keyframe_are_skipped() {
        let mut r = Ring::new(30, MAX_BYTES, 10);
        r.push_video(frame(0, false, 0));
        r.push_audio(AudioPacket { data: vec![1], pts_us: 0 });
        assert_eq!((r.groups(), r.bytes(), r.held_us()), (0, 0, 0));
        assert!(r.clip(None).is_none());
        r.push_video(frame(100, true, 0));
        assert_eq!(r.groups(), 1);
        assert_eq!(r.held_us(), 100_000);
    }

    #[test]
    fn a_keyframe_without_parameter_sets_uses_the_last_ones() {
        let mut r = Ring::new(30, MAX_BYTES, 10);
        r.push_video(frame(0, true, 0));
        let mut bare = frame(2000, true, 0);
        bare.data = vec![0, 0, 0, 1, 0x65, 1, 2];
        r.push_video(bare);
        let c = r.clip(None).unwrap();
        assert_eq!(c.gops[1].start_us, 2_000_000);
        assert_eq!((c.gops[1].sps.as_slice(), c.gops[1].pps.as_slice()), (SPS, PPS));
    }

    #[test]
    fn memory_stays_under_the_cap() {
        // 4 kB a frame is 80 kB a group; a 300 kB cap holds three groups, not 30 s.
        let mut r = Ring::new(120, 300_000, 10);
        fill(&mut r, 60, 4000);
        assert!(r.bytes() <= 300_000, "{}", r.bytes());
        assert!(r.groups() <= 4);
        assert!(r.held_us() >= 4_000_000);
        assert_eq!(r.lost, 0);
        // The bytes counted are the bytes held.
        let held: usize = r.closed.iter().map(|g| g.bytes).sum::<usize>() + r.open.as_ref().map_or(0, |g| g.bytes);
        assert_eq!(held, r.bytes());
    }

    #[test]
    fn one_group_over_the_cap_is_dropped_rather_than_growing_for_ever() {
        let mut r = Ring::new(30, 10_000, 10);
        r.push_video(frame(0, true, 0));
        for i in 1..100 {
            r.push_video(frame(i * 100, false, 500));
        }
        assert!(r.bytes() <= 10_000);
        assert!(r.lost >= 1);
        // The next keyframe starts again.
        r.push_video(frame(10_000, true, 0));
        assert_eq!(r.groups(), 1);
    }

    #[test]
    fn a_shorter_length_trims_at_once_and_a_clip_starts_at_a_keyframe_before_the_time_asked() {
        let mut r = Ring::new(120, MAX_BYTES, 10);
        fill(&mut r, 100, 0);
        assert!(r.held_us() >= 99_000_000);
        // 7 s asked: the group from 92 s (the keyframe at or before 93 s).
        let c = r.clip(Some(7)).unwrap();
        assert_eq!(c.start_us, 92_000_000);
        assert!((c.seconds() - 8.0).abs() < 1e-9);
        assert_eq!(c.frames(), 80);
        r.set_seconds(30);
        assert_eq!(r.seconds(), 30);
        assert!(r.held_us() < 32_000_000);
        // Lengths stay within 30-120 s.
        r.set_seconds(5);
        assert_eq!(r.seconds(), 30);
        assert_eq!(clamp_seconds(500), 120);
    }

    #[test]
    fn closed_groups_are_shared_by_a_clip_not_copied() {
        let mut r = Ring::new(30, MAX_BYTES, 10);
        fill(&mut r, 10, 100);
        let c = r.clip(None).unwrap();
        assert!(Arc::ptr_eq(&c.gops[0], r.closed.front().unwrap()));
    }

    fn probe(bytes: &[u8]) -> (u64, f64, usize, u32) {
        let rd = mp4::Mp4Reader::read_header(std::io::Cursor::new(bytes), bytes.len() as u64).unwrap();
        let v = rd.tracks().get(&1).unwrap();
        let audio = rd.tracks().get(&2).map_or(0, |a| a.sample_count());
        (v.sample_count() as u64, v.duration().as_secs_f64(), rd.tracks().len(), audio)
    }

    #[test]
    fn a_saved_clip_is_an_mp4_of_the_length_kept_with_its_sound() {
        let mut r = Ring::new(30, MAX_BYTES, 10);
        fill(&mut r, 45, 20);
        let clip = r.clip(None).unwrap();
        let mut out = std::io::Cursor::new(Vec::new());
        let w = write_mp4(&mut out, &clip, 640, 360, Some(&[0x11, 0x90])).unwrap();
        assert!(w.audio);
        assert_eq!(w.frames, clip.frames() as u64);
        // 45 s recorded, 30 asked: from the keyframe at 14 s to 45 s.
        assert_eq!(clip.start_us, 14_000_000);
        assert_eq!(w.frames, 310);
        let (frames, secs, tracks, audio) = probe(out.get_ref());
        assert_eq!((frames, tracks), (310, 2));
        assert!((secs - 31.0).abs() < 0.01, "{secs}");
        // About 31 s of 21.3 ms AAC packets.
        assert!((1450..1455).contains(&audio), "{audio}");
        // Without sound: one track.
        let mut out = std::io::Cursor::new(Vec::new());
        assert!(!write_mp4(&mut out, &clip, 640, 360, None).unwrap().audio);
        assert_eq!(probe(out.get_ref()).2, 1);
    }

    #[test]
    fn the_bitrate_fits_the_memory_cap() {
        // A 4K screen at the best quality and 60 fps would be far more than 512 MB in 120 s.
        let b = bitrate(3840, 2160, 60, 4, 120, MAX_BYTES);
        assert!(b < crate::video::bitrate(3840, 2160, 60, crate::video::VideoFormat::Mp4, 4));
        assert!((b as u64 + 160_000) * 124 / 8 <= MAX_BYTES as u64);
        // A small screen keeps the recording's own bitrate.
        assert_eq!(bitrate(1280, 720, 30, 2, 60, MAX_BYTES), crate::video::bitrate(1280, 720, 30, crate::video::VideoFormat::Mp4, 2));
        assert!(memory_mb(8_000_000, 60) >= 60);
    }

    #[test]
    fn messages_name_the_length_and_the_hotkey() {
        assert_eq!(on_message(60, "Ctrl+Shift+F8"), "Replay on: keeping the last 60 s. Ctrl+Shift+F8 saves it.");
        assert!(on_message(500, "").contains("120 s. Record > Save replay"));
        let s = Saved { path: PathBuf::from("x").join("Replay_1.mp4"), seconds: 58.4, frames: 1, audio: false };
        assert_eq!(saved_message(&s), "Saved Replay_1.mp4: the last 58 s. Record > Play last video.");
    }

    #[test]
    fn options_come_from_the_settings() {
        let mut s = crate::settings::Settings::default();
        assert!(!s.replay_at_start, "off by default");
        assert_eq!(s.replay_s, DEFAULT_S);
        s.video_fps = 120;
        s.replay_s = 10;
        let o = ReplayOptions::from_settings(&s);
        assert_eq!((o.fps, o.seconds), (60, 30));
        assert_eq!(o.folder, s.video_folder);
    }

    /// The moving test picture (`encode::test_picture`, the one `--video-test` records), 64 × 64.
    fn moving() -> FrameSource {
        let n = Arc::new(std::sync::atomic::AtomicU32::new(0));
        FrameSource::Rgba(Arc::new(move |b: &mut [u8]| {
            crate::encode::test_picture(b, 64, 64, n.fetch_add(1, Ordering::Relaxed));
            true
        }))
    }

    fn opts(folder: &std::path::Path) -> ReplayOptions {
        let mut o = ReplayOptions::from_settings(&crate::settings::Settings::default());
        o.fps = 30;
        o.seconds = 30;
        o.folder = folder.to_string_lossy().to_string();
        o
    }

    /// The encoders, a ring and the save, with fake encoders on the synthetic picture: no screen.
    fn fake_replay(o: &ReplayOptions) -> Result<Replay, String> {
        let mut cfg = Replay::config(64, 64, o.fps, o);
        let ring = Arc::new(Mutex::new(Ring::new(o.seconds, MAX_BYTES, o.fps)));
        let feed: crate::stream::engine::AudioFeed = Box::new(|n| vec![0; n * 2]);
        let mut r = Replay::launch(&mut cfg, moving(), SoundSource::Feed(feed), crate::stream::tests::fakes(60, 0), ring)?;
        r.folder = o.folder.clone();
        Ok(r)
    }

    #[test]
    fn turning_on_keeping_and_saving_in_the_background() {
        let _l = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        end();
        let dir = std::env::temp_dir().join(format!("snagster-replay-test-{}", std::process::id()));
        let o = opts(&dir);
        assert!(save(None).unwrap_err().contains("Replay is off"));
        begin_with(o.clone(), |o, _| fake_replay(o)).unwrap();
        assert!(begin_with(o.clone(), |o, _| fake_replay(o)).unwrap_err().contains("already on"));
        let s = wait_started(Duration::from_secs(10));
        assert_eq!(s.phase, Phase::On, "{:?}", s.error);
        assert!(s.info.as_ref().unwrap().sound);
        // Too early: nothing to save yet.
        assert!(save(None).unwrap_err().contains("Nothing to save yet"));
        let t = Instant::now();
        while status().held_s < 2.5 && t.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let saved = save(None).unwrap();
        assert!(saved.path.starts_with(&dir));
        assert!(saved.path.file_name().unwrap().to_string_lossy().starts_with("Replay_"));
        assert!(saved.frames >= 60 && saved.seconds >= 2.0, "{saved:?}");
        assert!(saved.audio);
        let bytes = std::fs::read(&saved.path).unwrap();
        assert_eq!(probe(&bytes).0, saved.frames);
        // In the background: the result shows in the status.
        let before = status().saved_no;
        let (tx, rx) = std::sync::mpsc::channel();
        save_in_background(Some(2), move || tx.send(()).unwrap()).unwrap();
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let s = status();
        assert_eq!(s.saved_no, before + 1);
        assert!(matches!(&s.saved, Some(Ok(v)) if v.path.exists()));
        assert!(end());
        assert!(!end());
        assert_eq!(status().phase, Phase::Off);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_start_says_why_and_stays_off() {
        let _l = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        end();
        begin_with(opts(&std::env::temp_dir()), |_, _| Err("The share was cancelled.".into())).unwrap();
        let s = wait_started(Duration::from_secs(5));
        assert_eq!(s.phase, Phase::Off);
        assert_eq!(s.error.as_deref(), Some("The share was cancelled."));
    }

    #[test]
    fn turning_off_while_starting_gives_no_error_and_never_undoes_a_newer_start() {
        let _l = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        end();
        let o = opts(&std::env::temp_dir());
        // A start that only ends once cancelled (the share dialog answered after "Stop replay").
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        begin_with(o.clone(), move |_, c| {
            while !c.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5));
            }
            let _ = rx.recv_timeout(Duration::from_secs(5));
            Err("The screen couldn't be copied.".into())
        })
        .unwrap();
        assert!(end());
        // On again before the first start has given up: the newer start keeps its place.
        let (tx2, rx2) = std::sync::mpsc::channel::<()>();
        begin_with(o, move |_, _| {
            let _ = rx2.recv_timeout(Duration::from_secs(5));
            Err("second".into())
        })
        .unwrap();
        tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let s = status();
        assert_eq!(s.phase, Phase::Starting, "the first start's end took the newer one's place");
        assert_eq!(s.error, None, "turning it off is not an error");
        tx2.send(()).unwrap();
        let s = wait_started(Duration::from_secs(5));
        assert_eq!((s.phase, s.error.as_deref()), (Phase::Off, Some("second")));
    }

    #[test]
    fn launch_is_quiet_except_through_a_screen_share_nobody_remembers() {
        // X11 and Windows and macOS: no screen share, nothing asks.
        assert!(quietly(false, || panic!("not asked")));
        assert!(quietly(true, || true));
        assert!(!quietly(true, || false));
        if cfg!(not(target_os = "linux")) {
            assert!(!uses_screen_share() && can_resume_quietly());
        }
    }

    #[test]
    fn another_encoders_parameter_sets_start_the_ring_again() {
        let mut r = Ring::new(30, MAX_BYTES, 10);
        fill(&mut r, 10, 0);
        assert!(r.groups() >= 5);
        // The encoder was replaced (a driver reset): a keyframe with other parameter sets.
        let mut k = frame(10_000, true, 0);
        k.data = [&[0, 0, 0, 1, 0x67, 0x64, 0, 0x28, 1][..], &[0, 0, 0, 1, 0x68, 0xEE][..], &[0, 0, 0, 1, 0x65, 9][..]].concat();
        r.push_video(k);
        r.push_video(frame(10_100, false, 0));
        let c = r.clip(None).unwrap();
        assert_eq!((c.gops.len(), c.start_us), (1, 10_000_000));
        assert_eq!(c.gops[0].sps, vec![0x67, 0x64, 0, 0x28, 1]);
        let held: usize = r.open.as_ref().map_or(0, |g| g.bytes);
        assert_eq!(r.bytes(), held);
    }

    #[test]
    fn sound_that_came_before_its_keyframe_is_saved_with_it() {
        let mut r = Ring::new(30, MAX_BYTES, 10);
        r.push_video(frame(0, true, 0));
        r.push_video(frame(100, false, 0));
        // Timed 2.0 s, but handed over before the keyframe at 2.0 s (a slow video encoder).
        r.push_audio(AudioPacket { data: vec![1, 2], pts_us: 2_000_000 });
        r.push_video(frame(2000, true, 0));
        r.push_audio(AudioPacket { data: vec![3, 4], pts_us: 2_021_333 });
        for ms in (2100..4000).step_by(100) {
            r.push_video(frame(ms, false, 0));
        }
        let c = r.clip(Some(1)).unwrap();
        assert_eq!(c.start_us, 2_000_000);
        assert_eq!(c.lead_audio.len(), 1);
        let mut out = std::io::Cursor::new(Vec::new());
        assert!(write_mp4(&mut out, &c, 64, 64, Some(&[0x11, 0x90])).unwrap().audio);
        assert_eq!(probe(out.get_ref()).3, 2);
    }

    #[test]
    fn the_real_software_encoder_makes_a_playable_replay() {
        // OpenH264 is built into Snagster: on every system, no graphics chip needed.
        let cfg = crate::encode::VideoConfig { width: 64, height: 64, fps: 30, bitrate_bps: 300_000, keyframe_every_s: 1, codec: Codec::H264, prefer_hardware: false, low_latency: true };
        let mut enc = crate::encode::open_backend(crate::encode::Backend::OpenH264, &cfg).unwrap();
        let mut r = Ring::new(30, MAX_BYTES, 30);
        let mut rgba = vec![0u8; 64 * 64 * 4];
        for n in 0..90u32 {
            crate::encode::test_picture(&mut rgba, 64, 64, n);
            for p in enc.encode(&rgba, n as i64 * 33_333, false).unwrap() {
                r.push_video(p);
            }
        }
        let clip = r.clip(None).unwrap();
        let mut out = std::io::Cursor::new(Vec::new());
        let w = write_mp4(&mut out, &clip, 64, 64, None).unwrap();
        assert!(w.frames >= 60, "{w:?}");
        let (frames, secs, _, _) = probe(out.get_ref());
        assert_eq!(frames, w.frames);
        assert!(secs > 1.9, "{secs}");
    }
}
