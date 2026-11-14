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
    /// Draw the mouse pointer.
    pub cursor: bool,
    /// Stop by itself after this many seconds (0: only when told).
    pub max_s: u32,
    pub path: PathBuf,
    /// Sound to record with it (MP4 only).
    pub audio: crate::audio::AudioSource,
    /// The microphone's id (empty: the default one).
    pub mic: String,
    /// Frames made by the caller (Studio mode) instead of the screen; `rect` then gives only
    /// the size.
    pub frames: Option<FrameFeed>,
}

impl VideoOpts {
    /// Plain options for a screen area: no sound, no custom frames.
    pub fn screen(rect: (i32, i32, u32, u32), fps: u32, format: VideoFormat, path: PathBuf) -> VideoOpts {
        VideoOpts { rect, fps, format, quality: 2, hardware: true, cursor: true, max_s: 0, path, audio: crate::audio::AudioSource::None, mic: String::new(), frames: None }
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
            cursor: s.gif_cursor,
            max_s: s.video_max_s,
            path: recording_path(&s.video_folder, "Recording", format.ext()),
            audio: s.audio_source,
            mic: s.audio_mic.clone(),
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
    crate::capture::background_thread(false);
    #[cfg(windows)]
    win::record(opts, stop, pause, &mut done);
    #[cfg(not(windows))]
    soft::record(opts, stop, pause, &mut done);
    done
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

/// The best way to copy `rect` on this system.
fn open_grab(rect: (i32, i32, u32, u32), cursor: bool) -> Result<Box<dyn Grab>, String> {
    #[cfg(windows)]
    return win::open_grab(rect, cursor);
    #[cfg(not(windows))]
    return Ok(Box::new(XcapGrab { rect, cursor }));
}

/// Linux and macOS: xcap, one grab per frame.
#[cfg(not(windows))]
struct XcapGrab {
    rect: (i32, i32, u32, u32),
    cursor: bool,
}

#[cfg(not(windows))]
impl Grab for XcapGrab {
    fn grab(&mut self, until: Instant, _force: bool, out: &mut Frame, fit: Option<(u32, u32)>) -> Result<bool, String> {
        let now = Instant::now();
        if until > now {
            std::thread::sleep(until - now);
        }
        let (x, y, w, h) = self.rect;
        let mut img = crate::capture::grab_rect(x, y, w, h)?;
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
}

/// The newest picture of a screen area, copied `fps` times a second by a thread of its own
/// into buffers that are reused. Stops when dropped.
pub struct ScreenFeed {
    sh: Arc<FeedShared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// A feed of `rect` (desktop pixels, physical): see `ScreenFeed::start`.
pub fn screen_feed(rect: (i32, i32, u32, u32), o: FeedOptions) -> Result<ScreenFeed, String> {
    ScreenFeed::start(rect, o)
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
}

impl Drop for ScreenFeed {
    fn drop(&mut self) {
        self.sh.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Makes the grabber for an area (`open_grab`, or a fake in the tests).
type Opener = fn((i32, i32, u32, u32), bool) -> Result<Box<dyn Grab>, String>;

fn feed_loop(sh: &FeedShared, mut rect: (i32, i32, u32, u32), o: FeedOptions, open: Opener) {
    crate::capture::background_thread(false);
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
        match g.grab(next, force || o.every_frame, &mut back, o.fit) {
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

    fn per_monitor_dpi() {
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
    struct Screen {
        adapter: IDXGIAdapter1,
        output: IDXGIOutput1,
        /// Area relative to the display's top-left corner.
        local: (u32, u32),
    }

    /// The display holding most of the area (DXGI coordinates are physical pixels).
    fn find_screen(x: i32, y: i32, w: u32, h: u32) -> R<Option<Screen>> {
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

    fn device(adapter: &IDXGIAdapter1) -> R<(ID3D11Device, ID3D11DeviceContext)> {
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

    fn texture(dev: &ID3D11Device, w: u32, h: u32, gdi: bool) -> R<ID3D11Texture2D> {
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
    struct Pointer {
        hotspots: std::collections::HashMap<isize, (i32, i32)>,
    }
    impl Pointer {
        fn new() -> Pointer {
            Pointer { hotspots: Default::default() }
        }

        /// Draw the current pointer onto a device context showing the area at (ox, oy).
        fn draw(&mut self, hdc: *mut core::ffi::c_void, ox: i32, oy: i32) {
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

    struct Dup {
        dev: ID3D11Device,
        ctx: ID3D11DeviceContext,
        dup: IDXGIOutputDuplication,
        output: IDXGIOutput1,
        /// Latest picture of the area.
        latest: ID3D11Texture2D,
        local: (u32, u32),
        w: u32,
        h: u32,
        /// The screen changed since this was last cleared (the screen feed skips copies of an
        /// unchanged screen).
        fresh: bool,
        /// The pointer moved or changed shape since this was last cleared.
        mouse: bool,
    }

    impl Dup {
        fn new(screen: &Screen, w: u32, h: u32) -> R<Dup> {
            let (dev, ctx) = device(&screen.adapter)?;
            let dup = unsafe { screen.output.DuplicateOutput(&dev)? };
            let latest = texture(&dev, w, h, false)?;
            Ok(Dup { dev, ctx, dup, output: screen.output.clone(), latest, local: screen.local, w, h, fresh: true, mouse: false })
        }

        /// Take screen updates until `until`; returns false when the duplication was lost and
        /// couldn't be made again.
        fn pump(&mut self, until: Instant) -> bool {
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
    fn clip_to(s: &Screen, rect: (i32, i32, u32, u32)) -> Option<(RECT, (i32, i32, u32, u32))> {
        let r = unsafe { s.output.GetDesc() }.ok()?.DesktopCoordinates;
        clip_rect(r, rect).map(|c| (r, c))
    }

    fn clip_rect(r: RECT, (x, y, w, h): (i32, i32, u32, u32)) -> Option<(i32, i32, u32, u32)> {
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
        let sound = if opts.audio != crate::audio::AudioSource::None && opts.format != VideoFormat::Wmv {
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
            let tries: &[bool] = if opts.hardware { &[true, false] } else { &[false] };
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
        if opts.hardware && gpu_ok {
            tries.push((true, true));
        }
        if opts.hardware && opts.format != VideoFormat::Wmv {
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
        let mut enc = crate::encode::open_video(&cfg)?;
        let name = enc.name();
        done.encoder = name.name;
        done.hardware = name.hardware;
        let file = std::fs::File::create(&opts.path).map_err(|e| e.to_string())?;
        let mut mux = Mux::new(std::io::BufWriter::new(file), w, h)?;
        let mut spacer = Spacer::new(fps);
        let mut clock = Clock::new(fps);
        let mut first = true;
        let mut rgba = vec![0u8; (w * h * 4) as usize];
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
            } else {
                let mut img = crate::capture::grab_rect(x, y, w, h)?;
                if opts.cursor
                    && let Some(c) = crate::capture::cursor_pos()
                {
                    crate::capture::stamp_pointer(&mut img, ((c.0 - x) as f32, (c.1 - y) as f32));
                }
                let img = if img.dimensions() != (w, h) { image::imageops::resize(&img, w, h, image::imageops::FilterType::Nearest) } else { img };
                rgba.copy_from_slice(img.as_raw());
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
        let r = mp4::Mp4Reader::read_header(std::io::BufReader::new(f), size).map_err(|e| e.to_string())?;
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for size in [(1728, 1080), (2304, 1440), (2560, 1600)] {
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
