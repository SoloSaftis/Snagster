//! macOS: the Accessibility API (what's inside other programs' windows) and synthetic mouse
//! wheel events (for scrolling capture). Both need Snagster to be allowed under System
//! Settings > Privacy & Security > Accessibility; without that, `trusted` is false and
//! Snagster snaps to whole windows and lets the person scroll.
//!
//! The same code is compiled into the tests on every system, against stand-ins for the
//! system functions that find nothing, so it is type-checked everywhere.

use super::detect::{Info, Tree, element_label};
use std::ffi::{CString, c_char, c_void};
use std::ptr::null;

type Ref = *const c_void;

const UTF8: u32 = 0x0800_0100;
const AX_POINT: u32 = 1;
const AX_SIZE: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[cfg(target_os = "macos")]
mod sys {
    use super::{CGPoint, Ref};
    use std::ffi::c_char;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        pub fn AXIsProcessTrusted() -> u8;
        pub fn AXUIElementCreateApplication(pid: i32) -> Ref;
        pub fn AXUIElementCopyAttributeValue(element: Ref, attribute: Ref, value: *mut Ref) -> i32;
        pub fn AXUIElementSetMessagingTimeout(element: Ref, seconds: f32) -> i32;
        pub fn AXValueGetValue(value: Ref, kind: u32, out: *mut std::ffi::c_void) -> u8;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        pub fn CGEventCreateScrollWheelEvent2(source: Ref, units: u32, count: u32, wheel1: i32, wheel2: i32, wheel3: i32) -> Ref;
        pub fn CGEventSetLocation(event: Ref, at: CGPoint);
        pub fn CGEventPost(tap: u32, event: Ref);
        pub fn CGWarpMouseCursorPosition(at: CGPoint) -> i32;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        pub fn CFStringCreateWithCString(alloc: Ref, text: *const c_char, encoding: u32) -> Ref;
        pub fn CFStringGetCString(text: Ref, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
        pub fn CFGetTypeID(value: Ref) -> usize;
        pub fn CFStringGetTypeID() -> usize;
        pub fn CFArrayGetTypeID() -> usize;
        pub fn CFArrayGetCount(array: Ref) -> isize;
        pub fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
        pub fn CFRetain(value: Ref) -> Ref;
        pub fn CFRelease(value: Ref);
    }
}

/// Stand-ins with the same signatures, for the tests on other systems: nothing is there.
#[cfg(not(target_os = "macos"))]
#[allow(non_snake_case)]
mod sys {
    use super::{CGPoint, Ref};
    use std::ffi::c_char;
    use std::ptr::null;

    pub unsafe fn AXIsProcessTrusted() -> u8 {
        0
    }
    pub unsafe fn AXUIElementCreateApplication(_pid: i32) -> Ref {
        null()
    }
    pub unsafe fn AXUIElementCopyAttributeValue(_element: Ref, _attribute: Ref, _value: *mut Ref) -> i32 {
        -25205
    }
    pub unsafe fn AXUIElementSetMessagingTimeout(_element: Ref, _seconds: f32) -> i32 {
        0
    }
    pub unsafe fn AXValueGetValue(_value: Ref, _kind: u32, _out: *mut std::ffi::c_void) -> u8 {
        0
    }
    pub unsafe fn CGEventCreateScrollWheelEvent2(_source: Ref, _units: u32, _count: u32, _w1: i32, _w2: i32, _w3: i32) -> Ref {
        null()
    }
    pub unsafe fn CGEventSetLocation(_event: Ref, _at: CGPoint) {}
    pub unsafe fn CGEventPost(_tap: u32, _event: Ref) {}
    pub unsafe fn CGWarpMouseCursorPosition(_at: CGPoint) -> i32 {
        0
    }
    pub unsafe fn CFStringCreateWithCString(_alloc: Ref, _text: *const c_char, _encoding: u32) -> Ref {
        null()
    }
    pub unsafe fn CFStringGetCString(_text: Ref, _buffer: *mut c_char, _size: isize, _encoding: u32) -> u8 {
        0
    }
    pub unsafe fn CFGetTypeID(_value: Ref) -> usize {
        0
    }
    pub unsafe fn CFStringGetTypeID() -> usize {
        1
    }
    pub unsafe fn CFArrayGetTypeID() -> usize {
        2
    }
    pub unsafe fn CFArrayGetCount(_array: Ref) -> isize {
        0
    }
    pub unsafe fn CFArrayGetValueAtIndex(_array: Ref, _index: isize) -> Ref {
        null()
    }
    pub unsafe fn CFRetain(value: Ref) -> Ref {
        value
    }
    pub unsafe fn CFRelease(_value: Ref) {}
}

/// A Core Foundation object this code owns (released when dropped).
pub struct Cf(Ref);

impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { sys::CFRelease(self.0) };
        }
    }
}

/// Whether Snagster may read other programs' controls and send them mouse events.
pub fn trusted() -> bool {
    unsafe { sys::AXIsProcessTrusted() != 0 }
}

fn cf_string(s: &str) -> Option<Cf> {
    let c = CString::new(s).ok()?;
    let r = unsafe { sys::CFStringCreateWithCString(null(), c.as_ptr(), UTF8) };
    (!r.is_null()).then_some(Cf(r))
}

