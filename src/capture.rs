//! Grabbing pictures of the screen, off the UI thread.
//!
//! Every capture is taken from the screen itself (one monitor, or all of them stitched
//! together by their desktop positions), then cut to the area or window wanted. That works for
//! every window, including ones that draw with the GPU. All positions are physical desktop
//! pixels.

use image::RgbaImage;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum CutShape {
    Rect,
    Ellipse,
    Rounded,
    Freehand,
    Polygon,
    Star,
    Heart,
    Triangle,
}

impl CutShape {
    pub const ALL: [CutShape; 8] = [
        CutShape::Rect,
        CutShape::Ellipse,
        CutShape::Rounded,
        CutShape::Freehand,
        CutShape::Polygon,
        CutShape::Star,
        CutShape::Heart,
        CutShape::Triangle,
    ];
    pub fn label(self) -> &'static str {
        match self {
            CutShape::Rect => "Rectangle",
            CutShape::Ellipse => "Circle / oval",
            CutShape::Rounded => "Rounded rectangle",
            CutShape::Freehand => "Freehand (lasso)",
            CutShape::Polygon => "Polygon",
            CutShape::Star => "Star",
            CutShape::Heart => "Heart",
            CutShape::Triangle => "Triangle",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            CutShape::Rect => "R",
            CutShape::Ellipse => "E",
            CutShape::Rounded => "O",
            CutShape::Freehand => "F",
            CutShape::Polygon => "P",
            CutShape::Star => "S",
            CutShape::Heart => "H",
            CutShape::Triangle => "T",
        }
    }
}

/// What the frozen-screen picker should do with the chosen area.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickPurpose {
    /// Show a toolbar after selecting: capture, scroll, record, grab text...
    AllInOne,
    Capture,
    Scrolling { horizontal: bool },
    /// The person scrolls; Snagster keeps taking frames and stitches them.
    ManualScroll,
    /// Choose several areas; they become one picture.
    Multi,
    RecordGif,
    GrabText,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CaptureKind {
    /// The monitor under the mouse.
    Screen,
    /// Every monitor stitched into one picture.
    AllScreens,
    /// One monitor by id.
    Monitor(u32),
    /// Freeze the screen and pick an area of the given shape (or click an object).
    Pick(CutShape, PickPurpose),
    /// Freeze the screen and click a window or object.
    WindowPick,
    /// A fixed-size frame that follows the mouse.
    Fixed(u32, u32),
    /// One window from the list, by id.
    Window(u32, String),
    /// The window in front (for the hotkey).
    ActiveWindow,
    /// The same desktop area as last time: x, y, width, height.
    LastRegion(i32, i32, u32, u32),
    /// Open menus (and their open sub-menus) after the delay.
    Menus,
    /// Freeze the screen and click a pixel to take its colour.
    ColorPicker,
    /// Freeze the screen and measure distances.
    Ruler,
}

impl CaptureKind {
    /// Kinds that freeze the screen and let the person choose on it.
    pub fn uses_overlay(&self) -> bool {
        matches!(
            self,
            CaptureKind::Pick(..) | CaptureKind::WindowPick | CaptureKind::Fixed(..) | CaptureKind::ColorPicker | CaptureKind::Ruler
        )
    }
}

