//! Which system Snagster runs on, and the small differences between their design rules:
//! how shortcut labels are written, the order of a dialog's buttons, and the system's own
//! words (Recycle Bin or Trash, Exit or Quit). The rules and their sources are in the private
//! notes (docs/DESIGN-GUIDELINES.md); the rest of the code asks here instead of using `cfg!`.

use eframe::egui;

/// The system, and on Linux the desktop, whose design rules apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum System {
    Windows,
    Mac,
    Gnome,
    Kde,
    /// Another Linux desktop (XFCE, MATE, LXQt...): follows the GNOME/GTK side.
    OtherLinux,
}

/// The system Snagster is running on (read once).
pub fn system() -> System {
    static SYSTEM: std::sync::OnceLock<System> = std::sync::OnceLock::new();
    *SYSTEM.get_or_init(|| {
        if cfg!(windows) {
            System::Windows
        } else if cfg!(target_os = "macos") {
            System::Mac
        } else {
            linux_desktop(&std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default())
        }
    })
}

/// The Linux desktop from `XDG_CURRENT_DESKTOP`, a colon list such as `ubuntu:GNOME`.
fn linux_desktop(current: &str) -> System {
    let names: Vec<String> = current.split(':').map(|s| s.trim().to_ascii_lowercase()).collect();
    let has = |n: &str| names.iter().any(|s| s == n);
    if has("kde") {
        System::Kde
    } else if ["gnome", "unity", "budgie", "pantheon", "x-cinnamon", "cinnamon"].iter().any(|n| has(n)) {
        System::Gnome
    } else {
        System::OtherLinux
    }
}

/// A shortcut label written the Windows way ("Ctrl+Shift+S", or "Ctrl + S" from a hotkey) in
/// the form people on this system know. Text around the shortcut is kept, so a tooltip such
/// as "Save at once (Ctrl+Shift+S)" works too.
pub fn keys(text: &str) -> String {
    keys_for(system(), text)
}

fn keys_for(sys: System, text: &str) -> String {
    static SHORTCUT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = SHORTCUT.get_or_init(|| regex::Regex::new(r"\b(?:(?:Ctrl|Alt|Shift|Win) ?\+ ?)+(?:[^\s,;)/]+|[,;/.])").expect("shortcut pattern"));
    re.replace_all(text, |c: &regex::Captures| one_shortcut(sys, &c[0])).into_owned()
}

/// One shortcut such as "Ctrl+Shift+S" for `sys`. In-app shortcuts use Ctrl, which egui
/// turns into Command on a Mac.
fn one_shortcut(sys: System, s: &str) -> String {
    let parts: Vec<&str> = s.split('+').map(str::trim).collect();
    let (mods, key) = parts.split_at(parts.len() - 1);
    let has = |m: &str| mods.contains(&m);
    match sys {
        System::Mac => {
            // Apple's order: Control, Option, Shift, Command.
            let mut out = String::new();
            if has("Alt") {
                out.push('⌥');
            }
            if has("Shift") {
                out.push('⇧');
            }
            if has("Ctrl") || has("Win") {
                out.push('⌘');
            }
            out.push_str(key[0]);
            out
        }
        _ => {
            let win = if sys == System::Windows { "Win" } else { "Super" };
            let mut out: Vec<&str> = Vec::new();
            for m in ["Ctrl", "Alt", "Shift", "Win"] {
                if has(m) {
                    out.push(if m == "Win" { win } else { m });
                }
            }
            out.push(key[0]);
            out.join("+")
        }
    }
}

/// A global hotkey's label: on a Mac, Ctrl is the Control key and Win the Command key.
pub fn hotkey_label(ctrl: bool, alt: bool, shift: bool, win: bool, key: &str) -> String {
    hotkey_label_for(system(), ctrl, alt, shift, win, key)
}

