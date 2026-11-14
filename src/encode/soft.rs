//! OpenH264 (Cisco's H.264 encoder, BSD-2-Clause, compiled into Snagster from its source): the
//! software encoder that works on every system, used when there's no graphics-chip encoder.

use super::*;
use openh264::OpenH264API;
use openh264::Timestamp;
use openh264::encoder::{BitRate, Complexity, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, RateControlMode, UsageType, VuiConfig};
use openh264::formats::YUVSlices;

pub const NAME: &str = "OpenH264 (software)";

pub struct OpenH264 {
    enc: Encoder,
    w: u32,
    h: u32,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    started: bool,
}

// The encoder holds raw pointers into its own memory; it isn't tied to the thread that made it,
// and it's only ever used by one thread at a time (`&mut self`).
unsafe impl Send for OpenH264 {}

impl OpenH264 {
    pub fn open(cfg: &VideoConfig) -> Result<OpenH264, String> {
        if cfg.codec != Codec::H264 {
            return Err("OpenH264 makes H.264 only".into());
        }
        let (long, short) = (cfg.width.max(cfg.height), cfg.width.min(cfg.height));
        if long > 3840 || short > 2160 {
            return Err(format!("OpenH264 encodes up to 3840 × 2160, not {} × {}", cfg.width, cfg.height));
        }
        let c = EncoderConfig::new()
            .bitrate(BitRate::from_bps(cfg.bitrate_bps))
            .max_frame_rate(FrameRate::from_hz(cfg.fps as f32))
            .rate_control_mode(RateControlMode::Bitrate)
            // Every picture comes out as a frame (a skipped one would be a dropped frame in a
            // recording); the bit rate is then a target OpenH264 may overshoot on busy pictures.
            .skip_frames(false)
            // Neither works on screen content (OpenH264 turns them off with a warning).
            .adaptive_quantization(false)
            .background_detection(false)
            .usage_type(if cfg.low_latency { UsageType::ScreenContentRealTime } else { UsageType::ScreenContentNonRealTime })
            .complexity(if cfg.low_latency { Complexity::Low } else { Complexity::Medium })
            .intra_frame_period(IntraFramePeriod::from_num_frames(cfg.gop_frames()))
            .vui(VuiConfig::bt709());
        let mut enc = Encoder::with_api_config(OpenH264API::from_source(), c).map_err(|e| e.to_string())?;
        // Quiet before the encoder starts: it otherwise prints notes about its parameters on
        // stdout. (25 is ENCODER_OPTION_TRACE_LEVEL, 0 is WELS_LOG_QUIET.)
        let mut quiet: std::ffi::c_int = 0;
        unsafe {
            enc.raw_api().set_option(25, &mut quiet as *mut std::ffi::c_int as *mut std::ffi::c_void);
        }
        let n = (cfg.width * cfg.height) as usize;
        Ok(OpenH264 { enc, w: cfg.width, h: cfg.height, y: vec![0; n], u: vec![0; n / 4], v: vec![0; n / 4], started: false })
    }
}

impl VideoEncoder for OpenH264 {
    fn encode(&mut self, rgba: &[u8], pts_us: i64, force_keyframe: bool) -> Result<Vec<VideoPacket>, String> {
        rgba_to_i420(rgba, self.w, self.h, &mut self.y, &mut self.u, &mut self.v);
        // The first picture is a keyframe anyway (and the encoder starts with it).
        if force_keyframe && self.started {
            self.enc.force_intra_frame();
        }
        self.started = true;
        let (w, h) = (self.w as usize, self.h as usize);
        let yuv = YUVSlices::new((&self.y, &self.u, &self.v), (w, h), (w, w / 2, w / 2));
        let bits = self.enc.encode_at(&yuv, Timestamp::from_millis(pts_us.max(0) as u64 / 1000)).map_err(|e| format!("OpenH264: {e}"))?;
        if matches!(bits.frame_type(), FrameType::Skip | FrameType::Invalid) {
            return Ok(Vec::new());
        }
        let data = bits.to_vec();
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
}
