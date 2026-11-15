//! Linux: H.264 and HEVC on NVIDIA graphics chips through NVENC.
//!
//! VA-API (`linux.rs`) can't encode on NVIDIA (its VA-API driver only decodes), so NVIDIA cards get
//! their own path: NVIDIA's encoder library and CUDA come with the NVIDIA driver and are loaded
//! while running (`libnvidia-encode.so.1`, `libcuda.so.1`), never linked, so Snagster starts
//! anywhere and simply skips this where there is no NVIDIA driver. The types come from NVIDIA's
//! MIT-licensed `nvEncodeAPI.h`, API 8.1, which every driver since 2018 accepts (`nvenc_sys.rs`);
//! the few CUDA calls are declared here from NVIDIA's documented driver API.
//!
//! Each picture: RGBA → NV12 straight into the encoder's input buffer, then one IDR or P frame
//! (no B-frames: live streaming needs every frame out at once), read back at once (NVENC on Linux
//! is synchronous). Keyframes carry SPS/PPS (and VPS for HEVC).

use super::nvenc_sys as nv;
use super::*;
use std::ffi::{c_char, c_int, c_uint, c_void};
use std::sync::{Arc, OnceLock};

// ---- the API version and the structures' versions (the header's macros, worked out) ----

/// `NVENCAPI_VERSION`: major | minor << 24.
pub(crate) const API_VERSION: u32 = nv::NVENCAPI_MAJOR_VERSION | (nv::NVENCAPI_MINOR_VERSION << 24);

/// `NVENCAPI_STRUCT_VERSION(ver)`, and with bit 31 for the structures that set it in the header.
pub(crate) const fn struct_version(ver: u32) -> u32 {
    API_VERSION | (ver << 16) | (0x7 << 28)
}
const fn struct_version_hi(ver: u32) -> u32 {
    struct_version(ver) | (1 << 31)
}

const FUNCTION_LIST_VER: u32 = struct_version(2);
const OPEN_SESSION_VER: u32 = struct_version(1);
const PRESET_CONFIG_VER: u32 = struct_version_hi(4);
const CONFIG_VER: u32 = struct_version_hi(7);
const RC_PARAMS_VER: u32 = struct_version(1);
const INITIALIZE_VER: u32 = struct_version_hi(5);
const CREATE_INPUT_VER: u32 = struct_version(1);
const CREATE_BITSTREAM_VER: u32 = struct_version(1);
const LOCK_INPUT_VER: u32 = struct_version(1);
const PIC_PARAMS_VER: u32 = struct_version_hi(4);
const LOCK_BITSTREAM_VER: u32 = struct_version(1);

/// The version `NvEncodeAPIGetMaxSupportedVersion` reports: major << 4 | minor.
const fn supported(major: u32, minor: u32) -> u32 {
    (major << 4) | minor
}

// ---- GUIDs from the header ----

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> nv::GUID {
    nv::GUID { Data1: d1, Data2: d2, Data3: d3, Data4: d4 }
}
const CODEC_H264: nv::GUID = guid(0x6bc82762, 0x4e63, 0x4ca4, [0xaa, 0x85, 0x1e, 0x50, 0xf3, 0x21, 0xf6, 0xbf]);
const CODEC_HEVC: nv::GUID = guid(0x790cdc88, 0x4522, 0x4d7b, [0x94, 0x25, 0xbd, 0xa9, 0x97, 0x5f, 0x76, 0x03]);
const PRESET_LOW_LATENCY_HQ: nv::GUID = guid(0xc5f733b9, 0xea97, 0x4cf9, [0xbe, 0xc2, 0xbf, 0x78, 0xa7, 0x4f, 0xd1, 0x05]);
const H264_PROFILE_HIGH: nv::GUID = guid(0xe7cbc309, 0x4f7a, 0x4b89, [0xaf, 0x2a, 0xd5, 0x37, 0xc9, 0x2b, 0xe3, 0x10]);
const HEVC_PROFILE_MAIN: nv::GUID = guid(0xb514c39a, 0xb55b, 0x40fa, [0x87, 0x8f, 0xf1, 0x25, 0x3b, 0x4d, 0xfd, 0xec]);

