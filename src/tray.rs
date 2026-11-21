//! Snagster's icon in the notification area (bottom-right corner of the taskbar): a left click
//! opens the capture panel, a right click opens a quick menu. On a Mac it is a menu bar extra:
//! a one-colour template picture that macOS tints for light and dark menu bars, and a click
//! opens the menu, as Apple's guidelines ask.

use crate::settings::{Action, Settings};
use std::sync::mpsc::Sender;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Clone, Debug, PartialEq)]
pub enum TrayCmd {
    Action(Action),
    Preset(usize),
    TogglePanel,
    Panel,
    Editor,
    Library,
    Settings,
    Quit,
    /// Stop the recording that is running (the menu shows it only then).
    StopRecording,
    /// An item of the Mac menu bar (`crate::menubar`): its menus and the tray's share muda's
    /// one event handler.
    #[cfg(any(target_os = "macos", test))]
    Menu(crate::menubar::Run),
}

pub struct Tray {
    icon: TrayIcon,
    menu: Menu,
    /// "Stop recording" and its separator, at the top of the menu while a recording runs.
    stop: (MenuItem, PredefinedMenuItem),
    recording: bool,
    replay: CheckMenuItem,
    save_replay: MenuItem,
}

impl Tray {
    /// Show whether a recording runs: "Stop recording" at the top of the menu and, on a Mac,
    /// a stop square in the menu bar instead of the viewfinder. Cheap when nothing changed.
    pub fn show_recording(&mut self, on: bool) {
        if self.recording == on {
            return;
        }
        self.recording = on;
        let (item, sep) = &self.stop;
        if on {
            let _ = self.menu.prepend_items(&[item, sep]);
        } else {
            let _ = self.menu.remove(item);
            let _ = self.menu.remove(sep);
        }
        #[cfg(target_os = "macos")]
        {
            let (rgba, size) = if on { stop_icon(TEMPLATE_PX) } else { template_icon(TEMPLATE_PX) };
            if let Ok(ic) = tray_icon::Icon::from_rgba(rgba, size, size) {
                let _ = self.icon.set_icon_templated(Some(ic));
            }
        }
        // Back from the stop square: the replay buffer's dot, when it is on.
        if !on && self.replay.is_checked() {
            self.set_replay(true, self.save_replay.is_enabled());
        }
        #[cfg(not(target_os = "macos"))]
        let _ = &self.icon;
    }
}

const ACTIONS: [Action; 14] = [
    Action::AllInOne,
    Action::Area,
    Action::Window,
    Action::Screen,
    Action::AllScreens,
    Action::Scrolling,
    Action::ManualScroll,
    Action::MultiArea,
    Action::Circle,
    Action::Freehand,
    Action::GrabText,
    Action::ColorPicker,
    Action::RecordGif,
    Action::RecordSteps,
];

fn parse(id: &str) -> Option<TrayCmd> {
    #[cfg(any(target_os = "macos", test))]
    if id.starts_with("mb:") {
        return crate::menubar::parse(id).map(TrayCmd::Menu);
    }
    if let Some(rest) = id.strip_prefix("a:") {
        return Action::ALL.iter().find(|a| format!("{a:?}") == rest).map(|a| TrayCmd::Action(*a));
    }
    if let Some(rest) = id.strip_prefix("p:") {
        return rest.parse().ok().map(TrayCmd::Preset);
    }
    Some(match id {
        "panel" => TrayCmd::Panel,
        "editor" => TrayCmd::Editor,
        "library" => TrayCmd::Library,
        "settings" => TrayCmd::Settings,
        "quit" => TrayCmd::Quit,
        "stop" => TrayCmd::StopRecording,
        _ => return None,
    })
}