#[derive(Clone, Debug)]
pub struct MonitorInfo {
    pub id: u32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

#[derive(Clone, Debug)]
pub struct WindowInfo {
    pub id: u32,
    pub label: String,
    pub app: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// A window, menu or control on screen, for snapping.
#[derive(Clone, Debug)]
pub struct Object {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub label: String,
    /// Controls inside it (buttons, lists, toolbars...), any depth.
    pub children: Vec<Object>,
}

/// What a capture brings back.
pub struct Shot {
    pub image: RgbaImage,
    /// Desktop position of the picture's top-left pixel.
    pub origin: (i32, i32),
    /// Windows and their controls on that screen, front to back (for snapping).
    pub objects: Vec<Object>,
    /// A name for the picture (a window title, a monitor name...).
    pub title: String,
    /// The program in front when it was taken.
    pub app: String,
    /// Where the mouse pointer was, in desktop pixels.
    pub cursor: Option<(i32, i32)>,
}

pub struct CaptureResult {
    pub kind: CaptureKind,
    pub shot: Result<Shot, String>,
}

pub fn monitors() -> Vec<MonitorInfo> {
    let Ok(all) = xcap::Monitor::all() else { return Vec::new() };
    all.iter()
        .filter_map(|m| {
            Some(MonitorInfo {
                id: m.id().ok()?,
                name: m.friendly_name().or_else(|_| m.name()).unwrap_or_else(|_| "Screen".into()),
                x: m.x().ok()?,
                y: m.y().ok()?,
                width: m.width().ok()?,
                height: m.height().ok()?,
                primary: m.is_primary().unwrap_or(false),
            })
        })
        .collect()
}

/// The visible, titled windows (not Snagster, not minimised), front to back.
pub fn list_windows() -> Vec<WindowInfo> {
    let me = std::process::id();
    let Ok(all) = xcap::Window::all() else { return Vec::new() };
    let mut out: Vec<(i32, WindowInfo)> = Vec::new();
    for w in all {
        if w.pid().ok() == Some(me) || w.is_minimized().unwrap_or(true) {
            continue;
        }
        let title = w.title().unwrap_or_default();
        let (Ok(x), Ok(y), Ok(width), Ok(height), Ok(id)) = (w.x(), w.y(), w.width(), w.height(), w.id()) else {
            continue;
        };
        if !listable(&title, width, height) {
            continue;
        }
        let app = w.app_name().unwrap_or_default();
        let label = window_label(title, &app);
        let z = w.z().unwrap_or(0);
        out.push((z, WindowInfo { id, label, app, x, y, width, height }));
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.into_iter().map(|(_, w)| w).collect()
}

/// Whether a window is worth listing: it has a title and isn't tiny.
fn listable(title: &str, width: u32, height: u32) -> bool {
    !title.trim().is_empty() && width >= 60 && height >= 40
}

/// The name shown for a window: its title, with the program added when the title doesn't
/// say it, cut to 70 characters.
fn window_label(title: String, app: &str) -> String {
    let label = if app.is_empty() || title.contains(app) { title } else { format!("{title}  ({app})") };
    if label.chars().count() > 70 { label.chars().take(67).collect::<String>() + "..." } else { label }
}

/// The desktop rectangle (x, y, width, height) that holds all of `objs`.
fn bounds(objs: &[&Object]) -> Option<(i32, i32, u32, u32)> {
    let x0 = objs.iter().map(|o| o.x).min()?;
    let y0 = objs.iter().map(|o| o.y).min()?;
    let x1 = objs.iter().map(|o| o.x + o.width as i32).max()?;
    let y1 = objs.iter().map(|o| o.y + o.height as i32).max()?;
    Some((x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// The name of a top-level window without a title, from its class.
#[cfg(any(windows, test))]
fn untitled_label(title: String, class: &str) -> String {
    if !title.is_empty() {
        title
    } else if class == "#32768" {
        "Menu".into()
    } else if class == "Progman" || class == "WorkerW" {
        "Desktop".into()
    } else {
        class.to_string()
    }
}

/// Where the mouse is on the desktop, in physical pixels.
pub fn cursor_pos() -> Option<(i32, i32)> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut p = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut p) != 0 {
            return Some((p.x, p.y));
        }
    }
    None
}

// ---------- Windows helpers ----------

#[cfg(windows)]
mod win {
    use super::Object;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    pub fn hwnd(id: u32) -> HWND {
        id as usize as HWND
    }

    fn text(h: HWND) -> String {
        let mut buf = [0u16; 256];
        let n = unsafe { GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    fn class(h: HWND) -> String {
        let mut buf = [0u16; 128];
        let n = unsafe { GetClassNameW(h, buf.as_mut_ptr(), buf.len() as i32) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    fn rect(h: HWND, top: bool) -> Option<RECT> {
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe {
            let ok = top
                && DwmGetWindowAttribute(
                    h,
                    DWMWA_EXTENDED_FRAME_BOUNDS as u32,
                    &mut r as *mut RECT as *mut core::ffi::c_void,
                    std::mem::size_of::<RECT>() as u32,
                ) == 0;
            if !ok && GetWindowRect(h, &mut r) == 0 {
                return None;
            }
        }
        (r.right - r.left >= 6 && r.bottom - r.top >= 6).then_some(r)
    }

    fn cloaked(h: HWND) -> bool {
        let mut c: u32 = 0;
        unsafe {
            DwmGetWindowAttribute(h, DWMWA_CLOAKED as u32, &mut c as *mut u32 as *mut core::ffi::c_void, 4) == 0 && c != 0
        }
    }

    pub fn pid_of(h: HWND) -> u32 {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(h, &mut pid) };
        pid
    }

    unsafe extern "system" fn collect(h: HWND, l: LPARAM) -> i32 {
        let v = unsafe { &mut *(l as *mut Vec<HWND>) };
        v.push(h);
        1
    }

    /// Visible top-level windows (menus and pop-ups included) with their controls,
    /// front to back.
    pub fn objects() -> Vec<Object> {
        let me = std::process::id();
        let mut tops: Vec<HWND> = Vec::new();
        unsafe { EnumWindows(Some(collect), &mut tops as *mut Vec<HWND> as LPARAM) };
        let mut out = Vec::new();
        for h in tops {
            unsafe {
                if IsWindowVisible(h) == 0 || IsIconic(h) != 0 || cloaked(h) || pid_of(h) == me {
                    continue;
                }
            }
            let cls = class(h);
            if cls == "tooltips_class32" {
                continue;
            }
            let Some(r) = rect(h, true) else { continue };
            let label = super::untitled_label(text(h), &cls);
            let mut kids: Vec<HWND> = Vec::new();
            unsafe { EnumChildWindows(h, Some(collect), &mut kids as *mut Vec<HWND> as LPARAM) };
            let mut children = Vec::new();
            for k in kids.into_iter().take(600) {
                if unsafe { IsWindowVisible(k) } == 0 {
                    continue;
                }
                if let Some(cr) = rect(k, false) {
                    let t = text(k);
                    children.push(Object {
                        x: cr.left,
                        y: cr.top,
                        width: (cr.right - cr.left) as u32,
                        height: (cr.bottom - cr.top) as u32,
                        label: if t.is_empty() { class(k) } else { t },
                        children: Vec::new(),
                    });
                }
            }
            out.push(Object {
                x: r.left,
                y: r.top,
                width: (r.right - r.left) as u32,
                height: (r.bottom - r.top) as u32,
                label,
                children,
            });
        }
        out
    }

    pub fn foreground() -> Option<HWND> {
        let h = unsafe { GetForegroundWindow() };
        (!h.is_null()).then_some(h)
    }

    pub fn title_of(h: HWND) -> String {
        text(h)
    }

    pub fn bring_to_front(id: u32) {
        let h = hwnd(id);
        unsafe {
            if IsIconic(h) != 0 {
                ShowWindow(h, SW_RESTORE);
            }
            SetForegroundWindow(h);
        }
    }

    pub fn window_at(x: i32, y: i32) -> Option<HWND> {
        let h = unsafe { WindowFromPoint(windows_sys::Win32::Foundation::POINT { x, y }) };
        if h.is_null() {
            return None;
        }
        Some(unsafe { GetAncestor(h, GA_ROOT) })
    }
}

pub fn objects() -> Vec<Object> {
    #[cfg(windows)]
    {
        win::objects()
    }
    #[cfg(not(windows))]
    {
        list_windows()
            .into_iter()
            .map(|w| Object { x: w.x, y: w.y, width: w.width, height: w.height, label: w.label, children: Vec::new() })
            .collect()
    }
}

/// The title and program of the window in front.
pub fn foreground_info() -> (String, String) {
    #[cfg(windows)]
    {
        if let Some(h) = win::foreground() {
            let id = h as usize as u32;
            let title = win::title_of(h);
            let app = xcap::Window::all()
                .ok()
                .and_then(|all| all.into_iter().find(|w| w.id().ok() == Some(id)))
                .and_then(|w| w.app_name().ok())
                .unwrap_or_default();
            return (title, app);
        }
    }
    (String::new(), String::new())
}

fn opaque(mut img: RgbaImage) -> RgbaImage {
    for p in img.pixels_mut() {
        p.0[3] = 255;
    }
    img
}

fn monitor_at(x: i32, y: i32) -> Result<xcap::Monitor, String> {
    xcap::Monitor::from_point(x, y).or_else(|_| {
        let all = xcap::Monitor::all().map_err(|e| e.to_string())?;
        let primary = all.iter().position(|m| m.is_primary().unwrap_or(false)).unwrap_or(0);
        all.into_iter().nth(primary).ok_or_else(|| "no screen found".to_string())
    })
}

fn shot(image: RgbaImage, origin: (i32, i32), title: String) -> Shot {
    let (fg_title, app) = foreground_info();
    Shot {
        image: opaque(image),
        origin,
        objects: Vec::new(),
        title: if title.is_empty() { fg_title } else { title },
        app,
        cursor: cursor_pos(),
    }
}

fn grab_monitor(m: &xcap::Monitor) -> Result<Shot, String> {
    let image = m.capture_image().map_err(|e| e.to_string())?;
    let name = m.friendly_name().or_else(|_| m.name()).unwrap_or_else(|_| "Screen".into());
    let mut s = shot(image, (m.x().unwrap_or(0), m.y().unwrap_or(0)), String::new());
    if s.title.is_empty() {
        s.title = name;
    }
    Ok(s)
}

/// Every monitor in one picture, placed by its desktop position.
pub fn grab_all_screens() -> Result<Shot, String> {
    let all = xcap::Monitor::all().map_err(|e| e.to_string())?;
    if all.is_empty() {
        return Err("no screen found".into());
    }
    let mut parts = Vec::new();
    for m in &all {
        let img = m.capture_image().map_err(|e| e.to_string())?;
        parts.push((m.x().unwrap_or(0), m.y().unwrap_or(0), img));
    }
    let min_x = parts.iter().map(|p| p.0).min().unwrap();
    let min_y = parts.iter().map(|p| p.1).min().unwrap();
    let max_x = parts.iter().map(|p| p.0 + p.2.width() as i32).max().unwrap();
    let max_y = parts.iter().map(|p| p.1 + p.2.height() as i32).max().unwrap();
    let mut canvas = RgbaImage::from_pixel((max_x - min_x) as u32, (max_y - min_y) as u32, image::Rgba([0, 0, 0, 255]));
    for (x, y, img) in &parts {
        image::imageops::replace(&mut canvas, img, (*x - min_x) as i64, (*y - min_y) as i64);
    }
    Ok(shot(canvas, (min_x, min_y), "All screens".into()))
}

/// Cut a desktop rectangle out of a picture that starts at `origin`.
pub fn crop_desktop(img: &RgbaImage, origin: (i32, i32), x: i32, y: i32, w: u32, h: u32) -> Option<RgbaImage> {
    let lx = (x - origin.0).max(0);
    let ly = (y - origin.1).max(0);
    let rx = (x + w as i32 - origin.0).min(img.width() as i32);
    let ry = (y + h as i32 - origin.1).min(img.height() as i32);
    if rx - lx < 2 || ry - ly < 2 {
        return None;
    }
    Some(image::imageops::crop_imm(img, lx as u32, ly as u32, (rx - lx) as u32, (ry - ly) as u32).to_image())
}

/// A desktop rectangle, grabbed from the monitor it sits on.
pub fn grab_rect(x: i32, y: i32, w: u32, h: u32) -> Result<RgbaImage, String> {
    let m = monitor_at(x + w as i32 / 2, y + h as i32 / 2)?;
    let (mx, my) = (m.x().unwrap_or(0), m.y().unwrap_or(0));
    let (mw, mh) = (m.width().unwrap_or(0), m.height().unwrap_or(0));
    let lx = (x - mx).clamp(0, mw as i32) as u32;
    let ly = (y - my).clamp(0, mh as i32) as u32;
    let rw = w.min(mw.saturating_sub(lx));
    let rh = h.min(mh.saturating_sub(ly));
    if rw < 2 || rh < 2 {
        return Err("that area is off screen".into());
    }
    m.capture_region(lx, ly, rw, rh).map(opaque).map_err(|e| e.to_string())
}

fn grab_window(id: u32, title: String, raise: bool) -> Result<Shot, String> {
    let info = list_windows().into_iter().find(|w| w.id == id);
    if raise {
        #[cfg(windows)]
        win::bring_to_front(id);
        std::thread::sleep(Duration::from_millis(250));
    }
    if let Some(w) = info {
        let all = grab_all_screens()?;
        if let Some(img) = crop_desktop(&all.image, all.origin, w.x, w.y, w.width, w.height) {
            let mut s = shot(img, (w.x, w.y), w.label);
            if !w.app.is_empty() {
                s.app = w.app;
            }
            return Ok(s);
        }
    }
    let win = xcap::Window::all()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|w| w.id().ok() == Some(id))
        .ok_or_else(|| "that window has closed".to_string())?;
    let image = win.capture_image().map_err(|e| e.to_string())?;
    Ok(shot(image, (win.x().unwrap_or(0), win.y().unwrap_or(0)), title))
}

pub fn grab(kind: &CaptureKind, fallback_at: (i32, i32)) -> Result<Shot, String> {
    let at = cursor_pos().unwrap_or(fallback_at);
    match kind {
        CaptureKind::Screen => grab_monitor(&monitor_at(at.0, at.1)?),
        CaptureKind::AllScreens => grab_all_screens(),
        CaptureKind::Monitor(id) => {
            let all = xcap::Monitor::all().map_err(|e| e.to_string())?;
            let m = all.into_iter().find(|m| m.id().ok() == Some(*id)).ok_or("that screen is gone")?;
            grab_monitor(&m)
        }
        CaptureKind::Pick(..) | CaptureKind::WindowPick | CaptureKind::Fixed(..) | CaptureKind::ColorPicker | CaptureKind::Ruler => {
            let objs = objects();
            let mut s = grab_monitor(&monitor_at(at.0, at.1)?)?;
            s.objects = objs;
            Ok(s)
        }
        CaptureKind::Window(id, title) => grab_window(*id, title.clone(), true),
        CaptureKind::ActiveWindow => {
            #[cfg(windows)]
            {
                let h = win::foreground().ok_or("no window is in front")?;
                grab_window(h as usize as u32, win::title_of(h), false)
            }
            #[cfg(not(windows))]
            {
                Err("not available on this system".into())
            }
        }
        CaptureKind::Menus => {
            let objs = objects();
            let menus: Vec<&Object> = objs.iter().filter(|o| o.label == "Menu").collect();
            let Some((x0, y0, mw, mh)) = bounds(&menus) else {
                return Err("No open menu was found. Open the menu during the countdown.".into());
            };
            let all = grab_all_screens()?;
            let img = crop_desktop(&all.image, all.origin, x0, y0, mw, mh).ok_or("the menu is off screen")?;
            let mut s = shot(img, (x0, y0), "Menu".into());
            s.app = all.app;
            Ok(s)
        }
        CaptureKind::LastRegion(x, y, w, h) => {
            let all = grab_all_screens()?;
            let img = crop_desktop(&all.image, all.origin, *x, *y, *w, *h).ok_or("the last area is off screen")?;
            Ok(shot(img, (*x, *y), "Area".into()))
        }
    }
}

/// Wait `delay_ms`, grab, and send the result back; the UI thread restores the window.
pub fn spawn(kind: CaptureKind, delay_ms: u64, at: (i32, i32), ctx: eframe::egui::Context, tx: Sender<CaptureResult>) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(delay_ms));
        let shot = grab(&kind, at);
        let _ = tx.send(CaptureResult { kind, shot });
        ctx.request_repaint();
    });
}

// ---------- scrolling and panoramic capture ----------

fn row_sigs(img: &RgbaImage) -> Vec<u64> {
    let w = img.width();
    (0..img.height())
        .map(|y| {
            let mut h: u64 = 1469598103934665603;
            let mut x = 0;
            while x < w {
                let p = img.get_pixel(x, y).0;
                for c in &p[..3] {
                    h ^= (*c >> 3) as u64;
                    h = h.wrapping_mul(1099511628211);
                }
                x += 3;
            }
            h
        })
        .collect()
}

/// How far `next` has moved up compared with `prev` (rows), and how sure we are.
fn find_shift(prev: &[u64], next: &[u64]) -> (usize, f32) {
    let h = prev.len().min(next.len());
    let min_overlap = (h / 6).max(8);
    let mut best = (0usize, 0f32);
    for dy in 0..h.saturating_sub(min_overlap) {
        let n = h - dy;
        let mut same = 0;
        let mut i = 0;
        while i < n {
            if prev[dy + i] == next[i] {
                same += 1;
            }
            i += 2;
        }
        let score = same as f32 / n.div_ceil(2) as f32;
        if score > best.1 + 0.001 {
            best = (dy, score);
        }
    }
    best
}

/// How many new rows a frame brings, from its shift against the last one. A weak match means
/// the content jumped: keep the whole frame rather than losing it.
fn rows_to_add(dy: usize, score: f32, frame_height: u32) -> u32 {
    if score >= 0.6 { dy as u32 } else { frame_height }
}

/// `out` with the bottom `add` rows of `frame` put underneath.
fn append_rows(out: &RgbaImage, frame: &RgbaImage, add: u32) -> RgbaImage {
    let add = add.min(frame.height());
    let mut grown = RgbaImage::new(out.width(), out.height() + add);
    image::imageops::replace(&mut grown, out, 0, 0);
    let part = image::imageops::crop_imm(frame, 0, frame.height() - add, frame.width(), add).to_image();
    image::imageops::replace(&mut grown, &part, 0, out.height() as i64);
    grown
}

#[cfg(windows)]
fn scroll_wheel(at: (i32, i32), horizontal: bool, notches: i32) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos;
    unsafe {
        SetCursorPos(at.0, at.1);
        let delta = if horizontal { 120 * notches } else { -120 * notches };
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: delta as u32,
                    dwFlags: if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}

#[cfg(not(windows))]
fn scroll_wheel(_at: (i32, i32), _horizontal: bool, _notches: i32) {}

/// Capture a desktop area while scrolling the content under it, and stitch the frames.
/// Sideways (`horizontal`) works the same on a turned picture.
pub fn scroll_capture(
    rect: (i32, i32, u32, u32),
    horizontal: bool,
    max_steps: u32,
    wait_ms: u64,
    stop: Arc<AtomicBool>,
    manual: bool,
) -> Result<RgbaImage, String> {
    let (x, y, w, h) = rect;
    let center = (x + w as i32 / 2, y + h as i32 / 2);
    let old_cursor = cursor_pos();
    let turn = |img: RgbaImage| if horizontal { image::imageops::rotate90(&img) } else { img };
    let mut frame = turn(grab_rect(x, y, w, h)?);
    let fh = frame.height();
    let mut out = frame.clone();
    let mut prev = row_sigs(&frame);
    let mut still = 0;
    let steps = if manual { 6000 } else { max_steps.clamp(1, 200) };
    for _ in 0..steps {
        if stop.load(Ordering::Relaxed) || out.height() > 30000 {
            break;
        }
        if manual {
            std::thread::sleep(Duration::from_millis(120));
        } else {
            scroll_wheel(center, horizontal, 3);
            std::thread::sleep(Duration::from_millis(wait_ms));
        }
        frame = turn(grab_rect(x, y, w, h)?);
        let sigs = row_sigs(&frame);
        let (dy, score) = find_shift(&prev, &sigs);
        if dy == 0 && score > 0.97 {
            still += 1;
            if still >= 2 && !manual {
                break;
            }
            continue;
        }
        still = 0;
        let add = rows_to_add(dy, score, fh);
        if add == 0 {
            continue;
        }
        out = append_rows(&out, &frame, add);
        prev = sigs;
    }
    #[cfg(windows)]
    if let Some(c) = old_cursor {
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos(c.0, c.1) };
    }
    let _ = old_cursor;
    Ok(if horizontal { image::imageops::rotate270(&out) } else { out })
}

