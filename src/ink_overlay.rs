//! Live ink over the desktop: where the person draws while streaming, and where they see the
//! ink. The strokes themselves are in `ink`; this shows them on every monitor and, while Draw
//! is on, turns the mouse into the pen.
//!
//! Each monitor gets a see-through native window filled from an `ink::Layer` (`Paint`), only
//! where something changed. Clicks pass through while Draw is off; while it is on, Snagster's
//! own window stays usable (it is left out of the overlay) and Esc and Ctrl+Z (⌘Z) go to the
//! ink, not to the program in front.
//!
//! - Windows: a layered window per monitor, kept out of screen captures, so the stream gets the
//!   ink once, drawn into the composed frame.
//! - macOS: a borderless panel per screen that never activates Snagster, at the screen saver's
//!   level and on every Space, with `sharingType = none` (kept out of screen captures). Its
//!   picture is a grid of image tiles at the screen's own resolution, so a stroke only sends the
//!   tiles it crosses.
//! - X11: an override-redirect window with a 32-bit (see-through) visual per monitor; its input
//!   shape is empty while Draw is off, so clicks go through. X11 can't keep a window out of
//!   screen copies, so the copies hold the ink already (`overlay_in_captures`), and the overlay
//!   draws only the ink (no border, no hint), which would otherwise be in the stream too. Without
//!   a compositing manager a see-through window would be black, so ink stays on the preview.
//! - Wayland: the desktop doesn't let a program cover the screen; ink is drawn on the Studio
//!   preview.

use crate::ink::{Area, Layer, Map, Placed, Snapshot, Space, union};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(target_os = "linux")]
mod x11;

#[cfg(target_os = "macos")]
mod mac;

/// Where ink can be drawn on this system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    /// Over the screen (and on the Studio preview).
    Screen,
    /// Only on the Studio preview.
    PreviewOnly,
}

/// The window system, as far as the overlay is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum System {
    Windows,
    Mac,
    /// An X11 desktop; `compositor` when a compositing manager blends see-through windows (and
    /// the server has the 32-bit visual and the SHAPE extension the overlay needs).
    X11 { compositor: bool },
    /// Wayland (Linux): a program can't place a window over the screen.
    Wayland,
}

/// Where ink can be drawn here.
pub fn support() -> Where {
    support_for(system())
}

fn support_for(s: System) -> Where {
    match s {
        System::Windows | System::Mac | System::X11 { compositor: true } => Where::Screen,
        System::X11 { compositor: false } | System::Wayland => Where::PreviewOnly,
    }
}

fn system() -> System {
    if cfg!(windows) {
        System::Windows
    } else if cfg!(target_os = "macos") {
        System::Mac
    } else if crate::capture::desktop_screenshots_only() {
        System::Wayland
    } else {
        System::X11 { compositor: x11_can_overlay() }
    }
}

#[cfg(target_os = "linux")]
fn x11_can_overlay() -> bool {
    x11::can_overlay()
}

#[cfg(not(target_os = "linux"))]
fn x11_can_overlay() -> bool {
    false
}

/// The screen copies hold the overlay's ink already (Windows before 10 version 2004, where the
/// overlay can't be kept out of captures, and X11 while the overlay is shown), so the engine
/// doesn't draw desktop ink a second time.
static IN_CAPTURES: AtomicBool = AtomicBool::new(false);

pub fn overlay_in_captures() -> bool {
    IN_CAPTURES.load(Ordering::Relaxed)
}

/// Every screen copy sees the overlay's ink, the Record tab's videos and replays included
/// (X11, and Windows where the overlay can't be kept out of captures).
pub fn ink_in_screen_copies() -> bool {
    in_screen_copies(system()) || overlay_in_captures()
}

fn in_screen_copies(s: System) -> bool {
    s == System::X11 { compositor: true }
}

/// The part of the overlay left out while drawing: Snagster's window, only while it is in front
/// and shown (in the tray, minimized or behind another program, its old place is drawn on like
/// the rest of the screen, and clicks there never reach the program in front).
fn cut_out(front_is_ours: bool, shown: bool, rect: Option<(i32, i32, u32, u32)>) -> Option<(i32, i32, u32, u32)> {
    rect.filter(|r| front_is_ours && shown && r.2 > 0 && r.3 > 0)
}

/// Whether the overlay takes a key while Draw is on: Esc, and Ctrl+Z (⌘Z on a Mac) without
/// Shift or Alt (Ctrl+Shift+Z is redo elsewhere). When Snagster is in front its own window
/// handles them (and Esc closes an open menu there first).
fn hook_takes(front_is_ours: bool, esc: bool, z: bool, ctrl: bool, shift: bool, alt: bool) -> bool {
    !front_is_ours && (esc || (z && ctrl && !shift && !alt))
}

/// The hint shown while drawing (on the screen the pointer is on, never captured).
pub const HINT: &str = "Drawing. Press Esc to stop, Ctrl+Z to undo.";

/// The hint in the interface language, with this system's keys.
#[cfg(any(windows, target_os = "macos"))]
fn hint_text() -> String {
    crate::platform::keys(crate::i18n::t(HINT))
}

