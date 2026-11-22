//! Live ink over the screen on macOS: a borderless, see-through panel on every screen
//! (`opaque = NO`, a clear background, no shadow) at the screen saver's level and on every
//! Space. It never makes Snagster the active app (a non-activating panel), and it is kept out
//! of screen copies (`sharingType = none`), so the stream gets the ink once, drawn into the
//! composed frame. While Draw is off it ignores the mouse; while it is on it takes it, except
//! over Snagster's own window when that is in front.
//!
//! AppKit wants its windows on the main thread (winit's), so the overlay's thread does the
//! pixel work and hands the main thread what to show twice a turn: a picture is a grid of image
//! tiles at the screen's own resolution, and only the tiles a stroke crosses are sent again.
//! The mouse on a panel comes in through a local event monitor on the main thread. Esc and ⌘Z
//! pressed in another program are read from the key state (they can't be taken from that
//! program as on Windows: it sees them too).

use super::Paint;
use crate::ink::{Area, Space};
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSBitmapFormat, NSBitmapImageRep, NSColor, NSDeviceRGBColorSpace, NSEvent, NSEventMask, NSEventType, NSImage, NSImageScaling, NSImageView, NSPanel, NSScreen,
    NSScreenSaverWindowLevel, NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowSharingType, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static STARTED: AtomicBool = AtomicBool::new(false);

/// A picture's tiles are this many pixels square.
const TILE: u32 = 256;

/// One screen: where it is on the desktop (points) and its pixels to a point.
type Screen = ((i32, i32, u32, u32), f32);

/// Start the overlay's thread (once; it runs while Snagster runs and shows nothing while there
/// is no ink and Draw is off).
pub fn run() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    if std::thread::Builder::new().name("snagster-ink".into()).spawn(main_loop).is_err() {
        STARTED.store(false, Ordering::SeqCst);
    }
}

/// Run `f` on the main thread and wait for it (None if the main thread isn't AppKit's).
fn on_main<R: Send>(f: impl FnOnce(MainThreadMarker) -> R + Send) -> Option<R> {
    let mut out = None;
    DispatchQueue::main().exec_sync(|| {
        if let Some(mtm) = MainThreadMarker::new() {
            out = Some(f(mtm));
        }
    });
    out
}

/// What the main thread tells the overlay's thread each turn.
struct Seen {
    screens: Vec<Screen>,
    /// The mouse pointer on the desktop.
    pointer: [f32; 2],
    /// Snagster is the active app, its window is shown, and where it is.
    ours: bool,
    shown: bool,
    front: Option<(i32, i32, u32, u32)>,
}

/// What a screen's panel should show: whether it is shown, whether it lets the mouse through,
/// and the tiles that changed (None: nothing in it any more).
struct Show {
    screen: Screen,
    show: bool,
    ignores: bool,
    tiles: Vec<(Area, Option<Vec<u8>>)>,
}

/// Esc and ⌘Z, read from the key state while Draw is on.
#[derive(Default)]
struct Keys {
    armed: bool,
    esc: bool,
    z: bool,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
    fn CGEventSourceFlagsState(state: i32) -> u64;
}

/// The keyboard as all programs see it (`kCGEventSourceStateCombinedSessionState`).
const COMBINED: i32 = 0;
const KEY_ESCAPE: u16 = 0x35;
const KEY_Z: u16 = 0x06;
const FLAG_SHIFT: u64 = 0x2_0000;
const FLAG_OPTION: u64 = 0x8_0000;
const FLAG_COMMAND: u64 = 0x10_0000;

impl Keys {
    /// Act on a fresh press of Esc or ⌘Z while Draw is `on` and another program is in front
    /// (Snagster's own window handles them itself).
    fn check(&mut self, on: bool, front_is_ours: bool) {
        if !on {
            self.armed = false;
            return;
        }
        // SAFETY: plain queries of the keyboard's state.
        let (esc, z) = unsafe { (CGEventSourceKeyState(COMBINED, KEY_ESCAPE), CGEventSourceKeyState(COMBINED, KEY_Z)) };
        let (new_esc, new_z) = (esc && !self.esc, z && !self.z);
        (self.esc, self.z) = (esc, z);
        // Keys already down when Draw was turned on aren't presses.
        if !std::mem::replace(&mut self.armed, true) || !(new_esc || new_z) {
            return;
        }
        let flags = unsafe { CGEventSourceFlagsState(COMBINED) };
        let has = |f: u64| flags & f != 0;
        if super::hook_takes(front_is_ours, new_esc, new_z, has(FLAG_COMMAND), has(FLAG_SHIFT), has(FLAG_OPTION)) {
            let now = Instant::now();
            crate::ink::with(|i| if new_esc { i.set_on(false, now) } else { i.undo(now) });
        }
    }
}