const SUCCESS: nv::NVENCSTATUS = nv::_NVENCSTATUS_NV_ENC_SUCCESS;
const NEED_MORE_INPUT: nv::NVENCSTATUS = nv::_NVENCSTATUS_NV_ENC_ERR_NEED_MORE_INPUT;

/// NVENC's status codes in words (`NVENCSTATUS`, 0..=25 in API 8.1).
pub(crate) fn status_text(s: nv::NVENCSTATUS) -> String {
    const NAMES: [&str; 26] = [
        "success",
        "no encode device",
        "unsupported device",
        "invalid encoder device",
        "invalid device",
        "device does not exist",
        "invalid pointer",
        "invalid event",
        "invalid parameter",
        "invalid call",
        "out of memory",
        "encoder not initialized",
        "unsupported parameter",
        "lock busy",
        "not enough buffer",
        "invalid version (the NVIDIA driver is too old)",
        "map failed",
        "need more input",
        "encoder busy",
        "event not registered",
        "generic error",
        "incompatible client key",
        "unimplemented",
        "resource register failed",
        "resource not registered",
        "resource not mapped",
    ];
    match NAMES.get(s as usize) {
        Some(n) => format!("NVENC: {n} ({s})"),
        None => format!("NVENC error {s}"),
    }
}

fn check(s: nv::NVENCSTATUS, what: &str) -> Result<(), String> {
    if s == SUCCESS { Ok(()) } else { Err(format!("{what}: {}", status_text(s))) }
}

// ---- CUDA: just enough to give NVENC a device ----

type CuResult = c_int;
type CuDevice = c_int;
type CuContext = *mut c_void;

struct Cuda {
    init: unsafe extern "C" fn(c_uint) -> CuResult,
    device_get_count: unsafe extern "C" fn(*mut c_int) -> CuResult,
    device_get: unsafe extern "C" fn(*mut CuDevice, c_int) -> CuResult,
    device_get_name: unsafe extern "C" fn(*mut c_char, c_int, CuDevice) -> CuResult,
    ctx_create: unsafe extern "C" fn(*mut CuContext, c_uint, CuDevice) -> CuResult,
    ctx_destroy: unsafe extern "C" fn(CuContext) -> CuResult,
    ctx_push: unsafe extern "C" fn(CuContext) -> CuResult,
    ctx_pop: unsafe extern "C" fn(*mut CuContext) -> CuResult,
}

/// The NVIDIA driver's libraries, loaded once.
struct Libs {
    api: nv::NV_ENCODE_API_FUNCTION_LIST,
    cuda: Cuda,
    _encode: libloading::Library,
    _cuda: libloading::Library,
}

// The function table is plain function pointers into the driver.
unsafe impl Send for Libs {}
unsafe impl Sync for Libs {}

fn sym<T: Copy>(lib: &libloading::Library, name: &[u8]) -> Result<T, String> {
    unsafe { lib.get::<T>(name).map(|s| *s).map_err(|e| format!("{}: {e}", String::from_utf8_lossy(&name[..name.len() - 1]))) }
}

fn libs() -> Result<Arc<Libs>, String> {
    static LIBS: OnceLock<Result<Arc<Libs>, String>> = OnceLock::new();
    LIBS.get_or_init(|| load().map(Arc::new)).clone()
}

