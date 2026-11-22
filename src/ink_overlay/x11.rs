//! Live ink over the screen on X11: an override-redirect window with a 32-bit (see-through)
//! visual on every monitor, filled from the ink. While Draw is off its input shape is empty, so
//! every click goes to the window below; while it is on, the overlay takes the clicks
//! everywhere but over Snagster's own window (when that is in front), and Esc and Ctrl+Z are
//! grabbed from the program in front.
//!
//! A see-through window needs a compositing manager (else it is black where it should be
//! clear): without one, nothing is shown and ink stays on the Studio preview (`can_overlay`).
//! The screen copies (Studio's sources) are of the composed screen, so they hold the overlay's
//! ink already (`IN_CAPTURES`), and the overlay draws nothing but ink.

use super::Paint;
use crate::ink::Space;
use std::os::fd::AsRawFd;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

type Res<T> = Result<T, Box<dyn std::error::Error>>;

static STARTED: AtomicBool = AtomicBool::new(false);

/// Keysyms of the keys the overlay takes while drawing.
const XK_ESCAPE: u32 = 0xff1b;
const XK_Z: u32 = 0x7a;

/// Whether ink can go over the screen on this X server: a compositing manager runs, and the
/// server has a 32-bit TrueColor visual and the SHAPE extension. Asked again every few seconds
/// (a compositor can be turned on and off), so the Studio tab always says what is true.
pub fn can_overlay() -> bool {
    static SEEN: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut g = SEEN.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, v)) = *g
        && at.elapsed() < Duration::from_secs(5)
    {
        return v;
    }
    let v = x11rb::connect(None).ok().is_some_and(|(c, n)| visual(&c, n).is_some());
    *g = Some((Instant::now(), v));
    v
}

/// The 32-bit visual to draw with, when the overlay can work on screen `n` now.
fn visual(c: &RustConnection, n: usize) -> Option<Visualid> {
    let screen = c.setup().roots.get(n)?;
    let composited = c
        .intern_atom(true, format!("_NET_WM_CM_S{n}").as_bytes())
        .ok()
        .and_then(|k| k.reply().ok())
        .filter(|a| a.atom != x11rb::NONE)
        .and_then(|a| c.get_selection_owner(a.atom).ok()?.reply().ok())
        .is_some_and(|o| o.owner != x11rb::NONE);
    if !composited || c.shape_query_version().ok().and_then(|k| k.reply().ok()).is_none() {
        return None;
    }
    screen
        .allowed_depths
        .iter()
        .filter(|d| d.depth == 32)
        .flat_map(|d| d.visuals.iter())
        .find(|v| v.class == VisualClass::TRUE_COLOR && v.red_mask == 0xff_0000 && v.green_mask == 0xff00 && v.blue_mask == 0xff)
        .map(|v| v.visual_id)
}

/// Start the overlay's thread (once; it runs while Snagster runs and shows nothing while there
/// is no ink and Draw is off). Nothing on Wayland.
pub fn run() {
    if crate::capture::desktop_screenshots_only() || STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new().name("snagster-ink".into()).spawn(|| {
        if let Err(e) = main_loop() {
            eprintln!("snagster: the ink overlay stopped: {e}");
        }
        super::IN_CAPTURES.store(false, Ordering::Relaxed);
        // Draw again starts it again.
        STARTED.store(false, Ordering::SeqCst);
    });
    if spawned.is_err() {
        STARTED.store(false, Ordering::SeqCst);
    }
}

/// One monitor's overlay window.
struct Overlay {
    win: Window,
    gc: Gcontext,
    paint: Paint,
    /// The picture (blue-green-red-alpha, premultiplied: a 32-bit visual's pixels).
    buf: Vec<u8>,
    /// The input shape it has now.
    input: Vec<(i16, i16, u16, u16)>,
}

/// What the loop keeps from the X server.
struct Server {
    c: RustConnection,
    n: usize,
    root: Window,
    active: Atom,
    pid: Atom,
    cursor: Cursor,
    esc: Option<Keycode>,
    z: Option<Keycode>,
    /// The server wants pixels' bytes most significant first.
    msb: bool,
}

fn atom(c: &RustConnection, name: &str) -> Res<Atom> {
    Ok(c.intern_atom(false, name.as_bytes())?.reply()?.atom)
}

