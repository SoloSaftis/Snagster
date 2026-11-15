//! Windows zero-copy screen feed for live streaming: the Desktop Duplication texture goes
//! through the graphics chip's Direct3D 11 video processor, which scales it to the stream's size
//! (in proportion, black around it), converts BGRA to NV12 (BT.709, video range) and writes one
//! of a ring of NV12 textures. Those go to the Media Foundation hardware encoder as DXGI
//! surfaces on the same device (`encode::win::open_video_d3d`), so no picture is read back to
//! memory and the processor never converts colours.
//!
//! Written from the Direct3D 11 video documentation ("ID3D11VideoContext::VideoProcessorBlt",
//! "D3D11_VIDEO_PROCESSOR_COLOR_SPACE", "Supporting Direct3D 11 Video Decoding in Media
//! Foundation" for the device manager). The CPU path (`ScreenFeed`, `rgba_to_nv12`) stays the
//! fallback when any step here can't start.

use super::win::{Dup, Pointer, clip_rect, clip_to, device, find_screen, per_monitor_dpi, texture};
use super::*;
use crate::encode::{D3dDevice, D3dTexture, Surface, SurfaceDevice};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::core::Interface;

type R<T> = windows::core::Result<T>;

/// NV12 textures written in turn. The encoder may still be reading a picture handed over a few
/// frames ago (hardware encoders work a frame or two behind); 8 covers well over that.
const RING: usize = 8;

fn fail(e: windows::core::Error) -> String {
    format!("{} ({:#010x})", e.message(), e.code().0 as u32)
}

/// Holds the device's lock (`ID3D11Multithread`) while the copy uses its one context, so the
/// encoder's own threads don't use it at the same moment.
struct Locked<'a>(Option<&'a ID3D11Multithread>);

impl<'a> Locked<'a> {
    fn new(mt: Option<&'a ID3D11Multithread>) -> Locked<'a> {
        if let Some(m) = mt {
            unsafe { m.Enter() };
        }
        Locked(mt)
    }
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        if let Some(m) = self.0 {
            unsafe { m.Leave() };
        }
    }
}

/// BGRA (any size) to NV12 at the stream's size on the graphics chip.
pub(super) struct Converter {
    vctx: ID3D11VideoContext,
    vdev: ID3D11VideoDevice,
    en: ID3D11VideoProcessorEnumerator,
    vp: ID3D11VideoProcessor,
    outs: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    next: usize,
    /// Input views made so far, by the texture they read (the feed uses one or two).
    inputs: Vec<(ID3D11Texture2D, ID3D11VideoProcessorInputView)>,
}