fn attr(element: Ref, name: &str) -> Option<Cf> {
    if element.is_null() {
        return None;
    }
    let key = cf_string(name)?;
    let mut out: Ref = null();
    let err = unsafe { sys::AXUIElementCopyAttributeValue(element, key.0, &mut out) };
    (err == 0 && !out.is_null()).then_some(Cf(out))
}

fn text(element: Ref, name: &str) -> String {
    let Some(v) = attr(element, name) else { return String::new() };
    if unsafe { sys::CFGetTypeID(v.0) != sys::CFStringGetTypeID() } {
        return String::new();
    }
    let mut buf = vec![0 as c_char; 512];
    if unsafe { sys::CFStringGetCString(v.0, buf.as_mut_ptr(), buf.len() as isize, UTF8) } == 0 {
        return String::new();
    }
    let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn pair(element: Ref, name: &str, kind: u32) -> Option<(f64, f64)> {
    let v = attr(element, name)?;
    let mut out = [0f64; 2];
    let ok = unsafe { sys::AXValueGetValue(v.0, kind, out.as_mut_ptr() as *mut c_void) };
    (ok != 0).then_some((out[0], out[1]))
}

/// An element's frame in points (top-left origin, like the rest of the desktop).
fn frame(element: Ref) -> Option<(f64, f64, f64, f64)> {
    let (x, y) = pair(element, "AXPosition", AX_POINT)?;
    let (w, h) = pair(element, "AXSize", AX_SIZE)?;
    Some((x, y, w, h))
}

fn list(element: Ref, name: &str) -> Vec<Cf> {
    let Some(a) = attr(element, name) else { return Vec::new() };
    if unsafe { sys::CFGetTypeID(a.0) != sys::CFArrayGetTypeID() } {
        return Vec::new();
    }
    let n = unsafe { sys::CFArrayGetCount(a.0) }.max(0);
    (0..n)
        .filter_map(|i| {
            let v = unsafe { sys::CFArrayGetValueAtIndex(a.0, i) };
            (!v.is_null()).then(|| Cf(unsafe { sys::CFRetain(v) }))
        })
        .collect()
}

/// The accessibility element of a program's window with about this frame (in points).
pub fn window_element(pid: u32, want: (f64, f64, f64, f64)) -> Option<Cf> {
    if pid == 0 {
        return None;
    }
    let app = unsafe { sys::AXUIElementCreateApplication(pid as i32) };
    if app.is_null() {
        return None;
    }
    let app = Cf(app);
    // A program that doesn't answer is skipped rather than waited for.
    unsafe { sys::AXUIElementSetMessagingTimeout(app.0, 0.3) };
    let off = |f: (f64, f64, f64, f64)| (f.0 - want.0).abs() + (f.1 - want.1).abs() + (f.2 - want.2).abs() + (f.3 - want.3).abs();
    list(app.0, "AXWindows")
        .into_iter()
        .filter_map(|w| frame(w.0).map(|f| (off(f), w)))
        .filter(|(d, _)| *d < 80.0)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, w)| w)
}

/// The accessibility tree, one level at a time.
pub struct Ax;

impl Tree for Ax {
    type Node = Cf;
    fn children(&mut self, n: &Cf, max: usize) -> Vec<(Cf, Info)> {
        list(n.0, "AXChildren")
            .into_iter()
            .take(max)
            .filter_map(|c| {
                let (x, y, w, h) = frame(c.0)?;
                let kind = super::detect::ax_kind(&text(c.0, "AXRole"));
                let mut name = text(c.0, "AXTitle");
                if name.is_empty() {
                    name = text(c.0, "AXDescription");
                }
                let label = element_label(&kind, &name);
                Some((c, Info { x, y, w, h, label, hidden: false }))
            })
            .collect()
    }
}

/// Turn the mouse wheel at `at` (points): positive `notches` scroll down (or right).
pub fn wheel(at: (f64, f64), horizontal: bool, notches: i32) {
    let p = CGPoint { x: at.0, y: at.1 };
    unsafe {
        sys::CGWarpMouseCursorPosition(p);
        // Lines; the second wheel is sideways. Negative moves the content up (scrolls down).
        let (v, h) = if horizontal { (0, -notches) } else { (-notches, 0) };
        let e = sys::CGEventCreateScrollWheelEvent2(null(), 1, 2, v, h, 0);
        if e.is_null() {
            return;
        }
        let e = Cf(e);
        sys::CGEventSetLocation(e.0, p);
        sys::CGEventPost(0, e.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_the_system_nothing_is_found() {
        // On a Mac the real functions answer; elsewhere the stand-ins find nothing, and
        // nothing may crash on the empty answers.
        if cfg!(target_os = "macos") {
            return;
        }
        assert!(!trusted());
        assert!(window_element(0, (0.0, 0.0, 10.0, 10.0)).is_none());
        assert!(window_element(42, (0.0, 0.0, 10.0, 10.0)).is_none());
        assert!(Ax.children(&Cf(null()), 10).is_empty());
        assert!(cf_string("AXChildren").is_none());
        assert_eq!(text(null(), "AXTitle"), "");
        assert!(frame(null()).is_none());
        assert!(list(null(), "AXChildren").is_empty());
        wheel((10.0, 10.0), false, 3);
        wheel((10.0, 10.0), true, -3);
    }
}
