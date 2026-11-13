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
        m |= Modifiers::SUPER;
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
    pub summary: String,
}

impl Hotkeys {
    pub fn none(summary: &str) -> Self {
        Self { _manager: None, list: Vec::new(), summary: summary.into() }
    }

    pub fn target(&self, id: u32) -> Option<Target> {
        self.list.iter().find(|r| (r.ok || r.hooked) && r.id == id).map(|r| r.target)
    }

    pub fn status_of(&self, t: Target) -> Option<bool> {
        self.list.iter().find(|r| r.target == t).map(|r| r.ok || r.hooked)
    }

    pub fn overridden(&self, t: Target) -> bool {
        self.list.iter().any(|r| r.target == t && r.hooked)
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
    } else {
        format!(
            "{active} hotkeys are active; {failed} couldn't be used because another program (or Windows' own Print Screen setting) already has them."
        )
    };
    Hotkeys { _manager: Some(manager), list, summary }
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
    println!("{}", hk.summary);
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
    swallow_up: Option<u32>,
}

fn hook_state() -> &'static std::sync::Mutex<HookState> {
    static S: std::sync::OnceLock<std::sync::Mutex<HookState>> = std::sync::OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(HookState { keys: Vec::new(), tx: None, wake: None, swallow_up: None }))
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
