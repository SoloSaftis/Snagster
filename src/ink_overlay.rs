//! Live ink over the desktop: where the person draws while streaming, and where they see the
//! ink. The strokes themselves are in `ink`; this shows them on every monitor and, while Draw
//! is on, turns the mouse into the pen.
//!
//! - Windows: a see-through native window per monitor (a layered window filled from the same
//!   pictures `ink::paint` draws), kept out of screen captures so the stream gets the ink only
//!   once, drawn into the composed frame. Clicks pass through while Draw is off.
//! - macOS and Linux on X11: see-through always-on-top windows of the app (`app::ink_ui`), with
//!   clicks passing through while Draw is off.
//! - Linux on Wayland: the desktop doesn't let a program place a window over the screen, so ink
//!   is drawn on the Studio preview there.

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
    support_for(crate::capture::desktop_screenshots_only())
}

fn support_for(wayland: bool) -> Where {
    if wayland { Where::PreviewOnly } else { Where::Screen }
}

/// The hint shown while drawing (on the screen the pointer is on, never captured).
pub const HINT: &str = "Drawing. Press Esc to stop, Ctrl+Z to undo.";

/// The box a hint of `hint` size goes in on a monitor at `m` (x, y, w, h): top centre, or the
/// bottom when the pointer is near the top.
pub fn hint_spot(m: (i32, i32, u32, u32), hint: (u32, u32), pointer: (i32, i32)) -> (i32, i32) {
    let x = m.0 + (m.2 as i32 - hint.0 as i32) / 2;
    let near_top = pointer.1 < m.1 + hint.1 as i32 + 60;
    let y = if near_top { m.1 + m.3 as i32 - hint.1 as i32 - 24 } else { m.1 + 24 };
    (x, y)
}

/// Draw what the overlay of monitor `m` shows into `out` (the monitor's size, see-through):
/// the ink on it, and while drawing a thin border and, on the pointer's monitor, the hint.
pub fn render(out: &mut image::RgbaImage, m: (i32, i32, u32, u32), ink: &crate::ink::Snapshot, on: bool, hint: Option<&image::RgbaImage>, pointer: (i32, i32), accent: [u8; 4]) {
    for p in out.pixels_mut() {
        *p = image::Rgba([0, 0, 0, 0]);
    }
    let mine = crate::ink::Placed { desk: m, canvas: (0, 0, m.2, m.3) };
    crate::ink::paint_placed(out, ink, &mine);
    if !on {
        return;
    }
    let (w, h) = out.dimensions();
    for x in 0..w {
        for y in [0, 1, h.saturating_sub(2), h.saturating_sub(1)] {
            out.put_pixel(x, y.min(h - 1), image::Rgba(accent));
        }
    }
    for y in 0..h {
        for x in [0, 1, w.saturating_sub(2), w.saturating_sub(1)] {
            out.put_pixel(x.min(w - 1), y, image::Rgba(accent));
        }
    }
    let inside = pointer.0 >= m.0 && pointer.1 >= m.1 && pointer.0 < m.0 + m.2 as i32 && pointer.1 < m.1 + m.3 as i32;
    if let Some(hint) = hint
        && inside
    {
        let (x, y) = hint_spot(m, hint.dimensions(), pointer);
        image::imageops::overlay(out, hint, (x - m.0) as i64, (y - m.1) as i64);
    }
}

/// The hint as a picture: white text on a dark rounded-off box.
pub fn hint_picture(text: &str) -> image::RgbaImage {
    let t = crate::render::text_image(text, 18.0, [255, 255, 255, 255]);
    let (tw, th) = t.dimensions();
    let mut out = image::RgbaImage::from_pixel(tw + 24, th + 8, image::Rgba([20, 22, 28, 220]));
    image::imageops::overlay(&mut out, &t, 12, 4);
    out
}

#[cfg(windows)]
pub use win::run;

/// Nothing to start here: the app draws the overlay itself (`app::ink_ui`).
#[cfg(not(windows))]
pub fn run() {}

