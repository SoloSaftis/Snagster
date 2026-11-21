//! Live ink over the desktop: where the person draws while streaming, and where they see the
//! ink. The strokes themselves are in `ink`; this shows them on every monitor and, while Draw
//! is on, turns the mouse into the pen.
//!
//! - Windows: a see-through native window per monitor (a layered window filled from an
//!   `ink::Layer`, only where something changed), kept out of screen captures so the stream gets
//!   the ink once, drawn into the composed frame. Clicks pass through while Draw is off; while it
//!   is on, Snagster's own window stays usable (it is cut out of the overlay) and Esc and Ctrl+Z
//!   go to the ink, not to the program in front.
//! - macOS and Linux: ink is drawn on the Studio preview. On Wayland the desktop doesn't let a
//!   program cover the screen; on macOS and X11 a see-through window over the screen needs
//!   window-system work of its own (a transparent OpenGL window would cover the screen in black
//!   where the window system can't blend it), still to do.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Where ink can be drawn on this system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    /// Over the screen (and on the Studio preview).
    Screen,
    /// Only on the Studio preview.
    PreviewOnly,
}

/// Where ink can be drawn here.
pub fn support() -> Where {
    support_for(cfg!(windows))
}

fn support_for(windows: bool) -> Where {
    if windows { Where::Screen } else { Where::PreviewOnly }
}

/// The overlay couldn't be kept out of screen captures (Windows before 10 version 2004): the
/// screen copies hold its ink already, so the engine doesn't draw desktop ink a second time.
static IN_CAPTURES: AtomicBool = AtomicBool::new(false);

pub fn overlay_in_captures() -> bool {
    IN_CAPTURES.load(Ordering::Relaxed)
}

/// Snagster's main window on the desktop (pixels), which the overlay leaves usable while
/// drawing; None while it is minimized or hidden.
static APP_RECT: Mutex<Option<(i32, i32, u32, u32)>> = Mutex::new(None);

pub fn set_app_rect(r: Option<(i32, i32, u32, u32)>) {
    *APP_RECT.lock().unwrap_or_else(|e| e.into_inner()) = r;
}

#[cfg(any(windows, test))]
fn app_rect() -> Option<(i32, i32, u32, u32)> {
    *APP_RECT.lock().unwrap_or_else(|e| e.into_inner())
}

/// The hint shown while drawing (on the screen the pointer is on, never captured).
pub const HINT: &str = "Drawing. Press Esc to stop, Ctrl+Z to undo.";

/// Where the hint (`hint` in size) goes on a monitor at `m` (x, y, w, h), in the monitor's
/// pixels: top centre, or the bottom when the pointer is near the top.
#[cfg(any(windows, test))]
fn hint_spot(m: (i32, i32, u32, u32), hint: (u32, u32), pointer: (i32, i32)) -> (i64, i64) {
    let x = (m.2 as i64 - hint.0 as i64) / 2;
    let near_top = pointer.1 < m.1 + hint.1 as i32 + 60;
    let y = if near_top { m.3 as i64 - hint.1 as i64 - 24 } else { 24 };
    (x, y)
}

/// The hint as a picture: white text on a dark box.
#[cfg(any(windows, test))]
fn hint_picture(text: &str) -> image::RgbaImage {
    let t = crate::render::text_image(text, 18.0, [255, 255, 255, 255]);
    let (tw, th) = t.dimensions();
    let mut out = image::RgbaImage::from_pixel(tw + 24, th + 8, image::Rgba([20, 22, 28, 220]));
    image::imageops::overlay(&mut out, &t, 12, 4);
    out
}