/// Where the hint (`hint` in size) goes on a monitor's picture, `size` pixels big, with the
/// pointer at `pointer` (picture pixels from the monitor's top-left corner) and `scale` picture
/// pixels to a desktop unit: top centre, or the bottom when the pointer is near the top.
fn hint_spot(size: (u32, u32), hint: (u32, u32), pointer: (f32, f32), scale: f32) -> (i64, i64) {
    let x = (size.0 as i64 - hint.0 as i64) / 2;
    let gap = (24.0 * scale).round() as i64;
    let near_top = pointer.1 < hint.1 as f32 + 60.0 * scale;
    let y = if near_top { size.1 as i64 - hint.1 as i64 - gap } else { gap };
    (x, y)
}

/// The hint as a picture: white text on a dark box, `scale` times the size of a plain screen's.
fn hint_picture(text: &str, scale: f32) -> image::RgbaImage {
    let t = crate::render::text_image(text, 18.0 * scale, [255, 255, 255, 255]);
    let (tw, th) = t.dimensions();
    let (px, py) = ((12.0 * scale).round() as u32, (4.0 * scale).round() as u32);
    let mut out = image::RgbaImage::from_pixel(tw + 2 * px, th + 2 * py, image::Rgba([20, 22, 28, 220]));
    image::imageops::overlay(&mut out, &t, px as i64, py as i64);
    out
}

/// `a` inside a picture `size` pixels big; None when nothing of it is.
fn clip(a: Area, size: (u32, u32)) -> Option<Area> {
    let c = (a.0.max(0), a.1.max(0), a.2.min(size.0 as i64), a.3.min(size.1 as i64));
    (c.0 < c.2 && c.1 < c.3).then_some(c)
}

/// Fill `area` of a monitor's overlay picture `dst` (`w` pixels wide, blue-green-red-alpha with
/// premultiplied alpha, as Windows, X11's 32-bit visuals and macOS's little-endian bitmaps want
/// it) from the ink layer at `alpha`, with a 2-pixel `border` round the monitor, the `hint`
/// picture at its spot on top, and `hole` left clear.
#[allow(clippy::too_many_arguments)]
fn compose(dst: &mut [u8], w: u32, h: u32, layer: &image::RgbaImage, alpha: f32, area: Area, border: Option<[u8; 4]>, hint: Option<(&image::RgbaImage, i64, i64)>, hole: Option<Area>) {
    let (x0, y0) = (area.0.max(0), area.1.max(0));
    let (x1, y1) = (area.2.min(w as i64), area.3.min(h as i64));
    let src = layer.as_raw();
    let a_mul = alpha.clamp(0.0, 1.0);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y as usize * w as usize + x as usize) * 4;
            if let Some((hx0, hy0, hx1, hy1)) = hole
                && x >= hx0
                && y >= hy0
                && x < hx1
                && y < hy1
            {
                dst[i..i + 4].fill(0);
                continue;
            }
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

/// `area` of a picture `w` pixels wide (4 bytes a pixel), row after row.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn crop(src: &[u8], w: u32, area: Area) -> Vec<u8> {
    let (x0, y0, x1, y1) = area;
    let row = (x1 - x0) as usize * 4;
    let mut out = Vec::with_capacity(row * (y1 - y0) as usize);
    for y in y0..y1 {
        let at = (y as usize * w as usize + x0 as usize) * 4;
        out.extend_from_slice(&src[at..at + row]);
    }
    out
}

/// One monitor's overlay picture and what it shows now; the window that shows it belongs to
/// each system's own code.
struct Paint {
    /// The monitor on the desktop, in desktop units (pixels; points on macOS).
    rect: (i32, i32, u32, u32),
    /// Picture pixels to a desktop unit (2 on a Retina screen).
    scale: f32,
    /// The picture's size in pixels.
    size: (u32, u32),
    layer: Layer,
    /// Whether the window is shown; the system's code keeps it up to date.
    shown: bool,
    /// What the picture shows now: the ink's strength, Draw on, the hint's box, the part left
    /// clear for Snagster's window.
    alpha: f32,
    on: bool,
    hint_at: Option<Area>,
    hole: Option<Area>,
    /// The hint's text and picture, made at this monitor's scale.
    hint: Option<(String, image::RgbaImage)>,
    /// The window system lost the picture (X11's Expose): draw it all again.
    redraw: bool,
}

impl Paint {
    fn new(rect: (i32, i32, u32, u32), scale: f32) -> Paint {
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let size = (((rect.2 as f32 * scale).round() as u32).max(1), ((rect.3 as f32 * scale).round() as u32).max(1));
        let placed = Placed { desk: rect, canvas: (0, 0, size.0, size.1) };
        let probe = image::RgbaImage::new(1, 1);
        let mut map = Map::placed(&placed, &probe);
        map.clip = (0, 0, size.0 as i64, size.1 as i64);
        Paint { rect, scale, size, layer: Layer::new(size.0, size.1, map), shown: false, alpha: 1.0, on: false, hint_at: None, hole: None, hint: None, redraw: false }
    }

