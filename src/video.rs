//! Screen video recording: MP4 (H.264 or HEVC), WMV, at 1-120 frames per second.
//!
//! Windows: the screen is copied on the graphics chip with DXGI Desktop Duplication and handed,
//! still on the graphics chip, to Media Foundation's sink writer with hardware transforms on.
//! Windows then uses the graphics vendor's own encoder (Intel Quick Sync, AMD AMF/VCN or NVIDIA
//! NVENC, each installed with the graphics driver), so recording costs very little processor
//! time. When that path isn't available it falls back to copying the screen with GDI, and then
//! to Windows' software encoder.
//!
//! Linux and macOS: the screen is grabbed with xcap and encoded by `encode::open_video` (the
//! graphics chip through VideoToolbox or VA-API when it can, otherwise OpenH264) into an MP4.
//!
//! Written from the Windows SDK documentation; no code was taken from other recorders.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// What a recording is saved as.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum VideoFormat {
    /// MP4 with H.264: plays everywhere.
    #[default]
    Mp4,
    /// MP4 with HEVC (H.265): about half the size, needs a newer player.
    Hevc,
    /// Windows Media Video.
    Wmv,
    /// Animated GIF (Snagster's own small-file recorder).
    Gif,
}

impl VideoFormat {
    pub const ALL: [VideoFormat; 4] = [VideoFormat::Mp4, VideoFormat::Hevc, VideoFormat::Wmv, VideoFormat::Gif];

    pub fn label(self) -> &'static str {
        match self {
            VideoFormat::Mp4 => "MP4 video (H.264)",
            VideoFormat::Hevc => "MP4 video (HEVC / H.265)",
            VideoFormat::Wmv => "WMV video",
            VideoFormat::Gif => "Animated GIF",
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            VideoFormat::Mp4 | VideoFormat::Hevc => "mp4",
            VideoFormat::Wmv => "wmv",
            VideoFormat::Gif => "gif",
        }
    }

    /// Whether this computer's Snagster can make it at all.
    pub fn available(self) -> bool {
        cfg!(windows) || matches!(self, VideoFormat::Mp4 | VideoFormat::Gif)
    }

    pub fn parse(s: &str) -> Option<VideoFormat> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mp4" | "h264" | "h.264" | "avc" => Some(VideoFormat::Mp4),
            "hevc" | "h265" | "h.265" | "mp4-hevc" => Some(VideoFormat::Hevc),
            "wmv" => Some(VideoFormat::Wmv),
            "gif" => Some(VideoFormat::Gif),
            _ => None,
        }
    }
}

/// Fills one frame: top-down BGRA rows of the recording's size. Returns false to stop.
pub type FrameFeed = Arc<dyn Fn(&mut [u8]) -> bool + Send + Sync>;

/// Everything one video recording needs.
#[derive(Clone)]
pub struct VideoOpts {
    /// Desktop area in physical pixels: left, top, width, height.
    pub rect: (i32, i32, u32, u32),
    /// Frames per second, 1-120.
    pub fps: u32,
    pub format: VideoFormat,
    /// 1 small file ... 4 best picture.
    pub quality: u8,
    /// Use the graphics chip's encoder when there is one.
    pub hardware: bool,
    /// The chosen encoder's name ("" automatic, see `encode::open_video_picked`).
    pub encoder: String,
    /// Draw the mouse pointer.
    pub cursor: bool,
    /// Stop by itself after this many seconds (0: only when told).
    pub max_s: u32,
    pub path: PathBuf,
    /// Sound to record with it (MP4 only).
    pub audio: crate::audio::AudioSource,
    /// The microphone's id (empty: the default one).
    pub mic: String,
    /// Studio's sound: every input its scenes use, mixed by the scene shown
    /// (`audio::set_scene_mix`). Takes the place of `audio` and `mic` when set.
    pub sound: Option<Vec<crate::audio::SoundInput>>,
    /// Frames made by the caller (Studio mode) instead of the screen; `rect` then gives only
    /// the size.
    pub frames: Option<FrameFeed>,
}

impl VideoOpts {
    /// Plain options for a screen area: no sound, no custom frames.
    pub fn screen(rect: (i32, i32, u32, u32), fps: u32, format: VideoFormat, path: PathBuf) -> VideoOpts {
        VideoOpts { rect, fps, format, quality: 2, hardware: true, encoder: String::new(), cursor: true, max_s: 0, path, audio: crate::audio::AudioSource::None, mic: String::new(), sound: None, frames: None }
    }

    /// The options a screen recording of `rect` gets from the settings (what the window uses):
    /// format, frame rate, quality, encoder, pointer, time limit, sound, and a new file in the
    /// recordings folder. A GIF or a format this system can't make becomes MP4.
    pub fn from_settings(s: &crate::settings::Settings, rect: (i32, i32, u32, u32)) -> VideoOpts {
        let format = s.rec_format.video_or_mp4();
        VideoOpts {
            rect,
            fps: s.video_fps.clamp(1, 120),
            format,
            quality: s.video_quality.clamp(1, 4),
            hardware: s.video_hardware,
            encoder: s.video_encoder.clone(),
            cursor: s.gif_cursor,
            max_s: s.video_max_s,
            path: recording_path(&s.video_folder, "Recording", format.ext()),
            audio: s.audio_source,
            mic: s.audio_mic.clone(),
            sound: None,
            frames: None,
        }
    }
}

impl VideoFormat {
    /// This format when it's a video this system can make, otherwise MP4 (what Studio does
    /// with a GIF setting).
    pub fn video_or_mp4(self) -> VideoFormat {
        if self != VideoFormat::Gif && self.available() { self } else { VideoFormat::Mp4 }
    }

    /// The video formats (not GIF) this system can make, by their short names.
    pub fn video_names() -> Vec<&'static str> {
        VideoFormat::ALL
            .into_iter()
            .filter(|f| *f != VideoFormat::Gif && f.available())
            .map(|f| match f {
                VideoFormat::Mp4 => "mp4",
                VideoFormat::Hevc => "hevc",
                VideoFormat::Wmv => "wmv",
                VideoFormat::Gif => "gif",
            })
            .collect()
    }
}

/// A new file in `folder` named `<prefix>_<date>_<time>.<ext>` (with a number if it exists).
pub fn recording_path(folder: &str, prefix: &str, ext: &str) -> PathBuf {
    let name = format!("{prefix}_{}", chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"));
    crate::settings::unique_path(std::path::Path::new(folder), &name, ext)
}

/// How a recording went.
#[derive(Clone, Debug)]
pub struct VideoDone {
    pub path: PathBuf,
    pub frames: u64,
    pub seconds: f32,
    /// The encoder that made it, e.g. "Intel® Quick Sync Video H.264 Encoder MFT".
    pub encoder: String,
    /// Whether that encoder runs on the graphics chip.
    pub hardware: bool,
    /// How the screen was copied ("graphics chip" or "GDI").
    pub capture: &'static str,
    pub width: u32,
    pub height: u32,
    pub result: Result<(), String>,
    /// Something the person should know about a recording that worked, e.g. that the sound
    /// was left out because there was nothing to record it from. Empty when all went as asked.
    pub note: String,
}

/// The note for Studio sound inputs that couldn't start ("" when all did).
#[cfg(windows)]
fn studio_sound_note(notes: &[String]) -> String {
    if notes.is_empty() { String::new() } else { format!("Some sound was left out: {}", notes.join("; ")) }
}

/// Sound that can't be started doesn't stop the recording: the video is made without it and
/// the reason is kept as a note (a computer with no speakers, a microphone that was unplugged).
fn sound_or_note<T>(r: Result<T, String>) -> (Option<T>, String) {
    match r {
        Ok(c) => (Some(c), String::new()),
        Err(e) => (None, format!("Recorded without sound: {e}")),
    }
}

/// A video encoder this computer offers.
#[derive(Clone, Debug, Serialize)]
pub struct EncoderInfo {
    pub name: String,
    pub codec: &'static str,
    pub hardware: bool,
    pub vendor: String,
}

/// What a saved video holds, read back from the file.
#[derive(Clone, Debug)]
pub struct Probe {
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    pub seconds: f64,
    pub first: Option<image::RgbaImage>,
    /// Whether the file has a sound track.
    pub audio: bool,
}

/// Bits per second for an area, frame rate, codec and quality step.
pub fn bitrate(w: u32, h: u32, fps: u32, format: VideoFormat, quality: u8) -> u32 {
    let bpp = match quality.clamp(1, 4) {
        1 => 0.04,
        2 => 0.08,
        3 => 0.14,
        _ => 0.24,
    };
    let px = w as f64 * h as f64 * bpp;
    // Motion between frames shrinks as the rate goes up, so later frames cost less.
    let fps = fps.clamp(1, 120) as f64;
    let rate = px * fps.min(30.0) + px * 0.4 * (fps - 30.0).max(0.0);
    let codec = if format == VideoFormat::Hevc { 0.6 } else { 1.0 };
    (rate * codec).clamp(250_000.0, 160_000_000.0) as u32
}

/// Video sizes must be even; tiny areas are refused.
pub fn even_size(w: u32, h: u32) -> Result<(u32, u32), String> {
    let (w, h) = (w & !1, h & !1);
    if w < 64 || h < 64 {
        return Err("Pick an area at least 64 × 64 pixels for a video.".into());
    }
    Ok((w.min(8192), h.min(8192)))
}

/// Record on a background thread; `done` gets the result. `pause` holds the clock while set.
pub fn spawn(opts: VideoOpts, stop: Arc<AtomicBool>, pause: Arc<AtomicBool>, ctx: Option<eframe::egui::Context>, done: Sender<VideoDone>) {
    std::thread::spawn(move || {
        let r = record(&opts, &stop, &pause);
        let _ = done.send(r);
        if let Some(c) = ctx {
            c.request_repaint();
        }
    });
}

/// Record until `stop` (or `max_s`), blocking this thread.
pub fn record(opts: &VideoOpts, stop: &AtomicBool, pause: &AtomicBool) -> VideoDone {
    let mut done = VideoDone {
        path: opts.path.clone(),
        frames: 0,
        seconds: 0.0,
        encoder: String::new(),
        hardware: false,
        capture: "",
        width: 0,
        height: 0,
        result: Ok(()),
        note: String::new(),
    };
    if opts.format == VideoFormat::Gif {
        done.result = Err("GIFs are made by the GIF recorder.".into());
        return done;
    }
    if !opts.format.available() {
        done.result = Err(format!("{} can't be made on this system; pick MP4.", opts.format.label()));
        return done;
    }
    if let Some(dir) = opts.path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        done.result = Err(e.to_string());
        return done;
    }
    // A file that was there before (an MCP caller's path, say) is never removed below.
    let existed = opts.path.exists();
    crate::capture::background_thread(false);
    RECORDING_LIVE.store(cfg!(windows), Ordering::Relaxed);
    #[cfg(windows)]
    win::record(opts, stop, pause, &mut done);
    #[cfg(not(windows))]
    soft::record(opts, stop, pause, &mut done);
    RECORDING_LIVE.store(false, Ordering::Relaxed);
    if done.result.is_err() && !existed {
        remove_empty(&opts.path);
    }
    done
}

/// A recording that failed before its first picture (the share was cancelled, say) leaves no
/// empty file behind. A file with pictures in it is kept, whatever went wrong later; `record`
/// only calls this for a file it made itself.
fn remove_empty(path: &std::path::Path) {
    if std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == 0) {
        let _ = std::fs::remove_file(path);
    }
}

/// Whether the recording that is running has its screen and is counting time. On Wayland a
/// recording first waits for the person in the desktop's "Share screen" dialog; the recording
/// bar says so instead of counting.
static RECORDING_LIVE: AtomicBool = AtomicBool::new(false);

pub fn recording_live() -> bool {
    RECORDING_LIVE.load(Ordering::Relaxed)
}

/// Paces frames at a steady rate, holding the clock while paused and skipping ticks when the
/// encoder falls behind.
struct Clock {
    interval: Duration,
    base: Instant,
    n_base: u64,
    n: u64,
    paused_at: Option<Instant>,
    recorded: Duration,
}

impl Clock {
    fn new(fps: u32) -> Clock {
        Clock {
            interval: Duration::from_nanos(1_000_000_000 / fps.clamp(1, 120) as u64),
            base: Instant::now(),
            n_base: 0,
            n: 0,
            paused_at: None,
            recorded: Duration::ZERO,
        }
    }

    /// When the next frame is due.
    fn deadline(&self) -> Instant {
        self.base + self.interval * (self.n - self.n_base) as u32
    }

    /// Time stamp and length of the frame being written, in 100 ns units.
    fn stamp(&self, fps: u32) -> (i64, i64) {
        let fps = fps.clamp(1, 120) as i64;
        let t = self.n as i64 * 10_000_000 / fps;
        let t1 = (self.n as i64 + 1) * 10_000_000 / fps;
        (t, t1 - t)
    }

    /// After writing a frame: move on, skipping frames the encoder had no time for.
    fn advance(&mut self) {
        self.n += 1;
        self.recorded += self.interval;
        let now = Instant::now();
        let late = now.saturating_duration_since(self.deadline());
        if late > self.interval * 2 {
            let skip = (late.as_nanos() / self.interval.as_nanos().max(1)) as u64;
            self.n += skip;
            self.recorded += self.interval * skip as u32;
        }
    }

    /// Returns true while paused (after sleeping a little).
    fn hold(&mut self, pause: &AtomicBool) -> bool {
        if pause.load(Ordering::Relaxed) {
            if self.paused_at.is_none() {
                self.paused_at = Some(Instant::now());
            }
            std::thread::sleep(Duration::from_millis(30));
            return true;
        }
        if self.paused_at.take().is_some() {
            self.base = Instant::now();
            self.n_base = self.n;
        }
        false
    }

    fn over(&self, max_s: u32) -> bool {
        max_s > 0 && self.recorded.as_secs() >= max_s as u64
    }
}

/// The encoders this computer offers for MP4.
pub fn encoders() -> Vec<EncoderInfo> {
    #[cfg(windows)]
    {
        win::encoders()
    }
    #[cfg(not(windows))]
    {
        // What `encode::open_video` can pick for an MP4 here: VideoToolbox or VA-API when the
        // graphics chip has it, and OpenH264 always.
        crate::encode::available()
            .into_iter()
            .filter(|(c, _)| *c == crate::encode::Codec::H264)
            .map(|(c, n)| EncoderInfo { vendor: crate::encode::vendor_in(&n.name).to_string(), name: n.name, codec: c.label(), hardware: n.hardware })
            .collect()
    }
}

/// For `--video-test`: the encoders `encode::open_video` can use (live streaming everywhere,
/// recording off Windows), then how fast each one in line encodes 1080p H.264 here. A
/// graphics-chip or system encoder that won't start is skipped; OpenH264 must always work.
/// Returns the lines to print and how many checks failed.
pub fn encoder_report(quick: bool) -> (Vec<String>, u32) {
    use crate::encode::{Codec, Os};
    let mut lines = vec!["Encoders that hand back frames (live streaming; recording off Windows):".to_string()];
    for (c, n) in crate::encode::available() {
        lines.push(format!("  {:5} {:9} {}", c.label(), if n.hardware { "hardware" } else { "software" }, n.name));
    }
    let frames = if quick { 90 } else { 300 };
    let mut failed = 0;
    for b in crate::encode::plan(Os::current(), Codec::H264, true) {
        match crate::encode::speed(b, 1920, 1080, frames) {
            Ok((n, fps)) => {
                let vendor = crate::encode::vendor_in(&n.name);
                let place = match (n.hardware, vendor) {
                    (false, _) => String::new(),
                    (true, "") => " (graphics chip)".to_string(),
                    (true, v) => format!(" ({v} graphics chip)"),
                };
                lines.push(format!("ok   1920x1080 H.264 encode speed via {}: {fps:.0} fps{place}", n.name));
            }
            Err(e) if b != crate::encode::Backend::OpenH264 => lines.push(format!("skip {}: {e}", b.label())),
            Err(e) => {
                failed += 1;
                lines.push(format!("FAIL {}: {e}", b.label()));
            }
        }
    }
    (lines, failed)
}

/// Holds one encoded frame back until the next arrives, so each frame in the file lasts until
/// the next one starts (frames the encoder skipped or the clock dropped don't shorten the
/// video). Lengths are in 100 ns units, as the MP4 track's time scale.
#[cfg(any(not(windows), test))]
struct Spacer {
    pending: Option<crate::encode::VideoPacket>,
    frame_100ns: i64,
}

#[cfg(any(not(windows), test))]
impl Spacer {
    fn new(fps: u32) -> Spacer {
        Spacer { pending: None, frame_100ns: 10_000_000 / fps.clamp(1, 120) as i64 }
    }

    /// The frame before `p`, with its length, once `p` has come.
    fn push(&mut self, p: crate::encode::VideoPacket) -> Option<(crate::encode::VideoPacket, i64)> {
        let prev = self.pending.replace(p)?;
        let next = self.pending.as_ref().map_or(prev.pts_us, |n| n.pts_us);
        let d = ((next - prev.pts_us) * 10).max(1);
        Some((prev, d))
    }

    /// The last frame, one frame long.
    fn last(&mut self) -> Option<(crate::encode::VideoPacket, i64)> {
        self.pending.take().map(|p| (p, self.frame_100ns))
    }
}

/// An Annex-B access unit as an MP4 sample (4-byte lengths), without the SPS and PPS (which
/// go into the track header), and the SPS and PPS when it carried them.
#[cfg(any(not(windows), test))]
fn avcc_sample(au: &[u8]) -> (Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>) {
    use crate::encode::{Codec, nal_type, split_annex_b};
    let (mut data, mut sps, mut pps) = (Vec::with_capacity(au.len() + 16), None, None);
    for n in split_annex_b(au) {
        match nal_type(Codec::H264, n) {
            7 => sps = Some(n.to_vec()),
            8 => pps = Some(n.to_vec()),
            // Access unit delimiters don't belong in MP4 samples.
            9 => {}
            _ => {
                data.extend_from_slice(&(n.len() as u32).to_be_bytes());
                data.extend_from_slice(n);
            }
        }
    }
    (data, sps, pps)
}

/// Read a saved video back: its size, frame count, length and first frame.
pub fn probe(path: &std::path::Path) -> Result<Probe, String> {
    #[cfg(windows)]
    {
        win::probe(path)
    }
    #[cfg(not(windows))]
    {
        soft::probe(path)
    }
}

/// "VEN_10DE" → "NVIDIA". (Only Windows lists graphics-chip encoders.)
#[cfg(any(windows, test))]
pub fn vendor_name(id: &str) -> String {
    let id = id.to_ascii_uppercase();
    if id.contains("8086") {
        "Intel".into()
    } else if id.contains("1002") || id.contains("1022") {
        "AMD".into()
    } else if id.contains("10DE") {
        "NVIDIA".into()
    } else if id.contains("5143") || id.contains("QCOM") {
        "Qualcomm".into()
    } else {
        id
    }
}

// ---------------------------------------------------------------------------------------------
// A screen feed: the newest picture of a screen area, copied by a thread of its own (live
// streaming). Windows copies with Desktop Duplication (falling back to GDI, as recording does);
// Linux and macOS grab with xcap.
// ---------------------------------------------------------------------------------------------

/// One picture from a `ScreenFeed`: RGBA, rows packed, top row first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Counts up from 1 with every new picture (0: none yet).
    pub number: u64,
}

/// How a `ScreenFeed` copies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeedOptions {
    pub fps: u32,
    /// Draw the mouse pointer in.
    pub cursor: bool,
    /// Bring every picture to this size, in proportion and centred with black around it (the
    /// stream's size); None keeps the area's own size.
    pub fit: Option<(u32, u32)>,
    /// A new picture every frame even when nothing on the screen changed. Normally an unchanged
    /// screen isn't copied again (the last picture stays the newest); this is for measuring.
    pub every_frame: bool,
}

/// Pixels in memory, 4 bytes each, `stride` bytes from one row to the next.
pub struct Pixels<'a> {
    pub data: &'a [u8],
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    /// Blue, green, red order (Windows' screen copies) rather than red, green, blue.
    pub bgra: bool,
}

/// Where a picture of `w` × `h` goes in a frame of `fw` × `fh`, in proportion and centred:
/// left, top, width, height.
pub fn fit_rect(w: u32, h: u32, fw: u32, fh: u32) -> (u32, u32, u32, u32) {
    if w == 0 || h == 0 || fw == 0 || fh == 0 {
        return (0, 0, fw, fh);
    }
    let s = (fw as f64 / w as f64).min(fh as f64 / h as f64);
    let (dw, dh) = (((w as f64 * s).round() as u32).clamp(1, fw), ((h as f64 * s).round() as u32).clamp(1, fh));
    ((fw - dw) / 2, (fh - dh) / 2, dw, dh)
}

const BLACK: [u8; 4] = [0, 0, 0, 255];

/// A pixel as RGBA (opaque: GDI copies leave alpha at 0) packed in a little-endian u32.
#[inline(always)]
fn opaque_rgba(p: u32, bgra: bool) -> u32 {
    let p = if bgra { (p & 0x0000_FF00) | ((p & 0xFF) << 16) | ((p >> 16) & 0xFF) } else { p };
    p | 0xFF00_0000
}

