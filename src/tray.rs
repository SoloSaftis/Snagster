//! Snagster's icon in the notification area (bottom-right corner of the taskbar): a left click
//! opens the capture panel, a right click opens a quick menu.

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
    let add = |m: &Menu, id: &str, text: &str| m.append(&MenuItem::with_id(id, crate::i18n::tr(text), true, None)).map_err(|e| e.to_string());
    add(&menu, "panel", "Capture panel...")?;
    menu.append(&PredefinedMenuItem::separator()).map_err(|e| e.to_string())?;
    for a in ACTIONS {
        let combo = s.binding(a);
        let text = if combo.is_set() { format!("{}\t{}", a.label(), combo.label()) } else { a.label().to_string() };
        add(&menu, &format!("a:{a:?}"), &text)?;
    }
    if !s.presets.is_empty() {
        let sub = Submenu::new("Presets", true);
        for (i, p) in s.presets.iter().enumerate() {
            sub.append(&MenuItem::with_id(format!("p:{i}"), &p.name, true, None)).map_err(|e| e.to_string())?;
        }
        menu.append(&sub).map_err(|e| e.to_string())?;
    }
    menu.append(&PredefinedMenuItem::separator()).map_err(|e| e.to_string())?;
    add(&menu, "editor", "Open editor")?;
    add(&menu, "library", "Library")?;
    add(&menu, "settings", "Settings")?;
    menu.append(&PredefinedMenuItem::separator()).map_err(|e| e.to_string())?;
    add(&menu, "quit", "Quit Snagster")?;

    let ic = crate::icon();
    let icon = tray_icon::Icon::from_rgba(ic.rgba, ic.width, ic.height).map_err(|e| e.to_string())?;
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Snagster: click to capture, right-click for more")
        .with_icon(icon)
        .with_menu_on_left_click(false)
        .build()
        .map_err(|e| e.to_string())?;

    let (tx2, wake2) = (tx.clone(), wake.clone());
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if let Some(cmd) = parse(e.id.as_ref()) {
            let _ = tx2.send(cmd);
            wake2();
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            let _ = tx.send(TrayCmd::TogglePanel);
            wake();
        }
    }));
    Ok(Tray { _icon: tray })
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
    fn unknown_ids_are_ignored() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("Quit"), None);
        assert_eq!(parse("a:"), None);
        assert_eq!(parse("a:NoSuchAction"), None);
        assert_eq!(parse("a:area"), None);
        assert_eq!(parse("x:1"), None);
    }
}
