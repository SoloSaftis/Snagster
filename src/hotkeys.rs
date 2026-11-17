//! Global hotkeys: turn the person's key combinations into registered system hotkeys.

use crate::settings::{Action, Combo, Settings};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use std::sync::mpsc::Sender;

/// The keys a hotkey can use, with the Windows virtual-key code (for the self-test).
pub const KEYS: &[(&str, Code, u16)] = &[
    ("Print Screen", Code::PrintScreen, 0x2C),
    ("Pause", Code::Pause, 0x13),
    ("Scroll Lock", Code::ScrollLock, 0x91),
    ("Insert", Code::Insert, 0x2D),
    ("Home", Code::Home, 0x24),
    ("End", Code::End, 0x23),
    ("Page Up", Code::PageUp, 0x21),
    ("Page Down", Code::PageDown, 0x22),
    ("F1", Code::F1, 0x70),
    ("F2", Code::F2, 0x71),
    ("F3", Code::F3, 0x72),
    ("F4", Code::F4, 0x73),
    ("F5", Code::F5, 0x74),
    ("F6", Code::F6, 0x75),
    ("F7", Code::F7, 0x76),
    ("F8", Code::F8, 0x77),
    ("F9", Code::F9, 0x78),
    ("F10", Code::F10, 0x79),
    ("F11", Code::F11, 0x7A),
    ("F12", Code::F12, 0x7B),
    ("A", Code::KeyA, 0x41),
    ("B", Code::KeyB, 0x42),
    ("C", Code::KeyC, 0x43),
    ("D", Code::KeyD, 0x44),
    ("E", Code::KeyE, 0x45),
    ("F", Code::KeyF, 0x46),
    ("G", Code::KeyG, 0x47),
    ("H", Code::KeyH, 0x48),
    ("I", Code::KeyI, 0x49),
    ("J", Code::KeyJ, 0x4A),
    ("K", Code::KeyK, 0x4B),
    ("L", Code::KeyL, 0x4C),
    ("M", Code::KeyM, 0x4D),
    ("N", Code::KeyN, 0x4E),
    ("O", Code::KeyO, 0x4F),
    ("P", Code::KeyP, 0x50),
    ("Q", Code::KeyQ, 0x51),
    ("R", Code::KeyR, 0x52),
    ("S", Code::KeyS, 0x53),
    ("T", Code::KeyT, 0x54),
    ("U", Code::KeyU, 0x55),
    ("V", Code::KeyV, 0x56),
    ("W", Code::KeyW, 0x57),
    ("X", Code::KeyX, 0x58),
    ("Y", Code::KeyY, 0x59),
    ("Z", Code::KeyZ, 0x5A),
    ("0", Code::Digit0, 0x30),
    ("1", Code::Digit1, 0x31),
    ("2", Code::Digit2, 0x32),
    ("3", Code::Digit3, 0x33),
    ("4", Code::Digit4, 0x34),
    ("5", Code::Digit5, 0x35),
    ("6", Code::Digit6, 0x36),
    ("7", Code::Digit7, 0x37),
    ("8", Code::Digit8, 0x38),
    ("9", Code::Digit9, 0x39),
];

pub fn to_hotkey(c: &Combo) -> Option<HotKey> {
    let code = KEYS.iter().find(|k| k.0 == c.key)?.1;
    let mut m = Modifiers::empty();
    if c.ctrl {
        m |= Modifiers::CONTROL;
    }
    if c.alt {
        m |= Modifiers::ALT;
    }
    if c.shift {
        m |= Modifiers::SHIFT;
    }
    if c.win {
        m |= Modifiers::META;
    }
    Some(HotKey::new(if m.is_empty() { None } else { Some(m) }, code))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Action(Action),
    Preset(usize),
}

/// One combination and how registering it went.
pub struct Registered {
    pub target: Target,
    pub combo: Combo,
    pub id: u32,
    pub ok: bool,
    /// Another program had it, so Snagster catches it with its keyboard hook instead.
    pub hooked: bool,
}

