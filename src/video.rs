//! Screen video recording: MP4 (H.264 or HEVC), WMV, at 1-120 frames per second.
//!
//! Windows: the screen is copied on the graphics chip with DXGI Desktop Duplication and handed,
//! still on the graphics chip, to Media Foundation's sink writer with hardware transforms on.
//! Windows then uses the graphics vendor's own encoder (Intel Quick Sync, AMD AMF/VCN or NVIDIA
//! NVENC, each installed with the graphics driver), so recording costs very little processor
//! time. When that path isn't available it falls back to copying the screen with GDI, and then
//! to Windows' software encoder.
//!
//! Linux and macOS: the screen is grabbed with xcap and encoded with OpenH264 into an MP4.
//!
//! Written from the Windows SDK documentation; no code was taken from other recorders.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// What a recording is saved as.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum VideoFormat {
    /// MP4 with H.264: plays everywhere.
    #[default]
    Mp4,
    /// MP4 with HEVC (H.265): about half the size, needs a newer player.
    Hevc,
    /// Windows Media Video.
    Wmv,
    /// Animated GIF (Snagster's own small-file recorder).
    Gif,
}

impl VideoFormat {
    pub const ALL: [VideoFormat; 4] = [VideoFormat::Mp4, VideoFormat::Hevc, VideoFormat::Wmv, VideoFormat::Gif];

    pub fn label(self) -> &'static str {
        match self {
            VideoFormat::Mp4 => "MP4 video (H.264)",
            VideoFormat::Hevc => "MP4 video (HEVC / H.265)",
            VideoFormat::Wmv => "WMV video",
            VideoFormat::Gif => "Animated GIF",
        }
    }

    pub fn ext(self) -> &'static str {
        match self {
            VideoFormat::Mp4 | VideoFormat::Hevc => "mp4",
            VideoFormat::Wmv => "wmv",
            VideoFormat::Gif => "gif",
        }
    }

    /// Whether this computer's Snagster can make it at all.
    pub fn available(self) -> bool {
        cfg!(windows) || matches!(self, VideoFormat::Mp4 | VideoFormat::Gif)
    }

    pub fn parse(s: &str) -> Option<VideoFormat> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mp4" | "h264" | "h.264" | "avc" => Some(VideoFormat::Mp4),
            "hevc" | "h265" | "h.265" | "mp4-hevc" => Some(VideoFormat::Hevc),
            "wmv" => Some(VideoFormat::Wmv),
            "gif" => Some(VideoFormat::Gif),
            _ => None,
        }
    }
}

/// Everything one video recording needs.
#[derive(Clone, Debug)]
pub struct VideoOpts {
    /// Desktop area in physical pixels: left, top, width, height.
    pub rect: (i32, i32, u32, u32),
    /// Frames per second, 1-120.
    pub fps: u32,
    pub format: VideoFormat,
    /// 1 small file ... 4 best picture.
    pub quality: u8,
    /// Use the graphics chip's encoder when there is one.
    pub hardware: bool,
    /// Draw the mouse pointer.
    pub cursor: bool,
    /// Stop by itself after this many seconds (0: only when told).
    pub max_s: u32,
    pub path: PathBuf,
}

/// How a recording went.
#[derive(Clone, Debug)]
pub struct VideoDone {
    pub path: PathBuf,
    pub frames: u64,
    pub seconds: f32,
    /// The encoder that made it, e.g. "Intel® Quick Sync Video H.264 Encoder MFT".
    pub encoder: String,
    /// Whether that encoder runs on the graphics chip.
    pub hardware: bool,
    /// How the screen was copied ("graphics chip" or "GDI").
    pub capture: &'static str,
    pub width: u32,
    pub height: u32,
    pub result: Result<(), String>,
}

/// A video encoder this computer offers.
#[derive(Clone, Debug, Serialize)]
pub struct EncoderInfo {
    pub name: String,
    pub codec: &'static str,
    pub hardware: bool,
    pub vendor: String,
}

/// What a saved video holds, read back from the file.
#[derive(Clone, Debug)]
pub struct Probe {
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    pub seconds: f64,
    pub first: Option<image::RgbaImage>,
}

/// Bits per second for an area, frame rate, codec and quality step.
pub fn bitrate(w: u32, h: u32, fps: u32, format: VideoFormat, quality: u8) -> u32 {
    let bpp = match quality.clamp(1, 4) {
        1 => 0.04,
        2 => 0.08,
        3 => 0.14,
        _ => 0.24,
    };
    let px = w as f64 * h as f64 * bpp;
    // Motion between frames shrinks as the rate goes up, so later frames cost less.
    let fps = fps.clamp(1, 120) as f64;
    let rate = px * fps.min(30.0) + px * 0.4 * (fps - 30.0).max(0.0);
    let codec = if format == VideoFormat::Hevc { 0.6 } else { 1.0 };
    (rate * codec).clamp(250_000.0, 160_000_000.0) as u32
}

/// Video sizes must be even; tiny areas are refused.
pub fn even_size(w: u32, h: u32) -> Result<(u32, u32), String> {
    let (w, h) = (w & !1, h & !1);
    if w < 64 || h < 64 {
        return Err("Pick an area at least 64 × 64 pixels for a video.".into());
    }
    Ok((w.min(8192), h.min(8192)))
}

/// Record on a background thread; `done` gets the result. `pause` holds the clock while set.
pub fn spawn(opts: VideoOpts, stop: Arc<AtomicBool>, pause: Arc<AtomicBool>, ctx: Option<eframe::egui::Context>, done: Sender<VideoDone>) {
    std::thread::spawn(move || {
        let r = record(&opts, &stop, &pause);
        let _ = done.send(r);
        if let Some(c) = ctx {
            c.request_repaint();
        }
    });
}

