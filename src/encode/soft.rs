//! OpenH264 (Cisco's H.264 encoder, BSD-2-Clause, compiled into Snagster from its source): the
//! software encoder that works on every system, used when there's no graphics-chip encoder.
//!
//! It is driven through its own C interface (`openh264-sys2`) rather than the `openh264` crate's
//! wrapper, because the wrapper always encodes one slice per picture, and OpenH264 only spreads
//! the work over several processor cores when a picture is cut into several slices. With up to
//! four slices (OpenH264's limit) on four threads, 1080p encodes about three times as fast.
//!
//! Its hand-written x86 assembly is built when `nasm` is installed where Snagster is compiled
//! (`openh264-sys2` falls back to plain C without it); on 64-bit ARM its NEON assembly is
//! always built.

use super::*;
use openh264_sys2 as sys;
use std::os::raw::{c_int, c_void};

pub const NAME: &str = "OpenH264 (software)";

/// OpenH264 runs at most this many encoding threads (`MAX_THREADS_NUM`).
const MAX_THREADS: usize = 4;

/// How many threads (and slices) for a picture of `w` × `h` on a computer with `cores`: one for
/// small pictures, where slices only cost compression, else one per core up to four.
pub fn threads_for(w: u32, h: u32, cores: usize) -> usize {
    if (w as u64 * h as u64) < 640 * 360 { 1 } else { cores.clamp(1, MAX_THREADS) }
}

/// Screen-content mode keeps text sharp at a given bit rate but costs about a third more time
/// (1080p here: 36 against 53 frames a second on four threads, without assembly). It is used up
/// to 1080p at 30 fps; beyond that the faster camera mode keeps up with the frame rate.
pub fn screen_mode(cfg: &VideoConfig) -> bool {
    cfg.width as u64 * cfg.height as u64 * cfg.fps as u64 <= 1920 * 1080 * 30
}

/// The settings OpenH264 is started with (also checked by the tests).
fn params(cfg: &VideoConfig, threads: usize, defaults: sys::SEncParamExt) -> sys::SEncParamExt {
    let mut p = defaults;
    let bitrate = cfg.bitrate_bps.min(i32::MAX as u32) as c_int;
    p.iUsageType = if screen_mode(cfg) { sys::SCREEN_CONTENT_REAL_TIME } else { sys::CAMERA_VIDEO_REAL_TIME };
    p.iPicWidth = cfg.width as c_int;
    p.iPicHeight = cfg.height as c_int;
    p.iTargetBitrate = bitrate;
    p.iRCMode = sys::RC_BITRATE_MODE;
    p.fMaxFrameRate = cfg.fps as f32;
    // Every picture comes out as a frame (a skipped one would be a dropped frame in a
    // recording); the bit rate is then a target OpenH264 may overshoot on busy pictures.
    p.bEnableFrameSkip = false;
    p.bEnableDenoise = false;
    // Neither works on screen content (OpenH264 turns them off with a warning).
    p.bEnableAdaptiveQuant = false;
    p.bEnableBackgroundDetection = false;
    p.bEnableLongTermReference = false;
    p.bEnableSceneChangeDetect = true;
    p.eSpsPpsIdStrategy = sys::CONSTANT_ID;
    p.iComplexityMode = if cfg.low_latency { sys::LOW_COMPLEXITY } else { sys::MEDIUM_COMPLEXITY };
    p.uiIntraPeriod = cfg.gop_frames();
    // 0 asks for the screen-content QP range.
    p.iMinQp = 0;
    p.iMaxQp = 51;
    p.iLoopFilterDisableIdc = 0;
    p.iSpatialLayerNum = 1;
    p.iTemporalLayerNum = 1;
    p.iLtrMarkPeriod = 30;
    p.iMultipleThreadIdc = threads as u16;
    let l = &mut p.sSpatialLayers[0];
    l.iVideoWidth = cfg.width as c_int;
    l.iVideoHeight = cfg.height as c_int;
    l.fFrameRate = cfg.fps as f32;
    l.iSpatialBitrate = bitrate;
    l.iMaxSpatialBitrate = bitrate;
    // BT.709, video range: what `rgba_to_i420` makes.
    l.bVideoSignalTypePresent = true;
    l.bColorDescriptionPresent = true;
    l.bFullRange = false;
    l.uiColorPrimaries = 1;
    l.uiTransferCharacteristics = 1;
    l.uiColorMatrix = 1;
    if threads > 1 {
        l.sSliceArgument.uiSliceMode = sys::SM_FIXEDSLCNUM_SLICE;
        l.sSliceArgument.uiSliceNum = threads as u32;
    } else {
        l.sSliceArgument.uiSliceMode = sys::SM_SINGLE_SLICE;
        l.sSliceArgument.uiSliceNum = 1;
    }
    p
}