fn main_loop() {
    let mut paints: Vec<(Paint, Vec<u8>)> = Vec::new();
    let mut last_shown = Instant::now();
    let mut keys = Keys::default();
    loop {
        let (on, fading) = crate::ink::with(|i| (i.on, i.fading(Instant::now())));
        let wait = if on || fading { 16 } else if paints.iter().any(|p| p.0.shown) { 50 } else { 120 };
        std::thread::sleep(Duration::from_millis(wait));
        let Some(seen) = on_main(|mtm| {
            watch_mouse(mtm);
            seen(mtm)
        }) else {
            return;
        };
        let now = Instant::now();
        let (on, snap) = crate::ink::with(|i| (i.on, i.snapshot(now)));
        keys.check(on, seen.ours && seen.shown);
        let want = on || !snap.is_empty();
        if !want && !paints.iter().any(|p| p.0.shown) {
            // Nothing shown for a while: give the pictures' memory back.
            if !paints.is_empty() && last_shown.elapsed() > Duration::from_secs(30) {
                paints.clear();
                on_main(|mtm| show(mtm, Vec::new(), None));
            }
            continue;
        }
        last_shown = now;
        if seen.screens != paints.iter().map(|p| (p.0.rect, p.0.scale)).collect::<Vec<_>>() {
            paints = seen.screens.iter().map(|&(rect, scale)| Paint::new(rect, scale)).map(|p| {
                let buf = vec![0; p.bytes()];
                (p, buf)
            }).collect();
        }
        let accent = crate::theme::system_accent().unwrap_or(crate::theme::BRAND).to_array();
        let text = super::hint_text();
        let app = if on { super::cut_out(seen.ours, seen.shown, seen.front) } else { None };
        let over_app = app.is_some_and(|r| super::inside(r, seen.pointer));
        let mut out = Vec::with_capacity(paints.len());
        for (p, buf) in &mut paints {
            let (dirty, has) = p.step(&snap, on, Some((accent, &text)), (seen.pointer[0], seen.pointer[1]), app, buf);
            let tiles = dirty
                .map(|a| super::tiles_touching(a, p.size, TILE))
                .unwrap_or_default()
                .into_iter()
                .map(|t| {
                    let data = super::crop(buf, p.size.0, t);
                    let clear = data.chunks_exact(4).all(|q| q[3] == 0);
                    (t, (!clear).then_some(data))
                })
                .collect();
            out.push(Show { screen: (p.rect, p.scale), show: has, ignores: !on || over_app, tiles });
            p.shown = has;
        }
        on_main(|mtm| show(mtm, out, app));
    }
}

/// One screen's panel (main thread only).
struct Win {
    panel: Retained<NSPanel>,
    screen: Screen,
    /// The picture's size in pixels.
    size: (u32, u32),
    tiles: HashMap<(i64, i64), Retained<NSImageView>>,
    ignores: bool,
    shown: bool,
}

thread_local! {
    static WINS: RefCell<Vec<Win>> = const { RefCell::new(Vec::new()) };
    /// The main screen's height in points: macOS counts y up from its bottom.
    static MAIN_H: Cell<f64> = const { Cell::new(0.0) };
    static MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    /// Snagster's window while it is left out of the overlay (desktop points).
    static APP: Cell<Option<(i32, i32, u32, u32)>> = const { Cell::new(None) };
}

fn frame_of(r: NSRect) -> (f64, f64, f64, f64) {
    (r.origin.x, r.origin.y, r.size.width, r.size.height)
}

fn rect((x, y, w, h): (f64, f64, f64, f64)) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

/// The screens, the pointer and Snagster's window, as the main thread sees them.
fn seen(mtm: MainThreadMarker) -> Seen {
    let all = NSScreen::screens(mtm);
    let main_h = all.firstObject().map(|s| s.frame().size.height).unwrap_or(0.0);
    MAIN_H.set(main_h);
    let screens = all.iter().map(|s| (super::desk_from_cocoa(frame_of(s.frame()), main_h), s.backingScaleFactor() as f32)).filter(|(r, _)| r.2 > 0 && r.3 > 0).collect();
    let p = NSEvent::mouseLocation();
    let app = NSApplication::sharedApplication(mtm);
    let (shown, front) = match app.keyWindow().or_else(|| app.mainWindow()) {
        Some(w) => (w.isVisible() && !w.isMiniaturized(), Some(super::desk_from_cocoa(frame_of(w.frame()), main_h))),
        None => (false, None),
    };
    Seen { screens, pointer: super::desk_point((p.x, p.y), main_h), ours: app.isActive(), shown, front }
}

/// Bring the panels up to `list`: new panels when the screens changed, then the mouse, the
/// tiles and showing or hiding.
fn show(mtm: MainThreadMarker, list: Vec<Show>, app: Option<(i32, i32, u32, u32)>) {
    APP.set(app);
    WINS.with_borrow_mut(|wins| {
        if wins.iter().map(|w| w.screen).ne(list.iter().map(|s| s.screen)) {
            for w in wins.drain(..) {
                w.panel.orderOut(None);
                w.panel.close();
            }
            let main_h = MAIN_H.get();
            wins.extend(list.iter().map(|s| make(mtm, s.screen, main_h)));
        }
        for (w, s) in wins.iter_mut().zip(list) {
            if w.ignores != s.ignores {
                w.panel.setIgnoresMouseEvents(s.ignores);
                w.ignores = s.ignores;
            }
            for (tile, data) in s.tiles {
                set_tile(mtm, w, tile, data);
            }
            if w.shown != s.show {
                if s.show {
                    w.panel.orderFrontRegardless();
                } else {
                    w.panel.orderOut(None);
                }
                w.shown = s.show;
            }
        }
    });
}

