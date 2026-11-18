//! The window's side of the Mac menu bar (`crate::menubar`): building it, greying out what
//! can't run, and running its items. Built for macOS, and for the tests on every system.

use super::*;
use crate::menubar::{self, Run};

/// The native menu bar.
#[cfg(target_os = "macos")]
#[derive(Default)]
pub(crate) struct MenuUi {
    bar: Option<menubar::Bar>,
    /// The language the menu bar was last built in (also after a failed try, so a failure
    /// isn't retried every frame).
    built_for: Option<&'static str>,
}

impl SnagApp {
    /// What the menus' items need, from the window's state.
    pub(crate) fn menu_state(&self) -> menubar::State {
        let entry = self.current.map(|i| &self.entries[i]);
        menubar::State {
            picture: entry.is_some(),
            file: entry.is_some_and(|e| e.path.is_some()),
            mark: self.selected.is_some(),
            pictures: !self.entries.is_empty(),
        }
    }

    /// Build the menu bar (the real window only), rebuild it when the language changes, and
    /// grey out what can't run.
    #[cfg(target_os = "macos")]
    pub(crate) fn apply_menu_bar(&mut self) {
        {
            if !self.persist {
                return;
            }
            let lang = crate::i18n::current();
            if self.x.menu.built_for != Some(lang) {
                self.x.menu.built_for = Some(lang);
                // The old menu leaves the app before the new one comes in.
                self.x.menu.bar = None;
                match menubar::Bar::install() {
                    Ok(b) => self.x.menu.bar = Some(b),
                    Err(e) => crate::append_crash_log(&format!("menu bar couldn't be built: {e}\n")),
                }
            }
            let state = self.menu_state();
            if let Some(b) = &mut self.x.menu.bar {
                b.update(state);
            }
        }
    }

    /// Run a menu bar item.
    pub(crate) fn run_menu(&mut self, ctx: &egui::Context, run: Run) {
        if let Run::Key(k) = run {
            // To whichever window has the keyboard, in its next frame.
            menubar::press(menubar::key_events(k, || arboard::Clipboard::new().and_then(|mut c| c.get_text()).unwrap_or_default()));
            ctx.request_repaint();
            return;
        }
        if self.panel_open {
            self.leave_panel(ctx);
        }
        match run {
            Run::Command(id) => self.run_command(ctx, id, None),
            // Settings opens on the pane it showed last (the guidelines' rule), not the first.
            Run::Settings => {
                self.settings_open = true;
                self.unhide(ctx);
            }
            Run::Quit => {
                self.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Run::Key(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_items_run_the_windows_commands() {
        let ctx = egui::Context::default();
        let mut a = SnagApp::blank(Settings::default(), false, None);
        a.settings_tab = SettingsTab::Hotkeys;
        a.run_menu(&ctx, Run::Settings);
        assert!(a.settings_open);
        assert!(a.settings_tab == SettingsTab::Hotkeys, "Settings… reopens the pane shown last");
        a.run_menu(&ctx, Run::Command("library"));
        assert!(a.library_open);
        a.run_menu(&ctx, Run::Command("about"));
        assert!(a.about_open);
        assert!(!a.quitting);
        a.run_menu(&ctx, Run::Quit);
        assert!(a.quitting, "Quit Snagster ends it even when closing would keep it in the menu bar");
    }

    #[test]
    fn the_menus_follow_the_picture() {
        let mut a = SnagApp::blank(Settings::default(), false, None);
        assert_eq!(a.menu_state(), menubar::State::default());
        a.add_entry("one".into(), Doc::new(RgbaImage::new(4, 4)));
        let s = a.menu_state();
        assert!(s.picture && s.pictures && !s.file, "a new picture has no file yet");
    }
}
