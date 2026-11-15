//! Video and sound encoders that hand back the compressed data itself (not a finished file), for
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
    /// Pictures the encoder threw away so far because it couldn't keep up (VideoToolbox in
    /// real-time mode may); 0 for encoders that never do.
    fn dropped(&self) -> u64 {
        0
    }
    /// Encode a picture that is already in graphics memory in the encoder's input format (see
    /// `Surface`): no copy through the processor. Only encoders opened with
    /// `open_video_for_surfaces` take them.
    #[cfg(zero_copy)]
    fn encode_surface(&mut self, surface: &Surface, pts_us: i64, force_keyframe: bool) -> Result<Vec<VideoPacket>, String> {
        let _ = (surface, pts_us, force_keyframe);
        Err("this encoder takes pictures from memory only".into())
    }
}

// ---------------------------------------------------------------------------------------------
// Zero-copy: pictures that stay on the graphics chip from the screen copy to the encoder
// ---------------------------------------------------------------------------------------------

/// How pictures travel from the source to the encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameFormat {
    /// RGBA in memory, converted to NV12 by the processor (`rgba_to_nv12`): every system.
    Rgba,
    /// Windows: an NV12 Direct3D 11 texture made by the graphics chip's video processor from
    /// the Desktop Duplication texture, handed to the Media Foundation encoder as a DXGI surface.
    D3d11Nv12,
    /// macOS: an IOSurface-backed CVPixelBuffer in NV12 video range ('420v') from the display
    /// stream, handed straight to VideoToolbox.
    PixelBuffer,
}

/// A picture in graphics memory, already NV12 at the stream's size.
#[cfg(zero_copy)]
#[derive(Clone)]
pub enum Surface {
    #[cfg(windows)]
    D3d11(win::D3dTexture),
    #[cfg(target_os = "macos")]
    PixelBuffer(mac::PixelBuffer),
}

/// Where a source's surfaces live, so the encoder can be opened on the same device.
#[cfg(zero_copy)]
#[derive(Clone)]
pub enum SurfaceDevice {
    #[cfg(windows)]
    D3d11(win::D3dDevice),
    /// VideoToolbox takes any CVPixelBuffer: nothing to share.
    #[cfg(target_os = "macos")]
    CoreVideo,
}

#[cfg(zero_copy)]
impl SurfaceDevice {
    pub fn format(&self) -> FrameFormat {
        match *self {
            #[cfg(windows)]
            SurfaceDevice::D3d11(_) => FrameFormat::D3d11Nv12,
            #[cfg(target_os = "macos")]
            SurfaceDevice::CoreVideo => FrameFormat::PixelBuffer,
        }
    }
}

/// The first of the formats a source `offers` (best first) that the encoder `accepts`.
pub fn negotiate(offers: &[FrameFormat], accepts: &[FrameFormat]) -> Option<FrameFormat> {
    offers.iter().find(|f| accepts.contains(f)).copied()
}

/// The formats a source on `os` can offer, best first. Zero-copy only where the system has
/// it: Windows when the graphics chip encodes (Media Foundation's software encoders want
/// memory), macOS always (VideoToolbox's software encoder takes pixel buffers too); never when
/// `disabled` (`SNAGSTER_ZERO_COPY=0`, for measuring the difference).
pub fn source_formats(os: Os, prefer_hardware: bool, disabled: bool) -> Vec<FrameFormat> {
    let mut v = Vec::new();
    if !disabled {
        match os {
            Os::Windows if prefer_hardware => v.push(FrameFormat::D3d11Nv12),
            Os::Mac => v.push(FrameFormat::PixelBuffer),
            _ => {}
        }
    }
    v.push(FrameFormat::Rgba);
    v
}

/// The formats encoders opened on a device whose surfaces are `device` take: those, and memory.
pub fn encoder_formats(device: Option<FrameFormat>) -> Vec<FrameFormat> {
    let mut v: Vec<FrameFormat> = device.into_iter().collect();
    v.push(FrameFormat::Rgba);
    v
}

/// `SNAGSTER_ZERO_COPY=0` turns zero-copy off (to compare it with the copying path).
pub fn zero_copy_disabled() -> bool {
    std::env::var("SNAGSTER_ZERO_COPY").is_ok_and(|v| v.trim() == "0")
}

