//! Global hotkeys on Wayland (Linux): through the desktop's GlobalShortcuts portal.
//!
//! On Wayland a program can't grab keys for the whole desktop itself (the X11 way only works
//! while an X11 window has the focus, and not at all in the Flatpak). It asks the desktop
//! instead: Snagster lists its hotkeys with the keys it would like, the desktop shows them to
//! the person (GNOME 48 and later, KDE Plasma 6) and tells Snagster when one is pressed. The
//! person can change the keys in the desktop's own keyboard settings.
//!
//! The key names (`trigger`) are plain code with tests; the D-Bus part needs a desktop and is
//! checked by hand.

use crate::settings::Combo;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

/// How asking the desktop went.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Still asking (the desktop may be showing its dialog).
    Asking,
    /// The desktop took this many hotkeys.
    Bound(usize),
    /// The desktop has no GlobalShortcuts portal (older GNOME, other desktops).
    NoPortal,
    /// Something else went wrong (the person said no, D-Bus failed).
    Failed(String),
}

/// One hotkey to ask for: Snagster's id for it, what it does, the combination it wants.
pub struct Wanted {
    pub id: u32,
    pub description: String,
    pub combo: Combo,
}

/// The running session with the desktop. Dropping it closes the session (the desktop forgets
/// the hotkeys), so new settings can ask again.
pub struct PortalKeys {
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<State>>,
}

impl PortalKeys {
    pub fn state(&self) -> State {
        self.state.lock().map(|s| s.clone()).unwrap_or(State::Asking)
    }
}

impl Drop for PortalKeys {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The key as the shortcuts spec names it (an XKB key name), with its modifiers:
/// "CTRL+SHIFT+a", "Print", "LOGO+F5".
pub fn trigger(c: &Combo) -> Option<String> {
    let key = match c.key.as_str() {
        "Print Screen" => "Print".to_string(),
        "Pause" => "Pause".into(),
        "Scroll Lock" => "Scroll_Lock".into(),
        "Insert" => "Insert".into(),
        "Home" => "Home".into(),
        "End" => "End".into(),
        "Page Up" => "Page_Up".into(),
        "Page Down" => "Page_Down".into(),
        k if k.len() == 1 && k.chars().all(|ch| ch.is_ascii_alphanumeric()) => k.to_ascii_lowercase(),
        k if k.starts_with('F') && k[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)) => k.to_string(),
        _ => return None,
    };
    let mut parts: Vec<&str> = Vec::new();
    if c.ctrl {
        parts.push("CTRL");
    }
    if c.alt {
        parts.push("ALT");
    }
    if c.shift {
        parts.push("SHIFT");
    }
    if c.win {
        parts.push("LOGO");
    }
    let mut out = parts.join("+");
    if !out.is_empty() {
        out.push('+');
    }
    out.push_str(&key);
    Some(out)
}

/// Ask the desktop for these hotkeys. Presses go to `tx` as the ids given, and `wake` is called
/// so Snagster acts on them even in the background.
pub fn start(wanted: Vec<Wanted>, tx: Sender<u32>, wake: Arc<dyn Fn() + Send + Sync>) -> PortalKeys {
    let stop = Arc::new(AtomicBool::new(false));
    let state = Arc::new(Mutex::new(State::Asking));
    let (stop2, state2) = (stop.clone(), state.clone());
    let wake2 = wake.clone();
    let spawned = std::thread::Builder::new().name("snagster-shortcuts".into()).spawn(move || {
        let set = |s: State| {
            if let Ok(mut g) = state2.lock() {
                *g = s;
            }
            wake2();
        };
        let result = pollster::block_on(run(&wanted, &tx, &stop2, &*wake2, &set));
        match result {
            Ok(()) => {}
            Err(e) => set(e),
        }
    });
    if let Err(e) = spawned {
        if let Ok(mut g) = state.lock() {
            *g = State::Failed(e.to_string());
        }
    }
    PortalKeys { stop, state }
}

async fn run(wanted: &[Wanted], tx: &Sender<u32>, stop: &AtomicBool, wake: &(dyn Fn() + Send + Sync), set: &dyn Fn(State)) -> Result<(), State> {
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
    use futures_util::StreamExt;
    let failed = |e: ashpd::Error| State::Failed(e.to_string());
    let portal = GlobalShortcuts::new().await.map_err(|_| State::NoPortal)?;
    let session = portal.create_session(Default::default()).await.map_err(failed)?;
    let shortcuts: Vec<NewShortcut> = wanted
        .iter()
        .map(|w| {
            let t = trigger(&w.combo);
            NewShortcut::new(w.id.to_string(), w.description.clone()).preferred_trigger(t.as_deref())
        })
        .collect();
    let bound = portal.bind_shortcuts(&session, &shortcuts, None, Default::default()).await.map_err(failed)?;
    let bound = bound.response().map_err(failed)?;
    set(State::Bound(bound.shortcuts().len()));
    let mut presses = portal.receive_activated().await.map_err(failed)?;
    // Wait for a press, looking every half second whether the settings asked to stop.
    loop {
        let tick = async_io::Timer::after(std::time::Duration::from_millis(500));
        match futures_util::future::select(presses.next(), Box::pin(tick)).await {
            futures_util::future::Either::Left((Some(p), _)) => {
                if let Ok(id) = p.shortcut_id().parse::<u32>() {
                    let _ = tx.send(id);
                    wake();
                }
            }
            futures_util::future::Either::Left((None, _)) => break,
            futures_util::future::Either::Right(_) => {}
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
    }
    let _ = session.close().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(ctrl: bool, alt: bool, shift: bool, win: bool, key: &str) -> Combo {
        Combo { ctrl, alt, shift, win, key: key.into() }
    }

    #[test]
    fn hotkeys_are_named_the_way_the_desktop_reads_them() {
        assert_eq!(trigger(&combo(false, false, false, false, "Print Screen")).as_deref(), Some("Print"));
        assert_eq!(trigger(&combo(true, false, true, false, "A")).as_deref(), Some("CTRL+SHIFT+a"));
        assert_eq!(trigger(&combo(true, true, true, true, "F5")).as_deref(), Some("CTRL+ALT+SHIFT+LOGO+F5"));
        assert_eq!(trigger(&combo(false, false, true, false, "Page Down")).as_deref(), Some("SHIFT+Page_Down"));
        assert_eq!(trigger(&combo(false, true, false, false, "7")).as_deref(), Some("ALT+7"));
        assert_eq!(trigger(&combo(false, false, false, false, "Scroll Lock")).as_deref(), Some("Scroll_Lock"));
        // Not a key Snagster offers: nothing to ask for (the desktop then picks or leaves it unset).
        assert_eq!(trigger(&combo(true, false, false, false, "Space bar")), None);
        assert_eq!(trigger(&combo(true, false, false, false, "F99")), None);
    }

    #[test]
    fn every_key_snagster_offers_has_a_desktop_name() {
        for (name, _, _) in crate::hotkeys::KEYS {
            assert!(trigger(&combo(false, false, false, false, name)).is_some(), "{name}");
        }
    }
}
