//! The Mac menu bar: Snagster, File, Edit, View, Capture, Window and Help, as Apple's Human
//! Interface Guidelines lay them out (the menu bar page). Windows and Linux keep the ribbon and
//! its menus, and build this file only for the tests (which check the menus on every system).
//! On macOS it builds them as the app's native menu with muda (MIT/Apache-2.0, already in the tree
//! through tray-icon).
//!
//! Every item runs what its ribbon button or in-window menu item runs: a catalogue command id
//! (`app_command`, the same route AI apps use), or, for the Edit menu's standard items, the key
//! the item stands for, so a text box still gets its own Undo, Copy and Paste.

/// What a menu item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Run {
    /// A command from `catalog::COMMANDS`, run like `app_command` runs it.
    Command(&'static str),
    /// The Edit menu's standard items: handed to the window as the key they stand for.
    Key(EditKey),
    /// Snagster > Settings…: opens Settings on the pane last shown.
    Settings,
    /// Snagster > Quit Snagster: ends Snagster even when closing keeps it in the menu bar.
    Quit,
    /// View > Hide Ribbon / Show Ribbon (Apple's Hide Toolbar).
    Ribbon,
}

/// The standard Edit items that a text box handles itself when it has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKey {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

/// The system's own items, which AppKit runs (muda's predefined items).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum System {
    Services,
    Hide,
    HideOthers,
    ShowAll,
    CloseWindow,
    Minimize,
    Zoom,
    FullScreen,
    BringAllToFront,
}

/// The modifier keys of a menu shortcut (Command is always there, as the guidelines want).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keys {
    pub shift: bool,
    pub option: bool,
    /// The key: a letter, a digit, one of `,` `?`, or `BACKSPACE` (shown as ⌫).
    pub key: char,
}

/// The Delete key left of Return (⌫ on a Mac keyboard), as a `Keys::key`.
pub const BACKSPACE: char = '\u{8}';

const fn cmd(key: char) -> Option<Keys> {
    Some(Keys { shift: false, option: false, key })
}
const fn shift_cmd(key: char) -> Option<Keys> {
    Some(Keys { shift: true, option: false, key })
}
const fn opt_cmd(key: char) -> Option<Keys> {
    Some(Keys { shift: false, option: true, key })
}
/// A Mac in-app key from `platform::app_key_for`.
const fn app_key(k: crate::platform::AppKey) -> Option<Keys> {
    let (option, shift, key) = crate::platform::app_key_for(true, k);
    Some(Keys { shift, option, key })
}

/// What an item needs to be available (it is greyed out otherwise).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Needs {
    Nothing,
    /// A picture open in the editor.
    Picture,
    /// The open picture is saved in a file.
    File,
    /// A mark on the picture is picked.
    Mark,
    /// At least one picture in Recent.
    Pictures,
    /// Edit > Undo: a text box has the keyboard, the picture has a step to undo, or a file was
    /// just moved to the Trash.
    Undo,
    /// Edit > Redo: a text box has the keyboard, or the picture has a step to redo.
    Redo,
    /// Edit > Cut: a text box has the keyboard, or a mark is picked with the selection tool.
    Cut,
    /// Edit > Copy: a text box has the keyboard, or there is a picture (or a mark) to copy.
    Copy,
    /// Edit > Select All: only a text box has something to select.
    Typing,
    /// File > Move to Trash (⌘⌫): a picture saved in a file (a picture never saved would be
    /// dropped with no Undo, as Preview greys the item), the main window is the key window, and no text
    /// box has the keyboard, where ⌘⌫ deletes to the start of the line (a greyed item leaves
    /// the key to the text box, or to the Library).
    Trash,
}

/// The window's state the menus depend on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub picture: bool,
    pub file: bool,
    pub mark: bool,
    pub pictures: bool,
    /// A text box has the keyboard (in Snagster's window or another of its windows), so the
    /// standard Edit items act on its text.
    pub typing: bool,
    /// Snagster's main window is the key window (not Settings, the Library or a dialog).
    pub main_focused: bool,
    /// Undo has something to undo: a step of the picture, or a file just moved to the Trash.
    pub undo: bool,
    /// What Undo undoes is the move to the Trash (Finder's "Undo Move to Trash").
    pub undo_trash: bool,
    pub redo: bool,
    /// A mark is picked with the selection tool (Cut takes it).
    pub cut: bool,
    /// The ribbon is folded away.
    pub ribbon_folded: bool,
}

impl Needs {
    pub fn met(self, s: State) -> bool {
        match self {
            Needs::Nothing => true,
            Needs::Picture => s.picture,
            Needs::File => s.file,
            Needs::Mark => s.mark,
            Needs::Pictures => s.pictures,
            Needs::Undo => s.typing || s.undo,
            Needs::Redo => s.typing || s.redo,
            Needs::Cut => s.typing || s.cut,
            Needs::Copy => s.typing || s.picture,
            Needs::Typing => s.typing,
            // Only from the main window: in the Library ⌘⌫ moves the capture picked there.
            Needs::Trash => s.file && !s.typing && s.main_focused,
        }
    }
}