fn hotkey_label_for(sys: System, ctrl: bool, alt: bool, shift: bool, win: bool, key: &str) -> String {
    if sys == System::Mac {
        let mut out = String::new();
        for (on, glyph) in [(ctrl, '⌃'), (alt, '⌥'), (shift, '⇧'), (win, '⌘')] {
            if on {
                out.push(glyph);
            }
        }
        return out + key;
    }
    let win_name = if sys == System::Windows { "Win" } else { "Super" };
    let mut parts = Vec::new();
    for (on, name) in [(ctrl, "Ctrl"), (alt, "Alt"), (shift, "Shift"), (win, win_name)] {
        if on {
            parts.push(name);
        }
    }
    parts.push(key);
    parts.join(" + ")
}

/// The in-app commands whose keys differ on a Mac, where Apple keeps ⇧⌘A (Deselect All),
/// ⇧⌘W (Close File) and ⇧⌘P (Page Setup) for other things.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppKey {
    AllInOne,
    Window,
    Screen,
    Pin,
}

#[cfg(test)]
impl AppKey {
    pub const ALL: [AppKey; 4] = [AppKey::AllInOne, AppKey::Window, AppKey::Screen, AppKey::Pin];
}

/// The key of an in-app command as (Alt/Option, Shift, key), with Ctrl (Command on a Mac)
/// always held: Ctrl+Shift+A/W/F and Ctrl+Shift+P elsewhere; ⌥⇧⌘A, ⌥⇧⌘W, ⇧⌘F and ⌥⌘P on a Mac.
pub const fn app_key_for(mac: bool, k: AppKey) -> (bool, bool, char) {
    match (mac, k) {
        (true, AppKey::AllInOne) => (true, true, 'a'),
        (true, AppKey::Window) => (true, true, 'w'),
        (true, AppKey::Pin) => (true, false, 'p'),
        (false, AppKey::AllInOne) => (false, true, 'a'),
        (false, AppKey::Window) => (false, true, 'w'),
        (false, AppKey::Pin) => (false, true, 'p'),
        (_, AppKey::Screen) => (false, true, 'f'),
    }
}

/// The key of an in-app command on this system, as egui's modifiers and key.
pub fn app_key(k: AppKey) -> (egui::Modifiers, egui::Key) {
    let (alt, shift, key) = app_key_for(system() == System::Mac, k);
    let mut m = egui::Modifiers::COMMAND;
    m.alt = alt;
    m.shift = shift;
    (m, egui::Key::from_name(&key.to_ascii_uppercase().to_string()).expect("a letter key"))
}

/// The key of an in-app command written the Windows way ("Ctrl+Shift+A", "Ctrl+Alt+P"), for
/// `keys` to show in this system's form.
pub fn app_key_text(k: AppKey) -> String {
    app_key_text_for(system() == System::Mac, k)
}

fn app_key_text_for(mac: bool, k: AppKey) -> String {
    let (alt, shift, key) = app_key_for(mac, k);
    format!("Ctrl+{}{}{}", if alt { "Alt+" } else { "" }, if shift { "Shift+" } else { "" }, key.to_ascii_uppercase())
}

/// The key that moves the picked capture in the Library to the Recycle Bin or Trash: Delete on
/// Windows and Linux, ⌘⌫ on a Mac (Finder's), as (Command held, key).
pub fn trash_key_for(sys: System) -> (bool, egui::Key) {
    if sys == System::Mac { (true, egui::Key::Backspace) } else { (false, egui::Key::Delete) }
}

/// Whether `trash_key_for(system())` was pressed in `i` (Command exactly as the key wants it).
pub fn trash_key_pressed(i: &egui::InputState) -> bool {
    let (cmd, key) = trash_key_for(system());
    i.key_pressed(key) && i.modifiers.command == cmd && !i.modifiers.shift && !i.modifiers.alt
}