    /// The bytes the picture needs.
    fn bytes(&self) -> usize {
        self.size.0 as usize * self.size.1 as usize * 4
    }

    /// Bring the picture `dst` (`bytes()` long, blue-green-red-alpha premultiplied) up to the
    /// ink in `snap`. `marks`: the border's colour and the hint's text, shown while Draw is on
    /// (None where the overlay is in screen copies); `pointer` and `hole` (left clear) are in
    /// desktop units. Returns the part of the picture that changed and whether the window
    /// should be shown.
    fn step(&mut self, snap: &Snapshot, on: bool, marks: Option<([u8; 4], &str)>, pointer: (f32, f32), hole: Option<(i32, i32, u32, u32)>, dst: &mut [u8]) -> (Option<Area>, bool) {
        let (x, y, w, h) = self.rect;
        let (l, t, r, b) = (x as f32, y as f32, (x + w as i32) as f32, (y + h as i32) as f32);
        let mine = snap.in_space(Space::Desktop).filter(|k| {
            let kb = k.bounds();
            kb.2 >= l && kb.3 >= t && kb.0 <= r && kb.1 <= b
        });
        let mut dirty = self.layer.update(mine);
        let alpha = if snap.is_empty() { 0.0 } else { snap.alpha };
        if alpha != self.alpha {
            dirty = union(dirty, self.layer.content());
            self.alpha = alpha;
        }
        let full = (0, 0, self.size.0 as i64, self.size.1 as i64);
        if on != self.on {
            if marks.is_some() {
                dirty = Some(full);
            }
            self.on = on;
        }
        let s = self.scale;
        let inside = pointer.0 >= l && pointer.1 >= t && pointer.0 < r && pointer.1 < b;
        let text = marks.filter(|_| on && inside).map(|m| m.1);
        if let Some(text) = text
            && self.hint.as_ref().is_none_or(|(t, _)| t != text)
        {
            self.hint = Some((text.to_string(), hint_picture(text, s)));
        }
        let at = ((pointer.0 - l) * s, (pointer.1 - t) * s);
        let hint_here = text.and(self.hint.as_ref()).map(|(_, img)| {
            let (hx, hy) = hint_spot(self.size, img.dimensions(), at, s);
            (img, hx, hy)
        });
        let hint_at = hint_here.map(|(img, hx, hy)| (hx, hy, hx + img.width() as i64, hy + img.height() as i64));
        if hint_at != self.hint_at {
            dirty = union(union(dirty, self.hint_at), hint_at);
            self.hint_at = hint_at;
        }
        let hole = hole.and_then(|(hx, hy, hw, hh)| {
            let (dx, dy) = ((hx - x) as f32 * s, (hy - y) as f32 * s);
            clip((dx.floor() as i64, dy.floor() as i64, (dx + hw as f32 * s).ceil() as i64, (dy + hh as f32 * s).ceil() as i64), self.size)
        });
        if hole != self.hole {
            dirty = union(union(dirty, self.hole), hole);
            self.hole = hole;
        }
        let has = on || self.layer.content().is_some() && alpha > 0.0;
        if has && !self.shown || std::mem::take(&mut self.redraw) {
            dirty = Some(full);
        }
        let dirty = dirty.and_then(|a| clip(a, self.size));
        if let Some(area) = dirty {
            compose(dst, self.size.0, self.size.1, self.layer.image(), alpha, area, marks.filter(|_| on).map(|m| m.0), hint_here, self.hole);
        }
        (dirty, has)
    }
}

/// The rectangles of a monitor's overlay (`size` pixels, at the window's top-left) that take
/// clicks on X11: none while Draw is off, else all of it but `hole`.
#[cfg(any(target_os = "linux", test))]
fn input_rects(size: (u32, u32), take: bool, hole: Option<Area>) -> Vec<(i16, i16, u16, u16)> {
    if !take {
        return Vec::new();
    }
    let (w, h) = (size.0.min(i16::MAX as u32) as i64, size.1.min(i16::MAX as u32) as i64);
    let rect = |x0: i64, y0: i64, x1: i64, y1: i64| (x0 < x1 && y0 < y1).then(|| (x0 as i16, y0 as i16, (x1 - x0) as u16, (y1 - y0) as u16));
    let Some((x0, y0, x1, y1)) = hole.and_then(|a| clip(a, (w as u32, h as u32))) else {
        return rect(0, 0, w, h).into_iter().collect();
    };
    // Above the hole, below it, and left and right of it.
    [rect(0, 0, w, y0), rect(0, y1, w, h), rect(0, y0, x0, y1), rect(x1, y0, w, y1)].into_iter().flatten().collect()
}

/// The rows of `area` in pieces that fit a request of at most `max` bytes, each row `row`
/// bytes (X11 requests have a size limit; a whole screen is bigger).
#[cfg(any(target_os = "linux", test))]
fn bands(area: Area, row: usize, max: usize) -> Vec<(i64, i64)> {
    let per = (max / row.max(1)).max(1) as i64;
    let mut out = Vec::new();
    let mut y = area.1;
    while y < area.3 {
        let end = (y + per).min(area.3);
        out.push((y, end));
        y = end;
    }
    out
}

