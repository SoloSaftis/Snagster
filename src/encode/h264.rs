//! A small H.264 bitstream writer and reader: the sequence and picture parameter sets (SPS, PPS)
//! that the VA-API encoder hands to the driver, and reading an SPS back (picture size, profile)
//! to check them. Written from the H.264 specification (ITU-T H.264, 7.3.2.1 and 7.3.2.2).

/// Writes bits most significant first, with Exp-Golomb codes.
#[derive(Default)]
pub struct BitWriter {
    bytes: Vec<u8>,
    cur: u8,
    used: u8,
}

impl BitWriter {
    pub fn bit(&mut self, b: bool) {
        self.cur = (self.cur << 1) | b as u8;
        self.used += 1;
        if self.used == 8 {
            self.bytes.push(self.cur);
            self.cur = 0;
            self.used = 0;
        }
    }

    /// The low `n` bits of `v` (n ≤ 32).
    pub fn bits(&mut self, n: u32, v: u32) {
        for i in (0..n).rev() {
            self.bit((v >> i) & 1 == 1);
        }
    }

    /// Unsigned Exp-Golomb.
    pub fn ue(&mut self, v: u32) {
        let x = v as u64 + 1;
        let len = 64 - x.leading_zeros();
        self.bits(len - 1, 0);
        for i in (0..len).rev() {
            self.bit((x >> i) & 1 == 1);
        }
    }

    /// Signed Exp-Golomb.
    pub fn se(&mut self, v: i32) {
        self.ue(if v > 0 { (v as u32) * 2 - 1 } else { (-(v as i64) as u32) * 2 });
    }

    /// rbsp_trailing_bits, then the bytes.
    pub fn finish(mut self) -> Vec<u8> {
        self.bit(true);
        while self.used != 0 {
            self.bit(false);
        }
        self.bytes
    }
}

