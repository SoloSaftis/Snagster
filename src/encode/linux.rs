//! Linux: H.264 on the graphics chip through VA-API (Intel's iHD and i965 drivers, AMD through
//! Mesa, NVIDIA where a VA-API driver is installed).
//!
//! libva is loaded while running (`libva.so.2`, `libva-drm.so.2`), not linked, so Snagster still
//! starts on a system without it and simply uses OpenH264 there. The structures below follow
//! libva's public headers (`va.h`, `va_enc_h264.h`, libva 2.x, MIT licence); Snagster writes its
//! own SPS and PPS (`h264.rs`) and lets the driver write the slice headers.
//!
//! Each picture: RGBA → NV12 into the input surface, then one IDR or P frame with one slice and
//! one reference picture, read back from the coded buffer straight after (no B-frames, no queue).

use super::h264::{self, Profile, SpsParams};
use super::*;
use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::fs::File;
use std::os::fd::AsRawFd;

type VADisplay = *mut c_void;
type VAStatus = c_int;
type VAId = c_uint;

const VA_INVALID_ID: VAId = 0xffff_ffff;
const VA_STATUS_SUCCESS: VAStatus = 0;

// VAProfile
const VA_PROFILE_H264_MAIN: c_int = 6;
const VA_PROFILE_H264_HIGH: c_int = 7;
const VA_PROFILE_H264_CONSTRAINED_BASELINE: c_int = 13;
// VAEntrypoint
const VA_ENTRYPOINT_ENC_SLICE: c_int = 6;
const VA_ENTRYPOINT_ENC_SLICE_LP: c_int = 8;
// VAConfigAttribType
const VA_CONFIG_ATTRIB_RT_FORMAT: c_int = 0;
const VA_CONFIG_ATTRIB_RATE_CONTROL: c_int = 5;
const VA_CONFIG_ATTRIB_ENC_PACKED_HEADERS: c_int = 10;
const VA_ATTRIB_NOT_SUPPORTED: u32 = 0x8000_0000;
const VA_RT_FORMAT_YUV420: u32 = 0x1;
const VA_RC_CBR: u32 = 0x2;
const VA_RC_VBR: u32 = 0x4;
const VA_RC_CQP: u32 = 0x10;
const VA_ENC_PACKED_HEADER_SEQUENCE: u32 = 0x1;
const VA_ENC_PACKED_HEADER_PICTURE: u32 = 0x2;
const VA_PROGRESSIVE: c_int = 0x1;
// VABufferType
const VA_ENC_CODED_BUFFER: c_int = 21;
const VA_ENC_SEQUENCE_PARAMETER_BUFFER: c_int = 22;
const VA_ENC_PICTURE_PARAMETER_BUFFER: c_int = 23;
const VA_ENC_SLICE_PARAMETER_BUFFER: c_int = 24;
const VA_ENC_PACKED_HEADER_PARAMETER_BUFFER: c_int = 25;
const VA_ENC_PACKED_HEADER_DATA_BUFFER: c_int = 26;
const VA_ENC_MISC_PARAMETER_BUFFER: c_int = 27;
// VAEncPackedHeaderType
const VA_ENC_PACKED_HEADER_TYPE_SEQUENCE: u32 = 1;
const VA_ENC_PACKED_HEADER_TYPE_PICTURE: u32 = 2;
// VAEncMiscParameterType
const VA_ENC_MISC_FRAME_RATE: u32 = 0;
const VA_ENC_MISC_RATE_CONTROL: u32 = 1;
const VA_ENC_MISC_HRD: u32 = 5;
// VAPictureH264 flags
const VA_PICTURE_H264_INVALID: u32 = 0x1;
const VA_PICTURE_H264_SHORT_TERM_REFERENCE: u32 = 0x8;
const NV12: u32 = u32::from_le_bytes(*b"NV12");