impl Server {
    fn open() -> Res<Server> {
        let (c, n) = x11rb::connect(None)?;
        let root = c.setup().roots[n].root;
        let msb = c.setup().image_byte_order == ImageOrder::MSB_FIRST;
        let (active, pid) = (atom(&c, "_NET_ACTIVE_WINDOW")?, atom(&c, "_NET_WM_PID")?);
        // The pen: X's crosshair from the cursor font.
        let font = c.generate_id()?;
        c.open_font(font, b"cursor")?;
        let cursor = c.generate_id()?;
        c.create_glyph_cursor(cursor, font, font, 34, 35, 0, 0, 0, 0xffff, 0xffff, 0xffff)?;
        c.close_font(font)?;
        let (min, max) = (c.setup().min_keycode, c.setup().max_keycode);
        let map = c.get_keyboard_mapping(min, max - min + 1)?.reply()?;
        let per = map.keysyms_per_keycode as usize;
        let esc = super::keycode_for(&map.keysyms, per, min, XK_ESCAPE);
        let z = super::keycode_for(&map.keysyms, per, min, XK_Z);
        Ok(Server { c, n, root, active, pid, cursor, esc, z, msb })
    }

    fn make(&self, rect: (i32, i32, u32, u32), visual: Visualid, colormap: Colormap) -> Res<Overlay> {
        let c = &self.c;
        let win = c.generate_id()?;
        let events = EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::BUTTON1_MOTION | EventMask::EXPOSURE;
        let aux = CreateWindowAux::new().background_pixel(0).border_pixel(0).colormap(colormap).override_redirect(1).event_mask(events).cursor(self.cursor);
        let (w, h) = (rect.2.min(u16::MAX as u32) as u16, rect.3.min(u16::MAX as u32) as u16);
        c.create_window(32, win, self.root, rect.0 as i16, rect.1 as i16, w, h, 0, WindowClass::INPUT_OUTPUT, visual, &aux)?;
        c.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, b"Snagster ink")?;
        c.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"snagster-ink\0Snagster\0")?;
        // Clicks go through until Draw is on.
        c.shape_rectangles(shape::SO::SET, shape::SK::INPUT, ClipOrdering::UNSORTED, win, 0, 0, &[])?;
        let gc = c.generate_id()?;
        c.create_gc(gc, win, &CreateGCAux::new())?;
        let paint = Paint::new(rect, 1.0);
        let buf = vec![0; paint.bytes()];
        Ok(Overlay { win, gc, paint, buf, input: Vec::new() })
    }

    fn drop_overlay(&self, o: Overlay) {
        let _ = self.c.free_gc(o.gc);
        let _ = self.c.destroy_window(o.win);
    }

    /// Put `area` of the overlay's picture on the screen, in requests the server takes.
    fn put(&self, o: &Overlay, area: crate::ink::Area) -> Res<()> {
        let (x0, x1) = (area.0, area.2);
        let row = (x1 - x0) as usize * 4;
        let max = self.c.maximum_request_bytes().saturating_sub(64).max(row);
        for (y0, y1) in super::bands(area, row, max) {
            let mut data = super::crop(&o.buf, o.paint.size.0, (x0, y0, x1, y1));
            if self.msb {
                for p in data.chunks_exact_mut(4) {
                    p.reverse();
                }
            }
            self.c.put_image(ImageFormat::Z_PIXMAP, o.win, o.gc, (x1 - x0) as u16, (y1 - y0) as u16, x0 as i16, y0 as i16, 0, 32, &data)?;
        }
        Ok(())
    }

    /// Whether the window in front is one of Snagster's, shown, and where (its frame).
    fn front(&self) -> (bool, bool, Option<(i32, i32, u32, u32)>) {
        self.front_window().unwrap_or((false, false, None))
    }

    fn front_window(&self) -> Res<(bool, bool, Option<(i32, i32, u32, u32)>)> {
        let c = &self.c;
        let active = c.get_property(false, self.root, self.active, AtomEnum::WINDOW, 0, 1)?.reply()?.value32().and_then(|mut v| v.next()).unwrap_or(0);
        if active == 0 {
            return Ok((false, false, None));
        }
        let pid = c.get_property(false, active, self.pid, AtomEnum::CARDINAL, 0, 1)?.reply()?.value32().and_then(|mut v| v.next());
        if pid != Some(std::process::id()) {
            return Ok((false, false, None));
        }
        let shown = c.get_window_attributes(active)?.reply()?.map_state == MapState::VIEWABLE;
        // The window manager's frame round it: the root's child it is in.
        let mut frame = active;
        for _ in 0..16 {
            let t = c.query_tree(frame)?.reply()?;
            if t.parent == self.root || t.parent == x11rb::NONE {
                break;
            }
            frame = t.parent;
        }
        let g = c.get_geometry(frame)?.reply()?;
        let at = c.translate_coordinates(frame, self.root, 0, 0)?.reply()?;
        let b = 2 * g.border_width as u32;
        Ok((true, shown, Some((at.dst_x as i32, at.dst_y as i32, g.width as u32 + b, g.height as u32 + b))))
    }

    /// Take Esc and Ctrl+Z from every program (`on`), or give them back. Ctrl+Z also with Caps
    /// Lock and Num Lock on, which X counts as modifiers.
    fn grab(&self, on: bool) {
        let mut keys = Vec::new();
        if let Some(k) = self.esc {
            keys.push((k, ModMask::ANY));
        }
        if let Some(k) = self.z {
            for m in [ModMask::CONTROL, ModMask::CONTROL | ModMask::LOCK, ModMask::CONTROL | ModMask::M2, ModMask::CONTROL | ModMask::LOCK | ModMask::M2] {
                keys.push((k, m));
            }
        }
        for (k, m) in keys {
            // Another program holding the same key is told to the event queue as an error,
            // which is ignored: then that program keeps it.
            let _ = if on { self.c.grab_key(false, self.root, m, k, GrabMode::ASYNC, GrabMode::ASYNC) } else { self.c.ungrab_key(k, self.root, m) };
        }
    }

    /// Handle what the server sent; whether there was anything.
    fn events(&self, overlays: &mut [Overlay]) -> Res<bool> {
        let mut any = false;
        while let Some(ev) = self.c.poll_for_event()? {
            any = true;
            match ev {
                Event::ButtonPress(e) if e.detail == 1 => {
                    let p = [e.root_x as f32, e.root_y as f32];
                    crate::ink::with(|i| {
                        if i.on {
                            i.begin(Space::Desktop, p, Instant::now());
                        }
                    });
                }
                Event::MotionNotify(e) => {
                    let p = [e.root_x as f32, e.root_y as f32];
                    crate::ink::with(|i| i.extend(p));
                }
                Event::ButtonRelease(e) if e.detail == 1 => crate::ink::with(|i| i.end(Instant::now())),
                Event::KeyPress(e) => {
                    let st = u16::from(e.state);
                    let has = |m: KeyButMask| st & u16::from(m) != 0;
                    let (esc, z) = (Some(e.detail) == self.esc, Some(e.detail) == self.z);
                    // The grab is only there while another program is in front.
                    if super::hook_takes(false, esc, z, has(KeyButMask::CONTROL), has(KeyButMask::SHIFT), has(KeyButMask::MOD1)) {
                        let now = Instant::now();
                        crate::ink::with(|i| {
                            if i.on {
                                if esc { i.set_on(false, now) } else { i.undo(now) }
                            }
                        });
                    }
                }
                Event::Expose(e) => {
                    for o in overlays.iter_mut().filter(|o| o.win == e.window) {
                        o.paint.redraw = true;
                    }
                }
                _ => {}
            }
        }
        Ok(any)
    }

    /// Wait up to `ms` for the server to send something.
    fn wait(&self, ms: i32) {
        let mut fd = libc::pollfd { fd: self.c.stream().as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd for the connection's open socket.
        unsafe { libc::poll(&mut fd, 1, ms) };
    }
}