#[cfg(windows)]
mod win {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HBITMAP, HDC, ReleaseDC,
        SelectObject,
    };
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

    struct Screen {
        hwnd: HWND,
        rect: (i32, i32, u32, u32),
        dc: HDC,
        bmp: HBITMAP,
        bits: *mut u8,
        picture: image::RgbaImage,
        shown: bool,
        /// The overlay showed something last time (it must be cleared once ink is gone).
        drawn: bool,
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
                        i.begin(crate::ink::Space::Desktop, p, Instant::now());
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

    unsafe fn make(rect: (i32, i32, u32, u32)) -> Option<Screen> {
        unsafe {
            let ex = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT;
            let hwnd = CreateWindowExW(ex, CLASS.as_ptr(), CLASS.as_ptr(), WS_POPUP, rect.0, rect.1, rect.2 as i32, rect.3 as i32, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
            if hwnd.is_null() {
                return None;
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, (((rect.0 as i64) << 32) | (rect.1 as u32 as i64)) as isize);
            // Never in a screen capture: the stream gets the ink from the composed frame.
            SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
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
            Some(Screen { hwnd, rect, dc, bmp, bits: bits as *mut u8, picture: image::RgbaImage::new(rect.2, rect.3), shown: false, drawn: false })
        }
    }

    unsafe fn drop_screen(s: Screen) {
        unsafe {
            DestroyWindow(s.hwnd);
            DeleteDC(s.dc);
            DeleteObject(s.bmp);
        }
    }

    /// Copy the picture into the window (premultiplied blue-green-red-alpha, as Windows wants).
    unsafe fn present(s: &mut Screen) {
        let n = (s.rect.2 * s.rect.3 * 4) as usize;
        // SAFETY: the DIB section holds width x height 32-bit pixels.
        let dst = unsafe { std::slice::from_raw_parts_mut(s.bits, n) };
        for (d, p) in dst.chunks_exact_mut(4).zip(s.picture.as_raw().chunks_exact(4)) {
            let a = p[3] as u32;
            d[0] = (p[2] as u32 * a / 255) as u8;
            d[1] = (p[1] as u32 * a / 255) as u8;
            d[2] = (p[0] as u32 * a / 255) as u8;
            d[3] = p[3];
        }
        let pos = POINT { x: s.rect.0, y: s.rect.1 };
        let size = SIZE { cx: s.rect.2 as i32, cy: s.rect.3 as i32 };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        unsafe { UpdateLayeredWindow(s.hwnd, std::ptr::null_mut(), &pos, &size, s.dc, &src, 0, &blend, ULW_ALPHA) };
    }

    unsafe fn set_clicks(s: &Screen, take: bool) {
        unsafe {
            let ex = GetWindowLongPtrW(s.hwnd, GWL_EXSTYLE) as u32;
            let want = if take { ex & !WS_EX_TRANSPARENT } else { ex | WS_EX_TRANSPARENT };
            if want != ex {
                SetWindowLongPtrW(s.hwnd, GWL_EXSTYLE, want as isize);
            }
        }
    }

    fn pressed(vk: u16) -> bool {
        // SAFETY: reads the key state.
        (unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000) != 0
    }

    unsafe fn main_loop() {
        unsafe {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(proc),
                lpszClassName: CLASS.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassExW(&wc);
            let mut screens: Vec<Screen> = Vec::new();
            let mut listed_at: Option<Instant> = None;
            let mut seen = (u64::MAX, false);
            let mut hint: Option<(String, image::RgbaImage)> = None;
            let (mut esc, mut undo) = (false, false);
            let accent = crate::theme::system_accent().unwrap_or(crate::theme::BRAND).to_array();
            loop {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                let now = Instant::now();
                let (on, generation, fading, snap) = crate::ink::with(|i| (i.on, i.generation(), i.fading(now), i.snapshot(now)));
                let want = on || !snap.is_empty();
                if !want && !screens.iter().any(|s| s.shown) {
                    std::thread::sleep(Duration::from_millis(40));
                    continue;
                }
                // The monitors, again every few seconds (one may have been plugged in).
                if listed_at.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
                    listed_at = Some(Instant::now());
                    let rects: Vec<(i32, i32, u32, u32)> = crate::capture::monitors().iter().map(|m| (m.x, m.y, m.width, m.height)).filter(|r| r.2 > 0 && r.3 > 0).collect();
                    if rects != screens.iter().map(|s| s.rect).collect::<Vec<_>>() {
                        for s in screens.drain(..) {
                            drop_screen(s);
                        }
                        screens = rects.into_iter().filter_map(|r| make(r)).collect();
                        seen.0 = u64::MAX;
                    }
                }
                // Keys while drawing: Esc stops, Ctrl+Z takes back the last stroke.
                if on {
                    let e = pressed(VK_ESCAPE);
                    if e && !esc {
                        crate::ink::with(|i| i.set_on(false, Instant::now()));
                    }
                    esc = e;
                    let z = pressed(VK_CONTROL) && pressed(b'Z' as u16);
                    if z && !undo {
                        crate::ink::with(|i| i.undo(Instant::now()));
                    }
                    undo = z;
                }
                let mut cursor = POINT { x: 0, y: 0 };
                GetCursorPos(&mut cursor);
                let text = crate::i18n::t(super::HINT).to_string();
                if hint.as_ref().is_none_or(|(t, _)| *t != text) {
                    hint = Some((text.clone(), super::hint_picture(&text)));
                }
                // Redraw when the ink changed, while it fades, and while drawing (the hint
                // follows the pointer's monitor).
                if generation != seen.0 || on != seen.1 || fading || on {
                    seen = (generation, on);
                    for s in &mut screens {
                        set_clicks(s, on);
                        let mine = crate::ink::Placed { desk: s.rect, canvas: (0, 0, s.rect.2, s.rect.3) };
                        let (l, t, r, b) = (mine.desk.0 as f32, mine.desk.1 as f32, (mine.desk.0 + mine.desk.2 as i32) as f32, (mine.desk.1 + mine.desk.3 as i32) as f32);
                        let ink_here = !snap.is_empty()
                            && snap.in_space(crate::ink::Space::Desktop).any(|k| {
                                let kb = k.bounds();
                                kb.2 >= l && kb.3 >= t && kb.0 <= r && kb.1 <= b
                            });
                        let has = on || ink_here;
                        if has || s.drawn {
                            super::render(&mut s.picture, s.rect, &snap, on, hint.as_ref().map(|h| &h.1), (cursor.x, cursor.y), accent);
                            present(s);
                            s.drawn = has;
                        }
                        if has != s.shown {
                            ShowWindow(s.hwnd, if has { SW_SHOWNOACTIVATE } else { SW_HIDE });
                            s.shown = has;
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(if on || fading { 16 } else { 40 }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ink::{Snapshot, Space, Stroke};

    #[test]
    fn wayland_draws_on_the_preview_only() {
        assert_eq!(support_for(true), Where::PreviewOnly);
        assert_eq!(support_for(false), Where::Screen);
    }

    #[test]
    fn the_hint_sits_top_centre_and_moves_away_from_the_pointer() {
        let m = (1920, 0, 1000, 800);
        assert_eq!(hint_spot(m, (200, 30), (2400, 500)), (1920 + 400, 24));
        assert_eq!(hint_spot(m, (200, 30), (2400, 20)), (1920 + 400, 800 - 30 - 24), "pointer near the top: bottom");
    }

    #[test]
    fn the_overlay_shows_the_ink_on_its_monitor_and_a_border_while_drawing() {
        let m = (100, 0, 40, 30);
        let ink = Snapshot { strokes: vec![Stroke { space: Space::Desktop, points: vec![[110.0, 15.0], [130.0, 15.0]], color: [255, 0, 0, 255], width: 4.0 }], alpha: 1.0 };
        let mut out = image::RgbaImage::new(40, 30);
        render(&mut out, m, &ink, false, None, (0, 0), [0, 0, 255, 255]);
        assert_eq!(out.get_pixel(20, 15).0, [255, 0, 0, 255], "desktop (120, 15) is (20, 15) here");
        assert_eq!(out.get_pixel(0, 0).0, [0, 0, 0, 0], "no border while Draw is off");
        assert_eq!(out.get_pixel(20, 5).0, [0, 0, 0, 0], "see-through elsewhere");
        let hint = image::RgbaImage::from_pixel(10, 4, image::Rgba([9, 9, 9, 255]));
        render(&mut out, m, &ink, true, Some(&hint), (120, 25), [0, 0, 255, 255]);
        assert_eq!(out.get_pixel(0, 0).0, [0, 0, 255, 255], "a border while drawing");
        assert_eq!(out.get_pixel(39, 29).0, [0, 0, 255, 255]);
        // A small monitor: the pointer is always near the top, so the hint is at the bottom.
        assert_eq!(out.get_pixel(15, 3).0, [9, 9, 9, 255], "the hint, centred");
        // The hint is only on the pointer's monitor.
        render(&mut out, m, &ink, true, Some(&hint), (5, 5), [0, 0, 255, 255]);
        assert_eq!(out.get_pixel(15, 3).0, [0, 0, 0, 0]);
    }
}
