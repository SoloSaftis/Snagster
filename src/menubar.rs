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
    /// The key: a letter, a digit, or one of `,` `?`.
    pub key: char,
}

const fn cmd(key: char) -> Option<Keys> {
    Some(Keys { shift: false, option: false, key })
}
const fn shift_cmd(key: char) -> Option<Keys> {
    Some(Keys { shift: true, option: false, key })
}
const fn opt_cmd(key: char) -> Option<Keys> {
    Some(Keys { shift: false, option: true, key })
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
}

/// The window's state the menus depend on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub picture: bool,
    pub file: bool,
    pub mark: bool,
    pub pictures: bool,
}

impl Needs {
    pub fn met(self, s: State) -> bool {
        match self {
            Needs::Nothing => true,
            Needs::Picture => s.picture,
            Needs::File => s.file,
            Needs::Mark => s.mark,
            Needs::Pictures => s.pictures,
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
const fn edit(text: &'static str, keys: Option<Keys>, k: EditKey) -> Entry {
    Entry::Item { text, keys, run: Run::Key(k), needs: Needs::Nothing }
}
use Entry::Separator as SEP;
use Needs::{File, Mark, Nothing, Picture, Pictures};

/// The menu bar, left to right. Custom menus (Capture) sit between View and Window, as the
/// guidelines ask.
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
            item("Library…", cmd('l'), "library", Nothing),
            SEP,
            Entry::System(System::CloseWindow),
            item("Save", cmd('s'), "save", Picture),
            item("Save as…", shift_cmd('s'), "save_as", Picture),
            item("Quick save to folder", opt_cmd('s'), "quick_save", Picture),
            item("Save as Snagster project (editable marks)…", None, "save_project_as", Picture),
            item("Rename…", None, "rename", Picture),
            item("Export pictures…", None, "export_pictures", Pictures),
            SEP,
            item("Move to Trash", None, "recycle", Picture),
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
            edit("Undo", cmd('z'), EditKey::Undo),
            edit("Redo", shift_cmd('z'), EditKey::Redo),
            SEP,
            edit("Cut", cmd('x'), EditKey::Cut),
            edit("Copy", cmd('c'), EditKey::Copy),
            edit("Paste", cmd('v'), EditKey::Paste),
            item("Paste a picture onto this one", None, "paste_onto", Picture),
            edit("Select All", cmd('a'), EditKey::SelectAll),
            SEP,
            item("Delete mark", None, "delete_mark", Mark),
            item("Make mark bigger", None, "mark_bigger", Mark),
            item("Make mark smaller", None, "mark_smaller", Mark),
            item("Clear all marks", None, "clear_marks", Picture),
            SEP,
            item("Duplicate", None, "duplicate", Picture),
            item("Grab text…", None, "grab_text_picture", Picture),
            item("Pin to screen", shift_cmd('p'), "pin", Picture),
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
            Entry::System(System::FullScreen),
        ],
    },
    MenuDef {
        title: "Capture",
        entries: &[
            item("All-in-one", shift_cmd('a'), "capture_all_in_one", Nothing),
            item("Area", None, "capture_area", Nothing),
            item("Window", shift_cmd('w'), "capture_window_click", Nothing),
            item("Screen", shift_cmd('f'), "capture_screen", Nothing),
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
            item("Getting started", None, "getting_started", Nothing),
            item("Keyboard shortcuts", cmd('?'), "shortcuts", Nothing),
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

/// An English menu text in Apple's Title Case ("Save as…" → "Save As…"). Only for English:
/// other languages keep their own rules.
pub fn title_case(text: &str) -> String {
    const SMALL: &[&str] = &["a", "an", "and", "at", "by", "for", "in", "nor", "of", "on", "or", "the", "to", "with"];
    let words: Vec<&str> = text.split(' ').collect();
    let last = words.len().saturating_sub(1);
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let bare = w.trim_start_matches('(');
            if i != 0 && i != last && SMALL.contains(&bare) {
                return w.to_string();
            }
            let lead = &w[..w.len() - bare.len()];
            let mut c = bare.chars();
            match c.next() {
                Some(f) => format!("{lead}{}{}", f.to_uppercase(), c.as_str()),
                None => w.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A menu text as shown: translated, and in Title Case when Snagster is in English.
pub fn shown(en: &'static str, lang: &str) -> String {
    let text = crate::i18n::tr(en);
    if lang == "en" || lang.starts_with("en-") { title_case(&text) } else { text }
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
    s.push(k.key);
    s
}

/// The native menu bar, kept so items can be greyed out and the texts follow the language.
#[cfg(target_os = "macos")]
pub struct Bar {
    menu: tray_icon::menu::Menu,
    items: Vec<(tray_icon::menu::MenuItem, Needs)>,
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
                        items.push((it, needs));
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

    /// Grey out what can't run now (only touches the menu when something changed).
    pub fn update(&mut self, s: State) {
        if self.state == Some(s) {
            return;
        }
        self.state = Some(s);
        for (it, needs) in &self.items {
            it.set_enabled(needs.met(s));
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
        assert_eq!(titles, ["", "File", "Edit", "View", "Capture", "Window", "Help"]);
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
        assert!(has(Run::Command("pin"), shift_cmd('p')));
        assert!(has(Run::Command("new_image"), cmd('n')));
        assert!(has(Run::Command("open"), cmd('o')));
        assert!(has(Run::Command("library"), cmd('l')));
        assert!(has(Run::Command("capture_all_in_one"), shift_cmd('a')));
    }

    #[test]
    fn items_grey_out_without_what_they_need() {
        let none = State::default();
        let all = State { picture: true, file: true, mark: true, pictures: true };
        for needs in [Picture, File, Mark, Pictures] {
            assert!(!needs.met(none) && needs.met(all), "{needs:?}");
        }
        assert!(Nothing.met(none));
        let save = items().find(|(r, ..)| *r == Run::Command("save")).unwrap();
        assert_eq!(save.3, Picture);
        let reveal = items().find(|(r, ..)| *r == Run::Command("show_in_folder")).unwrap();
        assert_eq!(reveal.3, File);
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
        assert_eq!(title_case("Save as Snagster project (editable marks)…"), "Save As Snagster Project (Editable Marks)…");
        assert_eq!(title_case("What's new"), "What's New");
        assert_eq!(title_case("All-in-one"), "All-in-one");
        assert_eq!(title_case("Paste a picture onto this one"), "Paste a Picture Onto This One");
        assert_eq!(title_case("Zoom in"), "Zoom In", "the last word is capitalised");
    }

    #[test]
    fn every_text_is_translated() {
        let en: std::collections::HashSet<String> = serde_json::from_str::<Vec<String>>(include_str!("../assets/i18n/strings.json")).unwrap().into_iter().collect();
        let mut texts: Vec<&str> = items().map(|(_, t, ..)| t).collect();
        texts.extend(MENUS.iter().map(|m| m.title).filter(|t| !t.is_empty()));
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
