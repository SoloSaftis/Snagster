//! Snagster's dialogs and secondary windows belong to its main window on X11: each one gets
//! `WM_TRANSIENT_FOR` set to the main window, so the window manager keeps it above Snagster,
//! moves it to the same desktop, leaves it out of the task bar and, for a fixed-size dialog
//! (`_NET_WM_WINDOW_TYPE_DIALOG`, set when it is made), shows no Minimise (KWin ignores the Motif
//! hints and minimises a parentless dialog on its own).
//!
//! eframe hands out only the main window's handle, so a dialog's X window is found by what
//! winit puts on it: Snagster's process id (`_NET_WM_PID`) and the dialog's title.
//! Nothing happens without an X server (Wayland, Windows, macOS).

use std::sync::atomic::{AtomicU32, Ordering};

/// The main window's X11 id; 0 until it is known or when the window isn't an X11 window.
static MAIN: AtomicU32 = AtomicU32::new(0);

/// Note the main window (from eframe's frame, every frame; cheap). Not X11: no main window.
pub(crate) fn set_main(handle: Option<raw_window_handle::RawWindowHandle>) {
    MAIN.store(handle.and_then(x11_id).unwrap_or(0), Ordering::Relaxed);
}

/// The X11 window id in a window handle (Xlib or XCB); None for any other kind of window.
pub(crate) fn x11_id(handle: raw_window_handle::RawWindowHandle) -> Option<u32> {
    use raw_window_handle::RawWindowHandle;
    match handle {
        RawWindowHandle::Xlib(h) => u32::try_from(h.window).ok().filter(|&id| id != 0),
        RawWindowHandle::Xcb(h) => Some(h.window.get()),
        _ => None,
    }
}

/// One top-level X11 window as the search sees it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Candidate {
    pub id: u32,
    pub pid: Option<u32>,
    pub title: Option<String>,
    /// Its `WM_TRANSIENT_FOR`, when it has one.
    pub transient_for: Option<u32>,
}

/// The window among `candidates` that is Snagster's (`pid`) window titled `title`, other than
/// the main window `main`. A window that already belongs to `main` isn't picked again, and two
/// matching windows (two boxes with one title) pick the newest, which X numbers highest.
pub(crate) fn pick(candidates: &[Candidate], pid: u32, title: &str, main: u32) -> Option<u32> {
    candidates
        .iter()
        .filter(|c| c.id != main && c.pid == Some(pid) && c.title.as_deref() == Some(title) && c.transient_for != Some(main))
        .map(|c| c.id)
        .max()
}

/// How many frames a new window is looked for before giving up (the window manager may take a
/// moment to map it; past this, something else is wrong and searching every frame is waste).
pub(crate) const TRIES: u32 = 120;

/// What is known about each open window: tries left, or done.
#[derive(Debug, Default)]
pub(crate) struct Tracker(std::collections::HashMap<String, u32>);

impl Tracker {
    /// Whether to look for `key`'s window this frame (and count the try).
    pub(crate) fn want(&mut self, key: &str) -> bool {
        let left = self.0.entry(key.to_string()).or_insert(TRIES);
        if *left == 0 {
            return false;
        }
        *left -= 1;
        true
    }
    /// `key`'s window has its parent: stop looking.
    pub(crate) fn done(&mut self, key: &str) {
        self.0.insert(key.to_string(), 0);
    }
    /// `key`'s window closed: a new one with that key is looked for again.
    pub(crate) fn closed(&mut self, key: &str) {
        self.0.remove(key);
    }
}

fn tracker() -> &'static std::sync::Mutex<Tracker> {
    static T: std::sync::OnceLock<std::sync::Mutex<Tracker>> = std::sync::OnceLock::new();
    T.get_or_init(Default::default)
}

/// Called every frame an `own_window` box (`key`, titled `title`) is open in a window of its own.
pub(crate) fn shown(key: &str, title: &str) {
    let main = MAIN.load(Ordering::Relaxed);
    if main == 0 {
        return;
    }
    let Ok(mut t) = tracker().lock() else { return };
    if !t.want(key) {
        return;
    }
    #[cfg(target_os = "linux")]
    if attach(title, main) {
        t.done(key);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = title;
}

/// Called when an `own_window` box closes.
pub(crate) fn closed(key: &str) {
    if let Ok(mut t) = tracker().lock() {
        t.closed(key);
    }
}

/// Find the window titled `title` and make it belong to `main`. True when that is done.
#[cfg(target_os = "linux")]
fn attach(title: &str, main: u32) -> bool {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode};
    use x11rb::wrapper::ConnectionExt as _;
    let Some(conn) = connection() else { return false };
    let Ok(conn) = conn.lock() else { return false };
    let Some((conn, root)) = conn.as_ref().map(|(c, r)| (c, *r)) else { return false };
    let atom = |name: &[u8]| conn.intern_atom(false, name).ok().and_then(|c| c.reply().ok()).map(|r| r.atom);
    let (Some(pid_atom), Some(name_atom), Some(utf8), Some(list_atom)) =
        (atom(b"_NET_WM_PID"), atom(b"_NET_WM_NAME"), atom(b"UTF8_STRING"), atom(b"_NET_CLIENT_LIST"))
    else {
        return false;
    };
    // The windows not yet mapped are children of the root; the window manager's own list has
    // the mapped ones (it puts them in frames of its own).
    let mut ids: Vec<u32> = conn.query_tree(root).ok().and_then(|c| c.reply().ok()).map(|r| r.children).unwrap_or_default();
    if let Some(list) = conn.get_property(false, root, list_atom, AtomEnum::WINDOW, 0, 4096).ok().and_then(|c| c.reply().ok())
        && let Some(v) = list.value32()
    {
        ids.extend(v);
    }
    ids.sort_unstable();
    ids.dedup();
    // Every question is sent before any answer is awaited: one round trip, not one per window.
    let asked: Vec<_> = ids
        .iter()
        .filter_map(|&id| {
            let pid = conn.get_property(false, id, pid_atom, AtomEnum::CARDINAL, 0, 1).ok()?;
            let name = conn.get_property(false, id, name_atom, utf8, 0, 256).ok()?;
            let parent = conn.get_property(false, id, AtomEnum::WM_TRANSIENT_FOR, AtomEnum::WINDOW, 0, 1).ok()?;
            Some((id, pid, name, parent))
        })
        .collect();
    let found: Vec<Candidate> = asked
        .into_iter()
        .map(|(id, pid, name, parent)| Candidate {
            id,
            pid: pid.reply().ok().and_then(|r| r.value32().and_then(|mut v| v.next())),
            title: name.reply().ok().and_then(|r| String::from_utf8(r.value).ok()),
            transient_for: parent.reply().ok().and_then(|r| r.value32().and_then(|mut v| v.next())),
        })
        .collect();
    let Some(id) = pick(&found, std::process::id(), title, main) else { return false };
    let ok = conn.change_property32(PropMode::REPLACE, id, AtomEnum::WM_TRANSIENT_FOR, AtomEnum::WINDOW, &[main]).is_ok();
    ok && conn.flush().is_ok()
}

