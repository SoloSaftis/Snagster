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
                    let b = egui::Button::new(egui::RichText::new(action).color(egui::Color32::WHITE).strong())
                        .fill(if destructive { crate::look::REC } else { crate::look::accent() })
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