#[repr(C)]
#[derive(Clone, Copy)]
struct VAConfigAttrib {
    kind: c_int,
    value: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VAImageFormat {
    fourcc: u32,
    byte_order: u32,
    bits_per_pixel: u32,
    depth: u32,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
    alpha_mask: u32,
    va_reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VAImage {
    image_id: VAId,
    format: VAImageFormat,
    buf: VAId,
    width: u16,
    height: u16,
    data_size: u32,
    num_planes: u32,
    pitches: [u32; 3],
    offsets: [u32; 3],
    num_palette_entries: i32,
    entry_bytes: i32,
    component_order: [i8; 4],
    va_reserved: [u32; 4],
}

#[repr(C)]
struct VACodedBufferSegment {
    size: u32,
    bit_offset: u32,
    status: u32,
    reserved: u32,
    buf: *mut c_void,
    next: *mut c_void,
    va_reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VAPictureH264 {
    picture_id: VAId,
    frame_idx: u32,
    flags: u32,
    top_field_order_cnt: i32,
    bottom_field_order_cnt: i32,
    va_reserved: [u32; 4],
}

const NO_PICTURE: VAPictureH264 = VAPictureH264 { picture_id: VA_INVALID_ID, frame_idx: 0, flags: VA_PICTURE_H264_INVALID, top_field_order_cnt: 0, bottom_field_order_cnt: 0, va_reserved: [0; 4] };

#[repr(C)]
struct VAEncSequenceParameterBufferH264 {
    seq_parameter_set_id: u8,
    level_idc: u8,
    intra_period: u32,
    intra_idr_period: u32,
    ip_period: u32,
    bits_per_second: u32,
    max_num_ref_frames: u32,
    picture_width_in_mbs: u16,
    picture_height_in_mbs: u16,
    seq_fields: u32,
    bit_depth_luma_minus8: u8,
    bit_depth_chroma_minus8: u8,
    num_ref_frames_in_pic_order_cnt_cycle: u8,
    offset_for_non_ref_pic: i32,
    offset_for_top_to_bottom_field: i32,
    offset_for_ref_frame: [i32; 256],
    frame_cropping_flag: u8,
    frame_crop_left_offset: u32,
    frame_crop_right_offset: u32,
    frame_crop_top_offset: u32,
    frame_crop_bottom_offset: u32,
    vui_parameters_present_flag: u8,
    vui_fields: u32,
    aspect_ratio_idc: u8,
    sar_width: u32,
    sar_height: u32,
    num_units_in_tick: u32,
    time_scale: u32,
    va_reserved: [u32; 4],
}

#[repr(C)]
struct VAEncPictureParameterBufferH264 {
    curr_pic: VAPictureH264,
    reference_frames: [VAPictureH264; 16],
    coded_buf: VAId,
    pic_parameter_set_id: u8,
    seq_parameter_set_id: u8,
    last_picture: u8,
    frame_num: u16,
    pic_init_qp: u8,
    num_ref_idx_l0_active_minus1: u8,
    num_ref_idx_l1_active_minus1: u8,
    chroma_qp_index_offset: i8,
    second_chroma_qp_index_offset: i8,
    pic_fields: u32,
    va_reserved: [u32; 8],
}

#[repr(C)]
struct VAEncSliceParameterBufferH264 {
    macroblock_address: u32,
    num_macroblocks: u32,
    macroblock_info: VAId,
    slice_type: u8,
    pic_parameter_set_id: u8,
    idr_pic_id: u16,
    pic_order_cnt_lsb: u16,
    delta_pic_order_cnt_bottom: i32,
    delta_pic_order_cnt: [i32; 2],
    direct_spatial_mv_pred_flag: u8,
    num_ref_idx_active_override_flag: u8,
    num_ref_idx_l0_active_minus1: u8,
    num_ref_idx_l1_active_minus1: u8,
    ref_pic_list0: [VAPictureH264; 32],
    ref_pic_list1: [VAPictureH264; 32],
    luma_log2_weight_denom: u8,
    chroma_log2_weight_denom: u8,
    luma_weight_l0_flag: u8,
    luma_weight_l0: [i16; 32],
    luma_offset_l0: [i16; 32],
    chroma_weight_l0_flag: u8,
    chroma_weight_l0: [[i16; 2]; 32],
    chroma_offset_l0: [[i16; 2]; 32],
    luma_weight_l1_flag: u8,
    luma_weight_l1: [i16; 32],
    luma_offset_l1: [i16; 32],
    chroma_weight_l1_flag: u8,
    chroma_weight_l1: [[i16; 2]; 32],
    chroma_offset_l1: [[i16; 2]; 32],
    cabac_init_idc: u8,
    slice_qp_delta: i8,
    disable_deblocking_filter_idc: u8,
    slice_alpha_c0_offset_div2: i8,
    slice_beta_offset_div2: i8,
    va_reserved: [u32; 4],
}

#[repr(C)]
struct VAEncPackedHeaderParameterBuffer {
    kind: u32,
    bit_length: u32,
    has_emulation_bytes: u8,
    va_reserved: [u32; 4],
}

#[repr(C)]
struct VAEncMiscParameterRateControl {
    bits_per_second: u32,
    target_percentage: u32,
    window_size: u32,
    initial_qp: u32,
    min_qp: u32,
    basic_unit_size: u32,
    rc_flags: u32,
    icq_quality_factor: u32,
    max_qp: u32,
    quality_factor: u32,
    target_frame_size: u32,
    va_reserved: [u32; 4],
}

#[repr(C)]
struct VAEncMiscParameterFrameRate {
    framerate: u32,
    framerate_flags: u32,
    va_reserved: [u32; 4],
}

#[repr(C)]
struct VAEncMiscParameterHRD {
    initial_buffer_fullness: u32,
    buffer_size: u32,
    va_reserved: [u32; 4],
}

/// The packed bit fields of the sequence parameters (`seq_fields`), lowest bit first.
fn seq_fields(log2_max_frame_num: u32) -> u32 {
    1 // chroma_format_idc = 1 (bits 0-1)
        | 1 << 2 // frame_mbs_only_flag
        | 1 << 5 // direct_8x8_inference_flag
        | (log2_max_frame_num - 4) << 6 // log2_max_frame_num_minus4 (bits 6-9)
        | 2 << 10 // pic_order_cnt_type (bits 10-11)
}

/// `vui_fields`: timing info, bitstream restriction, 15-bit motion vectors, vectors over edges.
fn vui_fields() -> u32 {
    1 << 1 | 1 << 2 | 15 << 3 | 15 << 8 | 1 << 15
}

/// `pic_fields`: IDR, reference, CABAC and 8 × 8 transform as the profile has them, deblocking
/// control present.
fn pic_fields(idr: bool, profile: Profile) -> u32 {
    idr as u32 | 1 << 1 | (profile.cabac() as u32) << 3 | (profile.transform_8x8() as u32) << 8 | 1 << 9
}

/// Which rate control to ask for out of what the driver offers.
fn rate_control(offered: u32) -> Option<u32> {
    [VA_RC_CBR, VA_RC_VBR, VA_RC_CQP].into_iter().find(|m| offered != VA_ATTRIB_NOT_SUPPORTED && offered & m != 0)
}

/// The libva functions Snagster calls, loaded from the system's libva.
struct Va {
    get_display_drm: unsafe extern "C" fn(c_int) -> VADisplay,
    initialize: unsafe extern "C" fn(VADisplay, *mut c_int, *mut c_int) -> VAStatus,
    terminate: unsafe extern "C" fn(VADisplay) -> VAStatus,
    error_str: unsafe extern "C" fn(VAStatus) -> *const c_char,
    vendor: unsafe extern "C" fn(VADisplay) -> *const c_char,
    set_info_callback: Option<unsafe extern "C" fn(VADisplay, *mut c_void, *mut c_void) -> *mut c_void>,
    max_entrypoints: unsafe extern "C" fn(VADisplay) -> c_int,
    query_entrypoints: unsafe extern "C" fn(VADisplay, c_int, *mut c_int, *mut c_int) -> VAStatus,
    get_config_attributes: unsafe extern "C" fn(VADisplay, c_int, c_int, *mut VAConfigAttrib, c_int) -> VAStatus,
    create_config: unsafe extern "C" fn(VADisplay, c_int, c_int, *mut VAConfigAttrib, c_int, *mut VAId) -> VAStatus,
    destroy_config: unsafe extern "C" fn(VADisplay, VAId) -> VAStatus,
    create_surfaces: unsafe extern "C" fn(VADisplay, c_uint, c_uint, c_uint, *mut VAId, c_uint, *mut c_void, c_uint) -> VAStatus,
    destroy_surfaces: unsafe extern "C" fn(VADisplay, *mut VAId, c_int) -> VAStatus,
    create_context: unsafe extern "C" fn(VADisplay, VAId, c_int, c_int, c_int, *mut VAId, c_int, *mut VAId) -> VAStatus,
    destroy_context: unsafe extern "C" fn(VADisplay, VAId) -> VAStatus,
    create_buffer: unsafe extern "C" fn(VADisplay, VAId, c_int, c_uint, c_uint, *mut c_void, *mut VAId) -> VAStatus,
    destroy_buffer: unsafe extern "C" fn(VADisplay, VAId) -> VAStatus,
    map_buffer: unsafe extern "C" fn(VADisplay, VAId, *mut *mut c_void) -> VAStatus,
    unmap_buffer: unsafe extern "C" fn(VADisplay, VAId) -> VAStatus,
    begin_picture: unsafe extern "C" fn(VADisplay, VAId, VAId) -> VAStatus,
    render_picture: unsafe extern "C" fn(VADisplay, VAId, *mut VAId, c_int) -> VAStatus,
    end_picture: unsafe extern "C" fn(VADisplay, VAId) -> VAStatus,
    sync_surface: unsafe extern "C" fn(VADisplay, VAId) -> VAStatus,
    derive_image: unsafe extern "C" fn(VADisplay, VAId, *mut VAImage) -> VAStatus,
    create_image: unsafe extern "C" fn(VADisplay, *mut VAImageFormat, c_int, c_int, *mut VAImage) -> VAStatus,
    put_image: unsafe extern "C" fn(VADisplay, VAId, VAId, c_int, c_int, c_uint, c_uint, c_int, c_int, c_uint, c_uint) -> VAStatus,
    destroy_image: unsafe extern "C" fn(VADisplay, VAId) -> VAStatus,
    _va: libloading::Library,
    _drm: libloading::Library,
}

impl Va {
    fn load() -> Result<Va, String> {
        unsafe {
            let va = libloading::Library::new("libva.so.2").map_err(|_| "libva isn't installed".to_string())?;
            let drm = libloading::Library::new("libva-drm.so.2").map_err(|_| "libva-drm isn't installed".to_string())?;
            macro_rules! sym {
                ($lib:expr, $name:literal) => {
                    *$lib.get(concat!($name, "\0").as_bytes()).map_err(|e| format!("libva has no {}: {e}", $name))?
                };
            }
            Ok(Va {
                get_display_drm: sym!(drm, "vaGetDisplayDRM"),
                initialize: sym!(va, "vaInitialize"),
                terminate: sym!(va, "vaTerminate"),
                error_str: sym!(va, "vaErrorStr"),
                vendor: sym!(va, "vaQueryVendorString"),
                set_info_callback: va.get(b"vaSetInfoCallback\0").ok().map(|s| *s),
                max_entrypoints: sym!(va, "vaMaxNumEntrypoints"),
                query_entrypoints: sym!(va, "vaQueryConfigEntrypoints"),
                get_config_attributes: sym!(va, "vaGetConfigAttributes"),
                create_config: sym!(va, "vaCreateConfig"),
                destroy_config: sym!(va, "vaDestroyConfig"),
                create_surfaces: sym!(va, "vaCreateSurfaces"),
                destroy_surfaces: sym!(va, "vaDestroySurfaces"),
                create_context: sym!(va, "vaCreateContext"),
                destroy_context: sym!(va, "vaDestroyContext"),
                create_buffer: sym!(va, "vaCreateBuffer"),
                destroy_buffer: sym!(va, "vaDestroyBuffer"),
                map_buffer: sym!(va, "vaMapBuffer"),
                unmap_buffer: sym!(va, "vaUnmapBuffer"),
                begin_picture: sym!(va, "vaBeginPicture"),
                render_picture: sym!(va, "vaRenderPicture"),
                end_picture: sym!(va, "vaEndPicture"),
                sync_surface: sym!(va, "vaSyncSurface"),
                derive_image: sym!(va, "vaDeriveImage"),
                create_image: sym!(va, "vaCreateImage"),
                put_image: sym!(va, "vaPutImage"),
                destroy_image: sym!(va, "vaDestroyImage"),
                _va: va,
                _drm: drm,
            })
        }
    }

    fn check(&self, what: &str, s: VAStatus) -> Result<(), String> {
        if s == VA_STATUS_SUCCESS {
            return Ok(());
        }
        let msg = unsafe {
            let p = (self.error_str)(s);
            if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
        };
        Err(format!("{what} failed: {msg} ({s})"))
    }
}

/// An initialised VA display on one render node.
struct Display {
    va: Va,
    dpy: VADisplay,
    vendor: String,
    _node: File,
}

impl Display {
    fn open() -> Result<Display, String> {
        let va = Va::load()?;
        let mut last = "no graphics render node (/dev/dri/renderD*)".to_string();
        for n in 128..136 {
            let path = format!("/dev/dri/renderD{n}");
            let Ok(node) = std::fs::OpenOptions::new().read(true).write(true).open(&path) else { continue };
            unsafe {
                let dpy = (va.get_display_drm)(node.as_raw_fd());
                if dpy.is_null() {
                    continue;
                }
                if let Some(f) = va.set_info_callback {
                    // Keep libva's "VA-API version ..." lines off the terminal.
                    f(dpy, std::ptr::null_mut(), std::ptr::null_mut());
                }
                let (mut major, mut minor) = (0, 0);
                let s = (va.initialize)(dpy, &mut major, &mut minor);
                if s != VA_STATUS_SUCCESS {
                    last = va.check(&format!("vaInitialize on {path}"), s).unwrap_err();
                    (va.terminate)(dpy);
                    continue;
                }
                let v = (va.vendor)(dpy);
                let vendor = if v.is_null() { "VA-API".into() } else { CStr::from_ptr(v).to_string_lossy().trim().to_string() };
                return Ok(Display { va, dpy, vendor, _node: node });
            }
        }
        Err(last)
    }

    /// The best H.264 profile with an encoding entry point: (profile, VA profile, entry point).
    fn h264_encoder(&self) -> Option<(Profile, c_int, c_int)> {
        unsafe {
            let max = (self.va.max_entrypoints)(self.dpy).max(1) as usize;
            for (p, vp) in [(Profile::High, VA_PROFILE_H264_HIGH), (Profile::Main, VA_PROFILE_H264_MAIN), (Profile::ConstrainedBaseline, VA_PROFILE_H264_CONSTRAINED_BASELINE)] {
                let mut list = vec![0 as c_int; max];
                let mut n = 0;
                if (self.va.query_entrypoints)(self.dpy, vp, list.as_mut_ptr(), &mut n) != VA_STATUS_SUCCESS {
                    continue;
                }
                let list = &list[..(n.max(0) as usize).min(max)];
                for e in [VA_ENTRYPOINT_ENC_SLICE, VA_ENTRYPOINT_ENC_SLICE_LP] {
                    if list.contains(&e) {
                        return Some((p, vp, e));
                    }
                }
            }
            None
        }
    }

    fn name(&self) -> String {
        format!("VA-API H.264 ({})", self.vendor)
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        unsafe {
            (self.va.terminate)(self.dpy);
        }
    }
}

pub fn list() -> Vec<(Codec, EncoderName)> {
    match Display::open() {
        Ok(d) if d.h264_encoder().is_some() => vec![(Codec::H264, EncoderName { name: d.name(), hardware: true })],
        _ => Vec::new(),
    }
}

pub fn open_video(cfg: &VideoConfig) -> Result<Box<dyn VideoEncoder>, String> {
    if cfg.codec != Codec::H264 {
        return Err("VA-API encoding here makes H.264 only".into());
    }
    VaEncoder::open(cfg).map(|e| Box::new(e) as Box<dyn VideoEncoder>)
}

struct VaEncoder {
    d: Display,
    cfg: VideoConfig,
    profile: Profile,
    config: VAId,
    context: VAId,
    /// The picture going in, then two reconstructed pictures used in turn as the reference.
    input: VAId,
    recon: [VAId; 2],
    coded: VAId,
    rc: u32,
    packed: u32,
    sps: Vec<u8>,
    pps: Vec<u8>,
    /// Frames since the last IDR, and how many IDRs so far.
    since_idr: u32,
    idr_count: u32,
    cur: usize,
    name: String,
}

// The VA display and its objects are used by one thread at a time (`&mut self`); libva allows
// calls from any thread.
unsafe impl Send for VaEncoder {}

const LOG2_MAX_FRAME_NUM: u32 = 8;

impl VaEncoder {
    fn open(cfg: &VideoConfig) -> Result<VaEncoder, String> {
        let d = Display::open()?;
        let (profile, vprofile, entry) = d.h264_encoder().ok_or_else(|| format!("{} can't encode H.264", d.vendor))?;
        let va = &d.va;
        unsafe {
            let mut attrs = [
                VAConfigAttrib { kind: VA_CONFIG_ATTRIB_RT_FORMAT, value: 0 },
                VAConfigAttrib { kind: VA_CONFIG_ATTRIB_RATE_CONTROL, value: 0 },
                VAConfigAttrib { kind: VA_CONFIG_ATTRIB_ENC_PACKED_HEADERS, value: 0 },
            ];
            va.check("vaGetConfigAttributes", (va.get_config_attributes)(d.dpy, vprofile, entry, attrs.as_mut_ptr(), 3))?;
            if attrs[0].value == VA_ATTRIB_NOT_SUPPORTED || attrs[0].value & VA_RT_FORMAT_YUV420 == 0 {
                return Err("the driver can't encode 4:2:0 pictures".into());
            }
            let rc = rate_control(attrs[1].value).ok_or("the driver offers no rate control Snagster knows")?;
            let want = VA_ENC_PACKED_HEADER_SEQUENCE | VA_ENC_PACKED_HEADER_PICTURE;
            let packed = if attrs[2].value != VA_ATTRIB_NOT_SUPPORTED && attrs[2].value & want == want { want } else { 0 };
            let mut chosen = vec![VAConfigAttrib { kind: VA_CONFIG_ATTRIB_RT_FORMAT, value: VA_RT_FORMAT_YUV420 }, VAConfigAttrib { kind: VA_CONFIG_ATTRIB_RATE_CONTROL, value: rc }];
            if packed != 0 {
                chosen.push(VAConfigAttrib { kind: VA_CONFIG_ATTRIB_ENC_PACKED_HEADERS, value: packed });
            }
            let mut config = VA_INVALID_ID;
            va.check("vaCreateConfig", (va.create_config)(d.dpy, vprofile, entry, chosen.as_mut_ptr(), chosen.len() as c_int, &mut config))?;
            let mut e = VaEncoder {
                cfg: cfg.clone(),
                profile,
                config,
                context: VA_INVALID_ID,
                input: VA_INVALID_ID,
                recon: [VA_INVALID_ID; 2],
                coded: VA_INVALID_ID,
                rc,
                packed,
                sps: Vec::new(),
                pps: Vec::new(),
                since_idr: 0,
                idr_count: 0,
                cur: 0,
                name: d.name(),
                d,
            };
            // From here on `Drop` frees whatever was made.
            let va = &e.d.va;
            let mut surfaces = [VA_INVALID_ID; 3];
            va.check("vaCreateSurfaces", (va.create_surfaces)(e.d.dpy, VA_RT_FORMAT_YUV420, cfg.width, cfg.height, surfaces.as_mut_ptr(), 3, std::ptr::null_mut(), 0))?;
            e.input = surfaces[0];
            e.recon = [surfaces[1], surfaces[2]];
            va.check("vaCreateContext", (va.create_context)(e.d.dpy, e.config, cfg.width as c_int, cfg.height as c_int, VA_PROGRESSIVE, surfaces.as_mut_ptr(), 3, &mut e.context))?;
            let coded_size = (cfg.width * cfg.height * 3 / 2).max(1 << 16) + (1 << 16);
            va.check("vaCreateBuffer (coded)", (va.create_buffer)(e.d.dpy, e.context, VA_ENC_CODED_BUFFER, coded_size, 1, std::ptr::null_mut(), &mut e.coded))?;
            let sp = e.sps_params();
            e.sps = h264::write_sps(&sp);
            e.pps = h264::write_pps(profile);
            // A check on Snagster's own SPS writer: it must read back as what was asked for.
            let info = h264::parse_sps(&e.sps[4..]).ok_or("Snagster's SPS doesn't read back")?;
            let expect = (cfg.width, cfg.height, profile.idc(), sp.level_idc, 2, 1);
            if (info.width, info.height, info.profile_idc, info.level_idc, info.poc_type, info.max_num_ref_frames) != expect {
                return Err("Snagster's SPS reads back wrong".into());
            }
            Ok(e)
        }
    }

    fn sps_params(&self) -> SpsParams {
        let c = &self.cfg;
        SpsParams { profile: self.profile, level_idc: h264::level_for(c.width, c.height, c.fps), width: c.width, height: c.height, fps: c.fps, log2_max_frame_num: LOG2_MAX_FRAME_NUM, max_num_ref_frames: 1 }
    }

    /// A parameter buffer holding `value`'s bytes.
    fn buffer<T>(&self, kind: c_int, value: &T) -> Result<VAId, String> {
        self.bytes_buffer(kind, unsafe { std::slice::from_raw_parts(value as *const T as *const u8, std::mem::size_of::<T>()) })
    }

    fn bytes_buffer(&self, kind: c_int, bytes: &[u8]) -> Result<VAId, String> {
        let va = &self.d.va;
        let mut id = VA_INVALID_ID;
        let s = unsafe { (va.create_buffer)(self.d.dpy, self.context, kind, bytes.len() as c_uint, 1, bytes.as_ptr() as *mut c_void, &mut id) };
        va.check("vaCreateBuffer", s)?;
        Ok(id)
    }

    /// A misc parameter buffer: its type, then the structure.
    fn misc<T>(&self, kind: u32, value: &T) -> Result<VAId, String> {
        let mut b = kind.to_ne_bytes().to_vec();
        b.extend_from_slice(unsafe { std::slice::from_raw_parts(value as *const T as *const u8, std::mem::size_of::<T>()) });
        self.bytes_buffer(VA_ENC_MISC_PARAMETER_BUFFER, &b)
    }

    fn upload(&mut self, rgba: &[u8]) -> Result<(), String> {
        let (w, h) = (self.cfg.width, self.cfg.height);
        let va = &self.d.va;
        unsafe {
            let mut img: VAImage = std::mem::zeroed();
            let derived = (va.derive_image)(self.d.dpy, self.input, &mut img) == VA_STATUS_SUCCESS && img.format.fourcc == NV12;
            if !derived {
                if img.image_id != 0 && img.image_id != VA_INVALID_ID {
                    (va.destroy_image)(self.d.dpy, img.image_id);
                }
                let mut f: VAImageFormat = std::mem::zeroed();
                f.fourcc = NV12;
                f.byte_order = 1;
                f.bits_per_pixel = 12;
                img = std::mem::zeroed();
                va.check("vaCreateImage", (va.create_image)(self.d.dpy, &mut f, w as c_int, h as c_int, &mut img))?;
            }
            let mut p = std::ptr::null_mut();
            let r = va.check("vaMapBuffer (picture)", (va.map_buffer)(self.d.dpy, img.buf, &mut p));
            if r.is_ok() {
                let base = p as *mut u8;
                let ys = img.pitches[0] as usize;
                let uvs = img.pitches[1] as usize;
                let y = std::slice::from_raw_parts_mut(base.add(img.offsets[0] as usize), ys * h as usize);
                let uv = std::slice::from_raw_parts_mut(base.add(img.offsets[1] as usize), uvs * h as usize / 2);
                rgba_to_nv12(rgba, w, h, y, ys, uv, uvs);
                (va.unmap_buffer)(self.d.dpy, img.buf);
            }
            let put = if r.is_ok() && !derived {
                va.check("vaPutImage", (va.put_image)(self.d.dpy, self.input, img.image_id, 0, 0, w, h, 0, 0, w, h))
            } else {
                Ok(())
            };
            (va.destroy_image)(self.d.dpy, img.image_id);
            r.and(put)
        }
    }

    fn encode_frame(&mut self, idr: bool) -> Result<Vec<u8>, String> {
        let c = self.cfg.clone();
        if idr {
            self.since_idr = 0;
        }
        let frame_num = self.since_idr % (1 << LOG2_MAX_FRAME_NUM);
        let poc = (2 * self.since_idr) as i32;
        let cur = self.recon[self.cur];
        let prev = self.recon[1 - self.cur];
        let mut bufs = Vec::new();
        let r = (|| -> Result<(), String> {
            unsafe {
                if idr {
                    let sp = self.sps_params();
                    let mut seq: Box<VAEncSequenceParameterBufferH264> = Box::new(std::mem::zeroed());
                    seq.level_idc = sp.level_idc;
                    seq.intra_period = c.gop_frames();
                    seq.intra_idr_period = c.gop_frames();
                    seq.ip_period = 1;
                    seq.bits_per_second = if self.rc == VA_RC_CQP { 0 } else { c.bitrate_bps };
                    seq.max_num_ref_frames = 1;
                    seq.picture_width_in_mbs = sp.width_mbs() as u16;
                    seq.picture_height_in_mbs = sp.height_mbs() as u16;
                    seq.seq_fields = seq_fields(LOG2_MAX_FRAME_NUM);
                    let (cr, cb) = sp.crop_right_bottom();
                    seq.frame_cropping_flag = (cr != 0 || cb != 0) as u8;
                    seq.frame_crop_right_offset = cr;
                    seq.frame_crop_bottom_offset = cb;
                    seq.vui_parameters_present_flag = 1;
                    seq.vui_fields = vui_fields();
                    seq.num_units_in_tick = 1;
                    seq.time_scale = c.fps * 2;
                    bufs.push(self.buffer(VA_ENC_SEQUENCE_PARAMETER_BUFFER, &*seq)?);
                    if self.rc != VA_RC_CQP {
                        let mut rc: VAEncMiscParameterRateControl = std::mem::zeroed();
                        rc.bits_per_second = c.bitrate_bps;
                        rc.target_percentage = if self.rc == VA_RC_CBR { 100 } else { 90 };
                        rc.window_size = 1000;
                        rc.initial_qp = 26;
                        bufs.push(self.misc(VA_ENC_MISC_RATE_CONTROL, &rc)?);
                        let hrd = VAEncMiscParameterHRD { initial_buffer_fullness: c.bitrate_bps / 2, buffer_size: c.bitrate_bps, va_reserved: [0; 4] };
                        bufs.push(self.misc(VA_ENC_MISC_HRD, &hrd)?);
                    }
                    let fr = VAEncMiscParameterFrameRate { framerate: c.fps, framerate_flags: 0, va_reserved: [0; 4] };
                    bufs.push(self.misc(VA_ENC_MISC_FRAME_RATE, &fr)?);
                    if self.packed != 0 {
                        for (kind, data) in [(VA_ENC_PACKED_HEADER_TYPE_SEQUENCE, &self.sps), (VA_ENC_PACKED_HEADER_TYPE_PICTURE, &self.pps)] {
                            let p = VAEncPackedHeaderParameterBuffer { kind, bit_length: data.len() as u32 * 8, has_emulation_bytes: 1, va_reserved: [0; 4] };
                            bufs.push(self.buffer(VA_ENC_PACKED_HEADER_PARAMETER_BUFFER, &p)?);
                            bufs.push(self.bytes_buffer(VA_ENC_PACKED_HEADER_DATA_BUFFER, data)?);
                        }
                    }
                }
                let mut pic: Box<VAEncPictureParameterBufferH264> = Box::new(std::mem::zeroed());
                pic.curr_pic = VAPictureH264 { picture_id: cur, frame_idx: frame_num, flags: 0, top_field_order_cnt: poc, bottom_field_order_cnt: poc, va_reserved: [0; 4] };
                pic.reference_frames = [NO_PICTURE; 16];
                let reference = VAPictureH264 {
                    picture_id: prev,
                    frame_idx: (frame_num + (1 << LOG2_MAX_FRAME_NUM) - 1) % (1 << LOG2_MAX_FRAME_NUM),
                    flags: VA_PICTURE_H264_SHORT_TERM_REFERENCE,
                    top_field_order_cnt: poc - 2,
                    bottom_field_order_cnt: poc - 2,
                    va_reserved: [0; 4],
                };
                if !idr {
                    pic.reference_frames[0] = reference;
                }
                pic.coded_buf = self.coded;
                pic.frame_num = frame_num as u16;
                pic.pic_init_qp = 26;
                pic.pic_fields = pic_fields(idr, self.profile);
                bufs.push(self.buffer(VA_ENC_PICTURE_PARAMETER_BUFFER, &*pic)?);

                let mut sl: Box<VAEncSliceParameterBufferH264> = Box::new(std::mem::zeroed());
                let sp = self.sps_params();
                sl.num_macroblocks = sp.width_mbs() * sp.height_mbs();
                sl.macroblock_info = VA_INVALID_ID;
                sl.slice_type = if idr { 2 } else { 0 };
                sl.idr_pic_id = (self.idr_count % 65536) as u16;
                sl.ref_pic_list0 = [NO_PICTURE; 32];
                sl.ref_pic_list1 = [NO_PICTURE; 32];
                if !idr {
                    sl.ref_pic_list0[0] = reference;
                }
                bufs.push(self.buffer(VA_ENC_SLICE_PARAMETER_BUFFER, &*sl)?);

                let va = &self.d.va;
                va.check("vaBeginPicture", (va.begin_picture)(self.d.dpy, self.context, self.input))?;
                let s = (va.render_picture)(self.d.dpy, self.context, bufs.as_mut_ptr(), bufs.len() as c_int);
                let e = (va.end_picture)(self.d.dpy, self.context);
                va.check("vaRenderPicture", s)?;
                va.check("vaEndPicture", e)?;
                va.check("vaSyncSurface", (va.sync_surface)(self.d.dpy, self.input))?;
                Ok(())
            }
        })();
        let va = &self.d.va;
        for b in bufs {
            unsafe {
                (va.destroy_buffer)(self.d.dpy, b);
            }
        }
        r?;
        let data = self.read_coded()?;
        if idr {
            self.idr_count += 1;
        }
        self.since_idr += 1;
        self.cur = 1 - self.cur;
        Ok(data)
    }

    fn read_coded(&self) -> Result<Vec<u8>, String> {
        let va = &self.d.va;
        let mut out = Vec::new();
        unsafe {
            let mut p = std::ptr::null_mut();
            va.check("vaMapBuffer (coded)", (va.map_buffer)(self.d.dpy, self.coded, &mut p))?;
            let mut seg = p as *const VACodedBufferSegment;
            let mut guard = 0;
            while !seg.is_null() && guard < 64 {
                let s = &*seg;
                if !s.buf.is_null() && s.size > 0 {
                    out.extend_from_slice(std::slice::from_raw_parts(s.buf as *const u8, s.size as usize));
                }
                seg = s.next as *const VACodedBufferSegment;
                guard += 1;
            }
            (va.unmap_buffer)(self.d.dpy, self.coded);
        }
        Ok(out)
    }
}

impl VideoEncoder for VaEncoder {
    fn encode(&mut self, rgba: &[u8], pts_us: i64, force_keyframe: bool) -> Result<Vec<VideoPacket>, String> {
        self.upload(rgba)?;
        let idr = force_keyframe || self.since_idr == 0 || self.since_idr >= self.cfg.gop_frames();
        let mut data = self.encode_frame(idr)?;
        if data.is_empty() {
            return Ok(Vec::new());
        }
        if idr && sps_pps(&data).is_none() {
            // The driver wrote no parameter sets: put Snagster's in front.
            let mut d = self.sps.clone();
            d.extend_from_slice(&self.pps);
            d.extend_from_slice(&data);
            data = d;
        }
        Ok(vec![VideoPacket { data, pts_us, dts_us: pts_us, keyframe: idr }])
    }

    fn finish(&mut self) -> Result<Vec<VideoPacket>, String> {
        // Every frame is read back as soon as it's encoded.
        Ok(Vec::new())
    }

    fn name(&self) -> EncoderName {
        EncoderName { name: self.name.clone(), hardware: true }
    }
}

impl Drop for VaEncoder {
    fn drop(&mut self) {
        let va = &self.d.va;
        unsafe {
            if self.coded != VA_INVALID_ID {
                (va.destroy_buffer)(self.d.dpy, self.coded);
            }
            if self.context != VA_INVALID_ID {
                (va.destroy_context)(self.d.dpy, self.context);
            }
            let mut s = [self.input, self.recon[0], self.recon[1]];
            if s[0] != VA_INVALID_ID {
                (va.destroy_surfaces)(self.d.dpy, s.as_mut_ptr(), 3);
            }
            (va.destroy_config)(self.d.dpy, self.config);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structures_have_libvas_sizes() {
        // Sizes from libva 2.x on 64-bit Linux (sizeof in C).
        assert_eq!(std::mem::size_of::<VAPictureH264>(), 36);
        assert_eq!(std::mem::size_of::<VAImageFormat>(), 48);
        assert_eq!(std::mem::size_of::<VAImage>(), 120);
        assert_eq!(std::mem::size_of::<VACodedBufferSegment>(), 48);
        assert_eq!(std::mem::size_of::<VAEncSequenceParameterBufferH264>(), 1132);
        assert_eq!(std::mem::size_of::<VAEncPictureParameterBufferH264>(), 664);
        assert_eq!(std::mem::size_of::<VAEncSliceParameterBufferH264>(), 3140);
        assert_eq!(std::mem::size_of::<VAEncPackedHeaderParameterBuffer>(), 28);
        assert_eq!(std::mem::size_of::<VAEncMiscParameterRateControl>(), 60);
    }

    #[test]
    fn bit_fields_are_packed_lowest_first() {
        assert_eq!(seq_fields(8), 0b10_0100_100101);
        assert_eq!(pic_fields(true, Profile::High), 0b11_0000_1011);
        assert_eq!(pic_fields(false, Profile::ConstrainedBaseline), 0b10_0000_0010);
        assert_eq!(vui_fields() & 0b111, 0b110, "timing info and bitstream restriction, no aspect ratio");
    }

    #[test]
    fn rate_control_prefers_constant_bit_rate() {
        assert_eq!(rate_control(VA_RC_CBR | VA_RC_VBR | VA_RC_CQP), Some(VA_RC_CBR));
        assert_eq!(rate_control(VA_RC_VBR | VA_RC_CQP), Some(VA_RC_VBR));
        assert_eq!(rate_control(VA_RC_CQP), Some(VA_RC_CQP));
        assert_eq!(rate_control(0), None);
        assert_eq!(rate_control(VA_ATTRIB_NOT_SUPPORTED), None);
    }

    /// Where libva is installed (CI installs it), every function Snagster calls must be found.
    #[test]
    fn libva_has_every_function_snagster_calls() {
        match Va::load() {
            Ok(va) => assert!(va.set_info_callback.is_some(), "libva 2 has vaSetInfoCallback"),
            Err(e) => {
                assert!(e.contains("isn't installed"), "libva is there but: {e}");
                println!("libva check skipped: {e}");
            }
        }
    }

    #[test]
    fn without_libva_or_a_graphics_chip_it_says_why() {
        let c = VideoConfig { width: 64, height: 64, fps: 30, bitrate_bps: 1_000_000, keyframe_every_s: 1, codec: Codec::H264, prefer_hardware: true, low_latency: true };
        match open_video(&c) {
            Ok(e) => println!("VA-API opened: {}", e.name().name),
            Err(e) => {
                println!("VA-API skipped: {e}");
                assert!(!e.is_empty());
            }
        }
    }
}