/// One OpenH264 encoder, destroyed on drop.
struct Raw(*mut sys::ISVCEncoder);

impl Raw {
    fn vt(&self) -> &sys::ISVCEncoderVtbl {
        // Made by WelsCreateSVCEncoder: a pointer to the encoder's function table.
        unsafe { &**self.0 }
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        unsafe {
            if let Some(un) = self.vt().Uninitialize {
                un(self.0);
            }
            sys::source::APILoader::WelsDestroySVCEncoder(self.0);
        }
    }
}

pub struct OpenH264 {
    enc: Raw,
    w: u32,
    h: u32,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    info: Box<sys::SFrameBSInfo>,
    started: bool,
}

// The encoder holds raw pointers into its own memory; it isn't tied to the thread that made it,
// and it's only ever used by one thread at a time (`&mut self`).
unsafe impl Send for OpenH264 {}

fn missing() -> String {
    "OpenH264's function table is incomplete".into()
}

impl OpenH264 {
    pub fn open(cfg: &VideoConfig) -> Result<OpenH264, String> {
        if cfg.codec != Codec::H264 {
            return Err("OpenH264 makes H.264 only".into());
        }
        let (long, short) = (cfg.width.max(cfg.height), cfg.width.min(cfg.height));
        if long > 3840 || short > 2160 {
            return Err(format!("OpenH264 encodes up to 3840 × 2160, not {} × {}", cfg.width, cfg.height));
        }
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        let threads = threads_for(cfg.width, cfg.height, cores);
        let mut ptr: *mut sys::ISVCEncoder = std::ptr::null_mut();
        if unsafe { sys::source::APILoader::WelsCreateSVCEncoder(&mut ptr) } != 0 || ptr.is_null() {
            return Err("OpenH264 couldn't start".into());
        }
        let enc = Raw(ptr);
        let vt = enc.vt();
        let (defaults, init, set) = (vt.GetDefaultParams.ok_or_else(missing)?, vt.InitializeExt.ok_or_else(missing)?, vt.SetOption.ok_or_else(missing)?);
        unsafe {
            // Quiet before the encoder starts: it otherwise prints notes about its parameters
            // on stdout.
            let mut quiet: c_int = sys::WELS_LOG_QUIET as c_int;
            set(enc.0, sys::ENCODER_OPTION_TRACE_LEVEL, &mut quiet as *mut c_int as *mut c_void);
            let mut d = sys::SEncParamExt::default();
            if defaults(enc.0, &mut d) != 0 {
                return Err("OpenH264 gave no default settings".into());
            }
            let p = params(cfg, threads, d);
            let r = init(enc.0, &p);
            if r != 0 {
                return Err(format!("OpenH264 refused the settings (error {r})"));
            }
            let mut fmt: c_int = sys::videoFormatI420 as c_int;
            set(enc.0, sys::ENCODER_OPTION_DATAFORMAT, &mut fmt as *mut c_int as *mut c_void);
        }
        let n = (cfg.width * cfg.height) as usize;
        Ok(OpenH264 { enc, w: cfg.width, h: cfg.height, y: vec![0; n], u: vec![0; n / 4], v: vec![0; n / 4], info: Box::default(), started: false })
    }

