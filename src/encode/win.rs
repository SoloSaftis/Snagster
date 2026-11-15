//! Windows: Media Foundation encoder transforms used directly (no sink writer), so the
//! compressed frames come back to Snagster instead of going into a file.
//!
//! Hardware transforms (Intel Quick Sync, NVIDIA NVENC, AMD AMF) are asynchronous: they say when
//! they want a picture (`METransformNeedInput`) and when a frame is ready (`METransformHaveOutput`)
//! through their event queue. Microsoft's software encoders are synchronous: put a picture in,
//! take frames out until they need more. Pictures go in as NV12 in system memory, or, for the
//! zero-copy path (`open_video_d3d`), as NV12 Direct3D 11 textures: the transform is given the
//! capture's device through a DXGI device manager (`MFT_MESSAGE_SET_D3D_MANAGER`, only for
//! transforms that say `MF_SA_D3D11_AWARE`) and each picture arrives as a DXGI surface buffer.
//!
//! Written from the Media Foundation documentation ("Basic MFT Processing Model", "Asynchronous
//! MFTs", "H.264 Video Encoder", "AAC Encoder", "Codec API properties").

use super::*;
use std::time::{Duration, Instant};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{GUID, Interface, PWSTR};

type R<T> = windows::core::Result<T>;

fn err(e: windows::core::Error) -> String {
    format!("{} ({:#010x})", e.message(), e.code().0 as u32)
}

/// COM (multithreaded) and Media Foundation for as long as an encoder lives.
struct Mf;

impl Mf {
    fn start() -> Result<Mf, String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).map_err(|e| format!("Media Foundation isn't available: {}", err(e)))?;
        }
        Ok(Mf)
    }
}

impl Drop for Mf {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
        }
    }
}

/// Calls from another thread than the one that opened the encoder need COM there too.
fn com_here() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
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

fn pack(a: u32, b: u32) -> u64 {
    ((a as u64) << 32) | b as u64
}

/// One registered encoder transform.
struct Candidate {
    activate: IMFActivate,
    name: String,
    hardware: bool,
}

fn enumerate(category: GUID, flags: MFT_ENUM_FLAG, input: Option<MFT_REGISTER_TYPE_INFO>, output: Option<MFT_REGISTER_TYPE_INFO>, fallback_name: &str) -> Vec<Candidate> {
    let hardware = flags.0 & MFT_ENUM_FLAG_HARDWARE.0 != 0;
    let mut v = Vec::new();
    unsafe {
        let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut n = 0u32;
        let i = input.as_ref().map(|t| t as *const _);
        let o = output.as_ref().map(|t| t as *const _);
        if MFTEnumEx(category, flags, i, o, &mut list, &mut n).is_err() || list.is_null() {
            return v;
        }
        for k in 0..n as usize {
            if let Some(a) = std::ptr::read(list.add(k)) {
                let name = a.cast::<IMFAttributes>().ok().and_then(|x| take_string(&x, &MFT_FRIENDLY_NAME_Attribute)).unwrap_or_else(|| fallback_name.to_string());
                v.push(Candidate { activate: a, name, hardware });
            }
        }
        CoTaskMemFree(Some(list as *const _));
    }
    v
}

fn video_subtype(c: Codec) -> GUID {
    match c {
        Codec::H264 => MFVideoFormat_H264,
        Codec::Hevc => MFVideoFormat_HEVC,
    }
}

fn video_candidates(codec: Codec, hardware: bool) -> Vec<Candidate> {
    let flags = if hardware { MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER } else { MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER };
    let out = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: video_subtype(codec) };
    let fallback = format!("Windows {} encoder{}", codec.label(), if hardware { " (graphics chip)" } else { " (software)" });
    enumerate(MFT_CATEGORY_VIDEO_ENCODER, flags, None, Some(out), &fallback)
}

/// The video encoders Media Foundation offers, graphics chip first.
pub fn list() -> Vec<(Codec, EncoderName)> {
    let Ok(_mf) = Mf::start() else { return Vec::new() };
    let mut v = Vec::new();
    for hardware in [true, false] {
        for codec in [Codec::H264, Codec::Hevc] {
            for c in video_candidates(codec, hardware) {
                v.push((codec, EncoderName { name: c.name, hardware }));
            }
        }
    }
    v
}

