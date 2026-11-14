//! macOS: VideoToolbox (`VTCompressionSession`) for H.264 and HEVC on the media engine of Apple
//! silicon and Intel Macs, and AudioToolbox (`AudioConverter`) for AAC.
//!
//! VideoToolbox hands frames back on its own thread in AVCC form (length-prefixed NAL units, the
//! parameter sets kept in the format description); they are turned into Annex-B here, with the
//! parameter sets in front of every keyframe. Real-time mode and no frame reordering keep it
//! low-latency (no B-frames). Written from Apple's VideoToolbox, Core Media, Core Video and Audio
//! Toolbox reference documentation; the declarations below are Apple's C interfaces.

use super::*;
use std::ffi::c_void;
use std::sync::Mutex;

type OSStatus = i32;
type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CFMutableDictionaryRef = *mut c_void;
type VTCompressionSessionRef = *mut c_void;
type CVPixelBufferRef = *mut c_void;
type CMSampleBufferRef = *mut c_void;
type AudioConverterRef = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

const CMTIME_VALID: u32 = 1;
const CMTIME_INVALID: CMTime = CMTime { value: 0, timescale: 0, flags: 0, epoch: 0 };

fn micros(us: i64) -> CMTime {
    CMTime { value: us, timescale: 1_000_000, flags: CMTIME_VALID, epoch: 0 }
}

fn to_micros(t: CMTime) -> Option<i64> {
    (t.flags & CMTIME_VALID != 0 && t.timescale > 0).then(|| (t.value as i128 * 1_000_000 / t.timescale as i128) as i64)
}

const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

const CODEC_H264: u32 = fourcc(b"avc1");
const CODEC_HEVC: u32 = fourcc(b"hvc1");
const PIXEL_420V: u32 = fourcc(b"420v");
const CF_NUMBER_SINT32: isize = 3;
const CF_NUMBER_FLOAT64: isize = 6;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
    static kCFBooleanTrue: CFTypeRef;
    static kCFBooleanFalse: CFTypeRef;
    fn CFRelease(cf: CFTypeRef);
    fn CFDictionaryCreateMutable(a: CFAllocatorRef, capacity: isize, keys: *const c_void, values: *const c_void) -> CFMutableDictionaryRef;
    fn CFDictionarySetValue(d: CFMutableDictionaryRef, key: *const c_void, value: *const c_void);
    fn CFDictionaryGetValue(d: *const c_void, key: *const c_void) -> *const c_void;
    fn CFNumberCreate(a: CFAllocatorRef, kind: isize, value: *const c_void) -> CFTypeRef;
    fn CFArrayGetCount(a: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(a: *const c_void, i: isize) -> *const c_void;
    fn CFBooleanGetValue(b: *const c_void) -> u8;
}