fn load() -> Result<Libs, String> {
    let no_driver = |e: libloading::Error| format!("no NVIDIA driver with NVENC here ({e})");
    let enc = unsafe { libloading::Library::new("libnvidia-encode.so.1") }.map_err(no_driver)?;
    let cu = unsafe { libloading::Library::new("libcuda.so.1") }.map_err(no_driver)?;
    let max: unsafe extern "C" fn(*mut u32) -> nv::NVENCSTATUS = sym(&enc, b"NvEncodeAPIGetMaxSupportedVersion\0")?;
    let create: unsafe extern "C" fn(*mut nv::NV_ENCODE_API_FUNCTION_LIST) -> nv::NVENCSTATUS = sym(&enc, b"NvEncodeAPICreateInstance\0")?;
    let mut have = 0u32;
    check(unsafe { max(&mut have) }, "NvEncodeAPIGetMaxSupportedVersion")?;
    if have < supported(nv::NVENCAPI_MAJOR_VERSION, nv::NVENCAPI_MINOR_VERSION) {
        return Err(format!("the NVIDIA driver supports NVENC {}.{}; Snagster needs 8.1 or newer", have >> 4, have & 0xf));
    }
    let mut api: nv::NV_ENCODE_API_FUNCTION_LIST = unsafe { std::mem::zeroed() };
    api.version = FUNCTION_LIST_VER;
    check(unsafe { create(&mut api) }, "NvEncodeAPICreateInstance")?;
    let cuda = Cuda {
        init: sym(&cu, b"cuInit\0")?,
        device_get_count: sym(&cu, b"cuDeviceGetCount\0")?,
        device_get: sym(&cu, b"cuDeviceGet\0")?,
        device_get_name: sym(&cu, b"cuDeviceGetName\0")?,
        ctx_create: sym(&cu, b"cuCtxCreate_v2\0")?,
        ctx_destroy: sym(&cu, b"cuCtxDestroy_v2\0")?,
        ctx_push: sym(&cu, b"cuCtxPushCurrent_v2\0")?,
        ctx_pop: sym(&cu, b"cuCtxPopCurrent_v2\0")?,
    };
    let r = unsafe { (cuda.init)(0) };
    if r != 0 {
        return Err(format!("CUDA couldn't start (error {r})"));
    }
    Ok(Libs { api, cuda, _encode: enc, _cuda: cu })
}

/// A function from NVENC's table, or an error naming it (an old driver may lack one).
macro_rules! f {
    ($libs:expr, $name:ident) => {
        $libs.api.$name.ok_or(concat!("the NVIDIA driver has no ", stringify!($name)))?
    };
}

// ---- the settings, kept apart from the driver so the tests can check them ----

/// NVENC's settings for `cfg`, starting from the low-latency preset's: the profile, one
/// keyframe every `gop_frames`, no B-frames, constant bitrate with a buffer of about one frame
/// (low delay), and SPS/PPS repeated on every keyframe.
pub(crate) fn tune(mut c: nv::NV_ENC_CONFIG, cfg: &VideoConfig) -> nv::NV_ENC_CONFIG {
    let gop = cfg.gop_frames().max(1);
    c.version = CONFIG_VER;
    c.gopLength = gop;
    c.frameIntervalP = 1;
    c.rcParams.version = RC_PARAMS_VER;
    c.rcParams.rateControlMode = if cfg.low_latency { nv::_NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_CBR_LOWDELAY_HQ } else { nv::_NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_CBR };
    c.rcParams.averageBitRate = cfg.bitrate_bps;
    c.rcParams.maxBitRate = cfg.bitrate_bps;
    let frame_bits = cfg.bitrate_bps / cfg.fps.max(1);
    c.rcParams.vbvBufferSize = if cfg.low_latency { frame_bits } else { cfg.bitrate_bps };
    c.rcParams.vbvInitialDelay = c.rcParams.vbvBufferSize;
    unsafe {
        match cfg.codec {
            Codec::H264 => {
                c.profileGUID = H264_PROFILE_HIGH;
                let h = &mut c.encodeCodecConfig.h264Config;
                h.idrPeriod = gop;
                h.set_repeatSPSPPS(1);
            }
            Codec::Hevc => {
                c.profileGUID = HEVC_PROFILE_MAIN;
                let h = &mut c.encodeCodecConfig.hevcConfig;
                h.idrPeriod = gop;
                h.set_repeatSPSPPS(1);
            }
        }
    }
    c
}

