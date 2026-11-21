//! The main window driven from the keyboard alone: real key events go through egui into the
//! whole window (ribbon, tools, picture, options, Recent), frame by frame, with AccessKit on as
//! a screen reader would have it. No screen is needed, so this runs with `cargo test` everywhere.

use super::*;
use crate::i18n::t;
use egui::Modifiers;

const WINDOW: Vec2 = vec2(1400.0, 900.0);

/// A person at Snagster with a keyboard and a screen reader.
struct Keys {
    app: SnagApp,
    ctx: egui::Context,
    time: f64,
    mods: Modifiers,
    /// What AccessKit was last told has the focus.
    reader: Option<egui::accesskit::NodeId>,
    /// The window's size.
    size: Vec2,
}

impl Keys {
    fn new() -> Keys {
        Keys::sized(WINDOW)
    }

    fn sized(size: Vec2) -> Keys {
        let mut app = SnagApp::blank(Settings::default(), false, None);
        app.add_entry("Test".into(), Doc::new(RgbaImage::from_pixel(400, 300, image::Rgba([255, 255, 255, 255]))));
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut k = Keys { app, ctx, time: 0.0, mods: Modifiers::NONE, reader: None, size };
        k.frame(Vec::new());
        k.frame(Vec::new());
        k
    }

    fn frame(&mut self, mut events: Vec<Event>) {
        self.time += 1.0 / 60.0;
        events.insert(0, Event::ModifiersChanged(self.mods));
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)), time: Some(self.time), events, ..Default::default() };
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(raw, |ui| app.ui_body(ui));
        out.textures_delta.clear();
        if let Some(u) = out.platform_output.accesskit_update {
            self.reader = Some(u.focus);
        }
    }

    fn press(&mut self, key: Key, modifiers: Modifiers) {
        let e = |pressed| Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
        self.frame(vec![e(true)]);
        self.frame(vec![e(false)]);
    }

    fn key(&mut self, key: Key) {
        self.press(key, Modifiers::NONE);
    }

    fn alt_tap(&mut self) {
        self.mods = Modifiers::ALT;
        self.frame(Vec::new());
        self.mods = Modifiers::NONE;
        self.frame(Vec::new());
    }

    fn alt_and(&mut self, key: Key) {
        self.mods = Modifiers::ALT;
        self.press(key, Modifiers::ALT);
        self.mods = Modifiers::NONE;
        self.frame(Vec::new());
    }

    fn focused(&self) -> Option<Id> {
        self.ctx.memory(|m| m.focused())
    }

    fn pane(&self) -> Option<Pane> {
        self.app.x.keys.pane_of(&self.ctx, self.focused())
    }

    /// The noted widget with the keyboard.
    fn item(&self) -> Option<Item> {
        let f = self.focused()?;
        self.app.x.keys.last.iter().find(|i| i.id == f).cloned()
    }

    /// The screen reader is told about every move.
    fn reader_follows(&self) {
        let f = self.focused().expect("something has the keyboard");
        assert_eq!(self.reader, Some(f.accesskit_id()), "AccessKit wasn't told about the focus");
    }

    /// The key tip of the widget called `label` in `scope` right now.
    fn tip_of(&self, scope: Scope, label: &str) -> String {
        let (items, tips) = scoped(&self.app.x.keys.last, scope);
        let i = items.iter().position(|i| i.label == label).unwrap_or_else(|| panic!("no {label:?} in {scope:?}: {:?}", items.iter().map(|i| &i.label).collect::<Vec<_>>()));
        tips[i].clone()
    }

    fn type_tip(&mut self, tip: &str) {
        for c in tip.chars() {
            self.key(Key::from_name(&c.to_string()).expect("a letter or digit key"));
        }
    }
}