impl Converter {
    pub(super) fn new(dev: &ID3D11Device, ctx: &ID3D11DeviceContext, input: (u32, u32), output: (u32, u32), fps: u32) -> R<Converter> {
        let (iw, ih) = input;
        let (ow, oh) = output;
        unsafe {
            let vdev: ID3D11VideoDevice = dev.cast()?;
            let vctx: ID3D11VideoContext = ctx.cast()?;
            let rate = DXGI_RATIONAL { Numerator: fps.max(1), Denominator: 1 };
            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: rate,
                InputWidth: iw,
                InputHeight: ih,
                OutputFrameRate: rate,
                OutputWidth: ow,
                OutputHeight: oh,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
            };
            let en = vdev.CreateVideoProcessorEnumerator(&desc)?;
            let input_ok = en.CheckVideoProcessorFormat(DXGI_FORMAT_B8G8R8A8_UNORM)?;
            let output_ok = en.CheckVideoProcessorFormat(DXGI_FORMAT_NV12)?;
            if input_ok & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT.0 as u32 == 0 || output_ok & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT.0 as u32 == 0 {
                return Err(windows::core::Error::new(windows::Win32::Foundation::E_NOTIMPL, "the video processor can't turn BGRA into NV12"));
            }
            let vp = vdev.CreateVideoProcessor(&en, 0)?;
            // The stream: progressive, nothing "improved" by the driver, the whole picture in.
            vctx.VideoProcessorSetStreamFrameFormat(&vp, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            vctx.VideoProcessorSetStreamAutoProcessingMode(&vp, 0, false);
            let all = RECT { left: 0, top: 0, right: iw as i32, bottom: ih as i32 };
            vctx.VideoProcessorSetStreamSourceRect(&vp, 0, true, Some(&all));
            let (l, t, r, b) = dest_rect((iw, ih), (ow, oh));
            let dest = RECT { left: l, top: t, right: r, bottom: b };
            vctx.VideoProcessorSetStreamDestRect(&vp, 0, true, Some(&dest));
            vctx.VideoProcessorSetOutputTargetRect(&vp, false, None);
            // Black around a picture of other proportions (given as RGB, made into video black).
            let black = D3D11_VIDEO_COLOR { Anonymous: D3D11_VIDEO_COLOR_0 { RGBA: D3D11_VIDEO_COLOR_RGBA { R: 0.0, G: 0.0, B: 0.0, A: 1.0 } } };
            vctx.VideoProcessorSetOutputBackgroundColor(&vp, false, &black);
            // Colours: full-range RGB in, BT.709 video range out (what `rgba_to_nv12` makes and
            // what the encoder's media type says).
            match vctx.cast::<ID3D11VideoContext1>() {
                Ok(v1) => {
                    v1.VideoProcessorSetStreamColorSpace1(&vp, 0, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709);
                    v1.VideoProcessorSetOutputColorSpace1(&vp, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709);
                }
                Err(_) => {
                    let input = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: colour_space_bits(false, true, NOMINAL_0_255) };
                    let output = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: colour_space_bits(false, true, NOMINAL_16_235) };
                    vctx.VideoProcessorSetStreamColorSpace(&vp, 0, &input);
                    vctx.VideoProcessorSetOutputColorSpace(&vp, &output);
                }
            }
            let mut outs = Vec::with_capacity(RING);
            for _ in 0..RING {
                let t = nv12_texture(dev, ow, oh)?;
                let vd = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC { ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D, Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } } };
                let mut view = None;
                vdev.CreateVideoProcessorOutputView(&t, &en, &vd, Some(&mut view))?;
                outs.push((t, view.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?));
            }
            Ok(Converter { vctx, vdev, en, vp, outs, next: 0, inputs: Vec::new() })
        }
    }

    fn input_view(&mut self, tex: &ID3D11Texture2D) -> R<ID3D11VideoProcessorInputView> {
        if let Some((_, v)) = self.inputs.iter().find(|(t, _)| t.as_raw() == tex.as_raw()) {
            return Ok(v.clone());
        }
        let vd = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC { FourCC: 0, ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D, Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 } } };
        let mut view = None;
        unsafe { self.vdev.CreateVideoProcessorInputView(tex, &self.en, &vd, Some(&mut view))? };
        let v = view.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
        self.inputs.push((tex.clone(), v.clone()));
        Ok(v)
    }

    /// Convert `tex` (BGRA, the input size) into the next NV12 texture of the ring.
    pub(super) fn convert(&mut self, tex: &ID3D11Texture2D) -> R<ID3D11Texture2D> {
        let iv = self.input_view(tex)?;
        let (out, ov) = self.outs[self.next].clone();
        self.next = (self.next + 1) % self.outs.len();
        let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            OutputIndex: 0,
            InputFrameOrField: 0,
            PastFrames: 0,
            FutureFrames: 0,
            ppPastSurfaces: std::ptr::null_mut(),
            pInputSurface: std::mem::ManuallyDrop::new(Some(iv)),
            ppFutureSurfaces: std::ptr::null_mut(),
            ppPastSurfacesRight: std::ptr::null_mut(),
            pInputSurfaceRight: std::mem::ManuallyDrop::new(None),
            ppFutureSurfacesRight: std::ptr::null_mut(),
        };
        let r = unsafe { self.vctx.VideoProcessorBlt(&self.vp, &ov, 0, std::slice::from_ref(&stream)) };
        unsafe {
            std::mem::ManuallyDrop::drop(&mut stream.pInputSurface);
            std::mem::ManuallyDrop::drop(&mut stream.pInputSurfaceRight);
        }
        r.map(|_| out)
    }
}

fn nv12_texture(dev: &ID3D11Device, w: u32, h: u32) -> R<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut t = None;
    unsafe { dev.CreateTexture2D(&desc, None, Some(&mut t))? };
    t.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))
}

/// The feed thread's side: duplication, converter, pointer.
struct DupSide {
    d: Dup,
    conv: Converter,
    mt: Option<ID3D11Multithread>,
    composed: Option<ID3D11Texture2D>,
    pointer: Pointer,
    display: RECT,
    origin: (i32, i32),
}

// Free-threaded Direct3D 11 objects, used by the feed thread alone once it starts.
unsafe impl Send for DupSide {}

impl DupSide {
    /// One picture: the newest screen (with the pointer) converted into the next NV12 texture.
    fn picture(&mut self) -> R<ID3D11Texture2D> {
        let _l = Locked::new(self.mt.as_ref());
        unsafe {
            let input = match &self.composed {
                Some(c) => {
                    self.d.ctx.CopyResource(c, &self.d.latest);
                    let surf: IDXGISurface1 = c.cast()?;
                    if let Ok(hdc) = surf.GetDC(false) {
                        self.pointer.draw(hdc.0, self.origin.0, self.origin.1);
                        let _ = surf.ReleaseDC(None);
                    }
                    c.clone()
                }
                None => self.d.latest.clone(),
            };
            let out = self.conv.convert(&input)?;
            self.d.ctx.Flush();
            Ok(out)
        }
    }

