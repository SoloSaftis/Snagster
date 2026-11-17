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

pub mod detect;
#[cfg(any(target_os = "macos", test))]
mod mac;
pub mod stitch;

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
    /// The system's handle of a top-level window (0 for controls, or when unknown).
    pub handle: u64,
    /// The program it belongs to (0 when unknown).
    pub pid: u32,
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
    /// From desktop pixels to the system's units (points on a Retina Mac).
    pub space: detect::Space,
}

pub struct CaptureResult {
    pub kind: CaptureKind,
    pub shot: Result<Shot, String>,
}

/// A call into the screen-capture library that can't take Snagster down. On some Wayland
/// desktops it panics instead of returning an error (WSLg: "UnsupportedVersion" from its
/// Wayland part, where the compositor lacks the screencopy protocol); that becomes an error
/// here, so Snagster says it can't see the screen rather than closing.
pub(crate) fn guarded<T, E: std::fmt::Display>(f: impl FnOnce() -> Result<T, E>) -> Result<T, String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r.map_err(|e| e.to_string()),
        Err(p) => {
            let why = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
            Err(format!("the screen library failed ({why})"))
        }
    }
}

/// The screens (none when the screen library can't see them; see `guarded`).
pub fn monitors() -> Vec<MonitorInfo> {
    guarded(|| Ok::<_, String>(monitors_raw())).unwrap_or_default()
}

/// The size of the screen Snagster's window is on, in pixels, as the window system tells the
/// window (set every frame by the window).
static WINDOW_SCREEN: std::sync::Mutex<Option<(u32, u32)>> = std::sync::Mutex::new(None);

/// Remember the size of the screen the window is on (pixels).
pub fn remember_window_screen(size: (u32, u32)) {
    if size.0 > 0 && size.1 > 0 {
        *WINDOW_SCREEN.lock().unwrap_or_else(|p| p.into_inner()) = Some(size);
    }
}

/// The screens to offer when the screen library sees none: inside a Flatpak on Wayland it
/// can't (it lists screens through X11, which the sandbox doesn't give), yet the desktop's
/// screen sharing still works, so the screen the window is on stands in.
fn stand_in_screens(listed: Vec<MonitorInfo>, wayland: bool, window_screen: Option<(u32, u32)>) -> Vec<MonitorInfo> {
    match window_screen {
        Some((width, height)) if listed.is_empty() && wayland => vec![MonitorInfo { id: 0, name: "Screen".into(), x: 0, y: 0, width, height, primary: true }],
        _ => listed,
    }
}

fn monitors_raw() -> Vec<MonitorInfo> {
    let listed = monitors_listed();
    stand_in_screens(listed, desktop_screenshots_only(), *WINDOW_SCREEN.lock().unwrap_or_else(|p| p.into_inner()))
}