#[test]
fn f6_goes_round_every_pane_and_the_screen_reader_follows() {
    let mut k = Keys::new();
    assert_eq!(k.focused(), None);
    k.key(Key::F6);
    assert_eq!(k.pane(), Some(Pane::Ribbon));
    let it = k.item().expect("a ribbon control");
    assert_eq!(it.tab, Some(RibbonTab::Home), "F6 lands on the open tab, as in Office");
    k.reader_follows();
    k.key(Key::F6);
    assert_eq!(k.pane(), Some(Pane::Tools));
    assert!(k.item().is_some_and(|i| i.on), "on the chosen tool");
    k.reader_follows();
    k.key(Key::F6);
    assert_eq!(k.pane(), Some(Pane::Canvas));
    k.reader_follows();
    k.key(Key::F6);
    assert_eq!(k.pane(), Some(Pane::Options), "the options panel's first control");
    k.reader_follows();
    k.key(Key::F6);
    assert_eq!(k.pane(), Some(Pane::Recent));
    assert!(k.item().is_some_and(|i| i.on && i.label == "Test"), "on the picture shown");
    k.reader_follows();
    k.key(Key::F6);
    assert_eq!(k.pane(), Some(Pane::Ribbon), "and round again");
    // Shift+F6 goes back.
    k.press(Key::F6, Modifiers::SHIFT);
    assert_eq!(k.pane(), Some(Pane::Recent));
    k.press(Key::F6, Modifiers::SHIFT);
    assert_eq!(k.pane(), Some(Pane::Options));
}

#[test]
fn f6_skips_hidden_panes() {
    let mut k = Keys::new();
    k.app.settings.show_side_panel = false;
    k.app.settings.show_tray = false;
    k.frame(Vec::new());
    let mut seen = Vec::new();
    for _ in 0..4 {
        k.key(Key::F6);
        seen.push(k.pane().unwrap());
    }
    assert_eq!(seen, [Pane::Ribbon, Pane::Tools, Pane::Canvas, Pane::Ribbon]);
}

#[test]
fn arrows_move_within_a_pane_and_never_out_of_it() {
    let mut k = Keys::new();
    k.key(Key::F6);
    k.key(Key::F6);
    assert_eq!(k.pane(), Some(Pane::Tools));
    let first = k.focused();
    k.key(Key::ArrowDown);
    assert_ne!(k.focused(), first, "Down goes to the next tool");
    assert_eq!(k.pane(), Some(Pane::Tools));
    k.reader_follows();
    // However far the keys go, the keyboard stays in the tool column.
    for key in [Key::ArrowRight, Key::ArrowRight, Key::ArrowRight, Key::ArrowUp, Key::ArrowUp, Key::ArrowLeft, Key::ArrowLeft, Key::ArrowLeft] {
        k.key(key);
        assert_eq!(k.pane(), Some(Pane::Tools), "{key:?} left the tools");
    }
    for _ in 0..20 {
        k.key(Key::ArrowDown);
        assert_eq!(k.pane(), Some(Pane::Tools));
    }
    // In the ribbon Right goes along the tabs, Down into the band, and Up out of the band
    // stays in the ribbon.
    k.press(Key::F6, Modifiers::SHIFT);
    assert_eq!(k.item().and_then(|i| i.tab), Some(RibbonTab::Home));
    k.key(Key::ArrowRight);
    assert_eq!(k.item().and_then(|i| i.tab), Some(RibbonTab::Capture));
    k.key(Key::ArrowLeft);
    k.key(Key::ArrowDown);
    assert_eq!(k.item().map(|i| i.scope), Some(Scope::Band));
    for _ in 0..5 {
        k.key(Key::ArrowUp);
        assert_eq!(k.pane(), Some(Pane::Ribbon));
    }
}

#[test]
fn enter_and_space_run_the_focused_control() {
    let mut k = Keys::new();
    k.key(Key::F6);
    k.key(Key::F6);
    let before = k.app.tool;
    k.key(Key::ArrowDown);
    let label = k.item().unwrap().label;
    k.key(Key::Enter);
    assert_ne!(k.app.tool, before);
    assert_eq!(t(k.app.tool.label()), label, "Enter chose the tool with the keyboard");
    k.key(Key::ArrowRight);
    k.key(Key::Space);
    assert_eq!(t(k.app.tool.label()), k.item().unwrap().label, "and Space");
}

#[test]
fn esc_in_a_pane_goes_back_to_the_picture_whose_keys_then_work() {
    let mut k = Keys::new();
    k.key(Key::F6);
    k.key(Key::F6);
    assert_eq!(k.pane(), Some(Pane::Tools));
    k.key(Key::Escape);
    assert_eq!(k.pane(), Some(Pane::Canvas));
    k.reader_follows();
    // With the picture focused its one-letter keys still choose tools.
    let target = Tool::ALL.into_iter().find(|t| *t != k.app.tool && *t != Tool::Select).unwrap();
    k.key(Key::from_name(target.key()).unwrap());
    assert_eq!(k.app.tool, target);
    assert_eq!(k.pane(), Some(Pane::Canvas), "a tool's key doesn't move the keyboard");
}