/// The names of the four modifier boxes in Settings > Hotkeys (Ctrl, Alt, Shift, and the
/// system key): Win on Windows, Super on Linux, the key symbols on a Mac (where Ctrl is
/// Control, Alt is Option and the system key is Command).
pub fn modifier_labels() -> [&'static str; 4] {
    modifier_labels_for(system())
}

fn modifier_labels_for(sys: System) -> [&'static str; 4] {
    match sys {
        System::Windows => ["Ctrl", "Alt", "Shift", "Win"],
        System::Mac => ["⌃", "⌥", "⇧", "⌘"],
        _ => ["Ctrl", "Alt", "Shift", "Super"],
    }
}

/// Whether a dialog's main (action) button comes first, on the left of Cancel. Windows and
/// KDE put it first; GNOME and macOS put Cancel first and the action at the far right.
fn action_first_for(sys: System) -> bool {
    matches!(sys, System::Windows | System::Kde)
}

/// What the person chose in a dialog's button row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Action,
    Cancel,
}

/// How a dialog's main button behaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An ordinary action: accent colour, Return runs it.
    Normal,
    /// Something that can't be taken back or reaches other people (delete, go live): red,
    /// and Return doesn't run it, so it is never pressed without being read.
    Destructive,
}

/// The order of a dialog's buttons from left to right, for `sys`.
fn button_order(sys: System) -> [Choice; 2] {
    if action_first_for(sys) { [Choice::Action, Choice::Cancel] } else { [Choice::Cancel, Choice::Action] }
}

/// A dialog's button row in the system's order, right-aligned: the main button `action`
/// (translated; none for a dialog that only informs) and the safe one, `cancel` (translated:
/// "Cancel", or "Close" when there is nothing to cancel). Esc picks `cancel`; Return picks the
/// action unless it is destructive or a text field has the keyboard.
pub fn dialog_buttons(ui: &mut egui::Ui, action: Option<&str>, cancel: &str, kind: Kind, enabled: bool) -> Option<Choice> {
    let mut choice = None;
    let destructive = kind == Kind::Destructive;
    ui.add_space(4.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        // Right to left: the first button added is the one at the far right.
        let mut order = button_order(system());
        order.reverse();
        for c in order {
            match c {
                Choice::Cancel => {
                    if ui.button(cancel).clicked() {
                        choice = Some(Choice::Cancel);
                    }
                }
                Choice::Action => {
                    let Some(action) = action else { continue };
                    let fill = if destructive { crate::theme::pal().danger } else { crate::theme::pal().accent };
                    let b = egui::Button::new(egui::RichText::new(action).color(crate::theme::text_on(fill)).strong())
                        .fill(fill)
                        .min_size(egui::vec2(80.0, 0.0));
                    if ui.add_enabled(enabled, b).clicked() {
                        choice = Some(Choice::Action);
                    }
                }
            }
        }
    });
    let (esc, enter) = ui.input(|i| (i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::Enter)));
    if choice.is_none() && esc {
        choice = Some(Choice::Cancel);
    }
    if choice.is_none() && enter && enabled && action.is_some() && !destructive && !ui.ctx().memory(|m| m.focused().is_some()) {
        choice = Some(Choice::Action);
    }
    choice
}

/// Whether the system has its own Recycle Bin (Windows) rather than a Trash.
pub fn recycle_bin() -> bool {
    system() == System::Windows
}

/// The English text naming the Recycle Bin on Windows and the Trash elsewhere (translate the
/// result with `t`; each is its own sentence in the packs).
pub fn trash_text(recycle_bin_text: &'static str, trash: &'static str) -> &'static str {
    if recycle_bin() { recycle_bin_text } else { trash }
}