fn monitors_listed() -> Vec<MonitorInfo> {
    let Ok(all) = guarded(xcap::Monitor::all) else { return Vec::new() };
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

/// The screen holding `at` (or the first one).
pub fn monitor_at(mons: &[MonitorInfo], at: (i32, i32)) -> Option<&MonitorInfo> {
    mons.iter().find(|m| at.0 >= m.x && at.1 >= m.y && at.0 < m.x + m.width as i32 && at.1 < m.y + m.height as i32).or(mons.first())
}

/// The desktop area of the screen under the mouse pointer (or the first screen).
pub fn monitor_under_cursor() -> Result<(i32, i32, u32, u32), String> {
    let mons = monitors();
    let at = cursor_pos().unwrap_or((0, 0));
    monitor_at(&mons, at).map(|m| (m.x, m.y, m.width, m.height)).ok_or_else(|| "no screen found".to_string())
}

/// The visible, titled windows (not Snagster, not minimised), front to back.
///
/// `xcap::Window::all` already lists windows front to back on Windows, macOS and Linux, so
/// the order is kept as it comes (asking each window for its `z()` enumerates every window
/// again, which made this O(n²)). The program name is looked up once per process.
pub fn list_windows() -> Vec<WindowInfo> {
    windows_and_programs().into_iter().map(|(w, _)| w).collect()
}

/// The windows `list_windows` gives, each with its program's process id (0 when unknown).
fn windows_and_programs() -> Vec<(WindowInfo, u32)> {
    guarded(|| Ok::<_, String>(windows_and_programs_raw())).unwrap_or_default()
}

fn windows_and_programs_raw() -> Vec<(WindowInfo, u32)> {
    let me = std::process::id();
    let Ok(all) = guarded(xcap::Window::all) else { return Vec::new() };
    let mut apps = AppNames::default();
    let mut out = Vec::new();
    for w in all {
        let pid = w.pid().ok();
        if pid == Some(me) || w.is_minimized().unwrap_or(true) {
            continue;
        }
        let title = w.title().unwrap_or_default();
        let (Ok(x), Ok(y), Ok(width), Ok(height), Ok(id)) = (w.x(), w.y(), w.width(), w.height(), w.id()) else {
            continue;
        };
        if !listable(&title, width, height) {
            continue;
        }
        let app = apps.get(pid, || w.app_name().unwrap_or_default());
        let label = window_label(title, &app);
        out.push((WindowInfo { id, label, app, x, y, width, height }, pid.unwrap_or(0)));
    }
    out
}

/// Program names by process id, so a program with many windows is looked up once.
#[derive(Default)]
struct AppNames(std::collections::HashMap<u32, String>);

impl AppNames {
    fn get(&mut self, pid: Option<u32>, look_up: impl FnOnce() -> String) -> String {
        match pid {
            Some(p) => self.0.entry(p).or_insert_with(look_up).clone(),
            None => look_up(),
        }
    }
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
                        handle: 0,
                        pid: 0,
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
                handle: h as usize as u64,
                pid: pid_of(h),
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
        windows_and_programs()
            .into_iter()
            .map(|(w, pid)| Object { x: w.x, y: w.y, width: w.width, height: w.height, label: w.label, children: Vec::new(), handle: w.id as u64, pid })
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
            let app = guarded(xcap::Window::all)
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

fn xcap_monitor_at(x: i32, y: i32) -> Result<xcap::Monitor, String> {
    guarded(|| xcap::Monitor::from_point(x, y)).or_else(|_| {
        let all = guarded(xcap::Monitor::all).map_err(|e| e.to_string())?;
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
        space: detect::Space { origin, scale: 1.0 },
    }
}

fn grab_monitor(m: &xcap::Monitor) -> Result<Shot, String> {
    let image = guarded(|| m.capture_image())?;
    let name = m.friendly_name().or_else(|_| m.name()).unwrap_or_else(|_| "Screen".into());
    let mut s = shot(image, (m.x().unwrap_or(0), m.y().unwrap_or(0)), String::new());
    if s.title.is_empty() {
        s.title = name;
    }
    Ok(s)
}

/// Every monitor in one picture, placed by its desktop position.
pub fn grab_all_screens() -> Result<Shot, String> {
    guarded(grab_all_screens_raw)
}

fn grab_all_screens_raw() -> Result<Shot, String> {
    let all = guarded(xcap::Monitor::all).map_err(|e| e.to_string())?;
    if all.is_empty() {
        return Err("no screen found".into());
    }
    let mut parts = Vec::new();
    for m in &all {
        let img = guarded(|| m.capture_image())?;
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

/// A desktop rectangle, grabbed from the monitor it sits on (see `guarded`).
pub fn grab_rect(x: i32, y: i32, w: u32, h: u32) -> Result<RgbaImage, String> {
    desktop_screenshot_budget()?;
    guarded(|| grab_rect_raw(x, y, w, h))
}

/// The captures a whole-desktop picture from the Screenshot portal can stand in for: a screen,
/// all screens, and the ones picked on a frozen picture of the screen. Windows, menus and the
/// last area keep their own way (and their own "not available" messages).
#[cfg(any(target_os = "linux", test))]
fn portal_covers(kind: &CaptureKind) -> bool {
    matches!(
        kind,
        CaptureKind::Screen
            | CaptureKind::AllScreens
            | CaptureKind::Monitor(_)
            | CaptureKind::Pick(..)
            | CaptureKind::WindowPick
            | CaptureKind::Fixed(..)
            | CaptureKind::ColorPicker
            | CaptureKind::Ruler
    )
}

/// Whether the screens can be listed directly (Linux: not in a sandbox without X11, nor on a
/// Wayland desktop xcap can't read). When they can't, pictures come from the Screenshot portal.
#[cfg(target_os = "linux")]
fn screens_listable() -> bool {
    guarded(xcap::Monitor::all).is_ok_and(|all| !all.is_empty())
}

/// The screen through the desktop's portals (Linux, see `crate::portal`). On Wayland: one
/// picture of a screen share, which the desktop asks about once and then remembers. Elsewhere
/// (a Flatpak on X11): the Screenshot portal.
#[cfg(target_os = "linux")]
fn portal_shot() -> Result<Shot, String> {
    if desktop_screenshots_only() {
        let (image, origin) = crate::portal::still(false, SHARE_WAIT)?;
        return Ok(shot(image, origin, "Screen".into()));
    }
    let image = crate::portal::screenshot(false)?;
    Ok(shot(image, (0, 0), "Screen".into()))
}

fn grab_rect_raw(x: i32, y: i32, w: u32, h: u32) -> Result<RgbaImage, String> {
    #[cfg(target_os = "linux")]
    if desktop_screenshots_only() || !screens_listable() {
        let all = portal_shot()?;
        return crop_desktop(&all.image, all.origin, x, y, w, h).ok_or_else(|| "that area is off screen".to_string());
    }
    let m = xcap_monitor_at(x + w as i32 / 2, y + h as i32 / 2)?;
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
    let win = guarded(xcap::Window::all)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|w| w.id().ok() == Some(id))
        .ok_or_else(|| "that window has closed".to_string())?;
    let image = guarded(|| win.capture_image())?;
    Ok(shot(image, (win.x().unwrap_or(0), win.y().unwrap_or(0)), title))
}

/// Linux on Wayland: the screen can only be read through the desktop's screenshot service
/// (GNOME Shell, the Screenshot portal). Each picture costs the desktop a flash, a sound or a
/// permission check, so pictures are taken one at a time, never as a stream (screen sharing is
/// for that: `portal::Cast`).
pub fn desktop_screenshots_only() -> bool {
    #[cfg(target_os = "linux")]
    {
        let var = |k: &str| std::env::var_os(k).unwrap_or_default().to_string_lossy().to_lowercase();
        var("XDG_SESSION_TYPE") == "wayland" || !var("WAYLAND_DISPLAY").is_empty()
    }
    #[cfg(not(target_os = "linux"))]
    false
}

/// At most this many desktop screenshots in `SCREENSHOT_WINDOW` (Wayland): more means something
/// is asking for a stream of them, which floods the desktop with flashes and sounds.
const SCREENSHOT_LIMIT: usize = 4;
const SCREENSHOT_WINDOW: Duration = Duration::from_secs(10);

/// Whether another desktop screenshot may be taken `now`, given the times of the recent ones
/// (which this updates).
fn screenshot_allowed(recent: &mut std::collections::VecDeque<Instant>, now: Instant) -> bool {
    while recent.front().is_some_and(|t| now.duration_since(*t) >= SCREENSHOT_WINDOW) {
        recent.pop_front();
    }
    if recent.len() >= SCREENSHOT_LIMIT {
        return false;
    }
    recent.push_back(now);
    true
}

/// The limit above, for this run (Wayland only; elsewhere screen copies are cheap and silent).
fn desktop_screenshot_budget() -> Result<(), String> {
    if !desktop_screenshots_only() {
        return Ok(());
    }
    static RECENT: std::sync::Mutex<std::collections::VecDeque<Instant>> = std::sync::Mutex::new(std::collections::VecDeque::new());
    let mut recent = RECENT.lock().unwrap_or_else(|p| p.into_inner());
    if screenshot_allowed(&mut recent, Instant::now()) {
        Ok(())
    } else {
        Err("Snagster stopped asking the desktop for screenshots: too many in a row. Live screen sources use the desktop's screen sharing instead.".into())
    }
}

/// The captures that, on Wayland, open the desktop's own screenshot tool: picking a window
/// (Snagster can't see the other windows there) and a plain rectangle (the person picks it in
/// the desktop's tool). A whole screen needs no picking: it is a silent picture of the
/// remembered screen share (the desktop asks once, see `portal::still`).
#[cfg(any(target_os = "linux", test))]
pub(crate) fn desktop_tool_covers(kind: &CaptureKind) -> bool {
    matches!(kind, CaptureKind::WindowPick | CaptureKind::Pick(CutShape::Rect, PickPurpose::Capture))
}

/// Whether scrolling capture can work here: it turns other windows' scroll wheels and takes a
/// picture after each turn, and a Wayland desktop allows neither (xdotool needs X11, and every
/// picture is a desktop screenshot).
pub fn scrolling_supported() -> bool {
    !desktop_screenshots_only()
}

/// What scrolling capture says where it isn't available (translated in the window).
pub const SCROLL_UNAVAILABLE: &str = "Scrolling capture isn't available on Wayland desktops yet: capture the visible part instead.";

/// Whether `kind` is a scrolling capture (automatic or by hand).
pub fn is_scrolling(kind: &CaptureKind) -> bool {
    matches!(kind, CaptureKind::Pick(_, PickPurpose::Scrolling { .. } | PickPurpose::ManualScroll))
}

/// Take the picture `kind` asks for (see `guarded`: the screen library can't take Snagster down).
pub fn grab(kind: &CaptureKind, fallback_at: (i32, i32)) -> Result<Shot, String> {
    desktop_screenshot_budget()?;
    guarded(|| grab_raw(kind, fallback_at))
}

/// Linux on Wayland, a capture started from Snagster's window: the desktop's own screenshot
/// tool, its picture as one screen-sized shot.
#[cfg(target_os = "linux")]
pub fn grab_with_desktop_tool() -> Result<Shot, String> {
    desktop_screenshot_budget()?;
    let image = crate::portal::screenshot(true)?;
    Ok(shot(image, (0, 0), "Screenshot".into()))
}

fn grab_raw(kind: &CaptureKind, fallback_at: (i32, i32)) -> Result<Shot, String> {
    // Wayland: always through the portals (the screen library would ask the desktop for a
    // screenshot of its own, confirmed each time).
    #[cfg(target_os = "linux")]
    if portal_covers(kind) && (desktop_screenshots_only() || !screens_listable()) {
        return portal_shot();
    }
    let at = cursor_pos().unwrap_or(fallback_at);
    match kind {
        CaptureKind::Screen => grab_monitor(&xcap_monitor_at(at.0, at.1)?),
        CaptureKind::AllScreens => grab_all_screens(),
        CaptureKind::Monitor(id) => {
            let all = guarded(xcap::Monitor::all).map_err(|e| e.to_string())?;
            let m = all.into_iter().find(|m| m.id().ok() == Some(*id)).ok_or("that screen is gone")?;
            grab_monitor(&m)
        }
        CaptureKind::Pick(..) | CaptureKind::WindowPick | CaptureKind::Fixed(..) | CaptureKind::ColorPicker | CaptureKind::Ruler => {
            let mut objs = objects();
            let m = xcap_monitor_at(at.0, at.1)?;
            let mut s = grab_monitor(&m)?;
            // macOS reports windows in points, the picture is in pixels.
            if cfg!(target_os = "macos") {
                s.space.scale = detect::Space::scale_for(s.image.width(), m.width().unwrap_or(0));
            }
            s.space.scale_objects(&mut objs);
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
        CaptureKind::Menus if !menus_supported() => Err(MENUS_UNAVAILABLE.into()),
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

/// Whether open menus can be found and captured here. Only Windows tells menus apart from
/// other windows (their window class); macOS and Linux would need the Accessibility API or
/// the window manager's menu type, which Snagster doesn't read yet.
pub fn menus_supported() -> bool {
    cfg!(windows)
}

/// What the Menu capture says where it isn't available (translated in the window).
pub const MENUS_UNAVAILABLE: &str = "Menu capture isn't available on this system yet: open the menu and capture an area instead.";

// ---------- permission to see the screen ----------

/// Whether Snagster may take pictures of the screen. Only macOS asks: there the person turns
/// Snagster on under System Settings > Privacy & Security > Screen Recording, and a program run
/// without a logged-in desktop (only a remote shell) has no screen at all. Windows and Linux
/// are always `Granted` (on Wayland the system's own sharing dialog asks while capturing).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScreenAccess {
    #[default]
    Granted,
    /// macOS: the Screen Recording permission is off.
    NotGranted,
    /// macOS: no window server session (a remote shell, or nobody logged in).
    NoDesktop,
}

impl ScreenAccess {
    pub fn granted(self) -> bool {
        self == ScreenAccess::Granted
    }
}

/// What a grab says on macOS when Snagster may not record the screen (translated in the window).
pub const SCREEN_ACCESS_NEEDED: &str =
    "Snagster needs permission to record the screen: System Settings > Privacy & Security > Screen & System Audio Recording (Screen Recording on older macOS), then turn on Snagster and choose Quit & Reopen.";
/// What a grab says on macOS without a logged-in desktop (translated in the window).
pub const NO_DESKTOP: &str = "Snagster can't see a screen: it needs a logged-in desktop, not only a remote shell. Log in on the Mac's screen (or with Screen Sharing) and start Snagster there.";
/// What `--video-test` and `--stream-test` print instead of FAIL when the screen can't be seen.
pub const NO_ACCESS_SKIP: &str = "skipped: no screen access (Screen Recording permission or no desktop)";
/// The Screen Recording page of macOS's System Settings.
pub const SCREEN_SETTINGS_URL: &str = "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

/// The answer from what macOS says: whether there is a desktop session, and what
/// `CGPreflightScreenCaptureAccess` says (`None`: macOS older than 10.15, which doesn't ask).
pub fn access_from(desktop: bool, preflight: Option<bool>) -> ScreenAccess {
    match (desktop, preflight) {
        (false, _) => ScreenAccess::NoDesktop,
        (true, Some(false)) => ScreenAccess::NotGranted,
        (true, _) => ScreenAccess::Granted,
    }
}

/// Whether Snagster may take pictures of the screen now (asks the system; never shows anything).
pub fn screen_access() -> ScreenAccess {
    #[cfg(target_os = "macos")]
    {
        mac::screen_access()
    }
    // Windows and Linux don't ask: like a macOS too old to have the permission.
    #[cfg(not(target_os = "macos"))]
    {
        access_from(true, None)
    }
}

/// What a check run (`--video-test`, `--stream-test`) prints instead of a failure when the
/// screen can't be seen: [`NO_ACCESS_SKIP`], or `None` when it can and a failure is real.
pub fn check_skip(access: ScreenAccess) -> Option<&'static str> {
    if access.granted() { None } else { Some(NO_ACCESS_SKIP) }
}

/// macOS: show the system's own "Snagster would like to record this computer's screen" prompt
/// (`CGRequestScreenCaptureAccess`; macOS shows it once, later calls only answer). True when
/// access is granted. Other systems: true.
pub fn request_screen_access() -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::request_screen_access().unwrap_or(true)
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Open the Screen Recording page of System Settings (macOS only).
pub fn open_screen_settings() -> Result<(), String> {
    if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(SCREEN_SETTINGS_URL).spawn().map(|_| ()).map_err(|e| e.to_string())
    } else {
        Err("not available on this system".into())
    }
}

/// The message for a failed grab, recording or stream: the system's own words (`raw`, such as
/// macOS's "Failed to copy data") unless Snagster may not see the screen, which gets a clear,
/// translated message saying what to do. `access` is [`screen_access`] at the time of the error.
pub fn grab_error_message(raw: &str, access: ScreenAccess) -> String {
    match access {
        ScreenAccess::Granted => raw.to_string(),
        ScreenAccess::NotGranted => crate::i18n::t(SCREEN_ACCESS_NEEDED).to_string(),
        ScreenAccess::NoDesktop => crate::i18n::t(NO_DESKTOP).to_string(),
    }
}

/// Wait `delay_ms`, grab, and send the result back; the UI thread restores the window.
pub fn spawn(kind: CaptureKind, delay_ms: u64, at: (i32, i32), ctx: eframe::egui::Context, tx: Sender<CaptureResult>) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(delay_ms));
        #[cfg(target_os = "linux")]
        if desktop_screenshots_only() && desktop_tool_covers(&kind) {
            // The desktop's tool has already picked the area: straight to the editor.
            let _ = tx.send(CaptureResult { kind: CaptureKind::AllScreens, shot: grab_with_desktop_tool() });
            ctx.request_repaint();
            return;
        }
        let shot = grab(&kind, at);
        let _ = tx.send(CaptureResult { kind, shot });
        ctx.request_repaint();
    });
}