type ParamSetFn = unsafe extern "C" fn(*const c_void, usize, *mut *const u8, *mut usize, *mut usize, *mut i32) -> OSStatus;
type OutputCallback = unsafe extern "C" fn(*mut c_void, *mut c_void, OSStatus, u32, CMSampleBufferRef);

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    static kVTCompressionPropertyKey_RealTime: CFStringRef;
    static kVTCompressionPropertyKey_AllowFrameReordering: CFStringRef;
    static kVTCompressionPropertyKey_AverageBitRate: CFStringRef;
    static kVTCompressionPropertyKey_MaxKeyFrameInterval: CFStringRef;
    static kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration: CFStringRef;
    static kVTCompressionPropertyKey_ExpectedFrameRate: CFStringRef;
    static kVTCompressionPropertyKey_ProfileLevel: CFStringRef;
    static kVTCompressionPropertyKey_ColorPrimaries: CFStringRef;
    static kVTCompressionPropertyKey_TransferFunction: CFStringRef;
    static kVTCompressionPropertyKey_YCbCrMatrix: CFStringRef;
    static kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder: CFStringRef;
    static kVTProfileLevel_H264_High_AutoLevel: CFStringRef;
    static kVTProfileLevel_HEVC_Main_AutoLevel: CFStringRef;
    static kVTEncodeFrameOptionKey_ForceKeyFrame: CFStringRef;
    static kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder: CFStringRef;
    static kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: CFStringRef;
    fn VTCompressionSessionCreate(
        a: CFAllocatorRef,
        width: i32,
        height: i32,
        codec: u32,
        spec: *const c_void,
        source_attrs: *const c_void,
        data_allocator: CFAllocatorRef,
        callback: Option<OutputCallback>,
        refcon: *mut c_void,
        out: *mut VTCompressionSessionRef,
    ) -> OSStatus;
    fn VTSessionSetProperty(s: *mut c_void, key: CFStringRef, value: CFTypeRef) -> OSStatus;
    fn VTSessionCopyProperty(s: *mut c_void, key: CFStringRef, a: CFAllocatorRef, out: *mut c_void) -> OSStatus;
    fn VTCompressionSessionPrepareToEncodeFrames(s: VTCompressionSessionRef) -> OSStatus;
    fn VTCompressionSessionEncodeFrame(s: VTCompressionSessionRef, image: CVPixelBufferRef, pts: CMTime, duration: CMTime, props: *const c_void, frame_refcon: *mut c_void, flags_out: *mut u32) -> OSStatus;
    fn VTCompressionSessionCompleteFrames(s: VTCompressionSessionRef, until: CMTime) -> OSStatus;
    fn VTCompressionSessionInvalidate(s: VTCompressionSessionRef);
    fn VTCompressionSessionGetPixelBufferPool(s: VTCompressionSessionRef) -> *mut c_void;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: CFStringRef;
    static kCVPixelBufferWidthKey: CFStringRef;
    static kCVPixelBufferHeightKey: CFStringRef;
    static kCVPixelBufferIOSurfacePropertiesKey: CFStringRef;
    static kCVImageBufferColorPrimaries_ITU_R_709_2: CFStringRef;
    static kCVImageBufferTransferFunction_ITU_R_709_2: CFStringRef;
    static kCVImageBufferYCbCrMatrix_ITU_R_709_2: CFStringRef;
    fn CVPixelBufferPoolCreatePixelBuffer(a: CFAllocatorRef, pool: *mut c_void, out: *mut CVPixelBufferRef) -> i32;
    fn CVPixelBufferCreate(a: CFAllocatorRef, w: usize, h: usize, format: u32, attrs: *const c_void, out: *mut CVPixelBufferRef) -> i32;
    fn CVPixelBufferLockBaseAddress(pb: CVPixelBufferRef, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(pb: CVPixelBufferRef, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddressOfPlane(pb: CVPixelBufferRef, plane: usize) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRowOfPlane(pb: CVPixelBufferRef, plane: usize) -> usize;
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    static kCMSampleAttachmentKey_NotSync: CFStringRef;
    fn CMSampleBufferGetDataBuffer(s: CMSampleBufferRef) -> *mut c_void;
    fn CMBlockBufferGetDataLength(b: *mut c_void) -> usize;
    fn CMBlockBufferCopyDataBytes(b: *mut c_void, offset: usize, len: usize, dst: *mut c_void) -> OSStatus;
    fn CMSampleBufferGetPresentationTimeStamp(s: CMSampleBufferRef) -> CMTime;
    fn CMSampleBufferGetDecodeTimeStamp(s: CMSampleBufferRef) -> CMTime;
    fn CMSampleBufferGetSampleAttachmentsArray(s: CMSampleBufferRef, create: u8) -> *const c_void;
    fn CMSampleBufferGetFormatDescription(s: CMSampleBufferRef) -> *const c_void;
    fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(d: *const c_void, i: usize, p: *mut *const u8, size: *mut usize, count: *mut usize, nal_len: *mut i32) -> OSStatus;
    fn CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(d: *const c_void, i: usize, p: *mut *const u8, size: *mut usize, count: *mut usize, nal_len: *mut i32) -> OSStatus;
}

/// A CoreFoundation dictionary built up and released on drop.
struct Dict(CFMutableDictionaryRef);

impl Dict {
    fn new() -> Dict {
        unsafe { Dict(CFDictionaryCreateMutable(std::ptr::null(), 0, &raw const kCFTypeDictionaryKeyCallBacks as *const c_void, &raw const kCFTypeDictionaryValueCallBacks as *const c_void)) }
    }

    fn set(&self, key: CFStringRef, value: CFTypeRef) {
        unsafe { CFDictionarySetValue(self.0, key, value) }
    }

    fn set_i32(&self, key: CFStringRef, v: i32) {
        let n = Num::i32(v);
        self.set(key, n.0);
    }
}

impl Drop for Dict {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}

struct Num(CFTypeRef);

impl Num {
    fn i32(v: i32) -> Num {
        unsafe { Num(CFNumberCreate(std::ptr::null(), CF_NUMBER_SINT32, &v as *const i32 as *const c_void)) }
    }

    fn f64(v: f64) -> Num {
        unsafe { Num(CFNumberCreate(std::ptr::null(), CF_NUMBER_FLOAT64, &v as *const f64 as *const c_void)) }
    }
}

impl Drop for Num {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}

fn bool_ref(b: bool) -> CFTypeRef {
    unsafe { if b { kCFBooleanTrue } else { kCFBooleanFalse } }
}

/// What the output callback fills, shared with the encoder.
struct Shared {
    codec: Codec,
    out: Mutex<Vec<Result<VideoPacket, String>>>,
    /// Frames VideoToolbox dropped (real-time mode drops what it can't keep up with).
    dropped: std::sync::atomic::AtomicU64,
}

/// `kVTEncodeInfo_FrameDropped` in the output callback's flags.
const FRAME_DROPPED: u32 = 1 << 1;

/// The output callback dropped a frame rather than encoding it.
fn was_dropped(status: OSStatus, flags: u32, has_sample: bool) -> bool {
    status == 0 && (flags & FRAME_DROPPED != 0 || !has_sample)
}

unsafe extern "C" fn on_frame(refcon: *mut c_void, _frame: *mut c_void, status: OSStatus, flags: u32, sample: CMSampleBufferRef) {
    let shared = unsafe { &*(refcon as *const Shared) };
    let r = if status != 0 {
        Err(format!("VideoToolbox couldn't encode a frame (error {status})"))
    } else if was_dropped(status, flags, !sample.is_null()) {
        shared.dropped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return;
    } else {
        unsafe { packet(shared.codec, sample) }
    };
    if let Ok(mut q) = shared.out.lock() {
        q.push(r);
    }
}

/// One encoded frame: AVCC data turned into Annex-B, parameter sets in front of keyframes.
unsafe fn packet(codec: Codec, s: CMSampleBufferRef) -> Result<VideoPacket, String> {
    unsafe {
        let bb = CMSampleBufferGetDataBuffer(s);
        if bb.is_null() {
            return Err("an encoded frame without data".into());
        }
        let len = CMBlockBufferGetDataLength(bb);
        let mut avcc = vec![0u8; len];
        if CMBlockBufferCopyDataBytes(bb, 0, len, avcc.as_mut_ptr() as *mut c_void) != 0 {
            return Err("couldn't copy an encoded frame".into());
        }
        let mut keyframe = true;
        let att = CMSampleBufferGetSampleAttachmentsArray(s, 0);
        if !att.is_null() && CFArrayGetCount(att) > 0 {
            let d = CFArrayGetValueAtIndex(att, 0);
            let not_sync = CFDictionaryGetValue(d, kCMSampleAttachmentKey_NotSync);
            keyframe = not_sync.is_null() || CFBooleanGetValue(not_sync) == 0;
        }
        let desc = CMSampleBufferGetFormatDescription(s);
        let get: ParamSetFn = match codec {
            Codec::H264 => CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
            Codec::Hevc => CMVideoFormatDescriptionGetHEVCParameterSetAtIndex,
        };
        let mut data = Vec::with_capacity(len + 128);
        let mut nal_len = 4i32;
        if !desc.is_null() {
            let (mut p, mut size, mut count) = (std::ptr::null(), 0usize, 0usize);
            if get(desc, 0, &mut p, &mut size, &mut count, &mut nal_len) == 0 && keyframe {
                for i in 0..count {
                    if get(desc, i, &mut p, &mut size, std::ptr::null_mut(), std::ptr::null_mut()) == 0 && !p.is_null() {
                        data.extend_from_slice(&[0, 0, 0, 1]);
                        data.extend_from_slice(std::slice::from_raw_parts(p, size));
                    }
                }
            }
        }
        data.extend(avcc_to_annex_b(&avcc, nal_len.clamp(1, 4) as usize)?);
        let pts = to_micros(CMSampleBufferGetPresentationTimeStamp(s)).unwrap_or(0);
        let dts = to_micros(CMSampleBufferGetDecodeTimeStamp(s)).unwrap_or(pts);
        Ok(VideoPacket { data, pts_us: pts, dts_us: dts, keyframe })
    }
}

struct Vt {
    session: VTCompressionSessionRef,
    shared: Box<Shared>,
    cfg: VideoConfig,
    name: String,
    hardware: bool,
}

// A compression session may be used from any thread; this one is used by one at a time.
unsafe impl Send for Vt {}

pub fn open_video(cfg: &VideoConfig, hardware: bool) -> Result<Box<dyn VideoEncoder>, String> {
    Vt::open(cfg, hardware).map(|e| Box::new(e) as Box<dyn VideoEncoder>)
}

impl Vt {
    fn open(cfg: &VideoConfig, hardware: bool) -> Result<Vt, String> {
        unsafe {
            let spec = Dict::new();
            spec.set(kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder, bool_ref(hardware));
            if hardware {
                spec.set(kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder, bool_ref(true));
            }
            let src = Dict::new();
            src.set_i32(kCVPixelBufferPixelFormatTypeKey, PIXEL_420V as i32);
            src.set_i32(kCVPixelBufferWidthKey, cfg.width as i32);
            src.set_i32(kCVPixelBufferHeightKey, cfg.height as i32);
            let io = Dict::new();
            src.set(kCVPixelBufferIOSurfacePropertiesKey, io.0);
            let shared = Box::new(Shared { codec: cfg.codec, out: Mutex::new(Vec::new()), dropped: Default::default() });
            let codec = match cfg.codec {
                Codec::H264 => CODEC_H264,
                Codec::Hevc => CODEC_HEVC,
            };
            let mut session = std::ptr::null_mut();
            let s = VTCompressionSessionCreate(
                std::ptr::null(),
                cfg.width as i32,
                cfg.height as i32,
                codec,
                spec.0,
                src.0,
                std::ptr::null(),
                Some(on_frame),
                &*shared as *const Shared as *mut c_void,
                &mut session,
            );
            if s != 0 || session.is_null() {
                return Err(format!("VideoToolbox can't make {} {} × {}{} (error {s})", cfg.codec.label(), cfg.width, cfg.height, if hardware { " on the graphics chip" } else { "" }));
            }
            let mut e = Vt { session, shared, cfg: cfg.clone(), name: String::new(), hardware };
            let set = |k: CFStringRef, v: CFTypeRef| VTSessionSetProperty(session, k, v);
            if cfg.low_latency {
                set(kVTCompressionPropertyKey_RealTime, bool_ref(true));
                set(kVTCompressionPropertyKey_AllowFrameReordering, bool_ref(false));
            }
            let rate = Num::i32(cfg.bitrate_bps.min(i32::MAX as u32) as i32);
            set(kVTCompressionPropertyKey_AverageBitRate, rate.0);
            let gop = Num::i32(cfg.gop_frames() as i32);
            set(kVTCompressionPropertyKey_MaxKeyFrameInterval, gop.0);
            let every = Num::f64(cfg.keyframe_every_s as f64);
            set(kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, every.0);
            let fps = Num::i32(cfg.fps as i32);
            set(kVTCompressionPropertyKey_ExpectedFrameRate, fps.0);
            set(
                kVTCompressionPropertyKey_ProfileLevel,
                match cfg.codec {
                    Codec::H264 => kVTProfileLevel_H264_High_AutoLevel,
                    Codec::Hevc => kVTProfileLevel_HEVC_Main_AutoLevel,
                },
            );
            set(kVTCompressionPropertyKey_ColorPrimaries, kCVImageBufferColorPrimaries_ITU_R_709_2);
            set(kVTCompressionPropertyKey_TransferFunction, kCVImageBufferTransferFunction_ITU_R_709_2);
            set(kVTCompressionPropertyKey_YCbCrMatrix, kCVImageBufferYCbCrMatrix_ITU_R_709_2);
            let s = VTCompressionSessionPrepareToEncodeFrames(session);
            if s != 0 {
                return Err(format!("VideoToolbox couldn't get ready (error {s})"));
            }
            let mut on_chip: CFTypeRef = std::ptr::null();
            let hw = VTSessionCopyProperty(session, kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder, std::ptr::null(), &mut on_chip as *mut CFTypeRef as *mut c_void) == 0
                && !on_chip.is_null()
                && CFBooleanGetValue(on_chip) != 0;
            if !on_chip.is_null() {
                CFRelease(on_chip);
            }
            if hardware && !hw {
                return Err("VideoToolbox gave a software encoder".into());
            }
            e.hardware = hw;
            e.name = format!("VideoToolbox {} ({})", cfg.codec.label(), if hw { "Apple media engine" } else { "software" });
            Ok(e)
        }
    }

    fn drain(&self) -> Result<Vec<VideoPacket>, String> {
        let got = std::mem::take(&mut *self.shared.out.lock().map_err(|_| "encoder queue poisoned")?);
        got.into_iter().collect()
    }
}

impl VideoEncoder for Vt {
    fn encode(&mut self, rgba: &[u8], pts_us: i64, force_keyframe: bool) -> Result<Vec<VideoPacket>, String> {
        let (w, h) = (self.cfg.width, self.cfg.height);
        unsafe {
            let mut pb: CVPixelBufferRef = std::ptr::null_mut();
            let pool = VTCompressionSessionGetPixelBufferPool(self.session);
            let made = if pool.is_null() {
                CVPixelBufferCreate(std::ptr::null(), w as usize, h as usize, PIXEL_420V, std::ptr::null(), &mut pb)
            } else {
                CVPixelBufferPoolCreatePixelBuffer(std::ptr::null(), pool, &mut pb)
            };
            if made != 0 || pb.is_null() {
                return Err(format!("no picture buffer for VideoToolbox (error {made})"));
            }
            if CVPixelBufferLockBaseAddress(pb, 0) != 0 {
                CFRelease(pb);
                return Err("couldn't lock a picture buffer".into());
            }
            let ys = CVPixelBufferGetBytesPerRowOfPlane(pb, 0);
            let uvs = CVPixelBufferGetBytesPerRowOfPlane(pb, 1);
            let y = std::slice::from_raw_parts_mut(CVPixelBufferGetBaseAddressOfPlane(pb, 0) as *mut u8, ys * h as usize);
            let uv = std::slice::from_raw_parts_mut(CVPixelBufferGetBaseAddressOfPlane(pb, 1) as *mut u8, uvs * h as usize / 2);
            rgba_to_nv12(rgba, w, h, y, ys, uv, uvs);
            CVPixelBufferUnlockBaseAddress(pb, 0);
            let props = force_keyframe.then(|| {
                let d = Dict::new();
                d.set(kVTEncodeFrameOptionKey_ForceKeyFrame, bool_ref(true));
                d
            });
            let s = VTCompressionSessionEncodeFrame(
                self.session,
                pb,
                micros(pts_us),
                micros(self.cfg.frame_us()),
                props.as_ref().map_or(std::ptr::null(), |d| d.0 as *const c_void),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            CFRelease(pb);
            if s != 0 {
                return Err(format!("VideoToolbox refused a picture (error {s})"));
            }
        }
        self.drain()
    }

    fn finish(&mut self) -> Result<Vec<VideoPacket>, String> {
        unsafe {
            VTCompressionSessionCompleteFrames(self.session, CMTIME_INVALID);
        }
        self.drain()
    }

    fn name(&self) -> EncoderName {
        EncoderName { name: self.name.clone(), hardware: self.hardware }
    }

    fn dropped(&self) -> u64 {
        self.shared.dropped.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Drop for Vt {
    fn drop(&mut self) {
        unsafe {
            // No callbacks after this, so `shared` can go.
            VTCompressionSessionCompleteFrames(self.session, CMTIME_INVALID);
            VTCompressionSessionInvalidate(self.session);
            CFRelease(self.session);
        }
    }
}

/// The encoders VideoToolbox has (media engine first), each found by opening a small session.
pub fn list() -> Vec<(Codec, EncoderName)> {
    let mut v = Vec::new();
    for hardware in [true, false] {
        for codec in [Codec::H264, Codec::Hevc] {
            let c = VideoConfig { width: 640, height: 360, fps: 30, bitrate_bps: 1_000_000, keyframe_every_s: 2, codec, prefer_hardware: hardware, low_latency: true };
            if let Ok(e) = Vt::open(&c, hardware) {
                v.push((codec, e.name()));
            }
        }
    }
    v
}

// ---------------------------------------------------------------------------------------------
// Sound: AudioToolbox's AAC encoder
// ---------------------------------------------------------------------------------------------

#[repr(C)]
struct AudioStreamBasicDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct AudioBuffer {
    channels: u32,
    data_byte_size: u32,
    data: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    count: u32,
    buffers: [AudioBuffer; 1],
}

#[repr(C)]
struct AudioStreamPacketDescription {
    start_offset: i64,
    variable_frames: u32,
    data_byte_size: u32,
}

type InputProc = unsafe extern "C" fn(AudioConverterRef, *mut u32, *mut AudioBufferList, *mut *mut AudioStreamPacketDescription, *mut c_void) -> OSStatus;

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    fn AudioConverterNew(src: *const AudioStreamBasicDescription, dst: *const AudioStreamBasicDescription, out: *mut AudioConverterRef) -> OSStatus;
    fn AudioConverterSetProperty(c: AudioConverterRef, id: u32, size: u32, data: *const c_void) -> OSStatus;
    fn AudioConverterFillComplexBuffer(c: AudioConverterRef, input: InputProc, user: *mut c_void, packets: *mut u32, out: *mut AudioBufferList, desc: *mut AudioStreamPacketDescription) -> OSStatus;
    fn AudioConverterDispose(c: AudioConverterRef) -> OSStatus;
}

const FORMAT_LPCM: u32 = fourcc(b"lpcm");
const FORMAT_AAC: u32 = fourcc(b"aac ");
const FLAGS_SIGNED_PACKED: u32 = 0x4 | 0x8;
const PROP_ENCODE_BIT_RATE: u32 = fourcc(b"brat");
/// What the input callback says when it has nothing more for now (not the end).
const NO_DATA_YET: OSStatus = fourcc(b"wait") as OSStatus;

/// Sound waiting to go in; `held` stays alive while the converter reads it.
struct Feed {
    pending: Vec<i16>,
    held: Vec<i16>,
    ended: bool,
}

unsafe extern "C" fn feed(_c: AudioConverterRef, packets: *mut u32, data: *mut AudioBufferList, _desc: *mut *mut AudioStreamPacketDescription, user: *mut c_void) -> OSStatus {
    unsafe {
        let f = &mut *(user as *mut Feed);
        let frames = (f.pending.len() / AUDIO_CHANNELS as usize).min(*packets as usize);
        if frames == 0 {
            *packets = 0;
            (*data).buffers[0].data_byte_size = 0;
            (*data).buffers[0].data = std::ptr::null_mut();
            return if f.ended { 0 } else { NO_DATA_YET };
        }
        f.held = f.pending.drain(..frames * AUDIO_CHANNELS as usize).collect();
        (*data).count = 1;
        (*data).buffers[0] = AudioBuffer { channels: AUDIO_CHANNELS, data_byte_size: (f.held.len() * 2) as u32, data: f.held.as_mut_ptr() as *mut c_void };
        *packets = frames as u32;
        0
    }
}

struct Aac {
    conv: AudioConverterRef,
    feed: Box<Feed>,
    start_us: Option<i64>,
    packets: u64,
}

unsafe impl Send for Aac {}

pub fn open_audio(bitrate_bps: u32) -> Result<Box<dyn AudioEncoder>, String> {
    let src = AudioStreamBasicDescription {
        sample_rate: AUDIO_RATE as f64,
        format_id: FORMAT_LPCM,
        format_flags: FLAGS_SIGNED_PACKED,
        bytes_per_packet: AUDIO_CHANNELS * 2,
        frames_per_packet: 1,
        bytes_per_frame: AUDIO_CHANNELS * 2,
        channels_per_frame: AUDIO_CHANNELS,
        bits_per_channel: 16,
        reserved: 0,
    };
    // Format flags for AAC are the MPEG-4 object type: 2 is AAC-LC.
    let dst = AudioStreamBasicDescription {
        sample_rate: AUDIO_RATE as f64,
        format_id: FORMAT_AAC,
        format_flags: 2,
        bytes_per_packet: 0,
        frames_per_packet: AAC_FRAME,
        bytes_per_frame: 0,
        channels_per_frame: AUDIO_CHANNELS,
        bits_per_channel: 0,
        reserved: 0,
    };
    unsafe {
        let mut conv = std::ptr::null_mut();
        let s = AudioConverterNew(&src, &dst, &mut conv);
        if s != 0 || conv.is_null() {
            return Err(format!("macOS's AAC encoder didn't start (error {s})"));
        }
        let rate = bitrate_bps.clamp(64_000, 320_000);
        let _ = AudioConverterSetProperty(conv, PROP_ENCODE_BIT_RATE, 4, &rate as *const u32 as *const c_void);
        Ok(Box::new(Aac { conv, feed: Box::new(Feed { pending: Vec::new(), held: Vec::new(), ended: false }), start_us: None, packets: 0 }))
    }
}

impl Aac {
    fn pull(&mut self, out: &mut Vec<AudioPacket>) -> Result<(), String> {
        let mut buf = vec![0u8; 8192];
        loop {
            if !self.feed.ended && self.feed.pending.len() < (AAC_FRAME * AUDIO_CHANNELS) as usize {
                return Ok(());
            }
            let mut list = AudioBufferList { count: 1, buffers: [AudioBuffer { channels: AUDIO_CHANNELS, data_byte_size: buf.len() as u32, data: buf.as_mut_ptr() as *mut c_void }] };
            let mut n = 1u32;
            let mut desc = AudioStreamPacketDescription { start_offset: 0, variable_frames: 0, data_byte_size: 0 };
            let s = unsafe { AudioConverterFillComplexBuffer(self.conv, feed, &mut *self.feed as *mut Feed as *mut c_void, &mut n, &mut list, &mut desc) };
            if n > 0 {
                let len = (list.buffers[0].data_byte_size as usize).min(buf.len());
                out.push(AudioPacket { data: buf[..len].to_vec(), pts_us: aac_packet_pts(self.start_us.unwrap_or(0), self.packets) });
                self.packets += 1;
            }
            if s != 0 && s != NO_DATA_YET {
                return Err(format!("macOS's AAC encoder failed (error {s})"));
            }
            if n == 0 {
                return Ok(());
            }
        }
    }
}

impl AudioEncoder for Aac {
    fn encode(&mut self, pcm: &[i16], pts_us: i64) -> Result<Vec<AudioPacket>, String> {
        self.start_us.get_or_insert(pts_us);
        self.feed.pending.extend_from_slice(pcm);
        let mut out = Vec::new();
        self.pull(&mut out)?;
        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<AudioPacket>, String> {
        self.feed.ended = true;
        let mut out = Vec::new();
        self.pull(&mut out)?;
        Ok(out)
    }

    fn audio_specific_config(&self) -> Vec<u8> {
        aac_lc_config(AUDIO_RATE, AUDIO_CHANNELS)
    }

    fn name(&self) -> EncoderName {
        EncoderName { name: "AudioToolbox AAC".into(), hardware: false }
    }
}

impl Drop for Aac {
    fn drop(&mut self) {
        unsafe {
            AudioConverterDispose(self.conv);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_frames_are_told_apart_from_errors_and_encoded_ones() {
        assert!(was_dropped(0, FRAME_DROPPED, false));
        assert!(was_dropped(0, FRAME_DROPPED, true));
        assert!(was_dropped(0, 0, false), "no picture back: dropped");
        assert!(!was_dropped(0, 0, true));
        assert!(!was_dropped(-12902, FRAME_DROPPED, false), "an error is an error, not a drop");
    }

    #[test]
    fn times_convert_between_core_media_and_microseconds() {
        assert_eq!(to_micros(micros(1_234_567)), Some(1_234_567));
        assert_eq!(to_micros(CMTime { value: 90_000, timescale: 90_000, flags: CMTIME_VALID, epoch: 0 }), Some(1_000_000));
        assert_eq!(to_micros(CMTIME_INVALID), None);
        assert_eq!(fourcc(b"avc1"), 0x6176_6331);
    }

    #[test]
    fn videotoolbox_encodes_when_this_mac_has_it() {
        let c = VideoConfig { width: 320, height: 240, fps: 30, bitrate_bps: 1_000_000, keyframe_every_s: 1, codec: Codec::H264, prefer_hardware: false, low_latency: true };
        let mut e = match Vt::open(&c, false) {
            Ok(e) => e,
            Err(err) => {
                println!("VideoToolbox skipped: {err}");
                return;
            }
        };
        let mut img = vec![0u8; c.rgba_len()];
        let mut out = Vec::new();
        for i in 0..40 {
            test_picture(&mut img, 320, 240, i);
            out.extend(e.encode(&img, i as i64 * c.frame_us(), i == 0).unwrap());
        }
        out.extend(e.finish().unwrap());
        assert!(out.len() >= 38, "{} packets", out.len());
        assert!(out[0].keyframe && sps_pps(&out[0].data).is_some());
    }

    #[test]
    fn aac_from_audiotoolbox() {
        let mut e = match open_audio(128_000) {
            Ok(e) => e,
            Err(err) => {
                println!("AudioToolbox AAC skipped: {err}");
                return;
            }
        };
        let pcm: Vec<i16> = (0..48_000 * 2).map(|i| ((i as f32 * 0.03).sin() * 6000.0) as i16).collect();
        let mut out = Vec::new();
        for piece in pcm.chunks(2 * 500) {
            out.extend(e.encode(piece, 0).unwrap());
        }
        out.extend(e.finish().unwrap());
        assert!((44..=50).contains(&out.len()), "{} packets", out.len());
        assert_eq!(e.audio_specific_config(), vec![0x11, 0x90]);
    }
}