/// A Direct3D 11 device shared by a zero-copy screen copy and the encoder.
#[derive(Clone)]
pub struct D3dDevice(pub windows::Win32::Graphics::Direct3D11::ID3D11Device);

/// An NV12 texture on that device, one picture at the stream's size.
#[derive(Clone)]
pub struct D3dTexture(pub windows::Win32::Graphics::Direct3D11::ID3D11Texture2D);

// Direct3D 11 devices are free-threaded, and Snagster's are made multithread-protected
// (`video::win::device`); a texture is written by the copy before it is handed on and only read
// after.
unsafe impl Send for D3dDevice {}
unsafe impl Sync for D3dDevice {}
unsafe impl Send for D3dTexture {}
unsafe impl Sync for D3dTexture {}

pub fn open_video(cfg: &VideoConfig, hardware: bool) -> Result<Box<dyn VideoEncoder>, String> {
    open_on(cfg, hardware, None, &Want::Any)
}

/// A graphics-chip encoder that takes NV12 textures of `dev` (zero-copy).
pub fn open_video_d3d(cfg: &VideoConfig, dev: &D3dDevice, want: &Want) -> Result<Box<dyn VideoEncoder>, String> {
    open_on(cfg, true, Some(dev), want)
}

/// The encoder `name` (as `list` names it), on the graphics chip or in software, whichever it is.
pub fn open_named(cfg: &VideoConfig, name: &str) -> Result<Box<dyn VideoEncoder>, String> {
    let want = Want::Only(name.to_string());
    open_on(cfg, true, None, &want).or_else(|hw| open_on(cfg, false, None, &want).map_err(|sw| if sw.starts_with("no ") { hw } else { sw }))
}

/// Which of the registered encoder transforms may be opened.
pub enum Want {
    Any,
    /// Only the one with this name.
    Only(String),
    /// Any but these (they failed already).
    Not(Vec<String>),
}

impl Want {
    pub fn allows(&self, name: &str) -> bool {
        match self {
            Want::Any => true,
            Want::Only(n) => n == name,
            Want::Not(v) => !v.iter().any(|n| n == name),
        }
    }
}

/// Any encoder `avoid` doesn't name, graphics chip first when `hardware`.
pub fn open_avoiding(cfg: &VideoConfig, hardware: bool, avoid: &[String]) -> Result<Box<dyn VideoEncoder>, String> {
    open_on(cfg, hardware, None, &Want::Not(avoid.to_vec()))
}

fn open_on(cfg: &VideoConfig, hardware: bool, d3d: Option<&D3dDevice>, want: &Want) -> Result<Box<dyn VideoEncoder>, String> {
    let mf = Mf::start()?;
    let list: Vec<Candidate> = video_candidates(cfg.codec, hardware).into_iter().filter(|c| want.allows(&c.name)).collect();
    if list.is_empty() {
        return Err(format!("no {} encoder {}", cfg.codec.label(), if hardware { "on the graphics chip" } else { "in software" }));
    }
    let mut tried = Vec::new();
    for c in list {
        let name = c.name.clone();
        match MfVideo::open(c, cfg, d3d) {
            Ok(e) => {
                drop(mf);
                return Ok(Box::new(e));
            }
            Err(e) => tried.push(format!("{name}: {e}")),
        }
    }
    Err(tried.join("; "))
}

/// Set a codec property; encoders that don't know one leave it (they're all optional).
fn set_codec(api: &Option<ICodecAPI>, key: &GUID, v: VARIANT) {
    if let Some(api) = api {
        unsafe {
            let _ = api.SetValue(key, &v);
        }
    }
}

/// Media Foundation's sample with `bytes` in one memory buffer.
fn memory_sample(bytes: &[u8], time_100ns: i64, dur_100ns: i64) -> R<IMFSample> {
    unsafe {
        let buf = MFCreateMemoryBuffer(bytes.len() as u32)?;
        let mut dst = std::ptr::null_mut();
        buf.Lock(&mut dst, None, None)?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        buf.Unlock()?;
        buf.SetCurrentLength(bytes.len() as u32)?;
        let s = MFCreateSample()?;
        s.AddBuffer(&buf)?;
        s.SetSampleTime(time_100ns)?;
        s.SetSampleDuration(dur_100ns)?;
        Ok(s)
    }
}