// ---------- scrolling and panoramic capture ----------

/// How a scrolling capture goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollOptions {
    /// Sideways (a panorama) instead of down.
    pub horizontal: bool,
    /// The person scrolls; Snagster only keeps taking pictures.
    pub manual: bool,
    /// Wait after each scroll for the program to redraw.
    pub delay_ms: u64,
    /// Mouse wheel notches per scroll.
    pub notches: u32,
    /// Scrolls at most.
    pub max_steps: u32,
    /// The longest picture, in pixels along the scrolling.
    pub max_length: u32,
    /// Go back to the top (or the left end) first.
    pub from_start: bool,
    /// From the desktop pixels the area is given in to the system's own units.
    pub space: detect::Space,
    /// Stop after this long, however far it got (AI apps waiting on a person who scrolls).
    pub time_limit: Option<Duration>,
}

impl ScrollOptions {
    /// The options a scrolling capture gets from the settings, the same for the window, presets
    /// and AI apps. `manual`: the person asked to scroll themselves; where Snagster can't turn
    /// other programs' mouse wheels (`can_scroll` false: Wayland, a Mac without the
    /// Accessibility permission, Linux without xdotool) it is manual anyway.
    pub fn from_settings(s: &crate::settings::Settings, horizontal: bool, manual: bool, can_scroll: bool, space: detect::Space) -> ScrollOptions {
        ScrollOptions {
            horizontal,
            manual: manual || !can_scroll,
            delay_ms: s.scroll_wait_ms,
            notches: s.scroll_notches,
            max_steps: s.scroll_max_steps,
            max_length: s.scroll_max_height,
            from_start: s.scroll_from_top,
            space,
            time_limit: None,
        }
    }
}

