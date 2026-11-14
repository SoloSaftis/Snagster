//! Video and sound encoders that hand                 assert_eq!(p.iter().any(|b| b.hardware()), hw && os != Os::Other, "{os:?} hardware {hw}");ack the compressed data itself (not a finished file), for
//! live streaming and, later, recording on every system. Each platform offers its graphics-chip
//! encoder first and falls back to software:
//!
//! - Windows: Media Foundation encoder transforms (Intel Quick Sync, NVIDIA NVENC, AMD AMF), then
//!   Microsoft's software H.264 encoder, then OpenH264.
//! - macOS: VideoToolbox (Apple silicon and Intel media engines), then OpenH264.
//! - Linux: VA-API (Intel, AMD; NVIDIA through its VA-API driver where installed), then OpenH264.
//!
//! Sound is AAC-LC, 48 kHz stereo (what streaming services expect): Media Foundation on Windows,
//! AudioToolbox on macOS. Where no AAC encoder exists, `open_audio` says so and the caller goes on
//! without sound, telling the person.
//!
//! The types here are the contract between the encoders and their users (`stream`, `video`);
//! change them together.

/// Which video format to make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
}

/// What a video encoder should make.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoConfig {
    /// Even numbers; callers round with `video::even_size`.
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u32,
    /// A keyframe at least this often (streaming services ask for 2 s).
    pub keyframe_every_s: u32,
    pub codec: Codec,
    /// Try the graphics chip first (falls back to software when it can't).
    pub prefer_hardware: bool,
    /// No B-frames, no look-ahead: what live streaming needs.
    pub low_latency: bool,
}

/// One compressed video frame: H.264/HEVC in Annex-B form (start codes), one access unit.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoPacket {
    pub data: Vec<u8>,
    /// Presentation time in microseconds from the first frame.
    pub pts_us: i64,
    /// Decode time; equals `pts_us` when there are no B-frames.
    pub dts_us: i64,
    pub keyframe: bool,
}

/// One compressed piece of sound: raw AAC (no ADTS header).
#[derive(Clone, Debug, PartialEq)]
pub struct AudioPacket {
    pub data: Vec<u8>,
    pub pts_us: i64,
}

/// Which encoder ended up doing the work, for the status line and `--video-test`.
#[derive(Clone, Debug, PartialEq)]
pub struct EncoderName {
    pub name: String,
    pub hardware: bool,
}

pub trait VideoEncoder: Send {
    /// Encode one picture: RGBA, `width * height * 4` bytes, top row first. May return no
    /// packets yet (the encoder is still filling its pipeline) or several.
    fn encode(&mut self, rgba: &[u8], pts_us: i64, force_keyframe: bool) -> Result<Vec<VideoPacket>, String>;
    /// Everything still inside the encoder, at the end.
    fn finish(&mut self) -> Result<Vec<VideoPacket>, String>;
    fn name(&self) -> EncoderName;
}

pub trait AudioEncoder: Send {
    /// 48 kHz stereo, interleaved i16 (what `audio::Capture::take` gives). Any length.
    fn encode(&mut self, pcm: &[i16], pts_us: i64) -> Result<Vec<AudioPacket>, String>;
    fn finish(&mut self) -> Result<Vec<AudioPacket>, String>;
    /// The AAC AudioSpecificConfig (2 bytes for AAC-LC 48 kHz stereo), for FLV/MP4 headers.
    fn audio_specific_config(&self) -> Vec<u8>;
    fn name(&self) -> EncoderName;
}

// ---------------------------------------------------------------------------------------------
// Opening encoders
// ---------------------------------------------------------------------------------------------

/// Parameter sets for VA-API (Linux), and an SPS reader for the tests.
#[cfg(any(target_os = "linux", test))]
pub mod h264;
mod soft;

#[cfg(windows)]
mod win;

#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "linux")]
mod linux;

/// Sound sample rate and channels the AAC encoders take (the same as `audio::Capture` gives).
#[cfg(any(windows, target_os = "macos", test))]
pub const AUDIO_RATE: u32 = 48_000;
#[cfg(any(windows, target_os = "macos", test))]
pub const AUDIO_CHANNELS: u32 = 2;
/// Sample frames in one AAC-LC packet.
#[cfg(any(windows, target_os = "macos", test))]
pub const AAC_FRAME: u32 = 1024;

impl Codec {
    pub fn label(self) -> &'static str {
        match self {
            Codec::H264 => "H.264",
            Codec::Hevc => "HEVC",
        }
    }
}

impl VideoConfig {
    /// Refuses settings no encoder can take, with a sentence the person can act on.
    pub fn validate(&self) -> Result<(), String> {
        if self.width % 2 != 0 || self.height % 2 != 0 {
            return Err(format!("The video size must be even, not {} × {}.", self.width, self.height));
        }
        if self.width < 16 || self.height < 16 || self.width > 8192 || self.height > 8192 {
            return Err(format!("A video must be between 16 × 16 and 8192 × 8192 pixels, not {} × {}.", self.width, self.height));
        }
        if !(1..=240).contains(&self.fps) {
            return Err(format!("The frame rate must be 1 to 240 frames per second, not {}.", self.fps));
        }
        if self.bitrate_bps < 10_000 {
            return Err(format!("The bit rate is too low ({} bit/s).", self.bitrate_bps));
        }
        if !(1..=600).contains(&self.keyframe_every_s) {
            return Err(format!("Keyframes must come every 1 to 600 seconds, not {}.", self.keyframe_every_s));
        }
        Ok(())
    }