/// `a` towards `b` by `w`/256, all four bytes at once (two at a time in 16-bit lanes).
#[inline(always)]
fn lerp_px(a: u32, b: u32, w: u32) -> u32 {
    let iw = 256 - w;
    let lo = (((a & 0x00FF_00FF) * iw + (b & 0x00FF_00FF) * w) >> 8) & 0x00FF_00FF;
    let hi = ((((a >> 8) & 0x00FF_00FF) * iw + ((b >> 8) & 0x00FF_00FF) * w) >> 8) & 0x00FF_00FF;
    lo | (hi << 8)
}

#[inline(always)]
fn px_at(row: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([row[i * 4], row[i * 4 + 1], row[i * 4 + 2], row[i * 4 + 3]])
}

/// Source column (and the next one) and weight for each output column, in 16.16 fixed point
/// with pixel centres lined up (the same sampling as `studio::blit_scaled`).
fn sample_points(src: u32, dst: u32) -> Vec<(u32, u32, u32)> {
    let f = ((src as u64) << 16) / dst.max(1) as u64;
    (0..dst)
        .map(|o| {
            let s = (o as u64 * f + f / 2).saturating_sub(1 << 15);
            let i = ((s >> 16) as u32).min(src - 1);
            (i, (i + 1).min(src - 1), ((s & 0xffff) >> 8) as u32)
        })
        .collect()
}

/// Draw `src` into `out` at `size`, in proportion and centred with black around it (bilinear
/// when the sizes differ, a straight copy when they match), as RGBA. Rows are done in bands on
/// several threads for big pictures; `out`'s buffer is reused.
pub fn fit_into(src: &Pixels, out: &mut Frame, size: (u32, u32)) {
    let (ow, oh) = size;
    out.width = ow;
    out.height = oh;
    out.rgba.resize(ow as usize * oh as usize * 4, 0);
    if ow == 0 || oh == 0 {
        return;
    }
    let (sw, sh) = (src.width, src.height);
    let (x, y, dw, dh) = fit_rect(sw, sh, ow, oh);
    let same = (dw, dh) == (sw, sh);
    let cols = if same || sw == 0 { Vec::new() } else { sample_points(sw, dw) };
    let rows = if same || sh == 0 { Vec::new() } else { sample_points(sh, dh) };
    let row_bytes = ow as usize * 4;
    let bands = crate::encode::bands_for(ow as usize, oh as usize).max(1);
    let per = (oh as usize).div_ceil(bands);
    let src_row = |r: u32| {
        let at = r as usize * src.stride;
        &src.data[at.min(src.data.len())..(at + sw as usize * 4).min(src.data.len())]
    };
    let band = |first: usize, part: &mut [u8]| {
        for (i, row) in part.chunks_exact_mut(row_bytes).enumerate() {
            let oy = (first + i) as u32;
            if sw == 0 || sh == 0 || oy < y || oy >= y + dh {
                row.chunks_exact_mut(4).for_each(|p| p.copy_from_slice(&BLACK));
                continue;
            }
            let (left, rest) = row.split_at_mut(x as usize * 4);
            let (mid, right) = rest.split_at_mut(dw as usize * 4);
            left.chunks_exact_mut(4).chain(right.chunks_exact_mut(4)).for_each(|p| p.copy_from_slice(&BLACK));
            let ry = oy - y;
            if same {
                let s = src_row(ry);
                for (d, p) in mid.chunks_exact_mut(4).zip(s.chunks_exact(4)) {
                    d.copy_from_slice(&opaque_rgba(u32::from_le_bytes([p[0], p[1], p[2], p[3]]), src.bgra).to_le_bytes());
                }
                continue;
            }
            let (r0, r1, wy) = rows[ry as usize];
            let (a, b) = (src_row(r0), src_row(r1));
            if a.len() < sw as usize * 4 || b.len() < sw as usize * 4 {
                mid.chunks_exact_mut(4).for_each(|p| p.copy_from_slice(&BLACK));
                continue;
            }
            for (d, &(c0, c1, wx)) in mid.chunks_exact_mut(4).zip(&cols) {
                let top = lerp_px(px_at(a, c0 as usize), px_at(a, c1 as usize), wx);
                let bottom = lerp_px(px_at(b, c0 as usize), px_at(b, c1 as usize), wx);
                d.copy_from_slice(&opaque_rgba(lerp_px(top, bottom, wy), src.bgra).to_le_bytes());
            }
        }
    };
    if bands == 1 {
        band(0, &mut out.rgba);
        return;
    }
    std::thread::scope(|s| {
        let band = &band;
        for (k, part) in out.rgba.chunks_mut(per * row_bytes).enumerate() {
            s.spawn(move || band(k * per, part));
        }
    });
}

/// Where pictures come from: Desktop Duplication or GDI on Windows, xcap elsewhere.
trait Grab {
    /// Wait until `until` (gathering screen changes where the system reports them), then put
    /// the area into `out` (at `fit`, or its own size) when something changed or `force`.
    /// Ok(false): nothing changed, `out` is untouched.
    fn grab(&mut self, until: Instant, force: bool, out: &mut Frame, fit: Option<(u32, u32)>) -> Result<bool, String>;
    /// Follow the area to `rect`; false when the grabber has to be made again for it.
    fn retarget(&mut self, rect: (i32, i32, u32, u32)) -> bool;
    fn name(&self) -> &'static str;
}