    fn retarget(&mut self, rect: (i32, i32, u32, u32)) {
        if let Some((gx, gy, w, h)) = clip_rect(self.display, rect)
            && (w, h) == (self.d.w, self.d.h)
        {
            self.d.local = ((gx - self.display.left) as u32, (gy - self.display.top) as u32);
            self.origin = (gx, gy);
            self.d.fresh = true;
        }
    }
}

/// Start a zero-copy feed of `rect` at `o.fit` (the stream's size) and `o.fps`.
pub(super) fn start(rect: (i32, i32, u32, u32), o: FeedOptions) -> Result<SurfaceFeed, String> {
    per_monitor_dpi();
    let screen = find_screen(rect.0, rect.1, rect.2, rect.3).map_err(fail)?.ok_or("the area isn't on a screen")?;
    let (display, (gx, gy, w, h)) = clip_to(&screen, rect).ok_or("the area isn't on a screen")?;
    let out = o.fit.unwrap_or((w & !1, h & !1));
    let d = Dup::new(&screen, w, h).map_err(|e| format!("screen copy: {}", fail(e)))?;
    let conv = Converter::new(&d.dev, &d.ctx, (w, h), out, o.fps).map_err(|e| format!("video processor: {}", fail(e)))?;
    let composed = if o.cursor { Some(texture(&d.dev, w, h, true).map_err(fail)?) } else { None };
    let mt = d.dev.cast::<ID3D11Multithread>().ok();
    let dev = SurfaceDevice::D3d11(D3dDevice(d.dev.clone()));
    let side = DupSide { d, conv, mt, composed, pointer: Pointer::new(), display, origin: (gx, gy) };
    let sh = Arc::new(SurfaceShared::default());
    let s = sh.clone();
    let thread = std::thread::Builder::new().name("snagster-gpu-feed".into()).spawn(move || dup_loop(&s, side, o)).map_err(|e| e.to_string())?;
    Ok(SurfaceFeed::new(sh, dev, "graphics chip, zero-copy", Box::new(move || {
        let _ = thread.join();
    })))
}