/// One line of a menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// `text` is the English text (translated, and in Title Case for English, when shown).
    Item { text: &'static str, keys: Option<Keys>, run: Run, needs: Needs },
    System(System),
    Separator,
}

/// One menu of the bar.
pub struct MenuDef {
    /// The English title ("" for the app menu, which macOS titles with the app's name).
    pub title: &'static str,
    pub entries: &'static [Entry],
}

const fn item(text: &'static str, keys: Option<Keys>, id: &'static str, needs: Needs) -> Entry {
    Entry::Item { text, keys, run: Run::Command(id), needs }
}
const fn edit(text: &'static str, keys: Option<Keys>, k: EditKey, needs: Needs) -> Entry {
    Entry::Item { text, keys, run: Run::Key(k), needs }
}
use Entry::Separator as SEP;
use Needs::{File, Mark, Nothing, Picture, Pictures};

/// The menu bar, left to right. Custom menus (Image, Capture) sit between View and Window, as
/// the guidelines ask. Every ribbon command that works on the picture has a menu item, since the
/// ribbon can be hidden.
pub const MENUS: &[MenuDef] = &[
    MenuDef {
        title: "",
        entries: &[
            item("About Snagster", None, "about", Nothing),
            SEP,
            Entry::Item { text: "Settings…", keys: cmd(','), run: Run::Settings, needs: Nothing },
            item("Check for updates", None, "check_updates", Nothing),
            SEP,
            Entry::System(System::Services),
            SEP,
            Entry::System(System::Hide),
            Entry::System(System::HideOthers),
            Entry::System(System::ShowAll),
            SEP,
            Entry::Item { text: "Quit Snagster", keys: cmd('q'), run: Run::Quit, needs: Nothing },
        ],
    },
    MenuDef {
        title: "File",
        entries: &[
            item("New image…", cmd('n'), "new_image", Nothing),
            item("New from clipboard", None, "capture_from_clipboard", Nothing),
            item("Open picture or project…", cmd('o'), "open", Nothing),
            item("Import pictures…", None, "import_pictures", Nothing),
            item("Library", cmd('l'), "library", Nothing),
            SEP,
            Entry::System(System::CloseWindow),
            item("Save", cmd('s'), "save", Picture),
            item("Save as…", shift_cmd('s'), "save_as", Picture),
            item("Quick save to folder", opt_cmd('s'), "quick_save", Picture),
            item("Save as Snagster project (editable marks)…", None, "save_project_as", Picture),
            item("Rename…", None, "rename", Picture),
            item("Export pictures…", None, "export_pictures", Pictures),
            SEP,
            // ⌘⌫, Finder's key for Move to Trash.
            item("Move to Trash", cmd(BACKSPACE), "recycle", Needs::Trash),
            SEP,
            item("Show in Finder", None, "show_in_folder", File),
            item("Copy file path", None, "copy_file_path", File),
            item("Open save folder", None, "open_save_folder", Nothing),
            SEP,
            item("Print…", cmd('p'), "print", Picture),
        ],
    },
    MenuDef {
        title: "Edit",
        entries: &[
            // Reads "Undo Move to Trash" right after a delete (`live_text`).
            edit("Undo", cmd('z'), EditKey::Undo, Needs::Undo),
            edit("Redo", shift_cmd('z'), EditKey::Redo, Needs::Redo),
            SEP,
            edit("Cut", cmd('x'), EditKey::Cut, Needs::Cut),
            edit("Copy", cmd('c'), EditKey::Copy, Needs::Copy),
            // Always on: what the clipboard holds isn't read for every frame.
            edit("Paste", cmd('v'), EditKey::Paste, Nothing),
            item("Paste a picture onto this one", None, "paste_onto", Picture),
            edit("Select All", cmd('a'), EditKey::SelectAll, Needs::Typing),
            SEP,
            item("Delete mark", None, "delete_mark", Mark),
            item("Make mark bigger", None, "mark_bigger", Mark),
            item("Make mark smaller", None, "mark_smaller", Mark),
            item("Clear all marks", None, "clear_marks", Picture),
            SEP,
            item("Duplicate", None, "duplicate", Picture),
            item("Grab text…", None, "grab_text_picture", Picture),
            // ⌥⌘P on a Mac: Apple keeps ⇧⌘P for Page Setup.
            item("Pin to screen", app_key(crate::platform::AppKey::Pin), "pin", Picture),
        ],
    },
    MenuDef {
        title: "View",
        entries: &[
            item("Fit to window", None, "zoom_fit", Picture),
            item("Actual size", None, "zoom_actual", Picture),
            item("Zoom in", None, "zoom_in", Picture),
            item("Zoom out", None, "zoom_out", Picture),
            SEP,
            item("Capture panel", None, "panel", Nothing),
            item("Studio", None, "studio", Nothing),
            SEP,
            // Apple's Hide Toolbar key; reads "Show Ribbon" while it is folded (`live_text`).
            Entry::Item { text: "Hide ribbon", keys: opt_cmd('t'), run: Run::Ribbon, needs: Nothing },
            Entry::System(System::FullScreen),
        ],
    },
    MenuDef {
        title: "Image",
        entries: &[
            item("Crop", None, "crop", Picture),
            item("Resize", None, "resize", Picture),
            item("Canvas", None, "canvas", Picture),
            item("Trim edges", None, "trim", Picture),
            SEP,
            item("Smart redact", None, "smart_redact", Picture),
            item("Simplify", None, "simplify", Picture),
        ],
    },
    MenuDef {
        title: "Capture",
        entries: &[
            // A Mac's own keys (`platform::app_key_for`): Apple keeps ⇧⌘A (Deselect All) and
            // ⇧⌘W (Close File) for other things, so these take ⌥⇧⌘.
            item("All-in-one", app_key(crate::platform::AppKey::AllInOne), "capture_all_in_one", Nothing),
            item("Area", None, "capture_area", Nothing),
            item("Window", app_key(crate::platform::AppKey::Window), "capture_window_click", Nothing),
            item("Screen", app_key(crate::platform::AppKey::Screen), "capture_screen", Nothing),
            item("All screens", None, "capture_all_screens", Nothing),
            item("Last area", None, "capture_last_area", Nothing),
            SEP,
            item("Scrolling", None, "capture_scrolling", Nothing),
            item("Timer", None, "timer", Nothing),
            SEP,
            item("Record", None, "record", Nothing),
            item("Recordings folder", None, "recordings_folder", Nothing),
        ],
    },
    MenuDef {
        title: "Window",
        entries: &[
            Entry::System(System::Minimize),
            Entry::System(System::Zoom),
            SEP,
            Entry::System(System::BringAllToFront),
        ],
    },
    MenuDef {
        title: "Help",
        entries: &[
            // "Snagster Help" on ⌘? comes first, as Apple asks.
            item("Snagster Help", cmd('?'), "getting_started", Nothing),
            item("Keyboard shortcuts", None, "shortcuts", Nothing),
            item("What's new", None, "whats_new", Nothing),
            SEP,
            item("AI apps", None, "ai_apps", Nothing),
            item("Problem reports", None, "problem_reports", Nothing),
        ],
    },
];