    /// The encoded frame's NAL units, Annex-B, one after another.
    fn bitstream(&self, out: &mut Vec<u8>) {
        let info = &*self.info;
        for layer in info.sLayerInfo.iter().take(info.iLayerNum.clamp(0, 128) as usize) {
            if layer.pNalLengthInByte.is_null() || layer.pBsBuf.is_null() || layer.iNalCount <= 0 {
                continue;
            }
            // The layer's NAL units sit back to back in pBsBuf, with their lengths alongside.
            let lens = unsafe { std::slice::from_raw_parts(layer.pNalLengthInByte, layer.iNalCount as usize) };
            let size: usize = lens.iter().map(|&l| l.max(0) as usize).sum();
            out.extend_from_slice(unsafe { std::slice::from_raw_parts(layer.pBsBuf, size) });
        }
    }
}

impl VideoEncoder for OpenH264 {
    fn encode(&mut self, rgba: &[u8], pts_us: i64, force_keyframe: bool) -> Result<Vec<VideoPacket>, String> {
        rgba_to_i420(rgba, self.w, self.h, &mut self.y, &mut self.u, &mut self.v);
        let vt = self.enc.vt();
        let (encode, intra) = (vt.EncodeFrame.ok_or_else(missing)?, vt.ForceIntraFrame.ok_or_else(missing)?);
        // The first picture is a keyframe anyway (and the encoder starts with it).
        if force_keyframe && self.started {
            unsafe { intra(self.enc.0, true) };
        }
        self.started = true;
        let w = self.w as c_int;
        let pic = sys::SSourcePicture {
            iColorFormat: sys::videoFormatI420 as c_int,
            iStride: [w, w / 2, w / 2, 0],
            // OpenH264 only reads the planes.
            pData: [self.y.as_mut_ptr(), self.u.as_mut_ptr(), self.v.as_mut_ptr(), std::ptr::null_mut()],
            iPicWidth: w,
            iPicHeight: self.h as c_int,
            uiTimeStamp: pts_us.max(0) / 1000,
            bPsnrY: false,
            bPsnrU: false,
            bPsnrV: false,
        };
        let r = unsafe { encode(self.enc.0, &pic, &mut *self.info) };
        if r != 0 {
            return Err(format!("OpenH264: encoding failed (error {r})"));
        }
        if matches!(self.info.eFrameType, sys::videoFrameTypeSkip | sys::videoFrameTypeInvalid) {
            return Ok(Vec::new());
        }
        let mut data = Vec::with_capacity(self.info.iFrameSizeInBytes.max(0) as usize);
        self.bitstream(&mut data);
        if data.is_empty() {
            return Ok(Vec::new());
        }
        let keyframe = is_keyframe(Codec::H264, &data);
        Ok(vec![VideoPacket { data, pts_us, dts_us: pts_us, keyframe }])
    }

    fn finish(&mut self) -> Result<Vec<VideoPacket>, String> {
        // OpenH264 hands each frame back at once; nothing waits inside.
        Ok(Vec::new())
    }