/// Record until `stop` (or `max_s`), blocking this thread.
pub fn record(opts: &VideoOpts, stop: &AtomicBool, pause: &AtomicBool) -> VideoDone {
    let mut done = VideoDone {
        path: opts.path.clone(),
        frames: 0,
        seconds: 0.0,
        encoder: String::new(),
        hardware: false,
        capture: "",
        width: 0,
        height: 0,
        result: Ok(()),
    };
    if opts.format == VideoFormat::Gif {
        done.result = Err("GIFs are made by the GIF recorder.".into());
        return done;
    }
    if !opts.format.available() {
        done.result = Err(format!("{} can't be made on this system; pick MP4.", opts.format.label()));
        return done;
    }
    if let Some(dir) = opts.path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        done.result = Err(e.to_string());
        return done;
    }
    crate::capture::background_thread(false);
    #[cfg(windows)]
    win::record(opts, stop, pause, &mut done);
    #[cfg(not(windows))]
    soft::record(opts, stop, pause, &mut done);
    done
}

/// Paces frames at a steady rate, holding the clock while paused and skipping ticks when the
/// encoder falls behind.
struct Clock {
    interval: Duration,
    base: Instant,
    n_base: u64,
    n: u64,
    paused_at: Option<Instant>,
    recorded: Duration,
}

impl Clock {
    fn new(fps: u32) -> Clock {
        Clock {
            interval: Duration::from_nanos(1_000_000_000 / fps.clamp(1, 120) as u64),
            base: Instant::now(),
            n_base: 0,
            n: 0,
            paused_at: None,
            recorded: Duration::ZERO,
        }
    }

    /// When the next frame is due.
    fn deadline(&self) -> Instant {
        self.base + self.interval * (self.n - self.n_base) as u32
    }

    /// Time stamp and length of the frame being written, in 100 ns units.
    fn stamp(&self, fps: u32) -> (i64, i64) {
        let fps = fps.clamp(1, 120) as i64;
        let t = self.n as i64 * 10_000_000 / fps;
        let t1 = (self.n as i64 + 1) * 10_000_000 / fps;
        (t, t1 - t)
    }

    /// After writing a frame: move on, skipping frames the encoder had no time for.
    fn advance(&mut self) {
        self.n += 1;
        self.recorded += self.interval;
        let now = Instant::now();
        let late = now.saturating_duration_since(self.deadline());
        if late > self.interval * 2 {
            let skip = (late.as_nanos() / self.interval.as_nanos().max(1)) as u64;
            self.n += skip;
            self.recorded += self.interval * skip as u32;
        }
    }

    /// Returns true while paused (after sleeping a little).
    fn hold(&mut self, pause: &AtomicBool) -> bool {
        if pause.load(Ordering::Relaxed) {
            if self.paused_at.is_none() {
                self.paused_at = Some(Instant::now());
            }
            std::thread::sleep(Duration::from_millis(30));
            return true;
        }
        if self.paused_at.take().is_some() {
            self.base = Instant::now();
            self.n_base = self.n;
        }
        false
    }

    fn over(&self, max_s: u32) -> bool {
        max_s > 0 && self.recorded.as_secs() >= max_s as u64
    }
}

/// The encoders this computer offers for MP4.
pub fn encoders() -> Vec<EncoderInfo> {
    #[cfg(windows)]
    {
        win::encoders()
    }
    #[cfg(not(windows))]
    {
        vec![EncoderInfo { name: "OpenH264 (software)".into(), codec: "H.264", hardware: false, vendor: String::new() }]
    }
}

/// Read a saved video back: its size, frame count, length and first frame.
pub fn probe(path: &std::path::Path) -> Result<Probe, String> {
    #[cfg(windows)]
    {
        win::probe(path)
    }
    #[cfg(not(windows))]
    {
        soft::probe(path)
    }
}

/// "VEN_10DE" → "NVIDIA".
pub fn vendor_name(id: &str) -> String {
    let id = id.to_ascii_uppercase();
    if id.contains("8086") {
        "Intel".into()
    } else if id.contains("1002") || id.contains("1022") {
        "AMD".into()
    } else if id.contains("10DE") {
        "NVIDIA".into()
    } else if id.contains("5143") || id.contains("QCOM") {
        "Qualcomm".into()
    } else {
        id
    }
}

// ---------------------------------------------------------------------------------------------
// Windows: DXGI Desktop Duplication + Media Foundation (hardware encoders of every vendor)
// ---------------------------------------------------------------------------------------------

#[cfg(windows)]
mod win {
    use super::*;
    use windows::Win32::Foundation::{HMODULE, RECT};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::Win32::Graphics::Dxgi::*;
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
    use windows::core::{GUID, Interface, PCWSTR, PWSTR};

    type R<T> = windows::core::Result<T>;