/// The keycode that types `sym` in an X keyboard map (`per` keysyms for each keycode from
/// `min`).
#[cfg(any(target_os = "linux", test))]
fn keycode_for(syms: &[u32], per: usize, min: u8, sym: u32) -> Option<u8> {
    syms.chunks(per.max(1)).position(|c| c.contains(&sym)).and_then(|i| u8::try_from(min as usize + i).ok())
}

/// A rectangle in macOS's screen coordinates (x, y, w, h; y from the bottom of the main screen,
/// whose height is `main_h`) on the desktop (y from the top, as Core Graphics and the screen
/// copies count it).
#[cfg(any(target_os = "macos", test))]
fn desk_from_cocoa(r: (f64, f64, f64, f64), main_h: f64) -> (i32, i32, u32, u32) {
    (r.0.round() as i32, (main_h - r.1 - r.3).round() as i32, r.2.round().max(0.0) as u32, r.3.round().max(0.0) as u32)
}

/// Whether the desktop point `p` is in `r` (x, y, w, h).
#[cfg(any(target_os = "macos", test))]
fn inside(r: (i32, i32, u32, u32), p: [f32; 2]) -> bool {
    p[0] >= r.0 as f32 && p[1] >= r.1 as f32 && p[0] < (r.0 + r.2 as i32) as f32 && p[1] < (r.1 + r.3 as i32) as f32
}

/// A desktop rectangle in macOS's screen coordinates.
#[cfg(any(target_os = "macos", test))]
fn cocoa_from_desk(r: (i32, i32, u32, u32), main_h: f64) -> (f64, f64, f64, f64) {
    (r.0 as f64, main_h - r.1 as f64 - r.3 as f64, r.2 as f64, r.3 as f64)
}

/// A point in macOS's screen coordinates on the desktop.
#[cfg(any(target_os = "macos", test))]
fn desk_point(p: (f64, f64), main_h: f64) -> [f32; 2] {
    [p.0 as f32, (main_h - p.1) as f32]
}

/// The tiles (`tile` pixels square, from the top-left) of a picture `size` big that `area`
/// touches, each cut to the picture.
#[cfg(any(target_os = "macos", test))]
fn tiles_touching(area: Area, size: (u32, u32), tile: u32) -> Vec<Area> {
    let Some((x0, y0, x1, y1)) = clip(area, size) else { return Vec::new() };
    let t = tile.max(1) as i64;
    let mut out = Vec::new();
    for ty in (y0 / t)..=((y1 - 1) / t) {
        for tx in (x0 / t)..=((x1 - 1) / t) {
            out.extend(clip((tx * t, ty * t, tx * t + t, ty * t + t), size));
        }
    }
    out
}

/// Where a tile of a picture `size` pixels big goes in its window, in points from the bottom
/// left (an unflipped macOS view): x, y, w, h.
#[cfg(any(target_os = "macos", test))]
fn tile_frame(tile: Area, size: (u32, u32), scale: f32) -> (f64, f64, f64, f64) {
    let s = scale as f64;
    (tile.0 as f64 / s, (size.1 as i64 - tile.3) as f64 / s, (tile.2 - tile.0) as f64 / s, (tile.3 - tile.1) as f64 / s)
}

#[cfg(windows)]
pub use win::run;

#[cfg(target_os = "linux")]
pub use x11::run;

#[cfg(target_os = "macos")]
pub use mac::run;

/// Nothing to start here: ink is drawn on the Studio preview.
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn run() {}