/// Write `rgba` as NV12 into a buffer whose rows are `pitch` bytes apart, the chroma plane
/// starting `pitch * h` bytes in (how NVENC lays out a locked NV12 input buffer).
pub(crate) fn fill_nv12(rgba: &[u8], w: u32, h: u32, pitch: usize, buf: &mut [u8]) {
    let (y, uv) = buf.split_at_mut(pitch * h as usize);
    rgba_to_nv12(rgba, w, h, y, pitch, uv, pitch);
}

// ---- the encoder ----

pub struct Nvenc {
    libs: Arc<Libs>,
    ctx: CuContext,
    enc: *mut c_void,
    input: nv::NV_ENC_INPUT_PTR,
    output: nv::NV_ENC_OUTPUT_PTR,
    cfg: VideoConfig,
    frame: u32,
    name: String,
}

// The session is used by one thread at a time (the encoder moves to the stream's thread);
// every call pushes the CUDA context onto that thread first.
unsafe impl Send for Nvenc {}

/// The first NVIDIA graphics chip's name, and a CUDA context on it.
fn cuda_context(l: &Libs) -> Result<(CuContext, String), String> {
    unsafe {
        let mut n = 0;
        if (l.cuda.device_get_count)(&mut n) != 0 || n < 1 {
            return Err("no NVIDIA graphics chip found".into());
        }
        let mut dev = 0;
        if (l.cuda.device_get)(&mut dev, 0) != 0 {
            return Err("CUDA couldn't open the graphics chip".into());
        }
        let mut name = [0 as c_char; 128];
        let gpu = if (l.cuda.device_get_name)(name.as_mut_ptr(), name.len() as c_int, dev) == 0 {
            std::ffi::CStr::from_ptr(name.as_ptr()).to_string_lossy().into_owned()
        } else {
            "NVIDIA".to_string()
        };
        let mut ctx: CuContext = std::ptr::null_mut();
        let r = (l.cuda.ctx_create)(&mut ctx, 0, dev);
        if r != 0 || ctx.is_null() {
            return Err(format!("CUDA couldn't make a context (error {r})"));
        }
        // Not left current on this thread: each NVENC call pushes it where it runs.
        let mut popped: CuContext = std::ptr::null_mut();
        (l.cuda.ctx_pop)(&mut popped);
        Ok((ctx, gpu))
    }
}

/// Run `f` with the CUDA context `ctx` current on this thread (and not after).
fn in_context<R>(libs: &Libs, ctx: CuContext, f: impl FnOnce() -> R) -> R {
    unsafe {
        (libs.cuda.ctx_push)(ctx);
        let r = f();
        let mut popped: CuContext = std::ptr::null_mut();
        (libs.cuda.ctx_pop)(&mut popped);
        r
    }
}

/// The encoder's name: what the person sees and picks in Settings.
pub(crate) fn encoder_name(gpu: &str) -> String {
    format!("NVIDIA NVENC ({gpu})")
}

impl Nvenc {
    pub fn open(cfg: &VideoConfig) -> Result<Nvenc, String> {
        let libs = libs()?;
        let (ctx, gpu) = cuda_context(&libs)?;
        let mut e = Nvenc { libs, ctx, enc: std::ptr::null_mut(), input: std::ptr::null_mut(), output: std::ptr::null_mut(), cfg: cfg.clone(), frame: 0, name: encoder_name(&gpu) };
        // On failure `e` is dropped, which frees whatever was made.
        e.start()?;
        Ok(e)
    }


