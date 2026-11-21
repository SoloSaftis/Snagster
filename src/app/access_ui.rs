//! macOS's Screen Recording permission in the window: the system's prompt the first time a
//! capture starts without it, a clear message (and a button to the right page of System
//! Settings) in the status line, and a small window the first time a capture, recording or
//! stream fails because of it. Other systems never get here (`screen_access` is `Granted`).

use super::*;
use crate::capture::ScreenAccess;
use crate::i18n::t;

/// The window's state for the permission.
#[derive(Default)]
pub(crate) struct AccessUi {
    /// A made-up answer for the button test (no real macOS call).
    pub fake: Option<ScreenAccess>,
    /// The system's prompt was asked for (once a session).
    pub asked: bool,
    /// The window explaining it was shown (once a session).
    pub shown: bool,
    /// The window is open, for this answer.
    pub open: Option<ScreenAccess>,
    /// The message that gives the status line an "Open Screen Recording settings" button while
    /// the line contains it.
    pub status_line: Option<String>,
    /// System Settings was opened from the window: macOS applies the permission only after
    /// Snagster starts again, so the window now offers Quit and Reopen.
    pub settings_opened: bool,
}

impl SnagApp {
    /// Whether Snagster may see the screen (the test's made-up answer if there is one).
    pub(crate) fn screen_access(&self) -> ScreenAccess {
        self.x.access.fake.unwrap_or_else(crate::capture::screen_access)
    }

    /// Before the first capture, recording or stream: without the permission, macOS's own
    /// prompt (shown once). The capture still goes ahead; if it fails, `grab_failed` explains.
    pub(crate) fn ask_screen_access_once(&mut self) {
        if self.x.access.asked {
            return;
        }
        if self.screen_access() == ScreenAccess::NotGranted {
            self.x.access.asked = true;
            if self.x.access.fake.is_none() {
                crate::capture::request_screen_access();
            }
        }
    }

    /// A capture, recording or stream failed with `raw`: the message for the status line
    /// (put into `template`'s `{error}`). Without screen access it says what to do, the status
    /// line gets a button to System Settings, and the first time a window explains it.
    pub(crate) fn grab_failed(&mut self, template: &'static str, raw: &str) -> String {
        let access = self.screen_access();
        let line = crate::i18n::tf(template, &[("error", &crate::capture::grab_error_message(raw, access))]);
        if access.granted() {
            self.x.access.status_line = None;
        } else {
            self.x.access.status_line = Some(line.clone());
            if !self.x.access.shown {
                self.x.access.shown = true;
                self.x.access.open = Some(access);
            }
        }
        line
    }

    /// The button under the status line while it says the permission is missing.
    pub(crate) fn screen_access_link(&mut self, ui: &mut egui::Ui) {
        if !self.x.access.status_line.as_deref().is_some_and(|l| self.status.contains(l)) {
            return;
        }
        if self.screen_access() == ScreenAccess::NotGranted && ui.link(t("Open Screen Recording settings")).clicked() {
            self.open_screen_settings();
        }
    }

    fn open_screen_settings(&mut self) {
        if let Err(e) = crate::capture::open_screen_settings() {
            self.status = crate::i18n::tf("Couldn't open it: {error}", &[("error", &e)]);
        }
    }

    /// The window the first time screen access stops a capture. Once System Settings is open
    /// it offers Quit and Reopen, as macOS's own prompt does: the permission only counts for a
    /// Snagster started after it was given.
    pub(crate) fn screen_access_window(&mut self, ctx: &egui::Context) {
        let Some(access) = self.x.access.open else { return };
        let (mut settings, mut close, mut reopen) = (false, false, false);
        let mut open = true;
        let after = self.x.access.settings_opened;
        crate::windows::own_window(ctx, crate::windows::Win { key: "screen-access", title: t("Screen Recording permission"), size: vec2(420.0, 200.0), resizable: false }, &mut open, |ui| {
                ui.set_max_width(380.0);
                if after {
                    ui.label(t("After you allow Snagster in System Settings, it has to start again to see the screen."));
                } else {
                    ui.label(crate::capture::grab_error_message("", access));
                }
                ui.add_space(6.0);
                let (action, cancel) = if after {
                    (Some(t("Quit and Reopen")), t("Later"))
                } else {
                    ((access == ScreenAccess::NotGranted).then(|| t("Open Screen Recording settings")), t("Close"))
                };
                match crate::platform::dialog_buttons(ui, action, cancel, crate::platform::Kind::Normal, true) {
                    Some(crate::platform::Choice::Action) if after => reopen = true,
                    Some(crate::platform::Choice::Action) => settings = true,
                    Some(crate::platform::Choice::Cancel) => close = true,
                    None => {}
                }
            });
        if settings {
            // The window stays, now with Quit and Reopen.
            self.open_screen_settings();
            self.x.access.settings_opened = true;
        }
        if reopen {
            self.quit_and_reopen(ctx);
            close = true;
        }
        if close || !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.x.access.open = None;
            self.x.access.settings_opened = false;
        }
    }

    /// Start Snagster again once this one has quit (macOS applies Screen Recording then).
    fn quit_and_reopen(&mut self, ctx: &egui::Context) {
        if self.x.access.fake.is_none() {
            let Ok(exe) = std::env::current_exe() else { return };
            let cmd = reopen_command(std::process::id(), &exe);
            if let Err(e) = std::process::Command::new(&cmd[0]).args(&cmd[1..]).spawn() {
                self.status = crate::i18n::tf("Couldn't open it: {error}", &[("error", &e.to_string())]);
                return;
            }
        }
        self.quitting = true;
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
    }
}