/// A sample holding `rgba` as NV12, converted straight into Media Foundation's buffer (no
/// picture-sized copy in between).
fn nv12_sample(rgba: &[u8], w: u32, h: u32, time_100ns: i64, dur_100ns: i64) -> R<IMFSample> {
    let ys = (w * h) as usize;
    let len = ys + ys / 2;
    unsafe {
        let buf = MFCreateMemoryBuffer(len as u32)?;
        let mut dst = std::ptr::null_mut();
        buf.Lock(&mut dst, None, None)?;
        let out = std::slice::from_raw_parts_mut(dst, len);
        let (y, uv) = out.split_at_mut(ys);
        rgba_to_nv12(rgba, w, h, y, w as usize, uv, w as usize);
        buf.Unlock()?;
        buf.SetCurrentLength(len as u32)?;
        let s = MFCreateSample()?;
        s.AddBuffer(&buf)?;
        s.SetSampleTime(time_100ns)?;
        s.SetSampleDuration(dur_100ns)?;
        Ok(s)
    }
}

fn sample_bytes(s: &IMFSample) -> R<Vec<u8>> {
    unsafe {
        let buf = s.ConvertToContiguousBuffer()?;
        let mut p = std::ptr::null_mut();
        let mut len = 0u32;
        buf.Lock(&mut p, None, Some(&mut len))?;
        let v = std::slice::from_raw_parts(p, len as usize).to_vec();
        buf.Unlock()?;
        Ok(v)
    }
}

/// What came out of `ProcessOutput` once.
enum Out {
    Sample(IMFSample),
    NeedMore,
    StreamChanged,
}

/// Ask a transform for one output sample (allocating it when the transform doesn't).
fn process_output(t: &IMFTransform, alloc: Option<u32>) -> R<Out> {
    unsafe {
        let sample = match alloc {
            Some(n) => {
                let s = MFCreateSample()?;
                s.AddBuffer(&MFCreateMemoryBuffer(n)?)?;
                Some(s)
            }
            None => None,
        };
        let mut b = [MFT_OUTPUT_DATA_BUFFER { dwStreamID: 0, pSample: std::mem::ManuallyDrop::new(sample), dwStatus: 0, pEvents: std::mem::ManuallyDrop::new(None) }];
        let mut status = 0u32;
        let r = t.ProcessOutput(0, &mut b, &mut status);
        let s = std::mem::ManuallyDrop::take(&mut b[0].pSample);
        drop(std::mem::ManuallyDrop::take(&mut b[0].pEvents));
        match r {
            Ok(()) => Ok(s.map(Out::Sample).unwrap_or(Out::NeedMore)),
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(Out::NeedMore),
            Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => Ok(Out::StreamChanged),
            Err(e) => Err(e),
        }
    }
}

/// How big an output sample must be when the transform wants the caller to supply it.
fn output_alloc(t: &IMFTransform, at_least: u32) -> R<Option<u32>> {
    unsafe {
        let info = t.GetOutputStreamInfo(0)?;
        let provides = info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32 != 0;
        Ok(if provides { None } else { Some(info.cbSize.max(at_least)) })
    }
}

fn start_streaming(t: &IMFTransform) -> R<()> {
    unsafe {
        t.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
        t.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
    }
}

// ---------------------------------------------------------------------------------------------
// Video
// ---------------------------------------------------------------------------------------------

struct MfVideo {
    t: IMFTransform,
    /// Set for asynchronous (hardware) transforms.
    events: Option<IMFMediaEventGenerator>,
    api: Option<ICodecAPI>,
    /// Pictures the asynchronous transform has asked for and not yet got.
    need_input: u32,
    alloc: Option<u32>,
    cfg: VideoConfig,
    name: String,
    hardware: bool,
    /// The zero-copy path: the device manager the transform was given (kept alive with it).
    manager: Option<IMFDXGIDeviceManager>,
    _mf: Mf,
}

// Media Foundation transforms are free-threaded COM objects (they live in the multithreaded
// apartment); one thread at a time uses this one (`&mut self`).
unsafe impl Send for MfVideo {}