fn dup_loop(sh: &SurfaceShared, mut side: DupSide, o: FeedOptions) {
    crate::capture::background_thread(false);
    timely_thread();
    let every = Duration::from_nanos(1_000_000_000 / o.fps.clamp(1, 240) as u64);
    let mut next = Instant::now();
    let mut force = true;
    let mut pacer = Pacer::calibrated();
    while !sh.stop.load(Ordering::Relaxed) {
        if let Some(r) = sh.moved.lock().ok().and_then(|mut m| m.take()) {
            side.retarget(r);
        }
        next += every;
        let now = Instant::now();
        if next < now {
            next = now;
        }
        // Wait here, then take what the screen has without waiting in DXGI: a duplication that
        // waits for the next screen update holds the device, and the encoder shares it.
        pacer.wait_until(next);
        if !side.d.pump(Instant::now()) {
            sh.set_error("the screen copy stopped".into());
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        let began = Instant::now();
        let changed = force || o.every_frame || side.d.fresh || (side.composed.is_some() && side.d.mouse);
        side.d.fresh = false;
        side.d.mouse = false;
        if !changed {
            continue;
        }
        match side.picture() {
            Ok(t) => {
                force = false;
                sh.publish(Surface::D3d11(D3dTexture(t)), began);
            }
            Err(e) => {
                sh.set_error(format!("graphics chip conversion: {}", fail(e)));
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// A moving test picture (`encode::MovingPicture`) of `w` × `h` uploaded to the graphics chip
/// `fps` times a second and converted there, for `--stream-test` where the screen is smaller
/// than the case: it runs the same encoder path as the zero-copy screen copy, with one upload a
/// picture instead of the duplication.
pub(super) fn test_feed(w: u32, h: u32, fps: u32) -> Result<SurfaceFeed, String> {
    let (dev, ctx) = first_device().map_err(fail)?;
    let up = upload_texture(&dev, w, h).map_err(fail)?;
    let conv = Converter::new(&dev, &ctx, (w, h), (w, h), fps).map_err(|e| format!("video processor: {}", fail(e)))?;
    let mt = dev.cast::<ID3D11Multithread>().ok();
    let side = TestSide { ctx, up, conv, mt, pic: crate::encode::MovingPicture::new(w, h), buf: vec![0; w as usize * h as usize * 4], w };
    let sh = Arc::new(SurfaceShared::default());
    let s = sh.clone();
    let thread = std::thread::Builder::new().name("snagster-gpu-test-feed".into()).spawn(move || test_loop(&s, side, fps)).map_err(|e| e.to_string())?;
    Ok(SurfaceFeed::new(sh, SurfaceDevice::D3d11(D3dDevice(dev)), "test picture uploaded to the graphics chip", Box::new(move || {
        let _ = thread.join();
    })))
}

struct TestSide {
    ctx: ID3D11DeviceContext,
    up: ID3D11Texture2D,
    conv: Converter,
    mt: Option<ID3D11Multithread>,
    pic: crate::encode::MovingPicture,
    buf: Vec<u8>,
    w: u32,
}

unsafe impl Send for TestSide {}

fn test_loop(sh: &SurfaceShared, mut side: TestSide, fps: u32) {
    timely_thread();
    let every = Duration::from_nanos(1_000_000_000 / fps.clamp(1, 240) as u64);
    let mut pacer = Pacer::calibrated();
    let mut next = Instant::now();
    let mut n = 0u32;
    while !sh.stop.load(Ordering::Relaxed) {
        next += every;
        if next < Instant::now() {
            next = Instant::now();
        }
        pacer.wait_until(next);
        let began = Instant::now();
        side.pic.frame(n, &mut side.buf);
        n = n.wrapping_add(1);
        let r = {
            let _l = Locked::new(side.mt.as_ref());
            unsafe { side.ctx.UpdateSubresource(&side.up, 0, None, side.buf.as_ptr() as *const _, side.w * 4, 0) };
            let r = side.conv.convert(&side.up);
            unsafe { side.ctx.Flush() };
            r
        };
        match r {
            Ok(t) => sh.publish(Surface::D3d11(D3dTexture(t)), began),
            Err(e) => {
                sh.set_error(format!("graphics chip conversion: {}", fail(e)));
                return;
            }
        }
    }
}

/// A device on the first graphics adapter (where the encoder is).
fn first_device() -> R<(ID3D11Device, ID3D11DeviceContext)> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        device(&factory.EnumAdapters1(0)?)
    }
}

fn upload_texture(dev: &ID3D11Device, w: u32, h: u32) -> R<ID3D11Texture2D> {
    texture(dev, w, h, false)
}

/// Check the graphics chip's conversion against `encode::rgba_to_nv12` on patches of solid
/// colour (`colour_check_picture`): the largest difference in Y and in U/V at the patches'
/// centres, and the adapter's name. For `--stream-test`.
pub(super) fn conversion_check() -> Result<(u8, u8, String), String> {
    let (w, h) = (CHECK_W, CHECK_H);
    let (dev, ctx) = first_device().map_err(fail)?;
    let name = unsafe {
        dev.cast::<IDXGIDevice>()
            .and_then(|d| d.GetAdapter())
            .and_then(|a| a.GetDesc())
            .map(|d| String::from_utf16_lossy(&d.Description).trim_end_matches('\0').to_string())
            .unwrap_or_default()
    };
    let rgba = colour_check_picture();
    let bgra: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
    let up = upload_texture(&dev, w, h).map_err(fail)?;
    let mut conv = Converter::new(&dev, &ctx, (w, h), (w, h), 30).map_err(|e| format!("video processor: {}", fail(e)))?;
    unsafe {
        ctx.UpdateSubresource(&up, 0, None, bgra.as_ptr() as *const _, w * 4, 0);
        let out = conv.convert(&up).map_err(fail)?;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut st = None;
        dev.CreateTexture2D(&desc, None, Some(&mut st)).map_err(fail)?;
        let st = st.ok_or("no staging texture")?;
        ctx.CopyResource(&st, &out);
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut m)).map_err(fail)?;
        let pitch = m.RowPitch as usize;
        let (y_at, uv_at) = nv12_planes(pitch, h as usize);
        let data = std::slice::from_raw_parts(m.pData as *const u8, uv_at + pitch * h as usize / 2);
        let gpu_y = |x: usize, y: usize| data[y_at + y * pitch + x];
        let gpu_uv = |x: usize, y: usize, k: usize| data[uv_at + (y / 2) * pitch + (x / 2) * 2 + k];
        let mut cpu = Vec::new();
        crate::encode::nv12(&rgba, w, h, &mut cpu);
        let ys = (w * h) as usize;
        let (mut dy, mut duv) = (0u8, 0u8);
        for (x, y) in check_points() {
            dy = dy.max(gpu_y(x, y).abs_diff(cpu[y * w as usize + x]));
            for k in 0..2 {
                duv = duv.max(gpu_uv(x, y, k).abs_diff(cpu[ys + (y / 2) * w as usize + (x / 2) * 2 + k]));
            }
        }
        ctx.Unmap(&st, 0);
        Ok((dy, duv, name))
    }
}