impl Default for ScrollOptions {
    fn default() -> Self {
        ScrollOptions {
            horizontal: false,
            manual: false,
            delay_ms: 350,
            notches: 3,
            max_steps: 40,
            max_length: 20000,
            from_start: false,
            space: detect::Space::default(),
            time_limit: None,
        }
    }
}

/// How far a scrolling capture has got, for the bar that shows it.
#[derive(Default)]
pub struct ScrollProgress {
    pub frames: std::sync::atomic::AtomicU32,
    pub length: std::sync::atomic::AtomicU32,
}

/// Whether Snagster can turn the mouse wheel for other programs here. Where it can't (Wayland,
/// a Mac without the Accessibility permission, Linux without `xdotool`), the person scrolls.
pub fn can_scroll_itself() -> bool {
    #[cfg(windows)]
    {
        true
    }
    #[cfg(target_os = "macos")]
    {
        mac::trusted()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        xdotool_available()
    }
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

#[cfg(target_os = "macos")]
fn scroll_wheel(at: (i32, i32), horizontal: bool, notches: i32) {
    mac::wheel((at.0 as f64, at.1 as f64), horizontal, notches);
}

#[cfg(all(unix, not(target_os = "macos")))]
fn scroll_wheel(at: (i32, i32), horizontal: bool, notches: i32) {
    let _ = std::process::Command::new("xdotool").args(xdotool_args(at, horizontal, notches)).output();
}

/// `xdotool` arguments that move the pointer to `at` and turn the wheel: buttons 4 and 5 are
/// up and down, 6 and 7 left and right.
#[cfg(any(all(unix, not(target_os = "macos")), test))]
fn xdotool_args(at: (i32, i32), horizontal: bool, notches: i32) -> Vec<String> {
    let button = match (horizontal, notches >= 0) {
        (false, true) => 5,
        (false, false) => 4,
        (true, true) => 7,
        (true, false) => 6,
    };
    ["mousemove", &at.0.to_string(), &at.1.to_string(), "click", "--repeat", &notches.unsigned_abs().max(1).to_string(), "--delay", "15", &button.to_string()]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn xdotool_available() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        let wayland = std::env::var("XDG_SESSION_TYPE").is_ok_and(|s| s.eq_ignore_ascii_case("wayland"));
        !wayland
            && std::env::var_os("DISPLAY").is_some()
            && std::process::Command::new("xdotool").arg("version").output().is_ok_and(|o| o.status.success())
    })
}