#[test]
fn arrows_on_the_picture_move_the_picked_mark() {
    let mut k = Keys::new();
    let ci = k.app.current.unwrap();
    let r = Rect::from_min_size(pos2(50.0, 50.0), vec2(40.0, 30.0));
    k.app.entries[ci].doc.shapes.push(Shape::Rect { rect: r, color: Color32::RED, width: 3.0, fill: false, radius: 0.0, shadow: false });
    k.app.selected = Some(0);
    for _ in 0..3 {
        k.key(Key::F6);
    }
    assert_eq!(k.pane(), Some(Pane::Canvas));
    k.key(Key::ArrowRight);
    k.press(Key::ArrowDown, Modifiers::SHIFT);
    let Shape::Rect { rect, .. } = &k.app.entries[ci].doc.shapes[0] else { panic!() };
    assert_eq!(rect.min, pos2(51.0, 60.0), "one pixel right, ten down with Shift");
    assert_eq!(k.pane(), Some(Pane::Canvas), "the arrows don't take the keyboard off the picture");
    assert_eq!(k.app.entries[ci].undo.len(), 1, "arrow after arrow (a held key) is one step to undo");
    // A while later it's a new step.
    k.time += 2.0;
    k.key(Key::ArrowLeft);
    assert_eq!(k.app.entries[ci].undo.len(), 2);
}

/// Wayland: a dialog inside the window has the keys. With the picture focused and a mark
/// picked, the arrows, Delete and F6 don't touch the picture or the panes behind it; Esc closes
/// the dialog, and then the arrows move the mark again.
#[test]
fn a_dialog_inside_the_window_keeps_the_editor_keys_off_the_picture() {
    let mut k = Keys::new();
    k.ctx.set_embed_viewports(true);
    let ci = k.app.current.unwrap();
    let r = Rect::from_min_size(pos2(50.0, 50.0), vec2(40.0, 30.0));
    k.app.entries[ci].doc.shapes.push(Shape::Rect { rect: r, color: Color32::RED, width: 3.0, fill: false, radius: 0.0, shadow: false });
    k.app.selected = Some(0);
    for _ in 0..3 {
        k.key(Key::F6);
    }
    assert_eq!(k.pane(), Some(Pane::Canvas));
    k.app.about_open = true;
    k.frame(Vec::new());
    assert_eq!(k.pane(), None, "the picture behind gave up the keyboard");
    for key in [Key::ArrowRight, Key::Delete] {
        k.key(key);
    }
    k.key(Key::F6);
    assert_eq!(k.pane(), None, "F6 doesn't move the keyboard into the panes behind the dialog");
    let Shape::Rect { rect, .. } = &k.app.entries[ci].doc.shapes[0] else { panic!("the mark was deleted") };
    assert_eq!(rect.min, r.min, "the arrows didn't move the mark behind the dialog");
    assert!(k.app.about_open, "the dialog is still open");
    k.key(Key::Escape);
    assert!(!k.app.about_open, "Esc closed the dialog");
    assert_eq!(k.app.entries[ci].doc.shapes.len(), 1, "and not the mark");
}

/// Wayland: a big box beside the editor (here Keyboard shortcuts) has the keys from its
/// opening: F6 and Delete stay off the editor until a click on the editor gives them back.
#[test]
fn a_big_box_inside_the_window_keeps_f6_and_delete_off_the_editor_until_a_click_on_it() {
    let mut k = Keys::new();
    k.ctx.set_embed_viewports(true);
    let ci = k.app.current.unwrap();
    let r = Rect::from_min_size(pos2(50.0, 50.0), vec2(40.0, 30.0));
    k.app.entries[ci].doc.shapes.push(Shape::Rect { rect: r, color: Color32::RED, width: 3.0, fill: false, radius: 0.0, shadow: false });
    k.app.selected = Some(0);
    k.app.shortcuts_open = true;
    k.frame(Vec::new());
    k.key(Key::F6);
    assert_eq!(k.pane(), None, "F6 is the box's, not the editor's panes");
    k.key(Key::Delete);
    assert_eq!(k.app.entries[ci].doc.shapes.len(), 1, "Delete didn't remove the mark behind");
    assert!(k.app.shortcuts_open);
    // A click on the editor, beside the box (the box is centred, so the left edge is clear).
    let at = pos2(4.0, WINDOW.y / 2.0);
    let b = |pressed| Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    k.frame(vec![Event::PointerMoved(at), b(true), b(false)]);
    k.frame(Vec::new());
    k.key(Key::F6);
    assert!(k.pane().is_some(), "the editor's F6 works again");
    assert!(k.app.shortcuts_open, "the box stays open beside the editor");
}