fn main_loop() -> Res<()> {
    let x = Server::open()?;
    let mut overlays: Vec<Overlay> = Vec::new();
    // The visual and colormap while the overlay can work (checked every few seconds).
    let mut can: Option<(Visualid, Colormap)> = None;
    let mut checked_at: Option<Instant> = None;
    let mut listed_at: Option<Instant> = None;
    let mut raised_at = Instant::now();
    let mut last_shown = Instant::now();
    let mut grabbed = false;
    loop {
        // Wake for the mouse at once, else a few times a second.
        let (on, fading) = crate::ink::with(|i| (i.on, i.fading(Instant::now())));
        let wait = if on || fading { 16 } else if overlays.iter().any(|o| o.paint.shown) { 50 } else { 120 };
        if !x.events(&mut overlays)? {
            x.wait(wait);
            x.events(&mut overlays)?;
        }
        let now = Instant::now();
        let (on, snap) = crate::ink::with(|i| (i.on, i.snapshot(now)));
        // The compositor, again every few seconds (it can be turned off).
        if checked_at.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            checked_at = Some(now);
            match (visual(&x.c, x.n), can) {
                (Some(v), Some((cv, _))) if v == cv => {}
                (Some(v), old) => {
                    if let Some((_, cm)) = old {
                        let _ = x.c.free_colormap(cm);
                    }
                    let cm = x.c.generate_id()?;
                    x.c.create_colormap(ColormapAlloc::NONE, cm, x.root, v)?;
                    can = Some((v, cm));
                    for o in overlays.drain(..) {
                        x.drop_overlay(o);
                    }
                }
                (None, old) => {
                    for o in overlays.drain(..) {
                        x.drop_overlay(o);
                    }
                    if let Some((_, cm)) = old {
                        let _ = x.c.free_colormap(cm);
                    }
                    can = None;
                }
            }
        }
        // Esc and Ctrl+Z belong to the ink only while drawing over another program.
        let (ours, shown, front) = if on && can.is_some() { x.front() } else { (false, false, None) };
        let grab = on && can.is_some() && !(ours && shown);
        if grab != grabbed {
            x.grab(grab);
            grabbed = grab;
        }
        let Some((visual, colormap)) = can else {
            // No compositor: ink is on the preview only.
            super::IN_CAPTURES.store(false, Ordering::Relaxed);
            x.c.flush()?;
            continue;
        };
        let want = on || !snap.is_empty();
        if !want && !overlays.iter().any(|o| o.paint.shown) {
            super::IN_CAPTURES.store(false, Ordering::Relaxed);
            // Nothing shown for a while: give the pictures' memory back.
            if !overlays.is_empty() && last_shown.elapsed() > Duration::from_secs(30) {
                for o in overlays.drain(..) {
                    x.drop_overlay(o);
                }
                listed_at = None;
            }
            x.c.flush()?;
            continue;
        }
        last_shown = now;
        // The monitors, again every few seconds while not drawing (one may have been plugged in).
        if overlays.is_empty() || (!on && listed_at.is_none_or(|t| t.elapsed() > Duration::from_secs(3))) {
            listed_at = Some(now);
            let rects: Vec<(i32, i32, u32, u32)> = crate::capture::monitors().iter().map(|m| (m.x, m.y, m.width, m.height)).filter(|r| r.2 > 0 && r.3 > 0).collect();
            if rects != overlays.iter().map(|o| o.paint.rect).collect::<Vec<_>>() {
                for o in overlays.drain(..) {
                    x.drop_overlay(o);
                }
                for r in rects {
                    overlays.push(x.make(r, visual, colormap)?);
                }
            }
        }
        let app = if on { super::cut_out(ours, shown, front) } else { None };
        // Override-redirect windows stack as they are mapped: up again now and then, over
        // menus and full-screen programs that came later.
        let raise = raised_at.elapsed() > Duration::from_secs(1);
        if raise {
            raised_at = now;
        }
        for o in &mut overlays {
            let (dirty, has) = o.paint.step(&snap, on, None, (0.0, 0.0), app, &mut o.buf);
            let input = super::input_rects(o.paint.size, on, o.paint.hole);
            if input != o.input {
                let rects: Vec<Rectangle> = input.iter().map(|r| Rectangle { x: r.0, y: r.1, width: r.2, height: r.3 }).collect();
                x.c.shape_rectangles(shape::SO::SET, shape::SK::INPUT, ClipOrdering::UNSORTED, o.win, 0, 0, &rects)?;
                o.input = input;
            }
            if let Some(area) = dirty {
                x.put(o, area)?;
            }
            if has != o.paint.shown {
                if has {
                    x.c.map_window(o.win)?;
                    x.c.configure_window(o.win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
                } else {
                    x.c.unmap_window(o.win)?;
                }
                o.paint.shown = has;
            } else if has && raise {
                x.c.configure_window(o.win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
            }
        }
        super::IN_CAPTURES.store(overlays.iter().any(|o| o.paint.shown), Ordering::Relaxed);
        x.c.flush()?;
    }
}