/// Windows: bring the window under `at` to the front and press Ctrl+Home, which takes pages,
/// documents and lists to their top.
#[cfg(windows)]
fn press_ctrl_home(at: (i32, i32)) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, SendInput, VK_CONTROL, VK_HOME,
    };
    if let Some(h) = win::window_at(at.0, at.1) {
        if win::pid_of(h) == std::process::id() {
            return;
        }
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(h) };
        std::thread::sleep(Duration::from_millis(120));
    }
    let key = |vk: u16, up: bool, ext: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: (if up { KEYEVENTF_KEYUP } else { 0 }) | (if ext { KEYEVENTF_EXTENDEDKEY } else { 0 }),
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let keys = [key(VK_CONTROL, false, false), key(VK_HOME, false, true), key(VK_HOME, true, true), key(VK_CONTROL, true, false)];
    unsafe { SendInput(keys.len() as u32, keys.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
}

/// Whether Esc is held down right now, anywhere (Snagster's bar may not have the keyboard).
fn escape_down() -> bool {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
        GetAsyncKeyState(VK_ESCAPE as i32) as u16 & 0x8000 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Wheel back to the top (or left end) until the picture stops changing.
fn scroll_to_start(center: (i32, i32), o: &ScrollOptions, stop: &AtomicBool, grab: &dyn Fn() -> Result<RgbaImage, String>) -> Result<(), String> {
    #[cfg(windows)]
    if !o.horizontal {
        press_ctrl_home(center);
    }
    let wait = Duration::from_millis(o.delay_ms.clamp(60, 400));
    std::thread::sleep(wait);
    let mut last = stitch::rows(&grab()?);
    for _ in 0..40 {
        if stop.load(Ordering::Relaxed) || escape_down() {
            break;
        }
        scroll_wheel(center, o.horizontal, -15);
        std::thread::sleep(wait);
        let now = stitch::rows(&grab()?);
        if now == last {
            break;
        }
        last = now;
    }
    Ok(())
}

/// Capture a desktop area (desktop pixels) while scrolling the content under it, and stitch
/// the frames. Stops at the end of the content, at the step or length limit, on `stop`, or
/// when Esc is pressed (or after `time_limit`). Sideways works the same on turned frames. The
/// window, presets and AI apps all scroll through here, with `ScrollOptions::from_settings`.
pub fn scroll_capture(
    rect: (i32, i32, u32, u32),
    o: &ScrollOptions,
    stop: Arc<AtomicBool>,
    progress: Option<Arc<ScrollProgress>>,
) -> Result<RgbaImage, String> {
    let (x, y, w, h) = rect;
    // The system's own units for grabbing and pointing (points on a Retina Mac).
    let (sx, sy, sw, sh) = o.space.from_pixels(x, y, w, h);
    let (gx, gy, gw, gh) = (sx.round() as i32, sy.round() as i32, sw.round().max(2.0) as u32, sh.round().max(2.0) as u32);
    let center = (gx + gw as i32 / 2, gy + gh as i32 / 2);
    let old_cursor = cursor_pos();
    let horizontal = o.horizontal;
    let turn = move |img: RgbaImage| if horizontal { image::imageops::rotate90(&img) } else { img };
    let grab = move || grab_rect(gx, gy, gw, gh).map(turn);
    if !o.manual && o.from_start {
        scroll_to_start(center, o, &stop, &grab)?;
    }
    let mut st = stitch::Stitcher::new(grab()?, o.max_length.max(100));
    let report = |st: &stitch::Stitcher| {
        if let Some(p) = &progress {
            p.frames.store(st.frames, Ordering::Relaxed);
            p.length.store(st.height(), Ordering::Relaxed);
        }
    };
    report(&st);
    let steps = if o.manual { 6000 } else { o.max_steps.clamp(1, 500) };
    let notches = o.notches.clamp(1, 50) as i32;
    let started = Instant::now();
    for _ in 0..steps {
        if stop.load(Ordering::Relaxed) || escape_down() || o.time_limit.is_some_and(|t| started.elapsed() >= t) {
            break;
        }
        if o.manual {
            std::thread::sleep(Duration::from_millis(120));
        } else {
            scroll_wheel(center, horizontal, notches);
            std::thread::sleep(Duration::from_millis(o.delay_ms.clamp(20, 5000)));
        }
        let frame = grab()?;
        let step = st.push(&frame);
        report(&st);
        match step {
            stitch::Step::Full => break,
            stitch::Step::Same if !o.manual && st.still() >= 2 => break,
            _ => {}
        }
    }
    #[cfg(windows)]
    if let Some(c) = old_cursor {
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos(c.0, c.1) };
    }
    let _ = old_cursor;
    let out = st.finish();
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
                        && let Ok((m, img)) = guarded(|| xcap_monitor_at(p.0, p.1).and_then(|m| m.capture_image().map(|i| (m, i)).map_err(|e| e.to_string())))
                    {
                        let (mx, my) = guarded(|| Ok::<_, String>((m.x().unwrap_or(0), m.y().unwrap_or(0)))).unwrap_or((0, 0));
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
    GIF_LIVE.store(false, Ordering::Relaxed);
    std::thread::spawn(move || {
        background_thread(false);
        let (x, y, w, h) = rect;
        let fps = fps.clamp(1, 30);
        let scale = if w > max_width.max(160) { max_width.max(160) as f32 / w as f32 } else { 1.0 };
        let (ow, oh) = (((w as f32 * scale) as u32).clamp(1, 65535), ((h as f32 * scale) as u32).clamp(1, 65535));
        // Wayland: one screen share (the person clicks Share once) instead of a desktop
        // screenshot per frame; the GIF starts when the first picture arrives.
        let feed = if desktop_screenshots_only() {
            let o = crate::video::FeedOptions { fps: fps.max(10), cursor: pointer, fit: None, every_frame: false };
            let started = crate::video::screen_feed(rect, o).and_then(|f| wait_for_first_picture(|| f.frames(), || f.error(), &stop, SHARE_WAIT).map(|_| f));
            match started {
                Ok(f) => Some(f),
                Err(e) => {
                    let _ = done.send(GifDone { path, frames: 0, seconds: 0.0, result: Err(e) });
                    ctx.request_repaint();
                    return;
                }
            }
        } else {
            None
        };
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
        GIF_LIVE.store(true, Ordering::Relaxed);
        let start = Instant::now();
        let mut next = start;
        let mut err = None;
        let mut prev: Option<RgbaImage> = None;
        let mut pending: Option<(RgbaImage, u32, u32, u32)> = None;
        let mut captured = 0u32;
        while !stop.load(Ordering::Relaxed) && start.elapsed().as_secs() < max_s as u64 {
            let grabbed = match &feed {
                Some(f) => feed_picture(f),
                None => grab_rect(x, y, w, h),
            };
            match grabbed {
                Ok(mut img) => {
                    if img.dimensions() != (ow, oh) {
                        // A plain box filter: far cheaper than smooth resampling, fine for GIFs.
                        img = if img.width() > ow { image::imageops::thumbnail(&img, ow, oh) } else { image::imageops::resize(&img, ow, oh, image::imageops::FilterType::Nearest) };
                    }
                    // (A screen share draws the pointer in itself.)
                    if pointer && feed.is_none() && let Some(c) = cursor_pos() {
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
        GIF_LIVE.store(false, Ordering::Relaxed);
        let _ = done.send(GifDone { path, frames, seconds, result });
        ctx.request_repaint();
    });
}

/// Whether a GIF recording is taking pictures (false while it waits for screen sharing).
static GIF_LIVE: AtomicBool = AtomicBool::new(false);

/// Whether the GIF recording has started taking pictures (on Wayland it first waits for the
/// person to click Share).
pub fn gif_live() -> bool {
    GIF_LIVE.load(Ordering::Relaxed)
}

/// How long a GIF recording waits for the person to click Share in the desktop's dialog.
const SHARE_WAIT: Duration = Duration::from_secs(120);

/// Wait until a screen share has its first picture (`frames` counts them): the desktop asks
/// the person first. Fails when the share failed (`error` before any picture), Stop was
/// pressed, or `limit` passed.
fn wait_for_first_picture(frames: impl Fn() -> u64, error: impl Fn() -> Option<String>, stop: &AtomicBool, limit: Duration) -> Result<(), String> {
    let t = Instant::now();
    loop {
        if frames() > 0 {
            return Ok(());
        }
        if let Some(e) = error() {
            return Err(e);
        }
        if stop.load(Ordering::Relaxed) {
            return Err("Stopped before screen sharing started.".into());
        }
        if t.elapsed() >= limit {
            return Err("Screen sharing didn't start: nothing was recorded.".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The newest picture of a screen share.
fn feed_picture(f: &crate::video::ScreenFeed) -> Result<RgbaImage, String> {
    f.with_latest(|fr| (fr.number > 0).then(|| RgbaImage::from_raw(fr.width, fr.height, fr.rgba.clone())).flatten())
        .ok_or_else(|| f.error().unwrap_or_else(|| "Screen sharing stopped.".into()))
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

    /// The screen library panicking (WSLg's Wayland, found by `--hw-report`) used to close
    /// Snagster; now it is an error like any other.
    #[test]
    fn a_panicking_screen_library_is_an_error_not_a_crash() {
        let r: Result<u32, String> = guarded(|| -> Result<u32, String> { panic!("UnsupportedVersion") });
        assert_eq!(r, Err("the screen library failed (UnsupportedVersion)".to_string()));
        assert_eq!(guarded(|| Ok::<_, String>(7)), Ok(7));
        assert_eq!(guarded(|| Err::<u32, _>("no screen")), Err("no screen".to_string()));
    }

    /// A tall "page" whose every row has its own colour, so rows can be told apart.
    fn page(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| Rgba([((y % 32) * 8) as u8, ((y / 32) * 8) as u8, if x % 2 == 0 { 40 } else { 200 }, 255]))
    }

    #[test]
    fn the_screen_under_the_pointer_is_found() {
        let m = |id, x, y| MonitorInfo { id, name: String::new(), x, y, width: 100, height: 50, primary: id == 1 };
        let mons = [m(1, 0, 0), m(2, 100, 0), m(3, -100, 0)];
        assert_eq!(monitor_at(&mons, (150, 10)).unwrap().id, 2);
        assert_eq!(monitor_at(&mons, (-1, 49)).unwrap().id, 3);
        assert_eq!(monitor_at(&mons, (0, 0)).unwrap().id, 1);
        assert_eq!(monitor_at(&mons, (5, 500)).unwrap().id, 1, "off every screen: the first");
        assert!(monitor_at(&[], (0, 0)).is_none());
    }

    fn obj(x: i32, y: i32, w: u32, h: u32, label: &str) -> Object {
        Object { x, y, width: w, height: h, label: label.into(), children: Vec::new(), handle: 0, pid: 0 }
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

    /// Inside the Flatpak on Wayland the screen library saw no screens (it asks X11, which the
    /// sandbox doesn't give), so Studio and recording had nothing to share ("no screen found").
    #[test]
    fn the_window_screen_stands_in_when_no_screen_is_listed_on_wayland() {
        let s = stand_in_screens(Vec::new(), true, Some((1920, 1200)));
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].x, s[0].y, s[0].width, s[0].height, s[0].primary), (0, 0, 1920, 1200, true));
        // A listed screen always wins.
        let real = MonitorInfo { id: 7, name: "DP-1".into(), x: 10, y: 0, width: 800, height: 600, primary: true };
        let s = stand_in_screens(vec![real], true, Some((1920, 1200)));
        assert_eq!(s[0].id, 7);
        // Not on X11 or Windows (no screen there is a real problem), nor before the window knows.
        assert!(stand_in_screens(Vec::new(), false, Some((1920, 1200))).is_empty());
        assert!(stand_in_screens(Vec::new(), true, None).is_empty());
        // The window tells it; nonsense sizes are ignored.
        remember_window_screen((0, 900));
        remember_window_screen((2560, 1440));
        assert_eq!(*WINDOW_SCREEN.lock().unwrap(), Some((2560, 1440)));
    }

    /// A GIF on Wayland took a desktop screenshot per frame, so the screenshot limit stopped it
    /// after a few frames; it now waits for one screen share's first picture.
    #[test]
    fn a_gif_waits_for_the_screen_share_to_start() {
        use std::cell::Cell;
        let stop = AtomicBool::new(false);
        let long = Duration::from_secs(5);
        // The picture arrives after the person clicks Share.
        let n = Cell::new(0u64);
        let frames = || {
            n.set(n.get() + 1);
            if n.get() > 3 { 1 } else { 0 }
        };
        assert!(wait_for_first_picture(frames, || None, &stop, long).is_ok());
        // The person said no: that error comes back.
        let e = wait_for_first_picture(|| 0, || Some("The person said no.".to_string()), &stop, long).unwrap_err();
        assert!(e.contains("said no"));
        // A picture that already arrived wins over a later error.
        assert!(wait_for_first_picture(|| 1, || Some("x".to_string()), &stop, long).is_ok());
        // Nobody answers: it gives up at the limit.
        let e = wait_for_first_picture(|| 0, || None, &stop, Duration::from_millis(50)).unwrap_err();
        assert!(e.contains("didn't start"));
        // Stop while waiting.
        stop.store(true, Ordering::Relaxed);
        let e = wait_for_first_picture(|| 0, || None, &stop, long).unwrap_err();
        assert!(e.contains("Stopped"));
    }

    /// Studio's screen preview asked GNOME for a screenshot every frame on Wayland (19 portal
    /// and 38 GNOME Shell screenshots in 10 s), flooding the desktop until it had to restart.
    #[test]
    fn a_stream_of_desktop_screenshots_is_refused() {
        let mut recent = std::collections::VecDeque::new();
        let t0 = Instant::now();
        for i in 0..SCREENSHOT_LIMIT {
            assert!(screenshot_allowed(&mut recent, t0 + Duration::from_millis(i as u64 * 33)), "picture {i}");
        }
        assert!(!screenshot_allowed(&mut recent, t0 + Duration::from_millis(200)), "a fifth one in a moment is refused");
        // Once the window has passed, single pictures are fine again.
        assert!(screenshot_allowed(&mut recent, t0 + SCREENSHOT_WINDOW + Duration::from_millis(200)));
    }

    #[test]
    fn the_desktop_tool_picks_windows_and_areas_on_wayland() {
        assert!(desktop_tool_covers(&CaptureKind::WindowPick));
        assert!(desktop_tool_covers(&CaptureKind::Pick(CutShape::Rect, PickPurpose::Capture)));
        // Whole screens: a silent picture (asked once), no tool to click through each time.
        assert!(!desktop_tool_covers(&CaptureKind::AllScreens));
        assert!(!desktop_tool_covers(&CaptureKind::Screen));
        assert!(!desktop_tool_covers(&CaptureKind::Monitor(0)));
        // Picking an area to record, scroll through or read keeps Snagster's overlay.
        assert!(!desktop_tool_covers(&CaptureKind::Pick(CutShape::Rect, PickPurpose::RecordGif)));
        assert!(!desktop_tool_covers(&CaptureKind::Pick(CutShape::Rect, PickPurpose::AllInOne)));
        assert!(!desktop_tool_covers(&CaptureKind::Pick(CutShape::Rect, PickPurpose::GrabText)));
        assert!(!desktop_tool_covers(&CaptureKind::Fixed(640, 480)));
        // A window by id, the last area and menus keep their own way.
        assert!(!desktop_tool_covers(&CaptureKind::Window(1, "x".into())));
        assert!(!desktop_tool_covers(&CaptureKind::LastRegion(0, 0, 10, 10)));
        assert!(!desktop_tool_covers(&CaptureKind::Menus));
        // The color picker and the ruler need Snagster's own overlay.
        assert!(!desktop_tool_covers(&CaptureKind::ColorPicker));
        // So does a cut that isn't a rectangle.
        assert!(!desktop_tool_covers(&CaptureKind::Pick(CutShape::Ellipse, PickPurpose::Capture)));
        assert!(!desktop_tool_covers(&CaptureKind::Pick(CutShape::Freehand, PickPurpose::Capture)));
        assert!(is_scrolling(&CaptureKind::Pick(CutShape::Rect, PickPurpose::Scrolling { horizontal: true })));
        assert!(is_scrolling(&CaptureKind::Pick(CutShape::Rect, PickPurpose::ManualScroll)));
        assert!(!is_scrolling(&CaptureKind::Pick(CutShape::Rect, PickPurpose::Capture)));
    }

    #[test]
    fn the_screenshot_portal_stands_in_only_for_whole_screen_pictures() {
        assert!(portal_covers(&CaptureKind::Screen));
        assert!(portal_covers(&CaptureKind::AllScreens));
        assert!(portal_covers(&CaptureKind::Monitor(3)));
        assert!(portal_covers(&CaptureKind::Pick(CutShape::Rect, PickPurpose::Capture)));
        assert!(portal_covers(&CaptureKind::ColorPicker));
        // These keep their own way, and their own message where they can't work.
        assert!(!portal_covers(&CaptureKind::Menus));
        assert!(!portal_covers(&CaptureKind::Window(1, "x".into())));
        assert!(!portal_covers(&CaptureKind::ActiveWindow));
        assert!(!portal_covers(&CaptureKind::LastRegion(0, 0, 10, 10)));
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
        assert_eq!(menus_supported(), cfg!(windows));
        if !menus_supported() {
            assert_eq!(grab(&CaptureKind::Menus, (0, 0)).err().as_deref(), Some(MENUS_UNAVAILABLE), "says so instead of finding nothing");
        }
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
    fn program_names_are_looked_up_once_per_process() {
        let mut names = AppNames::default();
        let mut calls = 0;
        for _ in 0..5 {
            assert_eq!(
                names.get(Some(7), || {
                    calls += 1;
                    "Editor".into()
                }),
                "Editor"
            );
        }
        assert_eq!(calls, 1, "one look-up for five windows of the same program");
        assert_eq!(names.get(Some(8), || "Browser".into()), "Browser");
        // Without a process id there is nothing to remember it by.
        let mut unknown = 0;
        for _ in 0..2 {
            names.get(None, || {
                unknown += 1;
                String::new()
            });
        }
        assert_eq!(unknown, 2);
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
    fn xdotool_turns_the_right_wheel() {
        let a = xdotool_args((10, 20), false, 3);
        assert_eq!(a, ["mousemove", "10", "20", "click", "--repeat", "3", "--delay", "15", "5"]);
        assert_eq!(xdotool_args((0, 0), false, -15).last().unwrap(), "4");
        assert_eq!(xdotool_args((0, 0), true, 2).last().unwrap(), "7");
        assert_eq!(xdotool_args((0, 0), true, -2).last().unwrap(), "6");
        assert_eq!(xdotool_args((0, 0), true, -2)[5], "2");
        assert_eq!(xdotool_args((0, 0), false, 0)[5], "1", "at least one notch");
    }

    #[test]
    fn scroll_options_come_from_the_settings() {
        let s = crate::settings::Settings { scroll_notches: 7, scroll_max_height: 9000, scroll_from_top: false, scroll_wait_ms: 123, scroll_max_steps: 17, ..Default::default() };
        let space = detect::Space { origin: (5, 6), scale: 2.0 };
        let o = ScrollOptions::from_settings(&s, true, false, true, space);
        assert!(o.horizontal && !o.manual && !o.from_start);
        assert_eq!((o.notches, o.max_length, o.delay_ms, o.max_steps, o.space, o.time_limit), (7, 9000, 123, 17, space, None));
        // Where Snagster can't scroll other programs (Wayland), the person scrolls.
        assert!(ScrollOptions::from_settings(&s, false, false, false, space).manual);
        assert!(ScrollOptions::from_settings(&s, false, true, true, space).manual, "asked for");
        let d = ScrollOptions::from_settings(&crate::settings::Settings::default(), false, false, true, space);
        assert!(d.from_start, "the settings start at the top unless told not to");
    }

    #[test]
    fn scroll_options_have_sensible_defaults() {
        let o = ScrollOptions::default();
        assert!(!o.horizontal && !o.manual && !o.from_start);
        assert_eq!((o.notches, o.max_length), (3, 20000));
        assert_eq!(o.space, detect::Space::default());
        let p = ScrollProgress::default();
        assert_eq!(p.frames.load(Ordering::Relaxed), 0);
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

    #[test]
    fn screen_access_from_what_macos_says() {
        assert_eq!(access_from(true, Some(true)), ScreenAccess::Granted);
        assert_eq!(access_from(true, Some(false)), ScreenAccess::NotGranted);
        // macOS before 10.15 has no Screen Recording permission.
        assert_eq!(access_from(true, None), ScreenAccess::Granted);
        // No desktop wins: a remote shell can't record even with the permission.
        assert_eq!(access_from(false, Some(true)), ScreenAccess::NoDesktop);
        assert_eq!(access_from(false, Some(false)), ScreenAccess::NoDesktop);
        assert_eq!(access_from(false, None), ScreenAccess::NoDesktop);
        assert!(ScreenAccess::Granted.granted() && !ScreenAccess::NotGranted.granted() && !ScreenAccess::NoDesktop.granted());
        assert_eq!(ScreenAccess::default(), ScreenAccess::Granted);
    }

    #[test]
    fn grab_errors_say_what_to_do() {
        let raw = "Failed to copy data";
        // Allowed: the system's own words stay (something else went wrong).
        assert_eq!(grab_error_message(raw, ScreenAccess::Granted), raw);
        assert_eq!(grab_error_message("", ScreenAccess::Granted), "");
        let need = grab_error_message(raw, ScreenAccess::NotGranted);
        assert_eq!(need, crate::i18n::t(SCREEN_ACCESS_NEEDED));
        assert!(!need.contains(raw));
        assert_eq!(grab_error_message(raw, ScreenAccess::NoDesktop), crate::i18n::t(NO_DESKTOP));
        // Mapping a message twice (the recorder and the window) gives the same message.
        assert_eq!(grab_error_message(&need, ScreenAccess::NotGranted), need);
        assert_ne!(SCREEN_ACCESS_NEEDED, NO_DESKTOP);
        assert!(NO_ACCESS_SKIP.starts_with("skipped: no screen access"));
        assert_eq!(check_skip(ScreenAccess::Granted), None);
        assert_eq!(check_skip(ScreenAccess::NotGranted), Some(NO_ACCESS_SKIP));
        assert_eq!(check_skip(ScreenAccess::NoDesktop), Some(NO_ACCESS_SKIP));
        assert!(SCREEN_SETTINGS_URL.starts_with("x-apple.systempreferences:") && SCREEN_SETTINGS_URL.ends_with("Privacy_ScreenCapture"));
    }

    #[test]
    fn screen_access_elsewhere_is_always_granted() {
        if cfg!(target_os = "macos") {
            return;
        }
        assert_eq!(screen_access(), ScreenAccess::Granted);
        assert!(request_screen_access());
        assert!(open_screen_settings().is_err());
    }
}