    fn start(&mut self) -> Result<(), String> {
        let (l, ctx) = (self.libs.clone(), self.ctx);
        let cfg = self.cfg.clone();
        let codec = if cfg.codec == Codec::Hevc { CODEC_HEVC } else { CODEC_H264 };
        in_context(&l, ctx, || unsafe {
            let mut open: nv::NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS = std::mem::zeroed();
            open.version = OPEN_SESSION_VER;
            open.deviceType = nv::_NV_ENC_DEVICE_TYPE_NV_ENC_DEVICE_TYPE_CUDA;
            open.device = self.ctx;
            open.apiVersion = API_VERSION;
            check(f!(l, nvEncOpenEncodeSessionEx)(&mut open, &mut self.enc), "opening an NVENC session")?;

            let mut preset: nv::NV_ENC_PRESET_CONFIG = std::mem::zeroed();
            preset.version = PRESET_CONFIG_VER;
            preset.presetCfg.version = CONFIG_VER;
            check(f!(l, nvEncGetEncodePresetConfig)(self.enc, codec, PRESET_LOW_LATENCY_HQ, &mut preset), "reading the low-latency preset")?;
            let mut config = tune(preset.presetCfg, &cfg);

            let mut init: nv::NV_ENC_INITIALIZE_PARAMS = std::mem::zeroed();
            init.version = INITIALIZE_VER;
            init.encodeGUID = codec;
            init.presetGUID = PRESET_LOW_LATENCY_HQ;
            init.encodeWidth = cfg.width;
            init.encodeHeight = cfg.height;
            init.darWidth = cfg.width;
            init.darHeight = cfg.height;
            init.maxEncodeWidth = cfg.width;
            init.maxEncodeHeight = cfg.height;
            init.frameRateNum = cfg.fps.max(1);
            init.frameRateDen = 1;
            init.enablePTD = 1;
            init.encodeConfig = &mut config;
            check(f!(l, nvEncInitializeEncoder)(self.enc, &mut init), &format!("starting {} {}x{} at {} fps", cfg.codec.label(), cfg.width, cfg.height, cfg.fps))?;

            let mut input: nv::NV_ENC_CREATE_INPUT_BUFFER = std::mem::zeroed();
            input.version = CREATE_INPUT_VER;
            input.width = cfg.width;
            input.height = cfg.height;
            input.bufferFmt = nv::_NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_NV12;
            check(f!(l, nvEncCreateInputBuffer)(self.enc, &mut input), "making the input buffer")?;
            self.input = input.inputBuffer;

            let mut out: nv::NV_ENC_CREATE_BITSTREAM_BUFFER = std::mem::zeroed();
            out.version = CREATE_BITSTREAM_VER;
            check(f!(l, nvEncCreateBitstreamBuffer)(self.enc, &mut out), "making the output buffer")?;
            self.output = out.bitstreamBuffer;
            Ok(())
        })
    }

    fn upload(&mut self, rgba: &[u8]) -> Result<(), String> {
        let (l, ctx) = (self.libs.clone(), self.ctx);
        let (w, h, input) = (self.cfg.width, self.cfg.height, self.input);
        in_context(&l, ctx, || unsafe {
            let mut lock: nv::NV_ENC_LOCK_INPUT_BUFFER = std::mem::zeroed();
            lock.version = LOCK_INPUT_VER;
            lock.inputBuffer = input;
            check(f!(l, nvEncLockInputBuffer)(self.enc, &mut lock), "locking the input buffer")?;
            let pitch = lock.pitch as usize;
            if pitch < w as usize || lock.bufferDataPtr.is_null() {
                f!(l, nvEncUnlockInputBuffer)(self.enc, input);
                return Err(format!("NVENC gave an input buffer with rows of {pitch} bytes for {w} pixels"));
            }
            let buf = std::slice::from_raw_parts_mut(lock.bufferDataPtr as *mut u8, pitch * h as usize * 3 / 2);
            fill_nv12(rgba, w, h, pitch, buf);
            check(f!(l, nvEncUnlockInputBuffer)(self.enc, input), "unlocking the input buffer")
        })
    }