impl MfVideo {
    fn open(c: Candidate, cfg: &VideoConfig, d3d: Option<&D3dDevice>) -> Result<MfVideo, String> {
        let mf = Mf::start()?;
        unsafe {
            let t: IMFTransform = c.activate.ActivateObject().map_err(err)?;
            let attrs = t.GetAttributes().ok();
            let is_async = attrs.as_ref().is_some_and(|a| a.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0);
            if let Some(a) = &attrs {
                if is_async {
                    a.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1).map_err(err)?;
                }
                if cfg.low_latency {
                    let _ = a.SetUINT32(&MF_LOW_LATENCY, 1);
                }
            }
            // Zero-copy: give the transform the capture's device before the media types are
            // set, so it takes its pictures as textures of that device.
            let manager = match d3d {
                Some(dev) => {
                    let aware = attrs.as_ref().is_some_and(|a| a.GetUINT32(&MF_SA_D3D11_AWARE).unwrap_or(0) != 0);
                    if !aware {
                        return Err("this encoder doesn't take Direct3D 11 textures".into());
                    }
                    let (mut token, mut m) = (0u32, None);
                    MFCreateDXGIDeviceManager(&mut token, &mut m).map_err(err)?;
                    let m: IMFDXGIDeviceManager = m.ok_or("no DXGI device manager")?;
                    m.ResetDevice(&dev.0, token).map_err(err)?;
                    t.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, m.as_raw() as usize).map_err(|e| format!("the encoder won't use the screen copy's device: {}", err(e)))?;
                    Some(m)
                }
                None => None,
            };
            let api: Option<ICodecAPI> = t.cast().ok();
            let settings = |api: &Option<ICodecAPI>| {
                let mode = if cfg.low_latency { eAVEncCommonRateControlMode_CBR } else { eAVEncCommonRateControlMode_UnconstrainedVBR };
                set_codec(api, &CODECAPI_AVEncCommonRateControlMode, VARIANT::from(mode.0 as u32));
                set_codec(api, &CODECAPI_AVEncCommonMeanBitRate, VARIANT::from(cfg.bitrate_bps));
                set_codec(api, &CODECAPI_AVEncMPVGOPSize, VARIANT::from(cfg.gop_frames()));
                if cfg.low_latency {
                    set_codec(api, &CODECAPI_AVLowLatencyMode, VARIANT::from(true));
                    set_codec(api, &CODECAPI_AVEncMPVDefaultBPictureCount, VARIANT::from(0u32));
                }
            };
            settings(&api);