    fn name(&self) -> EncoderName {
        EncoderName { name: NAME.into(), hardware: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(w: u32, h: u32, fps: u32, every: u32) -> VideoConfig {
        VideoConfig { width: w, height: h, fps, bitrate_bps: 6_000_000, keyframe_every_s: every, codec: Codec::H264, prefer_hardware: false, low_latency: true }
    }

    fn run(enc: &mut dyn VideoEncoder, c: &VideoConfig, frames: u32, force_at: &[u32]) -> Vec<VideoPacket> {
        let mut img = vec![0u8; c.rgba_len()];
        let mut out = Vec::new();
        for i in 0..frames {
            test_picture(&mut img, c.width, c.height, i);
            out.extend(enc.encode(&img, i as i64 * c.frame_us(), force_at.contains(&i)).unwrap());
        }
        out.extend(enc.finish().unwrap());
        out
    }

    #[test]
    fn every_picture_comes_out_with_its_time_and_the_first_is_a_keyframe_with_sps_pps() {
        let c = cfg(160, 96, 30, 10);
        let mut e = OpenH264::open(&c).unwrap();
        let out = run(&mut e, &c, 20, &[]);
        assert_eq!(out.len(), 20, "one packet per picture");
        for (i, p) in out.iter().enumerate() {
            assert_eq!(p.pts_us, i as i64 * 33_333);
            assert_eq!(p.dts_us, p.pts_us, "no B-frames");
            assert!(p.data.starts_with(&[0, 0, 0, 1]) || p.data.starts_with(&[0, 0, 1]), "Annex-B");
        }
        assert!(out[0].keyframe);
        let (sps, pps) = sps_pps(&out[0].data).expect("SPS and PPS on the first keyframe");
        assert!(!pps.is_empty());
        let info = h264::parse_sps(&sps).unwrap();
        assert_eq!((info.width, info.height), (160, 96));
        assert!(out[1..].iter().all(|p| !p.keyframe), "no keyframes until the interval is up");
        assert_eq!(e.name(), EncoderName { name: NAME.into(), hardware: false });
    }

    #[test]
    fn keyframes_come_every_interval_and_when_asked() {
        let c = cfg(128, 72, 30, 1);
        let mut e = open_backend(Backend::OpenH264, &c).map(|e| Paced::new(e, &c)).unwrap();
        let out = run(&mut e, &c, 95, &[45]);
        let keys: Vec<i64> = out.iter().filter(|p| p.keyframe).map(|p| p.pts_us / c.frame_us()).collect();
        assert_eq!(keys, vec![0, 30, 45, 75], "every second (30 frames) and the one asked for, counted from it");
        for p in out.iter().filter(|p| p.keyframe) {
            assert!(sps_pps(&p.data).is_some(), "the keyframe at {} carries SPS and PPS", p.pts_us);
        }
    }

    #[test]
    fn odd_sizes_and_other_codecs_are_refused() {
        assert!(OpenH264::open(&VideoConfig { codec: Codec::Hevc, ..cfg(64, 64, 30, 1) }).is_err());
        assert!(OpenH264::open(&cfg(4096, 2160, 30, 1)).is_err(), "beyond 4K");
        assert!(OpenH264::open(&cfg(2160, 3840, 30, 1)).is_ok(), "portrait 4K is fine");
    }

    #[test]
    fn the_picture_decodes_back_to_what_went_in() {
        use openh264::decoder::Decoder;
        use openh264::formats::YUVSource;
        let c = cfg(64, 64, 30, 1);
        let mut e = OpenH264::open(&VideoConfig { bitrate_bps: 4_000_000, ..c.clone() }).unwrap();
        // Top half white, bottom half black.
        let img: Vec<u8> = (0..64 * 64).flat_map(|i| if i < 64 * 32 { [255, 255, 255, 255] } else { [0, 0, 0, 255] }).collect();
        let p = e.encode(&img, 0, false).unwrap().remove(0);
        let mut d = Decoder::new().unwrap();
        let yuv = d.decode(&p.data).unwrap().expect("a picture");
        let (ys, _, _) = yuv.strides();
        let y = yuv.y();
        let (top, bottom) = (y[10 * ys + 10], y[50 * ys + 10]);
        assert!((top as i32 - 235).abs() <= 6, "top {top}");
        assert!((bottom as i32 - 16).abs() <= 6, "bottom {bottom}");
    }

    #[test]
    fn small_pictures_use_one_thread_and_big_ones_up_to_four() {
        assert_eq!(threads_for(320, 240, 16), 1);
        assert_eq!(threads_for(1280, 720, 1), 1, "one core, one thread");
        assert_eq!(threads_for(1280, 720, 2), 2);
        assert_eq!(threads_for(1920, 1080, 16), 4, "OpenH264's limit");
        assert_eq!(threads_for(1920, 1080, 0), 1);
    }

    #[test]
    fn the_settings_ask_for_slices_only_with_threads() {
        let c = cfg(1920, 1080, 60, 2);
        let one = params(&c, 1, sys::SEncParamExt::default());
        assert_eq!(one.sSpatialLayers[0].sSliceArgument.uiSliceMode, sys::SM_SINGLE_SLICE);
        let four = params(&c, 4, sys::SEncParamExt::default());
        assert_eq!((four.iMultipleThreadIdc, four.sSpatialLayers[0].sSliceArgument.uiSliceMode, four.sSpatialLayers[0].sSliceArgument.uiSliceNum), (4, sys::SM_FIXEDSLCNUM_SLICE, 4));
        assert_eq!((four.iPicWidth, four.iPicHeight, four.uiIntraPeriod), (1920, 1080, 120));
        assert_eq!(four.iUsageType, sys::CAMERA_VIDEO_REAL_TIME, "1080p60: the faster mode");
        assert_eq!(params(&VideoConfig { fps: 30, ..c.clone() }, 4, sys::SEncParamExt::default()).iUsageType, sys::SCREEN_CONTENT_REAL_TIME);
        assert!(screen_mode(&cfg(1280, 720, 60, 2)), "720p60 is no more work than 1080p30");
        assert!(!screen_mode(&cfg(2560, 1440, 30, 2)));
        assert!(!four.bEnableFrameSkip, "every picture becomes a frame");
        assert_eq!(params(&VideoConfig { low_latency: false, ..c }, 1, sys::SEncParamExt::default()).iComplexityMode, sys::MEDIUM_COMPLEXITY);
    }

    /// Several slices on several threads: each picture is still one access unit that decodes to
    /// the right picture.
    #[test]
    fn threaded_slices_decode_to_the_whole_picture() {
        use openh264::decoder::Decoder;
        use openh264::formats::YUVSource;
        let c = cfg(1280, 720, 30, 2);
        let mut e = OpenH264::open(&c).unwrap();
        let threads = threads_for(1280, 720, std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));
        let mut img = vec![0u8; c.rgba_len()];
        // Left half black, right half white, so every slice has both.
        for (i, p) in img.chunks_exact_mut(4).enumerate() {
            let v = if i % 1280 >= 640 { 255 } else { 0 };
            p.copy_from_slice(&[v, v, v, 255]);
        }
        let out: Vec<VideoPacket> = (0..3).flat_map(|i| e.encode(&img, i * c.frame_us(), false).unwrap()).collect();
        assert_eq!(out.len(), 3);
        let slices = split_annex_b(&out[0].data).iter().filter(|n| nal_type(Codec::H264, n) == 5).count();
        assert_eq!(slices, threads, "one IDR slice per thread");
        let mut d = Decoder::new().unwrap();
        let yuv = d.decode(&out[0].data).unwrap().expect("a picture");
        let (ys, _, _) = yuv.strides();
        let y = yuv.y();
        for row in [5, 360, 715] {
            assert!((y[row * ys + 100] as i32 - 16).abs() <= 6, "row {row} left");
            assert!((y[row * ys + 1200] as i32 - 235).abs() <= 6, "row {row} right");
        }
    }

    /// 1080p frames per second, for comparing settings:
    /// `cargo test --release -- --ignored openh264_speed --nocapture`.
    #[test]
    #[ignore]
    fn openh264_speed() {
        for (w, h) in [(1280, 720), (1920, 1080)] {
            let (_, fps) = speed(Backend::OpenH264, w, h, 120).unwrap();
            println!("OpenH264 {w}x{h}: {fps:.1} fps");
        }
    }
}