/// Put the icon in the notification area. Commands go to `tx`; `wake` makes Snagster act on
/// them even while its window is hidden.
pub fn create(s: &Settings, tx: Sender<TrayCmd>, wake: impl Fn() + Send + Sync + Clone + 'static) -> Result<Tray, String> {
    let menu = Menu::new();
    let add = |m: &Menu, id: &str, text: &str| m.append(&MenuItem::with_id(id, label(text), true, None)).map_err(|e| e.to_string());
    add(&menu, "panel", "Capture panel…")?;
    menu.append(&PredefinedMenuItem::separator()).map_err(|e| e.to_string())?;
    for a in ACTIONS {
        let combo = s.binding(a);
        // A Mac shows the hotkey as the item's own shortcut, right-aligned; elsewhere after a tab.
        #[cfg(target_os = "macos")]
        {
            let keys = if combo.is_set() { crate::menubar::hotkey_accelerator(combo.ctrl, combo.alt, combo.shift, combo.win, &combo.key) } else { None };
            let item = tray_icon::menu::MenuItemBuilder::new()
                .id(tray_icon::menu::MenuId::new(format!("a:{a:?}")))
                .text(label(a.label()))
                .key_accelerator(keys)
                .map_err(|e| e.to_string())?
                .build();
            menu.append(&item).map_err(|e| e.to_string())?;
        }
        #[cfg(not(target_os = "macos"))]
        {
            // The name translated before the key is added (`add` translates the whole text, which
            // with a key after it is never in the table).
            let text = if combo.is_set() { format!("{}\t{}", crate::i18n::tr(a.label()), combo.label()) } else { a.label().to_string() };
            add(&menu, &format!("a:{a:?}"), &text)?;
        }
    }
    // The replay buffer: on / off (ticked while on) and Save replay (only while on).
    menu.append(&PredefinedMenuItem::separator()).map_err(|e| e.to_string())?;
    let replay = CheckMenuItem::with_id(format!("a:{:?}", Action::ToggleReplay), label("Replay"), true, false, None);
    menu.append(&replay).map_err(|e| e.to_string())?;
    let combo = s.binding(Action::SaveReplay);
    let text = if combo.is_set() && !cfg!(target_os = "macos") { format!("{}\t{}", label("Save replay"), combo.label()) } else { label("Save replay") };
    let save_replay = MenuItem::with_id(format!("a:{:?}", Action::SaveReplay), text, false, None);
    menu.append(&save_replay).map_err(|e| e.to_string())?;
    if !s.presets.is_empty() {
        let sub = Submenu::new(label("Presets"), true);
        for (i, p) in s.presets.iter().enumerate() {
            sub.append(&MenuItem::with_id(format!("p:{i}"), &p.name, true, None)).map_err(|e| e.to_string())?;
        }
        menu.append(&sub).map_err(|e| e.to_string())?;
    }
    menu.append(&PredefinedMenuItem::separator()).map_err(|e| e.to_string())?;
    add(&menu, "editor", "Open editor")?;
    add(&menu, "library", "Library")?;
    // "Settings…" in a Mac's menus, as in the app menu.
    add(&menu, "settings", if cfg!(target_os = "macos") { "Settings…" } else { "Settings" })?;
    menu.append(&PredefinedMenuItem::separator()).map_err(|e| e.to_string())?;
    // "Exit" in the Windows notification area, "Quit Snagster" elsewhere.
    add(&menu, "quit", if crate::platform::recycle_bin() { "Exit" } else { "Quit Snagster" })?;

    // A Mac's menu bar extra is a template: black and transparency only, which macOS tints to
    // match the menu bar. Elsewhere the coloured app icon.
    let mac = cfg!(target_os = "macos");
    let stop = (MenuItem::with_id("stop", label("Stop recording"), true, None), PredefinedMenuItem::separator());
    let builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu.clone()))
        // Just the name: the guidelines keep instructions out of tray tooltips.
        .with_tooltip("Snagster")
        // A menu bar extra opens its menu on a click; the capture panel is in the menu.
        .with_menu_on_left_click(mac);
    #[cfg(target_os = "macos")]
    let builder = {
        let (rgba, size) = template_icon(TEMPLATE_PX);
        builder.with_icon_templated(tray_icon::Icon::from_rgba(rgba, size, size).map_err(|e| e.to_string())?)
    };
    #[cfg(not(target_os = "macos"))]
    let builder = {
        let ic = crate::icon();
        builder.with_icon(tray_icon::Icon::from_rgba(ic.rgba, ic.width, ic.height).map_err(|e| e.to_string())?)
    };
    let tray = builder.build().map_err(|e| e.to_string())?;

    listen(tx.clone(), wake.clone());
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e
            && !mac
        {
            let _ = tx.send(TrayCmd::TogglePanel);
            wake();
        }
    }));
    Ok(Tray { icon: tray, menu, stop, recording: false, replay, save_replay })
}