/// The English text for this system out of three: Windows (the taskbar corner), macOS (the
/// menu bar) and Linux (the tray). Translate the result with `t`.
pub fn by_system(windows: &'static str, mac: &'static str, linux: &'static str) -> &'static str {
    match system() {
        System::Windows => windows,
        System::Mac => mac,
        _ => linux,
    }
}

/// A Linux desktop (GNOME, KDE or another).
pub fn linux() -> bool {
    matches!(system(), System::Gnome | System::Kde | System::OtherLinux)
}

/// Whether the desktop shows tray (status) icons. Stock GNOME doesn't (only with an
/// extension, which Ubuntu ships), so there closing the window must really close it.
pub fn has_tray() -> bool {
    system() != System::Gnome || std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_ascii_lowercase().split(':').any(|d| d == "ubuntu")
}

/// Whether closing the window should keep Snagster running in the tray when nobody has
/// chosen yet: on Windows (where capture tools live in the notification area), not on Linux
/// or macOS, whose guidelines want it to be the person's choice.
pub fn close_to_tray_by_default() -> bool {
    system() == System::Windows
}

/// The menu command that ends Snagster: "Exit" (Alt+F4) on Windows, "Quit" (Ctrl+Q / ⌘Q)
/// elsewhere.
pub fn quit_item() -> (&'static str, &'static str) {
    match system() {
        System::Windows => ("Exit", "Alt+F4"),
        System::Mac => ("Quit Snagster", "Ctrl+Q"),
        _ => ("Quit", "Ctrl+Q"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hotkey_boxes_use_each_systems_key_names() {
        assert_eq!(modifier_labels_for(System::Windows), ["Ctrl", "Alt", "Shift", "Win"]);
        assert_eq!(modifier_labels_for(System::Gnome)[3], "Super");
        assert_eq!(modifier_labels_for(System::Kde)[3], "Super");
        assert_eq!(modifier_labels_for(System::Mac), ["⌃", "⌥", "⇧", "⌘"]);
        // The same names as the hotkey labels use.
        assert!(hotkey_label_for(System::Gnome, false, false, false, true, "A").starts_with("Super"));
    }

    #[test]
    fn linux_desktops_are_told_apart() {
        assert_eq!(linux_desktop("ubuntu:GNOME"), System::Gnome);
        assert_eq!(linux_desktop("GNOME"), System::Gnome);
        assert_eq!(linux_desktop("KDE"), System::Kde);
        assert_eq!(linux_desktop("X-Cinnamon"), System::Gnome);
        assert_eq!(linux_desktop("XFCE"), System::OtherLinux);
        assert_eq!(linux_desktop(""), System::OtherLinux);
    }

    #[test]
    fn shortcut_labels_follow_the_system() {
        assert_eq!(keys_for(System::Windows, "Ctrl+Shift+S"), "Ctrl+Shift+S");
        assert_eq!(keys_for(System::Mac, "Ctrl+Shift+S"), "⇧⌘S");
        assert_eq!(keys_for(System::Mac, "Ctrl+Alt+S"), "⌥⌘S");
        assert_eq!(keys_for(System::Mac, "Ctrl+,"), "⌘,");
        assert_eq!(keys_for(System::Gnome, "Win+Shift+S"), "Shift+Super+S");
        assert_eq!(keys_for(System::Mac, "Save at once (Ctrl+Shift+S)"), "Save at once (⇧⌘S)");
        assert_eq!(keys_for(System::Mac, "Ctrl+Z, Ctrl+Y"), "⌘Z, ⌘Y");
        assert_eq!(keys_for(System::Mac, "F2"), "F2", "a key alone stays");
        assert_eq!(keys_for(System::Kde, "Ctrl + Shift + F9"), "Ctrl+Shift+F9");
        assert_eq!(keys_for(System::Mac, "Ctrl+Shift+A / W / F"), "⇧⌘A / W / F");
    }

    #[test]
    fn in_app_capture_keys_leave_apples_keys_alone_on_a_mac() {
        // Apple's own: ⇧⌘A Deselect All, ⇧⌘W Close File, ⇧⌘P Page Setup; ⌥⌘W Close All.
        let apples = [(false, true, 'a'), (false, true, 'w'), (false, true, 'p'), (true, false, 'w')];
        let mut seen = Vec::new();
        for k in AppKey::ALL {
            let mac = app_key_for(true, k);
            assert!(!apples.contains(&mac), "{k:?} takes one of Apple's keys");
            assert!(!seen.contains(&mac), "{k:?} twice");
            seen.push(mac);
        }
        // Windows and Linux keep the keys people know.
        assert_eq!(app_key_text_for(false, AppKey::AllInOne), "Ctrl+Shift+A");
        assert_eq!(app_key_text_for(false, AppKey::Pin), "Ctrl+Shift+P");
        assert_eq!(keys_for(System::Mac, &app_key_text_for(true, AppKey::AllInOne)), "⌥⇧⌘A");
        assert_eq!(keys_for(System::Mac, &app_key_text_for(true, AppKey::Window)), "⌥⇧⌘W");
        assert_eq!(keys_for(System::Mac, &app_key_text_for(true, AppKey::Screen)), "⇧⌘F");
        assert_eq!(keys_for(System::Mac, &app_key_text_for(true, AppKey::Pin)), "⌥⌘P");
        // What the window listens for is what the text says.
        let (m, key) = app_key(AppKey::Screen);
        assert!(m.command && m.shift && !m.alt && key == egui::Key::F);
    }

    #[test]
    fn the_library_trash_key_follows_the_system() {
        assert_eq!(trash_key_for(System::Windows), (false, egui::Key::Delete));
        assert_eq!(trash_key_for(System::Kde), (false, egui::Key::Delete));
        assert_eq!(trash_key_for(System::Mac), (true, egui::Key::Backspace), "Finder's ⌘⌫");
        let press = |key, modifiers| {
            let mut i = egui::InputState::default();
            let raw = egui::RawInput { events: vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }], modifiers, ..Default::default() };
            i = i.begin_pass(raw, false, 1.0, egui::Options::default());
            trash_key_pressed(&i)
        };
        let (cmd, key) = trash_key_for(system());
        let m = if cmd { egui::Modifiers::COMMAND } else { egui::Modifiers::NONE };
        assert!(press(key, m));
        assert!(!press(key, m | egui::Modifiers::SHIFT), "not with Shift");
        assert!(!press(egui::Key::A, m));
    }

    #[test]
    fn hotkey_labels_use_mac_glyphs_in_apples_order() {
        assert_eq!(hotkey_label_for(System::Mac, true, true, true, true, "K"), "⌃⌥⇧⌘K");
        assert_eq!(hotkey_label_for(System::Windows, true, false, true, false, "F9"), "Ctrl + Shift + F9");
        assert_eq!(hotkey_label_for(System::Gnome, false, false, false, true, "S"), "Super + S");
    }

    #[test]
    fn dialog_button_order_follows_the_system() {
        assert!(action_first_for(System::Windows));
        assert!(action_first_for(System::Kde));
        assert!(!action_first_for(System::Gnome));
        assert!(!action_first_for(System::Mac));
        assert!(!action_first_for(System::OtherLinux));
        // Left to right: Windows and KDE "[Delete] [Cancel]", GNOME and macOS "[Cancel] [Delete]".
        assert_eq!(button_order(System::Windows), [Choice::Action, Choice::Cancel]);
        assert_eq!(button_order(System::Kde), [Choice::Action, Choice::Cancel]);
        assert_eq!(button_order(System::Gnome), [Choice::Cancel, Choice::Action]);
        assert_eq!(button_order(System::Mac), [Choice::Cancel, Choice::Action]);
    }

    #[test]
    fn quit_and_trash_words_follow_the_system() {
        let (quit, keys) = quit_item();
        assert!(!quit.is_empty() && !keys.is_empty());
        let t = trash_text("Move to Recycle Bin", "Move to Trash");
        assert_eq!(t == "Move to Recycle Bin", cfg!(windows));
    }
}