/// The best way to copy `rect` on this system: Desktop Duplication (or GDI) on Windows; on
/// Linux one X connection with shared memory on X11, and on Wayland (and in a Flatpak) a screen
/// share through the desktop's ScreenCast portal (XWayland when the desktop has none, e.g.
/// WSLg); on macOS `CGDisplayCreateImageForRect`; xcap where those can't be had. Never one
/// desktop screenshot per picture on Wayland (`per_picture_screenshots`).
fn open_grab(rect: (i32, i32, u32, u32), cursor: bool) -> Result<Box<dyn Grab>, String> {
    #[cfg(windows)]
    return win::open_grab(rect, cursor);
    #[cfg(target_os = "linux")]
    {
        let wayland = x11::is_wayland();
        let mut why = None;
        if use_portal(wayland, crate::update::managed_by() == Some("Flatpak")) {
            match PortalGrab::open(rect, cursor) {
                Ok(g) => return Ok(Box::new(g)),
                Err(e) => why = Some(e),
            }
        }
        // XWayland after the portal only where the desktop has no portal (WSLg): after a
        // Cancel or a refusal it would record XWayland's own black screen and call it saved.
        if x11_after_portal(wayland, why.is_some(), || crate::portal::screencast_available())
            && let Ok(g) = x11::X11Grab::open(rect, cursor)
        {
            return Ok(Box::new(g));
        }
        // The portal's own refusal is the answer on Wayland and in a Flatpak (a Flatpak on X11
        // may still read X11 above): never a screenshot per picture after it said no.
        if !per_picture_screenshots(wayland) || why.is_some() {
            return Err(why.unwrap_or_else(|| "the desktop's screen sharing isn't available".into()));
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(g) = cg::CgGrab::open(rect, cursor) {
        return Ok(Box::new(g));
    }
    #[cfg(not(windows))]
    return Ok(Box::new(XcapGrab { rect, cursor }));
}

/// Whether to read the screen through X11: always on X11; on Wayland only when the screen share
/// failed because the desktop has no portal (`portal_exists` is asked only then). A share the
/// person cancelled or the desktop refused must not end in XWayland, which sees only X11
/// windows on a black screen.
#[cfg(any(target_os = "linux", test))]
fn x11_after_portal(wayland: bool, portal_failed: bool, portal_exists: impl FnOnce() -> bool) -> bool {
    !wayland || (portal_failed && !portal_exists())
}

/// Whether a recording or stream may fall back to one screenshot per picture (xcap): not on
/// Wayland, where every screenshot is a request to the desktop (GNOME: a flash, a sound and a
/// permission check). Ubuntu froze under 2 a second; Wayland records through screen sharing.
#[cfg(any(target_os = "linux", test))]
fn per_picture_screenshots(wayland: bool) -> bool {
    !wayland
}

/// Whether Linux records through the ScreenCast portal first: on Wayland (where reading the
/// screen directly isn't allowed and xcap's way is one slow screenshot per picture) and inside
/// a Flatpak (where the sandbox only lets the portal in).
#[cfg(any(target_os = "linux", test))]
fn use_portal(wayland: bool, flatpak: bool) -> bool {
    wayland || flatpak
}

/// Linux on Wayland or in a Flatpak: the pictures of a screen share (`crate::portal::Cast`),
/// cut to the recorded area. The desktop asks the person which screen to share when it opens.
#[cfg(target_os = "linux")]
struct PortalGrab {
    cast: crate::portal::Cast,
    rect: (i32, i32, u32, u32),
    /// The newest whole picture of the shared screen, and its number and size.
    buf: Vec<u8>,
    seen: u64,
    size: (u32, u32),
}

#[cfg(target_os = "linux")]
impl PortalGrab {
    fn open(rect: (i32, i32, u32, u32), cursor: bool) -> Result<PortalGrab, String> {
        let cast = crate::portal::Cast::start(cursor)?;
        let mut g = PortalGrab { cast, rect, buf: Vec::new(), seen: 0, size: (0, 0) };
        // The first picture, so a recording never starts black (the share may take a moment).
        let until = Instant::now() + Duration::from_secs(5);
        while g.seen == 0 {
            g.take()?;
            if g.seen == 0 {
                if Instant::now() > until {
                    return Err("the screen share sent no picture".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        Ok(g)
    }

    /// Take the newest picture if there is one; Ok(true) when it is new.
    fn take(&mut self) -> Result<bool, String> {
        match self.cast.newer_than(self.seen, &mut self.buf)? {
            Some((w, h, n)) => {
                self.size = (w, h);
                self.seen = n;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

#[cfg(target_os = "linux")]
impl Grab for PortalGrab {
    fn grab(&mut self, until: Instant, force: bool, out: &mut Frame, fit: Option<(u32, u32)>) -> Result<bool, String> {
        sleep_until(until);
        let new = self.take()?;
        if !new && !force {
            return Ok(false);
        }
        let (bw, bh) = self.size;
        let (left, top, w, h) = crate::portal::crop_in_buffer(self.rect, self.cast.origin, self.cast.logical, (bw, bh)).unwrap_or((0, 0, bw, bh));
        let start = (top as usize * bw as usize + left as usize) * 4;
        let px = Pixels { data: &self.buf[start..], width: w, height: h, stride: bw as usize * 4, bgra: false };
        fit_into(&px, out, fit.unwrap_or((w, h)));
        Ok(true)
    }

    fn retarget(&mut self, rect: (i32, i32, u32, u32)) -> bool {
        self.rect = rect;
        true
    }

    fn name(&self) -> &'static str {
        "screen share (portal)"
    }
}

/// Wait until `until` (now if it has passed). Notes when the wait ended, so the feed can time
/// the copy on its own (a sleep that overruns isn't the copy's fault).
#[cfg(not(windows))]
fn sleep_until(until: Instant) {
    PACER.with(|p| p.borrow_mut().wait_until(until));
    WOKE.with(|w| w.set(Some(Instant::now())));
}

/// What `Pacer` does next with `left` to wait when sleeps overrun by about `overshoot`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Wait {
    Done,
    /// Sleep this long (the rest, less what the sleep will overrun by).
    Sleep(Duration),
    /// Too close for a sleep to end in time: give the processor away briefly and look again.
    Yield,
}

fn next_wait(left: Duration, overshoot: Duration) -> Wait {
    // A little margin on top of the usual overrun; sleeps shorter than 0.2 ms aren't worth it.
    let margin = overshoot + Duration::from_micros(200);
    if left.is_zero() {
        Wait::Done
    } else if left > margin + Duration::from_micros(200) {
        Wait::Sleep(left - margin)
    } else {
        Wait::Yield
    }
}

/// Waits until a moment on time even where sleeps run long. It learns how much this system's
/// sleeps overrun and, where that is a lot, sleeps that much less and yields the last part. On a
/// normal computer sleeps overrun by a fraction of a millisecond, so it only sleeps; the CI
/// Mac (a virtual machine) overran a 5 ms sleep by 25 ms, which held a 30 fps stream to 17.
#[derive(Debug, Default)]
pub struct Pacer {
    overshoot: Duration,
    /// Sleeps here overrun too much to sleep straight to the moment (decided once, by
    /// `calibrated`): cut them short and yield the rest. Otherwise plain sleeps, so a
    /// normal computer never spends processor time waiting.
    coarse: bool,
}

impl Pacer {
    /// Fold one sleep's overrun into the estimate: rises quickly (being late is what hurts),
    /// falls slowly (one quick wake-up proves little). Capped at 60 ms: a timer that overruns
    /// by more than a frame lasts (the CI Mac's did by 37 ms) can't be slept on at all, so the
    /// pacer yields instead. That only happens where `calibrated` found sleeps coarse; a normal
    /// computer sleeps plainly and never uses this estimate. The price on a coarse machine: after
    /// a long stall it may yield (use processor time) for good, but its frames stay on time.
    fn learn(&mut self, over: Duration) {
        let over = over.min(Self::MOST);
        self.overshoot = if over > self.overshoot { (self.overshoot + over * 3) / 4 } else { (self.overshoot * 15 + over) / 16 };
    }

    const MOST: Duration = Duration::from_millis(60);

    /// A pacer that starts from how much 5 ms sleeps overrun here (it keeps learning).
    pub fn calibrated() -> Pacer {
        // The middle overrun of five 5 ms sleeps (about a frame's wait). Not the smallest: a
        // real Mac running Snagster with no window in front let very short sleeps through on
        // time yet stretched 5 ms ones to 32 ms (macOS gathers a background program's timers),
        // and the smallest made it look precise. One stalled sleep doesn't move the middle.
        let mut overruns: Vec<Duration> = (0..5)
            .map(|_| {
                let t = Instant::now();
                std::thread::sleep(Duration::from_millis(5));
                t.elapsed().saturating_sub(Duration::from_millis(5))
            })
            .collect();
        overruns.sort();
        let middle = overruns[overruns.len() / 2].min(Self::MOST);
        Pacer { overshoot: middle, coarse: coarse_after(false, middle) }
    }

    /// How it waits, for a person: plain sleeps, or sleeps cut short and finished by yielding.
    pub fn mode(&self) -> String {
        pacer_mode(self.overshoot)
    }
}

/// Sleeps overrunning by less than this are taken as precise.
const COARSE: Duration = Duration::from_millis(2);

/// Whether to pace as on a coarse timer, given the current overrun estimate. Switches to coarse
/// at 2 ms and back only below 1 ms, so an estimate near the line doesn't flip every frame. The
/// timer can change while Snagster runs (macOS gathers timers once its window goes behind
/// others), so this is asked after every sleep, not only at the start.
fn coarse_after(coarse: bool, overshoot: Duration) -> bool {
    if coarse { overshoot >= COARSE / 2 } else { overshoot >= COARSE }
}

fn pacer_mode(overshoot: Duration) -> String {
    if overshoot < COARSE {
        format!("precise sleeps (overrun {:.1} ms)", overshoot.as_secs_f64() * 1000.0)
    } else {
        format!("coarse timer (sleeps overrun {:.0} ms): sleeps cut short, the rest yielded", overshoot.as_secs_f64() * 1000.0)
    }
}

impl Pacer {

    pub fn wait_until(&mut self, until: Instant) {
        if !self.coarse {
            let mut now = Instant::now();
            while now < until {
                std::thread::sleep(until - now);
                now = Instant::now();
            }
            // Keep learning: if sleeps start overrunning, switch to cutting them short.
            self.learn(now.saturating_duration_since(until));
            self.coarse = coarse_after(false, self.overshoot);
            return;
        }
        loop {
            let now = Instant::now();
            match next_wait(until.saturating_duration_since(now), self.overshoot) {
                Wait::Done => return,
                Wait::Sleep(d) => {
                    std::thread::sleep(d);
                    self.learn(now.elapsed().saturating_sub(d));
                    self.coarse = coarse_after(true, self.overshoot);
                }
                Wait::Yield => std::thread::yield_now(),
            }
        }
    }
}

thread_local! {
    /// The feed thread's pacer (see `sleep_until`).
    static PACER: std::cell::RefCell<Pacer> = std::cell::RefCell::new(Pacer::calibrated());
}

thread_local! {
    /// When the grabber on this thread last finished waiting (see `sleep_until`).
    static WOKE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// A thread that has to keep time to the frame (the stream's picture thread, the screen
/// feed). macOS: user-interactive quality of service, so its sleeps end on time rather than
/// being gathered up with other timers. (It doesn't cure a virtual machine whose every sleep
/// overruns by tens of milliseconds; `Pacer` copes with that.) Elsewhere the
/// default is already on time.
pub fn timely_thread() {
    #[cfg(target_os = "macos")]
    unsafe {
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(qos: u32, relative_priority: i32) -> i32;
        }
        const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
        pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

/// While this lives, macOS doesn't nap Snagster or stretch its timers (a stream or recording
/// in progress with the window in the background, or a process without a window): an
/// `NSProcessInfo` activity, user-initiated and latency-critical, as media programs take.
/// Nothing elsewhere.
pub struct TimelyActivity {
    #[cfg(target_os = "macos")]
    token: *mut std::ffi::c_void,
}

// The activity token is an Objective-C object that may be ended from any thread.
unsafe impl Send for TimelyActivity {}
unsafe impl Sync for TimelyActivity {}

#[cfg(target_os = "macos")]
mod objc {
    use std::ffi::{c_char, c_void};
    pub type Id = *mut c_void;
    #[link(name = "objc")]
    unsafe extern "C" {
        pub fn objc_getClass(name: *const c_char) -> Id;
        pub fn sel_registerName(name: *const c_char) -> Id;
        pub fn objc_msgSend();
    }
    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {}
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        pub fn CFStringCreateWithCString(a: *const c_void, s: *const c_char, encoding: u32) -> *const c_void;
        pub fn CFRelease(cf: *const c_void);
    }
    pub const UTF8: u32 = 0x0800_0100;
    /// `objc_msgSend` with no arguments, returning an object.
    pub unsafe fn send0(obj: Id, sel: &std::ffi::CStr) -> Id {
        let f: unsafe extern "C" fn(Id, Id) -> Id = unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { f(obj, sel_registerName(sel.as_ptr())) }
    }
}

impl TimelyActivity {
    pub fn begin(reason: &str) -> TimelyActivity {
        #[cfg(target_os = "macos")]
        unsafe {
            use objc::*;
            // NSActivityUserInitiated | NSActivityLatencyCritical
            const OPTIONS: u64 = 0x00FF_FFFF | 0xFF_0000_0000;
            let info = send0(objc_getClass(c"NSProcessInfo".as_ptr()), c"processInfo");
            let why = std::ffi::CString::new(reason).unwrap_or_default();
            let text = CFStringCreateWithCString(std::ptr::null(), why.as_ptr(), UTF8);
            let mut token = std::ptr::null_mut();
            if !info.is_null() && !text.is_null() {
                let begin: unsafe extern "C" fn(Id, Id, u64, *const std::ffi::c_void) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                token = begin(info, sel_registerName(c"beginActivityWithOptions:reason:".as_ptr()), OPTIONS, text);
                if !token.is_null() {
                    token = send0(token, c"retain");
                }
            }
            if !text.is_null() {
                CFRelease(text);
            }
            TimelyActivity { token }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = reason;
            TimelyActivity {}
        }
    }
}

impl Drop for TimelyActivity {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        unsafe {
            use objc::*;
            if self.token.is_null() {
                return;
            }
            let info = send0(objc_getClass(c"NSProcessInfo".as_ptr()), c"processInfo");
            if !info.is_null() {
                let end: unsafe extern "C" fn(Id, Id, Id) = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                end(info, sel_registerName(c"endActivity:".as_ptr()), self.token);
            }
            send0(self.token, c"release");
        }
    }
}

/// How long `sleep(ms)` really takes on this thread, on average over `n` sleeps (for
/// `--stream-test`: a system that oversleeps can't keep a frame rate).
pub fn measured_sleep_ms(ms: u64, n: u32) -> f64 {
    let t = Instant::now();
    for _ in 0..n.max(1) {
        std::thread::sleep(Duration::from_millis(ms));
    }
    t.elapsed().as_secs_f64() * 1000.0 / n.max(1) as f64
}

/// The part of `rect` inside `bounds` (both left, top, width, height); None when less than
/// 2 × 2 pixels of it is.
#[cfg(any(not(windows), test))]
fn clip_area(rect: (i32, i32, u32, u32), bounds: (i32, i32, u32, u32)) -> Option<(i32, i32, u32, u32)> {
    let (l, t) = (rect.0.max(bounds.0) as i64, rect.1.max(bounds.1) as i64);
    let r = (rect.0 as i64 + rect.2 as i64).min(bounds.0 as i64 + bounds.2 as i64);
    let b = (rect.1 as i64 + rect.3 as i64).min(bounds.1 as i64 + bounds.3 as i64);
    (r - l >= 2 && b - t >= 2).then(|| (l as i32, t as i32, (r - l) as u32, (b - t) as u32))
}

/// Where a pointer at `rel` (from the copied area's top left, in the area's units) lands in a
/// frame of `frame` pixels that shows an area of `area` units fitted in proportion.
#[cfg(any(not(windows), test))]
fn pointer_in_frame(rel: (f64, f64), area: (f64, f64), frame: (u32, u32)) -> (f32, f32) {
    if area.0 <= 0.0 || area.1 <= 0.0 {
        return (-100.0, -100.0);
    }
    // In proportion to the area's own size (fit_rect wants whole numbers; keep the precision).
    let s = (frame.0 as f64 / area.0).min(frame.1 as f64 / area.1);
    let (dw, dh) = (area.0 * s, area.1 * s);
    let (x, y) = ((frame.0 as f64 - dw) / 2.0, (frame.1 as f64 - dh) / 2.0);
    ((x + rel.0 * s) as f32, (y + rel.1 * s) as f32)
}

/// Draw the mouse pointer into a finished frame at `at` (frame pixels).
#[cfg(not(windows))]
fn stamp_frame_pointer(out: &mut Frame, at: (f32, f32)) {
    let (w, h) = (out.width, out.height);
    if let Some(mut img) = image::RgbaImage::from_raw(w, h, std::mem::take(&mut out.rgba)) {
        crate::capture::stamp_pointer(&mut img, at);
        out.rgba = img.into_raw();
    }
}

/// Linux and macOS when nothing quicker works (Wayland, an X server without the pixel format
/// handled here, a Mac without `CGDisplayCreateImageForRect`): xcap, one grab per frame. Each
/// xcap grab opens new X connections and looks the monitor up again, so it is slow (a few
/// pictures a second on X11); `X11Grab` and `CgGrab` are what normally runs.
#[cfg(not(windows))]
struct XcapGrab {
    rect: (i32, i32, u32, u32),
    cursor: bool,
}

#[cfg(not(windows))]
impl Grab for XcapGrab {
    fn grab(&mut self, until: Instant, _force: bool, out: &mut Frame, fit: Option<(u32, u32)>) -> Result<bool, String> {
        sleep_until(until);
        let (x, y, w, h) = self.rect;
        let mut img = crate::capture::grab_rect(x, y, w, h).map_err(|e| crate::capture::grab_error_message(&e, crate::capture::screen_access()))?;
        if self.cursor
            && let Some(c) = crate::capture::cursor_pos()
        {
            crate::capture::stamp_pointer(&mut img, ((c.0 - x) as f32, (c.1 - y) as f32));
        }
        let (iw, ih) = img.dimensions();
        fit_into(&Pixels { data: img.as_raw(), width: iw, height: ih, stride: iw as usize * 4, bgra: false }, out, fit.unwrap_or((iw, ih)));
        Ok(true)
    }

    fn retarget(&mut self, rect: (i32, i32, u32, u32)) -> bool {
        self.rect = rect;
        true
    }

    fn name(&self) -> &'static str {
        "screen grab"
    }
}

/// Bytes of shared memory for a `w` × `h` picture at 4 bytes a pixel, rounded up to whole
/// 4 KiB pages (a segment is made once and reused while pictures fit in it).
#[cfg(any(target_os = "linux", test))]
fn shm_segment_bytes(w: u32, h: u32) -> usize {
    (w as usize * h as usize * 4).div_ceil(4096).max(1) * 4096
}

/// Whether an X server's pictures can be read as 4-byte BGRA here: 24- or 32-bit colour stored
/// in 32 bits a pixel, least significant byte first (every PC X server; others use xcap).
#[cfg(any(target_os = "linux", test))]
fn x11_bgra(depth: u8, bits_per_pixel: u8, lsb_first: bool) -> bool {
    matches!(depth, 24 | 32) && bits_per_pixel == 32 && lsb_first
}

/// Linux on X11: one connection kept open, the area copied into shared memory with MIT-SHM
/// (`ShmGetImage`: the X server writes the pixels straight into memory Snagster reads) and
/// only the area asked for; plain `GetImage` where shared memory can't be had (a remote
/// display, a server without the extension).
#[cfg(target_os = "linux")]
mod x11 {
    use super::*;
    use x11rb::connection::{Connection, RequestConnection};
    use x11rb::protocol::shm::{self, ConnectionExt as _};
    use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, ImageOrder, Window};
    use x11rb::rust_connection::RustConnection;

    /// A Wayland session (X11 calls would only see XWayland's windows): xcap's Wayland path.
    pub fn is_wayland() -> bool {
        let var = |k: &str| std::env::var_os(k).unwrap_or_default().to_string_lossy().to_lowercase();
        var("XDG_SESSION_TYPE") == "wayland" || var("WAYLAND_DISPLAY").contains("wayland")
    }

    /// A System V shared-memory segment attached here and by the X server.
    struct Segment {
        seg: shm::Seg,
        addr: *mut u8,
        len: usize,
    }

    pub struct X11Grab {
        conn: RustConnection,
        root: Window,
        screen: (u32, u32),
        rect: (i32, i32, u32, u32),
        cursor: bool,
        /// None: not made yet (or given up on: `shm_ok` is false).
        segment: Option<Segment>,
        shm_ok: bool,
    }

    impl X11Grab {
        pub fn open(rect: (i32, i32, u32, u32), cursor: bool) -> Result<X11Grab, String> {
            let (conn, n) = RustConnection::connect(None).map_err(|e| format!("no X display: {e}"))?;
            let setup = conn.setup();
            let scr = setup.roots.get(n).ok_or("no X screen")?;
            let bpp = setup.pixmap_formats.iter().find(|f| f.depth == scr.root_depth).map_or(0, |f| f.bits_per_pixel);
            if !x11_bgra(scr.root_depth, bpp, setup.image_byte_order == ImageOrder::LSB_FIRST) {
                return Err(format!("{}-bit X screens are copied by xcap", scr.root_depth));
            }
            let (root, screen) = (scr.root, (scr.width_in_pixels as u32, scr.height_in_pixels as u32));
            let shm_ok = conn.extension_information(shm::X11_EXTENSION_NAME).ok().flatten().is_some() && conn.shm_query_version().ok().and_then(|c| c.reply().ok()).is_some();
            Ok(X11Grab { conn, root, screen, rect, cursor, segment: None, shm_ok })
        }

        /// A segment of at least `len` bytes, attached on both sides; None (and no more tries)
        /// when shared memory doesn't work here.
        fn segment(&mut self, len: usize) -> Option<&Segment> {
            if self.segment.as_ref().is_some_and(|s| s.len < len) {
                self.drop_segment();
            }
            if self.segment.is_none() && self.shm_ok {
                self.segment = self.make_segment(len);
                self.shm_ok = self.segment.is_some();
            }
            self.segment.as_ref()
        }

        fn make_segment(&self, len: usize) -> Option<Segment> {
            unsafe {
                let id = libc::shmget(libc::IPC_PRIVATE, len, libc::IPC_CREAT | 0o600);
                if id < 0 {
                    return None;
                }
                let addr = libc::shmat(id, std::ptr::null(), 0);
                let mapped = addr as isize != -1;
                let seg = if mapped { self.conn.generate_id().ok().filter(|&seg| self.conn.shm_attach(seg, id as u32, false).ok().is_some_and(|c| c.check().is_ok())) } else { None };
                // Marked for removal at once: the system frees it when both sides have let go
                // (also when Snagster ends without detaching).
                libc::shmctl(id, libc::IPC_RMID, std::ptr::null_mut());
                match seg {
                    Some(seg) => Some(Segment { seg, addr: addr as *mut u8, len }),
                    None => {
                        if mapped {
                            libc::shmdt(addr);
                        }
                        None
                    }
                }
            }
        }

        fn drop_segment(&mut self) {
            if let Some(s) = self.segment.take() {
                let _ = self.conn.shm_detach(s.seg).map(|c| c.ignore_error());
                let _ = self.conn.flush();
                unsafe {
                    libc::shmdt(s.addr as *const libc::c_void);
                }
            }
        }

        /// Copy the area into the shared segment (BGRA, rows packed); false: use GetImage.
        fn shm_copy(&mut self, x: i32, y: i32, w: u32, h: u32) -> bool {
            let root = self.root;
            let Some(seg) = self.segment(shm_segment_bytes(w, h)).map(|s| s.seg) else { return false };
            let ok = self
                .conn
                .shm_get_image(root, x as i16, y as i16, w as u16, h as u16, !0, ImageFormat::Z_PIXMAP.into(), seg, 0)
                .ok()
                .and_then(|c| c.reply().ok())
                .is_some();
            if !ok {
                // Shared memory stopped working (or never did for this drawable): GetImage.
                self.drop_segment();
                self.shm_ok = false;
            }
            ok
        }
    }

    impl Drop for X11Grab {
        fn drop(&mut self) {
            self.drop_segment();
        }
    }

    impl Grab for X11Grab {
        fn grab(&mut self, until: Instant, _force: bool, out: &mut Frame, fit: Option<(u32, u32)>) -> Result<bool, String> {
            sleep_until(until);
            let (x, y, w, h) = clip_area(self.rect, (0, 0, self.screen.0, self.screen.1)).ok_or("that area is off screen")?;
            let (w, h) = (w.min(u16::MAX as u32), h.min(u16::MAX as u32));
            let size = fit.unwrap_or((w, h));
            let len = w as usize * h as usize * 4;
            let plain;
            let data: &[u8] = match if self.shm_copy(x, y, w, h) { self.segment.as_ref() } else { None } {
                // The X server has written the picture into the segment (it is at least `len`).
                Some(s) => unsafe { std::slice::from_raw_parts(s.addr, len) },
                None => {
                    let r = self.conn.get_image(ImageFormat::Z_PIXMAP, self.root, x as i16, y as i16, w as u16, h as u16, !0).map_err(|e| e.to_string())?.reply().map_err(|e| format!("the X server didn't give the picture: {e}"))?;
                    plain = r.data;
                    &plain[..]
                }
            };
            if data.len() < w as usize * h as usize * 4 {
                return Err("the X server gave a short picture".into());
            }
            fit_into(&Pixels { data, width: w, height: h, stride: w as usize * 4, bgra: true }, out, size);
            if self.cursor
                && let Some(p) = self.conn.query_pointer(self.root).ok().and_then(|c| c.reply().ok())
            {
                let rel = ((p.root_x as i32 - x) as f64, (p.root_y as i32 - y) as f64);
                stamp_frame_pointer(out, pointer_in_frame(rel, (w as f64, h as f64), size));
            }
            Ok(true)
        }

        fn retarget(&mut self, rect: (i32, i32, u32, u32)) -> bool {
            self.rect = rect;
            true
        }

        fn name(&self) -> &'static str {
            if self.shm_ok { "X11 shared memory" } else { "X11" }
        }
    }
}

/// How a Core Graphics picture's pixels are laid out: Some(true) for BGRA (32-bit little-endian
/// with alpha or padding first, what screens give), Some(false) for RGBA, None for anything
/// else (wide colour, 16-bit).
#[cfg(any(target_os = "macos", test))]
fn cg_bgra(bits_per_pixel: usize, bitmap_info: u32) -> Option<bool> {
    const ALPHA_MASK: u32 = 0x1F;
    const ORDER_MASK: u32 = 0x7000;
    const ORDER_DEFAULT: u32 = 0;
    const ORDER_32_LITTLE: u32 = 2 << 12;
    const ORDER_32_BIG: u32 = 4 << 12;
    // CGImageAlphaInfo: 1 premultiplied last, 2 premultiplied first, 3 last, 4 first,
    // 5 none (skip last), 6 none (skip first).
    let first = matches!(bitmap_info & ALPHA_MASK, 2 | 4 | 6);
    let last = matches!(bitmap_info & ALPHA_MASK, 1 | 3 | 5);
    if bits_per_pixel != 32 || bitmap_info & 0x100 != 0 {
        return None; // not 8 bits a channel, or floating point
    }
    match bitmap_info & ORDER_MASK {
        ORDER_32_LITTLE if first => Some(true),
        ORDER_DEFAULT | ORDER_32_BIG if last => Some(false),
        _ => None,
    }
}

/// macOS: `CGDisplayCreateImageForRect` (the display's own picture, only the area asked for),
/// much quicker than the `CGWindowListCreateImage` xcap uses, which puts every window together
/// again for each grab. It is looked up while running, so a macOS without it uses xcap.
#[cfg(target_os = "macos")]
mod cg {
    use super::*;
    use std::ffi::c_void;
    use std::sync::atomic::AtomicBool;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGSize {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    type CreateForRect = unsafe extern "C" fn(u32, CGRect) -> *mut c_void;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGGetDisplaysWithPoint(p: CGPoint, max: u32, ids: *mut u32, count: *mut u32) -> i32;
        fn CGDisplayBounds(id: u32) -> CGRect;
        fn CGImageGetWidth(i: *mut c_void) -> usize;
        fn CGImageGetHeight(i: *mut c_void) -> usize;
        fn CGImageGetBytesPerRow(i: *mut c_void) -> usize;
        fn CGImageGetBitsPerPixel(i: *mut c_void) -> usize;
        fn CGImageGetBitmapInfo(i: *mut c_void) -> u32;
        fn CGImageGetDataProvider(i: *mut c_void) -> *mut c_void;
        fn CGDataProviderCopyData(p: *mut c_void) -> *const c_void;
        fn CGImageRelease(i: *mut c_void);
        fn CGEventCreate(source: *const c_void) -> *mut c_void;
        fn CGEventGetLocation(e: *mut c_void) -> CGPoint;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFDataGetBytePtr(d: *const c_void) -> *const u8;
        fn CFDataGetLength(d: *const c_void) -> isize;
        fn CFRelease(cf: *const c_void);
    }
    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const std::ffi::c_char) -> *mut c_void;
    }
    const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

    /// Set once a grab gave nothing (the call exists but doesn't work here): xcap from then on.
    static BROKEN: AtomicBool = AtomicBool::new(false);

    fn create_for_rect() -> Option<CreateForRect> {
        let p = unsafe { dlsym(RTLD_DEFAULT, c"CGDisplayCreateImageForRect".as_ptr()) };
        (!p.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, CreateForRect>(p) })
    }

    pub struct CgGrab {
        create: CreateForRect,
        rect: (i32, i32, u32, u32),
        cursor: bool,
    }

    impl CgGrab {
        pub fn open(rect: (i32, i32, u32, u32), cursor: bool) -> Option<CgGrab> {
            if BROKEN.load(Ordering::Relaxed) {
                return None;
            }
            Some(CgGrab { create: create_for_rect()?, rect, cursor })
        }
    }

    impl Grab for CgGrab {
        fn grab(&mut self, until: Instant, _force: bool, out: &mut Frame, fit: Option<(u32, u32)>) -> Result<bool, String> {
            sleep_until(until);
            let (x, y, w, h) = self.rect;
            unsafe {
                // The display under the area's centre, and the area in its own points.
                let centre = CGPoint { x: x as f64 + w as f64 / 2.0, y: y as f64 + h as f64 / 2.0 };
                let (mut id, mut n) = (0u32, 0u32);
                if CGGetDisplaysWithPoint(centre, 1, &mut id, &mut n) != 0 || n == 0 {
                    return Err("that area is off screen".into());
                }
                let b = CGDisplayBounds(id);
                let bounds = (b.origin.x as i32, b.origin.y as i32, b.size.width as u32, b.size.height as u32);
                let (cx, cy, cw, ch) = clip_area(self.rect, bounds).ok_or("that area is off screen")?;
                let local = CGRect { origin: CGPoint { x: (cx - bounds.0) as f64, y: (cy - bounds.1) as f64 }, size: CGSize { width: cw as f64, height: ch as f64 } };
                let img = (self.create)(id, local);
                if img.is_null() {
                    BROKEN.store(true, Ordering::Relaxed);
                    return Err("macOS gave no picture of the screen (is screen recording allowed?)".into());
                }
                let (iw, ih, stride) = (CGImageGetWidth(img), CGImageGetHeight(img), CGImageGetBytesPerRow(img));
                let Some(bgra) = cg_bgra(CGImageGetBitsPerPixel(img), CGImageGetBitmapInfo(img)) else {
                    CGImageRelease(img);
                    BROKEN.store(true, Ordering::Relaxed);
                    return Err("this screen's pixel format is copied by xcap".into());
                };
                let data = CGDataProviderCopyData(CGImageGetDataProvider(img));
                CGImageRelease(img);
                if data.is_null() {
                    return Err("couldn't read the screen's picture".into());
                }
                let bytes = std::slice::from_raw_parts(CFDataGetBytePtr(data), CFDataGetLength(data).max(0) as usize);
                let size = fit.unwrap_or((iw as u32, ih as u32));
                if bytes.len() >= stride * ih.saturating_sub(1) + iw * 4 {
                    fit_into(&Pixels { data: bytes, width: iw as u32, height: ih as u32, stride, bgra }, out, size);
                }
                CFRelease(data);
                if self.cursor {
                    let e = CGEventCreate(std::ptr::null());
                    if !e.is_null() {
                        let p = CGEventGetLocation(e);
                        CFRelease(e);
                        stamp_frame_pointer(out, pointer_in_frame((p.x - cx as f64, p.y - cy as f64), (cw as f64, ch as f64), size));
                    }
                }
            }
            Ok(true)
        }

        fn retarget(&mut self, rect: (i32, i32, u32, u32)) -> bool {
            self.rect = rect;
            true
        }

        fn name(&self) -> &'static str {
            "display copy"
        }
    }
}

#[derive(Default)]
struct FeedShared {
    front: std::sync::Mutex<Frame>,
    stop: AtomicBool,
    /// Where the area has moved to (a window being followed).
    moved: std::sync::Mutex<Option<(i32, i32, u32, u32)>>,
    method: std::sync::Mutex<&'static str>,
    error: std::sync::Mutex<Option<String>>,
    /// The number of the newest picture (the same as `front`'s, without taking the lock).
    count: std::sync::atomic::AtomicU64,
    /// Grabs made and the microseconds they took (not counting the wait for the next frame).
    grabs: std::sync::atomic::AtomicU64,
    grab_us: std::sync::atomic::AtomicU64,
}

/// The newest picture of a screen area, copied `fps` times a second by a thread of its own
/// into buffers that are reused. Stops when dropped.
pub struct ScreenFeed {
    sh: Arc<FeedShared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// A feed of `rect` (desktop pixels, physical): see `ScreenFeed::start`.
pub fn screen_feed(rect: (i32, i32, u32, u32), o: FeedOptions) -> Result<ScreenFeed, String> {
    ScreenFeed::start(rect, o).map_err(|e| crate::capture::grab_error_message(&e, crate::capture::screen_access()))
}

impl ScreenFeed {
    /// Start copying; waits (up to 2 s) for the first picture. Fails when the area can't be
    /// copied at all; a copy that is merely slow to start gives black until it does.
    pub fn start(rect: (i32, i32, u32, u32), o: FeedOptions) -> Result<ScreenFeed, String> {
        ScreenFeed::start_with(rect, o, open_grab)
    }

    /// `start` with another way of copying (the tests' fake screen).
    fn start_with(rect: (i32, i32, u32, u32), o: FeedOptions, open: Opener) -> Result<ScreenFeed, String> {
        if rect.2 < 2 || rect.3 < 2 {
            return Err("The area to copy is too small.".into());
        }
        let sh = Arc::new(FeedShared::default());
        let s = sh.clone();
        let thread = std::thread::Builder::new().name("snagster-screen-feed".into()).spawn(move || feed_loop(&s, rect, o, open)).map_err(|e| e.to_string())?;
        let feed = ScreenFeed { sh, thread: Some(thread) };
        let t = Instant::now();
        while feed.frames() == 0 && t.elapsed() < Duration::from_secs(2) {
            if let Some(e) = feed.error()
                && feed.thread.as_ref().is_some_and(|t| t.is_finished())
            {
                return Err(e);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(feed)
    }

    /// How many pictures have been made (the newest one's number).
    pub fn frames(&self) -> u64 {
        self.sh.count.load(Ordering::Relaxed)
    }

    /// Look at the newest picture (briefly: the capture thread waits for it to swap in the next).
    pub fn with_latest<R>(&self, f: impl FnOnce(&Frame) -> R) -> R {
        let g = self.sh.front.lock().unwrap_or_else(|p| p.into_inner());
        f(&g)
    }

    /// Copy from here on (a window that moved).
    pub fn move_to(&self, rect: (i32, i32, u32, u32)) {
        if rect.2 >= 2 && rect.3 >= 2 {
            *self.sh.moved.lock().unwrap_or_else(|p| p.into_inner()) = Some(rect);
        }
    }

    /// How it copies ("graphics chip", "GDI", "screen grab"), once started.
    pub fn method(&self) -> &'static str {
        *self.sh.method.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The last thing that went wrong copying, if anything.
    pub fn error(&self) -> Option<String> {
        self.sh.error.lock().ok().and_then(|e| e.clone())
    }

    /// Grabs made so far and the microseconds they took in all (for `--stream-test`).
    pub fn grab_time(&self) -> (u64, u64) {
        (self.sh.grabs.load(Ordering::Relaxed), self.sh.grab_us.load(Ordering::Relaxed))
    }
}

impl Drop for ScreenFeed {
    fn drop(&mut self) {
        self.sh.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Zero-copy screen feeds: pictures that stay in graphics memory, already NV12 at the stream's
// size, for an encoder on the same device (Windows: `gpu`, Direct3D 11; macOS: `display`,
// CGDisplayStream IOSurfaces).
// ---------------------------------------------------------------------------------------------

#[cfg(windows)]
mod gpu;
#[cfg(target_os = "macos")]
mod display;

/// What a zero-copy feed's thread (or callback) shares with its readers.
#[cfg(zero_copy)]
#[derive(Default)]
struct SurfaceShared {
    latest: std::sync::Mutex<Option<crate::encode::Surface>>,
    count: std::sync::atomic::AtomicU64,
    grabs: std::sync::atomic::AtomicU64,
    grab_us: std::sync::atomic::AtomicU64,
    error: std::sync::Mutex<Option<String>>,
    moved: std::sync::Mutex<Option<(i32, i32, u32, u32)>>,
    stop: AtomicBool,
}

#[cfg(zero_copy)]
impl SurfaceShared {
    /// A new picture, made in the time since `began`.
    fn publish(&self, s: crate::encode::Surface, began: Instant) {
        *self.latest.lock().unwrap_or_else(|p| p.into_inner()) = Some(s);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.grabs.fetch_add(1, Ordering::Relaxed);
        self.grab_us.fetch_add(began.elapsed().as_micros() as u64, Ordering::Relaxed);
    }

    #[cfg(windows)]
    fn set_error(&self, e: String) {
        *self.error.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
    }
}

/// A screen area copied into graphics memory at the stream's size, in the encoder's input
/// format: no picture passes through the processor (zero-copy). Stops when dropped.
#[cfg(zero_copy)]
pub struct SurfaceFeed {
    sh: Arc<SurfaceShared>,
    device: crate::encode::SurfaceDevice,
    method: &'static str,
    /// Stops the copying (joins the thread, or stops the display stream).
    end: Option<Box<dyn FnOnce() + Send + Sync>>,
}

#[cfg(zero_copy)]
impl SurfaceFeed {
    fn new(sh: Arc<SurfaceShared>, device: crate::encode::SurfaceDevice, method: &'static str, end: Box<dyn FnOnce() + Send + Sync>) -> SurfaceFeed {
        SurfaceFeed { sh, device, method, end: Some(end) }
    }

    /// The newest picture (None until the first).
    pub fn latest(&self) -> Option<crate::encode::Surface> {
        self.sh.latest.lock().ok().and_then(|l| l.clone())
    }

    /// Pictures made so far.
    pub fn frames(&self) -> u64 {
        self.sh.count.load(Ordering::Relaxed)
    }

    /// The device the pictures live on (to open the encoder there).
    pub fn device(&self) -> crate::encode::SurfaceDevice {
        self.device.clone()
    }

    /// Pictures made and the microseconds that took in all.
    pub fn grab_time(&self) -> (u64, u64) {
        (self.sh.grabs.load(Ordering::Relaxed), self.sh.grab_us.load(Ordering::Relaxed))
    }

    pub fn method(&self) -> &'static str {
        self.method
    }

    pub fn error(&self) -> Option<String> {
        self.sh.error.lock().ok().and_then(|e| e.clone())
    }

    /// Copy from here on (a window that moved; the same size only).
    pub fn move_to(&self, rect: (i32, i32, u32, u32)) {
        *self.sh.moved.lock().unwrap_or_else(|p| p.into_inner()) = Some(rect);
    }
}

#[cfg(zero_copy)]
impl Drop for SurfaceFeed {
    fn drop(&mut self) {
        self.sh.stop.store(true, Ordering::Relaxed);
        if let Some(end) = self.end.take() {
            end();
        }
    }
}

/// A zero-copy feed of `rect` brought to `o.fit` at `o.fps` (Windows: Desktop Duplication and
/// the video processor; macOS: a display stream). Waits up to 2 s for the first picture; fails
/// when any part can't start, and the caller then copies through memory (`screen_feed`).
#[cfg(zero_copy)]
pub fn surface_feed(rect: (i32, i32, u32, u32), o: FeedOptions) -> Result<SurfaceFeed, String> {
    if rect.2 < 2 || rect.3 < 2 {
        return Err("The area to copy is too small.".into());
    }
    let access = |e: String| crate::capture::grab_error_message(&e, crate::capture::screen_access());
    #[cfg(windows)]
    let f = gpu::start(rect, o).map_err(access)?;
    #[cfg(target_os = "macos")]
    let f = display::start(rect, o).map_err(access)?;
    first_surface(f).map_err(access)
}

/// `f` once it has made its first picture (2 s at most), else why not.
#[cfg(zero_copy)]
fn first_surface(f: SurfaceFeed) -> Result<SurfaceFeed, String> {
    let t = Instant::now();
    while f.frames() == 0 && t.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(5));
    }
    if f.frames() == 0 {
        return Err(f.error().unwrap_or_else(|| "no picture came from the zero-copy screen copy".into()));
    }
    Ok(f)
}

/// A moving test picture of `w` × `h` uploaded to the graphics chip and converted there,
/// through the same encoder path as the zero-copy screen copy (`--stream-test`).
#[cfg(windows)]
pub fn test_surface_feed(w: u32, h: u32, fps: u32) -> Result<SurfaceFeed, String> {
    first_surface(gpu::test_feed(w, h, fps)?)
}

/// The graphics chip's BGRA → NV12 conversion against `encode::rgba_to_nv12` (largest Y and
/// U/V differences at the centres of solid patches, and the adapter), for `--stream-test`.
#[cfg(windows)]
pub fn gpu_conversion_check() -> Result<(u8, u8, String), String> {
    gpu::conversion_check()
}

/// Where the picture goes in the video processor's output (left, top, right, bottom): in
/// proportion and centred, as `fit_into` does on the processor.
#[cfg(any(windows, test))]
fn dest_rect(input: (u32, u32), output: (u32, u32)) -> (i32, i32, i32, i32) {
    let (x, y, w, h) = fit_rect(input.0, input.1, output.0, output.1);
    (x as i32, y as i32, (x + w) as i32, (y + h) as i32)
}

/// `D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE`: 16-235 and 0-255.
#[cfg(any(windows, test))]
const NOMINAL_16_235: u32 = 1;
#[cfg(any(windows, test))]
const NOMINAL_0_255: u32 = 2;

/// The bits of `D3D11_VIDEO_PROCESSOR_COLOR_SPACE` (for drivers without
/// `ID3D11VideoContext1`): Usage (bit 0, 0 playback), RGB_Range (bit 1, 0 full 0-255, 1 limited),
/// YCbCr_Matrix (bit 2, 1 BT.709), YCbCr_xvYCC (bit 3, 0), Nominal_Range (bits 4-5).
#[cfg(any(windows, test))]
fn colour_space_bits(rgb_limited: bool, bt709: bool, nominal: u32) -> u32 {
    (rgb_limited as u32) << 1 | (bt709 as u32) << 2 | (nominal & 3) << 4
}

/// Where the planes of a mapped NV12 texture start: Y at 0, the interleaved U/V rows right after
/// the `h` rows of Y, both `pitch` bytes a row.
#[cfg(any(windows, test))]
fn nv12_planes(pitch: usize, h: usize) -> (usize, usize) {
    (0, pitch * h)
}

/// The conversion check's picture: 8 × 4 patches of 32 × 32 pixels in solid colours.
#[cfg(any(windows, test))]
const CHECK_W: u32 = 256;
#[cfg(any(windows, test))]
const CHECK_H: u32 = 128;

#[cfg(any(windows, test))]
const CHECK_COLOURS: [[u8; 3]; 32] = [
    [0, 0, 0], [255, 255, 255], [255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0], [0, 255, 255], [255, 0, 255],
    [128, 128, 128], [64, 64, 64], [192, 192, 192], [16, 16, 16], [235, 235, 235], [200, 100, 50], [50, 100, 200], [100, 200, 50],
    [30, 60, 90], [90, 60, 30], [250, 128, 114], [70, 130, 180], [255, 215, 0], [138, 43, 226], [46, 139, 87], [210, 105, 30],
    [240, 248, 255], [139, 0, 0], [0, 0, 128], [0, 100, 0], [255, 165, 0], [75, 0, 130], [173, 216, 230], [244, 164, 96],
];

#[cfg(any(windows, test))]
fn colour_check_picture() -> Vec<u8> {
    let mut v = Vec::with_capacity((CHECK_W * CHECK_H * 4) as usize);
    for y in 0..CHECK_H {
        for x in 0..CHECK_W {
            let c = CHECK_COLOURS[((y / 32) * 8 + x / 32) as usize];
            v.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    v
}

/// The centre of every patch (an even pixel, so its 2 × 2 chroma block is inside the patch).
#[cfg(any(windows, test))]
fn check_points() -> Vec<(usize, usize)> {
    (0..4).flat_map(|py| (0..8).map(move |px| (px * 32 + 16, py * 32 + 16))).collect()
}

/// Makes the grabber for an area (`open_grab`, or a fake in the tests).
type Opener = fn((i32, i32, u32, u32), bool) -> Result<Box<dyn Grab>, String>;

fn feed_loop(sh: &FeedShared, mut rect: (i32, i32, u32, u32), o: FeedOptions, open: Opener) {
    crate::capture::background_thread(false);
    timely_thread();
    let every = Duration::from_nanos(1_000_000_000 / o.fps.clamp(1, 240) as u64);
    let mut back = Frame::default();
    let mut grab: Option<Box<dyn Grab>> = None;
    let mut force = true;
    let mut next = Instant::now();
    while !sh.stop.load(Ordering::Relaxed) {
        if let Some(r) = sh.moved.lock().ok().and_then(|mut m| m.take())
            && r != rect
        {
            rect = r;
            if grab.as_mut().is_some_and(|g| !g.retarget(r)) {
                grab = None;
            }
            force = true;
        }
        if grab.is_none() {
            match open(rect, o.cursor) {
                Ok(g) => {
                    *sh.method.lock().unwrap_or_else(|p| p.into_inner()) = g.name();
                    force = true;
                    grab = Some(g);
                }
                Err(e) => {
                    *sh.error.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
                    if sh.count.load(Ordering::Relaxed) == 0 {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                    continue;
                }
            }
        }
        let Some(g) = grab.as_mut() else { continue };
        next += every;
        let now = Instant::now();
        if next < now {
            // Fell behind: carry on from now rather than rushing to catch up.
            next = now;
        }
        let r = g.grab(next, force || o.every_frame, &mut back, o.fit);
        // The copy's own time: from the end of its wait (or the frame's start) to done.
        let began = WOKE.with(|w| w.take()).unwrap_or(next.max(now));
        sh.grab_us.fetch_add(began.elapsed().as_micros() as u64, Ordering::Relaxed);
        sh.grabs.fetch_add(1, Ordering::Relaxed);
        match r {
            Ok(true) => {
                force = false;
                back.number = sh.count.load(Ordering::Relaxed) + 1;
                let mut f = sh.front.lock().unwrap_or_else(|p| p.into_inner());
                std::mem::swap(&mut *f, &mut back);
                sh.count.store(f.number, Ordering::Relaxed);
            }
            Ok(false) => {}
            Err(e) => {
                *sh.error.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
                grab = None;
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Windows: DXGI Desktop Duplication + Media Foundation (hardware encoders of every vendor)
// ---------------------------------------------------------------------------------------------

#[cfg(windows)]
mod win {
    use super::*;
    use windows::Win32::Foundation::{HMODULE, RECT};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::Win32::Graphics::Dxgi::*;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
    use windows::core::{GUID, Interface, PCWSTR, PWSTR};

    type R<T> = windows::core::Result<T>;

    /// COM and Media Foundation for this thread, undone on drop.
    struct MfGuard;
    impl MfGuard {
        fn new() -> Result<MfGuard, String> {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|e| format!("Media Foundation isn't available: {e}"))?;
            }
            Ok(MfGuard)
        }
    }
    impl Drop for MfGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = MFShutdown();
            }
        }
    }

    pub(super) fn per_monitor_dpi() {
        unsafe {
            use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    }

    fn subtype(f: VideoFormat) -> GUID {
        match f {
            VideoFormat::Hevc => MFVideoFormat_HEVC,
            VideoFormat::Wmv => MFVideoFormat_WMV3,
            _ => MFVideoFormat_H264,
        }
    }

    fn pack(a: u32, b: u32) -> u64 {
        ((a as u64) << 32) | b as u64
    }

    fn wide(s: &std::path::Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        s.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }

    fn take_string(a: &IMFAttributes, key: &GUID) -> Option<String> {
        unsafe {
            let mut p = PWSTR::null();
            let mut n = 0u32;
            a.GetAllocatedString(key, &mut p, &mut n).ok()?;
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        }
    }

    /// The area to record, on one display, with the device that owns that display.
    pub(super) struct Screen {
        pub(super) adapter: IDXGIAdapter1,
        pub(super) output: IDXGIOutput1,
        /// Area relative to the display's top-left corner.
        pub(super) local: (u32, u32),
    }

    /// The display holding most of the area (DXGI coordinates are physical pixels).
    pub(super) fn find_screen(x: i32, y: i32, w: u32, h: u32) -> R<Option<Screen>> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut best: Option<(i64, Screen)> = None;
            let mut ai = 0;
            while let Ok(adapter) = factory.EnumAdapters1(ai) {
                ai += 1;
                let mut oi = 0;
                while let Ok(out) = adapter.EnumOutputs(oi) {
                    oi += 1;
                    let d = out.GetDesc()?;
                    let r: RECT = d.DesktopCoordinates;
                    let ix = (x + w as i32).min(r.right) - x.max(r.left);
                    let iy = (y + h as i32).min(r.bottom) - y.max(r.top);
                    if ix <= 0 || iy <= 0 {
                        continue;
                    }
                    let rot = d.Rotation;
                    if rot != DXGI_MODE_ROTATION_IDENTITY && rot != DXGI_MODE_ROTATION_UNSPECIFIED {
                        continue;
                    }
                    let area = ix as i64 * iy as i64;
                    if best.as_ref().is_some_and(|b| b.0 >= area) {
                        continue;
                    }
                    let Ok(out1) = out.cast::<IDXGIOutput1>() else { continue };
                    // Keep the area on this display.
                    let lx = (x.max(r.left) - r.left) as u32;
                    let ly = (y.max(r.top) - r.top) as u32;
                    best = Some((area, Screen { adapter: adapter.clone(), output: out1, local: (lx, ly) }));
                }
            }
            Ok(best.map(|b| b.1))
        }
    }

    pub(super) fn device(adapter: &IDXGIAdapter1) -> R<(ID3D11Device, ID3D11DeviceContext)> {
        unsafe {
            let mut dev = None;
            let mut ctx = None;
            D3D11CreateDevice(
                adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut dev),
                None,
                Some(&mut ctx),
            )?;
            let dev = dev.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
            let ctx = ctx.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
            // Media Foundation uses the device from its own threads too.
            if let Ok(mt) = dev.cast::<ID3D11Multithread>() {
                let _ = mt.SetMultithreadProtected(true);
            }
            Ok((dev, ctx))
        }
    }

    pub(super) fn texture(dev: &ID3D11Device, w: u32, h: u32, gdi: bool) -> R<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: if gdi { D3D11_RESOURCE_MISC_GDI_COMPATIBLE.0 as u32 } else { 0 },
        };
        let mut t = None;
        unsafe { dev.CreateTexture2D(&desc, None, Some(&mut t))? };
        t.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))
    }

    /// Where to draw the pointer, in the area's pixels, and which pointer.
    pub(super) struct Pointer {
        hotspots: std::collections::HashMap<isize, (i32, i32)>,
    }
    impl Pointer {
        pub(super) fn new() -> Pointer {
            Pointer { hotspots: Default::default() }
        }

        /// Draw the current pointer onto a device context showing the area at (ox, oy).
        pub(super) fn draw(&mut self, hdc: *mut core::ffi::c_void, ox: i32, oy: i32) {
            use windows_sys::Win32::Graphics::Gdi::DeleteObject;
            use windows_sys::Win32::UI::WindowsAndMessaging::{CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, GetCursorInfo, GetIconInfo, ICONINFO};
            unsafe {
                let mut ci: CURSORINFO = std::mem::zeroed();
                ci.cbSize = std::mem::size_of::<CURSORINFO>() as u32;
                if GetCursorInfo(&mut ci) == 0 || ci.flags & CURSOR_SHOWING == 0 || ci.hCursor.is_null() {
                    return;
                }
                let key = ci.hCursor as isize;
                let hot = *self.hotspots.entry(key).or_insert_with(|| {
                    let mut ii: ICONINFO = std::mem::zeroed();
                    if GetIconInfo(ci.hCursor, &mut ii) != 0 {
                        if !ii.hbmMask.is_null() {
                            DeleteObject(ii.hbmMask);
                        }
                        if !ii.hbmColor.is_null() {
                            DeleteObject(ii.hbmColor);
                        }
                        (ii.xHotspot as i32, ii.yHotspot as i32)
                    } else {
                        (0, 0)
                    }
                });
                DrawIconEx(hdc, ci.ptScreenPos.x - hot.0 - ox, ci.ptScreenPos.y - hot.1 - oy, ci.hCursor, 0, 0, 0, std::ptr::null_mut(), DI_NORMAL);
            }
        }
    }

    /// The sink writer and what it picked.
    struct Writer {
        sw: IMFSinkWriter,
        stream: u32,
        /// The sound stream, when there is sound.
        audio: Option<u32>,
    }

    /// Add an AAC sound stream (48 kHz stereo, 192 kbit/s) fed 16-bit samples.
    fn add_audio(sw: &IMFSinkWriter) -> R<u32> {
        use crate::audio::{CHANNELS, RATE};
        unsafe {
            let out = MFCreateMediaType()?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
            out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
            out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS)?;
            out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000)?;
            let stream = sw.AddStream(&out)?;
            let inp = MFCreateMediaType()?;
            inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            inp.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
            inp.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            inp.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
            inp.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS)?;
            inp.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, CHANNELS * 2)?;
            inp.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, RATE * CHANNELS * 2)?;
            sw.SetInputMediaType(stream, &inp, None)?;
            Ok(stream)
        }
    }

    /// Write `samples` (interleaved stereo) starting at sample frame `at`.
    fn write_audio(wr: &Writer, samples: &[i16], at: u64) -> R<()> {
        let Some(stream) = wr.audio else { return Ok(()) };
        if samples.is_empty() {
            return Ok(());
        }
        unsafe {
            let bytes = (samples.len() * 2) as u32;
            let buf = MFCreateMemoryBuffer(bytes)?;
            let mut dst = std::ptr::null_mut();
            buf.Lock(&mut dst, None, None)?;
            std::ptr::copy_nonoverlapping(samples.as_ptr() as *const u8, dst, bytes as usize);
            buf.Unlock()?;
            buf.SetCurrentLength(bytes)?;
            let s = MFCreateSample()?;
            s.AddBuffer(&buf)?;
            let frames = (samples.len() / crate::audio::CHANNELS as usize) as i64;
            let rate = crate::audio::RATE as i64;
            s.SetSampleTime(at as i64 * 10_000_000 / rate)?;
            s.SetSampleDuration(frames * 10_000_000 / rate)?;
            wr.sw.WriteSample(stream, &s)
        }
    }

    /// Whether to try the graphics chip: Media Foundation's file writer picks the encoder
    /// itself (the screen's graphics chip), so a chosen software encoder means "not the
    /// graphics chip" here; any other choice, or automatic, tries it first.
    fn hardware_for(opts: &VideoOpts) -> bool {
        opts.hardware && !crate::encode::pick_is_software(&opts.encoder)
    }

    fn make_writer(opts: &VideoOpts, w: u32, h: u32, gpu: Option<&ID3D11Device>, hardware: bool, sound: bool) -> R<Writer> {
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 4)?;
            let attrs = attrs.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
            attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, hardware as u32)?;
            if let Some(dev) = gpu {
                let mut token = 0u32;
                let mut mgr = None;
                MFCreateDXGIDeviceManager(&mut token, &mut mgr)?;
                let mgr = mgr.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
                mgr.ResetDevice(dev, token)?;
                attrs.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &mgr)?;
            }
            let path = wide(&opts.path);
            let sw = MFCreateSinkWriterFromURL(PCWSTR(path.as_ptr()), None, &attrs)?;

            let fps = opts.fps.clamp(1, 120);
            let out = MFCreateMediaType()?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out.SetGUID(&MF_MT_SUBTYPE, &subtype(opts.format))?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate(w, h, fps, opts.format, opts.quality))?;
            out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            out.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
            out.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            if opts.format == VideoFormat::Mp4 {
                out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
            }
            let stream = sw.AddStream(&out)?;

            let inp = MFCreateMediaType()?;
            inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            // Texture frames are BGRA surfaces; memory frames are top-down BGRX rows.
            inp.SetGUID(&MF_MT_SUBTYPE, if gpu.is_some() { &MFVideoFormat_ARGB32 } else { &MFVideoFormat_RGB32 })?;
            inp.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            inp.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
            inp.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            inp.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            inp.SetUINT32(&MF_MT_DEFAULT_STRIDE, w * 4)?;
            sw.SetInputMediaType(stream, &inp, None)?;
            let audio = if sound { Some(add_audio(&sw)?) } else { None };
            sw.BeginWriting()?;
            Ok(Writer { sw, stream, audio })
        }
    }

    /// The name of the encoder the sink writer chose, and whether it's a hardware one.
    fn chosen_encoder(sw: &IMFSinkWriter, stream: u32, format: VideoFormat) -> (String, bool) {
        unsafe {
            let Ok(ex) = sw.cast::<IMFSinkWriterEx>() else { return (String::new(), false) };
            let mut i = 0;
            loop {
                let mut cat = GUID::zeroed();
                let mut t: Option<IMFTransform> = None;
                if ex.GetTransformForStream(stream, i, Some(&mut cat), &mut t).is_err() {
                    break;
                }
                i += 1;
                if cat != MFT_CATEGORY_VIDEO_ENCODER {
                    continue;
                }
                let Some(t) = t else { continue };
                let attrs = t.GetAttributes().ok();
                let hw = attrs.as_ref().is_some_and(|a| a.GetStringLength(&MFT_ENUM_HARDWARE_URL_Attribute).is_ok());
                let name = attrs.as_ref().and_then(|a| take_string(a, &MFT_FRIENDLY_NAME_Attribute));
                let name = match name {
                    Some(n) => n,
                    None if hw => {
                        let v = attrs.as_ref().and_then(|a| take_string(a, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute)).unwrap_or_default();
                        // The transform doesn't carry its name; the registered list does.
                        let vendor = vendor_name(&v);
                        let codec = match format {
                            VideoFormat::Hevc => "HEVC",
                            _ => "H.264",
                        };
                        encoders()
                            .into_iter()
                            .find(|e| e.hardware && e.codec == codec && (vendor.is_empty() || e.vendor == vendor))
                            .map(|e| e.name)
                            .unwrap_or_else(|| format!("{vendor} hardware encoder"))
                    }
                    None => match format {
                        VideoFormat::Wmv => "Windows Media Video 9 encoder (software)".into(),
                        VideoFormat::Hevc => "Windows HEVC encoder (software)".into(),
                        _ => "Windows H.264 encoder (software)".into(),
                    },
                };
                return (name, hw);
            }
            (String::new(), false)
        }
    }

    fn write(wr: &Writer, sample: &IMFSample, clock: &Clock, fps: u32) -> R<()> {
        let (t, d) = clock.stamp(fps);
        unsafe {
            sample.SetSampleTime(t)?;
            sample.SetSampleDuration(d)?;
            wr.sw.WriteSample(wr.stream, sample)
        }
    }

    // ---- graphics-chip path: Desktop Duplication → texture → encoder ----

    pub(super) struct Dup {
        pub(super) dev: ID3D11Device,
        pub(super) ctx: ID3D11DeviceContext,
        dup: IDXGIOutputDuplication,
        output: IDXGIOutput1,
        /// Latest picture of the area.
        pub(super) latest: ID3D11Texture2D,
        pub(super) local: (u32, u32),
        pub(super) w: u32,
        pub(super) h: u32,
        /// The screen changed since this was last cleared (the screen feed skips copies of an
        /// unchanged screen).
        pub(super) fresh: bool,
        /// The pointer moved or changed shape since this was last cleared.
        pub(super) mouse: bool,
    }

    impl Dup {
        pub(super) fn new(screen: &Screen, w: u32, h: u32) -> R<Dup> {
            let (dev, ctx) = device(&screen.adapter)?;
            let dup = unsafe { screen.output.DuplicateOutput(&dev)? };
            let latest = texture(&dev, w, h, false)?;
            Ok(Dup { dev, ctx, dup, output: screen.output.clone(), latest, local: screen.local, w, h, fresh: true, mouse: false })
        }

        /// Take screen updates until `until`; returns false when the duplication was lost and
        /// couldn't be made again.
        pub(super) fn pump(&mut self, until: Instant) -> bool {
            loop {
                let now = Instant::now();
                let ms = until.saturating_duration_since(now).as_millis() as u32;
                let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
                let mut res: Option<IDXGIResource> = None;
                match unsafe { self.dup.AcquireNextFrame(ms, &mut info, &mut res) } {
                    Ok(()) => {
                        if info.LastPresentTime != 0
                            && let Some(r) = res.and_then(|r| r.cast::<ID3D11Texture2D>().ok())
                        {
                            let (lx, ly) = self.local;
                            let bx = D3D11_BOX { left: lx, top: ly, front: 0, right: lx + self.w, bottom: ly + self.h, back: 1 };
                            unsafe { self.ctx.CopySubresourceRegion(&self.latest, 0, 0, 0, 0, &r, 0, Some(&bx)) };
                            self.fresh = true;
                        }
                        if info.LastMouseUpdateTime != 0 {
                            self.mouse = true;
                        }
                        unsafe {
                            let _ = self.dup.ReleaseFrame();
                        }
                        if ms == 0 {
                            return true;
                        }
                    }
                    Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return true,
                    Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                        // A full-screen game, a mode change or the lock screen: start again.
                        std::thread::sleep(Duration::from_millis(50));
                        match unsafe { self.output.DuplicateOutput(&self.dev) } {
                            Ok(d) => {
                                self.dup = d;
                                self.fresh = true;
                            }
                            Err(_) => {
                                if Instant::now() >= until {
                                    return true;
                                }
                            }
                        }
                    }
                    Err(_) => return false,
                }
                if Instant::now() >= until {
                    return true;
                }
            }
        }

        /// A new texture holding the latest picture (and the pointer), wrapped as a sample.
        fn sample(&self, pointer: Option<(&mut Pointer, i32, i32)>) -> R<IMFSample> {
            let tex = texture(&self.dev, self.w, self.h, true)?;
            unsafe {
                self.ctx.CopyResource(&tex, &self.latest);
                // Taking the surface's GDI handle waits (idle) for the copy to finish, so the
                // encoder never spins waiting for it: 5 s at 120 fps measured 1.6 s of
                // processor time this way against 3.7 s without.
                let surf: IDXGISurface1 = tex.cast()?;
                if let Ok(hdc) = surf.GetDC(false) {
                    if let Some((p, ox, oy)) = pointer {
                        p.draw(hdc.0, ox, oy);
                    }
                    let _ = surf.ReleaseDC(None);
                }
                let buf = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &tex, 0, false)?;
                let len = buf.cast::<IMF2DBuffer>()?.GetContiguousLength()?;
                buf.SetCurrentLength(len)?;
                let s = MFCreateSample()?;
                s.AddBuffer(&buf)?;
                Ok(s)
            }
        }
    }

    // ---- fallback path: GDI copy → memory → encoder ----

    struct Gdi {
        screen: *mut core::ffi::c_void,
        mem: *mut core::ffi::c_void,
        bmp: *mut core::ffi::c_void,
        old: *mut core::ffi::c_void,
        bits: *mut u8,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    }

    impl Gdi {
        fn new(x: i32, y: i32, w: u32, h: u32) -> Result<Gdi, String> {
            use windows_sys::Win32::Graphics::Gdi::*;
            unsafe {
                let screen = GetDC(std::ptr::null_mut());
                let mem = CreateCompatibleDC(screen);
                let mut bi: BITMAPINFO = std::mem::zeroed();
                bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                bi.bmiHeader.biWidth = w as i32;
                bi.bmiHeader.biHeight = -(h as i32); // top-down rows
                bi.bmiHeader.biPlanes = 1;
                bi.bmiHeader.biBitCount = 32;
                bi.bmiHeader.biCompression = BI_RGB;
                let mut bits = std::ptr::null_mut();
                let bmp = CreateDIBSection(mem, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
                if bmp.is_null() || bits.is_null() {
                    DeleteDC(mem);
                    ReleaseDC(std::ptr::null_mut(), screen);
                    return Err("couldn't make a picture buffer".into());
                }
                let old = SelectObject(mem, bmp);
                Ok(Gdi { screen, mem, bmp, old, bits: bits as *mut u8, x, y, w, h })
            }
        }

        fn sample(&self, pointer: Option<&mut Pointer>) -> R<IMFSample> {
            use windows_sys::Win32::Graphics::Gdi::{BitBlt, SRCCOPY};
            unsafe {
                BitBlt(self.mem, 0, 0, self.w as i32, self.h as i32, self.screen, self.x, self.y, SRCCOPY);
                if let Some(p) = pointer {
                    p.draw(self.mem, self.x, self.y);
                }
                let len = self.w * self.h * 4;
                let buf = MFCreateMemoryBuffer(len)?;
                let mut dst = std::ptr::null_mut();
                buf.Lock(&mut dst, None, None)?;
                std::ptr::copy_nonoverlapping(self.bits, dst, len as usize);
                buf.Unlock()?;
                buf.SetCurrentLength(len)?;
                let s = MFCreateSample()?;
                s.AddBuffer(&buf)?;
                Ok(s)
            }
        }
    }

    impl Drop for Gdi {
        fn drop(&mut self) {
            use windows_sys::Win32::Graphics::Gdi::*;
            unsafe {
                SelectObject(self.mem, self.old);
                DeleteObject(self.bmp);
                DeleteDC(self.mem);
                ReleaseDC(std::ptr::null_mut(), self.screen);
            }
        }
    }

    // ---- the screen feed (live streaming): Desktop Duplication → staging texture → memory ----

    /// `rect` clipped to the display `s` shows: left, top, width, height.
    pub(super) fn clip_to(s: &Screen, rect: (i32, i32, u32, u32)) -> Option<(RECT, (i32, i32, u32, u32))> {
        let r = unsafe { s.output.GetDesc() }.ok()?.DesktopCoordinates;
        clip_rect(r, rect).map(|c| (r, c))
    }

    pub(super) fn clip_rect(r: RECT, (x, y, w, h): (i32, i32, u32, u32)) -> Option<(i32, i32, u32, u32)> {
        let (gx, gy) = (x.max(r.left), y.max(r.top));
        let (gr, gb) = ((x + w as i32).min(r.right), (y + h as i32).min(r.bottom));
        (gr - gx >= 2 && gb - gy >= 2).then(|| (gx, gy, (gr - gx) as u32, (gb - gy) as u32))
    }

    fn staging(dev: &ID3D11Device, w: u32, h: u32) -> R<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut t = None;
        unsafe { dev.CreateTexture2D(&desc, None, Some(&mut t))? };
        t.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))
    }

    /// Desktop Duplication first (the graphics chip keeps the screen; only changed pictures are
    /// read back), GDI when it can't start.
    pub(super) fn open_grab(rect: (i32, i32, u32, u32), cursor: bool) -> Result<Box<dyn Grab>, String> {
        per_monitor_dpi();
        let dup = find_screen(rect.0, rect.1, rect.2, rect.3)
            .map_err(|e| e.to_string())
            .and_then(|s| s.ok_or_else(|| "the area isn't on a screen".to_string()))
            .and_then(|s| DupGrab::new(s, rect, cursor).map_err(|e| e.to_string()));
        match dup {
            Ok(g) => Ok(Box::new(g)),
            Err(e) => GdiGrab::new(rect, cursor).map(|g| Box::new(g) as Box<dyn Grab>).map_err(|g| format!("screen copy: {e}; GDI: {g}")),
        }
    }

    struct DupGrab {
        d: Dup,
        /// The picture with the pointer drawn in (GDI-compatible), when the pointer is wanted.
        composed: Option<ID3D11Texture2D>,
        staging: ID3D11Texture2D,
        pointer: Pointer,
        /// The display's desktop rectangle and where the copied area starts on the desktop.
        display: RECT,
        origin: (i32, i32),
    }

    impl DupGrab {
        fn new(screen: Screen, rect: (i32, i32, u32, u32), cursor: bool) -> R<DupGrab> {
            let Some((display, (gx, gy, w, h))) = clip_to(&screen, rect) else { return Err(windows::core::Error::from(windows::Win32::Foundation::E_INVALIDARG)) };
            let d = Dup::new(&screen, w, h)?;
            let composed = if cursor { Some(texture(&d.dev, w, h, true)?) } else { None };
            let staging = staging(&d.dev, w, h)?;
            Ok(DupGrab { d, composed, staging, pointer: Pointer::new(), display, origin: (gx, gy) })
        }

        /// Map the staging texture, waiting idly for the graphics chip's copy to finish.
        fn map(&self) -> R<D3D11_MAPPED_SUBRESOURCE> {
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            let t0 = Instant::now();
            loop {
                match unsafe { self.d.ctx.Map(&self.staging, 0, D3D11_MAP_READ, D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32, Some(&mut m)) } {
                    Ok(()) => return Ok(m),
                    Err(e) if e.code() == DXGI_ERROR_WAS_STILL_DRAWING && t0.elapsed() < Duration::from_millis(100) => std::thread::sleep(Duration::from_micros(250)),
                    Err(e) if e.code() == DXGI_ERROR_WAS_STILL_DRAWING => {
                        unsafe { self.d.ctx.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut m))? };
                        return Ok(m);
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    }

    impl Grab for DupGrab {
        fn grab(&mut self, until: Instant, force: bool, out: &mut Frame, fit: Option<(u32, u32)>) -> Result<bool, String> {
            if !self.d.pump(until) {
                return Err("the screen copy stopped".into());
            }
            let changed = force || self.d.fresh || (self.composed.is_some() && self.d.mouse);
            self.d.fresh = false;
            self.d.mouse = false;
            if !changed {
                return Ok(false);
            }
            let (w, h) = (self.d.w, self.d.h);
            let e = |e: windows::core::Error| format!("screen copy: {e}");
            unsafe {
                let ctx = &self.d.ctx;
                match &self.composed {
                    Some(c) => {
                        ctx.CopyResource(c, &self.d.latest);
                        let surf: IDXGISurface1 = c.cast().map_err(e)?;
                        if let Ok(hdc) = surf.GetDC(false) {
                            self.pointer.draw(hdc.0, self.origin.0, self.origin.1);
                            let _ = surf.ReleaseDC(None);
                        }
                        ctx.CopyResource(&self.staging, c);
                    }
                    None => ctx.CopyResource(&self.staging, &self.d.latest),
                }
                ctx.Flush();
                let m = self.map().map_err(e)?;
                let pitch = m.RowPitch as usize;
                let data = std::slice::from_raw_parts(m.pData as *const u8, pitch * (h as usize - 1) + w as usize * 4);
                fit_into(&Pixels { data, width: w, height: h, stride: pitch, bgra: true }, out, fit.unwrap_or((w, h)));
                ctx.Unmap(&self.staging, 0);
            }
            Ok(true)
        }

        fn retarget(&mut self, rect: (i32, i32, u32, u32)) -> bool {
            // Same display, same size: only the corner the copy starts from changes.
            match clip_rect(self.display, rect) {
                Some((gx, gy, w, h)) if (w, h) == (self.d.w, self.d.h) => {
                    self.d.local = ((gx - self.display.left) as u32, (gy - self.display.top) as u32);
                    self.origin = (gx, gy);
                    self.d.fresh = true;
                    true
                }
                _ => false,
            }
        }

        fn name(&self) -> &'static str {
            "graphics chip"
        }
    }

    struct GdiGrab {
        g: Gdi,
        pointer: Option<Pointer>,
    }

    impl GdiGrab {
        fn new((x, y, w, h): (i32, i32, u32, u32), cursor: bool) -> Result<GdiGrab, String> {
            Ok(GdiGrab { g: Gdi::new(x, y, w, h)?, pointer: cursor.then(Pointer::new) })
        }
    }

    impl Grab for GdiGrab {
        fn grab(&mut self, until: Instant, _force: bool, out: &mut Frame, fit: Option<(u32, u32)>) -> Result<bool, String> {
            use windows_sys::Win32::Graphics::Gdi::{BitBlt, GdiFlush, SRCCOPY};
            let now = Instant::now();
            if until > now {
                std::thread::sleep(until - now);
            }
            let g = &self.g;
            unsafe {
                BitBlt(g.mem, 0, 0, g.w as i32, g.h as i32, g.screen, g.x, g.y, SRCCOPY);
                if let Some(p) = &mut self.pointer {
                    p.draw(g.mem, g.x, g.y);
                }
                GdiFlush();
                let data = std::slice::from_raw_parts(g.bits, g.w as usize * g.h as usize * 4);
                fit_into(&Pixels { data, width: g.w, height: g.h, stride: g.w as usize * 4, bgra: true }, out, fit.unwrap_or((g.w, g.h)));
            }
            Ok(true)
        }

        fn retarget(&mut self, (x, y, w, h): (i32, i32, u32, u32)) -> bool {
            if (w, h) != (self.g.w, self.g.h) {
                return false;
            }
            self.g.x = x;
            self.g.y = y;
            true
        }

        fn name(&self) -> &'static str {
            "GDI"
        }
    }

    enum Source {
        Gpu(Dup),
        Gdi(Gdi),
        /// Frames made by the caller (Studio mode).
        Custom(FrameFeed, u32, u32),
    }

    fn custom_sample(feed: &FrameFeed, w: u32, h: u32) -> R<Option<IMFSample>> {
        unsafe {
            let len = w * h * 4;
            let buf = MFCreateMemoryBuffer(len)?;
            let mut dst = std::ptr::null_mut();
            buf.Lock(&mut dst, None, None)?;
            let more = feed(std::slice::from_raw_parts_mut(dst, len as usize));
            buf.Unlock()?;
            if !more {
                return Ok(None);
            }
            buf.SetCurrentLength(len)?;
            let s = MFCreateSample()?;
            s.AddBuffer(&buf)?;
            Ok(Some(s))
        }
    }

    /// Try the ways to record from best to plainest; the first that writes a frame is kept.
    pub fn record(opts: &VideoOpts, stop: &AtomicBool, pause: &AtomicBool, done: &mut VideoDone) {
        per_monitor_dpi();
        let _mf = match MfGuard::new() {
            Ok(g) => g,
            Err(e) => {
                done.result = Err(e);
                return;
            }
        };
        let (x, y, w0, h0) = opts.rect;
        // Sound starts first so the devices are ready; what it hears before the first frame
        // is dropped.
        let sound = if let Some(inputs) = opts.sound.as_ref().filter(|i| !i.is_empty() && opts.format != VideoFormat::Wmv) {
            // Studio: every input its scenes use; one that can't start is left out and noted.
            let c = crate::audio::Capture::start_inputs(inputs);
            done.note = studio_sound_note(c.notes());
            Some(c).filter(|c| !c.is_silent())
        } else if opts.audio != crate::audio::AudioSource::None && opts.format != VideoFormat::Wmv {
            let (c, note) = sound_or_note(crate::audio::Capture::start(opts.audio, &opts.mic));
            done.note = note;
            c.filter(|c| !c.is_silent())
        } else {
            None
        };
        if let Some(feed) = &opts.frames {
            let (w, h) = match even_size(w0, h0) {
                Ok(s) => s,
                Err(e) => {
                    done.result = Err(e);
                    return;
                }
            };
            done.width = w;
            done.height = h;
            let mut last_err = String::new();
            let tries: &[bool] = if hardware_for(opts) { &[true, false] } else { &[false] };
            for &hw in tries {
                let _ = std::fs::remove_file(&opts.path);
                let wr = match make_writer(opts, w, h, None, hw, sound.is_some()) {
                    Ok(wr) => wr,
                    Err(e) => {
                        last_err = format!("{e}");
                        continue;
                    }
                };
                match run(opts, Source::Custom(feed.clone(), w, h), &wr, (0, 0), sound.as_ref(), stop, pause, done) {
                    Ok(()) => {
                        done.capture = "Studio";
                        done.result = unsafe { wr.sw.Finalize() }.map_err(|e| format!("couldn't finish the file: {e}"));
                        return;
                    }
                    Err((started, e)) => {
                        last_err = e;
                        if started {
                            let _ = unsafe { wr.sw.Finalize() };
                            done.result = Err(last_err);
                            return;
                        }
                    }
                }
            }
            let _ = std::fs::remove_file(&opts.path);
            done.result = Err(format!("Couldn't record: {last_err}"));
            return;
        }
        // Keep the area on one display (the one holding most of it).
        let screen = find_screen(x, y, w0, h0).ok().flatten();
        let (gx, gy, gw, gh) = match &screen {
            Some(s) => {
                // Clip to that display.
                let d = unsafe { s.output.GetDesc() }.ok();
                match d {
                    Some(d) => {
                        let r = d.DesktopCoordinates;
                        let gx = x.max(r.left);
                        let gy = y.max(r.top);
                        let gr = (x + w0 as i32).min(r.right);
                        let gb = (y + h0 as i32).min(r.bottom);
                        (gx, gy, (gr - gx).max(0) as u32, (gb - gy).max(0) as u32)
                    }
                    None => (x, y, w0, h0),
                }
            }
            None => (x, y, w0, h0),
        };
        let (w, h) = match even_size(gw, gh) {
            Ok(s) => s,
            Err(e) => {
                done.result = Err(e);
                return;
            }
        };
        done.width = w;
        done.height = h;

        // (use the graphics chip to copy, use a hardware encoder)
        let mut tries: Vec<(bool, bool)> = Vec::new();
        let gpu_ok = screen.is_some() && opts.format != VideoFormat::Wmv;
        if hardware_for(opts) && gpu_ok {
            tries.push((true, true));
        }
        if hardware_for(opts) && opts.format != VideoFormat::Wmv {
            tries.push((false, true));
        }
        tries.push((false, false));

        let mut last_err = String::new();
        for (gpu, hw) in tries {
            let _ = std::fs::remove_file(&opts.path);
            let source = if gpu {
                match screen.as_ref().map(|s| Dup::new(s, w, h)) {
                    Some(Ok(d)) => Source::Gpu(d),
                    Some(Err(e)) => {
                        last_err = format!("screen copy: {e}");
                        continue;
                    }
                    None => continue,
                }
            } else {
                match Gdi::new(gx, gy, w, h) {
                    Ok(g) => Source::Gdi(g),
                    Err(e) => {
                        last_err = e;
                        continue;
                    }
                }
            };
            let dev = match &source {
                Source::Gpu(d) => Some(d.dev.clone()),
                Source::Gdi(_) | Source::Custom(..) => None,
            };
            let wr = match make_writer(opts, w, h, dev.as_ref(), hw, sound.is_some()) {
                Ok(wr) => wr,
                Err(e) => {
                    last_err = e.message().to_string();
                    if last_err.is_empty() {
                        last_err = format!("{e}");
                    }
                    continue;
                }
            };
            if let Some(s) = &sound {
                s.discard();
            }
            match run(opts, source, &wr, (gx, gy), sound.as_ref(), stop, pause, done) {
                Ok(()) => {
                    done.capture = if gpu { "graphics chip" } else { "GDI" };
                    if done.encoder.is_empty() {
                        let (n, h) = chosen_encoder(&wr.sw, wr.stream, opts.format);
                        done.encoder = n;
                        done.hardware = h;
                    }
                    let fin = unsafe { wr.sw.Finalize() };
                    done.result = fin.map_err(|e| format!("couldn't finish the file: {e}"));
                    return;
                }
                Err((started, e)) => {
                    last_err = e;
                    if started {
                        // Frames were written: finish what there is rather than lose it.
                        let _ = unsafe { wr.sw.Finalize() };
                        done.result = Err(last_err);
                        return;
                    }
                    drop(wr);
                }
            }
        }
        let _ = std::fs::remove_file(&opts.path);
        done.result = Err(match opts.format {
            VideoFormat::Hevc => format!("This computer can't make HEVC videos ({last_err}). Pick MP4 (H.264) instead."),
            _ => format!("Couldn't record: {last_err}"),
        });
    }

    /// The recording loop. Err((frames were written, why)).
    fn run(
        opts: &VideoOpts,
        mut source: Source,
        wr: &Writer,
        origin: (i32, i32),
        sound: Option<&crate::audio::Capture>,
        stop: &AtomicBool,
        pause: &AtomicBool,
        done: &mut VideoDone,
    ) -> Result<(), (bool, String)> {
        let rate = crate::audio::RATE as u64;
        let mut audio_at: u64 = 0;
        let fps = opts.fps.clamp(1, 120);
        let mut clock = Clock::new(fps);
        let mut pointer = Pointer::new();
        let mut first = true;
        // The first picture comes at once.
        if let Source::Gpu(d) = &mut source
            && !d.pump(Instant::now() + Duration::from_millis(300))
        {
            return Err((false, "the screen copy stopped".into()));
        }
        loop {
            if !first && (stop.load(Ordering::Relaxed) || clock.over(opts.max_s)) {
                break;
            }
            if !first && clock.hold(pause) {
                while pause.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(30));
                }
                if let Some(s) = sound {
                    s.discard();
                }
                continue;
            }
            let deadline = clock.deadline();
            let sample = match &mut source {
                Source::Custom(feed, w, h) => {
                    let now = Instant::now();
                    if !first && deadline > now {
                        std::thread::sleep(deadline - now);
                    }
                    match custom_sample(feed, *w, *h) {
                        Ok(Some(s)) => Ok(s),
                        Ok(None) => break,
                        Err(e) => Err(e),
                    }
                }
                Source::Gpu(d) => {
                    if !first && !d.pump(deadline) {
                        return Err((true, "the screen copy stopped".into()));
                    }
                    d.sample(opts.cursor.then_some((&mut pointer, origin.0, origin.1)))
                }
                Source::Gdi(g) => {
                    let now = Instant::now();
                    if !first && deadline > now {
                        std::thread::sleep(deadline - now);
                    }
                    g.sample(opts.cursor.then_some(&mut pointer))
                }
            };
            let r = sample.and_then(|s| write(wr, &s, &clock, fps));
            if let Err(e) = r {
                return Err((!first, format!("{e}")));
            }
            // Sound up to the end of this frame.
            if let Some(s) = sound {
                if first {
                    s.discard();
                }
                let due = (clock.n + 1) * rate / fps as u64;
                if due > audio_at {
                    let pcm = s.take((due - audio_at) as usize);
                    if let Err(e) = write_audio(wr, &pcm, audio_at) {
                        return Err((true, format!("sound: {e}")));
                    }
                    audio_at = due;
                }
            }
            if first {
                let (n, h) = chosen_encoder(&wr.sw, wr.stream, opts.format);
                done.encoder = n;
                done.hardware = h;
                first = false;
                done.frames += 1;
                // Starting the encoder takes a moment; the clock starts after it.
                clock.n += 1;
                clock.recorded += clock.interval;
                clock.base = Instant::now();
                clock.n_base = clock.n;
                continue;
            }
            done.frames += 1;
            clock.advance();
        }
        done.seconds = clock.recorded.as_secs_f32();
        Ok(())
    }

    /// Hardware and software encoders for H.264, HEVC and WMV.
    pub fn encoders() -> Vec<EncoderInfo> {
        let _mf = match MfGuard::new() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        let mut v = Vec::new();
        for (codec, sub) in [("H.264", MFVideoFormat_H264), ("HEVC", MFVideoFormat_HEVC), ("WMV", MFVideoFormat_WMV3)] {
            let info = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: sub };
            for hw in [true, false] {
                let flags = if hw {
                    MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER
                } else {
                    MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER
                };
                unsafe {
                    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
                    let mut n = 0u32;
                    if MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, flags, None, Some(&info), &mut list, &mut n).is_err() || list.is_null() {
                        continue;
                    }
                    for i in 0..n as usize {
                        let a = std::ptr::read(list.add(i));
                        if let Some(a) = a {
                            let attrs: IMFAttributes = match a.cast() {
                                Ok(x) => x,
                                Err(_) => continue,
                            };
                            let name = take_string(&attrs, &MFT_FRIENDLY_NAME_Attribute).unwrap_or_else(|| format!("{codec} encoder"));
                            let vendor = take_string(&attrs, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute).map(|v| vendor_name(&v)).unwrap_or_default();
                            if !v.iter().any(|e: &EncoderInfo| e.name == name && e.codec == codec) {
                                v.push(EncoderInfo { name, codec, hardware: hw, vendor });
                            }
                        }
                    }
                    CoTaskMemFree(Some(list as *const _));
                }
            }
        }
        v
    }

    /// Decode the file to count its frames and keep the first one.
    pub fn probe(path: &std::path::Path) -> Result<Probe, String> {
        let _mf = MfGuard::new()?;
        let e = |e: windows::core::Error| e.to_string();
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 1).map_err(e)?;
            let attrs = attrs.ok_or("no attributes")?;
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1).map_err(e)?;
            let p = wide(path);
            let rd = MFCreateSourceReaderFromURL(PCWSTR(p.as_ptr()), &attrs).map_err(e)?;
            let vs = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            let audio = rd.GetNativeMediaType(MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32, 0).is_ok();
            let _ = rd.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
            let _ = rd.SetStreamSelection(vs, true);
            let t = MFCreateMediaType().map_err(e)?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(e)?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(e)?;
            rd.SetCurrentMediaType(vs, None, &t).map_err(e)?;
            let cur = rd.GetCurrentMediaType(vs).map_err(e)?;
            let size = cur.GetUINT64(&MF_MT_FRAME_SIZE).map_err(e)?;
            let (w, h) = ((size >> 32) as u32, size as u32);
            let mut frames = 0u64;
            let mut last_t = 0i64;
            let mut first = None;
            loop {
                let mut flags = 0u32;
                let mut ts = 0i64;
                let mut s: Option<IMFSample> = None;
                rd.ReadSample(vs, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut s)).map_err(e)?;
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                let Some(s) = s else { continue };
                frames += 1;
                last_t = ts + s.GetSampleDuration().unwrap_or(0);
                let dump = std::env::var_os("SNAGSTER_VIDEO_DUMP").is_some() && [2u64, 3, 11, 61].contains(&frames);
                if first.is_none() || dump {
                    let mut img = image::RgbaImage::new(w, h);
                    let mut copy = |p0: *const u8, pitch: isize| {
                        for yy in 0..h {
                            let row = p0.offset(yy as isize * pitch);
                            for xx in 0..w {
                                let px = row.add(xx as usize * 4);
                                img.put_pixel(xx, yy, image::Rgba([*px.add(2), *px.add(1), *px, 255]));
                            }
                        }
                    };
                    let b0 = s.GetBufferByIndex(0).map_err(e)?;
                    if let Ok(b2) = b0.cast::<IMF2DBuffer>() {
                        // A picture buffer says where its top row is and how far apart rows are.
                        let mut p0 = std::ptr::null_mut();
                        let mut pitch = 0i32;
                        b2.Lock2D(&mut p0, &mut pitch).map_err(e)?;
                        copy(p0, pitch as isize);
                        let _ = b2.Unlock2D();
                    } else {
                        // A plain buffer: rows run top-down when the stride is positive, else bottom-up.
                        let buf = s.ConvertToContiguousBuffer().map_err(e)?;
                        let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|v| v as i32).unwrap_or(-(w as i32 * 4));
                        let mut p = std::ptr::null_mut();
                        let mut len = 0u32;
                        buf.Lock(&mut p, None, Some(&mut len)).map_err(e)?;
                        if len as usize >= (w * h * 4) as usize {
                            if stride < 0 {
                                copy(p.add(((h - 1) * w * 4) as usize), stride as isize);
                            } else {
                                copy(p, stride as isize);
                            }
                        }
                        let _ = buf.Unlock();
                    }
                    if dump {
                        let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                        let _ = img.save(std::env::temp_dir().join(format!("{stem}-frame{}.png", frames - 1)));
                    }
                    if first.is_none() {
                        first = Some(img);
                    }
                }
            }
            Ok(Probe { width: w, height: h, frames, seconds: last_t as f64 / 1e7, first, audio })
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Linux and macOS: xcap screen grabs + OpenH264 (software) into an MP4 file
// ---------------------------------------------------------------------------------------------