    /// COM and Media Foundation for this thread, undone on drop.
    struct MfGuard;
    impl MfGuard {
        fn new() -> Result<MfGuard, String> {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|e| format!("Media Foundation isn't available: {e}"))?;
            }
            Ok(MfGuard)
        }
    }
    impl Drop for MfGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = MFShutdown();
            }
        }
    }

    fn per_monitor_dpi() {
        unsafe {
            use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    }

    fn subtype(f: VideoFormat) -> GUID {
        match f {
            VideoFormat::Hevc => MFVideoFormat_HEVC,
            VideoFormat::Wmv => MFVideoFormat_WMV3,
            _ => MFVideoFormat_H264,
        }
    }

    fn pack(a: u32, b: u32) -> u64 {
        ((a as u64) << 32) | b as u64
    }

    fn wide(s: &std::path::Path) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        s.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
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

    /// The area to record, on one display, with the device that owns that display.
    struct Screen {
        adapter: IDXGIAdapter1,
        output: IDXGIOutput1,
        /// Area relative to the display's top-left corner.
        local: (u32, u32),
    }

    /// The display holding most of the area (DXGI coordinates are physical pixels).
    fn find_screen(x: i32, y: i32, w: u32, h: u32) -> R<Option<Screen>> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut best: Option<(i64, Screen)> = None;
            let mut ai = 0;
            while let Ok(adapter) = factory.EnumAdapters1(ai) {
                ai += 1;
                let mut oi = 0;
                while let Ok(out) = adapter.EnumOutputs(oi) {
                    oi += 1;
                    let d = out.GetDesc()?;
                    let r: RECT = d.DesktopCoordinates;
                    let ix = (x + w as i32).min(r.right) - x.max(r.left);
                    let iy = (y + h as i32).min(r.bottom) - y.max(r.top);
                    if ix <= 0 || iy <= 0 {
                        continue;
                    }
                    let rot = d.Rotation;
                    if rot != DXGI_MODE_ROTATION_IDENTITY && rot != DXGI_MODE_ROTATION_UNSPECIFIED {
                        continue;
                    }
                    let area = ix as i64 * iy as i64;
                    if best.as_ref().is_some_and(|b| b.0 >= area) {
                        continue;
                    }
                    let Ok(out1) = out.cast::<IDXGIOutput1>() else { continue };
                    // Keep the area on this display.
                    let lx = (x.max(r.left) - r.left) as u32;
                    let ly = (y.max(r.top) - r.top) as u32;
                    best = Some((area, Screen { adapter: adapter.clone(), output: out1, local: (lx, ly) }));
                }
            }
            Ok(best.map(|b| b.1))
        }
    }

    fn device(adapter: &IDXGIAdapter1) -> R<(ID3D11Device, ID3D11DeviceContext)> {
        unsafe {
            let mut dev = None;
            let mut ctx = None;
            D3D11CreateDevice(
                adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut dev),
                None,
                Some(&mut ctx),
            )?;
            let dev = dev.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
            let ctx = ctx.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
            // Media Foundation uses the device from its own threads too.
            if let Ok(mt) = dev.cast::<ID3D11Multithread>() {
                let _ = mt.SetMultithreadProtected(true);
            }
            Ok((dev, ctx))
        }
    }

    fn texture(dev: &ID3D11Device, w: u32, h: u32, gdi: bool) -> R<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: if gdi { D3D11_RESOURCE_MISC_GDI_COMPATIBLE.0 as u32 } else { 0 },
        };
        let mut t = None;
        unsafe { dev.CreateTexture2D(&desc, None, Some(&mut t))? };
        t.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))
    }

    /// Where to draw the pointer, in the area's pixels, and which pointer.
    struct Pointer {
        hotspots: std::collections::HashMap<isize, (i32, i32)>,
    }
    impl Pointer {
        fn new() -> Pointer {
            Pointer { hotspots: Default::default() }
        }

        /// Draw the current pointer onto a device context showing the area at (ox, oy).
        fn draw(&mut self, hdc: *mut core::ffi::c_void, ox: i32, oy: i32) {
            use windows_sys::Win32::Graphics::Gdi::DeleteObject;
            use windows_sys::Win32::UI::WindowsAndMessaging::{CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, GetCursorInfo, GetIconInfo, ICONINFO};
            unsafe {
                let mut ci: CURSORINFO = std::mem::zeroed();
                ci.cbSize = std::mem::size_of::<CURSORINFO>() as u32;
                if GetCursorInfo(&mut ci) == 0 || ci.flags & CURSOR_SHOWING == 0 || ci.hCursor.is_null() {
                    return;
                }
                let key = ci.hCursor as isize;
                let hot = *self.hotspots.entry(key).or_insert_with(|| {
                    let mut ii: ICONINFO = std::mem::zeroed();
                    if GetIconInfo(ci.hCursor, &mut ii) != 0 {
                        if !ii.hbmMask.is_null() {
                            DeleteObject(ii.hbmMask);
                        }
                        if !ii.hbmColor.is_null() {
                            DeleteObject(ii.hbmColor);
                        }
                        (ii.xHotspot as i32, ii.yHotspot as i32)
                    } else {
                        (0, 0)
                    }
                });
                DrawIconEx(hdc, ci.ptScreenPos.x - hot.0 - ox, ci.ptScreenPos.y - hot.1 - oy, ci.hCursor, 0, 0, 0, std::ptr::null_mut(), DI_NORMAL);
            }
        }
    }

    /// The sink writer and what it picked.
    struct Writer {
        sw: IMFSinkWriter,
        stream: u32,
    }

    fn make_writer(opts: &VideoOpts, w: u32, h: u32, gpu: Option<&ID3D11Device>, hardware: bool) -> R<Writer> {
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 4)?;
            let attrs = attrs.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
            attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, hardware as u32)?;
            if let Some(dev) = gpu {
                let mut token = 0u32;
                let mut mgr = None;
                MFCreateDXGIDeviceManager(&mut token, &mut mgr)?;
                let mgr = mgr.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
                mgr.ResetDevice(dev, token)?;
                attrs.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &mgr)?;
            }
            let path = wide(&opts.path);
            let sw = MFCreateSinkWriterFromURL(PCWSTR(path.as_ptr()), None, &attrs)?;

            let fps = opts.fps.clamp(1, 120);
            let out = MFCreateMediaType()?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out.SetGUID(&MF_MT_SUBTYPE, &subtype(opts.format))?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate(w, h, fps, opts.format, opts.quality))?;
            out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            out.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
            out.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            if opts.format == VideoFormat::Mp4 {
                out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
            }
            let stream = sw.AddStream(&out)?;

            let inp = MFCreateMediaType()?;
            inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            // Texture frames are BGRA surfaces; memory frames are top-down BGRX rows.
            inp.SetGUID(&MF_MT_SUBTYPE, if gpu.is_some() { &MFVideoFormat_ARGB32 } else { &MFVideoFormat_RGB32 })?;
            inp.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            inp.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
            inp.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
            inp.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            inp.SetUINT32(&MF_MT_DEFAULT_STRIDE, w * 4)?;
            sw.SetInputMediaType(stream, &inp, None)?;
            sw.BeginWriting()?;
            Ok(Writer { sw, stream })
        }
    }

    /// The name of the encoder the sink writer chose, and whether it's a hardware one.
    fn chosen_encoder(sw: &IMFSinkWriter, stream: u32, format: VideoFormat) -> (String, bool) {
        unsafe {
            let Ok(ex) = sw.cast::<IMFSinkWriterEx>() else { return (String::new(), false) };
            let mut i = 0;
            loop {
                let mut cat = GUID::zeroed();
                let mut t: Option<IMFTransform> = None;
                if ex.GetTransformForStream(stream, i, Some(&mut cat), &mut t).is_err() {
                    break;
                }
                i += 1;
                if cat != MFT_CATEGORY_VIDEO_ENCODER {
                    continue;
                }
                let Some(t) = t else { continue };
                let attrs = t.GetAttributes().ok();
                let hw = attrs.as_ref().is_some_and(|a| a.GetStringLength(&MFT_ENUM_HARDWARE_URL_Attribute).is_ok());
                let name = attrs.as_ref().and_then(|a| take_string(a, &MFT_FRIENDLY_NAME_Attribute));
                let name = match name {
                    Some(n) => n,
                    None if hw => {
                        let v = attrs.as_ref().and_then(|a| take_string(a, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute)).unwrap_or_default();
                        // The transform doesn't carry its name; the registered list does.
                        let vendor = vendor_name(&v);
                        let codec = match format {
                            VideoFormat::Hevc => "HEVC",
                            _ => "H.264",
                        };
                        encoders()
                            .into_iter()
                            .find(|e| e.hardware && e.codec == codec && (vendor.is_empty() || e.vendor == vendor))
                            .map(|e| e.name)
                            .unwrap_or_else(|| format!("{vendor} hardware encoder"))
                    }
                    None => match format {
                        VideoFormat::Wmv => "Windows Media Video 9 encoder (software)".into(),
                        VideoFormat::Hevc => "Windows HEVC encoder (software)".into(),
                        _ => "Windows H.264 encoder (software)".into(),
                    },
                };
                return (name, hw);
            }
            (String::new(), false)
        }
    }

    fn write(wr: &Writer, sample: &IMFSample, clock: &Clock, fps: u32) -> R<()> {
        let (t, d) = clock.stamp(fps);
        unsafe {
            sample.SetSampleTime(t)?;
            sample.SetSampleDuration(d)?;
            wr.sw.WriteSample(wr.stream, sample)
        }
    }

    // ---- graphics-chip path: Desktop Duplication → texture → encoder ----

    struct Dup {
        dev: ID3D11Device,
        ctx: ID3D11DeviceContext,
        dup: IDXGIOutputDuplication,
        output: IDXGIOutput1,
        /// Latest picture of the area.
        latest: ID3D11Texture2D,
        local: (u32, u32),
        w: u32,
        h: u32,
    }

    impl Dup {
        fn new(screen: &Screen, w: u32, h: u32) -> R<Dup> {
            let (dev, ctx) = device(&screen.adapter)?;
            let dup = unsafe { screen.output.DuplicateOutput(&dev)? };
            let latest = texture(&dev, w, h, false)?;
            Ok(Dup { dev, ctx, dup, output: screen.output.clone(), latest, local: screen.local, w, h })
        }

        /// Take screen updates until `until`; returns false when the duplication was lost and
        /// couldn't be made again.
        fn pump(&mut self, until: Instant) -> bool {
            loop {
                let now = Instant::now();
                let ms = until.saturating_duration_since(now).as_millis() as u32;
                let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
                let mut res: Option<IDXGIResource> = None;
                match unsafe { self.dup.AcquireNextFrame(ms, &mut info, &mut res) } {
                    Ok(()) => {
                        if info.LastPresentTime != 0
                            && let Some(r) = res.and_then(|r| r.cast::<ID3D11Texture2D>().ok())
                        {
                            let (lx, ly) = self.local;
                            let bx = D3D11_BOX { left: lx, top: ly, front: 0, right: lx + self.w, bottom: ly + self.h, back: 1 };
                            unsafe { self.ctx.CopySubresourceRegion(&self.latest, 0, 0, 0, 0, &r, 0, Some(&bx)) };
                        }
                        unsafe {
                            let _ = self.dup.ReleaseFrame();
                        }
                        if ms == 0 {
                            return true;
                        }
                    }
                    Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return true,
                    Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                        // A full-screen game, a mode change or the lock screen: start again.
                        std::thread::sleep(Duration::from_millis(50));
                        match unsafe { self.output.DuplicateOutput(&self.dev) } {
                            Ok(d) => self.dup = d,
                            Err(_) => {
                                if Instant::now() >= until {
                                    return true;
                                }
                            }
                        }
                    }
                    Err(_) => return false,
                }
                if Instant::now() >= until {
                    return true;
                }
            }
        }

        /// A new texture holding the latest picture (and the pointer), wrapped as a sample.
        fn sample(&self, pointer: Option<(&mut Pointer, i32, i32)>) -> R<IMFSample> {
            let tex = texture(&self.dev, self.w, self.h, true)?;
            unsafe {
                self.ctx.CopyResource(&tex, &self.latest);
                // Taking the surface's GDI handle waits (idle) for the copy to finish, so the
                // encoder never spins waiting for it: 5 s at 120 fps measured 1.6 s of
                // processor time this way against 3.7 s without.
                let surf: IDXGISurface1 = tex.cast()?;
                if let Ok(hdc) = surf.GetDC(false) {
                    if let Some((p, ox, oy)) = pointer {
                        p.draw(hdc.0, ox, oy);
                    }
                    let _ = surf.ReleaseDC(None);
                }
                let buf = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &tex, 0, false)?;
                let len = buf.cast::<IMF2DBuffer>()?.GetContiguousLength()?;
                buf.SetCurrentLength(len)?;
                let s = MFCreateSample()?;
                s.AddBuffer(&buf)?;
                Ok(s)
            }
        }
    }

    // ---- fallback path: GDI copy → memory → encoder ----

    struct Gdi {
        screen: *mut core::ffi::c_void,
        mem: *mut core::ffi::c_void,
        bmp: *mut core::ffi::c_void,
        old: *mut core::ffi::c_void,
        bits: *mut u8,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    }

    impl Gdi {
        fn new(x: i32, y: i32, w: u32, h: u32) -> Result<Gdi, String> {
            use windows_sys::Win32::Graphics::Gdi::*;
            unsafe {
                let screen = GetDC(std::ptr::null_mut());
                let mem = CreateCompatibleDC(screen);
                let mut bi: BITMAPINFO = std::mem::zeroed();
                bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
                bi.bmiHeader.biWidth = w as i32;
                bi.bmiHeader.biHeight = -(h as i32); // top-down rows
                bi.bmiHeader.biPlanes = 1;
                bi.bmiHeader.biBitCount = 32;
                bi.bmiHeader.biCompression = BI_RGB;
                let mut bits = std::ptr::null_mut();
                let bmp = CreateDIBSection(mem, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
                if bmp.is_null() || bits.is_null() {
                    DeleteDC(mem);
                    ReleaseDC(std::ptr::null_mut(), screen);
                    return Err("couldn't make a picture buffer".into());
                }
                let old = SelectObject(mem, bmp);
                Ok(Gdi { screen, mem, bmp, old, bits: bits as *mut u8, x, y, w, h })
            }
        }

        fn sample(&self, pointer: Option<&mut Pointer>) -> R<IMFSample> {
            use windows_sys::Win32::Graphics::Gdi::{BitBlt, SRCCOPY};
            unsafe {
                BitBlt(self.mem, 0, 0, self.w as i32, self.h as i32, self.screen, self.x, self.y, SRCCOPY);
                if let Some(p) = pointer {
                    p.draw(self.mem, self.x, self.y);
                }
                let len = self.w * self.h * 4;
                let buf = MFCreateMemoryBuffer(len)?;
                let mut dst = std::ptr::null_mut();
                buf.Lock(&mut dst, None, None)?;
                std::ptr::copy_nonoverlapping(self.bits, dst, len as usize);
                buf.Unlock()?;
                buf.SetCurrentLength(len)?;
                let s = MFCreateSample()?;
                s.AddBuffer(&buf)?;
                Ok(s)
            }
        }
    }

    impl Drop for Gdi {
        fn drop(&mut self) {
            use windows_sys::Win32::Graphics::Gdi::*;
            unsafe {
                SelectObject(self.mem, self.old);
                DeleteObject(self.bmp);
                DeleteDC(self.mem);
                ReleaseDC(std::ptr::null_mut(), self.screen);
            }
        }
    }

    enum Source {
        Gpu(Dup),
        Gdi(Gdi),
    }

    /// Try the ways to record from best to plainest; the first that writes a frame is kept.
    pub fn record(opts: &VideoOpts, stop: &AtomicBool, pause: &AtomicBool, done: &mut VideoDone) {
        per_monitor_dpi();
        let _mf = match MfGuard::new() {
            Ok(g) => g,
            Err(e) => {
                done.result = Err(e);
                return;
            }
        };
        let (x, y, w0, h0) = opts.rect;
        // Keep the area on one display (the one holding most of it).
        let screen = find_screen(x, y, w0, h0).ok().flatten();
        let (gx, gy, gw, gh) = match &screen {
            Some(s) => {
                // Clip to that display.
                let d = unsafe { s.output.GetDesc() }.ok();
                match d {
                    Some(d) => {
                        let r = d.DesktopCoordinates;
                        let gx = x.max(r.left);
                        let gy = y.max(r.top);
                        let gr = (x + w0 as i32).min(r.right);
                        let gb = (y + h0 as i32).min(r.bottom);
                        (gx, gy, (gr - gx).max(0) as u32, (gb - gy).max(0) as u32)
                    }
                    None => (x, y, w0, h0),
                }
            }
            None => (x, y, w0, h0),
        };
        let (w, h) = match even_size(gw, gh) {
            Ok(s) => s,
            Err(e) => {
                done.result = Err(e);
                return;
            }
        };
        done.width = w;
        done.height = h;

        // (use the graphics chip to copy, use a hardware encoder)
        let mut tries: Vec<(bool, bool)> = Vec::new();
        let gpu_ok = screen.is_some() && opts.format != VideoFormat::Wmv;
        if opts.hardware && gpu_ok {
            tries.push((true, true));
        }
        if opts.hardware && opts.format != VideoFormat::Wmv {
            tries.push((false, true));
        }
        tries.push((false, false));

        let mut last_err = String::new();
        for (gpu, hw) in tries {
            let _ = std::fs::remove_file(&opts.path);
            let source = if gpu {
                match screen.as_ref().map(|s| Dup::new(s, w, h)) {
                    Some(Ok(d)) => Source::Gpu(d),
                    Some(Err(e)) => {
                        last_err = format!("screen copy: {e}");
                        continue;
                    }
                    None => continue,
                }
            } else {
                match Gdi::new(gx, gy, w, h) {
                    Ok(g) => Source::Gdi(g),
                    Err(e) => {
                        last_err = e;
                        continue;
                    }
                }
            };
            let dev = match &source {
                Source::Gpu(d) => Some(d.dev.clone()),
                Source::Gdi(_) => None,
            };
            let wr = match make_writer(opts, w, h, dev.as_ref(), hw) {
                Ok(wr) => wr,
                Err(e) => {
                    last_err = e.message().to_string();
                    if last_err.is_empty() {
                        last_err = format!("{e}");
                    }
                    continue;
                }
            };
            match run(opts, source, &wr, (gx, gy), stop, pause, done) {
                Ok(()) => {
                    done.capture = if gpu { "graphics chip" } else { "GDI" };
                    if done.encoder.is_empty() {
                        let (n, h) = chosen_encoder(&wr.sw, wr.stream, opts.format);
                        done.encoder = n;
                        done.hardware = h;
                    }
                    let fin = unsafe { wr.sw.Finalize() };
                    done.result = fin.map_err(|e| format!("couldn't finish the file: {e}"));
                    return;
                }
                Err((started, e)) => {
                    last_err = e;
                    if started {
                        // Frames were written: finish what there is rather than lose it.
                        let _ = unsafe { wr.sw.Finalize() };
                        done.result = Err(last_err);
                        return;
                    }
                    drop(wr);
                }
            }
        }
        let _ = std::fs::remove_file(&opts.path);
        done.result = Err(match opts.format {
            VideoFormat::Hevc => format!("This computer can't make HEVC videos ({last_err}). Pick MP4 (H.264) instead."),
            _ => format!("Couldn't record: {last_err}"),
        });
    }

    /// The recording loop. Err((frames were written, why)).
    fn run(
        opts: &VideoOpts,
        mut source: Source,
        wr: &Writer,
        origin: (i32, i32),
        stop: &AtomicBool,
        pause: &AtomicBool,
        done: &mut VideoDone,
    ) -> Result<(), (bool, String)> {
        let fps = opts.fps.clamp(1, 120);
        let mut clock = Clock::new(fps);
        let mut pointer = Pointer::new();
        let _started = Instant::now();
        let mut paused_total = Duration::ZERO;
        let mut first = true;
        // The first picture comes at once.
        if let Source::Gpu(d) = &mut source
            && !d.pump(Instant::now() + Duration::from_millis(300))
        {
            return Err((false, "the screen copy stopped".into()));
        }
        loop {
            if !first && (stop.load(Ordering::Relaxed) || clock.over(opts.max_s)) {
                break;
            }
            if !first && clock.hold(pause) {
                let t = Instant::now();
                while pause.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(30));
                }
                paused_total += t.elapsed();
                continue;
            }
            let deadline = clock.deadline();
            let sample = match &mut source {
                Source::Gpu(d) => {
                    if !first && !d.pump(deadline) {
                        return Err((true, "the screen copy stopped".into()));
                    }
                    d.sample(opts.cursor.then_some((&mut pointer, origin.0, origin.1)))
                }
                Source::Gdi(g) => {
                    let now = Instant::now();
                    if !first && deadline > now {
                        std::thread::sleep(deadline - now);
                    }
                    g.sample(opts.cursor.then_some(&mut pointer))
                }
            };
            let r = sample.and_then(|s| write(wr, &s, &clock, fps));
            if let Err(e) = r {
                return Err((!first, format!("{e}")));
            }
            if first {
                let (n, h) = chosen_encoder(&wr.sw, wr.stream, opts.format);
                done.encoder = n;
                done.hardware = h;
                first = false;
                done.frames += 1;
                // Starting the encoder takes a moment; the clock starts after it.
                clock.n += 1;
                clock.recorded += clock.interval;
                clock.base = Instant::now();
                clock.n_base = clock.n;
                continue;
            }
            done.frames += 1;
            clock.advance();
        }
        done.seconds = clock.recorded.as_secs_f32();
        Ok(())
    }

    /// Hardware and software encoders for H.264, HEVC and WMV.
    pub fn encoders() -> Vec<EncoderInfo> {
        let _mf = match MfGuard::new() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        let mut v = Vec::new();
        for (codec, sub) in [("H.264", MFVideoFormat_H264), ("HEVC", MFVideoFormat_HEVC), ("WMV", MFVideoFormat_WMV3)] {
            let info = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: sub };
            for hw in [true, false] {
                let flags = if hw {
                    MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER
                } else {
                    MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER
                };
                unsafe {
                    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
                    let mut n = 0u32;
                    if MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, flags, None, Some(&info), &mut list, &mut n).is_err() || list.is_null() {
                        continue;
                    }
                    for i in 0..n as usize {
                        let a = std::ptr::read(list.add(i));
                        if let Some(a) = a {
                            let attrs: IMFAttributes = match a.cast() {
                                Ok(x) => x,
                                Err(_) => continue,
                            };
                            let name = take_string(&attrs, &MFT_FRIENDLY_NAME_Attribute).unwrap_or_else(|| format!("{codec} encoder"));
                            let vendor = take_string(&attrs, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute).map(|v| vendor_name(&v)).unwrap_or_default();
                            if !v.iter().any(|e: &EncoderInfo| e.name == name && e.codec == codec) {
                                v.push(EncoderInfo { name, codec, hardware: hw, vendor });
                            }
                        }
                    }
                    CoTaskMemFree(Some(list as *const _));
                }
            }
        }
        v
    }

    /// Decode the file to count its frames and keep the first one.
    pub fn probe(path: &std::path::Path) -> Result<Probe, String> {
        let _mf = MfGuard::new()?;
        let e = |e: windows::core::Error| e.to_string();
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 1).map_err(e)?;
            let attrs = attrs.ok_or("no attributes")?;
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1).map_err(e)?;
            let p = wide(path);
            let rd = MFCreateSourceReaderFromURL(PCWSTR(p.as_ptr()), &attrs).map_err(e)?;
            let vs = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            let t = MFCreateMediaType().map_err(e)?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(e)?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(e)?;
            rd.SetCurrentMediaType(vs, None, &t).map_err(e)?;
            let cur = rd.GetCurrentMediaType(vs).map_err(e)?;
            let size = cur.GetUINT64(&MF_MT_FRAME_SIZE).map_err(e)?;
            let (w, h) = ((size >> 32) as u32, size as u32);
            let mut frames = 0u64;
            let mut last_t = 0i64;
            let mut first = None;
            loop {
                let mut flags = 0u32;
                let mut ts = 0i64;
                let mut s: Option<IMFSample> = None;
                rd.ReadSample(vs, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut s)).map_err(e)?;
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                let Some(s) = s else { continue };
                frames += 1;
                last_t = ts + s.GetSampleDuration().unwrap_or(0);
                let dump = std::env::var_os("SNAGSTER_VIDEO_DUMP").is_some() && [2u64, 3, 11, 61].contains(&frames);
                if first.is_none() || dump {
                    let mut img = image::RgbaImage::new(w, h);
                    let mut copy = |p0: *const u8, pitch: isize| {
                        for yy in 0..h {
                            let row = p0.offset(yy as isize * pitch);
                            for xx in 0..w {
                                let px = row.add(xx as usize * 4);
                                img.put_pixel(xx, yy, image::Rgba([*px.add(2), *px.add(1), *px, 255]));
                            }
                        }
                    };
                    let b0 = s.GetBufferByIndex(0).map_err(e)?;
                    if let Ok(b2) = b0.cast::<IMF2DBuffer>() {
                        // A picture buffer says where its top row is and how far apart rows are.
                        let mut p0 = std::ptr::null_mut();
                        let mut pitch = 0i32;
                        b2.Lock2D(&mut p0, &mut pitch).map_err(e)?;
                        copy(p0, pitch as isize);
                        let _ = b2.Unlock2D();
                    } else {
                        // A plain buffer: rows run top-down when the stride is positive, else bottom-up.
                        let buf = s.ConvertToContiguousBuffer().map_err(e)?;
                        let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|v| v as i32).unwrap_or(-(w as i32 * 4));
                        let mut p = std::ptr::null_mut();
                        let mut len = 0u32;
                        buf.Lock(&mut p, None, Some(&mut len)).map_err(e)?;
                        if len as usize >= (w * h * 4) as usize {
                            if stride < 0 {
                                copy(p.add(((h - 1) * w * 4) as usize), stride as isize);
                            } else {
                                copy(p, stride as isize);
                            }
                        }
                        let _ = buf.Unlock();
                    }
                    if dump {
                        let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                        let _ = img.save(std::env::temp_dir().join(format!("{stem}-frame{}.png", frames - 1)));
                    }
                    if first.is_none() {
                        first = Some(img);
                    }
                }
            }
            Ok(Probe { width: w, height: h, frames, seconds: last_t as f64 / 1e7, first })
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Linux and macOS: xcap screen grabs + OpenH264 (software) into an MP4 file
// ---------------------------------------------------------------------------------------------