pub struct Hotkeys {
    _manager: Option<GlobalHotKeyManager>,
    pub list: Vec<Registered>,
    summary: String,
    /// Linux on Wayland: the hotkeys are the desktop's (GlobalShortcuts portal).
    #[cfg(target_os = "linux")]
    portal: Option<crate::keys_portal::PortalKeys>,
}

impl Hotkeys {
    pub fn none(summary: &str) -> Self {
        Self::with(None, Vec::new(), summary.into())
    }

    fn with(manager: Option<GlobalHotKeyManager>, list: Vec<Registered>, summary: String) -> Self {
        Self {
            _manager: manager,
            list,
            summary,
            #[cfg(target_os = "linux")]
            portal: None,
        }
    }

    /// One line on how the hotkeys are doing, for Settings > Hotkeys.
    pub fn summary(&self) -> String {
        #[cfg(target_os = "linux")]
        if let Some(p) = &self.portal {
            return portal_summary(&p.state(), std::env::var_os("FLATPAK_ID").is_some());
        }
        self.summary.clone()
    }

    pub fn target(&self, id: u32) -> Option<Target> {
        self.list.iter().find(|r| (r.ok || r.hooked) && r.id == id).map(|r| r.target)
    }

    pub fn status_of(&self, t: Target) -> Option<bool> {
        self.list.iter().find(|r| r.target == t).map(|r| r.ok || r.hooked)
    }

}

/// Every combination the settings ask for, presets included.
pub fn wanted(s: &Settings) -> Vec<(Target, Combo)> {
    let mut out: Vec<(Target, Combo)> = Vec::new();
    for b in &s.bindings {
        if b.combo.is_set() {
            out.push((Target::Action(b.action), b.combo.clone()));
        }
    }
    for (i, p) in s.presets.iter().enumerate() {
        if p.combo.is_set() {
            out.push((Target::Preset(i), p.combo.clone()));
        }
    }
    out
}

/// Combinations used twice (only the first gets the key).
pub fn duplicates(s: &Settings) -> Vec<String> {
    let w = wanted(s);
    let mut dups = Vec::new();
    for (i, (_, c)) in w.iter().enumerate() {
        if w[..i].iter().any(|(_, d)| d == c) && !dups.contains(&c.label()) {
            dups.push(c.label());
        }
    }
    dups
}

/// Register everything in the settings. Events go to `tx`, and `wake` is called so the
/// app runs even while it is in the background.
pub fn register(s: &Settings, tx: Sender<u32>, wake: impl Fn() + Send + Sync + 'static) -> Hotkeys {
    let wake_any: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(wake);
    let wake = move || wake_any();
    if !s.hotkeys_enabled {
        set_hook_keys(Vec::new(), tx, std::sync::Arc::new(|| {}));
        return Hotkeys::none("Hotkeys are switched off.");
    }
    #[cfg(target_os = "linux")]
    if crate::capture::desktop_screenshots_only() {
        return register_with_desktop(s, tx, std::sync::Arc::new(wake));
    }
    let manager = match GlobalHotKeyManager::new() {
        Ok(m) => m,
        Err(e) => return Hotkeys::none(&format!("Hotkeys are unavailable: {e}")),
    };
    let mut list = Vec::new();
    let mut failed = 0;
    for (target, combo) in wanted(s) {
        let Some(hk) = to_hotkey(&combo) else { continue };
        let dup = list.iter().any(|r: &Registered| r.ok && r.id == hk.id());
        let ok = !dup && manager.register(hk).is_ok();
        if !ok {
            failed += 1;
        }
        list.push(Registered { target, combo, id: hk.id(), ok, hooked: false });
    }
    let wake: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(wake);
    // Keys another program registered first: catch them with a keyboard hook if allowed.
    let mut hooked = Vec::new();
    if s.override_hotkeys {
        for r in list.iter_mut().filter(|r| !r.ok) {
            if !hooked.iter().any(|(c, _): &(Combo, u32)| *c == r.combo) {
                r.hooked = true;
                hooked.push((r.combo.clone(), r.id));
            }
        }
    }
    set_hook_keys(hooked, tx.clone(), wake.clone());
    let wake2 = wake.clone();
    GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
        if e.state() == HotKeyState::Pressed {
            let _ = tx.send(e.id());
            wake2();
        }
    }));
    let active = list.iter().filter(|r| r.ok).count();
    let taken_over = list.iter().filter(|r| r.hooked).count();
    failed -= taken_over;
    let summary = if taken_over > 0 && failed == 0 {
        format!("{} hotkeys are active ({taken_over} taken over from another program).", active + taken_over)
    } else if failed == 0 {
        format!("{active} hotkeys are active.")
    } else if cfg!(windows) {
        format!(
            "{active} hotkeys are active; {failed} couldn't be used because another program (or Windows' own Print Screen setting) already has them."
        )
    } else {
        format!("{active} hotkeys are active; {failed} couldn't be used because another program already has them.")
    };
    Hotkeys::with(Some(manager), list, summary)
}