#[cfg(not(windows))]
mod soft {
    use super::*;

    pub fn record(opts: &VideoOpts, stop: &AtomicBool, pause: &AtomicBool, done: &mut VideoDone) {
        let (x, y, w0, h0) = opts.rect;
        let (w, h) = match even_size(w0, h0) {
            Ok(s) => s,
            Err(e) => {
                done.result = Err(e);
                return;
            }
        };
        done.width = w;
        done.height = h;
        done.capture = "screen grab";
        if opts.audio != crate::audio::AudioSource::None {
            // Sound recording is Windows-only so far; say so instead of leaving it out quietly.
            done.note = super::sound_or_note::<()>(Err("sound recording isn't available on this system yet".into())).1;
        }
        done.result = encode(opts, (x, y, w, h), stop, pause, done);
    }

    fn encode(opts: &VideoOpts, rect: (i32, i32, u32, u32), stop: &AtomicBool, pause: &AtomicBool, done: &mut VideoDone) -> Result<(), String> {
        use crate::encode::{Codec, VideoConfig};
        let (x, y, w, h) = rect;
        let fps = opts.fps.clamp(1, 120);
        // The graphics chip's encoder when there is one (VideoToolbox, VA-API), else OpenH264.
        let cfg = VideoConfig {
            width: w,
            height: h,
            fps,
            bitrate_bps: bitrate(w, h, fps, opts.format, opts.quality),
            keyframe_every_s: 2,
            codec: Codec::H264,
            prefer_hardware: opts.hardware,
            low_latency: true,
        };
        let (mut enc, fell_back) = crate::encode::open_video_picked(&cfg, &opts.encoder)?;
        if let Some(n) = fell_back {
            done.note = if done.note.is_empty() { n } else { format!("{} {n}", done.note) };
        }
        let name = enc.name();
        done.encoder = name.name;
        done.hardware = name.hardware;
        let file = std::fs::File::create(&opts.path).map_err(|e| e.to_string())?;
        let mut mux = Mux::new(std::io::BufWriter::new(file), w, h)?;
        let mut spacer = Spacer::new(fps);
        let mut first = true;
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        // The screen through the best copy here (the same one streaming uses: X11 shared memory
        // on Linux, CoreGraphics on macOS), not a capture-library grab per picture.
        let mut grab = if opts.frames.is_none() { Some(super::open_grab((x, y, w, h), opts.cursor)?) } else { None };
        // The clock starts once the screen is there: on Wayland opening it waits for the person
        // in the desktop's "Share screen" dialog, and that wait isn't part of the video.
        let mut clock = Clock::new(fps);
        super::RECORDING_LIVE.store(true, Ordering::Relaxed);
        if let Some(g) = &grab {
            done.capture = g.name();
        }
        let mut frame = Frame::default();
        loop {
            if !first && (stop.load(Ordering::Relaxed) || clock.over(opts.max_s)) {
                break;
            }
            if !first && clock.hold(pause) {
                while pause.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(30));
                }
                continue;
            }
            let now = Instant::now();
            let deadline = clock.deadline();
            if !first && deadline > now {
                std::thread::sleep(deadline - now);
            }
            if let Some(feed) = &opts.frames {
                // Studio frames come as BGRA.
                if !feed(&mut rgba) {
                    break;
                }
                for p in rgba.chunks_exact_mut(4) {
                    p.swap(0, 2);
                }
            } else if let Some(g) = grab.as_mut() {
                g.grab(Instant::now(), true, &mut frame, Some((w, h))).map_err(|e| crate::capture::grab_error_message(&e, crate::capture::screen_access()))?;
                if frame.rgba.len() == rgba.len() {
                    rgba.copy_from_slice(&frame.rgba);
                }
            }
            let (t, _) = clock.stamp(fps);
            for p in enc.encode(&rgba, t / 10, false)? {
                if let Some((p, d)) = spacer.push(p) {
                    mux.frame(&p, d)?;
                }
            }
            first = false;
            done.frames += 1;
            clock.advance();
        }
        for p in enc.finish()? {
            if let Some((p, d)) = spacer.push(p) {
                mux.frame(&p, d)?;
            }
        }
        if let Some((p, d)) = spacer.last() {
            mux.frame(&p, d)?;
        }
        mux.finish()?;
        done.seconds = clock.recorded.as_secs_f32();
        Ok(())
    }

    /// Writes H.264 frames (Annex-B access units) into an MP4 file with the `mp4` crate; the
    /// track starts once the first SPS and PPS have come.
    pub(super) struct Mux<W: std::io::Write + std::io::Seek> {
        w: Option<W>,
        mp4: Option<mp4::Mp4Writer<W>>,
        width: u32,
        height: u32,
        sps: Vec<u8>,
        pps: Vec<u8>,
    }

    impl<W: std::io::Write + std::io::Seek> Mux<W> {
        pub(super) fn new(w: W, width: u32, height: u32) -> Result<Self, String> {
            Ok(Mux { w: Some(w), mp4: None, width, height, sps: Vec::new(), pps: Vec::new() })
        }

        /// One frame lasting `d` (100 ns units).
        pub(super) fn frame(&mut self, p: &crate::encode::VideoPacket, d: i64) -> Result<(), String> {
            let (data, sps, pps) = avcc_sample(&p.data);
            if let Some(s) = sps {
                self.sps = s;
            }
            if let Some(s) = pps {
                self.pps = s;
            }
            if self.mp4.is_none() {
                if self.sps.is_empty() || self.pps.is_empty() {
                    return Ok(());
                }
                let config = mp4::Mp4Config {
                    major_brand: mp4::FourCC::from(*b"isom"),
                    minor_version: 512,
                    compatible_brands: vec![
                        mp4::FourCC::from(*b"isom"),
                        mp4::FourCC::from(*b"iso2"),
                        mp4::FourCC::from(*b"avc1"),
                        mp4::FourCC::from(*b"mp41"),
                    ],
                    timescale: 10_000_000,
                };
                let w = self.w.take().ok_or("writer gone")?;
                let mut m = mp4::Mp4Writer::write_start(w, &config).map_err(|e| e.to_string())?;
                m.add_track(&mp4::TrackConfig {
                    track_type: mp4::TrackType::Video,
                    timescale: 10_000_000,
                    language: "und".into(),
                    media_conf: mp4::MediaConfig::AvcConfig(mp4::AvcConfig {
                        width: self.width as u16,
                        height: self.height as u16,
                        seq_param_set: self.sps.clone(),
                        pic_param_set: self.pps.clone(),
                    }),
                })
                .map_err(|e| e.to_string())?;
                self.mp4 = Some(m);
            }
            if data.is_empty() {
                return Ok(());
            }
            let m = self.mp4.as_mut().ok_or("no file")?;
            m.write_sample(
                1,
                &mp4::Mp4Sample { start_time: (p.pts_us * 10) as u64, duration: d.max(1) as u32, rendering_offset: 0, is_sync: p.keyframe, bytes: data.into() },
            )
            .map_err(|e| e.to_string())
        }

        pub(super) fn finish(mut self) -> Result<(), String> {
            match self.mp4.take() {
                Some(mut m) => m.write_end().map_err(|e| e.to_string()),
                None => Err("no frames were recorded".into()),
            }
        }
    }

    pub fn probe(path: &std::path::Path) -> Result<Probe, String> {
        let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let size = f.metadata().map_err(|e| e.to_string())?.len();
        super::mp4_probe(std::io::BufReader::new(f), size)
    }
}