/// The menu item id muda carries: "mb:" and the command, so the tray's ids never clash.
pub fn id(run: Run) -> String {
    match run {
        Run::Command(c) => format!("mb:cmd:{c}"),
        Run::Key(k) => format!("mb:key:{}", key_name(k)),
        Run::Settings => "mb:settings".into(),
        Run::Quit => "mb:quit".into(),
        Run::Ribbon => "mb:ribbon".into(),
    }
}

/// An item whose text follows the window's state: Undo names the move to the Trash it would
/// undo (Finder's wording; Redo never does), and the ribbon item says what it will do. `None`
/// keeps the item's own text.
pub fn live_text(run: Run, s: State) -> Option<&'static str> {
    match run {
        // A text box takes ⌘Z for its own text, so the item is a plain Undo then.
        Run::Key(EditKey::Undo) if s.undo_trash && !s.typing => Some("Undo Move to Trash"),
        Run::Key(EditKey::Undo) => Some("Undo"),
        Run::Ribbon if s.ribbon_folded => Some("Show ribbon"),
        Run::Ribbon => Some("Hide ribbon"),
        _ => None,
    }
}

fn key_name(k: EditKey) -> &'static str {
    match k {
        EditKey::Undo => "undo",
        EditKey::Redo => "redo",
        EditKey::Cut => "cut",
        EditKey::Copy => "copy",
        EditKey::Paste => "paste",
        EditKey::SelectAll => "select_all",
    }
}

/// Every item that runs something, in menu order.
pub fn items() -> impl Iterator<Item = (Run, &'static str, Option<Keys>, Needs)> {
    MENUS.iter().flat_map(|m| m.entries.iter()).filter_map(|e| match *e {
        Entry::Item { text, keys, run, needs } => Some((run, text, keys, needs)),
        _ => None,
    })
}

/// The item a muda id belongs to.
pub fn parse(id: &str) -> Option<Run> {
    let rest = id.strip_prefix("mb:")?;
    items().map(|(run, ..)| run).find(|run| self::id(*run).strip_prefix("mb:") == Some(rest))
}