/// The command that starts Snagster again after process `pid` has ended: a small shell waits
/// for it (one Snagster at a time: a new one would only hand over to this one), then opens the
/// app bundle `exe` is in, or `exe` itself outside a bundle.
fn reopen_command(pid: u32, exe: &std::path::Path) -> Vec<String> {
    let bundle = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app"));
    let open = match bundle {
        Some(app) => format!("open -n \"{}\"", app.display()),
        None => format!("\"{}\" &", exe.display()),
    };
    vec!["/bin/sh".into(), "-c".into(), format!("while kill -0 {pid} 2>/dev/null; do sleep 0.2; done; {open}")]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{NO_DESKTOP, SCREEN_ACCESS_NEEDED};

    #[test]
    fn reopening_waits_for_this_snagster_and_opens_its_app_bundle() {
        let exe = std::path::Path::new("/Applications/Snagster.app/Contents/MacOS/snagster");
        let cmd = reopen_command(4242, exe);
        assert_eq!(&cmd[..2], ["/bin/sh", "-c"]);
        assert!(cmd[2].starts_with("while kill -0 4242 2>/dev/null; do sleep 0.2; done;"), "{}", cmd[2]);
        assert!(cmd[2].ends_with("open -n \"/Applications/Snagster.app\""), "{}", cmd[2]);
        // Outside a bundle (a build run from the terminal) the program itself starts again.
        let dev = reopen_command(7, std::path::Path::new("/Users/me/snag/target/debug/snagster"));
        assert!(dev[2].ends_with("\"/Users/me/snag/target/debug/snagster\" &"), "{}", dev[2]);
    }

    /// Once System Settings was opened, Return chooses Quit and Reopen: Snagster quits (with the
    /// made-up answer nothing is started) and the window goes.
    #[test]
    fn after_opening_system_settings_return_quits_to_reopen() {
        let ctx = egui::Context::default();
        let mut a = SnagApp::blank(Settings::default(), false, None);
        a.x.access.fake = Some(ScreenAccess::NotGranted);
        a.x.access.open = Some(ScreenAccess::NotGranted);
        a.x.access.settings_opened = true;
        let mut out = ctx.run_ui(Default::default(), |_| a.screen_access_window(&ctx));
        out.textures_delta.clear();
        assert!(!a.quitting && a.x.access.open.is_some(), "nothing happens on its own");
        let enter = |pressed| egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE };
        let raw = egui::RawInput { events: vec![enter(true), enter(false)], ..Default::default() };
        let mut out = ctx.run_ui(raw, |_| a.screen_access_window(&ctx));
        out.textures_delta.clear();
        assert!(a.quitting, "Quit and Reopen");
        assert!(a.x.access.open.is_none() && !a.x.access.settings_opened);
    }

    const RAW: &str = "Failed to copy data";

    #[test]
    fn with_access_the_error_stays_and_nothing_opens() {
        let mut a = SnagApp::blank(Settings::default(), false, None);
        a.x.access.fake = Some(ScreenAccess::Granted);
        a.ask_screen_access_once();
        assert!(!a.x.access.asked, "nothing to ask for");
        let line = a.grab_failed("Capture failed: {error}", RAW);
        assert!(line.contains(RAW), "{line}");
        assert!(a.x.access.open.is_none() && a.x.access.status_line.is_none());
    }

    #[test]
    fn without_permission_it_says_what_to_do_and_explains_once() {
        let mut a = SnagApp::blank(Settings::default(), false, None);
        a.x.access.fake = Some(ScreenAccess::NotGranted);
        a.ask_screen_access_once();
        assert!(a.x.access.asked, "the system prompt is asked for");
        let line = a.grab_failed("Capture failed: {error}", RAW);
        assert!(line.contains(crate::i18n::t(SCREEN_ACCESS_NEEDED)) && !line.contains(RAW), "{line}");
        assert_eq!(a.x.access.open, Some(ScreenAccess::NotGranted));
        assert_eq!(a.x.access.status_line.as_deref(), Some(line.as_str()));
        // Closed, then another failure: the status line says it again, the window stays shut.
        a.x.access.open = None;
        let again = a.grab_failed("Video recording failed: {error}", RAW);
        assert!(again.contains(crate::i18n::t(SCREEN_ACCESS_NEEDED)));
        assert!(a.x.access.open.is_none());
        // Once it works again, the status line loses its button.
        a.x.access.fake = Some(ScreenAccess::Granted);
        a.grab_failed("Capture failed: {error}", "that window has closed");
        assert!(a.x.access.status_line.is_none());
    }

    #[test]
    fn without_a_desktop_it_says_so() {
        let mut a = SnagApp::blank(Settings::default(), false, None);
        a.x.access.fake = Some(ScreenAccess::NoDesktop);
        a.ask_screen_access_once();
        assert!(!a.x.access.asked, "no prompt can help without a desktop");
        let line = a.grab_failed("Capture failed: {error}", RAW);
        assert!(line.contains(crate::i18n::t(NO_DESKTOP)), "{line}");
        assert_eq!(a.x.access.open, Some(ScreenAccess::NoDesktop));
    }
}