    /// Frames from one keyframe to the next.
    pub fn gop_frames(&self) -> u32 {
        (self.fps.max(1) * self.keyframe_every_s.max(1)).clamp(1, 1 << 16)
    }

    /// One frame's length in microseconds.
    pub fn frame_us(&self) -> i64 {
        1_000_000 / self.fps.max(1) as i64
    }

    /// Bytes in one RGBA picture.
    pub fn rgba_len(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }
}

/// One way of encoding video, in the order `plan` tries them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// Windows: the graphics vendor's Media Foundation encoder (Quick Sync, NVENC, AMF).
    MediaFoundationHardware,
    /// Windows: Microsoft's own software encoder transform.
    MediaFoundationSoftware,
    /// macOS: VideoToolbox on the media engine.
    VideoToolboxHardware,
    /// macOS: VideoToolbox's software encoder.
    VideoToolboxSoftware,
    /// Linux: VA-API (Intel, AMD, and NVIDIA where its VA-API driver is installed).
    VaApi,
    /// Cisco's OpenH264, built into Snagster: works everywhere, H.264 only.
    OpenH264,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::MediaFoundationHardware => "Media Foundation (graphics chip)",
            Backend::MediaFoundationSoftware => "Media Foundation (software)",
            Backend::VideoToolboxHardware => "VideoToolbox (graphics chip)",
            Backend::VideoToolboxSoftware => "VideoToolbox (software)",
            Backend::VaApi => "VA-API (graphics chip)",
            Backend::OpenH264 => "OpenH264 (software)",
        }
    }

    pub fn hardware(self) -> bool {
        matches!(self, Backend::MediaFoundationHardware | Backend::VideoToolboxHardware | Backend::VaApi)
    }
}

/// The system Snagster runs on, for `plan`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
    Linux,
    Other,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(target_os = "linux") {
            Os::Linux
        } else {
            Os::Other
        }
    }
}

/// The encoders to try, best first: the graphics chip's (when asked for), then the system's own
/// software encoder, then OpenH264 (H.264 only). Empty when nothing on this system makes `codec`.
pub fn plan(os: Os, codec: Codec, prefer_hardware: bool) -> Vec<Backend> {
    let (hardware, software): (&[Backend], &[Backend]) = match (os, codec) {
        (Os::Windows, _) => (&[Backend::MediaFoundationHardware], &[Backend::MediaFoundationSoftware]),
        (Os::Mac, _) => (&[Backend::VideoToolboxHardware], &[Backend::VideoToolboxSoftware]),
        (Os::Linux, Codec::H264) => (&[Backend::VaApi], &[]),
        _ => (&[], &[]),
    };
    let mut v = Vec::new();
    if prefer_hardware {
        v.extend_from_slice(hardware);
    }
    v.extend_from_slice(software);
    if codec == Codec::H264 {
        v.push(Backend::OpenH264);
    }
    v
}

/// The best encoder this computer has for `cfg`: the graphics chip first when
/// `cfg.prefer_hardware`, then software. Keyframes come at least every `cfg.keyframe_every_s`
/// and every keyframe carries its parameter sets (SPS/PPS, and VPS for HEVC).
pub fn open_video(cfg: &VideoConfig) -> Result<Box<dyn VideoEncoder>, String> {
    cfg.validate()?;
    let mut tried = Vec::new();
    for b in plan(Os::current(), cfg.codec, cfg.prefer_hardware) {
        match open_backend(b, cfg) {
            Ok(e) => return Ok(Box::new(Paced::new(e, cfg))),
            Err(e) => tried.push(format!("{}: {e}", b.label())),
        }
    }
    Err(if tried.is_empty() {
        format!("{} video can't be made on this system.", cfg.codec.label())
    } else {
        format!("No {} encoder could start ({}).", cfg.codec.label(), tried.join("; "))
    })
}

/// Open one particular backend (no fallback), for `open_video` and tests.
pub fn open_backend(b: Backend, cfg: &VideoConfig) -> Result<Box<dyn VideoEncoder>, String> {
    cfg.validate()?;
    match b {
        Backend::OpenH264 => soft::OpenH264::open(cfg).map(|e| Box::new(e) as Box<dyn VideoEncoder>),
        #[cfg(windows)]
        Backend::MediaFoundationHardware => win::open_video(cfg, true),
        #[cfg(windows)]
        Backend::MediaFoundationSoftware => win::open_video(cfg, false),
        #[cfg(target_os = "macos")]
        Backend::VideoToolboxHardware => mac::open_video(cfg, true),
        #[cfg(target_os = "macos")]
        Backend::VideoToolboxSoftware => mac::open_video(cfg, false),
        #[cfg(target_os = "linux")]
        Backend::VaApi => linux::open_video(cfg),
        #[allow(unreachable_patterns)]
        _ => Err("not on this system".into()),
    }
}