impl Tray {
    /// Show whether the replay buffer is on: the item ticked, Save replay usable, a dot on the
    /// icon (the accent colour, not red: red means recording) and the tooltip saying so. Save
    /// replay works once it is `ready` (not while it is still starting).
    pub fn set_replay(&self, on: bool, ready: bool) {
        self.replay.set_checked(on);
        self.save_replay.set_enabled(ready);
        let tip = if on { format!("Snagster – {}", crate::i18n::tr("Replay on")) } else { "Snagster".to_string() };
        let _ = self.icon.set_tooltip(Some(tip));
        // While a recording runs, a Mac's menu bar keeps the stop square (`show_recording`).
        #[cfg(target_os = "macos")]
        if !self.recording {
            let (mut rgba, size) = template_icon(TEMPLATE_PX);
            if on {
                add_dot(&mut rgba, size, size, [0, 0, 0, 255]);
            }
            if let Ok(i) = tray_icon::Icon::from_rgba(rgba, size, size) {
                let _ = self.icon.set_icon_templated(Some(i));
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let ic = crate::icon();
            let mut rgba = ic.rgba;
            if on {
                let a = crate::look::accent();
                add_dot(&mut rgba, ic.width, ic.height, [a.r(), a.g(), a.b(), 255]);
            }
            if let Ok(i) = tray_icon::Icon::from_rgba(rgba, ic.width, ic.height) {
                let _ = self.icon.set_icon(Some(i));
            }
        }
    }
}

/// A dot in the bottom-right corner of an icon (a third of its size), with a ring of
/// transparency around it so it stands out on any taskbar.
pub fn add_dot(rgba: &mut [u8], w: u32, h: u32, color: [u8; 4]) {
    let d = w.min(h) as f32 / 3.0;
    let (cx, cy, r) = (w as f32 - d / 2.0 - 0.5, h as f32 - d / 2.0 - 0.5, d / 2.0);
    let ring = r + (w.min(h) as f32 / 16.0).max(1.0);
    for y in 0..h {
        for x in 0..w {
            let dist = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            let i = ((y * w + x) * 4) as usize;
            if dist <= r {
                rgba[i..i + 4].copy_from_slice(&color);
            } else if dist <= ring {
                rgba[i + 3] = 0;
            }
        }
    }
}

/// A menu text: translated, and on a Mac in Title Case for English.
fn label(en: &str) -> String {
    #[cfg(target_os = "macos")]
    return crate::menubar::shown(en, crate::i18n::current());
    #[cfg(not(target_os = "macos"))]
    crate::i18n::tr(en)
}

/// Send the items of every muda menu (the tray's and the Mac menu bar's) to `tx`. muda has one
/// event handler for the whole program, so this is the only place that sets it.
pub fn listen(tx: Sender<TrayCmd>, wake: impl Fn() + Send + Sync + 'static) {
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if let Some(cmd) = parse(e.id.as_ref()) {
            let _ = tx.send(cmd);
            wake();
        }
    }));
}

/// The menu bar extra's size in pixels: 18 points on a Retina screen, the height Apple gives
/// menu bar icons.
#[cfg(any(target_os = "macos", test))]
const TEMPLATE_PX: u32 = 36;