/// The egui events an Edit item hands to the window: what the key would have sent had the menu
/// not taken it (egui-winit turns ⌘X, ⌘C and ⌘V into Cut, Copy and Paste events). `clipboard`
/// is the clipboard's text, for Paste.
pub fn key_events(k: EditKey, clipboard: impl FnOnce() -> String) -> Vec<eframe::egui::Event> {
    use eframe::egui::{Event, Key, Modifiers};
    let press = |key: Key, shift: bool| -> Vec<Event> {
        let modifiers = Modifiers { shift, command: true, mac_cmd: cfg!(target_os = "macos"), ..Modifiers::NONE };
        [true, false]
            .into_iter()
            .map(|pressed| Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers })
            .collect()
    };
    match k {
        EditKey::Undo => press(Key::Z, false),
        EditKey::Redo => press(Key::Z, true),
        EditKey::SelectAll => press(Key::A, false),
        EditKey::Cut => vec![Event::Cut],
        EditKey::Copy => vec![Event::Copy],
        EditKey::Paste => vec![Event::Paste(clipboard())],
    }
}

/// Edit items' events waiting for the window that has the keyboard.
static PENDING: std::sync::Mutex<Vec<eframe::egui::Event>> = std::sync::Mutex::new(Vec::new());

/// Hand an Edit item's events to egui: the window with the keyboard (Snagster's, or Settings)
/// gets them in its next frame, as if the key had been pressed there.
pub fn press(events: Vec<eframe::egui::Event>) {
    if let Ok(mut q) = PENDING.lock() {
        q.extend(events);
    }
}

/// Adds the waiting Edit events to the input of the window that has the keyboard (eframe calls
/// this for every window, the separate Settings and dialog windows too).
#[cfg(target_os = "macos")]
pub struct KeyHooks;

#[cfg(target_os = "macos")]
impl eframe::ViewportHooks for KeyHooks {
    fn raw_input(&self, _ctx: &eframe::egui::Context, raw: &mut eframe::egui::RawInput) {
        deliver(raw);
    }
}

fn deliver(raw: &mut eframe::egui::RawInput) {
    if raw.focused
        && let Ok(mut q) = PENDING.lock()
    {
        raw.events.append(&mut q);
    }
}