// ---------- step recording ----------

pub struct StepEvent {
    pub image: RgbaImage,
    /// Where the click was, in the picture's pixels.
    pub click: (f32, f32),
    pub title: String,
    pub app: String,
}

/// Watch for left clicks outside Snagster; take a picture of the screen at each one.
pub fn spawn_step_recorder(stop: Arc<AtomicBool>, tx: Sender<StepEvent>, ctx: eframe::egui::Context) {
    std::thread::spawn(move || {
        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
            let me = std::process::id();
            let mut was_down = true;
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(12));
                let down = unsafe { GetAsyncKeyState(0x01) } as u16 & 0x8000 != 0;
                if down && !was_down
                    && let Some(p) = cursor_pos()
                {
                    let ours = win::window_at(p.0, p.1).map(|h| win::pid_of(h) == me).unwrap_or(false);
                    if !ours
                        && let Ok(m) = monitor_at(p.0, p.1)
                        && let Ok(img) = m.capture_image()
                    {
                        let (mx, my) = (m.x().unwrap_or(0), m.y().unwrap_or(0));
                        let (title, app) = foreground_info();
                        let _ = tx.send(StepEvent {
                            image: opaque(img),
                            click: ((p.0 - mx) as f32, (p.1 - my) as f32),
                            title,
                            app,
                        });
                        ctx.request_repaint();
                    }
                }
                was_down = down;
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (stop, tx, ctx);
        }
    });
}