/// Linux on Wayland: ask the desktop for the hotkeys (GlobalShortcuts portal). Each gets an id
/// of its own; the desktop says which one was pressed.
#[cfg(target_os = "linux")]
fn register_with_desktop(s: &Settings, tx: Sender<u32>, wake: std::sync::Arc<dyn Fn() + Send + Sync>) -> Hotkeys {
    let mut list = Vec::new();
    let mut asks = Vec::new();
    for (n, (target, combo)) in wanted(s).into_iter().enumerate() {
        let id = n as u32 + 1;
        let description = match target {
            Target::Action(a) => crate::i18n::tr(a.label()),
            Target::Preset(i) => s.presets.get(i).map(|p| p.name.clone()).unwrap_or_default(),
        };
        asks.push(crate::keys_portal::Wanted { id, description, combo: combo.clone() });
        list.push(Registered { target, combo, id, ok: true, hooked: false });
    }
    let mut hk = Hotkeys::with(None, list, String::new());
    hk.portal = Some(crate::keys_portal::start(asks, tx, wake));
    hk
}

/// What Settings says about hotkeys the desktop holds. In the Flatpak the command to put in a
/// desktop shortcut starts with `flatpak run`.
#[cfg(target_os = "linux")]
fn portal_summary(state: &crate::keys_portal::State, flatpak: bool) -> String {
    use crate::keys_portal::State;
    match state {
        State::Asking => crate::i18n::t("Asking the desktop for the hotkeys…").to_string(),
        State::Bound(n) => crate::i18n::tf("{count} hotkeys are set through the desktop. Change their keys in the desktop's keyboard settings.", &[("count", &n.to_string())]),
        State::NoPortal => {
            let cmd = if flatpak { "flatpak run io.github.SoloSaftis.Snagster --action Area" } else { "snagster --action Area" };
            crate::i18n::tf(
                "This desktop doesn't let apps set hotkeys. Add a shortcut in the desktop's keyboard settings that runs: {command} (or AllInOne, Window, Screen, RecordGif…).",
                &[("command", cmd)],
            )
        }
        State::Failed(e) => crate::i18n::tf("The desktop didn't set the hotkeys: {error}", &[("error", e)]),
    }
}