/// AAC-LC, 48 kHz stereo, at about `bitrate_bps` (encoders round to the rates they offer).
pub fn open_audio(bitrate_bps: u32) -> Result<Box<dyn AudioEncoder>, String> {
    #[cfg(windows)]
    {
        win::open_audio(bitrate_bps)
    }
    #[cfg(target_os = "macos")]
    {
        mac::open_audio(bitrate_bps)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = bitrate_bps;
        Err("AAC sound encoding isn't available on this system (there is no freely licensed AAC encoder Snagster could include)".into())
    }
}

/// The video encoders this computer offers, best first.
pub fn available() -> Vec<(Codec, EncoderName)> {
    #[cfg(windows)]
    let mut v = win::list();
    #[cfg(target_os = "macos")]
    let mut v = mac::list();
    #[cfg(target_os = "linux")]
    let mut v = linux::list();
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    let mut v = Vec::new();
    v.push((Codec::H264, EncoderName { name: soft::NAME.into(), hardware: false }));
    // One graphics chip can be registered once per display adapter: list each encoder once.
    let mut seen = Vec::new();
    v.retain(|e| {
        let new = !seen.contains(e);
        if new {
            seen.push(e.clone());
        }
        new
    });
    v
}

/// The graphics vendor named in an encoder's name ("Intel", "AMD", "NVIDIA", "Apple"), or "".
pub fn vendor_in(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    if n.contains("intel") || n.contains("quick sync") || n.contains("ihd") || n.contains("i965") {
        "Intel"
    } else if n.contains("amd") || n.contains("radeon") || n.contains("amf") {
        "AMD"
    } else if n.contains("nvidia") || n.contains("nvenc") {
        "NVIDIA"
    } else if n.contains("apple") || n.contains("videotoolbox") {
        "Apple"
    } else {
        ""
    }
}

/// Encode `frames` pictures of `w` × `h` with `b` as fast as it goes, for `--video-test`:
/// which encoder, and frames per second.
pub fn speed(b: Backend, w: u32, h: u32, frames: u32) -> Result<(EncoderName, f64), String> {
    let cfg = VideoConfig { width: w, height: h, fps: 60, bitrate_bps: 8_000_000, keyframe_every_s: 2, codec: Codec::H264, prefer_hardware: b.hardware(), low_latency: true };
    let mut e = open_backend(b, &cfg)?;
    let mut rgba = vec![0u8; cfg.rgba_len()];
    let t0 = std::time::Instant::now();
    let mut out = Vec::new();
    for i in 0..frames {
        test_picture(&mut rgba, w, h, i);
        out.extend(e.encode(&rgba, i as i64 * cfg.frame_us(), false)?);
    }
    out.extend(e.finish()?);
    let secs = t0.elapsed().as_secs_f64().max(1e-6);
    match out.first() {
        None => return Err("no frames came out".into()),
        Some(p) if !p.keyframe || sps_pps(&p.data).is_none() => return Err("the first frame isn't a keyframe with its SPS and PPS".into()),
        _ if out.len() + 4 < frames as usize => return Err(format!("{} of {frames} frames came out", out.len())),
        _ => {}
    }
    Ok((e.name(), frames as f64 / secs))
}