// ---------- GIF recording ----------

pub struct GifDone {
    pub path: std::path::PathBuf,
    pub frames: u32,
    pub seconds: f32,
    pub result: Result<(), String>,
}

/// Let a background thread step aside: lower priority, and on Windows 11 "efficiency mode"
/// (EcoQoS), which runs it on the power-saving cores at a lower clock.
pub fn background_thread(eco: bool) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadInformation, SetThreadPriority, THREAD_POWER_THROTTLING_CURRENT_VERSION,
            THREAD_POWER_THROTTLING_EXECUTION_SPEED, THREAD_POWER_THROTTLING_STATE, THREAD_PRIORITY_BELOW_NORMAL,
            ThreadPowerThrottling,
        };
        let me = GetCurrentThread();
        SetThreadPriority(me, THREAD_PRIORITY_BELOW_NORMAL);
        if eco {
            let state = THREAD_POWER_THROTTLING_STATE {
                Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
                StateMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
            };
            SetThreadInformation(
                me,
                ThreadPowerThrottling,
                &state as *const THREAD_POWER_THROTTLING_STATE as *const core::ffi::c_void,
                std::mem::size_of::<THREAD_POWER_THROTTLING_STATE>() as u32,
            );
        }
    }
    #[cfg(not(windows))]
    let _ = eco;
}