/// An encoder that takes `dev`'s surfaces (`VideoEncoder::encode_surface`): on Windows a
/// graphics-chip Media Foundation encoder given the device through a DXGI device manager; on
/// macOS VideoToolbox (hardware first). No software fallback: the caller then uses memory.
///
/// `pick`: the encoder the person chose ("" automatic): only that one is tried, so a pick
/// that can't take surfaces (a software encoder on Windows) makes the caller use memory, where
/// `open_video_picked` falls back and says so. `avoid`: encoders that failed while running.
#[cfg(zero_copy)]
pub fn open_video_for_surfaces(cfg: &VideoConfig, dev: &SurfaceDevice, pick: &str, avoid: &[String]) -> Result<Box<dyn VideoEncoder>, String> {
    cfg.validate()?;
    let pick = pick.trim();
    match *dev {
        #[cfg(windows)]
        SurfaceDevice::D3d11(ref d) => {
            let want = if pick.is_empty() { win::Want::Not(avoid.to_vec()) } else { win::Want::Only(pick.to_string()) };
            Ok(Box::new(Paced::new(win::open_video_d3d(cfg, d, &want)?, cfg)))
        }
        #[cfg(target_os = "macos")]
        SurfaceDevice::CoreVideo => {
            let mut tried = Vec::new();
            for hardware in [true, false] {
                match mac::open_video(cfg, hardware) {
                    Ok(e) if wanted(&e.name().name, pick, avoid) => return Ok(Box::new(Paced::new(e, cfg))),
                    Ok(e) => tried.push(format!("{}: not wanted", e.name().name)),
                    Err(e) => tried.push(e),
                }
            }
            Err(tried.join("; "))
        }
    }
}

