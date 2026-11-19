//! The replay buffer in the window: the Record tab's Replay group, the "Replay 60 s" pill in
//! the tab row, the tray's items and dot, the status line, and the settings (`crate::replay`
//! does the work).

use super::SnagApp;
use crate::i18n::{t, tf};
use crate::look::{self, ph};
use crate::replay::{self, Phase};
use crate::settings::Action;
use eframe::egui::{self, Color32, CornerRadius, Sense, pos2, vec2};
use std::time::Duration;

#[derive(Default)]
pub(crate) struct ReplayUi {
    /// The status last seen (to notice what changed).
    pub snap: replay::Status,
    /// "Turn replay on when Snagster starts" was looked at.
    started: bool,
}

/// The status line when replay can't come back at launch without the desktop asking.
pub(crate) const NEEDS_SCREEN: &str = "Replay is off: the desktop needs you to choose the screen again. Record > Replay turns it on.";

/// What launch does about "Turn replay on when Snagster starts".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AtStart {
    Nothing,
    TurnOn,
    /// It would need the desktop's dialog: stay off and say so.
    SayWhy,
}

pub(crate) fn at_start(wanted: bool, real_window: bool, quiet: bool) -> AtStart {
    match (wanted && real_window, quiet) {
        (false, _) => AtStart::Nothing,
        (true, true) => AtStart::TurnOn,
        (true, false) => AtStart::SayWhy,
    }
}

/// The pill's text: "Replay 60 s", or "Replay…" while starting.
pub(crate) fn pill_text(s: &replay::Status) -> String {
    match s.phase {
        Phase::Starting => format!("{}…", t("Replay")),
        _ => tf("Replay {seconds} s", &[("seconds", &s.seconds.to_string())]),
    }
}

