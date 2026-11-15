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

    /// The window the first time screen access stops a capture.
    pub(crate) fn screen_access_window(&mut self, ctx: &egui::Context) {
        let Some(access) = self.x.access.open else { return };
        let (mut settings, mut close) = (false, false);
        let mut open = true;
        crate::windows::own_window(ctx, crate::windows::Win { key: "screen-access", title: t("Screen Recording permission"), size: vec2(420.0, 200.0), resizable: false }, &mut open, |ui| {
                ui.set_max_width(380.0);
                ui.label(crate::capture::grab_error_message("", access));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if access == ScreenAccess::NotGranted
                        && ui.add(egui::Button::new(RichText::new(t("Open Screen Recording settings")).color(Color32::WHITE).strong()).fill(ACCENT)).clicked()
                    {
                        settings = true;
                    }
                    if ui.button(t("Close")).clicked() {
                        close = true;
                    }
                });
            });
        if settings {
            self.open_screen_settings();
            close = true;
        }
        if close || !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.x.access.open = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{NO_DESKTOP, SCREEN_ACCESS_NEEDED};

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