/// What an MP4 file's header says, read with the mp4 crate (how Linux and macOS look at a
/// video; Windows asks Media Foundation). Built into Windows tests too, for the fuzz target.
#[cfg(any(not(windows), test))]
fn mp4_probe<R: std::io::Read + std::io::Seek>(reader: R, size: u64) -> Result<Probe, String> {
    let r = mp4::Mp4Reader::read_header(Bounded::new(reader, size), size).map_err(|e| e.to_string())?;
    let t = r.tracks().get(&1).ok_or("no video track")?;
    Ok(Probe {
        width: t.width() as u32,
        height: t.height() as u32,
        frames: t.sample_count() as u64,
        seconds: t.duration().as_secs_f64(),
        first: None,
        audio: r.tracks().len() > 1,
    })
}

/// A file read for the mp4 crate that fails once far more was read, or the place moved far
/// more often, than reading a file of this size can need. The crate reads some broken files in
/// circles for ever (a box of size 0 inside `moov` sends it back to the same header again and
/// again); this turns that into an error.
#[cfg(any(not(windows), test))]
struct Bounded<R> {
    inner: R,
    read_left: u64,
    seeks_left: u64,
}

#[cfg(any(not(windows), test))]
impl<R> Bounded<R> {
    fn new(inner: R, size: u64) -> Self {
        // Every byte read about once, every box looked at a few times, and room to spare.
        Bounded { inner, read_left: size.saturating_mul(2).saturating_add(64 * 1024), seeks_left: (size / 2).saturating_add(4096) }
    }