/// The pill next to the tab row while replay is on (accent coloured: red means recording).
fn pill(ui: &mut egui::Ui, s: &replay::Status) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(pill_text(s), egui::FontId::proportional(12.0), Color32::WHITE);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 26.0, 20.0), Sense::click());
    ui.painter().rect_filled(rect, CornerRadius::same(10), look::accent());
    ui.painter().circle_filled(pos2(rect.left() + 10.0, rect.center().y), 4.0, Color32::WHITE);
    ui.painter().galley(pos2(rect.left() + 18.0, rect.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
    let tip = tf("Keeping the last {seconds} s of the screen ({held} s so far). Click for Record > Replay.", &[("seconds", &s.seconds.to_string()), ("held", &format!("{:.0}", s.held_s))]);
    resp.on_hover_text(tip)
}

impl SnagApp {
    /// The Save replay hotkey as the status line names it ("" when none is set).
    fn replay_hotkey(&self) -> String {
        let c = self.settings.binding(Action::SaveReplay);
        if c.is_set() { c.label() } else { String::new() }
    }

    /// Turn the replay buffer on or off (the button, the tray, the hotkey, app_command).
    pub(crate) fn toggle_replay(&mut self, ctx: &egui::Context) {
        if replay::status().on() {
            replay::end();
            self.status = t("Replay off.").into();
        } else {
            self.replay_on(ctx);
        }
        ctx.request_repaint();
    }

    fn replay_on(&mut self, ctx: &egui::Context) {
        match replay::begin(replay::ReplayOptions::from_settings(&self.settings)) {
            Ok(()) => {
                self.status = if cfg!(target_os = "linux") { t("Turning replay on: the desktop may ask which screen to share.").into() } else { t("Turning replay on…").into() };
            }
            Err(e) => self.status = e,
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    /// Save the last seconds at once, in the background; the result comes in `poll_replay`.
    pub(crate) fn save_replay(&mut self, ctx: &egui::Context) {
        let ctx2 = ctx.clone();
        match replay::save_in_background(None, move || ctx2.request_repaint()) {
            Ok(()) => self.status = t("Saving the replay…").into(),
            Err(e) => self.status = e,
        }
    }

    /// Every frame: settings in, and a status line when replay comes on, fails or saved.
    pub(crate) fn poll_replay(&mut self, ctx: &egui::Context) {
        if !self.x.replay.started {
            self.x.replay.started = true;
            match at_start(self.settings.replay_at_start, self.persist, replay::can_resume_quietly()) {
                AtStart::TurnOn => self.replay_on(ctx),
                AtStart::SayWhy => self.status = t(NEEDS_SCREEN).into(),
                AtStart::Nothing => {}
            }
        }
        let s = replay::status();
        if s.seconds != replay::clamp_seconds(self.settings.replay_s) {
            replay::set_seconds(self.settings.replay_s);
        }
        let was = self.x.replay.snap.clone();
        if s.phase == Phase::On && was.phase != Phase::On {
            self.status = replay::on_message(s.seconds, &self.replay_hotkey());
            if let Some(n) = s.info.as_ref().and_then(|i| i.notice.clone()) {
                self.status = format!("{} {n}", self.status);
            }
        }
        if s.error.is_some() && s.error != was.error {
            let e = self.grab_failed("{error}", s.error.as_deref().unwrap_or_default());
            self.status = tf("Replay stopped: {e}", &[("e", &e)]);
        }
        if s.saved_no != was.saved_no {
            match &s.saved {
                Some(Ok(v)) => {
                    self.status = replay::saved_message(v);
                    self.notify_away(ctx, t("Replay saved"), &v.path);
                    let info = s.info.clone();
                    self.videos.insert(
                        0,
                        crate::video::VideoDone {
                            path: v.path.clone(),
                            frames: v.frames,
                            seconds: v.seconds as f32,
                            encoder: info.as_ref().map(|i| i.encoder.clone()).unwrap_or_default(),
                            hardware: info.as_ref().is_some_and(|i| i.hardware),
                            capture: "",
                            width: info.as_ref().map_or(0, |i| i.width),
                            height: info.as_ref().map_or(0, |i| i.height),
                            result: Ok(()),
                            note: String::new(),
                        },
                    );
                    self.earlier.expire();
                }
                Some(Err(e)) => self.status = tf("The replay couldn't be saved: {e}", &[("e", e)]),
                None => {}
            }
        }
        if s.on() {
            // The pill's seconds count up.
            ctx.request_repaint_after(Duration::from_millis(1000));
        }
        if s.on() != was.on()
            && let Some(tr) = &self.tray
        {
            tr.set_replay(s.on());
        }
        self.x.replay.snap = s;
    }

    /// The pill in the tab row (clicking it opens the Record tab).
    pub(crate) fn replay_pill(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.x.replay.snap.on() && pill(ui, &self.x.replay.snap).clicked() {
            self.switch_tab(ctx, super::ribbon::RibbonTab::Record);
        }
    }

    /// Record > Replay: on / off, Save replay and how much to keep.
    pub(crate) fn replay_group(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let on = self.x.replay.snap.on();
        let secs = replay::clamp_seconds(self.settings.replay_s).to_string();
        look::group(ui, t("Replay"), |ui| {
            let (label, tip) = if on {
                (t("Stop replay"), t("Stop keeping the screen (nothing is saved)").to_string())
            } else {
                (t("Replay"), tf("Keep the last {seconds} s of the screen in memory, so you can save a moment after it happens", &[("seconds", &secs)]))
            };
            if look::big_tool(ui, ph::CLOCK_COUNTER_CLOCKWISE, label, &format!("{tip}{}", super::ribbon::kbd(&self.settings, Action::ToggleReplay)), true, on).clicked() {
                self.do_action(ctx, Action::ToggleReplay);
            }
            look::column(ui, |ui| {
                let tip = format!("{}{}", tf("Save the last {seconds} s as a video", &[("seconds", &secs)]), super::ribbon::kbd(&self.settings, Action::SaveReplay));
                if look::small_tool(ui, ph::DOWNLOAD_SIMPLE, t("Save replay"), &tip, self.x.replay.snap.phase == Phase::On).clicked() {
                    self.do_action(ctx, Action::SaveReplay);
                }
                look::small_menu(ui, ph::TIMER, &format!("{secs} s"), t("How much to keep"), |ui| {
                    for v in replay::LENGTHS {
                        if ui.radio(self.settings.replay_s == v, format!("{v} s")).clicked() {
                            self.settings.replay_s = v;
                            ui.close();
                        }
                    }
                });
            });
        });
    }
}

/// Settings > Recording > Replay.
pub(crate) fn settings_section(ui: &mut egui::Ui, s: &mut crate::settings::Settings) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(t("Replay")).strong());
    ui.horizontal(|ui| {
        ui.label(t("Keep the last"));
        for v in replay::LENGTHS {
            if ui.selectable_label(s.replay_s == v, format!("{v} s")).clicked() {
                s.replay_s = v;
            }
        }
    });
    let mb = replay::memory_mb(replay::bitrate(1920, 1080, s.video_fps.clamp(1, 60), s.video_quality, s.replay_s, replay::MAX_BYTES), s.replay_s);
    ui.label(egui::RichText::new(tf("Uses about {mb} MB of memory at the current quality (a full-HD screen).", &[("mb", &mb.to_string())])).color(look::pal().dim));
    if crate::audio::AudioSource::supported() {
        ui.horizontal_wrapped(|ui| {
            ui.label(t("Sound kept with replays"));
            for v in [crate::audio::AudioSource::System, crate::audio::AudioSource::Both, crate::audio::AudioSource::None] {
                ui.radio_value(&mut s.replay_audio, v, t(v.label()));
            }
        });
    }
    ui.checkbox(&mut s.replay_at_start, t("Turn replay on when Snagster starts"));
    ui.label(egui::RichText::new(t("Replay is off until you turn it on. While it is on, the screen is encoded all the time (on the graphics chip when it can) and only the last seconds are kept.")).color(look::pal().dim));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_turns_replay_on_only_when_asked_and_never_with_a_dialog() {
        assert_eq!(at_start(false, true, true), AtStart::Nothing);
        // The check runs (--ui-test, screenshots) never start it.
        assert_eq!(at_start(true, false, true), AtStart::Nothing);
        assert_eq!(at_start(true, true, true), AtStart::TurnOn);
        // Wayland without a remembered share: stay off and say why.
        assert_eq!(at_start(true, true, false), AtStart::SayWhy);
        assert!(!crate::settings::Settings::default().replay_at_start);
    }

    #[test]
    fn the_pill_names_the_length() {
        let mut s = replay::Status { phase: Phase::On, seconds: 90, ..Default::default() };
        assert_eq!(pill_text(&s), "Replay 90 s");
        s.phase = Phase::Starting;
        assert_eq!(pill_text(&s), "Replay…");
    }
}