/// The menu bar extra's template picture, `px` square, straight RGBA: a viewfinder (four
/// corners and a dot in the middle) in black, the rest transparent. macOS uses only its
/// transparency and paints it in the menu bar's own colour.
#[cfg(any(target_os = "macos", test))]
pub fn template_icon(px: u32) -> (Vec<u8>, u32) {
    use tiny_skia::{Color, FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};
    let mut pm = Pixmap::new(px, px).expect("icon size");
    let s = px as f32 / 36.0;
    let mut paint = Paint::default();
    paint.set_color(Color::BLACK);
    paint.anti_alias = true;
    let (lo, hi, arm) = (4.0 * s, 32.0 * s, 9.0 * s);
    let mut pb = PathBuilder::new();
    for (x, y, dx, dy) in [(lo, lo, 1.0, 1.0), (hi, lo, -1.0, 1.0), (lo, hi, 1.0, -1.0), (hi, hi, -1.0, -1.0)] {
        pb.move_to(x + dx * arm, y);
        pb.line_to(x, y);
        pb.line_to(x, y + dy * arm);
    }
    if let Some(path) = pb.finish() {
        let stroke = Stroke { width: 3.0 * s, line_cap: LineCap::Round, ..Stroke::default() };
        pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
    if let Some(dot) = PathBuilder::from_circle(18.0 * s, 18.0 * s, 5.0 * s) {
        pm.fill_path(&dot, &paint, FillRule::Winding, Transform::identity(), None);
    }
    // Black stays black when the premultiplied colour is undone: only the alpha changes.
    (pm.data().to_vec(), px)
}

/// The menu bar extra's template picture while recording, `px` square: a stop square with
/// round corners in black, the rest transparent (macOS tints it like `template_icon`).
#[cfg(any(target_os = "macos", test))]
pub fn stop_icon(px: u32) -> (Vec<u8>, u32) {
    use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Transform};
    let mut pm = Pixmap::new(px, px).expect("icon size");
    let s = px as f32 / 36.0;
    let mut paint = Paint::default();
    paint.set_color(Color::BLACK);
    paint.anti_alias = true;
    // A square 22 of 36 wide with round corners, in the middle.
    let (lo, hi, r) = (7.0 * s, 29.0 * s, 4.0 * s);
    let mut pb = PathBuilder::new();
    pb.move_to(lo + r, lo);
    pb.line_to(hi - r, lo);
    pb.quad_to(hi, lo, hi, lo + r);
    pb.line_to(hi, hi - r);
    pb.quad_to(hi, hi, hi - r, hi);
    pb.line_to(lo + r, hi);
    pb.quad_to(lo, hi, lo, hi - r);
    pb.line_to(lo, lo + r);
    pb.quad_to(lo, lo, lo + r, lo);
    pb.close();
    if let Some(path) = pb.finish() {
        pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
    (pm.data().to_vec(), px)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_action_id_reads_back() {
        // The ids `create` gives the menu items must parse to the same action.
        for a in ACTIONS {
            assert_eq!(parse(&format!("a:{a:?}")), Some(TrayCmd::Action(a)));
        }
        for a in Action::ALL {
            assert_eq!(parse(&format!("a:{a:?}")), Some(TrayCmd::Action(a)));
        }
    }

    #[test]
    fn menu_actions_are_real_and_listed_once() {
        for (i, a) in ACTIONS.iter().enumerate() {
            assert!(Action::ALL.contains(a), "{a:?}");
            assert!(!ACTIONS[i + 1..].contains(a), "{a:?} twice");
        }
    }

    #[test]
    fn preset_ids_read_back() {
        assert_eq!(parse("p:0"), Some(TrayCmd::Preset(0)));
        assert_eq!(parse("p:12"), Some(TrayCmd::Preset(12)));
        assert_eq!(parse("p:"), None);
        assert_eq!(parse("p:-1"), None);
        assert_eq!(parse("p:two"), None);
    }

    #[test]
    fn fixed_items_read_back() {
        assert_eq!(parse("panel"), Some(TrayCmd::Panel));
        assert_eq!(parse("editor"), Some(TrayCmd::Editor));
        assert_eq!(parse("library"), Some(TrayCmd::Library));
        assert_eq!(parse("settings"), Some(TrayCmd::Settings));
        assert_eq!(parse("quit"), Some(TrayCmd::Quit));
        assert_eq!(parse("stop"), Some(TrayCmd::StopRecording));
    }

    #[test]
    fn the_recording_icon_is_a_stop_square_template() {
        let (rgba, px) = stop_icon(TEMPLATE_PX);
        assert_eq!(rgba.len(), (px * px * 4) as usize);
        let alpha = |x: u32, y: u32| rgba[((y * px + x) * 4 + 3) as usize];
        for p in rgba.chunks(4) {
            assert_eq!(&p[..3], &[0, 0, 0], "one colour: black, shaped by transparency");
        }
        assert_eq!(alpha(px / 2, px / 2), 255, "the middle is filled (a square, not a ring)");
        assert_eq!(alpha(0, 0), 0, "a glyph, not a filled tile");
        assert_eq!(alpha(px / 2, 1), 0, "a margin above the square");
        assert_ne!(rgba, template_icon(TEMPLATE_PX).0, "not the viewfinder");
    }

    #[test]
    fn menu_bar_items_go_to_the_window() {
        use crate::menubar::Run;
        assert_eq!(parse("mb:quit"), Some(TrayCmd::Menu(Run::Quit)));
        assert_eq!(parse("mb:settings"), Some(TrayCmd::Menu(Run::Settings)));
        assert_eq!(parse("mb:cmd:save"), Some(TrayCmd::Menu(Run::Command("save"))));
        assert_eq!(parse("mb:cmd:nothing_like_it"), None);
        // The tray's own ids are not the menu bar's.
        assert_eq!(parse("quit"), Some(TrayCmd::Quit));
    }

    #[test]
    fn the_menu_bar_icon_is_a_template() {
        let (rgba, px) = template_icon(TEMPLATE_PX);
        assert_eq!(px, 36);
        assert_eq!(rgba.len(), (px * px * 4) as usize);
        let mut opaque = 0;
        let mut clear = 0;
        for p in rgba.chunks(4) {
            assert_eq!(&p[..3], &[0, 0, 0], "one colour: black, shaped by transparency");
            match p[3] {
                255 => opaque += 1,
                0 => clear += 1,
                _ => {}
            }
        }
        assert!(opaque > 100, "the glyph is drawn ({opaque} solid pixels)");
        assert!(clear > rgba.len() / 4 / 2, "most of it is transparent");
        // The corners of the square are transparent (a glyph, not a filled tile).
        for i in [0, px - 1, px * (px - 1), px * px - 1] {
            assert_eq!(rgba[i as usize * 4 + 3], 0);
        }
        // It scales: the 1x size draws the same shape.
        let (small, sp) = template_icon(18);
        assert_eq!(small.len(), (sp * sp * 4) as usize);
        assert!(small.chunks(4).any(|p| p[3] == 255));
    }

    #[test]
    fn the_replay_dot_sits_in_the_corner_with_a_clear_ring() {
        let mut px = vec![255u8; 48 * 48 * 4];
        add_dot(&mut px, 48, 48, [1, 2, 3, 255]);
        let at = |x: usize, y: usize| px[(y * 48 + x) * 4..(y * 48 + x) * 4 + 4].to_vec();
        assert_eq!(at(40, 40), vec![1, 2, 3, 255]);
        assert_eq!(at(4, 4), vec![255; 4]);
        assert_eq!(at(30, 40)[3], 0);
        // The replay items read back as their hotkey actions.
        assert_eq!(parse("a:ToggleReplay"), Some(TrayCmd::Action(Action::ToggleReplay)));
        assert_eq!(parse("a:SaveReplay"), Some(TrayCmd::Action(Action::SaveReplay)));
    }

    #[test]
    fn unknown_ids_are_ignored() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("Quit"), None);
        assert_eq!(parse("a:"), None);
        assert_eq!(parse("a:NoSuchAction"), None);
        assert_eq!(parse("a:area"), None);
        assert_eq!(parse("x:1"), None);
    }
}