            let (w, h, fps) = (cfg.width, cfg.height, cfg.fps);
            let out = MFCreateMediaType().map_err(err)?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(err)?;
            out.SetGUID(&MF_MT_SUBTYPE, &video_subtype(cfg.codec)).map_err(err)?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, cfg.bitrate_bps).map_err(err)?;
            out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(err)?;
            out.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h)).map_err(err)?;
            out.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1)).map_err(err)?;
            out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).map_err(err)?;
            let profile = match cfg.codec {
                Codec::H264 => eAVEncH264VProfile_High.0 as u32,
                Codec::Hevc => eAVEncH265VProfile_Main_420_8.0 as u32,
            };
            out.SetUINT32(&MF_MT_MPEG2_PROFILE, profile).map_err(err)?;
            colours(&out);
            t.SetOutputType(0, &out, 0).map_err(|e| format!("the encoder won't make {} {w} × {h} at {fps} fps: {}", cfg.codec.label(), err(e)))?;

            // NV12 in: the transform's own offer when it lists one, filled in with the size.
            let mut inp = None;
            for i in 0..64 {
                let Ok(it) = t.GetInputAvailableType(0, i) else { break };
                if it.GetGUID(&MF_MT_SUBTYPE).ok() == Some(MFVideoFormat_NV12) {
                    inp = Some(it);
                    break;
                }
            }
            let inp = match inp {
                Some(i) => i,
                None => {
                    let i = MFCreateMediaType().map_err(err)?;
                    i.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(err)?;
                    i.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).map_err(err)?;
                    i
                }
            };
            inp.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(err)?;
            inp.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h)).map_err(err)?;
            inp.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1)).map_err(err)?;
            inp.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).map_err(err)?;
            inp.SetUINT32(&MF_MT_DEFAULT_STRIDE, w).map_err(err)?;
            colours(&inp);
            t.SetInputType(0, &inp, 0).map_err(|e| format!("the encoder won't take NV12 pictures: {}", err(e)))?;
            // Some encoders only take these once the types are known.
            settings(&api);

            let events = if is_async { Some(t.cast::<IMFMediaEventGenerator>().map_err(err)?) } else { None };
            start_streaming(&t).map_err(err)?;
            let alloc = output_alloc(&t, w * h).map_err(err)?;
            Ok(MfVideo { t, events, api, need_input: 0, alloc, cfg: cfg.clone(), name: c.name, hardware: c.hardware, manager, _mf: mf })
        }
    }

    fn packet(&self, s: &IMFSample) -> R<VideoPacket> {
        unsafe {
            let data = sample_bytes(s)?;
            let pts = s.GetSampleTime().unwrap_or(0) / 10;
            let dts = s.GetUINT64(&MFSampleExtension_DecodeTimestamp).map(|v| v as i64 / 10).unwrap_or(pts);
            let clean = s.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0;
            let keyframe = clean || is_keyframe(self.cfg.codec, &data);
            Ok(VideoPacket { data, pts_us: pts, dts_us: dts, keyframe })
        }
    }

    /// Take one output; on a format change, accept the new type and try again.
    fn take(&mut self, out: &mut Vec<VideoPacket>) -> R<bool> {
        for _ in 0..4 {
            match process_output(&self.t, self.alloc)? {
                Out::Sample(s) => {
                    let p = self.packet(&s)?;
                    if !p.data.is_empty() {
                        out.push(p);
                    }
                    return Ok(true);
                }
                Out::NeedMore => return Ok(false),
                Out::StreamChanged => unsafe {
                    let t = self.t.GetOutputAvailableType(0, 0)?;
                    self.t.SetOutputType(0, &t, 0)?;
                    self.alloc = output_alloc(&self.t, self.cfg.width * self.cfg.height)?;
                    // An asynchronous transform says again when the frame is ready.
                    if self.events.is_some() {
                        return Ok(false);
                    }
                },
            }
        }
        Ok(false)
    }

    /// Asynchronous transforms: handle queued events. With `want_input`, wait until the
    /// transform asks for a picture; with `until_drained`, wait for the end of a drain.
    fn pump(&mut self, out: &mut Vec<VideoPacket>, want_input: bool, until_drained: bool) -> Result<(), String> {
        let Some(ev) = self.events.clone() else { return Ok(()) };
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if want_input && self.need_input > 0 {
                return Ok(());
            }
            match unsafe { ev.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(e) => {
                    let kind = unsafe { e.GetType() }.map_err(err)?;
                    if kind == METransformNeedInput.0 as u32 {
                        self.need_input += 1;
                    } else if kind == METransformHaveOutput.0 as u32 {
                        self.take(out).map_err(|e| format!("ProcessOutput: {}", err(e)))?;
                    } else if kind == METransformDrainComplete.0 as u32 && until_drained {
                        return Ok(());
                    }
                }
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {
                    if !want_input && !until_drained {
                        return Ok(());
                    }
                    if Instant::now() > deadline {
                        return Err(format!("{} stopped answering", self.name));
                    }
                    std::thread::sleep(Duration::from_micros(500));
                }
                Err(e) => return Err(err(e)),
            }
        }
    }
}

/// BT.709 video range, which is what `encode::rgba_to_nv12` makes.
fn colours(t: &IMFMediaType) {
    unsafe {
        let _ = t.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32);
        let _ = t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32);
        let _ = t.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32);
        let _ = t.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32);
    }
}

/// A sample holding one NV12 texture as a DXGI surface buffer (no copy).
fn texture_sample(tex: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D, time_100ns: i64, dur_100ns: i64) -> R<IMFSample> {
    unsafe {
        let buf = MFCreateDXGISurfaceBuffer(&windows::Win32::Graphics::Direct3D11::ID3D11Texture2D::IID, tex, 0, false)?;
        let len = buf.cast::<IMF2DBuffer>()?.GetContiguousLength()?;
        buf.SetCurrentLength(len)?;
        let s = MFCreateSample()?;
        s.AddBuffer(&buf)?;
        s.SetSampleTime(time_100ns)?;
        s.SetSampleDuration(dur_100ns)?;
        Ok(s)
    }
}