/// A moving picture with edges and gradients (something for an encoder to work on).
pub fn test_picture(rgba: &mut [u8], w: u32, h: u32, n: u32) {
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let xs = x.wrapping_add(n * 4);
            rgba[i] = (xs ^ y) as u8;
            rgba[i + 1] = (y.wrapping_add(n * 2)) as u8;
            rgba[i + 2] = if (xs / 64 + y / 64) % 2 == 0 { 230 } else { 30 };
            rgba[i + 3] = 255;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// What every encoder gets: keyframes on time and parameter sets on every keyframe
// ---------------------------------------------------------------------------------------------

/// Wraps a backend: checks picture sizes, keeps time stamps going forward, asks for a keyframe
/// when one is due, and puts the parameter sets in front of keyframes that lack them.
struct Paced {
    inner: Box<dyn VideoEncoder>,
    cadence: Cadence,
    sets: ParamSets,
    codec: Codec,
    rgba_len: usize,
    last_pts: Option<i64>,
}

impl Paced {
    fn new(inner: Box<dyn VideoEncoder>, cfg: &VideoConfig) -> Paced {
        Paced { inner, cadence: Cadence::new(cfg), sets: ParamSets::default(), codec: cfg.codec, rgba_len: cfg.rgba_len(), last_pts: None }
    }

    fn after(&mut self, mut packets: Vec<VideoPacket>) -> Vec<VideoPacket> {
        for p in &mut packets {
            self.sets.complete(self.codec, p);
            if p.keyframe {
                self.cadence.saw_keyframe(p.pts_us);
            }
        }
        packets
    }
}

impl VideoEncoder for Paced {
    fn encode(&mut self, rgba: &[u8], pts_us: i64, force_keyframe: bool) -> Result<Vec<VideoPacket>, String> {
        if rgba.len() != self.rgba_len {
            return Err(format!("a picture of {} bytes was given, the encoder takes {}", rgba.len(), self.rgba_len));
        }
        // Encoders refuse time going backwards or standing still; nudge such frames forward.
        let pts = match self.last_pts {
            Some(l) if pts_us <= l => l + 1,
            _ => pts_us,
        };
        self.last_pts = Some(pts);
        let key = self.cadence.due(pts) || force_keyframe;
        if key {
            self.cadence.forced(pts);
        }
        let p = self.inner.encode(rgba, pts, key)?;
        Ok(self.after(p))
    }

    fn finish(&mut self) -> Result<Vec<VideoPacket>, String> {
        let p = self.inner.finish()?;
        Ok(self.after(p))
    }

    fn name(&self) -> EncoderName {
        self.inner.name()
    }
}

/// When the next keyframe is due: `every_us` after the last one (half a frame early is on time).
#[derive(Clone, Debug)]
struct Cadence {
    every_us: i64,
    slack_us: i64,
    last: Option<i64>,
}

impl Cadence {
    fn new(cfg: &VideoConfig) -> Cadence {
        Cadence { every_us: cfg.keyframe_every_s.max(1) as i64 * 1_000_000, slack_us: cfg.frame_us() / 2, last: None }
    }

    fn due(&self, pts: i64) -> bool {
        match self.last {
            None => true,
            Some(k) => pts - k >= self.every_us - self.slack_us,
        }
    }

    fn forced(&mut self, pts: i64) {
        self.last = Some(pts);
    }

    /// The encoder made a keyframe by itself: count from there.
    fn saw_keyframe(&mut self, pts: i64) {
        self.last = Some(self.last.map_or(pts, |l| l.max(pts)));
    }
}

/// The parameter sets seen so far, put in front of keyframes that come without them.
#[derive(Clone, Debug, Default)]
pub struct ParamSets {
    /// Whole NAL units without start codes, in stream order (VPS, SPS, PPS).
    pub nals: Vec<Vec<u8>>,
}

impl ParamSets {
    /// Learn the sets in `p`; when it's a keyframe without them, put the known ones in front.
    pub fn complete(&mut self, codec: Codec, p: &mut VideoPacket) {
        let found = parameter_sets(codec, &p.data);
        if !found.is_empty() {
            self.nals = found.into_iter().map(|n| n.to_vec()).collect();
            return;
        }
        if p.keyframe && !self.nals.is_empty() {
            let mut d = Vec::with_capacity(p.data.len() + 64);
            for n in &self.nals {
                d.extend_from_slice(&[0, 0, 0, 1]);
                d.extend_from_slice(n);
            }
            d.extend_from_slice(&p.data);
            p.data = d;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Pictures: RGBA to the YUV 4:2:0 layouts encoders take (BT.709, video range)
// ---------------------------------------------------------------------------------------------

#[inline(always)]
fn luma(r: i32, g: i32, b: i32) -> u8 {
    (16 + ((47 * r + 157 * g + 16 * b + 128) >> 8)) as u8
}

#[inline(always)]
fn chroma(r: i32, g: i32, b: i32) -> (u8, u8) {
    let u = 128 + ((-26 * r - 86 * g + 112 * b + 128) >> 8);
    let v = 128 + ((112 * r - 102 * g - 10 * b + 128) >> 8);
    (u.clamp(16, 240) as u8, v.clamp(16, 240) as u8)
}

/// Converts RGBA (`w` × `h`, both even, top row first) to 4:2:0 YUV, handing each 2 × 2 block's
/// chroma to `put_uv(column, row, u, v)` in half-size coordinates.
fn rgba_to_420(rgba: &[u8], w: usize, h: usize, y: &mut [u8], y_stride: usize, mut put_uv: impl FnMut(usize, usize, u8, u8)) {
    debug_assert!(w % 2 == 0 && h % 2 == 0 && rgba.len() >= w * h * 4);
    for row in 0..h / 2 {
        let top = &rgba[row * 2 * w * 4..(row * 2 + 1) * w * 4];
        let bottom = &rgba[(row * 2 + 1) * w * 4..(row * 2 + 2) * w * 4];
        let (y0, y1) = y[row * 2 * y_stride..].split_at_mut(y_stride);
        for col in 0..w / 2 {
            let mut sum = [0i32; 3];
            for (line, out) in [(top, &mut *y0), (bottom, &mut *y1)] {
                for k in 0..2 {
                    let p = &line[(col * 2 + k) * 4..];
                    let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
                    out[col * 2 + k] = luma(r, g, b);
                    sum[0] += r;
                    sum[1] += g;
                    sum[2] += b;
                }
            }
            let (u, v) = chroma((sum[0] + 2) >> 2, (sum[1] + 2) >> 2, (sum[2] + 2) >> 2);
            put_uv(col, row, u, v);
        }
    }
}

/// RGBA to NV12: a full-size Y plane, then one half-size plane of interleaved U, V.
pub fn rgba_to_nv12(rgba: &[u8], w: u32, h: u32, y: &mut [u8], y_stride: usize, uv: &mut [u8], uv_stride: usize) {
    rgba_to_420(rgba, w as usize, h as usize, y, y_stride, |c, r, u, v| {
        uv[r * uv_stride + c * 2] = u;
        uv[r * uv_stride + c * 2 + 1] = v;
    });
}

/// RGBA to I420 (three planes: Y, then U, then V; strides `w`, `w/2`, `w/2`).
pub fn rgba_to_i420(rgba: &[u8], w: u32, h: u32, y: &mut [u8], u: &mut [u8], v: &mut [u8]) {
    let half = w as usize / 2;
    rgba_to_420(rgba, w as usize, h as usize, y, w as usize, |c, r, cu, cv| {
        u[r * half + c] = cu;
        v[r * half + c] = cv;
    });
}

/// A contiguous NV12 picture of `w` × `h` (what Media Foundation takes).
#[cfg(any(windows, test))]
pub fn nv12(rgba: &[u8], w: u32, h: u32, out: &mut Vec<u8>) {
    let ys = (w * h) as usize;
    out.resize(ys + ys / 2, 0);
    let (y, uv) = out.split_at_mut(ys);
    rgba_to_nv12(rgba, w, h, y, w as usize, uv, w as usize);
}

// ---------------------------------------------------------------------------------------------
// Bitstreams: Annex-B (start codes) and AVCC (length prefixes)
// ---------------------------------------------------------------------------------------------

/// The NAL units of an Annex-B stream, without their start codes.
pub fn split_annex_b(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 2 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &s) in starts.iter().enumerate() {
        let mut end = if k + 1 < starts.len() { starts[k + 1] - 3 } else { data.len() };
        // The zeros of a four-byte start code (and trailing zero bytes) aren't part of the unit.
        while end > s && data[end - 1] == 0 {
            end -= 1;
        }
        if end > s {
            out.push(&data[s..end]);
        }
    }
    out
}

/// AVCC (lengths of `len_size` bytes: 1, 2 or 4) to Annex-B with four-byte start codes.
#[cfg(any(target_os = "macos", test))]
pub fn avcc_to_annex_b(data: &[u8], len_size: usize) -> Result<Vec<u8>, String> {
    if !matches!(len_size, 1 | 2 | 4) {
        return Err(format!("NAL length size {len_size} isn't 1, 2 or 4"));
    }
    let mut out = Vec::with_capacity(data.len() + 16);
    let mut i = 0;
    while i < data.len() {
        if i + len_size > data.len() {
            return Err("a NAL length is cut short".into());
        }
        let n = data[i..i + len_size].iter().fold(0usize, |a, &b| (a << 8) | b as usize);
        i += len_size;
        if i + n > data.len() {
            return Err(format!("a NAL of {n} bytes runs past the end"));
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&data[i..i + n]);
        i += n;
    }
    Ok(out)
}

/// The NAL unit type (H.264: 5 bits; HEVC: 6 bits).
pub fn nal_type(codec: Codec, nal: &[u8]) -> u8 {
    match (codec, nal.first()) {
        (_, None) => 0,
        (Codec::H264, Some(b)) => b & 0x1f,
        (Codec::Hevc, Some(b)) => (b >> 1) & 0x3f,
    }
}

/// The parameter-set NAL units in `data` (VPS/SPS/PPS), in the order they appear.
pub fn parameter_sets(codec: Codec, data: &[u8]) -> Vec<&[u8]> {
    split_annex_b(data)
        .into_iter()
        .filter(|n| match codec {
            Codec::H264 => matches!(nal_type(codec, n), 7 | 8),
            Codec::Hevc => matches!(nal_type(codec, n), 32..=34),
        })
        .collect()
}

/// The H.264 SPS and PPS in `data` (the first of each), for MP4 and FLV headers.
pub fn sps_pps(data: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let nals = split_annex_b(data);
    let sps = nals.iter().find(|n| nal_type(Codec::H264, n) == 7)?;
    let pps = nals.iter().find(|n| nal_type(Codec::H264, n) == 8)?;
    Some((sps.to_vec(), pps.to_vec()))
}

/// Whether `data` holds a picture a decoder can start from (H.264 IDR; HEVC IDR, CRA or BLA).
pub fn is_keyframe(codec: Codec, data: &[u8]) -> bool {
    split_annex_b(data).iter().any(|n| match codec {
        Codec::H264 => nal_type(codec, n) == 5,
        Codec::Hevc => (16..=21).contains(&nal_type(codec, n)),
    })
}

// ---------------------------------------------------------------------------------------------
// Sound
// ---------------------------------------------------------------------------------------------

/// The AudioSpecificConfig for AAC-LC (object type 2) at `rate` with `channels`.
#[cfg(any(windows, target_os = "macos", test))]
pub fn aac_lc_config(rate: u32, channels: u32) -> Vec<u8> {
    const RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];
    let idx = RATES.iter().position(|&r| r == rate).unwrap_or(3) as u16;
    let bits: u16 = (2 << 11) | (idx << 7) | ((channels.min(7) as u16) << 3);
    bits.to_be_bytes().to_vec()
}

/// Time of the `n`th AAC packet after one starting at `start_us`.
#[cfg(any(windows, target_os = "macos", test))]
pub fn aac_packet_pts(start_us: i64, n: u64) -> i64 {
    start_us + (n * AAC_FRAME as u64 * 1_000_000 / AUDIO_RATE as u64) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn cfg(w: u32, h: u32) -> VideoConfig {
        VideoConfig { width: w, height: h, fps: 30, bitrate_bps: 2_000_000, keyframe_every_s: 1, codec: Codec::H264, prefer_hardware: false, low_latency: true }
    }

    #[test]
    fn configs_are_checked() {
        assert!(cfg(1920, 1080).validate().is_ok());
        assert!(cfg(1921, 1080).validate().unwrap_err().contains("even"));
        assert!(cfg(8, 8).validate().is_err());
        assert!(cfg(10000, 1000).validate().is_err());
        assert!(VideoConfig { fps: 0, ..cfg(64, 64) }.validate().unwrap_err().contains("frame rate"));
        assert!(VideoConfig { fps: 241, ..cfg(64, 64) }.validate().is_err());
        assert!(VideoConfig { bitrate_bps: 100, ..cfg(64, 64) }.validate().is_err());
        assert!(VideoConfig { keyframe_every_s: 0, ..cfg(64, 64) }.validate().unwrap_err().contains("Keyframes"));
        let c = VideoConfig { fps: 60, keyframe_every_s: 2, ..cfg(64, 64) };
        assert_eq!(c.gop_frames(), 120);
        assert_eq!(c.frame_us(), 16_666);
        assert_eq!(c.rgba_len(), 64 * 64 * 4);
    }

    #[test]
    fn hardware_first_then_the_systems_software_then_openh264() {
        use Backend::*;
        assert_eq!(plan(Os::Windows, Codec::H264, true), vec![MediaFoundationHardware, MediaFoundationSoftware, OpenH264]);
        assert_eq!(plan(Os::Windows, Codec::H264, false), vec![MediaFoundationSoftware, OpenH264]);
        assert_eq!(plan(Os::Windows, Codec::Hevc, true), vec![MediaFoundationHardware, MediaFoundationSoftware], "OpenH264 makes no HEVC");
        assert_eq!(plan(Os::Mac, Codec::H264, true), vec![VideoToolboxHardware, VideoToolboxSoftware, OpenH264]);
        assert_eq!(plan(Os::Mac, Codec::Hevc, false), vec![VideoToolboxSoftware]);
        assert_eq!(plan(Os::Linux, Codec::H264, true), vec![VaApi, OpenH264]);
        assert_eq!(plan(Os::Linux, Codec::H264, false), vec![OpenH264]);
        assert!(plan(Os::Linux, Codec::Hevc, true).is_empty(), "no HEVC encoder on Linux yet");
        assert_eq!(plan(Os::Other, Codec::H264, true), vec![OpenH264]);
        for os in [Os::Windows, Os::Mac, Os::Linux, Os::Other] {
            for hw in [true, false] {
                let p = plan(os, Codec::H264, hw);
                assert_eq!(p.last(), Some(&OpenH264), "H.264 always ends with OpenH264");
                assert_eq!(p.iter().any(|b| b.hardware()), hw && os != Os::Other, "{os:?} hardware {hw}");
            }
        }
    }

    #[test]
    fn hevc_that_no_encoder_makes_says_so() {
        if Os::current() == Os::Linux {
            let e = open_video(&VideoConfig { codec: Codec::Hevc, ..cfg(64, 64) }).err().unwrap();
            assert!(e.contains("HEVC"), "{e}");
        }
        assert!(open_video(&cfg(63, 64)).is_err(), "bad configs are refused before any encoder starts");
    }

    fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        (0..w * h).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect()
    }

    #[test]
    fn colours_convert_to_video_range_bt709() {
        for (rgb, yuv) in [
            ([0, 0, 0], (16, 128, 128)),
            ([255, 255, 255], (235, 128, 128)),
            ([255, 0, 0], (63, 102, 240)),
            ([0, 255, 0], (172, 42, 26)),
            ([0, 0, 255], (32, 240, 118)),
        ] {
            let img = solid(4, 2, rgb);
            let (mut y, mut u, mut v) = (vec![0; 8], vec![0; 2], vec![0; 2]);
            rgba_to_i420(&img, 4, 2, &mut y, &mut u, &mut v);
            assert!(y.iter().all(|&p| p == yuv.0), "{rgb:?}: Y {y:?}");
            for (got, want) in [(u[0], yuv.1), (v[0], yuv.2)] {
                assert!((got as i32 - want as i32).abs() <= 1, "{rgb:?}: got {got}, want {want}");
            }
            assert_eq!((u[0], v[0]), (u[1], v[1]));
        }
    }

    #[test]
    fn nv12_and_i420_hold_the_same_picture_in_their_own_layouts() {
        // 4 × 4: left half black, right half white, bottom-right block red.
        let mut img = vec![0u8; 4 * 4 * 4];
        for y in 0..4 {
            for x in 0..4 {
                let p = &mut img[(y * 4 + x) * 4..][..4];
                let c = if x >= 2 && y >= 2 { [255, 0, 0] } else if x >= 2 { [255, 255, 255] } else { [0, 0, 0] };
                p.copy_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        let (mut y, mut u, mut v) = (vec![0; 16], vec![0; 4], vec![0; 4]);
        rgba_to_i420(&img, 4, 4, &mut y, &mut u, &mut v);
        assert_eq!(&y[..4], &[16, 16, 235, 235]);
        assert_eq!(&y[12..], &[16, 16, 63, 63]);
        let mut n = Vec::new();
        nv12(&img, 4, 4, &mut n);
        assert_eq!(n.len(), 24);
        assert_eq!(&n[..16], &y[..]);
        assert_eq!(&n[16..], &[u[0], v[0], u[1], v[1], u[2], v[2], u[3], v[3]]);
        assert_eq!((u[3], v[3]), chroma(255, 0, 0), "the red block's chroma");
        // Strides wider than the picture leave the padding alone.
        let (mut ys, mut uvs) = (vec![9u8; 6 * 4], vec![9u8; 6 * 2]);
        rgba_to_nv12(&img, 4, 4, &mut ys, 6, &mut uvs, 6);
        assert_eq!(&ys[..6], &[16, 16, 235, 235, 9, 9]);
        assert_eq!(&uvs[4..6], &[9, 9]);
    }

    #[test]
    fn a_2x2_block_averages_its_colours() {
        let mut img = solid(2, 2, [0, 0, 0]);
        img[0..3].copy_from_slice(&[255, 255, 255]);
        let (mut y, mut u, mut v) = (vec![0; 4], vec![0; 1], vec![0; 1]);
        rgba_to_i420(&img, 2, 2, &mut y, &mut u, &mut v);
        assert_eq!(y, vec![235, 16, 16, 16]);
        assert_eq!((u[0], v[0]), (128, 128), "grey has no colour");
    }

    #[test]
    fn annex_b_splits_on_three_and_four_byte_start_codes() {
        let s = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 4, 5, 6, 0];
        let n = split_annex_b(&s);
        assert_eq!(n, vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 4, 5, 6][..]]);
        assert!(split_annex_b(&[]).is_empty());
        assert!(split_annex_b(&[1, 2, 3]).is_empty(), "no start code, no units");
        assert!(split_annex_b(&[0, 0, 1]).is_empty(), "an empty unit is dropped");
    }

    #[test]
    fn avcc_turns_into_annex_b() {
        let a = [0, 0, 0, 3, 0x67, 1, 2, 0, 0, 0, 2, 0x68, 3];
        assert_eq!(avcc_to_annex_b(&a, 4).unwrap(), vec![0, 0, 0, 1, 0x67, 1, 2, 0, 0, 0, 1, 0x68, 3]);
        assert_eq!(avcc_to_annex_b(&[0, 2, 0x41, 9], 2).unwrap(), vec![0, 0, 0, 1, 0x41, 9]);
        assert_eq!(avcc_to_annex_b(&[1, 0x41], 1).unwrap(), vec![0, 0, 0, 1, 0x41]);
        assert!(avcc_to_annex_b(&[0, 0, 0, 9, 1], 4).is_err(), "too short");
        assert!(avcc_to_annex_b(&[0, 0], 4).is_err());
        assert!(avcc_to_annex_b(&[1, 1], 3).is_err());
        assert!(avcc_to_annex_b(&[], 4).unwrap().is_empty());
    }

    #[test]
    fn parameter_sets_and_keyframes_are_found() {
        let h264 = [0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1, 0x65, 0x88];
        assert_eq!(parameter_sets(Codec::H264, &h264), vec![&[0x67, 0x42][..], &[0x68, 0xce][..]]);
        assert_eq!(sps_pps(&h264), Some((vec![0x67, 0x42], vec![0x68, 0xce])));
        assert!(is_keyframe(Codec::H264, &h264));
        assert!(!is_keyframe(Codec::H264, &[0, 0, 0, 1, 0x41, 0x9a]));
        assert_eq!(sps_pps(&[0, 0, 0, 1, 0x41, 0x9a]), None);
        // HEVC: VPS 32, SPS 33, PPS 34 (type << 1), IDR_W_RADL 19, TRAIL_R 1.
        let hevc = [0, 0, 0, 1, 0x40, 1, 0, 0, 0, 1, 0x42, 1, 0, 0, 0, 1, 0x44, 1, 0, 0, 0, 1, 0x26, 1];
        assert_eq!(parameter_sets(Codec::Hevc, &hevc).len(), 3);
        assert!(is_keyframe(Codec::Hevc, &hevc));
        assert!(!is_keyframe(Codec::Hevc, &[0, 0, 0, 1, 0x02, 1]));
        assert_eq!(nal_type(Codec::Hevc, &[0x26, 1]), 19);
        assert_eq!(nal_type(Codec::H264, &[]), 0);
    }

    #[test]
    fn keyframes_without_parameter_sets_get_the_last_ones() {
        let mut sets = ParamSets::default();
        let mut first = VideoPacket { data: vec![0, 0, 0, 1, 0x67, 7, 0, 0, 0, 1, 0x68, 8, 0, 0, 0, 1, 0x65, 5], pts_us: 0, dts_us: 0, keyframe: true };
        let copy = first.data.clone();
        sets.complete(Codec::H264, &mut first);
        assert_eq!(first.data, copy, "a keyframe with its sets stays as it is");
        let mut p = VideoPacket { data: vec![0, 0, 0, 1, 0x41, 1], pts_us: 33, dts_us: 33, keyframe: false };
        sets.complete(Codec::H264, &mut p);
        assert_eq!(p.data, vec![0, 0, 0, 1, 0x41, 1], "other frames are left alone");
        let mut k = VideoPacket { data: vec![0, 0, 1, 0x65, 9], pts_us: 66, dts_us: 66, keyframe: true };
        sets.complete(Codec::H264, &mut k);
        assert_eq!(k.data, vec![0, 0, 0, 1, 0x67, 7, 0, 0, 0, 1, 0x68, 8, 0, 0, 1, 0x65, 9]);
    }

    #[test]
    fn keyframes_are_due_on_time_with_half_a_frame_to_spare() {
        let mut c = Cadence::new(&VideoConfig { keyframe_every_s: 2, ..cfg(64, 64) });
        assert!(c.due(0), "the first frame is a keyframe");
        c.forced(0);
        assert!(!c.due(1_000_000));
        assert!(!c.due(1_900_000));
        assert!(c.due(1_990_000), "half a frame early counts");
        assert!(c.due(2_000_000));
        c.saw_keyframe(1_500_000);
        assert!(!c.due(2_000_000), "a keyframe the encoder made by itself counts too");
        c.saw_keyframe(100);
        assert_eq!(c.last, Some(1_500_000), "an older keyframe doesn't move the clock back");
    }

    #[test]
    fn the_aac_config_for_48khz_stereo() {
        assert_eq!(aac_lc_config(AUDIO_RATE, AUDIO_CHANNELS), vec![0x11, 0x90]);
        assert_eq!(aac_lc_config(44_100, 2), vec![0x12, 0x10]);
        assert_eq!(aac_lc_config(48_000, 1), vec![0x11, 0x88]);
        assert_eq!(aac_packet_pts(1000, 0), 1000);
        assert_eq!(aac_packet_pts(0, 3), 64_000, "3 × 1024 samples at 48 kHz");
        assert_eq!(aac_packet_pts(0, 375), 8_000_000, "375 packets are exactly 8 seconds");
    }

    #[test]
    fn sound_encoders_give_aac_lc_or_say_why_not() {
        match open_audio(128_000) {
            Ok(e) => assert_eq!(e.audio_specific_config(), vec![0x11, 0x90], "{}", e.name().name),
            Err(e) => {
                assert!(!cfg!(target_os = "macos"), "macOS always has AudioToolbox: {e}");
                assert!(e.contains("AAC"), "{e}");
            }
        }
    }

    #[test]
    fn vendors_are_read_from_encoder_names() {
        assert_eq!(vendor_in("Intel® Quick Sync Video H.264 Encoder MFT"), "Intel");
        assert_eq!(vendor_in("VA-API H.264 (Mesa Gallium driver for AMD Radeon RX 6600)"), "AMD");
        assert_eq!(vendor_in("NVIDIA H.264 Encoder MFT"), "NVIDIA");
        assert_eq!(vendor_in("VideoToolbox H.264 (Apple media engine)"), "Apple");
        assert_eq!(vendor_in("OpenH264 (software)"), "");
    }

    #[test]
    fn openh264_is_always_listed() {
        let v = available();
        assert!(v.iter().any(|(c, n)| *c == Codec::H264 && n.name == soft::NAME && !n.hardware));
    }

    #[test]
    fn wrong_picture_sizes_and_backward_time_are_handled() {
        let c = cfg(64, 64);
        let mut e = open_video(&c).unwrap();
        assert!(e.encode(&[0u8; 10], 0, false).is_err());
        let img = solid(64, 64, [10, 200, 30]);
        let a = e.encode(&img, 1000, false).unwrap();
        let b = e.encode(&img, 1000, false).unwrap();
        let all: Vec<_> = a.into_iter().chain(b).chain(e.finish().unwrap()).collect();
        assert!(all.windows(2).all(|w| w[1].pts_us > w[0].pts_us), "time stamps keep going forward");
    }

    /// Runs only where a graphics-chip encoder opens; prints "skipped" elsewhere (CI, VMs).
    #[test]
    fn hardware_smoke_test() {
        let backend = plan(Os::current(), Codec::H264, true).into_iter().find(|b| b.hardware());
        let Some(b) = backend else {
            println!("hardware encoder smoke test skipped: none on this system");
            return;
        };
        let c = VideoConfig { prefer_hardware: true, ..cfg(320, 240) };
        let mut e = match open_backend(b, &c) {
            Ok(e) => Box::new(Paced::new(e, &c)) as Box<dyn VideoEncoder>,
            Err(err) => {
                println!("hardware encoder smoke test skipped: {err}");
                return;
            }
        };
        assert!(e.name().hardware);
        let mut img = vec![0u8; c.rgba_len()];
        let mut out = Vec::new();
        for i in 0..45 {
            test_picture(&mut img, 320, 240, i);
            out.extend(e.encode(&img, i as i64 * c.frame_us(), false).unwrap());
        }
        out.extend(e.finish().unwrap());
        println!("hardware encoder smoke test: {} made {} packets", e.name().name, out.len());
        assert!(out.len() >= 40, "{} packets", out.len());
        assert!(out[0].keyframe && sps_pps(&out[0].data).is_some(), "starts with a keyframe and its SPS/PPS");
        let (sps, _) = sps_pps(&out[0].data).unwrap();
        assert_eq!(h264::parse_sps(&sps).map(|s| (s.width, s.height)), Some((320, 240)));
        assert!(out.iter().skip(1).any(|p| p.keyframe && p.pts_us >= 900_000), "a keyframe again after a second");
        assert!(out.windows(2).all(|w| w[1].pts_us > w[0].pts_us));
    }
}