/// One connection to the X server, kept for the whole run; None without one.
#[cfg(target_os = "linux")]
fn connection() -> Option<&'static std::sync::Mutex<Option<(x11rb::rust_connection::RustConnection, u32)>>> {
    static C: std::sync::OnceLock<std::sync::Mutex<Option<(x11rb::rust_connection::RustConnection, u32)>>> = std::sync::OnceLock::new();
    use x11rb::connection::Connection as _;
    Some(C.get_or_init(|| {
        std::sync::Mutex::new(x11rb::connect(None).ok().map(|(c, screen)| {
            let root = c.setup().roots.get(screen).map_or(0, |s| s.root);
            (c, root)
        }))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: u32, pid: u32, title: &str, transient_for: Option<u32>) -> Candidate {
        Candidate { id, pid: Some(pid), title: Some(title.into()), transient_for }
    }

    #[test]
    fn the_dialog_is_found_by_process_and_title() {
        let main = 0x0400_0001;
        let list = [c(main, 7, "New image", None), c(0x0400_0010, 7, "Settings", None), c(0x0400_0020, 7, "New image", None), c(0x0500_0001, 9, "New image", None)];
        assert_eq!(pick(&list, 7, "New image", main), Some(0x0400_0020), "Snagster's own, not the main window (same title) or another program's");
        assert_eq!(pick(&list, 7, "Resize", main), None, "no window with that title yet");
        assert_eq!(pick(&list, 8, "New image", main), None, "another process");
    }

    #[test]
    fn a_window_that_already_belongs_is_not_picked_again_and_the_newest_wins() {
        let main = 1;
        let list = [c(5, 7, "Rename", Some(main)), c(9, 7, "Rename", None), c(3, 7, "Rename", None)];
        assert_eq!(pick(&list, 7, "Rename", main), Some(9));
        assert_eq!(pick(&list[..1], 7, "Rename", main), None);
        let unnamed = [Candidate { id: 4, pid: Some(7), title: None, transient_for: None }, Candidate { id: 6, pid: None, title: Some("Rename".into()), transient_for: None }];
        assert_eq!(pick(&unnamed, 7, "Rename", main), None, "a window without pid or title isn't guessed at");
    }

    #[test]
    fn only_x11_handles_have_an_id() {
        use raw_window_handle::{RawWindowHandle, WaylandWindowHandle, XcbWindowHandle, XlibWindowHandle};
        assert_eq!(x11_id(RawWindowHandle::Xlib(XlibWindowHandle::new(0x0420_0007))), Some(0x0420_0007));
        assert_eq!(x11_id(RawWindowHandle::Xlib(XlibWindowHandle::new(0))), None);
        assert_eq!(x11_id(RawWindowHandle::Xcb(XcbWindowHandle::new(std::num::NonZeroU32::new(42).unwrap()))), Some(42));
        let surface = std::ptr::NonNull::<std::ffi::c_void>::dangling();
        assert_eq!(x11_id(RawWindowHandle::Wayland(WaylandWindowHandle::new(surface))), None);
    }

    #[test]
    fn a_window_is_looked_for_a_while_then_left_and_again_after_it_reopens() {
        let mut t = Tracker::default();
        let tries = (0..TRIES + 5).filter(|_| t.want("dialog")).count();
        assert_eq!(tries as u32, TRIES, "gives up after {TRIES} frames");
        t.closed("dialog");
        assert!(t.want("dialog"), "a new window with the key is looked for");
        t.done("dialog");
        assert!(!t.want("dialog"), "found: no more searching");
        assert!(t.want("settings"), "each window on its own");
    }

    #[test]
    fn without_an_x11_main_window_nothing_is_searched() {
        set_main(None);
        shown("no-main", "Anything");
        assert!(tracker().lock().unwrap().0.get("no-main").is_none(), "not even counted");
        tracker().lock().unwrap().want("closing");
        closed("closing");
        assert!(tracker().lock().unwrap().0.get("closing").is_none(), "a closed window is forgotten");
    }
}