impl MfVideo {
    /// Hand one picture to the transform and take what comes out.
    fn submit(&mut self, sample: IMFSample, force_keyframe: bool) -> Result<Vec<VideoPacket>, String> {
        if force_keyframe {
            set_codec(&self.api, &CODECAPI_AVEncVideoForceKeyFrame, VARIANT::from(1u32));
        }
        let mut out = Vec::new();
        if self.events.is_some() {
            self.pump(&mut out, true, false)?;
            unsafe { self.t.ProcessInput(0, &sample, 0) }.map_err(|e| format!("ProcessInput: {}", err(e)))?;
            self.need_input -= 1;
            self.pump(&mut out, false, false)?;
        } else {
            let mut r = unsafe { self.t.ProcessInput(0, &sample, 0) };
            if matches!(&r, Err(e) if e.code() == MF_E_NOTACCEPTING) {
                while self.take(&mut out).map_err(err)? {}
                r = unsafe { self.t.ProcessInput(0, &sample, 0) };
            }
            r.map_err(err)?;
            while self.take(&mut out).map_err(err)? {}
        }
        Ok(out)
    }
}

impl VideoEncoder for MfVideo {
    fn encode(&mut self, rgba: &[u8], pts_us: i64, force_keyframe: bool) -> Result<Vec<VideoPacket>, String> {
        com_here();
        let sample = nv12_sample(rgba, self.cfg.width, self.cfg.height, pts_us * 10, self.cfg.frame_us() * 10).map_err(err)?;
        self.submit(sample, force_keyframe)
    }

    fn encode_surface(&mut self, surface: &Surface, pts_us: i64, force_keyframe: bool) -> Result<Vec<VideoPacket>, String> {
        com_here();
        if self.manager.is_none() {
            return Err("this encoder was opened for pictures in memory".into());
        }
        let Surface::D3d11(t) = surface;
        let sample = texture_sample(&t.0, pts_us * 10, self.cfg.frame_us() * 10).map_err(err)?;
        self.submit(sample, force_keyframe)
    }

    fn finish(&mut self) -> Result<Vec<VideoPacket>, String> {
        com_here();
        let mut out = Vec::new();
        unsafe {
            let _ = self.t.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            self.t.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0).map_err(err)?;
        }
        if self.events.is_some() {
            self.pump(&mut out, false, true)?;
        } else {
            while self.take(&mut out).map_err(err)? {}
        }
        Ok(out)
    }

    fn name(&self) -> EncoderName {
        EncoderName { name: self.name.clone(), hardware: self.hardware }
    }
}

