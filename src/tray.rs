//! Snagster's icon in the notification area (bottom-right corner of the taskbar): a left click
//! opens the capture panel, a right click opens a quick menu. On a Mac it is a menu bar extra:
//! a one-colour template picture that macOS tints for light and dark menu bars, and a click
//! opens the menu, as Apple's guidelines ask.

use crate::settings::{Action, Settings};
use std::sync::mpsc::Sender;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
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
    /// An item of the Mac menu bar (`crate::menubar`): its menus and the tray's share muda's
    /// one event handler.
    #[cfg(any(target_os = "macos", test))]
    Menu(crate::menubar::Run),
}

pub struct Tray {
    _icon: TrayIcon,
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
            let text = if combo.is_set() { format!("{}\t{}", a.label(), combo.label()) } else { a.label().to_string() };
            add(&menu, &format!("a:{a:?}"), &text)?;
        }
    }
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
    let builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
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
    Ok(Tray { _icon: tray })
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
    fn unknown_ids_are_ignored() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("Quit"), None);
        assert_eq!(parse("a:"), None);
        assert_eq!(parse("a:NoSuchAction"), None);
        assert_eq!(parse("a:area"), None);
        assert_eq!(parse("x:1"), None);
    }
}