/// Press each registered hotkey for real and report which ones Snagster received.
#[cfg(windows)]
pub fn self_test(s: &Settings) {
    use std::sync::{Arc, Mutex};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage};

    let got: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = std::sync::mpsc::channel::<u32>();
    let hk = register(s, tx, || {});
    println!("{}", hk.summary());
    let pump = |ms: u64| {
        let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        while std::time::Instant::now() < end {
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    let key = |vk: u16, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { 0 }, time: 0, dwExtraInfo: 0 },
        },
    };
    let mut passed = 0;
    let mut tested = 0;
    for r in &hk.list {
        let name = match r.target {
            Target::Action(a) => a.label().to_string(),
            Target::Preset(i) => format!("Preset: {}", s.presets[i].name),
        };
        if !r.ok && !r.hooked {
            println!("  NOT REGISTERED  {:<28} {}", r.combo.label(), name);
            continue;
        }
        let Some(vk) = KEYS.iter().find(|k| k.0 == r.combo.key).map(|k| k.2) else { continue };
        let mut mods = Vec::new();
        if r.combo.ctrl {
            mods.push(0x11u16);
        }
        if r.combo.alt {
            mods.push(0x12);
        }
        if r.combo.shift {
            mods.push(0x10);
        }
        if r.combo.win {
            mods.push(0x5B);
        }
        let mut seq: Vec<INPUT> = mods.iter().map(|m| key(*m, false)).collect();
        seq.push(key(vk, false));
        seq.push(key(vk, true));
        seq.extend(mods.iter().rev().map(|m| key(*m, true)));
        unsafe {
            SendInput(seq.len() as u32, seq.as_ptr(), std::mem::size_of::<INPUT>() as i32);
        }
        pump(250);
        while let Ok(id) = rx.try_recv() {
            got.lock().unwrap().push(id);
        }
        tested += 1;
        let ok = got.lock().unwrap().contains(&r.id);
        if ok {
            passed += 1;
        }
        println!("  {}  {:<28} {}{}", if ok { "WORKS     " } else { "NO EVENT  " }, r.combo.label(), name, if r.hooked { "  (taken over from another program)" } else { "" });
        got.lock().unwrap().clear();
    }
    println!("{passed} of {tested} registered hotkeys fired.");
}

#[cfg(not(windows))]
pub fn self_test(_s: &Settings) {
    println!("The hotkey self-test runs on Windows only.");
}

// ---------- taking over keys another program registered ----------

type Wake = std::sync::Arc<dyn Fn() + Send + Sync>;

struct HookState {
    keys: Vec<(u16, bool, bool, bool, bool, u32)>,
    tx: Option<Sender<u32>>,
    wake: Option<Wake>,
    // The key-up of a caught combination, swallowed too (only the Windows hook catches keys).
    #[cfg(windows)]
    swallow_up: Option<u32>,
}