impl Drop for MfVideo {
    fn drop(&mut self) {
        unsafe {
            let _ = self.t.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            if self.events.is_some()
                && let Ok(s) = self.t.cast::<IMFShutdown>()
            {
                let _ = s.Shutdown();
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Sound: Microsoft's AAC encoder transform
// ---------------------------------------------------------------------------------------------

/// The bytes per second Microsoft's AAC encoder offers (96, 128, 160, 192 kbit/s), nearest first.
pub fn aac_bytes_per_second(bitrate_bps: u32) -> u32 {
    let want = bitrate_bps / 8;
    *[12_000u32, 16_000, 20_000, 24_000].iter().min_by_key(|&&b| (b as i64 - want as i64).abs()).unwrap_or(&16_000)
}

struct MfAac {
    t: IMFTransform,
    alloc: Option<u32>,
    asc: Vec<u8>,
    name: String,
    /// For frames that come without a time: the first input time and frames out so far.
    start_us: Option<i64>,
    count: u64,
    _mf: Mf,
}

unsafe impl Send for MfAac {}

pub fn open_audio(bitrate_bps: u32) -> Result<Box<dyn AudioEncoder>, String> {
    let mf = Mf::start()?;
    let inp = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Audio, guidSubtype: MFAudioFormat_PCM };
    let out = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Audio, guidSubtype: MFAudioFormat_AAC };
    let list = enumerate(MFT_CATEGORY_AUDIO_ENCODER, MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER, Some(inp), Some(out), "Microsoft AAC encoder");
    if list.is_empty() {
        return Err("Windows has no AAC encoder here (Windows N editions need the Media Feature Pack)".into());
    }
    let mut tried = Vec::new();
    for c in list {
        let name = c.name.clone();
        match MfAac::open(c, bitrate_bps) {
            Ok(e) => {
                drop(mf);
                return Ok(Box::new(e));
            }
            Err(e) => tried.push(format!("{name}: {e}")),
        }
    }
    Err(tried.join("; "))
}

impl MfAac {
    fn open(c: Candidate, bitrate_bps: u32) -> Result<MfAac, String> {
        let mf = Mf::start()?;
        unsafe {
            let t: IMFTransform = c.activate.ActivateObject().map_err(err)?;
            let pcm = MFCreateMediaType().map_err(err)?;
            pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(err)?;
            pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).map_err(err)?;
            pcm.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).map_err(err)?;
            pcm.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE).map_err(err)?;
            pcm.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, AUDIO_CHANNELS).map_err(err)?;
            pcm.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, AUDIO_CHANNELS * 2).map_err(err)?;
            pcm.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AUDIO_RATE * AUDIO_CHANNELS * 2).map_err(err)?;
            let aac = MFCreateMediaType().map_err(err)?;
            aac.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(err)?;
            aac.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC).map_err(err)?;
            aac.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).map_err(err)?;
            aac.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE).map_err(err)?;
            aac.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, AUDIO_CHANNELS).map_err(err)?;
            aac.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, aac_bytes_per_second(bitrate_bps)).map_err(err)?;
            // Raw AAC frames, no ADTS headers; AAC-LC level 2.
            aac.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0).map_err(err)?;
            aac.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29).map_err(err)?;
            // The AAC encoder wants its input type first; try the other order too.
            let r = t.SetInputType(0, &pcm, 0).and_then(|_| t.SetOutputType(0, &aac, 0));
            if r.is_err() {
                t.SetOutputType(0, &aac, 0).map_err(err)?;
                t.SetInputType(0, &pcm, 0).map_err(err)?;
            }
            // The output type's user data is HEAACWAVEINFO's last 12 bytes, then the
            // AudioSpecificConfig.
            let mut asc = aac_lc_config(AUDIO_RATE, AUDIO_CHANNELS);
            if let Ok(cur) = t.GetOutputCurrentType(0)
                && let Ok(n) = cur.GetBlobSize(&MF_MT_USER_DATA)
                && n > 12
            {
                let mut blob = vec![0u8; n as usize];
                if cur.GetBlob(&MF_MT_USER_DATA, &mut blob, None).is_ok() {
                    asc = blob[12..].to_vec();
                }
            }
            start_streaming(&t).map_err(err)?;
            let alloc = output_alloc(&t, 8192).map_err(err)?;
            Ok(MfAac { t, alloc, asc, name: c.name, start_us: None, count: 0, _mf: mf })
        }
    }

    fn drain(&mut self, out: &mut Vec<AudioPacket>) -> Result<(), String> {
        loop {
            match process_output(&self.t, self.alloc).map_err(err)? {
                Out::Sample(s) => {
                    let data = sample_bytes(&s).map_err(err)?;
                    let pts = unsafe { s.GetSampleTime() }.map(|t| t / 10).unwrap_or_else(|_| aac_packet_pts(self.start_us.unwrap_or(0), self.count));
                    self.count += 1;
                    if !data.is_empty() {
                        out.push(AudioPacket { data, pts_us: pts });
                    }
                }
                Out::NeedMore => return Ok(()),
                Out::StreamChanged => return Err("the AAC encoder changed its format".into()),
            }
        }
    }
}