/// A NAL unit with a four-byte start code: header byte, then the payload with emulation
/// prevention bytes (00 00 0x → 00 00 03 0x for x ≤ 3).
pub fn nal(header: u8, rbsp: &[u8]) -> Vec<u8> {
    let mut out = vec![0, 0, 0, 1, header];
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// Remove emulation prevention bytes (the reverse of `nal`, without the header).
pub fn unescape(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len());
    let mut zeros = 0;
    for &b in payload {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// Which H.264 profile: decides CABAC and the 8 × 8 transform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    ConstrainedBaseline,
    Main,
    High,
}

impl Profile {
    pub fn idc(self) -> u8 {
        match self {
            Profile::ConstrainedBaseline => 66,
            Profile::Main => 77,
            Profile::High => 100,
        }
    }

    /// constraint_set0..5 flags byte.
    pub fn constraints(self) -> u8 {
        match self {
            // Constrained Baseline is Baseline that Main decoders can play too.
            Profile::ConstrainedBaseline => 0xc0,
            _ => 0,
        }
    }

    pub fn cabac(self) -> bool {
        self != Profile::ConstrainedBaseline
    }

    pub fn transform_8x8(self) -> bool {
        self == Profile::High
    }
}

/// The smallest level that fits `w` × `h` at `fps` (macroblocks per frame and per second),
/// as level_idc (31 = 3.1). Bit rate limits aren't checked: drivers clamp them.
pub fn level_for(w: u32, h: u32, fps: u32) -> u8 {
    let mbs = w.div_ceil(16) as u64 * h.div_ceil(16) as u64;
    let rate = mbs * fps.max(1) as u64;
    // (level_idc, max macroblocks per second, max frame size in macroblocks) — Table A-1.
    const LEVELS: [(u8, u64, u64); 16] = [
        (10, 1485, 99),
        (11, 3000, 396),
        (12, 6000, 396),
        (13, 11880, 396),
        (20, 11880, 396),
        (21, 19800, 792),
        (22, 20250, 1620),
        (30, 40500, 1620),
        (31, 108000, 3600),
        (32, 216000, 5120),
        (40, 245760, 8192),
        (41, 245760, 8192),
        (42, 522240, 8704),
        (50, 589824, 22080),
        (51, 983040, 36864),
        (52, 2073600, 36864),
    ];
    for (idc, max_rate, max_mbs) in LEVELS {
        if mbs <= max_mbs && rate <= max_rate {
            return idc;
        }
    }
    // Beyond 5.2 (8K, or 4K above 60 fps): levels 6.0-6.2.
    if rate <= 4_177_920 {
        60
    } else if rate <= 8_355_840 {
        61
    } else {
        62
    }
}

/// What goes into the SPS Snagster writes.
#[derive(Clone, Debug, PartialEq)]
pub struct SpsParams {
    pub profile: Profile,
    pub level_idc: u8,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// frame_num runs 0 .. 2^log2_max_frame_num (4..16).
    pub log2_max_frame_num: u32,
    pub max_num_ref_frames: u32,
}

impl SpsParams {
    pub fn width_mbs(&self) -> u32 {
        self.width.div_ceil(16)
    }

    pub fn height_mbs(&self) -> u32 {
        self.height.div_ceil(16)
    }

    /// Cropping (in 2-pixel units for 4:2:0) from the macroblock-aligned size to the real one.
    pub fn crop_right_bottom(&self) -> (u32, u32) {
        ((self.width_mbs() * 16 - self.width) / 2, (self.height_mbs() * 16 - self.height) / 2)
    }
}

/// An SPS with id 0: progressive frames, picture order count type 2 (no B-frames), BT.709 video
/// range colours, the frame rate, and "no frame reordering" so decoders show each frame at once.
pub fn write_sps(p: &SpsParams) -> Vec<u8> {
    let mut w = BitWriter::default();
    w.bits(8, p.profile.idc() as u32);
    w.bits(8, p.profile.constraints() as u32);
    w.bits(8, p.level_idc as u32);
    w.ue(0); // seq_parameter_set_id
    if p.profile == Profile::High {
        w.ue(1); // chroma_format_idc 4:2:0
        w.ue(0); // bit_depth_luma_minus8
        w.ue(0); // bit_depth_chroma_minus8
        w.bit(false); // qpprime_y_zero_transform_bypass_flag
        w.bit(false); // seq_scaling_matrix_present_flag
    }
    w.ue(p.log2_max_frame_num - 4);
    w.ue(2); // pic_order_cnt_type
    w.ue(p.max_num_ref_frames);
    w.bit(false); // gaps_in_frame_num_value_allowed_flag
    w.ue(p.width_mbs() - 1);
    w.ue(p.height_mbs() - 1);
    w.bit(true); // frame_mbs_only_flag
    w.bit(true); // direct_8x8_inference_flag
    let (cr, cb) = p.crop_right_bottom();
    w.bit(cr != 0 || cb != 0);
    if cr != 0 || cb != 0 {
        w.ue(0);
        w.ue(cr);
        w.ue(0);
        w.ue(cb);
    }
    w.bit(true); // vui_parameters_present_flag
    w.bit(false); // aspect_ratio_info_present_flag
    w.bit(false); // overscan_info_present_flag
    w.bit(true); // video_signal_type_present_flag
    w.bits(3, 5); // video_format: unspecified
    w.bit(false); // video_full_range_flag
    w.bit(true); // colour_description_present_flag
    w.bits(8, 1); // colour_primaries BT.709
    w.bits(8, 1); // transfer_characteristics BT.709
    w.bits(8, 1); // matrix_coefficients BT.709
    w.bit(false); // chroma_loc_info_present_flag
    w.bit(true); // timing_info_present_flag
    w.bits(32, 1); // num_units_in_tick
    w.bits(32, p.fps.max(1) * 2); // time_scale (two ticks per frame)
    w.bit(false); // fixed_frame_rate_flag
    w.bit(false); // nal_hrd_parameters_present_flag
    w.bit(false); // vcl_hrd_parameters_present_flag
    w.bit(false); // pic_struct_present_flag
    w.bit(true); // bitstream_restriction_flag
    w.bit(true); // motion_vectors_over_pic_boundaries_flag
    w.ue(2); // max_bytes_per_pic_denom
    w.ue(1); // max_bits_per_mb_denom
    w.ue(15); // log2_max_mv_length_horizontal
    w.ue(15); // log2_max_mv_length_vertical
    w.ue(0); // max_num_reorder_frames
    w.ue(p.max_num_ref_frames); // max_dec_frame_buffering
    nal(0x67, &w.finish())
}

/// A PPS with id 0 for the SPS above; QP 26 to start from.
pub fn write_pps(profile: Profile) -> Vec<u8> {
    let mut w = BitWriter::default();
    w.ue(0); // pic_parameter_set_id
    w.ue(0); // seq_parameter_set_id
    w.bit(profile.cabac()); // entropy_coding_mode_flag
    w.bit(false); // bottom_field_pic_order_in_frame_present_flag
    w.ue(0); // num_slice_groups_minus1
    w.ue(0); // num_ref_idx_l0_default_active_minus1
    w.ue(0); // num_ref_idx_l1_default_active_minus1
    w.bit(false); // weighted_pred_flag
    w.bits(2, 0); // weighted_bipred_idc
    w.se(0); // pic_init_qp_minus26
    w.se(0); // pic_init_qs_minus26
    w.se(0); // chroma_qp_index_offset
    w.bit(true); // deblocking_filter_control_present_flag
    w.bit(false); // constrained_intra_pred_flag
    w.bit(false); // redundant_pic_cnt_present_flag
    if profile.transform_8x8() {
        w.bit(true); // transform_8x8_mode_flag
        w.bit(false); // pic_scaling_matrix_present_flag
        w.se(0); // second_chroma_qp_index_offset
    }
    nal(0x68, &w.finish())
}

/// Reads bits most significant first.
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }

    pub fn bit(&mut self) -> Option<bool> {
        let b = *self.data.get(self.pos / 8)?;
        let v = (b >> (7 - self.pos % 8)) & 1 == 1;
        self.pos += 1;
        Some(v)
    }

    pub fn bits(&mut self, n: u32) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit()? as u32;
        }
        Some(v)
    }

    pub fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while !self.bit()? {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some(((1u64 << zeros) - 1 + self.bits(zeros)? as u64) as u32)
    }

    pub fn se(&mut self) -> Option<i32> {
        let k = self.ue()? as i64;
        Some(if k % 2 == 1 { ((k + 1) / 2) as i32 } else { -(k / 2) as i32 })
    }
}

