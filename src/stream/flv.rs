//! FLV tag bodies, which are what RTMP audio and video messages carry: H.264 (AVC) and HEVC
//! (Enhanced RTMP, fourCC `hvc1`) video, AAC sound and the `onMetaData` description. Written from
//! the public FLV/F4V specification (Adobe), ISO/IEC 14496-15 (avcC/hvcC) and the Enhanced RTMP
//! specification (Veovera). Pure functions; the encoders hand us Annex-B access units.

use super::amf::Amf;
use crate::encode::Codec;

/// The NAL units in an Annex-B stream (start codes `00 00 01` or `00 00 00 01`), without their
/// start codes. Trailing zero bytes before a start code belong to the start code.
pub fn split_annexb(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new(); // (payload start, start-code start)
    let mut i = 0;
    while i + 2 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            let mut sc = i;
            while sc > 0 && data[sc - 1] == 0 {
                sc -= 1;
            }
            starts.push((i + 3, sc));
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::new();
    for (k, &(s, _)) in starts.iter().enumerate() {
        let end = starts.get(k + 1).map(|&(_, sc)| sc).unwrap_or(data.len());
        if end > s {
            out.push(&data[s..end]);
        }
    }
    if starts.is_empty() && !data.is_empty() {
        // Not Annex-B at all: treat it as one NAL unit.
        out.push(data);
    }
    out
}

fn nal_type(codec: Codec, nal: &[u8]) -> u8 {
    match codec {
        Codec::H264 => nal.first().map(|b| b & 0x1F).unwrap_or(0),
        Codec::Hevc => nal.first().map(|b| (b >> 1) & 0x3F).unwrap_or(0),
    }
}

/// Access unit delimiters: FLV doesn't want them.
fn is_aud(codec: Codec, nal: &[u8]) -> bool {
    match codec {
        Codec::H264 => nal_type(codec, nal) == 9,
        Codec::Hevc => nal_type(codec, nal) == 35,
    }
}

/// Annex-B → AVCC/HVCC: each NAL unit with a 4-byte big-endian length in front.
pub fn annexb_to_length_prefixed(codec: Codec, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 16);
    for nal in split_annexb(data) {
        if is_aud(codec, nal) {
            continue;
        }
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

/// The parameter sets in an access unit: (VPS, SPS, PPS); VPS only for HEVC.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParamSets {
    pub vps: Vec<Vec<u8>>,
    pub sps: Vec<Vec<u8>>,
    pub pps: Vec<Vec<u8>>,
}

impl ParamSets {
    pub fn find(codec: Codec, data: &[u8]) -> ParamSets {
        let mut p = ParamSets::default();
        for nal in split_annexb(data) {
            let t = nal_type(codec, nal);
            let slot = match (codec, t) {
                (Codec::H264, 7) | (Codec::Hevc, 33) => &mut p.sps,
                (Codec::H264, 8) | (Codec::Hevc, 34) => &mut p.pps,
                (Codec::Hevc, 32) => &mut p.vps,
                _ => continue,
            };
            if !slot.iter().any(|x| x == nal) {
                slot.push(nal.to_vec());
            }
        }
        p
    }

    /// Enough to build a decoder configuration.
    pub fn complete(&self, codec: Codec) -> bool {
        !self.sps.is_empty() && !self.pps.is_empty() && (codec == Codec::H264 || !self.vps.is_empty())
    }
}

/// AVCDecoderConfigurationRecord (avcC) from SPS and PPS.
pub fn avcc(p: &ParamSets) -> Result<Vec<u8>, String> {
    let sps = p.sps.first().ok_or("the first keyframe has no SPS")?;
    if sps.len() < 4 || p.pps.is_empty() {
        return Err("the first keyframe has no usable SPS/PPS".into());
    }
    let mut out = vec![1, sps[1], sps[2], sps[3], 0xFF, 0xE0 | (p.sps.len().min(31) as u8)];
    for s in p.sps.iter().take(31) {
        out.extend_from_slice(&(s.len() as u16).to_be_bytes());
        out.extend_from_slice(s);
    }
    out.push(p.pps.len().min(255) as u8);
    for s in p.pps.iter().take(255) {
        out.extend_from_slice(&(s.len() as u16).to_be_bytes());
        out.extend_from_slice(s);
    }
    Ok(out)
}