fn make(mtm: MainThreadMarker, screen: Screen, main_h: f64) -> Win {
    let frame = rect(super::cocoa_from_desk(screen.0, main_h));
    let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(NSPanel::alloc(mtm), frame, style, NSBackingStoreType::Buffered, false);
    // SAFETY: `Win` holds the panel, so it must not free itself when closed.
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(false);
    panel.setIgnoresMouseEvents(true);
    panel.setLevel(NSScreenSaverWindowLevel);
    // Never in a screen copy: the stream gets the ink from the composed frame.
    panel.setSharingType(NSWindowSharingType::None);
    panel.setCollectionBehavior(NSWindowCollectionBehavior::CanJoinAllSpaces | NSWindowCollectionBehavior::Stationary | NSWindowCollectionBehavior::FullScreenAuxiliary | NSWindowCollectionBehavior::IgnoresCycle);
    panel.setHidesOnDeactivate(false);
    panel.setBecomesKeyOnlyIfNeeded(true);
    panel.setAnimationBehavior(NSWindowAnimationBehavior::None);
    panel.setFrame_display(frame, false);
    let p = Paint::new(screen.0, screen.1);
    Win { panel, screen, size: p.size, tiles: HashMap::new(), ignores: true, shown: false }
}

/// Show `data` (blue-green-red-alpha, premultiplied) in a panel's tile; None empties it.
fn set_tile(mtm: MainThreadMarker, w: &mut Win, tile: Area, data: Option<Vec<u8>>) {
    let key = (tile.0, tile.1);
    let Some(data) = data else {
        if let Some(v) = w.tiles.get(&key) {
            v.setImage(None);
        }
        return;
    };
    let (fx, fy, fw, fh) = super::tile_frame(tile, w.size, w.screen.1);
    let view = w.tiles.entry(key).or_insert_with(|| {
        let v = NSImageView::initWithFrame(NSImageView::alloc(mtm), rect((fx, fy, fw, fh)));
        v.setImageScaling(NSImageScaling::ScaleAxesIndependently);
        if let Some(content) = w.panel.contentView() {
            content.addSubview(&v);
        }
        v
    });
    let (tw, th) = ((tile.2 - tile.0) as isize, (tile.3 - tile.1) as isize);
    // SAFETY: the bitmap allocates its own pixels (no planes passed), `tw` x `th`, 4 bytes each
    // in rows of exactly `tw * 4`: blue, green, red, alpha in memory (32-bit little-endian
    // with alpha first), premultiplied.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            tw,
            th,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            NSBitmapFormat::ThirtyTwoBitLittleEndian | NSBitmapFormat::AlphaFirst,
            tw * 4,
            32,
        )
    };
    let Some(rep) = rep else { return };
    let bits = rep.bitmapData();
    if bits.is_null() {
        return;
    }
    // SAFETY: the bitmap holds `tw * th * 4` bytes and `data` is that tile's crop.
    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), bits, data.len().min((tw * th * 4) as usize)) };
    let img = NSImage::initWithSize(NSImage::alloc(), NSSize::new(fw, fh));
    img.addRepresentation(&rep);
    view.setImage(Some(&img));
}

/// Follow the mouse on the panels (once): presses, drags and releases there are strokes.
fn watch_mouse(_mtm: MainThreadMarker) {
    MONITOR.with_borrow_mut(|m| {
        if m.is_some() {
            return;
        }
        let block = RcBlock::new(|ev: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit hands the handler a live event.
            mouse(unsafe { ev.as_ref() });
            ev.as_ptr()
        });
        let mask = NSEventMask::LeftMouseDown | NSEventMask::LeftMouseDragged | NSEventMask::LeftMouseUp;
        // SAFETY: the handler only reads the event and passes it on unchanged.
        *m = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) };
    });
}

fn mouse(e: &NSEvent) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let n = e.windowNumber();
    let ours = WINS.with(|w| w.try_borrow().is_ok_and(|w| w.iter().any(|x| x.panel.windowNumber() == n)));
    if !ours {
        return;
    }
    let Some(win) = e.window(mtm) else { return };
    let (f, l) = (win.frame(), e.locationInWindow());
    let p = super::desk_point((f.origin.x + l.x, f.origin.y + l.y), MAIN_H.get());
    let now = Instant::now();
    match e.r#type() {
        // Not on Snagster's window (the panel lets the mouse through there a moment later).
        NSEventType::LeftMouseDown if !APP.get().is_some_and(|r| super::inside(r, p)) => crate::ink::with(|i| {
            if i.on {
                i.begin(Space::Desktop, p, now);
            }
        }),
        NSEventType::LeftMouseDragged => crate::ink::with(|i| i.extend(p)),
        NSEventType::LeftMouseUp => crate::ink::with(|i| i.end(now)),
        _ => {}
    }
}