/// Fill `area` of a monitor's overlay picture `dst` (`w` pixels wide, blue-green-red-alpha with
/// premultiplied alpha, as Windows wants it) from the ink layer at `alpha`, with a 2-pixel
/// `border` round the monitor and the `hint` picture at its spot on top.
#[cfg(any(windows, test))]
fn compose(dst: &mut [u8], w: u32, h: u32, layer: &image::RgbaImage, alpha: f32, area: crate::ink::Area, border: Option<[u8; 4]>, hint: Option<(&image::RgbaImage, i64, i64)>) {
    let (x0, y0) = (area.0.max(0), area.1.max(0));
    let (x1, y1) = (area.2.min(w as i64), area.3.min(h as i64));
    let src = layer.as_raw();
    let a_mul = alpha.clamp(0.0, 1.0);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y as usize * w as usize + x as usize) * 4;
            let mut p = [src[i], src[i + 1], src[i + 2], (src[i + 3] as f32 * a_mul).round() as u8];
            if let Some(c) = border
                && (x < 2 || y < 2 || x >= w as i64 - 2 || y >= h as i64 - 2)
            {
                p = c;
            }
            if let Some((img, hx, hy)) = hint
                && x >= hx
                && y >= hy
                && x < hx + img.width() as i64
                && y < hy + img.height() as i64
            {
                let q = img.get_pixel((x - hx) as u32, (y - hy) as u32).0;
                let qa = q[3] as f32 / 255.0;
                let da = p[3] as f32 / 255.0;
                let oa = qa + da * (1.0 - qa);
                if oa > 0.0 {
                    for k in 0..3 {
                        p[k] = ((q[k] as f32 * qa + p[k] as f32 * da * (1.0 - qa)) / oa).round() as u8;
                    }
                    p[3] = (oa * 255.0).round() as u8;
                }
            }
            let a = p[3] as u32;
            dst[i] = (p[2] as u32 * a / 255) as u8;
            dst[i + 1] = (p[1] as u32 * a / 255) as u8;
            dst[i + 2] = (p[0] as u32 * a / 255) as u8;
            dst[i + 3] = p[3];
        }
    }
}

#[cfg(windows)]
pub use win::run;

/// Nothing to start here: ink is drawn on the Studio preview.
#[cfg(not(windows))]
pub fn run() {}