    fn circles() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "the file is broken (reading it goes round in circles)")
    }
}

#[cfg(any(not(windows), test))]
impl<R: std::io::Read> std::io::Read for Bounded<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.read_left == 0 {
            return Err(Self::circles());
        }
        let n = self.inner.read(buf)?;
        self.read_left = self.read_left.saturating_sub(n.max(1) as u64);
        Ok(n)
    }
}

#[cfg(any(not(windows), test))]
impl<R: std::io::Seek> std::io::Seek for Bounded<R> {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        if self.seeks_left == 0 {
            return Err(Self::circles());
        }
        self.seeks_left -= 1;
        self.inner.seek(to)
    }
}

/// Fuzz target (`src/fuzz.rs`): a video file someone hands to video_info or the editor.
#[cfg(test)]
pub(crate) mod fuzzing {
    use super::*;
    use crate::fuzz::Target;

    /// A small MP4 with two H.264 samples (the parameter sets are plausible, not decodable).
    pub fn small_mp4() -> Vec<u8> {
        let config = mp4::Mp4Config { major_brand: mp4::FourCC::from(*b"isom"), minor_version: 512, compatible_brands: vec![mp4::FourCC::from(*b"isom"), mp4::FourCC::from(*b"avc1")], timescale: 1000 };
        let mut m = mp4::Mp4Writer::write_start(std::io::Cursor::new(Vec::new()), &config).unwrap();
        m.add_track(&mp4::TrackConfig {
            track_type: mp4::TrackType::Video,
            timescale: 1000,
            language: "und".into(),
            media_conf: mp4::MediaConfig::AvcConfig(mp4::AvcConfig { width: 16, height: 16, seq_param_set: vec![0x67, 0x42, 0xC0, 0x1F, 0xDA], pic_param_set: vec![0x68, 0xCE, 0x3C, 0x80] }),
        })
        .unwrap();
        for i in 0..2u64 {
            m.write_sample(1, &mp4::Mp4Sample { start_time: i * 33, duration: 33, rendering_offset: 0, is_sync: i == 0, bytes: vec![0, 0, 0, 4, 0x65, 1, 2, 3].into() }).unwrap();
        }
        m.write_end().unwrap();
        m.into_writer().into_inner()
    }

    pub fn targets() -> Vec<Target> {
        vec![Target {
            name: "video_probe",
            seeds: || vec![small_mp4()],
            words: || ["ftyp", "moov", "mvhd", "trak", "tkhd", "mdia", "mdhd", "hdlr", "minf", "stbl", "stsd", "avc1", "avcC", "stts", "stsz", "stsc", "stco", "co64", "mdat", "\u{ff}\u{ff}\u{ff}\u{ff}"].iter().map(|s| s.to_string()).collect(),
            run: |b| {
                let _ = mp4_probe(std::io::Cursor::new(b), b.len() as u64);
            },
            budget: |n| (16 << 20) + 64 * n,
        }]
    }