/// The part of `next` that differs from `prev` (same size), or None when nothing changed.
/// Rows are compared as whole slices, which is very fast.
pub fn changed_rect(prev: &RgbaImage, next: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = next.dimensions();
    if prev.dimensions() != (w, h) {
        return Some((0, 0, w, h));
    }
    let stride = w as usize * 4;
    let (a, b) = (prev.as_raw(), next.as_raw());
    let row = |y: u32| &a[y as usize * stride..(y as usize + 1) * stride] != &b[y as usize * stride..(y as usize + 1) * stride];
    let top = (0..h).find(|&y| row(y))?;
    let bottom = (top..h).rev().find(|&y| row(y)).unwrap_or(top);
    let mut left = w;
    let mut right = 0;
    for y in top..=bottom {
        let ra = &a[y as usize * stride..(y as usize + 1) * stride];
        let rb = &b[y as usize * stride..(y as usize + 1) * stride];
        if ra == rb {
            continue;
        }
        if let Some(x) = (0..w as usize).find(|&x| ra[x * 4..x * 4 + 4] != rb[x * 4..x * 4 + 4]) {
            left = left.min(x as u32);
        }
        if let Some(x) = (0..w as usize).rev().find(|&x| ra[x * 4..x * 4 + 4] != rb[x * 4..x * 4 + 4]) {
            right = right.max(x as u32);
        }
    }
    if right < left {
        return None;
    }
    Some((left, top, right - left + 1, bottom - top + 1))
}

