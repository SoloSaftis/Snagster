//! Bringing Snagster's minimized window back on Wayland.
//!
//! A Wayland program can't move, show or un-minimize its own window. While recording, Snagster's
//! main window is minimized and the recording bar is a window of its own; when Stop is pressed
//! on the bar, the main window has to come back where it was. The desktop does that for an
//! `xdg_activation_v1` request whose token was asked for with the click that pressed Stop (the
//! bar has the focus, so the desktop knows the person asked for it): GNOME then un-minimizes the
//! window at its old place and size. A token asked for without a click only gets a
//! "Snagster is ready" notice, which still brings the window back when clicked.
//!
//! winit doesn't offer this for a window that already exists, so this talks to the compositor
//! itself over winit's own connection (a second event queue on the same `wl_display`): it
//! watches the pointer for the surface it is over and the last button serial, and sends the
//! activation when asked.

use std::ffi::c_void;

use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_pointer, wl_registry, wl_seat, wl_surface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::activation::v1::client::{xdg_activation_token_v1, xdg_activation_v1};

/// What the pointer last did on one of Snagster's windows.
#[derive(Default)]
struct State {
    /// The surface the pointer is over (one of Snagster's windows) and the serial of its enter.
    over: Option<(wl_surface::WlSurface, u32)>,
    /// The serial of the last button press on that surface.
    pressed: Option<u32>,
    /// A token that arrived, waiting to be used.
    token: Option<String>,
}

/// The serial a token is asked for with: the last press when there was one, else the enter.
fn token_serial(enter: Option<u32>, pressed: Option<u32>) -> Option<u32> {
    pressed.or(enter)
}

pub struct Activator {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    activation: xdg_activation_v1::XdgActivationV1,
    seat: wl_seat::WlSeat,
    _pointer: Option<wl_pointer::WlPointer>,
    /// The window to bring back once the token arrives.
    target: Option<wl_surface::WlSurface>,
}

impl Activator {
    /// Join winit's connection. `display` is the `wl_display` from the window's raw display
    /// handle. None where the desktop has no activation protocol or no seat.
    ///
    /// # Safety
    /// `display` must be the live `wl_display` the windows were made on, and stay alive as long
    /// as the returned value.
    pub unsafe fn new(display: *mut c_void) -> Option<Self> {
        // SAFETY: the caller hands over winit's live display.
        let backend = unsafe { Backend::from_foreign_display(display.cast()) };
        let conn = Connection::from_backend(backend);
        let (globals, queue) = registry_queue_init::<State>(&conn).ok()?;
        let qh = queue.handle();
        let activation: xdg_activation_v1::XdgActivationV1 = globals.bind(&qh, 1..=1, ()).ok()?;
        let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=5, ()).ok()?;
        let mut me = Self { conn, queue, state: State::default(), activation, seat, _pointer: None, target: None };
        // The seat's capabilities arrive as an event; the pointer is made when they do.
        me.queue.roundtrip(&mut me.state).ok()?;
        me._pointer = Some(me.seat.get_pointer(&qh, ()));
        let _ = me.queue.roundtrip(&mut me.state);
        Some(me)
    }

    /// Handle what has arrived (winit reads the socket; this only sorts this queue's events),
    /// and send the activation once its token is here. True once it was sent.
    pub fn poll(&mut self) -> bool {
        let _ = self.queue.dispatch_pending(&mut self.state);
        if let (Some(token), Some(target)) = (self.state.token.take(), self.target.take()) {
            self.activation.activate(token, &target);
            let _ = self.conn.flush();
            return true;
        }
        false
    }

    /// Bring back the window whose `wl_surface` is `surface` with a token the desktop handed
    /// over (with a hotkey press).
    ///
    /// # Safety
    /// `surface` must be a live `wl_surface` on the same connection.
    pub unsafe fn activate_with_token(&mut self, token: String, surface: *mut c_void) -> bool {
        // SAFETY: as the caller promises.
        let Some(target) = (unsafe { self.surface(surface) }) else { return false };
        self.activation.activate(token, &target);
        self.conn.flush().is_ok()
    }

    /// The surface behind a raw pointer of winit's.
    ///
    /// # Safety
    /// `surface` must be a live `wl_surface` on the same connection.
    unsafe fn surface(&self, surface: *mut c_void) -> Option<wl_surface::WlSurface> {
        // SAFETY: the caller hands over a live surface of this connection.
        let id = unsafe { ObjectId::from_ptr(wl_surface::WlSurface::interface(), surface.cast()) }.ok()?;
        wl_surface::WlSurface::from_id(&self.conn, id).ok()
    }

    /// Ask the desktop to bring back the window whose `wl_surface` is `surface` (from the main
    /// window's raw window handle). The token is asked for with the pointer's last press on
    /// one of Snagster's windows (the Stop button), so call this while the bar still exists.
    ///
    /// # Safety
    /// `surface` must be a live `wl_surface` on the same connection.
    pub unsafe fn activate(&mut self, surface: *mut c_void) -> bool {
        let _ = self.queue.dispatch_pending(&mut self.state);
        // SAFETY: as the caller promises.
        let Some(target) = (unsafe { self.surface(surface) }) else { return false };
        let qh = self.queue.handle();
        let token = self.activation.get_activation_token(&qh, ());
        if let Some(serial) = token_serial(self.state.over.as_ref().map(|o| o.1), self.state.pressed) {
            token.set_serial(serial, &self.seat);
        }
        if let Some((over, _)) = &self.state.over {
            token.set_surface(over);
        }
        token.set_app_id(crate::APP_ID.to_string());
        token.commit();
        self.target = Some(target);
        self.conn.flush().is_ok()
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<xdg_activation_v1::XdgActivationV1, ()> for State {
    fn event(_: &mut Self, _: &xdg_activation_v1::XdgActivationV1, _: xdg_activation_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, ()> for State {
    fn event(state: &mut Self, proxy: &xdg_activation_token_v1::XdgActivationTokenV1, event: xdg_activation_token_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            state.token = Some(token);
            proxy.destroy();
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(state: &mut Self, _: &wl_pointer::WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_pointer::Event::Enter { serial, surface, .. } => {
                state.over = Some((surface, serial));
                state.pressed = None;
            }
            wl_pointer::Event::Button { serial, state: WEnum::Value(wl_pointer::ButtonState::Pressed), .. } => state.pressed = Some(serial),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_token_carries_the_press_that_stopped_the_recording() {
        use super::token_serial;
        assert_eq!(token_serial(Some(4), Some(9)), Some(9), "the click on Stop");
        assert_eq!(token_serial(Some(4), None), Some(4), "no press seen: the pointer's arrival on the bar");
        assert_eq!(token_serial(None, None), None, "nothing: the desktop decides (a notice)");
    }
}