fn hook_state() -> &'static std::sync::Mutex<HookState> {
    static S: std::sync::OnceLock<std::sync::Mutex<HookState>> = std::sync::OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(HookState { keys: Vec::new(), tx: None, wake: None, #[cfg(windows)] swallow_up: None }))
}

/// Tell the keyboard hook which combinations to catch (starting it the first time).
fn set_hook_keys(keys: Vec<(Combo, u32)>, tx: Sender<u32>, wake: Wake) {
    let list: Vec<_> = keys
        .iter()
        .filter_map(|(c, id)| KEYS.iter().find(|k| k.0 == c.key).map(|k| (k.2, c.ctrl, c.alt, c.shift, c.win, *id)))
        .collect();
    let need = !list.is_empty();
    if let Ok(mut s) = hook_state().lock() {
        s.keys = list;
        s.tx = Some(tx);
        s.wake = Some(wake);
    }
    #[cfg(windows)]
    if need {
        start_hook();
    }
    let _ = need;
}

#[cfg(windows)]
fn start_hook() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, MSG, SetWindowsHookExW, TranslateMessage, WH_KEYBOARD_LL};
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), std::ptr::null_mut(), 0);
        if hook.is_null() {
            return;
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

#[cfg(windows)]
unsafe extern "system" fn hook_proc(code: i32, wparam: usize, lparam: isize) -> isize {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    use windows_sys::Win32::UI::WindowsAndMessaging::{CallNextHookEx, KBDLLHOOKSTRUCT, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP};
    if code >= 0 {
        let kb = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let vk = kb.vkCode;
        let msg = wparam as u32;
        if let Ok(mut s) = hook_state().lock() {
            if (msg == WM_KEYUP || msg == WM_SYSKEYUP) && s.swallow_up == Some(vk) {
                s.swallow_up = None;
                return 1;
            }
            if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
                let down = |k: i32| unsafe { GetAsyncKeyState(k) } as u16 & 0x8000 != 0;
                let (c, a, sh, w) = (down(0x11), down(0x12), down(0x10), down(0x5B) || down(0x5C));
                if let Some(&(_, _, _, _, _, id)) =
                    s.keys.iter().find(|k| k.0 as u32 == vk && k.1 == c && k.2 == a && k.3 == sh && k.4 == w)
                {
                    if let Some(tx) = &s.tx {
                        let _ = tx.send(id);
                    }
                    if let Some(wk) = &s.wake {
                        wk();
                    }
                    s.swallow_up = Some(vk);
                    return 1;
                }
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Binding, Preset, default_bindings};
    use std::collections::HashSet;

    fn settings_with(bindings: Vec<Binding>, presets: Vec<Preset>) -> Settings {
        Settings { bindings, presets, ..Default::default() }
    }

    #[test]
    fn key_names_and_codes_are_unique() {
        let names: HashSet<&str> = KEYS.iter().map(|k| k.0).collect();
        let codes: HashSet<Code> = KEYS.iter().map(|k| k.1).collect();
        let vks: HashSet<u16> = KEYS.iter().map(|k| k.2).collect();
        assert_eq!(names.len(), KEYS.len());
        assert_eq!(codes.len(), KEYS.len());
        assert_eq!(vks.len(), KEYS.len());
        // Letters and digits use their ASCII value as the Windows virtual-key code.
        for (name, _, vk) in KEYS {
            if name.len() == 1 {
                assert_eq!(*vk, name.as_bytes()[0] as u16, "{name}");
            }
        }
    }

    #[test]
    fn every_default_binding_uses_a_known_key() {
        for b in default_bindings() {
            if b.combo.is_set() {
                assert!(to_hotkey(&b.combo).is_some(), "{} isn't a known key", b.combo.key);
            }
        }
    }

    #[test]
    fn combos_become_hotkeys() {
        let hk = to_hotkey(&Combo::new(true, false, true, "F9")).unwrap();
        assert_eq!(hk, HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::F9));
        let hk = to_hotkey(&Combo::new(false, false, false, "Print Screen")).unwrap();
        assert_eq!(hk, HotKey::new(None, Code::PrintScreen));
        let all = Combo { ctrl: true, alt: true, shift: true, win: true, key: "A".into() };
        assert_eq!(
            to_hotkey(&all).unwrap(),
            HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT | Modifiers::META), Code::KeyA)
        );
    }

    #[test]
    fn unknown_or_empty_keys_are_not_hotkeys() {
        assert!(to_hotkey(&Combo::default()).is_none());
        assert!(to_hotkey(&Combo::new(true, false, false, "NoSuchKey")).is_none());
        // Names are matched exactly.
        assert!(to_hotkey(&Combo::new(true, false, false, "f9")).is_none());
        assert!(to_hotkey(&Combo::new(true, false, false, "PrintScreen")).is_none());
    }

    #[test]
    fn different_combos_get_different_ids() {
        let mut ids = HashSet::new();
        for (name, _, _) in KEYS {
            for (c, a, s) in [(false, false, false), (true, false, false), (false, true, false), (true, true, true)] {
                let hk = to_hotkey(&Combo::new(c, a, s, name)).unwrap();
                assert!(ids.insert(hk.id()), "{name} {c} {a} {s}");
            }
        }
        // The same combo always gets the same id.
        let a = to_hotkey(&Combo::new(true, false, false, "F1")).unwrap().id();
        let b = to_hotkey(&Combo::new(true, false, false, "F1")).unwrap().id();
        assert_eq!(a, b);
    }

    #[test]
    fn wanted_lists_set_bindings_then_presets() {
        let s = settings_with(
            vec![
                Binding { action: Action::Area, combo: Combo::new(true, false, false, "A") },
                Binding { action: Action::Window, combo: Combo::default() },
                Binding { action: Action::Screen, combo: Combo::new(false, true, false, "S") },
            ],
            vec![
                Preset { combo: Combo::default(), ..Default::default() },
                Preset { combo: Combo::new(false, false, true, "P"), ..Default::default() },
            ],
        );
        let w = wanted(&s);
        assert_eq!(
            w,
            vec![
                (Target::Action(Action::Area), Combo::new(true, false, false, "A")),
                (Target::Action(Action::Screen), Combo::new(false, true, false, "S")),
                (Target::Preset(1), Combo::new(false, false, true, "P")),
            ]
        );
        assert!(wanted(&settings_with(Vec::new(), Vec::new())).is_empty());
    }

    #[test]
    fn default_settings_have_no_duplicates() {
        let s = Settings::default();
        assert!(duplicates(&s).is_empty());
        assert_eq!(wanted(&s).len(), default_bindings().iter().filter(|b| b.combo.is_set()).count());
    }

    #[test]
    fn duplicates_are_reported_once() {
        let f1 = Combo::new(true, false, false, "F1");
        let s = settings_with(
            vec![
                Binding { action: Action::Area, combo: f1.clone() },
                Binding { action: Action::Window, combo: f1.clone() },
                Binding { action: Action::Screen, combo: Combo::new(false, false, false, "F1") },
            ],
            vec![Preset { combo: f1.clone(), ..Default::default() }],
        );
        assert_eq!(duplicates(&s), vec!["Ctrl + F1".to_string()]);
    }

    fn registered(target: Target, id: u32, ok: bool, hooked: bool) -> Registered {
        Registered { target, combo: Combo::default(), id, ok, hooked }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn settings_say_how_the_desktop_took_the_hotkeys() {
        use crate::keys_portal::State;
        assert!(portal_summary(&State::Bound(3), false).starts_with("3 "));
        // No portal: the command for a desktop shortcut, the Flatpak way inside the Flatpak.
        assert!(portal_summary(&State::NoPortal, false).contains("snagster --action Area"));
        assert!(portal_summary(&State::NoPortal, true).contains("flatpak run io.github.SoloSaftis.Snagster --action Area"));
        assert!(portal_summary(&State::Failed("refused".into()), false).contains("refused"));
    }

    #[test]
    fn targets_are_found_only_for_working_keys() {
        let hk = Hotkeys::with(
            None,
            vec![
                registered(Target::Action(Action::Area), 1, true, false),
                registered(Target::Action(Action::Window), 2, false, true),
                registered(Target::Preset(0), 3, false, false),
            ],
            String::new(),
        );
        assert_eq!(hk.target(1), Some(Target::Action(Action::Area)));
        assert_eq!(hk.target(2), Some(Target::Action(Action::Window)), "taken over keys work too");
        assert_eq!(hk.target(3), None, "a key that failed doesn't fire");
        assert_eq!(hk.target(99), None);
        assert_eq!(hk.status_of(Target::Action(Action::Area)), Some(true));
        assert_eq!(hk.status_of(Target::Action(Action::Window)), Some(true));
        assert_eq!(hk.status_of(Target::Preset(0)), Some(false));
        assert_eq!(hk.status_of(Target::Preset(1)), None);
    }

    #[test]
    fn none_has_only_a_summary() {
        let hk = Hotkeys::none("Hotkeys are switched off.");
        assert!(hk.list.is_empty());
        assert_eq!(hk.summary(), "Hotkeys are switched off.");
        assert_eq!(hk.target(0), None);
    }
}