#[test]
fn with_a_ribbon_button_focused_the_shortcuts_still_work() {
    let mut k = Keys::new();
    k.key(Key::F6);
    assert_eq!(k.pane(), Some(Pane::Ribbon));
    let target = Tool::ALL.into_iter().find(|t| *t != k.app.tool && *t != Tool::Select).unwrap();
    k.key(Key::from_name(target.key()).unwrap());
    assert_eq!(k.app.tool, target, "a tool's letter works as in GTK, Qt and Windows apps");
}

#[test]
fn alt_puts_the_keyboard_on_the_open_tab_and_esc_gives_it_back() {
    if tip_style(crate::platform::system()) != TipStyle::Office {
        return;
    }
    let mut k = Keys::new();
    for _ in 0..3 {
        k.key(Key::F6);
    }
    let canvas = k.focused();
    assert_eq!(k.pane(), Some(Pane::Canvas));
    k.alt_tap();
    assert_eq!(k.item().and_then(|i| i.tab), Some(RibbonTab::Home), "as in Office, the open tab has the keyboard");
    k.reader_follows();
    k.key(Key::Escape);
    assert_eq!(k.app.x.keys.tips, Tips::Off);
    assert_eq!(k.focused(), canvas, "back where it was");
    // From nowhere, back to nowhere.
    k.ctx.memory_mut(|m| m.surrender_focus(canvas.unwrap()));
    k.frame(Vec::new());
    k.key(Key::F10);
    assert!(k.focused().is_some());
    k.key(Key::F10);
    assert_eq!(k.focused(), None);
}

#[test]
fn key_tips_open_a_tab_then_run_its_tool_as_each_system_does() {
    let style = tip_style(crate::platform::system());
    let mut k = Keys::new();
    k.alt_tap();
    if style != TipStyle::Office {
        // Linux shows the letters only while Alt is held; the Mac has its menu bar.
        assert_eq!(k.app.x.keys.tips, Tips::Off);
    } else {
        assert_eq!(k.app.x.keys.tips, Tips::Tabs, "Alt shows the tabs' key tips");
        let view = k.tip_of(Scope::TabRow, t("View"));
        k.type_tip(&view);
        assert_eq!(k.app.x.tab, RibbonTab::View);
        assert_eq!(k.app.x.keys.tips, Tips::Band, "the tab's tools show theirs");
        k.reader_follows();
        // Esc goes back a level at a time.
        k.key(Key::Escape);
        assert_eq!(k.app.x.keys.tips, Tips::Tabs);
        k.key(Key::Escape);
        assert_eq!(k.app.x.keys.tips, Tips::Off);
        // F10 does what Alt does; Alt again hides them.
        k.key(Key::F10);
        assert_eq!(k.app.x.keys.tips, Tips::Tabs);
        k.alt_tap();
        assert_eq!(k.app.x.keys.tips, Tips::Off);
        // A tool's tip runs it: Recent pictures off.
        k.key(Key::F10);
        k.type_tip(&view);
        assert!(k.app.settings.show_tray);
        let recent = k.tip_of(Scope::Band, t("Recent pictures"));
        k.type_tip(&recent);
        assert!(!k.app.settings.show_tray, "the checkbox was ticked off by its key tip");
        assert_eq!(k.app.x.keys.tips, Tips::Off);
    }
    // Alt+letter opens a tab at once on Windows and Linux.
    k.app.switch_tab(&k.ctx.clone(), RibbonTab::Home);
    k.frame(Vec::new());
    let edit = k.tip_of(Scope::TabRow, t("Edit"));
    k.alt_and(Key::from_name(&edit).unwrap());
    if style == TipStyle::Off {
        assert_eq!(k.app.x.tab, RibbonTab::Home, "Option+letter types on a Mac");
    } else {
        assert_eq!(k.app.x.tab, RibbonTab::Edit);
        assert_eq!(k.app.x.keys.tips, Tips::Band);
    }
}