/// Record a desktop area as an animated GIF until `stop` (or the time limit).
///
/// Built to cost little: the area is copied with one small GDI copy per frame, identical
/// frames aren't stored at all (the previous one just stays up longer), changed frames store
/// only the rectangle that changed, colour reduction uses the fastest setting, and both
/// threads run at low priority (the encoder in Windows' efficiency mode).
#[allow(clippy::too_many_arguments)]
pub fn spawn_gif_recorder(
    rect: (i32, i32, u32, u32),
    fps: u32,
    max_s: u32,
    max_width: u32,
    pointer: bool,
    path: std::path::PathBuf,
    stop: Arc<AtomicBool>,
    ctx: eframe::egui::Context,
    done: Sender<GifDone>,
) {
    std::thread::spawn(move || {
        background_thread(false);
        let (x, y, w, h) = rect;
        let fps = fps.clamp(1, 30);
        let scale = if w > max_width.max(160) { max_width.max(160) as f32 / w as f32 } else { 1.0 };
        let (ow, oh) = (((w as f32 * scale) as u32).clamp(1, 65535), ((h as f32 * scale) as u32).clamp(1, 65535));
        // (picture part, left, top, delay in ms)
        let (ftx, frx) = std::sync::mpsc::channel::<(RgbaImage, u32, u32, u32)>();
        let enc_path = path.clone();
        let encoder = std::thread::spawn(move || -> Result<u32, String> {
            background_thread(true);
            if let Some(dir) = enc_path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            let file = std::io::BufWriter::new(std::fs::File::create(&enc_path).map_err(|e| e.to_string())?);
            let mut enc = gif::Encoder::new(file, ow as u16, oh as u16, &[]).map_err(|e| e.to_string())?;
            enc.set_repeat(gif::Repeat::Infinite).map_err(|e| e.to_string())?;
            let mut n = 0;
            for (img, left, top, ms) in frx {
                let (fw, fh) = img.dimensions();
                let mut raw = img.into_raw();
                // Speed 30 is the fastest colour reduction the encoder offers.
                let mut f = gif::Frame::from_rgba_speed(fw as u16, fh as u16, &mut raw, 30);
                f.left = left as u16;
                f.top = top as u16;
                f.delay = (ms / 10).clamp(2, 65535) as u16;
                f.dispose = gif::DisposalMethod::Keep;
                enc.write_frame(&f).map_err(|e| e.to_string())?;
                n += 1;
            }
            Ok(n)
        });
        let frame_ms = 1000 / fps;
        let start = Instant::now();
        let mut next = start;
        let mut err = None;
        let mut prev: Option<RgbaImage> = None;
        let mut pending: Option<(RgbaImage, u32, u32, u32)> = None;
        let mut captured = 0u32;
        while !stop.load(Ordering::Relaxed) && start.elapsed().as_secs() < max_s as u64 {
            match grab_rect(x, y, w, h) {
                Ok(mut img) => {
                    if scale < 1.0 {
                        // A plain box filter: far cheaper than smooth resampling, fine for GIFs.
                        img = image::imageops::thumbnail(&img, ow, oh);
                    }
                    if pointer && let Some(c) = cursor_pos() {
                        stamp_pointer(&mut img, ((c.0 - x) as f32 * scale, (c.1 - y) as f32 * scale));
                    }
                    captured += 1;
                    let change = match &prev {
                        None => Some((0, 0, ow, oh)),
                        Some(p) => changed_rect(p, &img),
                    };
                    match change {
                        None => {
                            // Nothing moved: keep showing the last frame a little longer.
                            if let Some(p) = &mut pending {
                                p.3 += frame_ms;
                            }
                        }
                        Some((l, t, cw, ch)) => {
                            if let Some(p) = pending.take()
                                && ftx.send(p).is_err()
                            {
                                break;
                            }
                            let part = if (cw, ch) == (ow, oh) { img.clone() } else { image::imageops::crop_imm(&img, l, t, cw, ch).to_image() };
                            pending = Some((part, l, t, frame_ms));
                            prev = Some(img);
                        }
                    }
                }
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
            next += Duration::from_millis(frame_ms as u64);
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else {
                next = now;
            }
        }
        if let Some(p) = pending.take() {
            let _ = ftx.send(p);
        }
        let seconds = start.elapsed().as_secs_f32();
        drop(ftx);
        let enc = encoder.join().unwrap_or_else(|_| Err("the encoder stopped".into()));
        let (frames, result) = match (enc, err) {
            (Ok(_), None) => (captured, Ok(())),
            (Ok(_), Some(e)) => (captured, Err(e)),
            (Err(e), _) => (0, Err(e)),
        };
        let _ = done.send(GifDone { path, frames, seconds, result });
        ctx.request_repaint();
    });
}

/// A small arrow pointer picture, drawn once.
fn pointer_sprite() -> &'static RgbaImage {
    static P: std::sync::OnceLock<RgbaImage> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let mut pm = tiny_skia::Pixmap::new(16, 24).unwrap();
        let pts = [(1.0, 1.0), (1.0, 18.0), (5.5, 14.0), (8.5, 20.5), (11.0, 19.5), (8.0, 13.0), (13.5, 13.0)];
        let mut pb = tiny_skia::PathBuilder::new();
        pb.move_to(pts[0].0, pts[0].1);
        for p in &pts[1..] {
            pb.line_to(p.0, p.1);
        }
        pb.close();
        if let Some(path) = pb.finish() {
            let mut white = tiny_skia::Paint::default();
            white.set_color_rgba8(255, 255, 255, 255);
            white.anti_alias = true;
            let mut black = tiny_skia::Paint::default();
            black.set_color_rgba8(20, 20, 20, 255);
            black.anti_alias = true;
            let id = tiny_skia::Transform::identity();
            pm.fill_path(&path, &white, tiny_skia::FillRule::Winding, id, None);
            pm.stroke_path(&path, &black, &tiny_skia::Stroke { width: 1.2, ..Default::default() }, id, None);
        }
        crate::render::from_pixmap(pm)
    })
}