    /// Encode what is in the input buffer (or, `flags` EOS, flush) and read the frame back.
    fn encode_buffer(&mut self, pts_us: i64, flags: u32) -> Result<Option<(Vec<u8>, bool)>, String> {
        let (l, ctx) = (self.libs.clone(), self.ctx);
        let eos = flags & nv::_NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_EOS != 0;
        let (w, h) = (self.cfg.width, self.cfg.height);
        let frame = self.frame;
        let (input, output) = (self.input, self.output);
        let r = in_context(&l, ctx, || unsafe {
            let mut pic: nv::NV_ENC_PIC_PARAMS = std::mem::zeroed();
            pic.version = PIC_PARAMS_VER;
            pic.encodePicFlags = flags;
            if !eos {
                pic.inputWidth = w;
                pic.inputHeight = h;
                pic.inputBuffer = input;
                pic.outputBitstream = output;
                pic.bufferFmt = nv::_NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_NV12;
                pic.pictureStruct = nv::_NV_ENC_PIC_STRUCT_NV_ENC_PIC_STRUCT_FRAME;
                pic.frameIdx = frame;
                pic.inputTimeStamp = pts_us.max(0) as u64;
            }
            let s = f!(l, nvEncEncodePicture)(self.enc, &mut pic);
            if eos || s == NEED_MORE_INPUT {
                return if eos || s == SUCCESS { Ok(None) } else { check(s, "flushing").map(|_| None) };
            }
            check(s, "encoding a picture")?;
            let mut lock: nv::NV_ENC_LOCK_BITSTREAM = std::mem::zeroed();
            lock.version = LOCK_BITSTREAM_VER;
            lock.outputBitstream = output;
            check(f!(l, nvEncLockBitstream)(self.enc, &mut lock), "reading the encoded picture")?;
            let data = std::slice::from_raw_parts(lock.bitstreamBufferPtr as *const u8, lock.bitstreamSizeInBytes as usize).to_vec();
            let key = lock.pictureType == nv::_NV_ENC_PIC_TYPE_NV_ENC_PIC_TYPE_IDR;
            check(f!(l, nvEncUnlockBitstream)(self.enc, output), "releasing the encoded picture")?;
            Ok(Some((data, key)))
        });
        if !eos {
            self.frame = self.frame.wrapping_add(1);
        }
        r
    }
}

impl VideoEncoder for Nvenc {
    fn encode(&mut self, rgba: &[u8], pts_us: i64, force_keyframe: bool) -> Result<Vec<VideoPacket>, String> {
        if rgba.len() < self.cfg.rgba_len() {
            return Err(format!("a picture of {} bytes for {}x{}", rgba.len(), self.cfg.width, self.cfg.height));
        }
        self.upload(rgba)?;
        let first = self.frame == 0;
        let flags = if force_keyframe || first { nv::_NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_FORCEIDR | nv::_NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_OUTPUT_SPSPPS } else { 0 };
        Ok(match self.encode_buffer(pts_us, flags)? {
            Some((data, key)) if !data.is_empty() => {
                let keyframe = key || is_keyframe(self.cfg.codec, &data);
                vec![VideoPacket { data, pts_us, dts_us: pts_us, keyframe }]
            }
            _ => Vec::new(),
        })
    }

    fn finish(&mut self) -> Result<Vec<VideoPacket>, String> {
        // Every frame was read back at once; tell the encoder the stream ends.
        self.encode_buffer(0, nv::_NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_EOS)?;
        Ok(Vec::new())
    }

    fn name(&self) -> EncoderName {
        EncoderName { name: self.name.clone(), hardware: true }
    }
}