#[cfg(windows)]
mod win {
    use super::Paint;
    use crate::ink::{Area, Space};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, CombineRgn, CreateCompatibleDC, CreateDIBSection, CreateRectRgn, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
        HBITMAP, HDC, RGN_DIFF, ReleaseDC, SelectObject, SetWindowRgn,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_ESCAPE, VK_MENU, VK_SHIFT};
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
        dc: HDC,
        bmp: HBITMAP,
        bits: *mut u8,
        paint: Paint,
        /// The cut-out for Snagster's window.
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
            let (esc, z) = (vk == VK_ESCAPE as u32, vk == b'Z' as u32);
            if (esc || z) && crate::ink::with(|i| i.on) {
                let down = |k: u16| (unsafe { GetAsyncKeyState(k as i32) } as u16 & 0x8000) != 0;
                let (ours, shown, _) = unsafe { front_is_ours() };
                if super::hook_takes(ours && shown, esc, z, down(VK_CONTROL), down(VK_SHIFT), down(VK_MENU)) {
                    let now = Instant::now();
                    crate::ink::with(|i| if esc { i.set_on(false, now) } else { i.undo(now) });
                    return 1;
                }
            }
        }
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, wp, lp) }
    }

    /// Whether the window in front is one of Snagster's own (not an overlay), shown, and where.
    unsafe fn front_is_ours() -> (bool, bool, Option<(i32, i32, u32, u32)>) {
        unsafe {
            let fg = GetForegroundWindow();
            if fg.is_null() {
                return (false, false, None);
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(fg, &mut pid);
            let mut class = [0u16; 16];
            let n = GetClassNameW(fg, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
            let overlay = class[..n] == CLASS[..CLASS.len() - 1];
            let ours = pid == GetCurrentProcessId() && !overlay;
            let shown = IsWindowVisible(fg) != 0 && IsIconic(fg) == 0;
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            let rect = (GetWindowRect(fg, &mut r) != 0).then(|| (r.left, r.top, (r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32));
            (ours, shown, rect)
        }
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
            Some(Screen { hwnd, dc, bmp, bits: bits as *mut u8, paint: Paint::new(rect, 1.0), cut: None })
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
        let r = s.paint.rect;
        let pos = POINT { x: r.0, y: r.1 };
        let size = SIZE { cx: r.2 as i32, cy: r.3 as i32 };
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
            let r = s.paint.rect;
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
                        let all = CreateRectRgn(0, 0, r.2 as i32, r.3 as i32);
                        let hole = CreateRectRgn(x - r.0, y - r.1, x - r.0 + w as i32, y - r.1 + h as i32);
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
            loop {
                // The desktop's accent, read each turn (cached for two seconds), so a change shows.
                let accent = crate::theme::system_accent().unwrap_or(crate::theme::BRAND).to_array();
                // Wake for the mouse at once, else a few times a second.
                let (on, fading) = crate::ink::with(|i| (i.on, i.fading(Instant::now())));
                let wait = if on || fading { 16 } else if screens.iter().any(|s| s.paint.shown) { 50 } else { 120 };
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
                if !want && !screens.iter().any(|s| s.paint.shown) {
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
                // The monitors, again every few seconds (one may have been plugged in); not while
                // drawing, when this thread also answers every key press (the keyboard hook).
                if screens.is_empty() || (!on && listed_at.is_none_or(|t| t.elapsed() > Duration::from_secs(3))) {
                    listed_at = Some(now);
                    let rects: Vec<(i32, i32, u32, u32)> = crate::capture::monitors().iter().map(|m| (m.x, m.y, m.width, m.height)).filter(|r| r.2 > 0 && r.3 > 0).collect();
                    if rects != screens.iter().map(|s| s.paint.rect).collect::<Vec<_>>() {
                        for s in screens.drain(..) {
                            drop_screen(s);
                        }
                        screens = rects.into_iter().filter_map(|r| make(r)).collect();
                    }
                }
                let mut cursor = POINT { x: 0, y: 0 };
                GetCursorPos(&mut cursor);
                let text = super::hint_text();
                let (ours, shown, front) = front_is_ours();
                let app = super::cut_out(ours, shown, front);
                for s in &mut screens {
                    set_clicks(s, on, app);
                    // SAFETY: the DIB section holds the picture's width x height 32-bit pixels.
                    let dst = std::slice::from_raw_parts_mut(s.bits, s.paint.bytes());
                    // (Snagster's window is cut out of the window itself, `set_clicks`.)
                    let (dirty, has) = s.paint.step(&snap, on, Some((accent, &text)), (cursor.x as f32, cursor.y as f32), None, dst);
                    if let Some(area) = dirty {
                        present(s, area);
                    }
                    if has != s.paint.shown {
                        ShowWindow(s.hwnd, if has { SW_SHOWNOACTIVATE } else { SW_HIDE });
                        s.paint.shown = has;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ink::{Layer, Map, Placed, Snapshot, Space, Stroke};

    #[test]
    fn ink_goes_over_the_screen_where_the_window_system_can_blend_a_see_through_window() {
        assert_eq!(support_for(System::Windows), Where::Screen);
        assert_eq!(support_for(System::Mac), Where::Screen);
        assert_eq!(support_for(System::X11 { compositor: true }), Where::Screen);
        assert_eq!(support_for(System::X11 { compositor: false }), Where::PreviewOnly, "no compositor: a see-through window would be black");
        assert_eq!(support_for(System::Wayland), Where::PreviewOnly, "Wayland doesn't let a program cover the screen");
        if cfg!(any(windows, target_os = "macos")) {
            assert_eq!(support(), Where::Screen);
        }
        // X11 can't keep the overlay out of screen copies: the Record tab and replays get the
        // ink too. Windows and macOS keep it out.
        assert!(in_screen_copies(System::X11 { compositor: true }));
        for s in [System::Windows, System::Mac, System::X11 { compositor: false }, System::Wayland] {
            assert!(!in_screen_copies(s), "{s:?}");
        }
    }

    #[test]
    fn the_hint_sits_top_centre_and_moves_away_from_the_pointer() {
        let size = (1000, 800);
        assert_eq!(hint_spot(size, (200, 30), (480.0, 500.0), 1.0), (400, 24));
        assert_eq!(hint_spot(size, (200, 30), (480.0, 20.0), 1.0), (400, 800 - 30 - 24), "pointer near the top: bottom");
        // A Retina screen: twice the pixels, the same place on the glass.
        assert_eq!(hint_spot((2000, 1600), (400, 60), (960.0, 1000.0), 2.0), (800, 48));
        assert_eq!(hint_spot((2000, 1600), (400, 60), (960.0, 150.0), 2.0), (800, 1600 - 60 - 48));
        let small = hint_picture("Drawing", 1.0);
        let big = hint_picture("Drawing", 2.0);
        assert!(small.width() > 24);
        assert!(big.width() > small.width() * 3 / 2 && big.height() > small.height() * 3 / 2, "the hint is as big on a Retina screen");
    }

    /// Bug: Snagster's old place stayed cut out of the overlay while it was in the tray or
    /// behind the browser, so strokes there clicked the browser instead.
    #[test]
    fn snagsters_window_is_left_usable_only_while_in_front_and_shown() {
        let r = Some((10, 20, 300, 200));
        assert_eq!(cut_out(true, true, r), r);
        assert_eq!(cut_out(false, true, r), None, "another program in front");
        assert_eq!(cut_out(true, false, r), None, "minimized or in the tray");
        assert_eq!(cut_out(true, true, Some((0, 0, 0, 10))), None);
        assert_eq!(cut_out(true, true, None), None);
    }

    #[test]
    fn the_keyboard_hook_takes_only_esc_and_plain_ctrl_z_from_other_programs() {
        assert!(hook_takes(false, true, false, false, false, false), "Esc");
        assert!(hook_takes(false, false, true, true, false, false), "Ctrl+Z");
        assert!(!hook_takes(false, false, true, false, false, false), "Z alone is typing");
        assert!(!hook_takes(false, false, true, true, true, false), "Ctrl+Shift+Z is redo elsewhere");
        assert!(!hook_takes(false, false, true, true, false, true), "Ctrl+Alt+Z");
        assert!(!hook_takes(true, true, false, false, false, false), "Snagster in front handles Esc itself (an open menu first)");
        assert!(!hook_takes(true, false, true, true, false, false));
    }

    /// The overlay picture: the ink at the fade's strength (premultiplied, blue first), a
    /// border while drawing, the hint on top, a hole left clear, and only the area asked for.
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
        compose(&mut dst, 40, 30, layer.image(), 1.0, (0, 0, 40, 30), None, None, None);
        assert_eq!(px(&dst, 20, 15), [0, 0, 255, 255], "desktop (120, 15) is (20, 15) here, red as blue-green-red");
        assert_eq!(px(&dst, 0, 0), [0, 0, 0, 0], "no border while Draw is off");
        compose(&mut dst, 40, 30, layer.image(), 0.5, (0, 0, 40, 30), None, None, None);
        assert_eq!(px(&dst, 20, 15), [0, 0, 128, 128], "half faded, premultiplied");
        let hint = image::RgbaImage::from_pixel(10, 4, image::Rgba([9, 9, 9, 255]));
        compose(&mut dst, 40, 30, layer.image(), 1.0, (0, 0, 40, 30), Some([0, 0, 255, 255]), Some((&hint, 15, 2)), None);
        assert_eq!(px(&dst, 0, 0), [255, 0, 0, 255], "a blue border while drawing");
        assert_eq!(px(&dst, 39, 29), [255, 0, 0, 255]);
        assert_eq!(px(&dst, 16, 3), [9, 9, 9, 255], "the hint on top");
        compose(&mut dst, 40, 30, layer.image(), 1.0, (0, 0, 40, 30), Some([0, 0, 255, 255]), None, Some((15, 10, 25, 20)));
        assert_eq!(px(&dst, 20, 15), [0, 0, 0, 0], "Snagster's window is left clear");
        assert_eq!(px(&dst, 12, 15), [0, 0, 255, 255], "ink next to it is drawn");
        // Only the area asked for changes.
        let before = dst.clone();
        compose(&mut dst, 40, 30, layer.image(), 0.0, (30, 0, 40, 30), None, None, None);
        assert_eq!(px(&dst, 12, 15), px(&before, 12, 15));
        assert_eq!(px(&dst, 39, 29), [0, 0, 0, 0]);
    }

    fn line(points: Vec<[f32; 2]>) -> Stroke {
        Stroke { space: Space::Desktop, points, color: [255, 0, 0, 255], width: 4.0 }
    }

    /// What a monitor's overlay does as the ink changes: nothing shown with no ink, everything
    /// drawn at first show, nothing when nothing changed, only the new piece as a stroke grows,
    /// and hidden again when the ink has faded.
    #[test]
    fn a_monitors_picture_follows_the_ink() {
        let mut p = Paint::new((1000, 0, 200, 100), 1.0);
        let mut dst = vec![0u8; p.bytes()];
        let empty = Snapshot::default();
        assert_eq!(p.step(&empty, false, None, (0.0, 0.0), None, &mut dst), (None, false), "no ink, Draw off: hidden, nothing drawn");
        let snap = Snapshot { strokes: vec![line(vec![[1050.0, 50.0], [1100.0, 50.0]])], alpha: 1.0 };
        let (dirty, has) = p.step(&snap, false, None, (0.0, 0.0), None, &mut dst);
        assert!(has);
        assert_eq!(dirty, Some((0, 0, 200, 100)), "the first show draws the whole picture");
        p.shown = true;
        assert_eq!(dst[(50 * 200 + 75) * 4 + 2], 255, "desktop (1075, 50) is (75, 50) on this monitor");
        assert_eq!(p.step(&snap, false, None, (0.0, 0.0), None, &mut dst).0, None, "nothing changed");
        let grown = Snapshot { strokes: vec![line(vec![[1050.0, 50.0], [1100.0, 50.0], [1100.0, 90.0]])], alpha: 1.0 };
        let (dirty, _) = p.step(&grown, false, None, (0.0, 0.0), None, &mut dst);
        let d = dirty.expect("the new piece is drawn");
        assert!(d.0 >= 90 && d.2 <= 110 && d.3 <= 100, "only round the new piece: {d:?}");
        // A stroke on another monitor isn't this one's.
        let elsewhere = Snapshot { strokes: vec![grown.strokes[0].clone(), line(vec![[10.0, 10.0], [20.0, 20.0]])], alpha: 1.0 };
        assert_eq!(p.step(&elsewhere, false, None, (0.0, 0.0), None, &mut dst).0, None);
        let faded = Snapshot { strokes: grown.strokes.clone(), alpha: 0.0 };
        let (dirty, has) = p.step(&faded, false, None, (0.0, 0.0), None, &mut dst);
        assert!(!has, "faded away: hidden");
        assert!(dirty.is_some());
        assert_eq!(dst[(50 * 200 + 75) * 4 + 3], 0);
    }

    /// Draw on: the window is shown (empty), with the border and the hint where the overlay
    /// isn't in screen copies, and Snagster's window left clear; a lost picture (X11's Expose)
    /// is drawn again.
    #[test]
    fn drawing_shows_the_marks_and_leaves_snagsters_window_clear() {
        let mut p = Paint::new((0, 0, 400, 300), 1.0);
        let mut dst = vec![0u8; p.bytes()];
        let empty = Snapshot::default();
        let marks = Some(([0, 0, 255, 255], "Drawing"));
        let (dirty, has) = p.step(&empty, true, marks, (200.0, 200.0), None, &mut dst);
        assert!(has, "shown while drawing");
        assert_eq!(dirty, Some((0, 0, 400, 300)));
        p.shown = true;
        assert_eq!(&dst[0..4], &[255, 0, 0, 255], "the border");
        let hint = p.hint_at.expect("the hint is on the screen with the pointer");
        assert_eq!(hint.1, 24, "top centre");
        // The pointer on another monitor: no hint here.
        let (dirty, _) = p.step(&empty, true, marks, (900.0, 200.0), None, &mut dst);
        assert_eq!(dirty, Some(hint));
        assert_eq!(p.hint_at, None);
        // Snagster's window in front: left clear.
        let (dirty, _) = p.step(&empty, true, marks, (900.0, 200.0), Some((-10, 100, 60, 50)), &mut dst);
        assert_eq!(dirty, Some((0, 100, 50, 150)), "the hole, cut to the monitor");
        assert_eq!(&dst[(120 * 400) * 4..(120 * 400) * 4 + 4], &[0, 0, 0, 0], "the border isn't drawn over Snagster");
        // No marks (X11): nothing but ink in a picture the screen copies hold.
        let mut q = Paint::new((0, 0, 400, 300), 1.0);
        let mut qd = vec![0u8; q.bytes()];
        let (dirty, has) = q.step(&empty, true, None, (200.0, 200.0), None, &mut qd);
        assert!(has && dirty.is_some(), "first show");
        q.shown = true;
        assert!(qd.iter().all(|&b| b == 0), "no border, no hint");
        assert_eq!(q.step(&empty, false, None, (200.0, 200.0), None, &mut qd), (None, false), "Draw off redraws nothing");
        q.redraw = true;
        assert_eq!(q.step(&empty, true, None, (200.0, 200.0), None, &mut qd).0, Some((0, 0, 400, 300)), "Expose: all again");
    }

    /// A Retina screen: the picture has twice the pixels each way and the ink lands at the
    /// same place on the glass, as wide as on a plain screen.
    #[test]
    fn a_retina_screens_picture_has_its_own_pixels() {
        let mut p = Paint::new((-1440, 0, 1440, 900), 2.0);
        assert_eq!(p.size, (2880, 1800));
        let mut dst = vec![0u8; p.bytes()];
        let snap = Snapshot { strokes: vec![line(vec![[-1000.0, 100.0], [-900.0, 100.0]])], alpha: 1.0 };
        p.step(&snap, false, None, (0.0, 0.0), None, &mut dst);
        let a = |x: usize, y: usize| dst[(y * 2880 + x) * 4 + 3];
        assert_eq!(a(1000, 200), 255, "desktop (-940, 100) is (1000, 200) in pixels");
        assert_eq!(a(1000, 205), 0, "4 points wide is 8 pixels, centred on 200");
        assert!(a(1000, 197) > 0 && a(1000, 202) > 0);
        assert_eq!(Paint::new((0, 0, 10, 10), f32::NAN).scale, 1.0, "a nonsense scale is taken as 1");
    }

    #[test]
    fn x11_takes_clicks_everywhere_but_snagsters_window_and_only_while_drawing() {
        assert!(input_rects((1920, 1080), false, None).is_empty(), "Draw off: clicks go through");
        assert_eq!(input_rects((1920, 1080), true, None), vec![(0, 0, 1920, 1080)]);
        let r = input_rects((1920, 1080), true, Some((100, 200, 600, 700)));
        assert_eq!(r, vec![(0, 0, 1920, 200), (0, 700, 1920, 380), (0, 200, 100, 500), (600, 200, 1320, 500)]);
        let covered: u64 = r.iter().map(|q| q.2 as u64 * q.3 as u64).sum();
        assert_eq!(covered, 1920 * 1080 - 500 * 500, "the rest of the screen, once");
        assert_eq!(input_rects((1920, 1080), true, Some((0, 0, 1920, 1080))), vec![], "Snagster covers the screen");
        assert_eq!(input_rects((1920, 1080), true, Some((0, 0, 1920, 100))), vec![(0, 100, 1920, 980)]);
    }

    #[test]
    fn x11_pictures_go_in_requests_that_fit() {
        assert_eq!(bands((0, 0, 100, 10), 400, 4000), vec![(0, 10)]);
        assert_eq!(bands((0, 3, 100, 10), 400, 1000), vec![(3, 5), (5, 7), (7, 9), (9, 10)]);
        assert_eq!(bands((0, 0, 100, 2), 400, 10), vec![(0, 1), (1, 2)], "a row is never split");
        assert!(bands((0, 5, 100, 5), 400, 1000).is_empty());
        let pic: Vec<u8> = (0..4 * 4 * 3).map(|i| i as u8).collect();
        assert_eq!(crop(&pic, 4, (1, 1, 3, 3)), [20, 21, 22, 23, 24, 25, 26, 27, 36, 37, 38, 39, 40, 41, 42, 43]);
    }

    #[test]
    fn x11_keys_are_found_in_the_keyboard_map() {
        // Keycodes from 8, two keysyms each.
        let syms = [0, 0, 0xff1b, 0, 0x7a, 0x5a];
        assert_eq!(keycode_for(&syms, 2, 8, 0xff1b), Some(9), "Escape");
        assert_eq!(keycode_for(&syms, 2, 8, 0x7a), Some(10), "z");
        assert_eq!(keycode_for(&syms, 2, 8, 0x41), None);
        assert_eq!(keycode_for(&syms, 1, 8, 0xff1b), Some(10), "one keysym each");
    }

    /// macOS counts y up from the bottom of the main screen; the desktop (Core Graphics, the
    /// screen copies, the ink) counts down from its top.
    #[test]
    fn mac_screen_coordinates_turn_into_desktop_ones() {
        let main_h = 900.0;
        assert_eq!(desk_from_cocoa((0.0, 0.0, 1440.0, 900.0), main_h), (0, 0, 1440, 900), "the main screen");
        // A screen above the main one, and one to the right reaching lower.
        assert_eq!(desk_from_cocoa((0.0, 900.0, 1920.0, 1080.0), main_h), (0, -1080, 1920, 1080));
        assert_eq!(desk_from_cocoa((1440.0, -180.0, 1280.0, 1024.0), main_h), (1440, 56, 1280, 1024));
        for r in [(0, 0, 1440, 900), (0, -1080, 1920, 1080), (1440, 56, 1280, 1024)] {
            assert_eq!(desk_from_cocoa(cocoa_from_desk(r, main_h), main_h), r, "there and back");
        }
        assert_eq!(desk_point((100.0, 800.0), main_h), [100.0, 100.0]);
        assert_eq!(desk_point((100.0, 1000.0), main_h), [100.0, -100.0], "on the screen above");
        // Over Snagster's window: the mouse goes to it, not to the ink.
        let app = (100, 50, 300, 200);
        assert!(inside(app, [100.0, 50.0]));
        assert!(inside(app, [399.5, 249.5]));
        assert!(!inside(app, [400.0, 100.0]), "right edge not included");
        assert!(!inside(app, [99.9, 100.0]));
    }

    #[test]
    fn mac_strokes_send_only_the_tiles_they_cross() {
        let size = (1000, 600);
        assert_eq!(tiles_touching((10, 10, 20, 20), size, 256), vec![(0, 0, 256, 256)]);
        assert_eq!(tiles_touching((250, 250, 260, 260), size, 256), vec![(0, 0, 256, 256), (256, 0, 512, 256), (0, 256, 256, 512), (256, 256, 512, 512)]);
        let all = tiles_touching((0, 0, 1000, 600), size, 256);
        assert_eq!(all.len(), 4 * 3);
        assert_eq!(all.last(), Some(&(768, 512, 1000, 600)), "the last tiles are cut to the picture");
        assert_eq!(all.iter().map(|t| (t.2 - t.0) * (t.3 - t.1)).sum::<i64>(), 1000 * 600);
        assert!(tiles_touching((2000, 0, 2100, 10), size, 256).is_empty());
        // In the window, from the bottom left, in points (a Retina screen).
        assert_eq!(tile_frame((0, 0, 256, 256), size, 2.0), (0.0, 172.0, 128.0, 128.0), "the top-left tile is at the top");
        assert_eq!(tile_frame((768, 512, 1000, 600), size, 2.0), (384.0, 0.0, 116.0, 44.0));
    }
}