/// What an SPS says (enough to check an encoder's output).
#[derive(Clone, Debug, PartialEq)]
pub struct SpsInfo {
    pub profile_idc: u8,
    pub level_idc: u8,
    pub width: u32,
    pub height: u32,
    pub poc_type: u32,
    pub max_num_ref_frames: u32,
}

/// Read an SPS NAL unit (header byte first, no start code).
pub fn parse_sps(nal: &[u8]) -> Option<SpsInfo> {
    if nal.first()? & 0x1f != 7 {
        return None;
    }
    let rbsp = unescape(&nal[1..]);
    let mut r = BitReader::new(&rbsp);
    let profile_idc = r.bits(8)? as u8;
    r.bits(8)?;
    let level_idc = r.bits(8)? as u8;
    r.ue()?;
    let mut chroma = 1;
    if matches!(profile_idc, 100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135) {
        chroma = r.ue()?;
        if chroma == 3 {
            r.bit()?;
        }
        r.ue()?;
        r.ue()?;
        r.bit()?;
        if r.bit()? {
            return None; // scaling matrices: not needed here
        }
    }
    r.ue()?; // log2_max_frame_num_minus4
    let poc_type = r.ue()?;
    if poc_type == 0 {
        r.ue()?;
    } else if poc_type == 1 {
        r.bit()?;
        r.se()?;
        r.se()?;
        for _ in 0..r.ue()? {
            r.se()?;
        }
    }
    let max_num_ref_frames = r.ue()?;
    r.bit()?;
    let wmbs = r.ue()? + 1;
    let hmaps = r.ue()? + 1;
    let frame_mbs_only = r.bit()?;
    if !frame_mbs_only {
        r.bit()?;
    }
    r.bit()?;
    let hmbs = if frame_mbs_only { hmaps } else { hmaps * 2 };
    let (mut cl, mut cr, mut ct, mut cb) = (0, 0, 0, 0);
    if r.bit()? {
        cl = r.ue()?;
        cr = r.ue()?;
        ct = r.ue()?;
        cb = r.ue()?;
    }
    let (ux, uy) = match chroma {
        0 | 3 => (1, if frame_mbs_only { 1 } else { 2 }),
        2 => (2, if frame_mbs_only { 1 } else { 2 }),
        _ => (2, if frame_mbs_only { 2 } else { 4 }),
    };
    let width = (wmbs * 16).checked_sub(ux * (cl + cr))?;
    let height = (hmbs * 16).checked_sub(uy * (ct + cb))?;
    Some(SpsInfo { profile_idc, level_idc, width, height, poc_type, max_num_ref_frames })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_golomb_codes_round_trip() {
        let mut w = BitWriter::default();
        for v in [0u32, 1, 2, 3, 7, 8, 254, 255, 65535, 1 << 20] {
            w.ue(v);
        }
        for v in [0i32, 1, -1, 2, -2, 100, -100] {
            w.se(v);
        }
        w.bits(5, 0b10110);
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes);
        for v in [0u32, 1, 2, 3, 7, 8, 254, 255, 65535, 1 << 20] {
            assert_eq!(r.ue(), Some(v));
        }
        for v in [0i32, 1, -1, 2, -2, 100, -100] {
            assert_eq!(r.se(), Some(v));
        }
        assert_eq!(r.bits(5), Some(0b10110));
        assert_eq!(r.bit(), Some(true), "the stop bit");
    }

    #[test]
    fn known_exp_golomb_bit_patterns() {
        // ue(0) = 1, ue(1) = 010, ue(2) = 011, ue(3) = 00100 → 1010 0110 0100 + stop bit 1, pad.
        let mut w = BitWriter::default();
        for v in 0..4 {
            w.ue(v);
        }
        assert_eq!(w.finish(), vec![0b1010_0110, 0b0100_1000]);
        // se(1) = 010, se(-1) = 011.
        let mut w = BitWriter::default();
        w.se(1);
        w.se(-1);
        assert_eq!(w.finish(), vec![0b0100_1110]);
    }

    #[test]
    fn emulation_prevention_bytes_go_in_and_come_out() {
        let raw = [0, 0, 1, 0, 0, 0, 0, 0, 3, 5, 0, 0];
        let n = nal(0x67, &raw);
        assert_eq!(&n[..5], &[0, 0, 0, 1, 0x67]);
        assert_eq!(&n[5..], &[0, 0, 3, 1, 0, 0, 3, 0, 0, 3, 0, 3, 5, 0, 0]);
        assert_eq!(unescape(&n[5..]), raw.to_vec());
        assert!(!n[5..].windows(3).any(|w| w[0] == 0 && w[1] == 0 && w[2] <= 2), "no start code inside");
    }

    #[test]
    fn levels_follow_the_picture_size_and_rate() {
        assert_eq!(level_for(640, 480, 30), 30);
        assert_eq!(level_for(1280, 720, 30), 31);
        assert_eq!(level_for(1280, 720, 60), 32);
        assert_eq!(level_for(1920, 1080, 30), 40);
        assert_eq!(level_for(1920, 1080, 60), 42);
        assert_eq!(level_for(2560, 1440, 60), 51);
        assert_eq!(level_for(3840, 2160, 30), 51);
        assert_eq!(level_for(3840, 2160, 60), 52);
        assert_eq!(level_for(7680, 4320, 30), 60);
        assert_eq!(level_for(176, 144, 15), 10);
    }

    fn params(profile: Profile, w: u32, h: u32) -> SpsParams {
        SpsParams { profile, level_idc: level_for(w, h, 30), width: w, height: h, fps: 30, log2_max_frame_num: 8, max_num_ref_frames: 1 }
    }

    #[test]
    fn written_sps_reads_back_with_its_size_and_cropping() {
        for profile in [Profile::High, Profile::Main, Profile::ConstrainedBaseline] {
            for (w, h) in [(1920, 1080), (1280, 720), (64, 64), (1366, 768), (322, 202)] {
                let p = params(profile, w, h);
                let sps = write_sps(&p);
                assert_eq!(&sps[..4], &[0, 0, 0, 1]);
                let info = parse_sps(&sps[4..]).unwrap_or_else(|| panic!("{profile:?} {w}x{h}"));
                assert_eq!((info.width, info.height), (w, h), "{profile:?}");
                assert_eq!(info.profile_idc, profile.idc());
                assert_eq!(info.level_idc, p.level_idc);
                assert_eq!((info.poc_type, info.max_num_ref_frames), (2, 1));
            }
        }
        assert_eq!(params(Profile::High, 1920, 1080).crop_right_bottom(), (0, 4), "1088 → 1080");
        assert_eq!(params(Profile::High, 1920, 1080).height_mbs(), 68);
    }

    #[test]
    fn the_sps_bytes_for_720p_high() {
        // Checked bit by bit against the syntax in H.264 7.3.2.1.1: profile 100, level 3.1, id 0,
        // chroma 1, 8-bit, frame_num 8 bits, POC type 2, 1 reference, 80 × 45 macroblocks.
        let sps = write_sps(&params(Profile::High, 1280, 720));
        assert_eq!(&sps[..8], &[0, 0, 0, 1, 0x67, 100, 0, 31]);
        assert_eq!(sps[8], 0b1010_1100, "ue(0) ue(1) ue(0) ue(0) 0 0 then ue(4) begins");
    }

    #[test]
    fn pps_fields_follow_the_profile() {
        let high = write_pps(Profile::High);
        let base = write_pps(Profile::ConstrainedBaseline);
        assert_eq!(&high[..5], &[0, 0, 0, 1, 0x68]);
        // ue(0) ue(0) then entropy_coding_mode_flag.
        let mut r = BitReader::new(&high[5..]);
        assert_eq!((r.ue(), r.ue(), r.bit()), (Some(0), Some(0), Some(true)));
        let mut r = BitReader::new(&base[5..]);
        assert_eq!((r.ue(), r.ue(), r.bit()), (Some(0), Some(0), Some(false)));
        // The rest of High's PPS: no slice groups or weighting, QP 26, deblocking control on,
        // then the 8 × 8 transform.
        let mut r = BitReader::new(&high[5..]);
        let head = (r.ue(), r.ue(), r.bit(), r.bit(), r.ue(), r.ue(), r.ue(), r.bit(), r.bits(2));
        assert_eq!(head, (Some(0), Some(0), Some(true), Some(false), Some(0), Some(0), Some(0), Some(false), Some(0)));
        assert_eq!((r.se(), r.se(), r.se()), (Some(0), Some(0), Some(0)));
        assert_eq!((r.bit(), r.bit(), r.bit()), (Some(true), Some(false), Some(false)));
        assert_eq!((r.bit(), r.bit(), r.se()), (Some(true), Some(false), Some(0)), "transform_8x8_mode_flag");
        assert_eq!(r.bit(), Some(true), "then the stop bit");
        assert_ne!(high, base);
    }

    #[test]
    fn broken_or_foreign_units_are_not_read_as_an_sps() {
        assert_eq!(parse_sps(&[]), None);
        assert_eq!(parse_sps(&[0x68, 1, 2]), None);
        assert_eq!(parse_sps(&[0x67, 100]), None, "cut short");
    }
}