/// Whether the encoder `name` may be used: it is the pick (or there is none) and it hasn't
/// failed already.
pub fn wanted(name: &str, pick: &str, avoid: &[String]) -> bool {
    (pick.is_empty() || name == pick) && !avoid.iter().any(|a| a == name)
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
#[cfg(windows)]
pub use win::{D3dDevice, D3dTexture};

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub use mac::PixelBuffer;

#[cfg(target_os = "linux")]
mod linux;

/// Sound sample rate and channels the AAC encoders take (the same as `audio::Capture` gives).
pub const AUDIO_RATE: u32 = 48_000;
#[cfg(any(windows, target_os = "macos", test))]
pub const AUDIO_CHANNELS: u32 = 2;
/// Sample frames in one AAC-LC packet.
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

/// `open_video` with the person's choice: `pick` is an encoder's name as `available` gives it
/// ("" automatic). A pick that can't start (gone, doesn't make this codec, its driver fails)
/// never stops the recording or stream: the best encoder that works is used instead, and the
/// second value says so for the status line.
pub fn open_video_picked(cfg: &VideoConfig, pick: &str) -> Result<(Box<dyn VideoEncoder>, Option<String>), String> {
    let pick = pick.trim();
    if pick.is_empty() {
        return open_video(cfg).map(|e| (e, None));
    }
    match open_named(cfg, pick) {
        Ok(e) => Ok((e, None)),
        Err(why) => {
            let e = open_video(cfg)?;
            let note = fallback_note(pick, &why, &e.name().name);
            Ok((e, Some(note)))
        }
    }
}

/// Whether `pick` names a software encoder of this computer (and no graphics-chip one).
pub fn pick_is_software(pick: &str) -> bool {
    let pick = pick.trim();
    if pick.is_empty() {
        return false;
    }
    let here = available();
    here.iter().any(|(_, n)| n.name == pick && !n.hardware) && !here.iter().any(|(_, n)| n.name == pick && n.hardware)
}

/// What the status line says when the chosen encoder couldn't start.
pub fn fallback_note(pick: &str, why: &str, used: &str) -> String {
    crate::i18n::tf("The chosen encoder {pick} couldn't start ({error}), so {used} is used.", &[("pick", pick), ("error", why), ("used", used)])
}

/// What the status line says when an encoder stopped working and another took over.
pub fn switched_note(failed: &str, why: &str, used: &str) -> String {
    crate::i18n::tf("{failed} stopped working ({error}); switched to {used}.", &[("failed", failed), ("error", why), ("used", used)])
}

/// The encoder called `name` (as `available` names it), and no other.
pub fn open_named(cfg: &VideoConfig, name: &str) -> Result<Box<dyn VideoEncoder>, String> {
    cfg.validate()?;
    let here = available();
    if !here.iter().any(|(_, n)| n.name == name) {
        return Err("it isn't on this computer".into());
    }
    if !here.iter().any(|(c, n)| n.name == name && *c == cfg.codec) {
        return Err(format!("it doesn't make {}", cfg.codec.label()));
    }
    let e: Box<dyn VideoEncoder> = if name == soft::NAME {
        Box::new(soft::OpenH264::open(cfg)?)
    } else {
        open_system_named(cfg, name, &here)?
    };
    Ok(Box::new(Paced::new(e, cfg)))
}

#[allow(unused_variables)]
fn open_system_named(cfg: &VideoConfig, name: &str, here: &[(Codec, EncoderName)]) -> Result<Box<dyn VideoEncoder>, String> {
    #[cfg(windows)]
    return win::open_named(cfg, name);
    #[cfg(target_os = "macos")]
    {
        let hardware = here.iter().any(|(_, n)| n.name == name && n.hardware);
        let e = mac::open_video(cfg, hardware)?;
        return if e.name().name == name { Ok(e) } else { Err(format!("macOS gave {} instead", e.name().name)) };
    }
    #[cfg(target_os = "linux")]
    return linux::open_video(cfg);
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    Err("no system encoders here".into())
}

/// The best encoder for `cfg` except those named in `avoid` (they failed while running): for
/// switching encoders without stopping. Each backend's own encoders are skipped by name.
pub fn open_video_avoiding(cfg: &VideoConfig, avoid: &[String]) -> Result<Box<dyn VideoEncoder>, String> {
    cfg.validate()?;
    let mut tried = Vec::new();
    for b in plan(Os::current(), cfg.codec, cfg.prefer_hardware) {
        let r = match b {
            #[cfg(windows)]
            Backend::MediaFoundationHardware => win::open_avoiding(cfg, true, avoid),
            #[cfg(windows)]
            Backend::MediaFoundationSoftware => win::open_avoiding(cfg, false, avoid),
            _ => open_backend(b, cfg).and_then(|e| if wanted(&e.name().name, "", avoid) { Ok(e) } else { Err("it failed already".into()) }),
        };
        match r {
            Ok(e) => return Ok(Box::new(Paced::new(e, cfg))),
            Err(e) => tried.push(format!("{}: {e}", b.label())),
        }
    }
    Err(format!("No other {} encoder could start ({}).", cfg.codec.label(), tried.join("; ")))
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
    // The system's own encoder first (Media Foundation, AudioToolbox), then Snagster's own
    // AAC encoder (src/aac), which is the only one on Linux.
    #[cfg(windows)]
    let system = win::open_audio(bitrate_bps);
    #[cfg(target_os = "macos")]
    let system = mac::open_audio(bitrate_bps);
    #[cfg(not(any(windows, target_os = "macos")))]
    let system: Result<Box<dyn AudioEncoder>, String> = Err("no system AAC encoder".into());
    system.or_else(|e| {
        OwnAac::open(bitrate_bps)
            .map(|a| Box::new(a) as Box<dyn AudioEncoder>)
            .map_err(|own| format!("{e}; Snagster's AAC encoder: {own}"))
    })
}

/// Snagster's own AAC-LC encoder (src/aac) behind the `AudioEncoder` contract.
pub struct OwnAac {
    enc: crate::aac::AacEncoder,
    first_pts: Option<i64>,
    frames: i64,
}

impl OwnAac {
    pub fn open(bitrate_bps: u32) -> Result<OwnAac, String> {
        let enc = crate::aac::AacEncoder::new(bitrate_bps.clamp(64_000, 320_000))?;
        Ok(OwnAac { enc, first_pts: None, frames: 0 })
    }

    /// Frame k plays 1024 samples after frame k-1, counted from the first sound handed in.
    fn packets(&mut self, frames: Vec<Vec<u8>>) -> Vec<AudioPacket> {
        let first = self.first_pts.unwrap_or(0);
        frames
            .into_iter()
            .map(|data| {
                let pts_us = first + self.frames * AAC_FRAME as i64 * 1_000_000 / AUDIO_RATE as i64;
                self.frames += 1;
                AudioPacket { data, pts_us }
            })
            .collect()
    }
}

impl AudioEncoder for OwnAac {
    fn encode(&mut self, pcm: &[i16], pts_us: i64) -> Result<Vec<AudioPacket>, String> {
        self.first_pts.get_or_insert(pts_us);
        let f = self.enc.encode(pcm);
        Ok(self.packets(f))
    }

    fn finish(&mut self) -> Result<Vec<AudioPacket>, String> {
        let f = self.enc.finish();
        Ok(self.packets(f))
    }

    fn audio_specific_config(&self) -> Vec<u8> {
        crate::aac::AacEncoder::audio_specific_config().to_vec()
    }

    fn name(&self) -> EncoderName {
        EncoderName { name: format!("Snagster AAC (software, {} kbit/s)", self.enc.bitrate() / 1000), hardware: false }
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
    // Only the encoder's time counts (drawing the test picture is not its work).
    let mut busy = std::time::Duration::ZERO;
    let mut out = Vec::new();
    for i in 0..frames {
        test_picture(&mut rgba, w, h, i);
        let t = std::time::Instant::now();
        out.extend(e.encode(&rgba, i as i64 * cfg.frame_us(), false)?);
        busy += t.elapsed();
    }
    let t = std::time::Instant::now();
    out.extend(e.finish()?);
    busy += t.elapsed();
    let secs = busy.as_secs_f64().max(1e-6);
    match out.first() {
        None => return Err("no frames came out".into()),
        Some(p) if !p.keyframe || sps_pps(&p.data).is_none() => return Err("the first frame isn't a keyframe with its SPS and PPS".into()),
        _ if out.len() + 4 < frames as usize => return Err(format!("{} of {frames} frames came out", out.len())),
        _ => {}
    }
    Ok((e.name(), frames as f64 / secs))
}

/// `test_picture` drawn once and then moved by copying rows: each frame is the first one
/// scrolled 2 rows up and 4 pixels left per frame number, at the speed of a memory copy, so a
/// 4K test stream isn't held up by drawing its own pictures.
pub struct MovingPicture {
    base: Vec<u8>,
    w: u32,
    h: u32,
}

impl MovingPicture {
    pub fn new(w: u32, h: u32) -> MovingPicture {
        let mut base = vec![0u8; w as usize * h as usize * 4];
        test_picture(&mut base, w, h, 0);
        MovingPicture { base, w, h }
    }

    /// Frame `n` into `out` (`w * h * 4` bytes).
    pub fn frame(&self, n: u32, out: &mut [u8]) {
        let (w, h) = (self.w as usize, self.h as usize);
        if w == 0 || h == 0 || out.len() < w * h * 4 {
            return;
        }
        let dy = (n as usize * 2) % h;
        let dx = ((n as usize * 4) % w) * 4;
        let row = w * 4;
        for (y, o) in out.chunks_exact_mut(row).take(h).enumerate() {
            let src = &self.base[((y + dy) % h) * row..][..row];
            let (a, b) = o.split_at_mut(row - dx);
            a.copy_from_slice(&src[dx..]);
            b.copy_from_slice(&src[..dx]);
        }
    }
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

    /// The time stamp and keyframe decision for the next picture. Encoders refuse time going
    /// backwards or standing still, so such frames are nudged forward.
    fn stamp(&mut self, pts_us: i64, force_keyframe: bool) -> (i64, bool) {
        let pts = match self.last_pts {
            Some(l) if pts_us <= l => l + 1,
            _ => pts_us,
        };
        self.last_pts = Some(pts);
        let key = self.cadence.due(pts) || force_keyframe;
        if key {
            self.cadence.forced(pts);
        }
        (pts, key)
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
        let (pts, key) = self.stamp(pts_us, force_keyframe);
        let p = self.inner.encode(rgba, pts, key)?;
        Ok(self.after(p))
    }

    #[cfg(zero_copy)]
    fn encode_surface(&mut self, surface: &Surface, pts_us: i64, force_keyframe: bool) -> Result<Vec<VideoPacket>, String> {
        let (pts, key) = self.stamp(pts_us, force_keyframe);
        let p = self.inner.encode_surface(surface, pts, key)?;
        Ok(self.after(p))
    }

    fn finish(&mut self) -> Result<Vec<VideoPacket>, String> {
        let p = self.inner.finish()?;
        Ok(self.after(p))
    }

    fn name(&self) -> EncoderName {
        self.inner.name()
    }

    fn dropped(&self) -> u64 {
        self.inner.dropped()
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

/// One 2 × 2 block: two RGBA pixels from the upper row (`t`, 8 bytes) and two from the lower
/// (`b`): the four lumas (upper pair, lower pair) and the block's average chroma.
#[inline(always)]
fn block(t: &[u8], b: &[u8]) -> ([u8; 2], [u8; 2], u8, u8) {
    let px = |s: &[u8], i: usize| (s[i] as i32, s[i + 1] as i32, s[i + 2] as i32);
    let (p0, p1, p2, p3) = (px(t, 0), px(t, 4), px(b, 0), px(b, 4));
    let (u, v) = chroma((p0.0 + p1.0 + p2.0 + p3.0 + 2) >> 2, (p0.1 + p1.1 + p2.1 + p3.1 + 2) >> 2, (p0.2 + p1.2 + p2.2 + p3.2 + 2) >> 2);
    ([luma(p0.0, p0.1, p0.2), luma(p1.0, p1.1, p1.2)], [luma(p2.0, p2.1, p2.2), luma(p3.0, p3.1, p3.2)], u, v)
}

/// Where one pair of rows' chroma goes.
enum ChromaRow<'a> {
    /// NV12: U and V interleaved.
    Nv12(&'a mut [u8]),
    /// I420: separate U and V rows.
    I420(&'a mut [u8], &'a mut [u8]),
}

/// Two rows of RGBA (`w` pixels each) to two rows of Y and one of chroma. Iterators instead of
/// indices, so there are no bounds checks inside the loop.
#[inline(always)]
fn row_pair(top: &[u8], bottom: &[u8], y0: &mut [u8], y1: &mut [u8], c: ChromaRow) {
    let blocks = top.chunks_exact(8).zip(bottom.chunks_exact(8)).zip(y0.chunks_exact_mut(2).zip(y1.chunks_exact_mut(2)));
    match c {
        ChromaRow::Nv12(uv) => {
            for (((t, b), (ya, yb)), uv) in blocks.zip(uv.chunks_exact_mut(2)) {
                let (a, l, u, v) = block(t, b);
                ya.copy_from_slice(&a);
                yb.copy_from_slice(&l);
                uv[0] = u;
                uv[1] = v;
            }
        }
        ChromaRow::I420(us, vs) => {
            for (((t, b), (ya, yb)), (u, v)) in blocks.zip(us.iter_mut().zip(vs.iter_mut())) {
                let (a, l, cu, cv) = block(t, b);
                ya.copy_from_slice(&a);
                yb.copy_from_slice(&l);
                *u = cu;
                *v = cv;
            }
        }
    }
}

/// Bands of rows converted side by side for a picture of `w` × `h`: one for small pictures (a
/// thread costs more than it saves), else one per core up to 8.
pub(crate) fn bands_for(w: usize, h: usize) -> usize {
    if w * h < 640 * 360 {
        return 1;
    }
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 8).min(h / 2)
}

/// Run `f(rgba rows, [plane rows...])` over bands of row pairs on several
/// threads. Each band gets its own slices of every plane: `planes` are (buffer, bytes per row
/// pair) and are cut at the same row pairs.
fn in_bands<const N: usize>(w: usize, h: usize, rgba: &[u8], mut planes: [(&mut [u8], usize); N], f: impl Fn(&[u8], [&mut [u8]; N]) + Sync) {
    let pairs = h / 2;
    let bands = bands_for(w, h);
    let per = pairs.div_ceil(bands.max(1));
    std::thread::scope(|s| {
        let mut rest = rgba;
        let mut start = 0;
        while start < pairs {
            let n = per.min(pairs - start);
            let (src, r) = rest.split_at((n * 2 * w * 4).min(rest.len()));
            rest = r;
            let outs: [&mut [u8]; N] = std::array::from_fn(|k| {
                let (buf, step) = &mut planes[k];
                let cut = (n * *step).min(buf.len());
                let (mine, others) = std::mem::take(buf).split_at_mut(cut);
                *buf = others;
                mine
            });
            let f = &f;
            if start + n >= pairs {
                f(src, outs);
            } else {
                s.spawn(move || f(src, outs));
            }
            start += n;
        }
    });
}

/// Row `i` (`w` bytes) of a plane with rows `stride` apart, or nothing when it isn't there.
fn plane_row(p: &mut [u8], i: usize, stride: usize, w: usize) -> &mut [u8] {
    let at = (i * stride).min(p.len());
    let end = (at + w).min(p.len());
    &mut p[at..end]
}

/// RGBA to NV12: a full-size Y plane, then one half-size plane of interleaved U, V.
pub fn rgba_to_nv12(rgba: &[u8], w: u32, h: u32, y: &mut [u8], y_stride: usize, uv: &mut [u8], uv_stride: usize) {
    let (w, h) = (w as usize, h as usize);
    debug_assert!(w % 2 == 0 && h % 2 == 0 && rgba.len() >= w * h * 4);
    in_bands(w, h, rgba, [(y, 2 * y_stride), (uv, uv_stride)], |src, [y, uv]| {
        for (j, rows) in src.chunks_exact(w * 8).enumerate() {
            let (top, bottom) = rows.split_at(w * 4);
            let (upper, lower) = y.split_at_mut(((2 * j + 1) * y_stride).min(y.len()));
            let y0 = plane_row(upper, 2 * j, y_stride, w);
            let y1 = plane_row(lower, 0, y_stride, w);
            row_pair(top, bottom, y0, y1, ChromaRow::Nv12(plane_row(uv, j, uv_stride, w)));
        }
    });
}

/// RGBA to I420 (three planes: Y, then U, then V; strides `w`, `w/2`, `w/2`).
pub fn rgba_to_i420(rgba: &[u8], w: u32, h: u32, y: &mut [u8], u: &mut [u8], v: &mut [u8]) {
    let (w, h) = (w as usize, h as usize);
    debug_assert!(w % 2 == 0 && h % 2 == 0 && rgba.len() >= w * h * 4);
    let half = w / 2;
    in_bands(w, h, rgba, [(y, 2 * w), (u, half), (v, half)], |src, [y, u, v]| {
        for (((rows, ys), us), vs) in src.chunks_exact(w * 8).zip(y.chunks_exact_mut(2 * w)).zip(u.chunks_exact_mut(half)).zip(v.chunks_exact_mut(half)) {
            let (top, bottom) = rows.split_at(w * 4);
            let (y0, y1) = ys.split_at_mut(w);
            row_pair(top, bottom, y0, y1, ChromaRow::I420(us, vs));
        }
    });
}

/// A contiguous NV12 picture of `w` × `h` (for the tests and the graphics-chip conversion check).
#[cfg(any(test, windows))]
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

    #[test]
    fn snagsters_own_aac_makes_evenly_timed_packets_and_says_what_it_is() {
        let mut a = OwnAac::open(128_000).unwrap();
        assert_eq!(a.audio_specific_config(), vec![0x11, 0x90]);
        assert_eq!(a.name(), EncoderName { name: "Snagster AAC (software, 128 kbit/s)".into(), hardware: false });
        // One second of a 440 Hz tone in odd-sized pieces, starting at 5 s.
        let tone: Vec<i16> = (0..48_000).flat_map(|i| {
            let v = ((i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 8000.0) as i16;
            [v, v]
        }).collect();
        let mut out = Vec::new();
        for (k, piece) in tone.chunks(2 * 777).enumerate() {
            out.extend(a.encode(piece, 5_000_000 + k as i64 * 16_187).unwrap());
        }
        out.extend(a.finish().unwrap());
        // ~47 frames per second (plus the encoder's delay), 21 333.3 µs apart, from the first time
        // given; counted from the start, so rounding never adds up.
        assert!((47..=50).contains(&out.len()), "{} packets", out.len());
        assert_eq!(out[0].pts_us, 5_000_000);
        for w in out.windows(2) {
            assert!((21_333..=21_334).contains(&(w[1].pts_us - w[0].pts_us)), "packets not evenly spaced");
        }
        let n = out.len() as i64 - 1;
        assert_eq!(out[n as usize].pts_us, 5_000_000 + n * 1024 * 1_000_000 / 48_000);
        assert!(out.iter().all(|p| !p.data.is_empty()));
        // Out-of-range rates are brought into what the encoder takes.
        assert!(OwnAac::open(8_000).is_ok());
        assert!(OwnAac::open(1_000_000).is_ok());
    }

    pub(crate) fn cfg(w: u32, h: u32) -> VideoConfig {
        VideoConfig { width: w, height: h, fps: 30, bitrate_bps: 2_000_000, keyframe_every_s: 1, codec: Codec::H264, prefer_hardware: false, low_latency: true }
    }

    /// The person's encoder choice: Snagster's own encoder is on every system, so choosing it
    /// works everywhere; a choice that isn't there (or doesn't make HEVC) never fails the
    /// recording: the best encoder that works is used and the note says why.
    #[test]
    fn a_chosen_encoder_is_used_and_a_bad_choice_falls_back_with_a_note() {
        let (e, note) = open_video_picked(&cfg(64, 64), soft::NAME).unwrap();
        assert_eq!((e.name().name.as_str(), note), (soft::NAME, None));
        let (e, note) = open_video_picked(&cfg(64, 64), "").unwrap();
        assert!(note.is_none() && !e.name().name.is_empty(), "automatic says nothing");
        let (e, note) = open_video_picked(&cfg(64, 64), "  Gone Encoder 9000  ").unwrap();
        let note = note.expect("the fallback is said");
        assert_eq!(note, format!("The chosen encoder Gone Encoder 9000 couldn't start (it isn't on this computer), so {} is used.", e.name().name));
        // OpenH264 makes H.264 only.
        let hevc = VideoConfig { codec: Codec::Hevc, ..cfg(64, 64) };
        assert_eq!(open_named(&hevc, soft::NAME).err().unwrap(), "it doesn't make HEVC");
        assert!(pick_is_software(soft::NAME));
        assert!(!pick_is_software("") && !pick_is_software("Gone Encoder 9000"));
        assert!(available().iter().any(|(_, n)| n.name == soft::NAME), "Snagster's own encoder is always offered");
    }

    /// Every encoder this computer offers opens by its name and is the one asked for, for both
    /// codecs it makes, and encodes a keyframe. Needs the real graphics chip:
    /// `cargo test -- --ignored every_offered_encoder`.
    #[test]
    #[ignore]
    fn every_offered_encoder_opens_by_its_name() {
        for (codec, n) in available() {
            let c = VideoConfig { width: 640, height: 360, codec, ..cfg(640, 360) };
            let mut e = open_named(&c, &n.name).unwrap_or_else(|err| panic!("{} ({}): {err}", n.name, codec.label()));
            assert_eq!(e.name().name, n.name);
            let mut out = Vec::new();
            for i in 0..5 {
                out.extend(e.encode(&vec![90u8; c.rgba_len()], i * c.frame_us(), i == 0).unwrap());
            }
            out.extend(e.finish().unwrap());
            assert!(out.first().is_some_and(|p| p.keyframe), "{}: no keyframe first", n.name);
            println!("ok  {} {} ({})", codec.label(), n.name, if n.hardware { "graphics chip" } else { "processor" });
        }
    }

    #[test]
    fn switching_encoders_skips_those_that_failed() {
        assert!(wanted("A", "", &[]));
        assert!(!wanted("A", "", &["A".to_string()]));
        assert!(wanted("A", "A", &[]) && !wanted("B", "A", &[]));
        // Everything here failed already: nothing left, and it says so.
        let all: Vec<String> = available().into_iter().map(|(_, n)| n.name).collect();
        let e = open_video_avoiding(&cfg(64, 64), &all).err().expect("every encoder was avoided");
        assert!(e.starts_with("No other H.264 encoder could start"), "{e}");
        // With only the processor's encoders left it still works.
        let hw: Vec<String> = available().into_iter().filter(|(_, n)| n.hardware).map(|(_, n)| n.name).collect();
        let e = open_video_avoiding(&cfg(64, 64), &hw).unwrap();
        assert!(!hw.contains(&e.name().name), "{}", e.name().name);
        assert_eq!(switched_note("Chip", "reset", "OpenH264"), "Chip stopped working (reset); switched to OpenH264.");
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

    /// The plain way, one pixel at a time: what the banded, iterator-based conversion must match.
    fn reference_420(rgba: &[u8], w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let (mut y, mut u, mut v) = (vec![0; w * h], vec![0; w * h / 4], vec![0; w * h / 4]);
        let at = |x: usize, r: usize, c: usize| rgba[(r * w + x) * 4 + c] as i32;
        for r in 0..h {
            for x in 0..w {
                y[r * w + x] = luma(at(x, r, 0), at(x, r, 1), at(x, r, 2));
            }
        }
        for r in 0..h / 2 {
            for x in 0..w / 2 {
                let avg = |c| (at(2 * x, 2 * r, c) + at(2 * x + 1, 2 * r, c) + at(2 * x, 2 * r + 1, c) + at(2 * x + 1, 2 * r + 1, c) + 2) >> 2;
                let (cu, cv) = chroma(avg(0), avg(1), avg(2));
                u[r * w / 2 + x] = cu;
                v[r * w / 2 + x] = cv;
            }
        }
        (y, u, v)
    }

    #[test]
    fn the_fast_conversion_matches_the_plain_one_at_every_size() {
        // Small (one band), odd numbers of row pairs per band, and big (several bands).
        for (w, h) in [(2, 2), (6, 4), (64, 38), (642, 362), (1280, 722), (1920, 1080)] {
            let mut img = vec![0u8; w * h * 4];
            test_picture(&mut img, w as u32, h as u32, 7);
            // Some noise so neighbouring pixels differ.
            let mut s = 12345u32;
            for p in img.iter_mut().step_by(3) {
                s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
                *p ^= (s >> 24) as u8;
            }
            let (ry, ru, rv) = reference_420(&img, w, h);
            let (mut y, mut u, mut v) = (vec![0; w * h], vec![0; w * h / 4], vec![0; w * h / 4]);
            rgba_to_i420(&img, w as u32, h as u32, &mut y, &mut u, &mut v);
            assert!(y == ry && u == ru && v == rv, "I420 {w}x{h}");
            let mut n = Vec::new();
            nv12(&img, w as u32, h as u32, &mut n);
            assert_eq!(&n[..w * h], &ry[..], "NV12 Y {w}x{h}");
            let inter: Vec<u8> = ru.iter().zip(&rv).flat_map(|(a, b)| [*a, *b]).collect();
            assert_eq!(&n[w * h..], &inter[..], "NV12 UV {w}x{h}");
            // Padded rows (as graphics drivers lay out surfaces): the padding stays as it was,
            // and the last row needs no padding after it.
            let (ys, uvs) = (w + 32, w + 64);
            let (mut yp, mut uvp) = (vec![7u8; ys * (h - 1) + w], vec![7u8; uvs * (h / 2 - 1) + w]);
            rgba_to_nv12(&img, w as u32, h as u32, &mut yp, ys, &mut uvp, uvs);
            for r in 0..h {
                assert_eq!(&yp[r * ys..r * ys + w], &ry[r * w..(r + 1) * w], "padded Y row {r} of {w}x{h}");
                if r + 1 < h {
                    assert!(yp[r * ys + w..(r + 1) * ys].iter().all(|&b| b == 7), "Y padding row {r}");
                }
            }
            for r in 0..h / 2 {
                assert_eq!(&uvp[r * uvs..r * uvs + w], &inter[r * w..(r + 1) * w], "padded UV row {r} of {w}x{h}");
            }
        }
    }

    #[test]
    fn small_pictures_convert_on_one_thread() {
        assert_eq!(bands_for(320, 240), 1);
        assert_eq!(bands_for(4, 2), 1);
        let b = bands_for(1920, 1080);
        assert!((1..=8).contains(&b));
        assert!(bands_for(4096, 4) <= 2, "never more bands than row pairs");
    }

    /// How long one 1080p picture takes to become NV12 and I420:
    /// `cargo test --release -- --ignored conversion_speed --nocapture`.
    #[test]
    #[ignore]
    fn conversion_speed() {
        // 1080p and 4K: the processor's share of every picture on the paths without zero-copy
        // (Linux, software encoders, test pictures).
        for (w, h) in [(1920u32, 1080u32), (3840, 2160)] {
            conversion_speed_at(w, h);
        }
    }

    fn conversion_speed_at(w: u32, h: u32) {
        let mut img = vec![0u8; (w * h * 4) as usize];
        test_picture(&mut img, w, h, 3);
        let mut n = Vec::new();
        let (mut y, mut u, mut v) = (vec![0; (w * h) as usize], vec![0; (w * h / 4) as usize], vec![0; (w * h / 4) as usize]);
        nv12(&img, w, h, &mut n);
        let runs = 200;
        let t = std::time::Instant::now();
        for _ in 0..runs {
            nv12(std::hint::black_box(&img), w, h, &mut n);
        }
        let nv = t.elapsed().as_secs_f64() * 1000.0 / runs as f64;
        let t = std::time::Instant::now();
        for _ in 0..runs {
            rgba_to_i420(std::hint::black_box(&img), w, h, &mut y, &mut u, &mut v);
        }
        let i4 = t.elapsed().as_secs_f64() * 1000.0 / runs as f64;
        println!("{w}x{h} RGBA to NV12: {nv:.2} ms, to I420: {i4:.2} ms");
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
    fn zero_copy_is_negotiated_where_source_and_encoder_share_a_format() {
        use FrameFormat::*;
        assert_eq!(source_formats(Os::Windows, true, false), vec![D3d11Nv12, Rgba]);
        assert_eq!(source_formats(Os::Windows, false, false), vec![Rgba], "Media Foundation's software encoders want memory");
        assert_eq!(source_formats(Os::Mac, false, false), vec![PixelBuffer, Rgba], "VideoToolbox's software encoder takes pixel buffers too");
        assert_eq!(source_formats(Os::Linux, true, false), vec![Rgba], "Linux copies through memory");
        assert_eq!(source_formats(Os::Windows, true, true), vec![Rgba], "SNAGSTER_ZERO_COPY=0");
        assert_eq!(encoder_formats(Some(D3d11Nv12)), vec![D3d11Nv12, Rgba]);
        assert_eq!(encoder_formats(None), vec![Rgba]);
        assert_eq!(negotiate(&source_formats(Os::Windows, true, false), &encoder_formats(Some(D3d11Nv12))), Some(D3d11Nv12));
        assert_eq!(negotiate(&source_formats(Os::Windows, true, false), &encoder_formats(None)), Some(Rgba), "an encoder in memory only");
        assert_eq!(negotiate(&source_formats(Os::Mac, true, false), &encoder_formats(Some(D3d11Nv12))), Some(Rgba), "formats that don't match");
        assert_eq!(negotiate(&[D3d11Nv12], &[Rgba]), None);
    }

    #[test]
    fn the_moving_picture_scrolls_two_rows_and_four_pixels_a_frame() {
        let (w, h) = (64u32, 32u32);
        let m = MovingPicture::new(w, h);
        let mut base = vec![0u8; (w * h * 4) as usize];
        test_picture(&mut base, w, h, 0);
        let mut out = vec![0u8; base.len()];
        m.frame(0, &mut out);
        assert_eq!(out, base);
        let px = |b: &[u8], x: u32, y: u32| b[((y * w + x) * 4) as usize..][..4].to_vec();
        for n in [1u32, 5, 17, 40] {
            m.frame(n, &mut out);
            for (x, y) in [(0, 0), (63, 31), (10, 20), (60, 3)] {
                assert_eq!(px(&out, x, y), px(&base, (x + n * 4) % w, (y + n * 2) % h), "frame {n} at {x},{y}");
            }
        }
        m.frame(3, &mut [0u8; 8]);
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