/// Draw a small arrow pointer onto a frame (only the pixels under it are touched).
pub fn stamp_pointer(img: &mut RgbaImage, at: (f32, f32)) {
    let (w, h) = img.dimensions();
    if at.0 < -16.0 || at.1 < -24.0 || at.0 >= w as f32 || at.1 >= h as f32 {
        return;
    }
    image::imageops::overlay(img, pointer_sprite(), at.0 as i64 - 1, at.1 as i64 - 1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// A tall "page" whose every row has its own colour, so rows can be told apart.
    fn page(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| Rgba([((y % 32) * 8) as u8, ((y / 32) * 8) as u8, if x % 2 == 0 { 40 } else { 200 }, 255]))
    }

    fn rows(img: &RgbaImage, top: u32, h: u32) -> RgbaImage {
        image::imageops::crop_imm(img, 0, top, img.width(), h).to_image()
    }

    fn obj(x: i32, y: i32, w: u32, h: u32, label: &str) -> Object {
        Object { x, y, width: w, height: h, label: label.into(), children: Vec::new() }
    }

    #[test]
    fn shape_keys_are_unique_and_real_keys() {
        let keys: std::collections::HashSet<_> = CutShape::ALL.iter().map(|s| s.key()).collect();
        assert_eq!(keys.len(), CutShape::ALL.len());
        for s in CutShape::ALL {
            assert!(eframe::egui::Key::from_name(s.key()).is_some(), "{s:?}");
            assert!(!s.label().is_empty());
        }
    }

    #[test]
    fn only_picking_kinds_freeze_the_screen() {
        assert!(CaptureKind::Pick(CutShape::Rect, PickPurpose::Capture).uses_overlay());
        assert!(CaptureKind::WindowPick.uses_overlay());
        assert!(CaptureKind::Fixed(10, 10).uses_overlay());
        assert!(CaptureKind::ColorPicker.uses_overlay());
        assert!(CaptureKind::Ruler.uses_overlay());
        assert!(!CaptureKind::Screen.uses_overlay());
        assert!(!CaptureKind::AllScreens.uses_overlay());
        assert!(!CaptureKind::Window(1, "x".into()).uses_overlay());
        assert!(!CaptureKind::LastRegion(0, 0, 10, 10).uses_overlay());
        assert!(!CaptureKind::Menus.uses_overlay());
    }

    #[test]
    fn opaque_sets_every_alpha() {
        let img = opaque(RgbaImage::from_pixel(3, 2, Rgba([1, 2, 3, 0])));
        assert!(img.pixels().all(|p| p.0 == [1, 2, 3, 255]));
    }

    #[test]
    fn crop_inside_the_picture() {
        let img = page(100, 80);
        let c = crop_desktop(&img, (0, 0), 10, 20, 30, 40).unwrap();
        assert_eq!(c.dimensions(), (30, 40));
        assert_eq!(c.get_pixel(0, 0), img.get_pixel(10, 20));
    }

    #[test]
    fn crop_uses_the_picture_origin() {
        // A monitor left of and above the primary one starts at negative desktop pixels.
        let img = page(100, 80);
        let c = crop_desktop(&img, (-100, -80), -90, -70, 20, 20).unwrap();
        assert_eq!(c.dimensions(), (20, 20));
        assert_eq!(c.get_pixel(0, 0), img.get_pixel(10, 10));
    }

    #[test]
    fn crop_is_clipped_at_the_edges() {
        let img = page(100, 80);
        let c = crop_desktop(&img, (0, 0), -10, -5, 30, 25).unwrap();
        assert_eq!(c.dimensions(), (20, 20));
        assert_eq!(c.get_pixel(0, 0), img.get_pixel(0, 0));
        let c = crop_desktop(&img, (0, 0), 90, 70, 50, 50).unwrap();
        assert_eq!(c.dimensions(), (10, 10));
    }

    #[test]
    fn crop_off_the_picture_or_too_small_is_none() {
        let img = page(100, 80);
        assert!(crop_desktop(&img, (0, 0), 200, 200, 10, 10).is_none());
        assert!(crop_desktop(&img, (0, 0), -50, 0, 10, 10).is_none());
        assert!(crop_desktop(&img, (0, 0), 10, 10, 1, 50).is_none());
        assert!(crop_desktop(&img, (0, 0), 99, 10, 10, 10).is_none());
    }

    #[test]
    fn windows_without_a_title_or_too_small_are_not_listed() {
        assert!(listable("Notepad", 800, 600));
        assert!(listable("x", 60, 40));
        assert!(!listable("", 800, 600));
        assert!(!listable("   ", 800, 600));
        assert!(!listable("Tiny", 59, 600));
        assert!(!listable("Tiny", 800, 39));
    }

    #[test]
    fn window_label_adds_the_program_once() {
        assert_eq!(window_label("notes.txt".into(), "Notepad"), "notes.txt  (Notepad)");
        assert_eq!(window_label("notes.txt - Notepad".into(), "Notepad"), "notes.txt - Notepad");
        assert_eq!(window_label("notes.txt".into(), ""), "notes.txt");
    }

    #[test]
    fn long_window_labels_are_cut_to_70_characters() {
        // Counted in characters, not bytes, so non-English titles don't panic or cut short.
        let long = "\u{e5}".repeat(100);
        let l = window_label(long, "");
        assert_eq!(l.chars().count(), 70);
        assert!(l.ends_with("..."));
        let exact = "a".repeat(70);
        assert_eq!(window_label(exact.clone(), ""), exact);
    }

    #[test]
    fn untitled_windows_are_named_by_class() {
        assert_eq!(untitled_label("Title".into(), "#32768"), "Title");
        assert_eq!(untitled_label(String::new(), "#32768"), "Menu");
        assert_eq!(untitled_label(String::new(), "Progman"), "Desktop");
        assert_eq!(untitled_label(String::new(), "WorkerW"), "Desktop");
        assert_eq!(untitled_label(String::new(), "Shell_TrayWnd"), "Shell_TrayWnd");
    }

    #[test]
    fn menu_bounds_hold_every_open_menu() {
        let a = obj(100, 50, 200, 300, "Menu");
        let b = obj(290, 120, 180, 100, "Menu");
        assert_eq!(bounds(&[&a, &b]), Some((100, 50, 370, 300)));
        assert_eq!(bounds(&[&a]), Some((100, 50, 200, 300)));
        let c = obj(-300, -20, 100, 40, "Menu");
        assert_eq!(bounds(&[&a, &c]), Some((-300, -20, 600, 370)));
        assert_eq!(bounds(&[]), None);
    }

    #[test]
    fn row_signatures_tell_rows_apart() {
        let img = page(30, 100);
        let s = row_sigs(&img);
        assert_eq!(s.len(), 100);
        let distinct: std::collections::HashSet<_> = s.iter().collect();
        assert_eq!(distinct.len(), 100);
        // Small colour noise (under 8 levels) doesn't change a signature.
        let mut noisy = img.clone();
        for p in noisy.pixels_mut() {
            p.0[0] |= 3;
        }
        assert_eq!(row_sigs(&noisy), s);
    }

    #[test]
    fn find_shift_measures_how_far_content_scrolled() {
        let p = page(30, 400);
        let a = row_sigs(&rows(&p, 0, 120));
        for step in [1, 7, 20, 60] {
            let b = row_sigs(&rows(&p, step, 120));
            let (dy, score) = find_shift(&a, &b);
            assert_eq!(dy, step as usize, "step {step}");
            assert!(score > 0.99);
        }
    }

    #[test]
    fn find_shift_of_the_same_frame_is_zero() {
        let p = page(30, 120);
        let s = row_sigs(&p);
        assert_eq!(find_shift(&s, &s), (0, 1.0));
        // A blank area matches everywhere; the smallest shift wins.
        let blank = row_sigs(&RgbaImage::from_pixel(30, 120, Rgba([255, 255, 255, 255])));
        assert_eq!(find_shift(&blank, &blank), (0, 1.0));
    }

    #[test]
    fn find_shift_is_unsure_when_content_jumped() {
        let p = page(30, 600);
        let a = row_sigs(&rows(&p, 0, 120));
        let b = row_sigs(&rows(&p, 400, 120));
        let (_, score) = find_shift(&a, &b);
        assert!(score < 0.6, "{score}");
        assert_eq!(rows_to_add(5, score, 120), 120);
    }

    #[test]
    fn find_shift_of_tiny_frames_finds_nothing() {
        let s = row_sigs(&page(10, 5));
        assert_eq!(find_shift(&s, &s), (0, 0.0));
        assert_eq!(find_shift(&[], &[]), (0, 0.0));
    }

    #[test]
    fn rows_to_add_trusts_good_matches() {
        assert_eq!(rows_to_add(30, 0.95, 200), 30);
        assert_eq!(rows_to_add(30, 0.6, 200), 30);
        assert_eq!(rows_to_add(30, 0.59, 200), 200);
    }

    #[test]
    fn append_rows_puts_the_new_bottom_under() {
        let p = page(20, 200);
        let out = rows(&p, 0, 100);
        let frame = rows(&p, 30, 100);
        let grown = append_rows(&out, &frame, 30);
        assert_eq!(grown.dimensions(), (20, 130));
        assert_eq!(grown, rows(&p, 0, 130));
        // Asking for more rows than the frame has adds the whole frame.
        assert_eq!(append_rows(&out, &frame, 500).height(), 200);
    }

    #[test]
    fn stitching_scrolled_frames_rebuilds_the_page() {
        // What scroll_capture does, on frames of a synthetic page instead of the screen.
        let p = page(40, 1000);
        let fh = 150;
        let mut out = rows(&p, 0, fh);
        let mut prev = row_sigs(&out);
        let mut top = 0;
        for step in [40, 55, 13, 90, 70] {
            top += step;
            let frame = rows(&p, top, fh);
            let sigs = row_sigs(&frame);
            let (dy, score) = find_shift(&prev, &sigs);
            let add = rows_to_add(dy, score, fh);
            out = append_rows(&out, &frame, add);
            prev = sigs;
        }
        assert_eq!(out.height(), top + fh);
        assert_eq!(out, rows(&p, 0, top + fh));
    }

    #[test]
    fn changed_rect_of_identical_frames_is_none() {
        let a = page(50, 40);
        assert_eq!(changed_rect(&a, &a.clone()), None);
    }

    #[test]
    fn changed_rect_of_a_new_size_is_everything() {
        let a = page(50, 40);
        let b = page(60, 40);
        assert_eq!(changed_rect(&a, &b), Some((0, 0, 60, 40)));
    }

    #[test]
    fn changed_rect_finds_one_pixel() {
        let a = page(50, 40);
        let mut b = a.clone();
        b.put_pixel(17, 23, Rgba([1, 2, 3, 4]));
        assert_eq!(changed_rect(&a, &b), Some((17, 23, 1, 1)));
    }

    #[test]
    fn changed_rect_holds_every_change() {
        let a = page(50, 40);
        let mut b = a.clone();
        b.put_pixel(40, 5, Rgba([0, 0, 0, 0]));
        b.put_pixel(3, 30, Rgba([0, 0, 0, 0]));
        b.put_pixel(20, 15, Rgba([0, 0, 0, 0]));
        assert_eq!(changed_rect(&a, &b), Some((3, 5, 38, 26)));
        // Corners too.
        let mut c = a.clone();
        c.put_pixel(0, 0, Rgba([9, 9, 9, 9]));
        c.put_pixel(49, 39, Rgba([9, 9, 9, 9]));
        assert_eq!(changed_rect(&a, &c), Some((0, 0, 50, 40)));
    }

    #[test]
    fn pointer_is_drawn_only_near_its_spot() {
        let blank = RgbaImage::from_pixel(80, 60, Rgba([0, 120, 0, 255]));
        let mut img = blank.clone();
        stamp_pointer(&mut img, (10.0, 10.0));
        let (l, t, w, h) = changed_rect(&blank, &img).expect("pointer drawn");
        assert!(l >= 9 && t >= 9, "{l},{t}");
        assert!(l + w <= 9 + 16 && t + h <= 9 + 24, "{w}x{h}");
    }

    #[test]
    fn pointer_off_the_frame_changes_nothing() {
        let blank = RgbaImage::from_pixel(40, 30, Rgba([0, 120, 0, 255]));
        for at in [(-17.0, 5.0), (5.0, -25.0), (40.0, 5.0), (5.0, 30.0)] {
            let mut img = blank.clone();
            stamp_pointer(&mut img, at);
            assert_eq!(img, blank, "{at:?}");
        }
        // Partly off the left edge still draws the part that shows.
        let mut img = blank.clone();
        stamp_pointer(&mut img, (-5.0, 5.0));
        assert_ne!(img, blank);
    }
}