impl AudioEncoder for MfAac {
    fn encode(&mut self, pcm: &[i16], pts_us: i64) -> Result<Vec<AudioPacket>, String> {
        com_here();
        self.start_us.get_or_insert(pts_us);
        let mut out = Vec::new();
        if pcm.is_empty() {
            return Ok(out);
        }
        let bytes = unsafe { std::slice::from_raw_parts(pcm.as_ptr() as *const u8, pcm.len() * 2) };
        let frames = (pcm.len() / AUDIO_CHANNELS as usize) as i64;
        let s = memory_sample(bytes, pts_us * 10, frames * 10_000_000 / AUDIO_RATE as i64).map_err(err)?;
        let mut r = unsafe { self.t.ProcessInput(0, &s, 0) };
        if matches!(&r, Err(e) if e.code() == MF_E_NOTACCEPTING) {
            self.drain(&mut out)?;
            r = unsafe { self.t.ProcessInput(0, &s, 0) };
        }
        r.map_err(err)?;
        self.drain(&mut out)?;
        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<AudioPacket>, String> {
        com_here();
        let mut out = Vec::new();
        unsafe {
            let _ = self.t.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            self.t.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0).map_err(err)?;
        }
        self.drain(&mut out)?;
        Ok(out)
    }

    fn audio_specific_config(&self) -> Vec<u8> {
        self.asc.clone()
    }

    fn name(&self) -> EncoderName {
        EncoderName { name: self.name.clone(), hardware: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aac_rates_round_to_what_the_encoder_offers() {
        assert_eq!(aac_bytes_per_second(160_000), 20_000);
        assert_eq!(aac_bytes_per_second(128_000), 16_000);
        assert_eq!(aac_bytes_per_second(1), 12_000);
        assert_eq!(aac_bytes_per_second(320_000), 24_000);
        assert_eq!(aac_bytes_per_second(110_000), 12_000);
    }

    #[test]
    fn the_software_h264_transform_encodes_with_keyframes_on_time() {
        let c = VideoConfig { width: 320, height: 180, fps: 30, bitrate_bps: 1_000_000, keyframe_every_s: 1, codec: Codec::H264, prefer_hardware: false, low_latency: true };
        let e = match open_backend(Backend::MediaFoundationSoftware, &c) {
            Ok(e) => e,
            Err(err) => {
                println!("Media Foundation software encoder skipped: {err}");
                return;
            }
        };
        let mut e = Paced::new(e, &c);
        let mut img = vec![0u8; c.rgba_len()];
        let mut out = Vec::new();
        for i in 0..70 {
            test_picture(&mut img, 320, 180, i);
            out.extend(e.encode(&img, i as i64 * c.frame_us(), false).unwrap());
        }
        out.extend(e.finish().unwrap());
        assert!(out.len() >= 68, "{} packets", out.len());
        assert!(out[0].keyframe && sps_pps(&out[0].data).is_some());
        let (sps, _) = sps_pps(&out[0].data).unwrap();
        assert_eq!(h264::parse_sps(&sps).map(|s| (s.width, s.height)), Some((320, 180)));
        let keys: Vec<i64> = out.iter().filter(|p| p.keyframe).map(|p| p.pts_us).collect();
        assert!(keys.len() >= 3, "keyframes at {keys:?}");
        assert!(out.windows(2).all(|w| w[1].dts_us >= w[0].dts_us));
    }

    #[test]
    fn aac_comes_out_in_1024_sample_frames_with_its_config() {
        let mut e = match open_audio(128_000) {
            Ok(e) => e,
            Err(err) => {
                println!("AAC encoder skipped: {err}");
                return;
            }
        };
        assert_eq!(e.audio_specific_config(), vec![0x11, 0x90]);
        // One second of a 440 Hz tone in pieces of odd sizes.
        let pcm: Vec<i16> = (0..48_000).flat_map(|i| {
            let s = ((i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 8000.0) as i16;
            [s, s]
        }).collect();
        let mut out = Vec::new();
        let mut at = 0usize;
        for piece in pcm.chunks(2 * 777) {
            out.extend(e.encode(piece, (at / 2) as i64 * 1_000_000 / 48_000).unwrap());
            at += piece.len();
        }
        out.extend(e.finish().unwrap());
        assert!((45..=49).contains(&out.len()), "48000 / 1024 ≈ 47 packets, got {}", out.len());
        assert!(out.iter().all(|p| !p.data.is_empty() && p.data.len() < 2048));
        assert!(out.windows(2).all(|w| w[1].pts_us > w[0].pts_us));
        assert!(!(0xff == out[0].data[0] && out[0].data[1] & 0xf0 == 0xf0), "no ADTS header");
    }
}