/// An English text in Apple's Title Case ("Open picture or project…" → "Open Picture or
/// Project…"). Only for English: other languages keep their own rules.
pub fn title_case(text: &str) -> String {
    const SMALL: &[&str] = &["a", "an", "and", "as", "at", "by", "for", "from", "in", "into", "nor", "of", "on", "onto", "or", "the", "to", "with"];
    // Apple's own spelling of the standard item.
    if text == "Save as…" {
        return "Save As…".into();
    }
    let cap = |w: &str| {
        let bare = w.trim_start_matches('(');
        let lead = &w[..w.len() - bare.len()];
        let mut c = bare.chars();
        match c.next() {
            Some(f) => format!("{lead}{}{}", f.to_uppercase(), c.as_str()),
            None => w.to_string(),
        }
    };
    let words: Vec<&str> = text.split(' ').collect();
    let last = words.len().saturating_sub(1);
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            // Each part of a hyphenated word too ("All-in-One").
            w.split('-')
                .enumerate()
                .map(|(j, part)| {
                    let edge = (i == 0 && j == 0) || (i == last && j == w.split('-').count() - 1);
                    if !edge && SMALL.contains(&part.trim_start_matches('(')) { part.to_string() } else { cap(part) }
                })
                .collect::<Vec<_>>()
                .join("-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A menu text as shown: translated, and in Title Case when Snagster is in English.
pub fn shown(en: &str, lang: &str) -> String {
    in_title_case(crate::i18n::tr(en), lang)
}

/// A translated menu or window title, in Title Case when Snagster is in English.
pub fn in_title_case(text: String, lang: &str) -> String {
    if lang == "en" || lang.starts_with("en-") { title_case(&text) } else { text }
}

/// A global hotkey as a muda key accelerator ("Ctrl+Alt+Shift+Cmd+F9"), so a menu bar extra's
/// menu shows it the Mac way, right-aligned. Ctrl is the Control key and Win the Command key.
pub fn hotkey_accelerator(ctrl: bool, alt: bool, shift: bool, win: bool, key: &str) -> Option<String> {
    if key.is_empty() {
        return None;
    }
    let mut s = String::new();
    for (on, name) in [(ctrl, "Ctrl+"), (alt, "Alt+"), (shift, "Shift+"), (win, "Cmd+")] {
        if on {
            s.push_str(name);
        }
    }
    // A letter in lower case: an upper-case one would add Shift on its own.
    let key = if key.chars().count() == 1 { key.to_lowercase() } else { key.to_string() };
    let s = s + &key;
    s.parse::<tray_icon::menu::accelerator::KeyAccelerator>().is_ok().then_some(s)
}

/// The English text of a system item (each is its own sentence in the packs).
pub fn system_text(s: System) -> &'static str {
    match s {
        System::Services => "Services",
        System::Hide => "Hide Snagster",
        System::HideOthers => "Hide Others",
        System::ShowAll => "Show All",
        System::CloseWindow => "Close Window",
        System::Minimize => "Minimize",
        System::Zoom => "Zoom",
        System::FullScreen => "Enter Full Screen",
        System::BringAllToFront => "Bring All to Front",
    }
}

/// A shortcut as muda's key accelerators take it ("Cmd+Shift+s"): by the character, so ⌘, and
/// ⌘? work on every keyboard layout.
pub fn accelerator_text(k: Keys) -> String {
    let mut s = String::from("Cmd+");
    if k.option {
        s.push_str("Alt+");
    }
    if k.shift {
        s.push_str("Shift+");
    }
    if k.key == BACKSPACE {
        s.push_str("Backspace");
    } else {
        s.push(k.key);
    }
    s
}

/// The native menu bar, kept so items can be greyed out and the texts follow the language.
#[cfg(target_os = "macos")]
pub struct Bar {
    menu: tray_icon::menu::Menu,
    items: Vec<(tray_icon::menu::MenuItem, Run, Needs)>,
    state: Option<State>,
}

#[cfg(target_os = "macos")]
impl Bar {
    /// Build the menus in the current language and make them the app's menu bar.
    pub fn install() -> Result<Bar, String> {
        use tray_icon::menu::{Menu, MenuId, MenuItemBuilder, PredefinedMenuItem, Submenu};
        let lang = crate::i18n::current();
        let menu = Menu::new();
        let mut items = Vec::new();
        for m in MENUS {
            let title = if m.title.is_empty() { "Snagster".to_string() } else { shown(m.title, lang) };
            let sub = Submenu::new(title, true);
            for e in m.entries {
                let res = match *e {
                    Entry::Separator => sub.append(&PredefinedMenuItem::separator()),
                    Entry::System(s) => {
                        let text = crate::i18n::tr(system_text(s));
                        let t = Some(text.as_str());
                        sub.append(&match s {
                            System::Services => PredefinedMenuItem::services(t),
                            System::Hide => PredefinedMenuItem::hide(t),
                            System::HideOthers => PredefinedMenuItem::hide_others(t),
                            System::ShowAll => PredefinedMenuItem::show_all(t),
                            System::CloseWindow => PredefinedMenuItem::close_window(t),
                            System::Minimize => PredefinedMenuItem::minimize(t),
                            System::Zoom => PredefinedMenuItem::maximize(t),
                            System::FullScreen => PredefinedMenuItem::fullscreen(t),
                            System::BringAllToFront => PredefinedMenuItem::bring_all_to_front(t),
                        })
                    }
                    Entry::Item { text, keys, run, needs } => {
                        let it = MenuItemBuilder::new()
                            .id(MenuId::new(id(run)))
                            .text(shown(text, lang))
                            .key_accelerator(keys.map(accelerator_text))
                            .map_err(|e| e.to_string())?
                            .build();
                        let r = sub.append(&it);
                        items.push((it, run, needs));
                        r
                    }
                };
                res.map_err(|e| e.to_string())?;
            }
            menu.append(&sub).map_err(|e| e.to_string())?;
            match m.title {
                "Window" => sub.set_as_windows_menu_for_nsapp(),
                "Help" => sub.set_as_help_menu_for_nsapp(),
                _ => {}
            }
        }
        menu.init_for_nsapp();
        Ok(Bar { menu, items, state: None })
    }

    /// Grey out what can't run now and set the texts that follow the state (only touches the
    /// menu when something changed).
    pub fn update(&mut self, s: State) {
        if self.state == Some(s) {
            return;
        }
        self.state = Some(s);
        let lang = crate::i18n::current();
        for (it, run, needs) in &self.items {
            it.set_enabled(needs.met(s));
            if let Some(text) = live_text(*run, s) {
                let text = shown(text, lang);
                if it.text() != text {
                    it.set_text(text);
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for Bar {
    fn drop(&mut self) {
        self.menu.remove_for_nsapp();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_has_apples_menus_in_order() {
        let titles: Vec<&str> = MENUS.iter().map(|m| m.title).collect();
        assert_eq!(titles, ["", "File", "Edit", "View", "Image", "Capture", "Window", "Help"]);
    }

    #[test]
    fn the_app_menu_is_laid_out_as_apple_asks() {
        let app = MENUS[0].entries;
        let first = |e: &Entry| matches!(e, Entry::Item { text: "About Snagster", .. });
        assert!(first(&app[0]), "About comes first");
        assert!(app.iter().any(|e| matches!(e, Entry::Item { text: "Settings…", keys: Some(Keys { key: ',', shift: false, option: false }), run: Run::Settings, .. })));
        for s in [System::Services, System::Hide, System::HideOthers, System::ShowAll] {
            assert!(app.contains(&Entry::System(s)), "{s:?}");
        }
        assert!(matches!(app.last(), Some(Entry::Item { text: "Quit Snagster", keys: Some(Keys { key: 'q', shift: false, option: false }), run: Run::Quit, .. })));
        let window = MENUS.iter().find(|m| m.title == "Window").unwrap().entries;
        assert!(window.contains(&Entry::System(System::Minimize)) && window.contains(&Entry::System(System::BringAllToFront)));
    }

    #[test]
    fn every_command_is_a_real_window_command() {
        for (run, text, ..) in items() {
            if let Run::Command(c) = run {
                assert!(crate::catalog::COMMANDS.iter().any(|k| k.id == c), "{text}: {c} isn't in the catalogue");
                assert!(crate::app::commands_ui::parse(c, None).is_ok(), "{text}: the window doesn't run {c}");
            }
        }
    }

    #[test]
    fn every_id_reads_back_and_is_unique() {
        let mut seen = std::collections::HashSet::new();
        for (run, text, ..) in items() {
            let id = id(run);
            assert_eq!(parse(&id), Some(run), "{text}");
            assert!(seen.insert(id.clone()), "{id} twice");
        }
        assert_eq!(parse("mb:cmd:no_such"), None);
        assert_eq!(parse("cmd:save"), None);
        assert_eq!(parse("a:Area"), None);
    }

    #[test]
    fn no_two_items_share_a_shortcut() {
        let mut seen = std::collections::HashSet::new();
        for (_, text, keys, _) in items() {
            if let Some(k) = keys {
                assert!(seen.insert((k.shift, k.option, k.key)), "{text}: {} twice", accelerator_text(k));
            }
        }
        // The system items' own keys: ⌘H, ⌥⌘H, ⌘W, ⌘M, ⌃⌘F.
        assert!(!seen.contains(&(false, false, 'h')) && !seen.contains(&(false, true, 'h')));
        assert!(!seen.contains(&(false, false, 'w')) && !seen.contains(&(false, false, 'm')));
        // Standard Mac shortcuts Apple keeps for other commands: Deselect All, Close File,
        // Page Setup.
        for k in ['a', 'w', 'p'] {
            assert!(!seen.contains(&(true, false, k)), "⇧⌘{k} is Apple's");
        }
    }

    #[test]
    fn shortcuts_read_as_muda_accelerators() {
        for (_, text, keys, _) in items() {
            if let Some(k) = keys {
                let s = accelerator_text(k);
                assert!(s.parse::<tray_icon::menu::accelerator::KeyAccelerator>().is_ok(), "{text}: {s}");
            }
        }
        assert_eq!(accelerator_text(Keys { shift: true, option: false, key: 's' }), "Cmd+Shift+s");
        assert_eq!(accelerator_text(Keys { shift: false, option: true, key: 's' }), "Cmd+Alt+s");
        assert_eq!(accelerator_text(Keys { shift: false, option: false, key: ',' }), "Cmd+,");
        assert_eq!(accelerator_text(Keys { shift: false, option: false, key: '?' }), "Cmd+?");
    }

    #[test]
    fn the_menus_use_the_windows_own_keys() {
        // The in-window shortcuts (`SnagApp::shortcuts`) and the menu agree, so a key does the
        // same with or without the menu bar.
        let keys: Vec<(Run, Keys)> = items().filter_map(|(r, _, k, _)| k.map(|k| (r, k))).collect();
        let has = |run: Run, k: Option<Keys>| keys.contains(&(run, k.unwrap()));
        assert!(has(Run::Command("save"), cmd('s')));
        assert!(has(Run::Command("save_as"), shift_cmd('s')));
        assert!(has(Run::Command("quick_save"), opt_cmd('s')));
        assert!(has(Run::Command("print"), cmd('p')));

        assert!(has(Run::Command("new_image"), cmd('n')));
        assert!(has(Run::Command("open"), cmd('o')));
        assert!(has(Run::Command("library"), cmd('l')));
        assert!(has(Run::Command("getting_started"), cmd('?')), "⌘? opens Snagster Help");
    }

    #[test]
    fn items_grey_out_without_what_they_need() {
        let none = State::default();
        let all = State { picture: true, file: true, mark: true, pictures: true, undo: true, redo: true, cut: true, main_focused: true, ..State::default() };
        for needs in [Picture, File, Mark, Pictures, Needs::Undo, Needs::Redo, Needs::Cut, Needs::Copy, Needs::Trash] {
            assert!(!needs.met(none) && needs.met(all), "{needs:?}");
        }
        assert!(Nothing.met(none));
        let save = items().find(|(r, ..)| *r == Run::Command("save")).unwrap();
        assert_eq!(save.3, Picture);
        let reveal = items().find(|(r, ..)| *r == Run::Command("show_in_folder")).unwrap();
        assert_eq!(reveal.3, File);
    }

    fn needs_of(run: Run) -> Needs {
        items().find(|(r, ..)| *r == run).unwrap().3
    }

    #[test]
    fn edit_items_grey_out_when_they_cant_act() {
        let key = |k| needs_of(Run::Key(k));
        // Nothing open, nothing typed: only Paste (the clipboard isn't read every frame).
        let idle = State::default();
        for k in [EditKey::Undo, EditKey::Redo, EditKey::Cut, EditKey::Copy, EditKey::SelectAll] {
            assert!(!key(k).met(idle), "{k:?} greys out with nothing to act on");
        }
        assert!(key(EditKey::Paste).met(idle));
        // A text box has the keyboard: every standard item is its own.
        let typing = State { typing: true, ..idle };
        for k in [EditKey::Undo, EditKey::Redo, EditKey::Cut, EditKey::Copy, EditKey::Paste, EditKey::SelectAll] {
            assert!(key(k).met(typing), "{k:?} works in a text box");
        }
        // A picture: Copy copies it; Cut needs a picked mark; Undo/Redo need a step.
        let picture = State { picture: true, ..idle };
        assert!(key(EditKey::Copy).met(picture));
        assert!(!key(EditKey::Cut).met(picture) && key(EditKey::Cut).met(State { cut: true, ..picture }));
        assert!(!key(EditKey::Undo).met(picture) && key(EditKey::Undo).met(State { undo: true, ..picture }));
        assert!(!key(EditKey::Redo).met(picture) && key(EditKey::Redo).met(State { redo: true, ..picture }));
        assert!(!key(EditKey::SelectAll).met(picture), "nothing to select outside a text box");
        // Right after a delete, Undo brings the file back even with no picture open.
        assert!(key(EditKey::Undo).met(State { undo: true, undo_trash: true, ..idle }));
    }

    #[test]
    fn move_to_trash_leaves_command_delete_to_a_text_box() {
        let trash = needs_of(Run::Command("recycle"));
        let picture = State { picture: true, file: true, main_focused: true, ..State::default() };
        assert!(trash.met(picture));
        assert!(!trash.met(State { file: false, ..picture }), "a picture never saved isn't dropped by one ⌘⌫ (no Undo for it)");
        assert!(!trash.met(State { typing: true, ..picture }), "⌘⌫ deletes text in a text box, never the picture");
        assert!(!trash.met(State { main_focused: false, ..picture }), "in the Library ⌘⌫ is the Library's own");
        assert!(!trash.met(State::default()));
        let k = items().find(|(r, ..)| *r == Run::Command("recycle")).unwrap().2.unwrap();
        assert_eq!(accelerator_text(k), "Cmd+Backspace", "Finder's ⌘⌫");
    }

    #[test]
    fn undo_names_the_move_to_the_trash() {
        let undo = Run::Key(EditKey::Undo);
        assert_eq!(live_text(undo, State::default()), Some("Undo"));
        let after_delete = State { undo: true, undo_trash: true, ..State::default() };
        assert_eq!(live_text(undo, after_delete), Some("Undo Move to Trash"));
        assert_eq!(live_text(undo, State { typing: true, ..after_delete }), Some("Undo"), "a text box's own Undo");
        assert_eq!(live_text(Run::Key(EditKey::Redo), after_delete), None, "never Redo Move to Trash");
        assert_eq!(live_text(Run::Command("save"), after_delete), None);
        assert_eq!(shown("Undo Move to Trash", "en"), "Undo Move to Trash");
    }

    #[test]
    fn the_view_menu_hides_and_shows_the_ribbon() {
        let view = MENUS.iter().find(|m| m.title == "View").unwrap().entries;
        let at = view.iter().position(|e| matches!(e, Entry::Item { run: Run::Ribbon, .. })).expect("a ribbon item in View");
        assert!(matches!(view[at], Entry::Item { keys: Some(Keys { option: true, shift: false, key: 't' }), .. }), "Apple's ⌥⌘T (Hide Toolbar)");
        assert_eq!(live_text(Run::Ribbon, State::default()), Some("Hide ribbon"));
        assert_eq!(live_text(Run::Ribbon, State { ribbon_folded: true, ..State::default() }), Some("Show ribbon"));
        assert_eq!(shown("Show ribbon", "en"), "Show Ribbon");
        assert_eq!(parse(&id(Run::Ribbon)), Some(Run::Ribbon));
    }

    #[test]
    fn capture_items_show_the_macs_own_keys() {
        use crate::platform::{AppKey, app_key_for};
        for (run, k) in [("capture_all_in_one", AppKey::AllInOne), ("capture_window_click", AppKey::Window), ("capture_screen", AppKey::Screen), ("pin", AppKey::Pin)] {
            let keys = items().find(|(r, ..)| *r == Run::Command(run)).unwrap().2.expect(run);
            let (option, shift, key) = app_key_for(true, k);
            assert_eq!((keys.option, keys.shift, keys.key), (option, shift, key), "{run}: the menu and the window agree");
        }
    }

    #[test]
    fn edit_items_send_what_the_key_would() {
        use eframe::egui::{Event, Key};
        assert_eq!(key_events(EditKey::Copy, || unreachable!()), vec![Event::Copy]);
        assert_eq!(key_events(EditKey::Cut, || unreachable!()), vec![Event::Cut]);
        assert_eq!(key_events(EditKey::Paste, || "hi".into()), vec![Event::Paste("hi".into())]);
        let undo = key_events(EditKey::Undo, String::new);
        assert_eq!(undo.len(), 2, "pressed and let go");
        match (&undo[0], &undo[1]) {
            (Event::Key { key: Key::Z, pressed: true, modifiers: m, .. }, Event::Key { key: Key::Z, pressed: false, .. }) => assert!(m.command && !m.shift),
            other => panic!("{other:?}"),
        }
        match &key_events(EditKey::Redo, String::new)[0] {
            Event::Key { key: Key::Z, modifiers: m, .. } => assert!(m.command && m.shift),
            other => panic!("{other:?}"),
        }
        assert!(matches!(&key_events(EditKey::SelectAll, String::new)[0], Event::Key { key: Key::A, .. }));
    }

    #[test]
    fn edit_events_go_to_the_window_with_the_keyboard() {
        use eframe::egui::{Event, RawInput};
        press(vec![Event::Copy]);
        let mut behind = RawInput { focused: false, ..Default::default() };
        deliver(&mut behind);
        assert!(behind.events.is_empty(), "a window without the keyboard gets nothing");
        let mut front = RawInput { focused: true, ..Default::default() };
        deliver(&mut front);
        assert_eq!(front.events, vec![Event::Copy]);
        let mut again = RawInput { focused: true, ..Default::default() };
        deliver(&mut again);
        assert!(again.events.is_empty(), "delivered once");
    }

    #[test]
    fn hotkeys_read_as_accelerators() {
        assert_eq!(hotkey_accelerator(true, false, true, true, "A").as_deref(), Some("Ctrl+Shift+Cmd+a"));
        assert_eq!(hotkey_accelerator(false, true, false, true, "F9").as_deref(), Some("Alt+Cmd+F9"));
        assert_eq!(hotkey_accelerator(false, false, false, true, "2").as_deref(), Some("Cmd+2"));
        assert_eq!(hotkey_accelerator(true, false, false, false, ""), None);
        // Every key the hotkey editor offers either reads or is left out (never a broken menu).
        for (name, _, _) in crate::hotkeys::KEYS {
            let _ = hotkey_accelerator(false, false, true, true, name);
        }
    }

    #[test]
    fn menu_texts_are_title_case_only_in_english() {
        assert_eq!(shown("Save as…", "en"), "Save As…");
        assert_eq!(shown("Save as…", "en-GB"), "Save As…");
        // Not loaded as the interface language here, so the English comes back unchanged.
        assert_eq!(shown("Save as…", "de"), "Save as…");
    }

    #[test]
    fn english_menus_are_in_title_case() {
        assert_eq!(title_case("Save as…"), "Save As…");
        assert_eq!(title_case("Open picture or project…"), "Open Picture or Project…");
        assert_eq!(title_case("Move to Trash"), "Move to Trash");
        assert_eq!(title_case("Save as Snagster project (editable marks)…"), "Save as Snagster Project (Editable Marks)…");
        assert_eq!(title_case("New from clipboard"), "New from Clipboard");
        assert_eq!(title_case("What's new"), "What's New");
        assert_eq!(title_case("All-in-one"), "All-in-One");
        assert_eq!(title_case("Paste a picture onto this one"), "Paste a Picture onto This One");
        assert_eq!(title_case("Saving & library"), "Saving & Library");
        assert_eq!(title_case("Zoom in"), "Zoom In", "the last word is capitalised");
    }

    #[test]
    fn every_text_is_translated() {
        let en: std::collections::HashSet<String> = serde_json::from_str::<Vec<String>>(include_str!("../assets/i18n/strings.json")).unwrap().into_iter().collect();
        let mut texts: Vec<&str> = items().map(|(_, t, ..)| t).collect();
        texts.extend(MENUS.iter().map(|m| m.title).filter(|t| !t.is_empty()));
        let changed = State { undo_trash: true, ribbon_folded: true, ..State::default() };
        texts.extend(items().filter_map(|(run, ..)| live_text(run, changed)));
        let systems = [
            System::Services,
            System::Hide,
            System::HideOthers,
            System::ShowAll,
            System::CloseWindow,
            System::Minimize,
            System::Zoom,
            System::FullScreen,
            System::BringAllToFront,
        ];
        texts.extend(systems.map(system_text));
        for t in texts {
            assert!(en.contains(t), "{t:?} isn't in assets/i18n/strings.json");
        }
    }
}