#[cfg(windows)]
mod win {
    use crate::ink::{Area, Layer, Map, Placed, Space, union};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CombineRgn, CreateCompatibleDC, CreateDIBSection, CreateRectRgn, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
        HBITMAP, HDC, RGN_DIFF, ReleaseDC, SelectObject, SetWindowRgn,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_ESCAPE};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    static STARTED: AtomicBool = AtomicBool::new(false);

    /// Start the overlay's thread (once; it runs while Snagster runs and shows nothing while
    /// there is no ink and Draw is off).
    pub fn run() {
        if STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = std::thread::Builder::new().name("snagster-ink".into()).spawn(|| {
            // SAFETY: plain Win32 calls on this thread's own windows.
            unsafe {
                SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
                main_loop();
            }
        });
    }

    /// One monitor's overlay window.
    struct Screen {
        hwnd: HWND,
        rect: (i32, i32, u32, u32),
        dc: HDC,
        bmp: HBITMAP,
        bits: *mut u8,
        layer: Layer,
        shown: bool,
        /// What the window shows now: the ink's strength, Draw on, the hint's box, the cut-out
        /// for Snagster's window.
        alpha: f32,
        on: bool,
        hint_at: Option<Area>,
        cut: Option<(i32, i32, u32, u32)>,
    }

    const CLASS: &[u16] = &[b'S' as u16, b'n' as u16, b'a' as u16, b'g' as u16, b'I' as u16, b'n' as u16, b'k' as u16, 0];

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        // The monitor's top-left corner, kept with the window.
        let origin = || {
            let v = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as i64;
            ((v >> 32) as i32, v as i32)
        };
        let at = || {
            let (ox, oy) = origin();
            let x = (lp & 0xffff) as u16 as i16 as f32;
            let y = ((lp >> 16) & 0xffff) as u16 as i16 as f32;
            [ox as f32 + x, oy as f32 + y]
        };
        match msg {
            WM_LBUTTONDOWN => {
                unsafe { SetCapture(hwnd) };
                let p = at();
                crate::ink::with(|i| {
                    if i.on {
                        i.begin(Space::Desktop, p, Instant::now());
                    }
                });
                0
            }
            WM_MOUSEMOVE => {
                let p = at();
                crate::ink::with(|i| i.extend(p));
                0
            }
            WM_LBUTTONUP | WM_CAPTURECHANGED => {
                if msg == WM_LBUTTONUP {
                    unsafe { ReleaseCapture() };
                }
                crate::ink::with(|i| i.end(Instant::now()));
                0
            }
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            WM_SETCURSOR => {
                unsafe { SetCursor(LoadCursorW(std::ptr::null_mut(), IDC_CROSS)) };
                1
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }

    /// While Draw is on, Esc and Ctrl+Z belong to the ink: the program in front (Snagster
    /// included) never sees them.
    unsafe extern "system" fn keys(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 && (wp == WM_KEYDOWN as WPARAM || wp == WM_SYSKEYDOWN as WPARAM) {
            // SAFETY: a low-level keyboard hook's lParam is a KBDLLHOOKSTRUCT.
            let vk = unsafe { (*(lp as *const KBDLLHOOKSTRUCT)).vkCode };
            let ctrl = (unsafe { GetAsyncKeyState(VK_CONTROL as i32) } as u16 & 0x8000) != 0;
            let now = Instant::now();
            let took = crate::ink::with(|i| {
                if !i.on {
                    false
                } else if vk == VK_ESCAPE as u32 {
                    i.set_on(false, now);
                    true
                } else if vk == b'Z' as u32 && ctrl {
                    i.undo(now);
                    true
                } else {
                    false
                }
            });
            if took {
                return 1;
            }
        }
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, wp, lp) }
    }

    unsafe fn make(rect: (i32, i32, u32, u32)) -> Option<Screen> {
        unsafe {
            let ex = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT;
            let hwnd = CreateWindowExW(ex, CLASS.as_ptr(), CLASS.as_ptr(), WS_POPUP, rect.0, rect.1, rect.2 as i32, rect.3 as i32, std::ptr::null_mut(), std::ptr::null_mut(), GetModuleHandleW(std::ptr::null()), std::ptr::null());
            if hwnd.is_null() {
                return None;
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, (((rect.0 as i64) << 32) | (rect.1 as u32 as i64)) as isize);
            // Never in a screen capture: the stream gets the ink from the composed frame.
            if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) == 0 {
                super::IN_CAPTURES.store(true, Ordering::Relaxed);
            }
            let screen = GetDC(std::ptr::null_mut());
            let dc = CreateCompatibleDC(screen);
            ReleaseDC(std::ptr::null_mut(), screen);
            let mut bi: BITMAPINFO = std::mem::zeroed();
            bi.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: rect.2 as i32,
                biHeight: -(rect.3 as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            if bmp.is_null() || bits.is_null() {
                DeleteDC(dc);
                DestroyWindow(hwnd);
                return None;
            }
            SelectObject(dc, bmp);
            let placed = Placed { desk: rect, canvas: (0, 0, rect.2, rect.3) };
            let probe = image::RgbaImage::new(1, 1);
            let mut map = Map::placed(&placed, &probe);
            map.clip = (0, 0, rect.2 as i64, rect.3 as i64);
            Some(Screen { hwnd, rect, dc, bmp, bits: bits as *mut u8, layer: Layer::new(rect.2, rect.3, map), shown: false, alpha: 1.0, on: false, hint_at: None, cut: None })
        }
    }

    unsafe fn drop_screen(s: Screen) {
        unsafe {
            DestroyWindow(s.hwnd);
            DeleteDC(s.dc);
            DeleteObject(s.bmp);
        }
    }

    /// Put `area` of the picture on the screen (the rest stays as it was).
    unsafe fn present(s: &Screen, area: Area) {
        let pos = POINT { x: s.rect.0, y: s.rect.1 };
        let size = SIZE { cx: s.rect.2 as i32, cy: s.rect.3 as i32 };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        let dirty = RECT { left: area.0 as i32, top: area.1 as i32, right: area.2 as i32, bottom: area.3 as i32 };
        let info = UPDATELAYEREDWINDOWINFO {
            cbSize: std::mem::size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
            hdcDst: std::ptr::null_mut(),
            pptDst: &pos,
            psize: &size,
            hdcSrc: s.dc,
            pptSrc: &src,
            crKey: 0,
            pblend: &blend,
            dwFlags: ULW_ALPHA,
            prcDirty: &dirty,
        };
        unsafe { UpdateLayeredWindowIndirect(s.hwnd, &info) };
    }

    /// While drawing, clicks go to the overlay except over Snagster's own window.
    unsafe fn set_clicks(s: &mut Screen, take: bool, app: Option<(i32, i32, u32, u32)>) {
        unsafe {
            let ex = GetWindowLongPtrW(s.hwnd, GWL_EXSTYLE) as u32;
            let want = if take { ex & !WS_EX_TRANSPARENT } else { ex | WS_EX_TRANSPARENT };
            if want != ex {
                SetWindowLongPtrW(s.hwnd, GWL_EXSTYLE, want as isize);
            }
            let cut = if take { app } else { None };
            if cut != s.cut {
                s.cut = cut;
                match cut {
                    Some((x, y, w, h)) => {
                        let all = CreateRectRgn(0, 0, s.rect.2 as i32, s.rect.3 as i32);
                        let hole = CreateRectRgn(x - s.rect.0, y - s.rect.1, x - s.rect.0 + w as i32, y - s.rect.1 + h as i32);
                        CombineRgn(all, all, hole, RGN_DIFF);
                        DeleteObject(hole);
                        // The window owns the region from here.
                        SetWindowRgn(s.hwnd, all, 1);
                    }
                    None => {
                        SetWindowRgn(s.hwnd, std::ptr::null_mut(), 1);
                    }
                }
            }
        }
    }

    unsafe fn main_loop() {
        unsafe {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(proc),
                hInstance: GetModuleHandleW(std::ptr::null()),
                lpszClassName: CLASS.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassExW(&wc);
            let mut screens: Vec<Screen> = Vec::new();
            let mut listed_at: Option<Instant> = None;
            let mut last_shown = Instant::now();
            let mut hook: HHOOK = std::ptr::null_mut();
            let mut hint: Option<(String, image::RgbaImage)> = None;
            let accent = crate::theme::system_accent().unwrap_or(crate::theme::BRAND).to_array();
            loop {
                // Wake for the mouse at once, else a few times a second.
                let (on, fading) = crate::ink::with(|i| (i.on, i.fading(Instant::now())));
                let wait = if on || fading { 16 } else if screens.iter().any(|s| s.shown) { 50 } else { 120 };
                MsgWaitForMultipleObjects(0, std::ptr::null(), 0, wait, QS_ALLINPUT);
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                let now = Instant::now();
                let (on, snap) = crate::ink::with(|i| (i.on, i.snapshot(now)));
                // Esc and Ctrl+Z belong to the ink only while drawing.
                if on && hook.is_null() {
                    hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keys), GetModuleHandleW(std::ptr::null()), 0);
                } else if !on && !hook.is_null() {
                    UnhookWindowsHookEx(hook);
                    hook = std::ptr::null_mut();
                }
                let want = on || !snap.is_empty();
                if !want && !screens.iter().any(|s| s.shown) {
                    // Nothing shown for a while: give the pictures' memory back.
                    if !screens.is_empty() && last_shown.elapsed() > Duration::from_secs(30) {
                        for s in screens.drain(..) {
                            drop_screen(s);
                        }
                        listed_at = None;
                    }
                    continue;
                }
                last_shown = now;
                // The monitors, again every few seconds (one may have been plugged in).
                if listed_at.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
                    listed_at = Some(now);
                    let rects: Vec<(i32, i32, u32, u32)> = crate::capture::monitors().iter().map(|m| (m.x, m.y, m.width, m.height)).filter(|r| r.2 > 0 && r.3 > 0).collect();
                    if rects != screens.iter().map(|s| s.rect).collect::<Vec<_>>() {
                        for s in screens.drain(..) {
                            drop_screen(s);
                        }
                        screens = rects.into_iter().filter_map(|r| make(r)).collect();
                    }
                }
                let mut cursor = POINT { x: 0, y: 0 };
                GetCursorPos(&mut cursor);
                let text = crate::i18n::t(super::HINT).to_string();
                if hint.as_ref().is_none_or(|(t, _)| *t != text) {
                    hint = Some((text.clone(), super::hint_picture(&text)));
                }
                let app = super::app_rect();
                for s in &mut screens {
                    set_clicks(s, on, app);
                    let (l, t, r, b) = (s.rect.0 as f32, s.rect.1 as f32, (s.rect.0 + s.rect.2 as i32) as f32, (s.rect.1 + s.rect.3 as i32) as f32);
                    let mine: Vec<&crate::ink::Stroke> = snap
                        .in_space(Space::Desktop)
                        .filter(|k| {
                            let kb = k.bounds();
                            kb.2 >= l && kb.3 >= t && kb.0 <= r && kb.1 <= b
                        })
                        .collect();
                    let mut dirty = s.layer.update(mine.into_iter());
                    let alpha = if snap.is_empty() { 0.0 } else { snap.alpha };
                    if alpha != s.alpha {
                        dirty = union(dirty, s.layer.content());
                        s.alpha = alpha;
                    }
                    let full = (0, 0, s.rect.2 as i64, s.rect.3 as i64);
                    if on != s.on {
                        dirty = Some(full);
                        s.on = on;
                    }
                    let inside = cursor.x >= s.rect.0 && cursor.y >= s.rect.1 && cursor.x < s.rect.0 + s.rect.2 as i32 && cursor.y < s.rect.1 + s.rect.3 as i32;
                    let hint_here = hint.as_ref().filter(|_| on && inside).map(|(_, img)| {
                        let (x, y) = super::hint_spot(s.rect, img.dimensions(), (cursor.x, cursor.y));
                        (img, x, y)
                    });
                    let hint_at = hint_here.map(|(img, x, y)| (x, y, x + img.width() as i64, y + img.height() as i64));
                    if hint_at != s.hint_at {
                        dirty = union(union(dirty, s.hint_at), hint_at);
                        s.hint_at = hint_at;
                    }
                    let has = on || s.layer.content().is_some() && alpha > 0.0;
                    if has && !s.shown {
                        dirty = Some(full);
                    }
                    if let Some(area) = dirty {
                        let n = (s.rect.2 * s.rect.3 * 4) as usize;
                        // SAFETY: the DIB section holds width x height 32-bit pixels.
                        let dst = std::slice::from_raw_parts_mut(s.bits, n);
                        super::compose(dst, s.rect.2, s.rect.3, s.layer.image(), alpha, area, on.then_some(accent), hint_here);
                        present(s, area);
                    }
                    if has != s.shown {
                        ShowWindow(s.hwnd, if has { SW_SHOWNOACTIVATE } else { SW_HIDE });
                        s.shown = has;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ink::{Layer, Map, Placed, Space, Stroke};

    #[test]
    fn the_screen_overlay_is_windows_only_for_now() {
        assert_eq!(support_for(true), Where::Screen);
        assert_eq!(support_for(false), Where::PreviewOnly, "macOS and Linux draw on the preview");
        assert_eq!(support(), support_for(cfg!(windows)));
    }

    #[test]
    fn the_hint_sits_top_centre_and_moves_away_from_the_pointer() {
        let m = (1920, 0, 1000, 800);
        assert_eq!(hint_spot(m, (200, 30), (2400, 500)), (400, 24));
        assert_eq!(hint_spot(m, (200, 30), (2400, 20)), (400, 800 - 30 - 24), "pointer near the top: bottom");
        assert!(hint_picture("Drawing").width() > 24);
    }

    #[test]
    fn snagsters_window_rect_is_kept_for_the_overlay() {
        set_app_rect(Some((10, 20, 300, 200)));
        assert_eq!(app_rect(), Some((10, 20, 300, 200)));
        set_app_rect(None);
        assert_eq!(app_rect(), None);
    }

    /// The overlay picture: the ink at the fade's strength (premultiplied, blue first), a
    /// border while drawing, the hint on top, and only the area asked for.
    #[test]
    fn the_overlay_picture_is_composed_from_the_ink_layer() {
        let rect = (100, 0, 40, 30);
        let mut map = Map::placed(&Placed { desk: rect, canvas: (0, 0, 40, 30) }, &image::RgbaImage::new(40, 30));
        map.clip = (0, 0, 40, 30);
        let mut layer = Layer::new(40, 30, map);
        let stroke = Stroke { space: Space::Desktop, points: vec![[110.0, 15.0], [130.0, 15.0]], color: [255, 0, 0, 255], width: 4.0 };
        layer.update([&stroke].into_iter());
        let mut dst = vec![0u8; 40 * 30 * 4];
        let px = |d: &[u8], x: usize, y: usize| d[(y * 40 + x) * 4..(y * 40 + x) * 4 + 4].to_vec();
        compose(&mut dst, 40, 30, layer.image(), 1.0, (0, 0, 40, 30), None, None);
        assert_eq!(px(&dst, 20, 15), [0, 0, 255, 255], "desktop (120, 15) is (20, 15) here, red as blue-green-red");
        assert_eq!(px(&dst, 0, 0), [0, 0, 0, 0], "no border while Draw is off");
        compose(&mut dst, 40, 30, layer.image(), 0.5, (0, 0, 40, 30), None, None);
        assert_eq!(px(&dst, 20, 15), [0, 0, 128, 128], "half faded, premultiplied");
        let hint = image::RgbaImage::from_pixel(10, 4, image::Rgba([9, 9, 9, 255]));
        compose(&mut dst, 40, 30, layer.image(), 1.0, (0, 0, 40, 30), Some([0, 0, 255, 255]), Some((&hint, 15, 2)));
        assert_eq!(px(&dst, 0, 0), [255, 0, 0, 255], "a blue border while drawing");
        assert_eq!(px(&dst, 39, 29), [255, 0, 0, 255]);
        assert_eq!(px(&dst, 16, 3), [9, 9, 9, 255], "the hint on top");
        // Only the area asked for changes.
        let before = dst.clone();
        compose(&mut dst, 40, 30, layer.image(), 0.0, (30, 0, 40, 30), None, None);
        assert_eq!(px(&dst, 20, 15), px(&before, 20, 15));
        assert_eq!(px(&dst, 39, 29), [0, 0, 0, 0]);
    }
}