    /// Bug (found by the fuzz test): a 658-byte file whose `moov` held a box of size 0 after
    /// `mvhd` made the mp4 crate read the same header for ever, so video_info (and opening the
    /// video on Linux and macOS) never came back.
    #[test]
    fn a_box_of_size_zero_is_an_error_not_a_hang() {
        let good = small_mp4();
        assert_eq!(mp4_probe(std::io::Cursor::new(&good), good.len() as u64).unwrap().width, 16);
        let at = |name: &[u8]| good.windows(4).position(|w| w == name).unwrap() - 4;
        let size = |o: usize| u32::from_be_bytes(good[o..o + 4].try_into().unwrap()) as usize;
        let (moov, mvhd) = (at(b"moov"), at(b"mvhd"));
        let mut broken = good[..moov].to_vec();
        let inner = [&good[mvhd..mvhd + size(mvhd)], &[0, 0, 0, 0, b'f', b'r', b'e', b'e'][..]].concat();
        broken.extend(((inner.len() + 8) as u32).to_be_bytes());
        broken.extend(b"moov");
        broken.extend(inner);
        let t0 = std::time::Instant::now();
        let e = mp4_probe(std::io::Cursor::new(&broken), broken.len() as u64).unwrap_err();
        assert!(t0.elapsed() < std::time::Duration::from_secs(5), "{:?}", t0.elapsed());
        assert!(e.contains("circles"), "{e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// X11 on X11; on Wayland and in a Flatpak screen sharing, XWayland only when that fails, and
    /// never a desktop screenshot per picture on Wayland.
    #[test]
    fn linux_copies_through_x11_or_screen_sharing() {
        assert!(!use_portal(false, false), "plain X11 copies directly");
        assert!(use_portal(true, false), "Wayland shares the screen through the portal");
        assert!(use_portal(false, true), "a Flatpak only gets the screen through the portal");
        assert!(per_picture_screenshots(false), "X11 copies are cheap: the last resort may grab per picture");
        assert!(!per_picture_screenshots(true), "never a desktop screenshot per picture on Wayland");
    }

    #[test]
    fn a_failed_recording_leaves_no_empty_file() {
        let dir = std::env::temp_dir().join(format!("snagster-empty-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let empty = dir.join("cancelled.mp4");
        let kept = dir.join("half.mp4");
        std::fs::write(&empty, b"").unwrap();
        std::fs::write(&kept, b"some pictures").unwrap();
        remove_empty(&empty);
        remove_empty(&kept);
        remove_empty(&dir.join("never-made.mp4"));
        assert!(!empty.exists(), "nothing was recorded: no file");
        assert!(kept.exists(), "pictures already written are kept");
        // A file the caller already had is never touched, even an empty one: record() checks
        // that before recording and skips remove_empty then.
        let theirs = dir.join("theirs.mp4");
        std::fs::write(&theirs, b"").unwrap();
        let opts = VideoOpts::screen((0, 0, 64, 64), 30, VideoFormat::Gif, theirs.clone());
        let done = record(&opts, &AtomicBool::new(false), &AtomicBool::new(false));
        assert!(done.result.is_err(), "GIFs fail here at once");
        assert!(theirs.exists(), "an empty file that was there before stays");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Found on KDE Plasma: Cancel in the share dialog fell back to XWayland and saved a black
    /// video. XWayland only where the desktop has no portal at all (WSLg).
    #[test]
    fn a_cancelled_share_never_records_xwaylands_black_screen() {
        let asked = std::cell::Cell::new(false);
        let has_portal = || {
            asked.set(true);
            true
        };
        assert!(!x11_after_portal(true, true, has_portal), "Cancel or a refusal on GNOME/KDE: no XWayland");
        assert!(x11_after_portal(true, true, || false), "no portal at all (WSLg): XWayland is all there is");
        asked.set(false);
        assert!(x11_after_portal(false, false, has_portal), "X11 copies directly");
        assert!(!asked.get(), "the portal is only asked about after it failed on Wayland");
    }

    fn solid_px(w: u32, h: u32, p: [u8; 4]) -> Vec<u8> {
        (0..w * h).flat_map(|_| p).collect()
    }

    #[test]
    fn same_size_copies_turn_bgra_into_opaque_rgba() {
        // Blue, green, red, alpha 0 (what GDI leaves) → red, green, blue, opaque.
        let src = solid_px(4, 2, [10, 20, 30, 0]);
        let mut out = Frame::default();
        fit_into(&Pixels { data: &src, width: 4, height: 2, stride: 16, bgra: true }, &mut out, (4, 2));
        assert_eq!((out.width, out.height, out.rgba.len()), (4, 2, 32));
        assert!(out.rgba.chunks(4).all(|p| p == [30, 20, 10, 255]), "{:?}", &out.rgba[..8]);
        fit_into(&Pixels { data: &src, width: 4, height: 2, stride: 16, bgra: false }, &mut out, (4, 2));
        assert!(out.rgba.chunks(4).all(|p| p == [10, 20, 30, 255]));
    }

    #[test]
    fn the_pacer_sleeps_less_by_what_sleeps_overrun_and_yields_the_rest() {
        let ms = Duration::from_millis;
        // Sleeps on time: sleep nearly all of it.
        assert_eq!(next_wait(ms(33), Duration::ZERO), Wait::Sleep(ms(33) - Duration::from_micros(200)));
        // Sleeps overrun by 25 ms (the CI Mac): sleep 8 ms less the margin, then yield.
        assert_eq!(next_wait(ms(33), ms(25)), Wait::Sleep(ms(8) - Duration::from_micros(200)));
        assert_eq!(next_wait(ms(20), ms(25)), Wait::Yield);
        assert_eq!(next_wait(Duration::from_micros(300), Duration::ZERO), Wait::Yield);
        assert_eq!(next_wait(Duration::ZERO, ms(5)), Wait::Done);
    }

    #[test]
    fn the_pacer_learns_overruns_quickly_and_forgets_them_slowly() {
        let ms = Duration::from_millis;
        let mut p = Pacer::default();
        for _ in 0..10 {
            p.learn(ms(25));
        }
        assert!(p.overshoot > ms(24) && p.overshoot <= ms(25), "{:?}", p.overshoot);
        p.learn(Duration::ZERO);
        assert!(p.overshoot > ms(23), "one quick wake-up changes little: {:?}", p.overshoot);
        // A stall of seconds counts only as the most.
        let mut q = Pacer::default();
        for _ in 0..20 {
            q.learn(Duration::from_secs(2));
        }
        assert!(q.overshoot <= Pacer::MOST && q.overshoot > ms(59), "{:?}", q.overshoot);
        // A timer overrunning by more than a 30 fps frame (37 ms, seen on the CI Mac): no sleep
        // can end in time, so it yields; with a 25 ms overrun a short sleep still fits.
        let mut r = Pacer::default();
        for _ in 0..10 {
            r.learn(ms(37));
        }
        assert_eq!(next_wait(ms(33), r.overshoot), Wait::Yield);
        assert!(matches!(next_wait(ms(33), ms(25)), Wait::Sleep(_)));
    }

    #[test]
    fn the_pacer_switches_to_coarse_when_sleeps_start_overrunning_and_back_with_slack() {
        let ms = |n| Duration::from_millis(n);
        // Starts precise (as on a Mac whose window is in front)...
        let mut p = Pacer::default();
        assert!(!coarse_after(false, p.overshoot));
        // ...then the window goes behind others and 5 ms sleeps take 32 ms (seen on a real Mac).
        p.learn(ms(27));
        assert!(coarse_after(false, p.overshoot), "{:?}", p.overshoot);
        // Near the line it stays where it is (no flipping every frame).
        assert!(coarse_after(true, Duration::from_micros(1500)));
        assert!(!coarse_after(false, Duration::from_micros(1500)));
        // Back to plain sleeps once the overrun is well under a millisecond.
        assert!(!coarse_after(true, Duration::from_micros(400)));
    }

    #[test]
    fn the_pacing_mode_reads_for_a_person() {
        assert_eq!(pacer_mode(Duration::from_micros(300)), "precise sleeps (overrun 0.3 ms)");
        assert_eq!(pacer_mode(Duration::from_millis(25)), "coarse timer (sleeps overrun 25 ms): sleeps cut short, the rest yielded");
        assert!(Pacer::calibrated().overshoot <= Pacer::MOST);
    }

    #[test]
    fn the_pacer_never_wakes_early() {
        let mut p = Pacer::default();
        for _ in 0..5 {
            let until = Instant::now() + Duration::from_millis(3);
            p.wait_until(until);
            assert!(Instant::now() >= until);
        }
        // A moment already gone: back at once.
        let t = Instant::now();
        p.wait_until(t - Duration::from_millis(1));
        assert!(t.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn the_copied_area_is_clipped_to_the_screen() {
        let screen = (0, 0, 1920, 1080);
        assert_eq!(clip_area((100, 50, 640, 360), screen), Some((100, 50, 640, 360)));
        assert_eq!(clip_area((-10, -20, 100, 100), screen), Some((0, 0, 90, 80)));
        assert_eq!(clip_area((1900, 1000, 100, 100), screen), Some((1900, 1000, 20, 80)));
        assert_eq!(clip_area((1919, 0, 100, 100), screen), None, "1 pixel wide is too little");
        assert_eq!(clip_area((3000, 0, 100, 100), screen), None);
        // A second screen left of the first, at negative desktop pixels.
        assert_eq!(clip_area((-1500, 10, 2000, 100), (-1280, 0, 1280, 1024)), Some((-1280, 10, 1280, 100)));
        assert_eq!(clip_area((i32::MAX - 1, 0, u32::MAX, 10), screen), None, "no overflow");
    }

    #[test]
    fn the_pointer_lands_where_the_area_is_drawn_in_the_frame() {
        // The same size: where it is.
        assert_eq!(pointer_in_frame((10.0, 20.0), (1280.0, 720.0), (1280, 720)), (10.0, 20.0));
        // 1920 × 1080 into 1280 × 720: two thirds.
        assert_eq!(pointer_in_frame((960.0, 540.0), (1920.0, 1080.0), (1280, 720)), (640.0, 360.0));
        // 1024 × 768 (4:3) into 1280 × 720: 960 × 720 in the middle, 160 px bars at the sides.
        assert_eq!(pointer_in_frame((0.0, 0.0), (1024.0, 768.0), (1280, 720)), (160.0, 0.0));
        assert_eq!(pointer_in_frame((1024.0, 768.0), (1024.0, 768.0), (1280, 720)), (1120.0, 720.0));
        // Macs: an area in points (1440 × 900) and a frame in pixels.
        assert_eq!(pointer_in_frame((720.0, 450.0), (1440.0, 900.0), (2880, 1800)), (1440.0, 900.0));
        // Outside the area: outside the picture (stamp_pointer leaves it out).
        assert!(pointer_in_frame((-50.0, 0.0), (100.0, 100.0), (100, 100)).0 < -16.0);
        assert!(pointer_in_frame((5.0, 5.0), (0.0, 100.0), (100, 100)).0 < -16.0, "an empty area draws no pointer");
    }

    #[test]
    fn shared_memory_segments_are_whole_pages_big_enough_for_the_picture() {
        assert_eq!(shm_segment_bytes(1920, 1080), 1920 * 1080 * 4);
        // A 4K screen: 33 MB, far below Linux's default SHMMAX (unlimited since 3.16) and
        // SHMALL; a system that refuses it gets plain GetImage copies instead.
        assert_eq!(shm_segment_bytes(3840, 2160), 33_177_600);
        assert_eq!(shm_segment_bytes(1, 1), 4096);
        assert_eq!(shm_segment_bytes(0, 0), 4096, "never empty");
        let odd = shm_segment_bytes(1279, 719);
        assert!(odd >= 1279 * 719 * 4 && odd % 4096 == 0 && odd - 1279 * 719 * 4 < 4096);
        // The biggest X allows (65535 × 65535) doesn't overflow.
        assert!(shm_segment_bytes(65535, 65535) >= 65535 * 65535 * 4);
    }

    #[test]
    fn x11_screens_read_as_bgra_only_in_32_bit_pixels_low_byte_first() {
        assert!(x11_bgra(24, 32, true));
        assert!(x11_bgra(32, 32, true));
        assert!(!x11_bgra(24, 24, true), "packed 3-byte pixels");
        assert!(!x11_bgra(16, 16, true));
        assert!(!x11_bgra(24, 32, false), "a big-endian server");
    }

    #[test]
    fn core_graphics_pixel_layouts() {
        let little = 2 << 12;
        // What a Mac screen gives: 32-bit little-endian, alpha (or padding) first: BGRA.
        assert_eq!(cg_bgra(32, little | 6), Some(true));
        assert_eq!(cg_bgra(32, little | 2), Some(true));
        // Big-endian or default order with alpha last: RGBA.
        assert_eq!(cg_bgra(32, 1), Some(false));
        assert_eq!(cg_bgra(32, (4 << 12) | 5), Some(false));
        // Others go to xcap.
        assert_eq!(cg_bgra(64, little | 6), None, "16 bits a channel");
        assert_eq!(cg_bgra(32, little | 6 | 0x100), None, "floating point");
        assert_eq!(cg_bgra(32, little | 1), None, "alpha last in little-endian is ABGR");
        assert_eq!(cg_bgra(32, 0), None, "no alpha information");
    }

    #[test]
    fn fitting_keeps_proportions_with_black_bars_and_reads_padded_rows() {
        // 4 × 2, left half red, right half blue, rows padded to 24 bytes.
        let mut src = vec![99u8; 24 * 2];
        for y in 0..2 {
            for x in 0..4 {
                let c = if x < 2 { [255, 0, 0, 255] } else { [0, 0, 255, 255] };
                src[y * 24 + x * 4..][..4].copy_from_slice(&c);
            }
        }
        let mut out = Frame::default();
        fit_into(&Pixels { data: &src, width: 4, height: 2, stride: 24, bgra: false }, &mut out, (16, 16));
        let px = |x: usize, y: usize| out.rgba[(y * 16 + x) * 4..][..4].to_vec();
        // 4 × 2 into 16 × 16: 16 × 8 in the middle, bars above and below.
        assert_eq!(fit_rect(4, 2, 16, 16), (0, 4, 16, 8));
        assert_eq!(px(8, 0), BLACK);
        assert_eq!(px(8, 15), BLACK);
        assert_eq!(px(1, 8), vec![255, 0, 0, 255]);
        assert_eq!(px(14, 8), vec![0, 0, 255, 255]);
        assert!(out.rgba.chunks(4).all(|p| p[3] == 255), "the padding never shows");
        // Old pictures are reused at the new size.
        let cap = out.rgba.capacity();
        fit_into(&Pixels { data: &src, width: 4, height: 2, stride: 24, bgra: false }, &mut out, (8, 8));
        assert_eq!(out.rgba.len(), 8 * 8 * 4);
        assert_eq!(out.rgba.capacity(), cap);
    }

    #[test]
    fn scaling_matches_studios_scaler_on_big_pictures() {
        // Big enough to be done in bands on several threads.
        let (sw, sh, ow, oh) = (1300u32, 812u32, 960u32, 600u32);
        let mut src = vec![0u8; (sw * sh * 4) as usize];
        crate::encode::test_picture(&mut src, sw, sh, 5);
        let mut out = Frame::default();
        fit_into(&Pixels { data: &src, width: sw, height: sh, stride: sw as usize * 4, bgra: false }, &mut out, (ow, oh));
        let img = image::RgbaImage::from_raw(sw, sh, src).unwrap();
        let mut want = image::RgbaImage::from_pixel(ow, oh, image::Rgba(BLACK));
        let (x, y, w, h) = fit_rect(sw, sh, ow, oh);
        crate::studio::blit_scaled(&mut want, &img, x as i64, y as i64, w, h, false);
        let worst = out.rgba.iter().zip(want.as_raw()).enumerate().filter(|(i, _)| i % 4 != 3).map(|(_, (a, b))| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(worst <= 2, "off by {worst}");
    }

    #[test]
    fn the_graphics_chip_conversion_parameters() {
        // Letterboxing on the video processor matches `fit_into`.
        assert_eq!(dest_rect((2560, 1600), (2560, 1600)), (0, 0, 2560, 1600));
        assert_eq!(dest_rect((2560, 1600), (1920, 1080)), (96, 0, 1824, 1080), "bars left and right");
        assert_eq!(dest_rect((1920, 1080), (1280, 720)), (0, 0, 1280, 720));
        assert_eq!(dest_rect((1000, 2000), (1280, 720)), (460, 0, 820, 720));
        // Full-range RGB in, BT.709 studio range out.
        assert_eq!(colour_space_bits(false, true, NOMINAL_0_255), 0b10_0100);
        assert_eq!(colour_space_bits(false, true, NOMINAL_16_235), 0b01_0100);
        assert_eq!(colour_space_bits(true, false, 0), 0b10);
        assert_eq!(colour_space_bits(false, false, 7), 0b11_0000, "two bits of range only");
        assert_eq!(nv12_planes(4096, 2160), (0, 4096 * 2160));
        // The check picture: solid 32 x 32 patches, every centre an even pixel inside its patch.
        let p = colour_check_picture();
        assert_eq!(p.len(), (CHECK_W * CHECK_H * 4) as usize);
        let pts = check_points();
        assert_eq!(pts.len(), CHECK_COLOURS.len());
        for (i, &(x, y)) in pts.iter().enumerate() {
            assert!(x % 2 == 0 && y % 2 == 0);
            let at = (y * CHECK_W as usize + x) * 4;
            assert_eq!(&p[at..at + 3], &CHECK_COLOURS[i]);
            // The whole 2 x 2 block has the patch's colour.
            let next = ((y + 1) * CHECK_W as usize + x + 1) * 4;
            assert_eq!(&p[next..next + 3], &CHECK_COLOURS[i]);
        }
    }

    #[test]
    fn pixels_blend_four_bytes_at_once() {
        assert_eq!(lerp_px(0x1020_3040, 0xFFFF_FFFF, 0), 0x1020_3040);
        assert_eq!(lerp_px(0, 0xFFFF_FFFF, 256), 0xFFFF_FFFF);
        assert_eq!(lerp_px(0, 0xFFFF_FFFF, 128), 0x7F7F_7F7F);
        assert_eq!(lerp_px(0x0000_00FF, 0x00FF_0000, 64), 0x003F_00BF);
        assert_eq!(opaque_rgba(0x0011_2233, true), 0xFF33_2211);
        assert_eq!(sample_points(4, 4), vec![(0, 1, 0), (1, 2, 0), (2, 3, 0), (3, 3, 0)]);
        assert!(sample_points(1000, 7).iter().all(|&(a, b, _)| a < 1000 && b < 1000));
    }

    /// A screen that changes on every second look and paints its look count in.
    struct FakeGrab {
        looks: u8,
    }

    static FAKE_OPENS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static FAKE_RETARGETS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    impl Grab for FakeGrab {
        fn grab(&mut self, until: Instant, force: bool, out: &mut Frame, fit: Option<(u32, u32)>) -> Result<bool, String> {
            std::thread::sleep(until.saturating_duration_since(Instant::now()));
            self.looks = self.looks.wrapping_add(1);
            if !force && self.looks % 2 == 0 {
                return Ok(false);
            }
            let src = solid_px(8, 4, [self.looks, 0, 0, 255]);
            fit_into(&Pixels { data: &src, width: 8, height: 4, stride: 32, bgra: false }, out, fit.unwrap_or((8, 4)));
            Ok(true)
        }
        fn retarget(&mut self, r: (i32, i32, u32, u32)) -> bool {
            FAKE_RETARGETS.fetch_add(1, Ordering::Relaxed);
            r.2 == 8
        }
        fn name(&self) -> &'static str {
            "fake"
        }
    }

    fn fake_open(_: (i32, i32, u32, u32), _: bool) -> Result<Box<dyn Grab>, String> {
        FAKE_OPENS.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(FakeGrab { looks: 0 }))
    }

    fn broken_open(_: (i32, i32, u32, u32), _: bool) -> Result<Box<dyn Grab>, String> {
        Err("no screen here".into())
    }

    #[test]
    fn the_feed_keeps_the_newest_picture_and_follows_a_moving_area() {
        let o = FeedOptions { fps: 200, cursor: false, fit: Some((16, 8)), every_frame: false };
        let f = ScreenFeed::start_with((0, 0, 8, 4), o, fake_open).unwrap();
        assert!(f.frames() >= 1, "start waits for the first picture");
        assert_eq!(f.method(), "fake");
        let t = Instant::now();
        while f.frames() < 5 && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        f.with_latest(|p| {
            assert_eq!((p.width, p.height, p.number), (16, 8, p.number.max(5)));
            // The fake paints its look count; only changed looks (odd ones) become pictures.
            assert_eq!(p.rgba[0] % 2, 1, "an unchanged screen isn't copied again");
            assert_eq!(&p.rgba[1..4], &[0, 0, 255]);
        });
        // Same size: moved in place. Another size: made again.
        let opens = FAKE_OPENS.load(Ordering::Relaxed);
        f.move_to((5, 5, 8, 4));
        f.move_to((5, 5, 1, 1));
        let t = Instant::now();
        while FAKE_RETARGETS.load(Ordering::Relaxed) == 0 && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(FAKE_OPENS.load(Ordering::Relaxed), opens, "too small a move is ignored; the same size moves in place");
        f.move_to((0, 0, 20, 10));
        let t = Instant::now();
        while FAKE_OPENS.load(Ordering::Relaxed) == opens && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(FAKE_OPENS.load(Ordering::Relaxed), opens + 1);
        drop(f);
        // Copying every frame makes a picture every time.
        let f = ScreenFeed::start_with((0, 0, 8, 4), FeedOptions { every_frame: true, ..o }, fake_open).unwrap();
        let t = Instant::now();
        while f.frames() < 6 && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        f.with_latest(|p| assert_eq!(p.rgba[0] as u64 % 256, p.number % 256, "every look became a picture"));
        assert!(ScreenFeed::start_with((0, 0, 1, 1), o, fake_open).is_err(), "too small");
        let e = ScreenFeed::start_with((0, 0, 8, 4), o, broken_open).err().unwrap();
        assert_eq!(e, "no screen here");
    }

    /// How long bringing a 2560 × 1600 BGRA screen to a 1080p and a 1440p stream takes:
    /// `cargo test --release -- --ignored fit_speed --nocapture`.
    #[test]
    #[ignore]
    fn fit_speed() {
        let (sw, sh) = (2560u32, 1600u32);
        let mut src = vec![0u8; (sw * sh * 4) as usize];
        crate::encode::test_picture(&mut src, sw, sh, 1);
        let px = Pixels { data: &src, width: sw, height: sh, stride: sw as usize * 4, bgra: true };
        for size in [(1728, 1080), (2304, 1440), (2560, 1600), (3840, 2160)] {
            let mut out = Frame::default();
            fit_into(&px, &mut out, size);
            let runs = 100;
            let t = Instant::now();
            for _ in 0..runs {
                fit_into(std::hint::black_box(&px), &mut out, size);
            }
            println!("2560x1600 into {}x{}: {:.2} ms", size.0, size.1, t.elapsed().as_secs_f64() * 1000.0 / runs as f64);
        }
    }

    #[test]
    fn sound_that_cant_start_leaves_a_note_instead_of_stopping_the_recording() {
        let (c, note) = sound_or_note::<u8>(Err("no speakers or headphones to record from".into()));
        assert!(c.is_none());
        assert_eq!(note, "Recorded without sound: no speakers or headphones to record from");
        let (c, note) = sound_or_note(Ok(7u8));
        assert_eq!(c, Some(7));
        assert!(note.is_empty());
    }

    #[test]
    fn formats_parse_and_name_their_files() {
        assert_eq!(VideoFormat::parse("MP4"), Some(VideoFormat::Mp4));
        assert_eq!(VideoFormat::parse("h265"), Some(VideoFormat::Hevc));
        assert_eq!(VideoFormat::parse("wmv"), Some(VideoFormat::Wmv));
        assert_eq!(VideoFormat::parse("gif"), Some(VideoFormat::Gif));
        assert_eq!(VideoFormat::parse("avi"), None);
        assert_eq!(VideoFormat::Hevc.ext(), "mp4");
        assert!(VideoFormat::Mp4.available() && VideoFormat::Gif.available());
    }

    #[test]
    fn sizes_are_even_and_not_tiny() {
        assert_eq!(even_size(1921, 1081), Ok((1920, 1080)));
        assert!(even_size(63, 400).is_err());
        assert_eq!(even_size(20000, 20000), Ok((8192, 8192)));
    }

    #[test]
    fn bitrate_grows_with_quality_size_and_rate_and_stays_in_bounds() {
        let b = |w, h, fps, q| bitrate(w, h, fps, VideoFormat::Mp4, q);
        assert!(b(1920, 1080, 30, 3) > b(1920, 1080, 30, 1));
        assert!(b(3840, 2160, 30, 2) > b(1920, 1080, 30, 2));
        assert!(b(1920, 1080, 120, 2) > b(1920, 1080, 30, 2));
        assert!(bitrate(1920, 1080, 30, VideoFormat::Hevc, 2) < b(1920, 1080, 30, 2));
        assert!(b(64, 64, 1, 1) >= 250_000);
        assert!(b(8192, 8192, 120, 4) <= 160_000_000);
    }

    #[test]
    fn the_clock_stamps_frames_at_the_frame_rate() {
        let mut c = Clock::new(30);
        assert_eq!(c.stamp(30), (0, 333_333));
        c.n = 30;
        assert_eq!(c.stamp(30).0, 10_000_000);
        assert!(!c.over(0));
    }

    #[test]
    fn vendors_are_named() {
        assert_eq!(vendor_name("VEN_8086"), "Intel");
        assert_eq!(vendor_name("VEN_10DE"), "NVIDIA");
        assert_eq!(vendor_name("VEN_1002"), "AMD");
    }

    #[test]
    fn every_format_has_a_label_an_extension_and_parses_back() {
        let mut labels: Vec<&str> = VideoFormat::ALL.iter().map(|f| f.label()).collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), VideoFormat::ALL.len());
        for f in VideoFormat::ALL {
            assert!(["mp4", "wmv", "gif"].contains(&f.ext()), "{f:?}");
            assert_eq!(VideoFormat::parse(f.ext()).map(|p| p.ext()), Some(f.ext()));
        }
        for (s, f) in [(" H.264 ", VideoFormat::Mp4), ("avc", VideoFormat::Mp4), ("H.265", VideoFormat::Hevc), ("mp4-hevc", VideoFormat::Hevc), ("GIF", VideoFormat::Gif)] {
            assert_eq!(VideoFormat::parse(s), Some(f), "{s}");
        }
        assert_eq!(VideoFormat::parse(""), None);
        assert_eq!(VideoFormat::default(), VideoFormat::Mp4);
    }

    #[test]
    fn formats_save_by_name_in_settings() {
        for f in VideoFormat::ALL {
            let s = serde_json::to_string(&f).unwrap();
            assert_eq!(serde_json::from_str::<VideoFormat>(&s).unwrap(), f);
        }
        assert_eq!(serde_json::to_string(&VideoFormat::Hevc).unwrap(), "\"Hevc\"");
    }

    #[test]
    fn what_each_system_can_make() {
        assert!(VideoFormat::Mp4.available() && VideoFormat::Gif.available());
        assert_eq!(VideoFormat::Hevc.available(), cfg!(windows));
        assert_eq!(VideoFormat::Wmv.available(), cfg!(windows));
    }

    #[test]
    fn even_sizes_round_down_and_keep_the_limits() {
        assert_eq!(even_size(64, 64), Ok((64, 64)));
        assert_eq!(even_size(65, 65), Ok((64, 64)), "odd sizes lose a pixel");
        assert!(even_size(64, 63).is_err(), "63 rounds to 62, too small");
        assert!(even_size(0, 0).is_err());
        assert_eq!(even_size(8193, 100), Ok((8192, 100)));
        assert_eq!(even_size(u32::MAX, u32::MAX), Ok((8192, 8192)));
        let (w, h) = even_size(1367, 769).unwrap();
        assert!(w % 2 == 0 && h % 2 == 0);
    }

    #[test]
    fn bitrate_numbers_come_out_as_expected() {
        // 1920 x 1080 x 0.08 bits per pixel x 30 frames.
        assert_eq!(bitrate(1920, 1080, 30, VideoFormat::Mp4, 2), 4_976_640);
        // HEVC needs about 60 %.
        assert_eq!(bitrate(1920, 1080, 30, VideoFormat::Hevc, 2), 2_985_984);
        // Above 30 fps each extra frame costs 40 %: 30 + 0.4 x 30 = 42 frames' worth at 60 fps.
        assert_eq!(bitrate(1920, 1080, 60, VideoFormat::Mp4, 2), 6_967_296);
        // WMV and MP4 cost the same.
        assert_eq!(bitrate(1280, 720, 30, VideoFormat::Wmv, 3), bitrate(1280, 720, 30, VideoFormat::Mp4, 3));
    }

    #[test]
    fn bitrate_clamps_odd_quality_and_rates() {
        let b = |fps, q| bitrate(1920, 1080, fps, VideoFormat::Mp4, q);
        assert_eq!(b(30, 0), b(30, 1), "quality 0 is 1");
        assert_eq!(b(30, 200), b(30, 4), "quality above 4 is 4");
        assert_eq!(b(0, 2), b(1, 2), "0 fps is 1");
        assert_eq!(b(1000, 2), b(120, 2), "rates stop at 120");
        assert_eq!(bitrate(0, 0, 30, VideoFormat::Mp4, 2), 250_000, "never below 250 kbit/s");
        for q in 1..4 {
            assert!(b(30, q) < b(30, q + 1));
        }
    }

    #[test]
    fn clock_intervals_follow_the_rate_and_are_clamped() {
        assert_eq!(Clock::new(30).interval, Duration::from_nanos(33_333_333));
        assert_eq!(Clock::new(60).interval, Duration::from_nanos(16_666_666));
        assert_eq!(Clock::new(0).interval, Duration::from_secs(1), "0 fps is 1");
        assert_eq!(Clock::new(1000).interval, Duration::from_nanos(8_333_333), "at most 120");
    }

    #[test]
    fn frame_stamps_add_up_without_drift() {
        for fps in [1, 24, 25, 30, 50, 60, 120] {
            let mut c = Clock::new(fps);
            let mut total = 0i64;
            for n in 0..fps as u64 {
                c.n = n;
                let (t, d) = c.stamp(fps);
                assert_eq!(t, total, "frame {n} at {fps} fps starts where the last ended");
                assert!(d > 0);
                total += d;
            }
            assert_eq!(total, 10_000_000, "{fps} frames last exactly one second");
        }
        // The rate used for stamps is clamped like the clock's.
        let mut c = Clock::new(30);
        c.n = 1;
        assert_eq!(c.stamp(0), (10_000_000, 10_000_000));
        assert_eq!(c.stamp(500), c.stamp(120));
    }

    #[test]
    fn deadlines_step_one_interval_per_frame() {
        let mut c = Clock::new(10);
        let base = c.base;
        assert_eq!(c.deadline(), base);
        c.n = 3;
        assert_eq!(c.deadline(), base + Duration::from_millis(300));
        // After a pause the count restarts from n_base.
        c.n_base = 3;
        assert_eq!(c.deadline(), base);
    }

    #[test]
    fn advancing_counts_recorded_time_and_skips_late_frames() {
        let mut c = Clock::new(100);
        c.advance();
        assert_eq!(c.n, 1);
        assert_eq!(c.recorded, Duration::from_millis(10));
        // Pretend the encoder took a whole second: the frames it missed are skipped.
        let mut c = Clock::new(100);
        c.base -= Duration::from_secs(1);
        c.advance();
        assert!(c.n >= 90, "skipped ahead to about now: {}", c.n);
        assert_eq!(c.recorded, Duration::from_millis(10) * c.n as u32, "recorded time covers skipped frames");
    }

    #[test]
    fn recording_stops_after_max_seconds_only_when_set() {
        let mut c = Clock::new(30);
        c.recorded = Duration::from_millis(1999);
        assert!(!c.over(0), "0 means no limit");
        assert!(!c.over(2));
        assert!(c.over(1));
        c.recorded = Duration::from_secs(2);
        assert!(c.over(2));
        c.recorded = Duration::from_secs(100_000);
        assert!(!c.over(0));
    }

    #[test]
    fn pausing_holds_the_clock_and_resuming_restarts_it() {
        let mut c = Clock::new(30);
        c.n = 7;
        let pause = AtomicBool::new(false);
        assert!(!c.hold(&pause), "not paused");
        assert_eq!(c.n_base, 0, "nothing restarts without a pause");
        pause.store(true, Ordering::Relaxed);
        assert!(c.hold(&pause));
        assert!(c.paused_at.is_some());
        let before = c.base;
        pause.store(false, Ordering::Relaxed);
        assert!(!c.hold(&pause));
        assert!(c.paused_at.is_none());
        assert_eq!(c.n_base, 7, "the count restarts where it was");
        assert!(c.base >= before);
        assert_eq!(c.recorded, Duration::ZERO, "paused time is not recorded time");
    }

    #[test]
    fn plain_screen_options() {
        let o = VideoOpts::screen((1, 2, 300, 200), 60, VideoFormat::Hevc, PathBuf::from("x.mp4"));
        assert_eq!(o.rect, (1, 2, 300, 200));
        assert_eq!((o.fps, o.format, o.quality, o.max_s), (60, VideoFormat::Hevc, 2, 0));
        assert!(o.hardware && o.cursor && o.frames.is_none() && o.mic.is_empty());
        assert_eq!(o.audio, crate::audio::AudioSource::None);
    }

    #[test]
    fn options_from_the_settings() {
        let dir = std::env::temp_dir().join(format!("snagster-video-opts-{}", std::process::id()));
        let mut s = crate::settings::Settings { video_folder: dir.to_string_lossy().to_string(), ..Default::default() };
        s.video_fps = 500;
        s.video_quality = 9;
        s.video_hardware = false;
        s.gif_cursor = false;
        s.video_max_s = 12;
        s.audio_source = crate::audio::AudioSource::Both;
        s.audio_mic = "mic-1".into();
        let o = VideoOpts::from_settings(&s, (5, 6, 640, 480));
        assert_eq!(o.rect, (5, 6, 640, 480));
        assert_eq!((o.fps, o.quality, o.max_s), (120, 4, 12), "kept in range");
        assert!(!o.hardware && !o.cursor && o.frames.is_none());
        assert_eq!((o.audio, o.mic.as_str()), (crate::audio::AudioSource::Both, "mic-1"));
        assert_eq!(o.path.parent(), Some(dir.as_path()));
        let name = o.path.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("Recording_") && name.ends_with(".mp4"), "{name}");
        // A GIF setting records MP4 here.
        s.rec_format = VideoFormat::Gif;
        assert_eq!(VideoOpts::from_settings(&s, (0, 0, 64, 64)).format, VideoFormat::Mp4);
        s.rec_format = VideoFormat::Wmv;
        let o = VideoOpts::from_settings(&s, (0, 0, 64, 64));
        assert_eq!(o.format, if cfg!(windows) { VideoFormat::Wmv } else { VideoFormat::Mp4 });
        assert_eq!(o.path.extension().unwrap(), o.format.ext());
    }

    #[test]
    fn only_formats_this_system_makes_are_offered() {
        let names = VideoFormat::video_names();
        assert_eq!(names[0], "mp4");
        assert!(!names.contains(&"gif"));
        assert_eq!(names.len(), if cfg!(windows) { 3 } else { 1 });
        for n in names {
            let f = VideoFormat::parse(n).unwrap();
            assert!(f.available());
            assert_eq!(f.video_or_mp4(), f);
        }
        assert_eq!(VideoFormat::Gif.video_or_mp4(), VideoFormat::Mp4);
    }

    #[test]
    fn recording_paths_are_new_files_in_the_folder() {
        let dir = std::env::temp_dir().join(format!("snagster-video-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = recording_path(&dir.to_string_lossy(), "Studio", "wmv");
        assert!(a.file_name().unwrap().to_string_lossy().starts_with("Studio_"));
        assert_eq!(a.extension().unwrap(), "wmv");
        std::fs::write(&a, b"x").unwrap();
        let b = recording_path(&dir.to_string_lossy(), "Studio", "wmv");
        // Same second: a number is added; a later second: a new name. Either way not the same file.
        assert_ne!(a, b);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The recording bar counts from the first picture: the recording is "live" only while it
    /// takes pictures (on Wayland the Share dialog comes first), and not after it ends.
    #[test]
    fn a_recording_is_live_while_it_takes_pictures_and_not_after() {
        let dir = std::env::temp_dir().join(format!("snagster-video-live-{}", std::process::id()));
        let mut o = VideoOpts::screen((0, 0, 64, 64), 10, VideoFormat::Mp4, dir.join("live.mp4"));
        o.hardware = false;
        let seen = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let (s2, c2) = (seen.clone(), calls.clone());
        o.frames = Some(Arc::new(move |px: &mut [u8]| {
            if recording_live() {
                s2.store(true, Ordering::Relaxed);
            }
            px.fill(128);
            c2.fetch_add(1, Ordering::Relaxed) < 3
        }));
        let (stop, pause) = (AtomicBool::new(false), AtomicBool::new(false));
        let d = record(&o, &stop, &pause);
        assert!(d.result.is_ok(), "{:?}", d.result);
        assert!(seen.load(Ordering::Relaxed), "live while pictures were taken");
        assert!(!recording_live(), "not live after the recording ended");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gifs_are_refused_before_anything_starts() {
        let dir = std::env::temp_dir().join(format!("snagster-video-gif-{}", std::process::id())).join("not-made");
        let o = VideoOpts::screen((0, 0, 64, 64), 10, VideoFormat::Gif, dir.join("a.gif"));
        let stop = AtomicBool::new(false);
        let pause = AtomicBool::new(false);
        let d = record(&o, &stop, &pause);
        assert!(d.result.unwrap_err().contains("GIF recorder"));
        assert_eq!((d.frames, d.width, d.height), (0, 0, 0));
        assert_eq!(d.path, dir.join("a.gif"));
        assert!(!dir.exists(), "no folder was made");
    }

    /// Off Windows, HEVC and WMV can't be made: say so before touching the screen.
    #[cfg(not(windows))]
    #[test]
    fn formats_this_system_cannot_make_are_refused() {
        let stop = AtomicBool::new(false);
        let pause = AtomicBool::new(false);
        for f in [VideoFormat::Hevc, VideoFormat::Wmv] {
            let o = VideoOpts::screen((0, 0, 64, 64), 10, f, PathBuf::from("unused.mp4"));
            assert!(record(&o, &stop, &pause).result.unwrap_err().contains("pick MP4"), "{f:?}");
        }
    }

    #[test]
    fn there_is_always_an_encoder_listed() {
        // Off Windows: the graphics chip's H.264 encoder where there is one, and the built-in
        // software encoder last. On Windows it asks Media Foundation, which every desktop
        // Windows has.
        let list = encoders();
        if cfg!(not(windows)) {
            let last = list.last().expect("OpenH264 at least");
            assert!(!last.hardware);
            assert_eq!(last.name, "OpenH264 (software)");
            assert!(list.iter().all(|e| e.codec == "H.264"));
        }
        for e in &list {
            assert!(!e.name.is_empty());
        }
    }

    fn packet(data: Vec<u8>, pts_us: i64, keyframe: bool) -> crate::encode::VideoPacket {
        crate::encode::VideoPacket { data, pts_us, dts_us: pts_us, keyframe }
    }

    #[test]
    fn each_frame_lasts_until_the_next_one() {
        let mut s = Spacer::new(30);
        assert!(s.push(packet(vec![1], 0, true)).is_none(), "the first waits for the second");
        let (p, d) = s.push(packet(vec![2], 33_333, false)).unwrap();
        assert_eq!((p.data, d), (vec![1], 333_330));
        // A dropped frame: the one before lasts twice as long.
        let (p, d) = s.push(packet(vec![3], 100_000, false)).unwrap();
        assert_eq!((p.data, d), (vec![2], 666_670));
        let (p, d) = s.last().unwrap();
        assert_eq!((p.data, d), (vec![3], 333_333), "the last one lasts one frame");
        assert!(s.last().is_none());
        // Equal times never give a zero length.
        let mut s = Spacer::new(60);
        s.push(packet(vec![1], 5, true));
        assert_eq!(s.push(packet(vec![2], 5, false)).unwrap().1, 1);
    }

    #[test]
    fn access_units_become_mp4_samples_and_give_up_their_parameter_sets() {
        let au = [0, 0, 0, 1, 0x09, 0xf0, 0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3, 0, 0, 1, 0x06, 7, 0, 0, 0, 1, 0x65, 8, 9];
        let (data, sps, pps) = avcc_sample(&au);
        assert_eq!(sps, Some(vec![0x67, 1, 2]));
        assert_eq!(pps, Some(vec![0x68, 3]));
        assert_eq!(data, vec![0, 0, 0, 2, 0x06, 7, 0, 0, 0, 3, 0x65, 8, 9], "SEI and slice stay, delimiter goes");
        let (data, sps, pps) = avcc_sample(&[0, 0, 1, 0x41, 5]);
        assert_eq!((data, sps, pps), (vec![0, 0, 0, 2, 0x41, 5], None, None));
    }

    /// The whole Linux/macOS recording path minus the screen: frames from the encoder, through
    /// the spacer, into an MP4 that reads back with every frame and the right length.
    #[cfg(not(windows))]
    #[test]
    fn encoded_frames_make_a_readable_mp4() {
        use crate::encode::{Codec, VideoConfig};
        let c = VideoConfig { width: 128, height: 96, fps: 30, bitrate_bps: 4_000_000, keyframe_every_s: 1, codec: Codec::H264, prefer_hardware: false, low_latency: true };
        let mut e = crate::encode::open_video(&c).unwrap();
        let path = std::env::temp_dir().join(format!("snagster-mux-test-{}.mp4", std::process::id()));
        let mut mux = soft::Mux::new(std::io::BufWriter::new(std::fs::File::create(&path).unwrap()), 128, 96).unwrap();
        let mut s = Spacer::new(30);
        let mut img = vec![0u8; c.rgba_len()];
        for i in 0..45 {
            crate::encode::test_picture(&mut img, 128, 96, i);
            for p in e.encode(&img, i as i64 * 33_333, false).unwrap().into_iter().chain(if i == 44 { e.finish().unwrap() } else { vec![] }) {
                if let Some((p, d)) = s.push(p) {
                    mux.frame(&p, d).unwrap();
                }
            }
        }
        let (p, d) = s.last().unwrap();
        mux.frame(&p, d).unwrap();
        mux.finish().unwrap();
        let pr = probe(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!((pr.width, pr.height, pr.frames), (128, 96, 45));
        assert!((pr.seconds - 1.5).abs() < 0.05, "{} s", pr.seconds);
    }

    #[test]
    fn unknown_vendors_keep_their_id_in_capitals() {
        assert_eq!(vendor_name("ven_8086&dev_1234"), "Intel");
        assert_eq!(vendor_name("VEN_1022"), "AMD");
        assert_eq!(vendor_name("QCOM"), "Qualcomm");
        assert_eq!(vendor_name("VEN_5143"), "Qualcomm");
        assert_eq!(vendor_name("ven_abcd"), "VEN_ABCD");
        assert_eq!(vendor_name(""), "");
    }

    #[test]
    fn probing_a_missing_or_broken_file_fails_politely() {
        let dir = std::env::temp_dir().join(format!("snagster-video-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(probe(&dir.join("missing.mp4")).is_err());
        let junk = dir.join("junk.mp4");
        std::fs::write(&junk, b"this is not an mp4 file at all").unwrap();
        assert!(probe(&junk).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