#[test]
fn a_tab_letter_does_not_also_choose_a_tool() {
    if tip_style(crate::platform::system()) != TipStyle::Office {
        return;
    }
    let mut k = Keys::new();
    let before = k.app.tool;
    k.alt_tap();
    // Home's letter is H, which is also the highlighter's key.
    let home = k.tip_of(Scope::TabRow, t("Home"));
    k.type_tip(&home);
    assert_eq!(k.app.tool, before, "the key went to the key tip only");
    assert_eq!(k.app.x.keys.tips, Tips::Band);
    // A key that is no tip leaves them be; a click hides them.
    k.frame(vec![Event::PointerButton { pos: pos2(700.0, 600.0), button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
    assert_eq!(k.app.x.keys.tips, Tips::Off);
}

#[test]
fn every_tab_has_a_key_tip_of_its_own_in_every_language() {
    // The tab row's tips, worked out from the tabs' names in each language.
    const TABS: [&str; 11] = ["File", "Home", "Capture", "Record", "Studio", "Stream", "Edit", "Image", "Share", "View", "Help"];
    assert_eq!(TABS.len(), RibbonTab::ALL.len() + 1, "a new tab belongs here too");
    for code in crate::i18n::CODES {
        let names: Vec<&str> = TABS.iter().map(|s| crate::i18n::t_in(code, s)).collect();
        let tips = assign(&names);
        assert_eq!(tips, assign(&names), "{code}: the same every time");
        let set: std::collections::HashSet<&String> = tips.iter().collect();
        assert_eq!(set.len(), tips.len(), "{code}: {tips:?}");
        assert!(tips.iter().all(|t| t.len() == 1 && t.chars().all(|c| c.is_ascii_alphanumeric())), "{code}: {tips:?}");
    }
}

#[test]
fn arrows_in_the_options_panel_stay_in_it() {
    let mut k = Keys::new();
    for _ in 0..4 {
        k.key(Key::F6);
    }
    assert_eq!(k.pane(), Some(Pane::Options));
    for key in [Key::ArrowLeft, Key::ArrowUp, Key::ArrowRight, Key::ArrowLeft] {
        k.key(key);
        k.frame(Vec::new());
        assert_eq!(k.pane(), Some(Pane::Options), "{key:?} took the keyboard out of the options");
    }
    for _ in 0..20 {
        k.key(Key::ArrowDown);
        k.frame(Vec::new());
        assert_eq!(k.pane(), Some(Pane::Options));
    }
}

#[test]
fn a_control_in_a_floating_window_keeps_the_picture_keys_off() {
    let mut k = Keys::new();
    k.app.x.lang_open = true;
    k.frame(Vec::new());
    // Tab along until a control of the language list has the keyboard.
    let floating = |k: &Keys| k.focused().is_some_and(|f| k.ctx.read_response(f).is_some_and(|r| r.layer_id.order != egui::Order::Background));
    for _ in 0..400 {
        if floating(&k) && !k.ctx.text_edit_focused() {
            break;
        }
        k.key(Key::Tab);
    }
    assert!(floating(&k) && !k.ctx.text_edit_focused(), "Tab never reached a control in the language list");
    assert_eq!(k.pane(), None, "a window over the panes is in none of them");
    let before = k.app.tool;
    let other = Tool::ALL.into_iter().find(|t| *t != before && *t != Tool::Select).unwrap();
    k.key(Key::from_name(other.key()).unwrap());
    assert_eq!(k.app.tool, before, "a letter in the language list doesn't choose a tool");
}

/// The options controls Down reaches from the first, in a window this tall.
fn options_reached(height: f32) -> (Keys, Vec<Id>) {
    let mut k = Keys::sized(vec2(1400.0, height));
    for _ in 0..4 {
        k.key(Key::F6);
    }
    assert_eq!(k.pane(), Some(Pane::Options));
    let mut seen = vec![k.focused().unwrap()];
    for _ in 0..40 {
        k.key(Key::ArrowDown);
        k.frame(Vec::new());
        assert_eq!(k.pane(), Some(Pane::Options));
        let f = k.focused().unwrap();
        if !seen.contains(&f) {
            seen.push(f);
        }
    }
    (k, seen)
}

#[test]
fn in_a_short_window_the_arrows_reach_the_options_below_the_fold() {
    let (_, tall) = options_reached(900.0);
    let (k, short) = options_reached(420.0);
    assert!(tall.len() > 6, "{} controls", tall.len());
    assert_eq!(short, tall, "in a short window Down reaches the same controls, scrolling them into view");
    // The control the keyboard is on was scrolled into view.
    let r = k.ctx.read_response(k.focused().unwrap()).unwrap();
    let pane = k.app.x.keys.panes.iter().find(|p| p.0 == Pane::Options).unwrap().1;
    assert!(pane.intersects(r.rect), "{:?} is out of the panel {pane:?}", r.rect);
}



/// Bug: while drawing, Ctrl+Z / ⌘Z undid the picture (or brought back a deleted file) instead
/// of the last stroke, and on Wayland never reached the ink at all.
#[test]
fn while_drawing_ctrl_z_takes_back_ink_not_the_picture() {
    let _one = crate::ink::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut k = Keys::new();
    let ci = k.app.current.unwrap();
    k.app.apply_effect(crate::effects::Effect::Invert);
    let steps = k.app.entries[ci].undo.len();
    assert!(steps > 0, "an edit to take back");
    let now = std::time::Instant::now();
    crate::ink::with(|i| {
        i.clear();
        i.set_on(true, now);
        for x in [1.0, 2.0] {
            i.begin(crate::ink::Space::Canvas, [x, 1.0], now);
            i.end(now);
        }
    });
    k.press(Key::Z, Modifiers::COMMAND);
    let left = crate::ink::with(|i| i.snapshot(std::time::Instant::now()).strokes.len());
    assert_eq!(k.app.entries[ci].undo.len(), steps, "the picture's edit stays");
    assert_eq!(left, 1, "one stroke taken back");
    k.key(Key::Escape);
    assert!(crate::ink::with(|i| !i.on), "Esc stops drawing");
    k.press(Key::Z, Modifiers::COMMAND);
    assert_eq!(k.app.entries[ci].undo.len(), steps - 1, "not drawing: Ctrl+Z is the picture's again");
    crate::ink::with(|i| i.clear());
}

/// Ctrl+= / Ctrl+- / Ctrl+0 (⌘ on a Mac) zoom the picture, as in other editors. They used to
/// scale the whole interface instead (egui's own zoom keys), and the picture's zoom needed the
/// bare keys.
#[test]
fn ctrl_plus_minus_and_zero_zoom_the_picture_not_the_interface() {
    let mut k = Keys::new();
    assert_eq!(k.app.zoom, None, "fitted at first");
    k.press(Key::Equals, Modifiers::COMMAND);
    let bigger = k.app.zoom.expect("Ctrl+= zooms in");
    assert_eq!(k.ctx.zoom_factor(), 1.0, "the interface keeps its size");
    k.press(Key::Plus, Modifiers::COMMAND);
    assert!(k.app.zoom.unwrap() > bigger, "Ctrl++ (with Shift on most keyboards) zooms in too");
    let now = k.app.zoom.unwrap();
    k.press(Key::Minus, Modifiers::COMMAND);
    assert!(k.app.zoom.unwrap() < now, "Ctrl+- zooms out");
    k.press(Key::Num0, Modifiers::COMMAND);
    assert_eq!(k.app.zoom, None, "Ctrl+0 fits the picture to the window again");
    assert_eq!(k.ctx.zoom_factor(), 1.0);
    // The bare keys still work.
    k.key(Key::Minus);
    assert!(k.app.zoom.is_some());
    k.key(Key::Num0);
    assert_eq!(k.app.zoom, None);
}

/// The window's title names the open picture, and follows when another is picked or it closes.
#[test]
fn the_window_title_names_the_open_picture() {
    let mut k = Keys::new();
    let mac = crate::platform::system() == crate::platform::System::Mac;
    assert_eq!(k.app.title_sent, if mac { "Test".to_string() } else { "Test \u{2013} Snagster".to_string() });
    k.app.add_entry("Picture 2".into(), Doc::new(RgbaImage::from_pixel(10, 10, image::Rgba([0, 0, 0, 255]))));
    k.frame(Vec::new());
    assert!(k.app.title_sent.starts_with("Picture 2"), "{}", k.app.title_sent);
    // The capture panel: the window is about capturing, not the picture.
    k.app.panel_open = true;
    k.frame(Vec::new());
    assert_eq!(k.app.title_sent, "Snagster");
    k.app.panel_open = false;
    k.frame(Vec::new());
    assert!(k.app.title_sent.starts_with("Picture 2"), "back on the picture");
    // A recording (its bar up, the window minimised): the same.
    k.app.mini = Some(Mini::Gif);
    k.frame(Vec::new());
    assert_eq!(k.app.title_sent, "Snagster");
    k.app.mini = None;
    k.frame(Vec::new());
    assert!(k.app.title_sent.starts_with("Picture 2"));
    k.app.entries.clear();
    k.app.current = None;
    k.frame(Vec::new());
    assert_eq!(k.app.title_sent, "Snagster", "no picture open");
}

/// The first close to the tray says so, once: Enter (Keep running) and Esc both keep Snagster
/// running in the tray; neither turns the setting off.
#[test]
fn the_still_running_notice_keeps_snagster_in_the_tray_on_enter_and_esc() {
    for key in [Key::Enter, Key::Escape] {
        let mut k = Keys::new();
        let before = k.app.settings.close_to_tray;
        k.app.dialog = Some(crate::windows::Dialog::StillRunning);
        k.frame(Vec::new());
        k.frame(Vec::new());
        k.key(key);
        k.frame(Vec::new());
        assert!(k.app.dialog.is_none(), "{key:?}: the notice closed");
        assert!(k.app.in_tray || crate::capture::desktop_screenshots_only(), "{key:?}: Snagster went to the tray");
        assert_eq!(k.app.settings.close_to_tray, before, "{key:?}: the setting is unchanged");
    }
}

/// The close button with Snagster kept in the tray: the first close shows the notice, a close
/// while it is up counts as Keep running, later closes go straight to the tray, and another
/// open dialog is never thrown away for it.
#[test]
fn closing_to_the_tray_shows_the_notice_once_and_keeps_other_dialogs() {
    use crate::windows::Dialog;
    let mut k = Keys::new();
    let ctx = k.ctx.clone();
    k.app.settings.close_hint_seen = false;
    // Another dialog is open: it stays, the window goes to the tray, the notice waits.
    k.app.dialog = Some(Dialog::new_image());
    k.app.on_close(&ctx, true);
    assert!(matches!(k.app.dialog, Some(Dialog::NewImage { .. })), "the half-filled dialog is kept");
    assert!(!k.app.settings.close_hint_seen);
    k.app.dialog = None;
    k.app.in_tray = false;
    // The first close: the notice, nothing hidden yet.
    k.app.on_close(&ctx, true);
    assert!(matches!(k.app.dialog, Some(Dialog::StillRunning)));
    assert!(k.app.settings.close_hint_seen && !k.app.in_tray);
    // Closed again while it is up: Keep running.
    k.app.on_close(&ctx, true);
    assert!(k.app.dialog.is_none());
    assert!(k.app.in_tray || crate::capture::desktop_screenshots_only());
    // Later closes: straight to the tray, no notice.
    k.app.in_tray = false;
    k.app.on_close(&ctx, true);
    assert!(k.app.dialog.is_none());
}

/// Always exit (the notice's second choice, reached from the keyboard) turns close-to-tray off
/// and closes the notice. Its place in the row depends on the system's button order, so each
/// Tab position is tried in a fresh window.
#[test]
fn always_exit_turns_closing_to_the_tray_off() {
    let mut found = false;
    for tabs in 1..=3 {
        let mut k = Keys::new();
        k.app.settings.close_to_tray = true;
        k.app.dialog = Some(crate::windows::Dialog::StillRunning);
        k.frame(Vec::new());
        k.frame(Vec::new());
        for _ in 0..tabs {
            k.key(Key::Tab);
        }
        k.key(Key::Space);
        k.frame(Vec::new());
        if !k.app.settings.close_to_tray {
            assert!(k.app.dialog.is_none(), "the notice closed");
            found = true;
            break;
        }
    }
    assert!(found, "no Tab position reached Always exit");
}