impl Drop for Nvenc {
    fn drop(&mut self) {
        let (l, ctx) = (self.libs.clone(), self.ctx);
        let (enc, input, output) = (self.enc, self.input, self.output);
        in_context(&l, ctx, || unsafe {
            if !enc.is_null() {
                if !input.is_null()
                    && let Some(f) = l.api.nvEncDestroyInputBuffer
                {
                    f(enc, input);
                }
                if !output.is_null()
                    && let Some(f) = l.api.nvEncDestroyBitstreamBuffer
                {
                    f(enc, output);
                }
                if let Some(f) = l.api.nvEncDestroyEncoder {
                    f(enc);
                }
            }
        });
        if !self.ctx.is_null() {
            unsafe {
                (self.libs.cuda.ctx_destroy)(self.ctx);
            }
        }
    }
}

pub fn open_video(cfg: &VideoConfig) -> Result<Box<dyn VideoEncoder>, String> {
    Nvenc::open(cfg).map(|e| Box::new(e) as Box<dyn VideoEncoder>)
}

/// The NVENC encoders here (H.264 and HEVC where the chip makes them), found once by opening a
/// small one of each.
pub fn list() -> Vec<(Codec, EncoderName)> {
    static FOUND: OnceLock<Vec<(Codec, EncoderName)>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            [Codec::H264, Codec::Hevc]
                .into_iter()
                .filter_map(|codec| {
                    let c = VideoConfig { width: 256, height: 144, fps: 30, bitrate_bps: 500_000, keyframe_every_s: 2, codec, prefer_hardware: true, low_latency: true };
                    Nvenc::open(&c).ok().map(|e| (codec, e.name()))
                })
                .collect()
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(codec: Codec) -> VideoConfig {
        VideoConfig { width: 1920, height: 1080, fps: 60, bitrate_bps: 6_000_000, keyframe_every_s: 2, codec, prefer_hardware: true, low_latency: true }
    }

    /// The header's version macros worked out: NVENCAPI_VERSION is 8 | 1 << 24, and a structure's
    /// version adds its own number at bit 16, 7 at bit 28 and (for some) bit 31.
    #[test]
    fn versions_are_the_headers() {
        assert_eq!(API_VERSION, 0x0100_0008);
        assert_eq!(API_VERSION, nv::NVENCAPI_VERSION, "the generated constant agrees");
        assert_eq!(FUNCTION_LIST_VER, 0x7002_0000 | 0x0100_0008);
        assert_eq!(PIC_PARAMS_VER, 0xF004_0000 | 0x0100_0008);
        assert_eq!(CONFIG_VER, 0xF007_0000 | 0x0100_0008);
        assert_eq!(INITIALIZE_VER, 0xF005_0000 | 0x0100_0008);
        assert_eq!(supported(8, 1), 0x81);
    }

    #[test]
    fn the_settings_are_low_latency_constant_bitrate_without_b_frames() {
        let preset: nv::NV_ENC_CONFIG = unsafe { std::mem::zeroed() };
        let c = tune(preset, &cfg(Codec::H264));
        assert_eq!((c.version, c.gopLength, c.frameIntervalP), (CONFIG_VER, 120, 1));
        assert_eq!(c.rcParams.rateControlMode, nv::_NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_CBR_LOWDELAY_HQ);
        assert_eq!((c.rcParams.averageBitRate, c.rcParams.maxBitRate, c.rcParams.vbvBufferSize), (6_000_000, 6_000_000, 100_000), "about one frame of buffer");
        assert_eq!(c.profileGUID.Data1, H264_PROFILE_HIGH.Data1);
        unsafe {
            assert_eq!(c.encodeCodecConfig.h264Config.idrPeriod, 120);
            assert_eq!(c.encodeCodecConfig.h264Config.repeatSPSPPS(), 1, "SPS/PPS on every keyframe");
        }
        let c = tune(preset, &VideoConfig { low_latency: false, ..cfg(Codec::Hevc) });
        assert_eq!(c.rcParams.rateControlMode, nv::_NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_CBR);
        assert_eq!(c.rcParams.vbvBufferSize, 6_000_000);
        assert_eq!(c.profileGUID.Data1, HEVC_PROFILE_MAIN.Data1);
        unsafe {
            assert_eq!(c.encodeCodecConfig.hevcConfig.repeatSPSPPS(), 1);
        }
    }

    /// NVENC's input rows may be longer than the picture (pitch): the planes land where NVENC
    /// reads them, and match the plain conversion.
    #[test]
    fn nv12_lands_at_the_buffers_pitch() {
        let (w, h, pitch) = (6u32, 4u32, 8usize);
        let rgba: Vec<u8> = (0..w * h * 4).map(|i| (i * 7 % 251) as u8).collect();
        let mut buf = vec![0xEEu8; pitch * h as usize * 3 / 2];
        fill_nv12(&rgba, w, h, pitch, &mut buf);
        let mut plain = Vec::new();
        nv12(&rgba, w, h, &mut plain);
        for row in 0..h as usize {
            assert_eq!(&buf[row * pitch..row * pitch + w as usize], &plain[row * w as usize..(row + 1) * w as usize], "Y row {row}");
            assert_eq!(&buf[row * pitch + w as usize..(row + 1) * pitch], &[0xEE; 2], "the padding is left alone");
        }
        let (yb, yp) = (pitch * h as usize, (w * h) as usize);
        for row in 0..h as usize / 2 {
            assert_eq!(&buf[yb + row * pitch..yb + row * pitch + w as usize], &plain[yp + row * w as usize..yp + (row + 1) * w as usize], "UV row {row}");
        }
    }

    #[test]
    fn status_codes_read_as_words() {
        assert_eq!(status_text(0), "NVENC: success (0)");
        assert_eq!(status_text(15), "NVENC: invalid version (the NVIDIA driver is too old) (15)");
        assert_eq!(status_text(99), "NVENC error 99");
        assert!(check(2, "opening").unwrap_err().starts_with("opening: NVENC: unsupported device"));
        assert_eq!(encoder_name("NVIDIA GeForce RTX 3060"), "NVIDIA NVENC (NVIDIA GeForce RTX 3060)");
        assert_eq!(vendor_in(&encoder_name("Tesla T4")), "NVIDIA");
    }

    /// Without an NVIDIA driver (the CI machines) opening fails with a reason, never a crash,
    /// and nothing is listed.
    #[test]
    fn without_an_nvidia_driver_it_says_so() {
        match Nvenc::open(&VideoConfig { width: 64, height: 64, ..cfg(Codec::H264) }) {
            Ok(e) => assert!(e.name().name.starts_with("NVIDIA NVENC ("), "an NVIDIA machine: {}", e.name().name),
            Err(why) => {
                assert!(!why.is_empty());
                assert!(list().is_empty(), "nothing listed without a working encoder");
            }
        }
    }

    /// Needs an NVIDIA graphics chip and driver: `cargo test -- --ignored nvenc_encodes`.
    #[test]
    #[ignore]
    fn nvenc_encodes_keyframes_with_parameter_sets_and_p_frames() {
        for codec in [Codec::H264, Codec::Hevc] {
            let c = VideoConfig { width: 640, height: 360, ..cfg(codec) };
            let mut e = Nvenc::open(&c).unwrap_or_else(|e| panic!("{}: {e}", codec.label()));
            let mut out = Vec::new();
            let mut rgba = vec![0u8; c.rgba_len()];
            for i in 0..30 {
                test_picture(&mut rgba, 640, 360, i);
                out.extend(e.encode(&rgba, i as i64 * c.frame_us(), i == 20).unwrap());
            }
            out.extend(e.finish().unwrap());
            assert!(out.len() >= 29, "{}: {} frames", codec.label(), out.len());
            assert!(out[0].keyframe && !parameter_sets(codec, &out[0].data).is_empty(), "{}: first frame", codec.label());
            assert!(out[1..20].iter().all(|p| !p.keyframe), "{}: P frames in between", codec.label());
            assert!(out[20].keyframe, "{}: the forced keyframe", codec.label());
            println!("ok  {} via {}", codec.label(), e.name().name);
        }
    }
}