#[cfg(not(windows))]
mod soft {
    use super::*;

    pub fn record(opts: &VideoOpts, stop: &AtomicBool, pause: &AtomicBool, done: &mut VideoDone) {
        let (x, y, w0, h0) = opts.rect;
        let (w, h) = match even_size(w0, h0) {
            Ok(s) => s,
            Err(e) => {
                done.result = Err(e);
                return;
            }
        };
        done.width = w;
        done.height = h;
        done.capture = "screen grab";
        done.encoder = "OpenH264 (software)".into();
        done.result = encode(opts, (x, y, w, h), stop, pause, done);
    }

    fn encode(opts: &VideoOpts, rect: (i32, i32, u32, u32), stop: &AtomicBool, pause: &AtomicBool, done: &mut VideoDone) -> Result<(), String> {
        use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameRate};
        use openh264::formats::{RgbSliceU8, YUVBuffer};
        let (x, y, w, h) = rect;
        let fps = opts.fps.clamp(1, 120);
        let cfg = EncoderConfig::new()
            .bitrate(BitRate::from_bps(bitrate(w, h, fps, opts.format, opts.quality)))
            .max_frame_rate(FrameRate::from_hz(fps as f32));
        let api = openh264::OpenH264API::from_source();
        let mut enc = Encoder::with_api_config(api, cfg).map_err(|e| e.to_string())?;
        let file = std::fs::File::create(&opts.path).map_err(|e| e.to_string())?;
        let mut mux = Mux::new(std::io::BufWriter::new(file), w, h, fps)?;
        let mut clock = Clock::new(fps);
        let _started = Instant::now();
        let mut paused = Duration::ZERO;
        let mut first = true;
        let mut rgb = vec![0u8; (w * h * 3) as usize];
        loop {
            if !first && (stop.load(Ordering::Relaxed) || clock.over(opts.max_s)) {
                break;
            }
            if !first && clock.hold(pause) {
                let t = Instant::now();
                while pause.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(30));
                }
                paused += t.elapsed();
                continue;
            }
            let now = Instant::now();
            let deadline = clock.deadline();
            if !first && deadline > now {
                std::thread::sleep(deadline - now);
            }
            let mut img = crate::capture::grab_rect(x, y, w, h)?;
            if opts.cursor
                && let Some(c) = crate::capture::cursor_pos()
            {
                crate::capture::stamp_pointer(&mut img, ((c.0 - x) as f32, (c.1 - y) as f32));
            }
            let img = if img.dimensions() != (w, h) { image::imageops::resize(&img, w, h, image::imageops::FilterType::Nearest) } else { img };
            for (d, s) in rgb.chunks_exact_mut(3).zip(img.as_raw().chunks_exact(4)) {
                d.copy_from_slice(&s[..3]);
            }
            let yuv = YUVBuffer::from_rgb_source(RgbSliceU8::new(&rgb, (w as usize, h as usize)));
            let bits = enc.encode(&yuv).map_err(|e| e.to_string())?;
            let (t, d) = clock.stamp(fps);
            let mut nals = Vec::new();
            for l in 0..bits.num_layers() {
                let layer = bits.layer(l).ok_or("missing layer")?;
                for n in 0..layer.nal_count() {
                    nals.push(layer.nal_unit(n).ok_or("missing NAL")?.to_vec());
                }
            }
            mux.frame(&nals, t, d)?;
            first = false;
            done.frames += 1;
            clock.advance();
        }
        mux.finish()?;
        done.seconds = started.elapsed().saturating_sub(paused).as_secs_f32();
        Ok(())
    }

    /// Writes H.264 frames into an MP4 file with the `mp4` crate.
    struct Mux<W: std::io::Write + std::io::Seek> {
        w: Option<W>,
        mp4: Option<mp4::Mp4Writer<W>>,
        width: u32,
        height: u32,
        fps: u32,
        sps: Vec<u8>,
        pps: Vec<u8>,
    }

    /// Strip the Annex B start code (00 00 01 or 00 00 00 01).
    fn payload(n: &[u8]) -> &[u8] {
        if n.starts_with(&[0, 0, 0, 1]) {
            &n[4..]
        } else if n.starts_with(&[0, 0, 1]) {
            &n[3..]
        } else {
            n
        }
    }

    impl<W: std::io::Write + std::io::Seek> Mux<W> {
        fn new(w: W, width: u32, height: u32, fps: u32) -> Result<Self, String> {
            Ok(Mux { w: Some(w), mp4: None, width, height, fps, sps: Vec::new(), pps: Vec::new() })
        }

        fn frame(&mut self, nals: &[Vec<u8>], t: i64, d: i64) -> Result<(), String> {
            let mut data = Vec::new();
            let mut key = false;
            for n in nals {
                let p = payload(n);
                if p.is_empty() {
                    continue;
                }
                match p[0] & 0x1f {
                    7 => self.sps = p.to_vec(),
                    8 => self.pps = p.to_vec(),
                    5 => {
                        key = true;
                        data.extend_from_slice(&(p.len() as u32).to_be_bytes());
                        data.extend_from_slice(p);
                    }
                    _ => {
                        data.extend_from_slice(&(p.len() as u32).to_be_bytes());
                        data.extend_from_slice(p);
                    }
                }
            }
            if self.mp4.is_none() {
                if self.sps.is_empty() || self.pps.is_empty() {
                    return Ok(());
                }
                let config = mp4::Mp4Config {
                    major_brand: mp4::FourCC::from(*b"isom"),
                    minor_version: 512,
                    compatible_brands: vec![
                        mp4::FourCC::from(*b"isom"),
                        mp4::FourCC::from(*b"iso2"),
                        mp4::FourCC::from(*b"avc1"),
                        mp4::FourCC::from(*b"mp41"),
                    ],
                    timescale: 10_000_000,
                };
                let w = self.w.take().ok_or("writer gone")?;
                let mut m = mp4::Mp4Writer::write_start(w, &config).map_err(|e| e.to_string())?;
                m.add_track(&mp4::TrackConfig {
                    track_type: mp4::TrackType::Video,
                    timescale: 10_000_000,
                    language: "und".into(),
                    media_conf: mp4::MediaConfig::AvcConfig(mp4::AvcConfig {
                        width: self.width as u16,
                        height: self.height as u16,
                        seq_param_set: self.sps.clone(),
                        pic_param_set: self.pps.clone(),
                    }),
                })
                .map_err(|e| e.to_string())?;
                self.mp4 = Some(m);
                let _ = self.fps;
            }
            if data.is_empty() {
                return Ok(());
            }
            let m = self.mp4.as_mut().ok_or("no file")?;
            m.write_sample(
                1,
                &mp4::Mp4Sample { start_time: t as u64, duration: d as u32, rendering_offset: 0, is_sync: key, bytes: data.into() },
            )
            .map_err(|e| e.to_string())
        }

        fn finish(mut self) -> Result<(), String> {
            match self.mp4.take() {
                Some(mut m) => m.write_end().map_err(|e| e.to_string()),
                None => Err("no frames were recorded".into()),
            }
        }
    }

    pub fn probe(path: &std::path::Path) -> Result<Probe, String> {
        let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let size = f.metadata().map_err(|e| e.to_string())?.len();
        let r = mp4::Mp4Reader::read_header(std::io::BufReader::new(f), size).map_err(|e| e.to_string())?;
        let t = r.tracks().get(&1).ok_or("no video track")?;
        Ok(Probe {
            width: t.width() as u32,
            height: t.height() as u32,
            frames: t.sample_count() as u64,
            seconds: t.duration().as_secs_f64(),
            first: None,
        })
    }
}