/// RBSP: the NAL payload with emulation-prevention bytes (`00 00 03`) removed.
pub fn unescape(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// HEVCDecoderConfigurationRecord (hvcC) from VPS, SPS and PPS. The profile, tier and level are
/// read from the SPS; chroma 4:2:0 and 8-bit samples are assumed (what our encoders make).
pub fn hvcc(p: &ParamSets) -> Result<Vec<u8>, String> {
    if !p.complete(Codec::Hevc) {
        return Err("the first HEVC keyframe has no VPS/SPS/PPS".into());
    }
    let sps = unescape(&p.sps[0]);
    // 2 bytes NAL header, 1 byte (vps id, max sub layers, temporal id nesting), then the
    // general profile_tier_level: 1 + 4 + 6 + 1 bytes.
    if sps.len() < 15 {
        return Err("the HEVC SPS is too short".into());
    }
    let nesting = sps[2] & 1;
    let ptl = &sps[3..15];
    let mut out = vec![1];
    out.extend_from_slice(ptl); // profile space/tier/idc, compatibility flags, constraints, level
    out.extend_from_slice(&[0xF0, 0x00]); // min_spatial_segmentation_idc 0
    out.push(0xFC); // parallelismType 0
    out.push(0xFC | 1); // chroma_format_idc 1 (4:2:0)
    out.push(0xF8); // bit depth luma 8
    out.push(0xF8); // bit depth chroma 8
    out.extend_from_slice(&[0, 0]); // avgFrameRate unknown
    // constantFrameRate 0, numTemporalLayers 1, temporalIdNested, lengthSizeMinusOne 3.
    out.push((1 << 3) | (nesting << 2) | 3);
    out.push(3);
    for (t, list) in [(32u8, &p.vps), (33, &p.sps), (34, &p.pps)] {
        out.push(0x80 | t);
        out.extend_from_slice(&(list.len() as u16).to_be_bytes());
        for n in list {
            out.extend_from_slice(&(n.len() as u16).to_be_bytes());
            out.extend_from_slice(n);
        }
    }
    Ok(out)
}

/// Video message body for the decoder configuration (sent before the first frame).
pub fn video_sequence_header(codec: Codec, p: &ParamSets) -> Result<Vec<u8>, String> {
    Ok(match codec {
        Codec::H264 => {
            let mut b = vec![0x17, 0, 0, 0, 0];
            b.extend(avcc(p)?);
            b
        }
        Codec::Hevc => {
            // Enhanced RTMP: IsExHeader | key frame | PacketTypeSequenceStart, fourCC.
            let mut b = vec![0x80 | (1 << 4), b'h', b'v', b'c', b'1'];
            b.extend(hvcc(p)?);
            b
        }
    })
}

fn si24(v: i32) -> [u8; 3] {
    let v = v.clamp(-(1 << 23), (1 << 23) - 1) as u32;
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

/// Video message body for one access unit (Annex-B in). `cts_ms` is presentation minus decode
/// time.
pub fn video_frame(codec: Codec, annexb: &[u8], keyframe: bool, cts_ms: i32) -> Vec<u8> {
    let nals = annexb_to_length_prefixed(codec, annexb);
    let frame_type: u8 = if keyframe { 1 } else { 2 };
    let mut b = Vec::with_capacity(nals.len() + 8);
    match codec {
        Codec::H264 => {
            b.push((frame_type << 4) | 7);
            b.push(1); // AVC NALU
            b.extend_from_slice(&si24(cts_ms));
        }
        Codec::Hevc => {
            if cts_ms == 0 {
                b.push(0x80 | (frame_type << 4) | 3); // CodedFramesX: no composition time
                b.extend_from_slice(b"hvc1");
            } else {
                b.push(0x80 | (frame_type << 4) | 1); // CodedFrames
                b.extend_from_slice(b"hvc1");
                b.extend_from_slice(&si24(cts_ms));
            }
        }
    }
    b.extend(nals);
    b
}

/// End of the video (sent before unpublishing; harmless where ignored).
pub fn video_end(codec: Codec) -> Vec<u8> {
    match codec {
        Codec::H264 => vec![0x17, 2, 0, 0, 0],
        Codec::Hevc => vec![0x80 | (1 << 4) | 2, b'h', b'v', b'c', b'1'],
    }
}

/// AAC, 44 kHz flag (FLV says always 3 for AAC), 16-bit, stereo.
const AAC_HEADER: u8 = 0xAF;

pub fn audio_sequence_header(asc: &[u8]) -> Vec<u8> {
    let mut b = vec![AAC_HEADER, 0];
    b.extend_from_slice(asc);
    b
}

pub fn audio_frame(raw_aac: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(raw_aac.len() + 2);
    b.push(AAC_HEADER);
    b.push(1);
    b.extend_from_slice(raw_aac);
    b
}

/// What `onMetaData` says about the stream.
#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub video_kbps: u32,
    pub codec: Codec,
    /// AAC bitrate, None without sound.
    pub audio_kbps: Option<u32>,
}

/// The `@setDataFrame` data message body.
pub fn metadata(m: &Meta) -> Vec<u8> {
    let codec_id = match m.codec {
        Codec::H264 => 7.0,
        Codec::Hevc => u32::from_be_bytes(*b"hvc1") as f64,
    };
    let mut f = vec![
        ("duration".to_string(), Amf::Number(0.0)),
        ("width".into(), Amf::Number(m.width as f64)),
        ("height".into(), Amf::Number(m.height as f64)),
        ("videodatarate".into(), Amf::Number(m.video_kbps as f64)),
        ("framerate".into(), Amf::Number(m.fps as f64)),
        ("videocodecid".into(), Amf::Number(codec_id)),
    ];
    if let Some(a) = m.audio_kbps {
        f.extend([
            ("audiodatarate".to_string(), Amf::Number(a as f64)),
            ("audiosamplerate".into(), Amf::Number(48_000.0)),
            ("audiosamplesize".into(), Amf::Number(16.0)),
            ("audiochannels".into(), Amf::Number(2.0)),
            ("stereo".into(), Amf::Bool(true)),
            ("audiocodecid".into(), Amf::Number(10.0)),
        ]);
    }
    f.push(("encoder".into(), Amf::Str(format!("Snagster {}", env!("CARGO_PKG_VERSION")))));
    super::amf::encode(&[Amf::str("@setDataFrame"), Amf::str("onMetaData"), Amf::EcmaArray(f)])
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPS: &[u8] = &[0x67, 0x64, 0x00, 0x1F, 0xAC, 0xD9];
    const PPS: &[u8] = &[0x68, 0xEB, 0xE3, 0xCB];
    const IDR: &[u8] = &[0x65, 0x88, 0x84, 0x00];

    fn h264_key() -> Vec<u8> {
        let mut v = vec![0, 0, 0, 1, 0x09, 0xF0]; // AUD
        v.extend([0, 0, 0, 1]);
        v.extend(SPS);
        v.extend([0, 0, 1]);
        v.extend(PPS);
        v.extend([0, 0, 0, 1]);
        v.extend(IDR);
        v
    }

    #[test]
    fn annexb_splits_on_three_and_four_byte_start_codes() {
        let k = h264_key();
        let nals = split_annexb(&k);
        assert_eq!(nals, vec![&[0x09, 0xF0][..], SPS, PPS, IDR]);
        assert_eq!(split_annexb(&[]), Vec::<&[u8]>::new());
        assert_eq!(split_annexb(&[0x65, 1, 2]), vec![&[0x65, 1, 2][..]]);
    }

    #[test]
    fn length_prefixed_drops_the_delimiter() {
        let out = annexb_to_length_prefixed(Codec::H264, &h264_key());
        let mut want = Vec::new();
        for n in [SPS, PPS, IDR] {
            want.extend((n.len() as u32).to_be_bytes());
            want.extend(n);
        }
        assert_eq!(out, want);
    }

    #[test]
    fn avcc_comes_from_the_first_keyframes_parameter_sets() {
        let p = ParamSets::find(Codec::H264, &h264_key());
        assert!(p.complete(Codec::H264));
        let c = avcc(&p).unwrap();
        assert_eq!(&c[..6], &[1, 0x64, 0x00, 0x1F, 0xFF, 0xE1]);
        assert_eq!(&c[6..8], &(SPS.len() as u16).to_be_bytes());
        assert_eq!(&c[8..8 + SPS.len()], SPS);
        assert_eq!(c[8 + SPS.len()], 1);
        assert_eq!(&c[c.len() - PPS.len()..], PPS);
        assert!(avcc(&ParamSets::default()).is_err());
    }

    #[test]
    fn avc_frames_carry_type_and_composition_time() {
        let b = video_frame(Codec::H264, &h264_key(), true, 40);
        assert_eq!(&b[..5], &[0x17, 1, 0, 0, 40]);
        let p = video_frame(Codec::H264, &[0, 0, 1, 0x41, 9], false, -2);
        assert_eq!(&p[..5], &[0x27, 1, 0xFF, 0xFF, 0xFE]);
        assert_eq!(&p[5..], &[0, 0, 0, 2, 0x41, 9]);
        let h = video_sequence_header(Codec::H264, &ParamSets::find(Codec::H264, &h264_key())).unwrap();
        assert_eq!(&h[..5], &[0x17, 0, 0, 0, 0]);
    }

    #[test]
    fn emulation_prevention_bytes_are_removed() {
        assert_eq!(unescape(&[0, 0, 3, 1, 0, 0, 3, 0, 3]), vec![0, 0, 1, 0, 0, 0, 3]);
    }

    #[test]
    fn hevc_uses_enhanced_rtmp_and_hvcc() {
        let vps = vec![0x40, 0x01, 0x0C];
        // SPS: header 42 01, then 0x01 (one sub layer, nesting), then a 12-byte PTL:
        // Main profile (1), compatibility 0x60000000, constraints 0x90.., level 93.
        let sps = vec![0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x00, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 93, 0xA0];
        let pps = vec![0x44, 0x01, 0xC1];
        let idr = vec![0x26, 0x01, 0xAF];
        let mut au = Vec::new();
        for n in [&vps, &sps, &pps, &idr] {
            au.extend([0, 0, 0, 1]);
            au.extend(n.iter());
        }
        let p = ParamSets::find(Codec::Hevc, &au);
        assert!(p.complete(Codec::Hevc));
        let c = hvcc(&p).unwrap();
        assert_eq!(c[0], 1);
        assert_eq!(c[1], 0x01); // profile Main
        assert_eq!(c[12], 93); // level
        assert_eq!(c[21] & 3, 3); // 4-byte lengths
        assert_eq!(c[22], 3); // three arrays
        assert_eq!(c[23], 0x80 | 32);
        let h = video_sequence_header(Codec::Hevc, &p).unwrap();
        assert_eq!(&h[..5], b"\x90hvc1");
        let f = video_frame(Codec::Hevc, &au, true, 0);
        assert_eq!(&f[..5], b"\x93hvc1");
        assert_eq!(&f[5..9], &(vps.len() as u32).to_be_bytes());
        let f = video_frame(Codec::Hevc, &[0, 0, 1, 2, 1, 5], false, 33);
        assert_eq!(&f[..8], b"\xA1hvc1\x00\x00\x21");
        assert!(hvcc(&ParamSets::default()).is_err());
    }

    #[test]
    fn aac_tags() {
        assert_eq!(audio_sequence_header(&[0x11, 0x90]), vec![0xAF, 0, 0x11, 0x90]);
        assert_eq!(audio_frame(&[1, 2, 3]), vec![0xAF, 1, 1, 2, 3]);
    }

    #[test]
    fn metadata_describes_the_stream() {
        let m = Meta { width: 1280, height: 720, fps: 30, video_kbps: 4500, codec: Codec::H264, audio_kbps: Some(160) };
        let v = super::super::amf::decode_all(&metadata(&m)).unwrap();
        assert_eq!(v[0].as_str(), Some("@setDataFrame"));
        assert_eq!(v[1].as_str(), Some("onMetaData"));
        assert_eq!(v[2].get("width").and_then(Amf::as_number), Some(1280.0));
        assert_eq!(v[2].get("videocodecid").and_then(Amf::as_number), Some(7.0));
        assert_eq!(v[2].get("audiocodecid").and_then(Amf::as_number), Some(10.0));
        let silent = Meta { audio_kbps: None, ..m };
        let v = super::super::amf::decode_all(&metadata(&silent)).unwrap();
        assert!(v[2].get("audiocodecid").is_none());
    }

    /// Whatever an encoder hands over (cut short, start codes in odd places), the tags are
    /// made without a panic.
    #[test]
    fn odd_access_units_never_panic() {
        let good = [&[0u8, 0, 0, 1, 0x67, 0x42, 0xC0, 0x1F, 0xDA][..], &[0, 0, 1, 0x68, 0xCE, 0x3C, 0x80], &[0, 0, 0, 1, 0x65, 1, 2, 3], &[0, 0, 1, 0x40, 1, 0x0C, 0, 0, 0, 1, 0x42, 1, 1, 1, 0x60, 0, 0, 3, 0, 0, 1, 0x44]].concat();
        let mut g = crate::testalloc::Bytes::new(9);
        for i in 0..5_000 {
            let au = if i % 2 == 0 { g.mutate(&good) } else { g.noise(64) };
            for codec in [Codec::H264, Codec::Hevc] {
                let nals = split_annexb(&au);
                assert!(nals.iter().map(|n| n.len()).sum::<usize>() <= au.len());
                let p = ParamSets::find(codec, &au);
                let _ = video_sequence_header(codec, &p);
                assert!(video_frame(codec, &au, i % 3 == 0, (i as i32) - 2500).len() <= 9 + au.len() + 4 * nals.len());
            }
        }
    }
}
