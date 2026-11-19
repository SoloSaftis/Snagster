//! Live streaming in the window: the Stream tab's band, going live (asked once per session),
//! stopping at once, the red LIVE badge, the stream key, and the Settings page. The stream
//! itself runs in `stream::live` on threads of its own; the window only reads its snapshot.

use super::*;
use crate::i18n::{t, tf};
use crate::look::{self, ph};
use crate::stream::live::{self, LiveOptions, Phase, Snapshot, StudioFrames};
use crate::stream::engine::Stats;
use crate::stream::health::{Level, Steady, Warning};
use crate::stream::{Encoders, PRESETS, Service, Source, State, StreamTarget, keystore, mask};

pub(crate) const LIVE_RED: Color32 = Color32::from_rgb(200, 36, 44);

#[derive(Default)]
pub(crate) struct StreamUi {
    /// The stream key, read from the key store when first needed (only kept in memory in
    /// check runs, which never touch the person's key).
    key: Option<String>,
    /// What the key box on the Settings page shows.
    pub key_edit: Option<String>,
    pub show_key: bool,
    /// Going live was confirmed once in this session.
    pub confirmed: bool,
    pub confirm_open: bool,
    /// An area is being picked on the screen for the stream (true: go live once it is picked).
    pub picking_area: Option<bool>,
    /// The stream's state this frame.
    pub snap: Snapshot,
    /// Encoders for the button test (None: this computer's).
    pub encoders: Option<Encoders>,
    /// The stream shows the Studio window's scene: Studio stays open while it runs.
    pub uses_studio: bool,
    /// The extra services' keys by id (read from the key store when first needed), and what
    /// their key boxes show.
    also_keys: std::collections::HashMap<u32, String>,
    also_edit: std::collections::HashMap<u32, String>,
    /// The one stream warning shown, held steady (no flicker).
    steady: Steady,
    /// What the band and the panel say about how the stream goes (None: all is well).
    pub health_line: Option<HealthLine>,
}

/// The band's and the panel's line about how the stream goes when something is wrong: a few
/// words, and the advice for hovering and the details.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HealthLine {
    pub short: String,
    pub advice: String,
    pub bad: bool,
}

/// What to say about the stream's health: a service of several that is reconnecting or has
/// failed (bad), else the warning `Steady` shows.
pub(crate) fn health_line(snap: &Snapshot, shown: Option<&Warning>) -> Option<HealthLine> {
    let s = snap.stats.as_ref().filter(|_| snap.phase == Phase::Running)?;
    if s.outputs.len() > 1
        && s.state != State::Failed
        && let Some(o) = s.outputs.iter().find(|o| matches!(o.state, State::Reconnecting | State::Failed))
    {
        let short = format!("{}: {}", o.service, output_word(o.state).0);
        return Some(HealthLine { advice: o.error.clone().unwrap_or_else(|| short.clone()), short, bad: true });
    }
    let w = shown?;
    Some(HealthLine { short: w.fill(t(w.short())), advice: w.fill(t(w.advice())), bad: w.level == Level::Bad })
}

/// The icon and colour of a warning (bad: red octagon), so colour is never the only sign.
pub(crate) fn health_icon(bad: bool) -> (&'static str, Color32) {
    if bad { (ph::WARNING_OCTAGON, pal().err) } else { (ph::WARNING, pal().warn) }
}

/// The health line as a label, the advice on hover.
pub(crate) fn health_label(ui: &mut egui::Ui, l: &HealthLine, size: f32) -> egui::Response {
    let (icon, color) = health_icon(l.bad);
    ui.label(RichText::new(format!("{icon} {}", l.short)).size(size).color(color)).on_hover_text(&l.advice)
}

/// A share as a percentage: one decimal below 10 %, whole from there.
pub(crate) fn percent(share: f32) -> String {
    let p = (share * 100.0).max(0.0);
    let n = if p < 10.0 { format!("{p:.1}") } else { format!("{p:.0}") };
    tf("{pct}%", &[("pct", &n)])
}

/// "4.1 Mbit/s of 6.2 Mbit/s": what the services that are measured sent against what was set
/// for them (video plus sound), or "Measuring…".
pub(crate) fn sent_of_set(s: &Stats) -> String {
    let measured: Vec<_> = s.health.outputs.iter().filter(|o| o.measured).collect();
    if measured.is_empty() {
        return t("Measuring…").to_string();
    }
    let sent: u64 = measured.iter().map(|o| o.sent_kbps as u64 * 1000).sum();
    let set = s.health.set_kbps as u64 * 1000 * measured.len() as u64;
    tf("{sent} of {set}", &[("sent", &rate(sent)), ("set", &rate(set))])
}

/// The video encoder in words: "NVENC H.264 (graphics chip)".
fn encoder_text(s: &Stats) -> String {
    s.video_encoder.as_ref().map(|e| format!("{} ({})", e.name, if e.hardware { t("graphics chip") } else { t("processor") })).unwrap_or_default()
}

/// The stream's numbers in full, for the details popover: a column per service, then the
/// computer's, then the warning's advice.
pub(crate) fn stream_details(ui: &mut egui::Ui, s: &Stats, line: Option<&HealthLine>) {
    let h = &s.health;
    let measuring = t("Measuring…");
    let dim = |t: String| RichText::new(t).color(pal().dim);
    ui.set_min_width(280.0);
    egui::Grid::new("stream-details-services").num_columns(1 + h.outputs.len()).spacing(vec2(16.0, 4.0)).show(ui, |ui| {
        if h.outputs.len() > 1 {
            ui.label("");
            for o in &h.outputs {
                ui.label(RichText::new(&o.service).strong());
            }
            ui.end_row();
        }
        ui.label(t("Sent"));
        for o in &h.outputs {
            ui.label(if o.measured { tf("{sent} of {set}", &[("sent", &rate(o.sent_kbps as u64 * 1000)), ("set", &rate(h.set_kbps as u64 * 1000))]) } else { measuring.to_string() });
        }
        ui.end_row();
        ui.label(t("Waiting to send"));
        for o in &h.outputs {
            let text = tf("{ms} ms", &[("ms", &o.queue_ms.to_string())]);
            let c = if o.queue_ms >= crate::stream::health::QUEUE_BAD_MS {
                pal().err
            } else if o.queue_ms >= crate::stream::health::QUEUE_WARN_MS {
                pal().warn
            } else {
                pal().text
            };
            ui.label(RichText::new(text).color(c));
        }
        ui.end_row();
        ui.label(t("Dropped by the network"));
        for o in &h.outputs {
            // The count from the start; its share once it is measured.
            ui.label(if o.measured { tf("{n} ({pct})", &[("n", &o.dropped.to_string()), ("pct", &percent(o.dropped_share))]) } else { o.dropped.to_string() });
        }
        ui.end_row();
        ui.label(t("Connected for"));
        for (o, out) in h.outputs.iter().zip(&s.outputs) {
            ui.label(if out.state == State::Live { clock(o.connected_s) } else { output_word(out.state).0.to_string() });
        }
        ui.end_row();
        ui.label(t("Reconnects"));
        for o in &h.outputs {
            ui.label(o.reconnects.to_string());
        }
        ui.end_row();
    });
    ui.separator();
    egui::Grid::new("stream-details-computer").num_columns(2).spacing(vec2(16.0, 4.0)).show(ui, |ui| {
        ui.label(t("Dropped by the computer"));
        ui.label(if h.window_s > 0.0 { tf("{n} ({pct})", &[("n", &h.encoder_lost.to_string()), ("pct", &percent(h.encoder_lost_share))]) } else { h.encoder_lost.to_string() });
        ui.end_row();
        ui.label(t("Encoder"));
        ui.label(encoder_text(s));
        ui.end_row();
        ui.label(t("Time per frame"));
        ui.label(if h.window_s > 0.0 { tf("{ms} of {budget} ms", &[("ms", &format!("{:.0}", h.encode_ms)), ("budget", &format!("{:.0}", h.frame_budget_ms))]) } else { measuring.to_string() });
        ui.end_row();
    });
    ui.label(dim(t("Percentages: last 10 seconds").to_string()).size(11.0));
    if let Some(l) = line {
        let (icon, color) = health_icon(l.bad);
        ui.add_space(4.0);
        ui.label(RichText::new(format!("{icon} {}", l.advice)).color(color));
    }
    ui.ctx().request_repaint_after(Duration::from_millis(500));
}

/// A service's name as shown (only "Custom server" is ours to translate).
pub(crate) fn service_name(s: Service) -> &'static str {
    if s == Service::Custom { t("Custom server") } else { s.preset().name }
}

/// "Twitch" or, when multistreaming, "Twitch + YouTube + Kick".
pub(crate) fn services_named(main: Service, also: impl Iterator<Item = Service>) -> String {
    std::iter::once(main).chain(also).map(service_name).collect::<Vec<_>>().join(" + ")
}

/// `h:mm:ss` or `m:ss`.
pub(crate) fn clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// Bits per second, readable.
pub(crate) fn rate(bps: u64) -> String {
    if bps >= 1_000_000 { format!("{:.1} Mbit/s", bps as f64 / 1e6) } else { format!("{} kbit/s", bps / 1000) }
}

/// What the stream is doing, in words.
pub(crate) fn state_label(snap: &Snapshot) -> &'static str {
    match (snap.phase, snap.stats.as_ref().map(|s| s.state)) {
        (Phase::Idle, _) => t("Off the air"),
        (Phase::Starting, _) | (Phase::Running, Some(State::Connecting)) | (Phase::Running, None) => t("Connecting…"),
        (Phase::Running, Some(State::Live)) => t("Live"),
        (Phase::Running, Some(State::Reconnecting)) => t("Reconnecting…"),
        (Phase::Running, Some(State::Failed)) => t("The connection failed"),
        (Phase::Stopping, _) | (Phase::Running, Some(State::Stopping | State::Stopped)) => t("Stopping…"),
    }
}

/// One service's state in a word for the band, and whether it is a warning.
pub(crate) fn output_word(s: State) -> (&'static str, bool) {
    match s {
        State::Connecting => (t("Connecting…"), false),
        State::Live => (t("Live"), false),
        State::Reconnecting => (t("Reconnecting…"), true),
        State::Stopping | State::Stopped => (t("Stopping…"), false),
        State::Failed => (t("The connection failed"), true),
    }
}

/// The red LIVE badge with how long it has been on the air (grey while connecting).
pub(crate) fn live_badge(ui: &mut egui::Ui, snap: &Snapshot) -> egui::Response {
    let secs = snap.stats.as_ref().map(|s| s.uptime_s).unwrap_or(0.0);
    let text = if snap.live() { format!("{}  {}", t("LIVE"), clock(secs)) } else { t("LIVE").to_string() };
    let galley = ui.painter().layout_no_wrap(text, egui::FontId::proportional(12.0), Color32::WHITE);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 26.0, 20.0), Sense::click());
    let fill = if snap.live() { LIVE_RED } else { Color32::from_rgb(110, 60, 64) };
    ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
    // A blinking dot while live.
    let on = !snap.live() || (ui.input(|i| i.time) * 1.25).fract() < 0.6;
    if on {
        ui.painter().circle_filled(pos2(rect.left() + 10.0, rect.center().y), 4.0, Color32::WHITE);
    }
    ui.painter().galley(pos2(rect.left() + 18.0, rect.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
    if snap.live() {
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }
    resp.on_hover_text(state_label(snap))
}

impl SnagApp {
    /// The stream key (from the key store the first time).
    pub(crate) fn stream_key(&mut self) -> String {
        if self.x.stream.key.is_none() {
            self.x.stream.key = Some(if self.persist { keystore::load().unwrap_or_default() } else { String::new() });
        }
        self.x.stream.key.clone().unwrap_or_default()
    }

    /// Keep a new stream key (in the key store; in memory only during check runs).
    pub(crate) fn set_stream_key(&mut self, key: &str) {
        let key = key.trim().to_string();
        if self.persist
            && let Err(e) = keystore::save(&key)
        {
            self.status = mask(&e, &key);
        }
        self.x.stream.key = Some(key);
    }

    /// An extra service's stream key (from the key store the first time).
    pub(crate) fn also_key(&mut self, id: u32) -> String {
        let persist = self.persist;
        self.x.stream.also_keys.entry(id).or_insert_with(|| if persist { keystore::load_for(id).unwrap_or_default() } else { String::new() }).clone()
    }

    /// Keep an extra service's key (in memory only during check runs).
    pub(crate) fn set_also_key(&mut self, id: u32, key: &str) {
        let key = key.trim().to_string();
        if self.persist
            && let Err(e) = keystore::save_for(id, &key)
        {
            self.status = mask(&e, &key);
        }
        self.x.stream.also_keys.insert(id, key);
    }

    /// The options for going live now: the settings, the main key and the extra services' keys.
    pub(crate) fn live_options(&mut self) -> LiveOptions {
        let key = self.stream_key();
        let ids: Vec<u32> = self.settings.stream.also_on().map(|a| a.id).collect();
        let keys: std::collections::HashMap<u32, String> = ids.into_iter().map(|id| (id, self.also_key(id))).collect();
        LiveOptions::from_settings(&self.settings, &key, |id| keys.get(&id).cloned().unwrap_or_default())
    }

    /// Where the stream goes, with the key (for checks; never shown).
    fn stream_target(&mut self) -> StreamTarget {
        let key = self.stream_key();
        StreamTarget { service: self.settings.stream.service, url: self.settings.stream.url.clone(), key }
    }

    /// Go live, or say what's missing. The first time in a session it asks first.
    pub(crate) fn go_live(&mut self, ctx: &egui::Context) {
        if self.x.stream.snap.phase != Phase::Idle || live::snapshot().phase != Phase::Idle {
            return;
        }
        self.ask_screen_access_once();
        let target = self.stream_target();
        if let Err(e) = target.endpoint() {
            self.status = tf("Can't go live yet: {e}", &[("e", &mask(&e, &target.key))]);
            self.settings_open = true;
            self.settings_tab = SettingsTab::Streaming;
            return;
        }
        for t in self.live_options().also {
            if let Err(e) = t.endpoint() {
                let e = format!("{}: {}", service_name(t.service), mask(&e, &t.key));
                self.status = tf("Can't go live yet: {e}", &[("e", &e)]);
                self.settings_open = true;
                self.settings_tab = SettingsTab::Streaming;
                return;
            }
        }
        if let Source::Area(_, _, w, h) = self.settings.stream.source
            && (w < 16 || h < 16)
        {
            self.pick_stream_area(ctx, true);
            return;
        }
        if !self.x.stream.confirmed {
            self.x.stream.confirm_open = true;
            return;
        }
        self.start_stream_now(ctx);
    }

    /// Start at once (after the confirmation).
    pub(crate) fn start_stream_now(&mut self, ctx: &egui::Context) {
        let o = self.live_options();
        let keys: Vec<String> = std::iter::once(&o.target).chain(&o.also).map(|t| t.key.clone()).collect();
        self.x.stream.uses_studio = false;
        let studio = match (&o.source, &self.x.studio) {
            (Source::Studio, Some(s)) => {
                self.x.stream.uses_studio = true;
                Some(StudioFrames::Running(s.engine_shared(), self.settings.studio.width.max(16), self.settings.studio.height.max(16)))
            }
            (Source::Studio, None) => {
                // Studio runs in the window, so the Stream tab's scene buttons and mixer drive
                // the stream too.
                let s = super::studio_ui::StudioUi::start(&self.settings, ctx);
                let shared = s.engine_shared();
                self.x.studio = Some(s);
                self.x.stream.uses_studio = true;
                Some(StudioFrames::Running(shared, self.settings.studio.width.max(16), self.settings.studio.height.max(16)))
            }
            _ => None,
        };
        let enc = self.x.stream.encoders.clone().unwrap_or_else(|| Encoders::system_picked(&self.settings.video_encoder));
        let service = services_named(o.target.service, o.also.iter().map(|t| t.service));
        match live::begin(o, studio, enc) {
            Ok(()) => self.status = tf("Going live on {service}…", &[("service", &service)]),
            Err(e) => self.status = keys.iter().fold(e, |e, k| mask(&e, k)),
        }
        ctx.request_repaint();
    }

    /// Stop now; the stream closes in the background.
    pub(crate) fn stop_stream(&mut self) {
        if live::end() {
            self.status = t("Stopping the stream…").into();
        }
    }

    pub(crate) fn toggle_stream(&mut self, ctx: &egui::Context) {
        if self.x.stream.snap.phase == Phase::Idle { self.go_live(ctx) } else { self.stop_stream() }
    }

    /// Drag the area to stream on the screen (then go live when `then_live`).
    pub(crate) fn pick_stream_area(&mut self, ctx: &egui::Context, then_live: bool) {
        self.x.stream.picking_area = Some(then_live);
        self.start_capture(ctx, CaptureKind::Pick(CutShape::Rect, PickPurpose::RecordGif));
        self.status = t("Drag the area to stream").into();
    }

    /// An area was picked on the screen for the stream: true when it was ours.
    pub(crate) fn stream_area_picked(&mut self, ctx: &egui::Context, rect: (i32, i32, u32, u32)) -> bool {
        let Some(then_live) = self.x.stream.picking_area.take() else { return false };
        self.settings.stream.source = Source::Area(rect.0, rect.1, rect.2, rect.3);
        if then_live {
            self.go_live(ctx);
        } else {
            self.status = tf("The stream shows the area {w} x {h}.", &[("w", &rect.2.to_string()), ("h", &rect.3.to_string())]);
        }
        true
    }

    /// Every frame: the stream's state, and a line in the status bar when it ends or fails.
    pub(crate) fn poll_stream(&mut self, ctx: &egui::Context) {
        // A stream of Studio follows its scene and mixer from any tab.
        if self.x.studio.is_some() && self.x.tab != super::ribbon::RibbonTab::Studio {
            self.studio_tick();
        }
        let snap = live::snapshot();
        if snap.phase != Phase::Idle {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        if snap.error.is_some() && snap.error != self.x.stream.snap.error {
            let e = self.grab_failed("{error}", snap.error.as_deref().unwrap_or_default());
            self.status = tf("Couldn't go live: {e}", &[("e", &e)]);
        }
        if let Some(end) = &snap.last
            && Some(end) != self.x.stream.snap.last.as_ref()
        {
            let s = &end.stats;
            let mut line = match &s.error {
                Some(e) if s.state == State::Failed => {
                    let e = self.grab_failed("{error}", e);
                    tf("The stream stopped: {e}", &[("e", &e)])
                }
                _ => tf("The stream ended after {time}: {frames} frames sent, {dropped} dropped.", &[
                    ("time", &clock(s.uptime_s)),
                    ("frames", &s.frames_sent.to_string()),
                    ("dropped", &s.frames_dropped.to_string()),
                ]),
            };
            match &end.copy {
                Some(Ok(c)) => line = format!("{line} {}", tf("A copy is in {file}.", &[("file", &c.path.display().to_string())])),
                Some(Err(e)) => line = format!("{line} {e}"),
                None => {}
            }
            self.status = line;
        }
        // Studio's compositor draws at the stream's rate while it is streamed.
        if snap.phase == Phase::Running
            && self.x.stream.uses_studio
            && let (Some(st), Some(info)) = (&self.x.studio, &snap.info)
            && let Ok(mut f) = st.engine_shared().recording_fps.lock()
            && f.is_none()
        {
            *f = Some(info.fps);
        }
        if snap.phase == Phase::Idle {
            self.x.stream.uses_studio = false;
        }
        self.health_tick(ctx.input(|i| i.time), &snap);
        self.x.stream.snap = snap;
    }

    /// The health line for this frame (`secs`: the window's clock), held steady; a bad one that
    /// is new also goes to the status line, so it is read out and seen in the panel.
    pub(crate) fn health_tick(&mut self, secs: f64, snap: &Snapshot) {
        let shown = match &snap.stats {
            Some(s) if snap.phase == Phase::Running => self.x.stream.steady.update((secs * 1000.0) as u64, &s.health.warnings).cloned(),
            _ => {
                self.x.stream.steady.reset();
                None
            }
        };
        let line = health_line(snap, shown.as_ref());
        if let Some(l) = line.as_ref().filter(|l| l.bad)
            && self.x.stream.health_line.as_ref().is_none_or(|p| !p.bad || p.short != l.short)
        {
            self.status = l.advice.clone();
        }
        // The status line still gives advice for a problem that has gone: say it's fine again.
        if line.is_none()
            && let Some(p) = &self.x.stream.health_line
            && self.status == p.advice
        {
            self.status = t("The stream is fine again.").into();
        }
        self.x.stream.health_line = line;
    }

    /// The first time in a session: "Go live on Twitch?".
    pub(crate) fn stream_confirm_window(&mut self, ctx: &egui::Context) {
        if !self.x.stream.confirm_open {
            return;
        }
        let (mut go, mut cancel) = (false, false);
        let st = &self.settings.stream;
        let service = services_named(st.service, st.also_on().map(|a| a.service));
        let source = t(self.settings.stream.source.label());
        let mut open = true;
        crate::windows::own_window(ctx, crate::windows::Win { key: "stream-confirm", title: t("Go live?"), size: vec2(400.0, 160.0), resizable: false }, &mut open, |ui| {
                ui.set_max_width(360.0);
                ui.label(tf("Everyone watching on {service} will see this from now on: {source}.", &[("service", &service), ("source", source)]));
                ui.label(RichText::new(t("Snagster asks once each time it starts.")).color(pal().dim));
                ui.add_space(6.0);
                // Going live reaches other people: red, and Return doesn't start it.
                match crate::platform::dialog_buttons(ui, Some(t("Go live")), t("Cancel"), crate::platform::Kind::Destructive, true) {
                    Some(crate::platform::Choice::Action) => go = true,
                    Some(crate::platform::Choice::Cancel) => cancel = true,
                    None => {}
                }
            });
        if go {
            self.x.stream.confirmed = true;
            self.x.stream.confirm_open = false;
            self.start_stream_now(ctx);
        } else if cancel || !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.x.stream.confirm_open = false;
        }
    }

    /// The services as radio buttons.
    pub(crate) fn service_menu(&mut self, ui: &mut egui::Ui) {
        for p in PRESETS {
            if ui.radio(self.settings.stream.service == p.service, service_name(p.service)).clicked() {
                if self.settings.stream.service != p.service {
                    self.settings.stream.service = p.service;
                    self.settings.stream.url.clear();
                    self.settings.stream = self.settings.stream.clamped();
                }
                ui.close();
            }
        }
    }

    /// The windows to stream (from the window list).
    fn window_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.win_cache.get(Some(&ctx), capture::list_windows);
        if !self.win_cache.loaded() {
            ui.spinner();
        } else if self.win_cache.items().is_empty() {
            ui.label(RichText::new(t("No windows found")).color(pal().dim));
        }
        let mut pick = None;
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            for w in self.win_cache.items() {
                if ui.selectable_label(self.settings.stream.source == Source::Window(w.label.clone()), &w.label).clicked() {
                    pick = Some(w.label.clone());
                    ui.close();
                }
            }
        });
        if let Some(l) = pick {
            self.settings.stream.source = Source::Window(l);
        }
    }

    /// The Stream tab's "All commands" menu.
    pub(crate) fn stream_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let idle = self.x.stream.snap.phase == Phase::Idle;
        if item_on(ui, idle, t("Go live"), "") {
            self.go_live(ctx);
        }
        if item_on(ui, !idle, t("Stop the stream"), "") {
            self.stop_stream();
        }
        ui.separator();
        ui.menu_button(t("Service"), |ui| self.service_menu(ui));
        ui.menu_button(t("Source"), |ui| {
            if ui.radio(self.settings.stream.source == Source::Screen, t("Whole screen")).clicked() {
                self.settings.stream.source = Source::Screen;
            }
            ui.menu_button(t("A window"), |ui| self.window_menu(ui));
            if ui.button(t("Pick an area…")).clicked() {
                ui.close();
                self.pick_stream_area(ctx, false);
            }
            if ui.radio(self.settings.stream.source == Source::Studio, t("Studio scene")).clicked() {
                self.settings.stream.source = Source::Studio;
            }
        });
        ui.checkbox(&mut self.settings.stream.record_copy, t("Record a copy"));
        ui.checkbox(&mut self.settings.stream.cursor, t("Mouse pointer"));
        ui.separator();
        if item(ui, t("Stream settings…"), "") {
            self.settings_open = true;
            self.settings_tab = SettingsTab::Streaming;
        }
        if item(ui, t("Recordings folder"), "") {
            let f = PathBuf::from(&self.settings.video_folder);
            let _ = std::fs::create_dir_all(&f);
            self.open_folder(&f);
        }
    }

    /// The Settings window's Streaming page.
    pub(crate) fn streaming_tab(&mut self, ui: &mut egui::Ui) {
        let p = self.settings.stream.service.preset();
        ui.label(RichText::new(t("Service")).strong());
        egui::ComboBox::from_id_salt("stream-service").width(240.0).selected_text(service_name(p.service)).show_ui(ui, |ui| self.service_menu(ui));
        ui.label(RichText::new(t(p.key_help)).color(pal().dim));
        ui.add_space(4.0);
        let p = self.settings.stream.service.preset();
        ui.horizontal(|ui| {
            ui.label(t("Server"));
            let mut u = if self.settings.stream.url.is_empty() { p.url.to_string() } else { self.settings.stream.url.clone() };
            if ui.add(egui::TextEdit::singleline(&mut u).hint_text("rtmp://... / rtmps://...").desired_width(330.0)).changed() {
                self.settings.stream.url = if u.trim() == p.url { String::new() } else { u };
            }
            if !self.settings.stream.url.is_empty() && !p.url.is_empty() && ui.button(t("Usual server")).on_hover_text(p.url).clicked() {
                self.settings.stream.url.clear();
            }
        });
        // The key: a password box, kept out of settings.json.
        if self.x.stream.key_edit.is_none() {
            self.x.stream.key_edit = Some(self.stream_key());
        }
        let mut save = None;
        ui.horizontal(|ui| {
            ui.label(t("Stream key"));
            let show = self.x.stream.show_key;
            let edit = self.x.stream.key_edit.get_or_insert_with(String::new);
            let r = ui.add(egui::TextEdit::singleline(edit).password(!show).desired_width(260.0).hint_text(t("Paste it from the service")));
            if r.lost_focus() || (r.changed() && edit.trim().is_empty()) {
                save = Some(edit.clone());
            }
            let (icon, tip) = if show { (ph::EYE_SLASH, t("Hide the key")) } else { (ph::EYE, t("Show the key")) };
            if look::icon_button(ui, icon, tip).clicked() {
                self.x.stream.show_key = !show;
            }
            if ui.button(t("Forget")).on_hover_text(t("Remove the saved stream key")).clicked() {
                save = Some(String::new());
                self.x.stream.key_edit = Some(String::new());
            }
        });
        if let Some(k) = save
            && Some(k.trim()) != self.x.stream.key.as_deref()
        {
            self.set_stream_key(&k);
        }
        ui.label(RichText::new(t(keystore::how_kept())).color(pal().dim));
        ui.add_space(6.0);
        self.also_section(ui);
        ui.add_space(6.0);
        let pickable = self.x.pickable.get(Some(ui.ctx()), crate::encode::available).to_vec();
        let s = &mut self.settings.stream;
        let p = s.service.preset();
        ui.label(RichText::new(t("Picture")).strong());
        ui.horizontal(|ui| {
            ui.label(t("Size"));
            let shown = if s.max_height == 0 { t("The source's size").to_string() } else { format!("{}p", s.max_height) };
            egui::ComboBox::from_id_salt("stream-size").selected_text(shown).show_ui(ui, |ui| {
                ui.selectable_value(&mut s.max_height, 0, t("The source's size"));
                for h in crate::stream::services::HEIGHTS {
                    if h <= p.max_height {
                        ui.selectable_value(&mut s.max_height, h, format!("{h}p"));
                    }
                }
            });
            ui.label(t("Frames per second"));
            egui::ComboBox::from_id_salt("stream-fps").selected_text(s.fps.to_string()).show_ui(ui, |ui| {
                // Only the rates the service takes at the chosen size (4K: up to 60).
                for f in crate::stream::services::FRAME_RATES {
                    if p.offers(s.max_height, f) {
                        ui.selectable_value(&mut s.fps, f, f.to_string());
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label(t("Bitrate"));
            ui.add(egui::DragValue::new(&mut s.kbps).range(0..=p.max_video_kbps).speed(50).suffix(" kbit/s"));
            ui.label(RichText::new(tf("0: recommended for the size; {service} takes up to {max} kbit/s.", &[("service", service_name(p.service)), ("max", &p.max_video_kbps.to_string())])).color(pal().dim));
        });
        ui.horizontal(|ui| {
            ui.label(t("A keyframe every"));
            ui.add(egui::DragValue::new(&mut s.keyframe_s).range(1..=4).suffix(" s"));
            ui.label(RichText::new(t("(services ask for 2 s)")).color(pal().dim));
        });
        ui.checkbox(&mut s.hardware, t("Use the graphics chip to encode (Intel, AMD or NVIDIA)"));
        crate::windows::encoder_picker(ui, "stream-encoder", &mut self.settings.video_encoder, &pickable);
        let s = &mut self.settings.stream;
        ui.checkbox(&mut s.hevc, t("Send HEVC (H.265) where the service takes it")).on_hover_text(t("YouTube and custom servers take HEVC over Enhanced RTMP; other services, and computers without an HEVC encoder, get H.264."));
        ui.checkbox(&mut s.cursor, t("Show the mouse pointer"));
        ui.add_space(4.0);
        ui.label(RichText::new(t("Sound")).strong());
        if crate::audio::AudioSource::supported() {
            ui.horizontal_wrapped(|ui| {
                for v in crate::audio::AudioSource::ALL {
                    ui.radio_value(&mut s.audio, v, t(v.label()));
                }
            });
        } else {
            ui.label(RichText::new(t("There's no computer sound on this system yet, so the stream's audio track is silent.")).color(pal().dim));
        }
        ui.add_space(4.0);
        ui.label(RichText::new(t("Connection")).strong());
        ui.horizontal(|ui| {
            ui.label(t("When the connection drops, try again"));
            ui.add(egui::DragValue::new(&mut s.reconnects).range(0..=50));
            ui.label(t("times"));
        });
        ui.checkbox(&mut s.record_copy, t("Also record what is streamed (an FLV video in the recordings folder)"));
        *s = s.clamped();
    }

    /// "Also stream to": more services fed from the same encode, each with its own server and
    /// key (kept like the main key).
    fn also_section(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new(t("Also stream to")).strong());
        ui.label(RichText::new(t("The same picture goes to every service at once, encoded once. It is sent at the size, frame rate and bitrate all of them take.")).color(pal().dim));
        let mut remove = None;
        let n = self.settings.stream.also.len();
        for i in 0..n {
            let a = self.settings.stream.also[i].clone();
            ui.push_id(("also", a.id), |ui| {
                ui.horizontal(|ui| {
                    let mut on = a.on;
                    if ui.checkbox(&mut on, "").on_hover_text(t("Stream to this service too")).changed() {
                        self.settings.stream.also[i].on = on;
                    }
                    egui::ComboBox::from_id_salt("also-service").width(150.0).selected_text(service_name(a.service)).show_ui(ui, |ui| {
                        for p in PRESETS {
                            if ui.selectable_label(a.service == p.service, service_name(p.service)).clicked() && a.service != p.service {
                                self.settings.stream.also[i].service = p.service;
                                self.settings.stream.also[i].url.clear();
                            }
                        }
                    });
                    let p = self.settings.stream.also[i].service.preset();
                    let mut u = if a.url.is_empty() { p.url.to_string() } else { a.url.clone() };
                    if ui.add(egui::TextEdit::singleline(&mut u).hint_text("rtmp://... / rtmps://...").desired_width(200.0)).on_hover_text(t("Server")).changed() {
                        self.settings.stream.also[i].url = if u.trim() == p.url { String::new() } else { u };
                    }
                    if !self.x.stream.also_edit.contains_key(&a.id) {
                        let k = self.also_key(a.id);
                        self.x.stream.also_edit.insert(a.id, k);
                    }
                    let show = self.x.stream.show_key;
                    let mut save = None;
                    if let Some(edit) = self.x.stream.also_edit.get_mut(&a.id) {
                        let r = ui.add(egui::TextEdit::singleline(edit).password(!show).desired_width(150.0).hint_text(t("Stream key")));
                        if r.lost_focus() || (r.changed() && edit.trim().is_empty()) {
                            save = Some(edit.clone());
                        }
                    }
                    if let Some(k) = save
                        && Some(k.trim()) != self.x.stream.also_keys.get(&a.id).map(String::as_str)
                    {
                        self.set_also_key(a.id, &k);
                    }
                    if look::icon_button(ui, ph::TRASH, t("Remove this service and its saved key")).clicked() {
                        remove = Some(i);
                    }
                });
            });
        }
        if let Some(i) = remove {
            let a = self.settings.stream.also.remove(i);
            self.set_also_key(a.id, "");
            self.x.stream.also_edit.remove(&a.id);
        }
        if self.settings.stream.also.len() < crate::settings::MAX_ALSO && ui.button(format!("{} {}", ph::PLUS, t("Add a service"))).clicked() {
            let id = self.settings.stream.new_also_id();
            // A different service from the main one to start with.
            let service = if self.settings.stream.service == Service::YouTube { Service::Twitch } else { Service::YouTube };
            self.settings.stream.also.push(crate::settings::ExtraService { id, service, url: String::new(), on: true });
        }
    }

    /// The live numbers for the band.
    fn stream_status(&mut self, ui: &mut egui::Ui) {
        let snap = self.x.stream.snap.clone();
        let line = self.x.stream.health_line.clone();
        look::column(ui, |ui| {
            // Numbers change every second: a steady width keeps the ribbon from jumping.
            ui.set_min_width(230.0);
            ui.horizontal(|ui| {
                if snap.on_air() {
                    live_badge(ui, &snap);
                }
                ui.label(RichText::new(state_label(&snap)).strong().color(if snap.live() { look::pal().err } else { look::pal().text }));
                if let Some(s) = snap.stats.as_ref().filter(|_| snap.on_air()) {
                    let r = look::icon_button(ui, ph::CHART_LINE, t("Stream details"));
                    egui::Popup::from_toggle_button_response(&r).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| stream_details(ui, s, line.as_ref()));
                }
            });
            match (&snap.stats, &snap.info) {
                (Some(s), Some(i)) => {
                    ui.label(RichText::new(tf("{w} x {h}, {fps} fps, {rate}", &[("w", &i.width.to_string()), ("h", &i.height.to_string()), ("fps", &format!("{:.0}", s.fps)), ("rate", &sent_of_set(s))])).size(11.5).color(pal().dim));
                    // All is well: the frames dropped (by the network and the computer) and the
                    // encoder. Else the one warning, with its advice on hover.
                    match &line {
                        Some(l) => {
                            health_label(ui, l, 11.5);
                        }
                        None => {
                            let dropped = s.frames_dropped + s.health.encoder_lost;
                            ui.label(RichText::new(tf("{dropped} dropped frames · {enc}", &[("dropped", &dropped.to_string()), ("enc", &encoder_text(s))])).size(11.5).color(pal().dim));
                            if let Some(e) = s.error.as_ref().or(s.notice.as_ref()) {
                                ui.label(RichText::new(e).size(11.5).color(pal().warn));
                            }
                        }
                    }
                }
                _ => {
                    let target = self.settings.stream.clone();
                    let names = services_named(target.service, target.also_on().map(|a| a.service));
                    let where_to = if self.x.stream.snap.phase == Phase::Idle { tf("To {service}, {source}", &[("service", &names), ("source", t(target.source.label()))]) } else { String::new() };
                    ui.label(RichText::new(where_to).size(11.5).color(pal().dim));
                }
            }
        });
    }
}

/// The Stream tab's band.
pub(crate) fn band(app: &mut SnagApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    let snap = app.x.stream.snap.clone();
    let idle = snap.phase == Phase::Idle;
    look::group(ui, t("Live"), |ui| {
        if snap.on_air() {
            if look::big_tool(ui, ph::STOP, t("Stop"), t("End the stream now"), true, true).clicked() {
                app.stop_stream();
            }
        } else {
            let st = &app.settings.stream;
            let tip = tf("Start streaming to {service}: {source}, {fps} fps", &[("service", &services_named(st.service, st.also_on().map(|a| a.service))), ("source", t(st.source.label())), ("fps", &st.fps.to_string())]);
            if look::big_tool(ui, ph::BROADCAST, t("Go live"), &tip, idle, false).clicked() {
                app.go_live(ctx);
            }
        }
    });
    // Ctrl+number switches scenes even when the Scenes group sits in the ribbon's More menu
    // (a narrow window), and only once a frame.
    if app.settings.stream.source == Source::Studio
        && !crate::app::ribbon::fit::in_overflow()
        && let Some(i) = scene_key(ctx)
        && i < app.settings.studio.scenes.len()
    {
        switch_scene(app, i);
    }
    // Streaming Studio's scenes: switch scenes and mute sounds right here while live.
    if app.settings.stream.source == Source::Studio {
        look::group(ui, t("Scenes"), |ui| scene_buttons(app, ui, ctx));
        look::group(ui, t("Sound"), |ui| sound_buttons(app, ui));
    }
    look::group(ui, t("Service"), |ui| {
        look::column(ui, |ui| {
            let name = service_name(app.settings.stream.service);
            look::small_menu(ui, ph::GLOBE, name, t("Where the stream goes"), |ui| app.service_menu(ui));
            let has_key = !app.stream_key().is_empty();
            let tip = if has_key { t("A stream key is saved; change it in the stream settings") } else { t("No stream key yet: paste it in the stream settings") };
            if look::small_tool(ui, ph::KEY, t("Stream key"), tip, true).clicked() {
                app.settings_open = true;
                app.settings_tab = SettingsTab::Streaming;
            }
            if look::small_tool(ui, ph::GEAR, t("Stream settings"), t("Server, picture size, bitrate, sound and reconnecting"), true).clicked() {
                app.settings_open = true;
                app.settings_tab = SettingsTab::Streaming;
            }
        });
    });
    look::group(ui, t("Source"), |ui| {
        let src = app.settings.stream.source.clone();
        if look::big_tool(ui, ph::MONITOR, t("Whole screen"), t("The screen the mouse is on"), idle, src == Source::Screen).clicked() {
            app.settings.stream.source = Source::Screen;
        }
        let window = if let Source::Window(t) = &src { t.clone() } else { String::new() };
        let tip = if window.is_empty() { t("Pick a window; it is followed when it moves").to_string() } else { window.clone() };
        let r = look::big_tool(ui, ph::APP_WINDOW, &format!("{} {}", t("A window"), ph::CARET_DOWN), &tip, idle, !window.is_empty());
        egui::Popup::menu(&r).show(|ui| {
            ui.set_min_width(240.0);
            app.window_menu(ui);
        });
        let tip = match src {
            Source::Area(x, y, w, h) => format!("{w} x {h} @ {x}, {y}"),
            _ => t("Drag the area to stream").to_string(),
        };
        if look::big_tool(ui, ph::SELECTION, t("An area"), &tip, idle, matches!(src, Source::Area(..))).clicked() {
            app.pick_stream_area(ctx, false);
        }
        if look::big_tool(ui, ph::FILM_SLATE, t("Studio scene"), t("Studio's current scene, with all its sources"), idle, src == Source::Studio).clicked() {
            app.settings.stream.source = Source::Studio;
        }
    });
    look::group(ui, t("Options"), |ui| {
        look::column(ui, |ui| {
            ui.add_enabled(idle, egui::Checkbox::new(&mut app.settings.stream.record_copy, t("Record a copy"))).on_hover_text(t("Also save what is streamed as an FLV video in the recordings folder"));
            ui.add_enabled(idle, egui::Checkbox::new(&mut app.settings.stream.cursor, t("Mouse pointer"))).on_hover_text(t("Show the mouse pointer in the stream"));
            let a = app.settings.stream.audio;
            let icon = if a == crate::audio::AudioSource::None { ph::SPEAKER_SLASH } else if a.mic() { ph::MICROPHONE } else { ph::SPEAKER };
            look::small_menu(ui, icon, t(a.label()), t("Sound to stream"), |ui| {
                if !crate::audio::AudioSource::supported() {
                    ui.label(RichText::new(t("There's no computer sound on this system yet, so the stream's audio track is silent.")).color(pal().dim));
                    return;
                }
                for v in crate::audio::AudioSource::ALL {
                    ui.radio_value(&mut app.settings.stream.audio, v, t(v.label()));
                }
            });
        });
    });
    look::group(ui, t("Status"), |ui| app.stream_status(ui));
}

/// Ctrl (Cmd on a Mac) + 1..9: the scene to switch to, when one of those was pressed.
pub(crate) fn scene_key(ctx: &egui::Context) -> Option<usize> {
    use egui::Key::*;
    let keys = [Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9];
    ctx.input(|i| if i.modifiers.command { keys.iter().position(|k| i.key_pressed(*k)) } else { None })
}

/// Switch Studio to scene `i` (the stream and recording follow at once, sound fading).
pub(crate) fn switch_scene(app: &mut SnagApp, i: usize) {
    if app.settings.studio.set_current(i).is_ok() {
        if let Some(s) = &mut app.x.studio {
            s.select_none();
        }
        let name = app.settings.studio.scenes[i].name.clone();
        app.status = tf("Scene: {name}", &[("name", &name)]);
    }
}

/// The Studio scenes as buttons, three to a column ("1 Intro"), and Edit in Studio.
fn scene_buttons(app: &mut SnagApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    let names: Vec<String> = app.settings.studio.scenes.iter().map(|s| s.name.clone()).collect();
    let cur = app.settings.studio.current.min(names.len().saturating_sub(1));
    let all: Vec<usize> = (0..names.len()).collect();
    let mut pick = None;
    for chunk in all.chunks(3) {
        look::column(ui, |ui| {
            for &i in chunk {
                let label = format!("{}  {}", i + 1, names[i].chars().take(16).collect::<String>());
                let tip = if i < 9 { tf("Switch to this scene (Ctrl+{n})", &[("n", &(i + 1).to_string())]) } else { t("Switch to this scene").to_string() };
                if ui.add(egui::Button::new(label).selected(i == cur).min_size(vec2(112.0, 0.0))).on_hover_text(tip).clicked() {
                    pick = Some(i);
                }
            }
        });
    }
    if let Some(i) = pick {
        switch_scene(app, i);
    }
    look::column(ui, |ui| {
        if look::small_tool(ui, ph::FILM_SLATE, t("Edit in Studio"), t("Arrange the scenes' sources and sound in Studio"), true).clicked() {
            app.switch_tab(ctx, super::ribbon::RibbonTab::Studio);
        }
    });
}

/// The current scene's sounds as buttons, three to a column: click to mute or turn on, with
/// its level under it. (Volumes are set in Studio.)
fn sound_buttons(app: &mut SnagApp, ui: &mut egui::Ui) {
    let st = &app.settings.studio;
    let c = st.current.min(st.scenes.len().saturating_sub(1));
    let rows: Vec<(bool, usize, crate::studio::SceneSound)> = st
        .shared_sounds
        .iter()
        .enumerate()
        .map(|(i, s)| (true, i, s.clone()))
        .chain(st.scenes.get(c).into_iter().flat_map(|sc| sc.sounds.iter().enumerate().map(|(i, s)| (false, i, s.clone()))))
        .collect();
    if rows.is_empty() {
        look::column(ui, |ui| {
            ui.label(RichText::new(t("No sound in this scene")).color(pal().dim));
        });
        return;
    }
    let mut flip = None;
    for chunk in rows.chunks(3) {
        look::column(ui, |ui| {
            for (shared, i, s) in chunk {
                let name = crate::i18n::tr(&s.name).chars().take(14).collect::<String>();
                let label = format!("{}  {name}", super::studio_ui::sound_icon(&s.input, s.muted));
                let r = ui.add(egui::Button::new(label).selected(!s.muted).min_size(vec2(118.0, 0.0))).on_hover_text(if s.muted { t("Muted; click to turn on") } else { t("Click to mute (volumes are set in Studio)") });
                if r.clicked() {
                    flip = Some((*shared, *i));
                }
                // Its level, under the button.
                if let Some(level) = app.studio_level(&s.input).filter(|_| !s.muted) {
                    let f = super::studio_ui::meter_fill(level, s.volume);
                    let y = r.rect.bottom() - 2.0;
                    ui.painter().line_segment([pos2(r.rect.left() + 3.0, y), pos2(r.rect.left() + 3.0 + (r.rect.width() - 6.0) * f, y)], Stroke::new(2.0, super::studio_ui::meter_color_on(f, look::pal().dark)));
                }
            }
        });
    }
    if let Some((shared, i)) = flip {
        let st = &mut app.settings.studio;
        let s = if shared { st.shared_sounds.get_mut(i) } else { st.scenes.get_mut(c).and_then(|sc| sc.sounds.get_mut(i)) };
        if let Some(s) = s {
            s.muted = !s.muted;
        }
    }
    ui.ctx().request_repaint_after(Duration::from_millis(100));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_and_rate_read_well() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(65.4), "1:05");
        assert_eq!(clock(3725.0), "1:02:05");
        assert_eq!(clock(-3.0), "0:00");
        assert_eq!(rate(4_500_000), "4.5 Mbit/s");
        assert_eq!(rate(800_000), "800 kbit/s");
        assert_eq!(rate(0), "0 kbit/s");
    }

    #[test]
    fn the_state_follows_the_phase_and_the_connection() {
        let mut s = Snapshot::default();
        assert_eq!(state_label(&s), "Off the air");
        s.phase = Phase::Starting;
        assert_eq!(state_label(&s), "Connecting…");
        s.phase = Phase::Running;
        s.stats = Some(crate::stream::engine::Stats { state: State::Live, ..Default::default() });
        assert_eq!(state_label(&s), "Live");
        assert!(s.live() && s.on_air());
        s.stats = Some(crate::stream::engine::Stats { state: State::Reconnecting, ..Default::default() });
        assert_eq!(state_label(&s), "Reconnecting…");
        assert!(!s.live() && s.on_air());
        s.phase = Phase::Stopping;
        assert_eq!(state_label(&s), "Stopping…");
        assert!(!s.on_air());
    }

    #[test]
    fn several_services_are_named_together() {
        assert_eq!(services_named(Service::Twitch, std::iter::empty()), "Twitch");
        assert_eq!(services_named(Service::Twitch, [Service::YouTube, Service::Custom].into_iter()), "Twitch + YouTube + Custom server");
        assert_eq!(output_word(State::Failed), ("The connection failed", true));
        assert_eq!(output_word(State::Live), ("Live", false));
    }

    #[test]
    fn an_extra_service_without_a_key_stops_going_live_and_opens_the_settings() {
        let ctx = egui::Context::default();
        let mut a = SnagApp::blank(Settings::default(), false, None);
        a.set_stream_key("live_abc");
        let id = a.settings.stream.new_also_id();
        a.settings.stream.also.push(crate::settings::ExtraService { id, service: Service::YouTube, url: String::new(), on: true });
        a.go_live(&ctx);
        assert!(a.status.starts_with("Can't go live yet: YouTube: "), "{}", a.status);
        assert!(a.settings_open && !a.x.stream.confirm_open);
        // Its key, kept apart from the main one (in memory in check runs).
        a.set_also_key(id, " yt_key ");
        assert_eq!((a.also_key(id), a.stream_key()), ("yt_key".to_string(), "live_abc".to_string()));
        assert_eq!(a.live_options().also[0].key, "yt_key");
        a.go_live(&ctx);
        assert!(a.x.stream.confirm_open, "with every key it asks first");
        // Switched off, it isn't streamed to.
        a.settings.stream.also[0].on = false;
        assert!(a.live_options().also.is_empty());
    }

    /// Ctrl (Cmd) + 1..9 picks a scene; a plain number doesn't (the editor zooms with it).
    #[test]
    fn ctrl_and_a_number_switches_scenes() {
        let pressed = |key: egui::Key, command: bool| {
            let ctx = egui::Context::default();
            let mods = if command { egui::Modifiers::COMMAND } else { egui::Modifiers::NONE };
            let raw = egui::RawInput { events: vec![egui::Event::ModifiersChanged(mods), egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: mods }], ..Default::default() };
            let mut got = None;
            let mut out = ctx.run_ui(raw, |ui| got = scene_key(ui.ctx()));
            out.textures_delta.clear();
            got
        };
        assert_eq!(pressed(egui::Key::Num1, true), Some(0));
        assert_eq!(pressed(egui::Key::Num9, true), Some(8));
        assert_eq!(pressed(egui::Key::Num3, false), None, "a plain 3 is the editor's");
        assert_eq!(pressed(egui::Key::Num0, true), None);
        let mut a = SnagApp::blank(Settings::default(), false, None);
        a.settings.studio.add_scene(Some("Game"));
        switch_scene(&mut a, 1);
        assert_eq!((a.settings.studio.current, a.status.as_str()), (1, "Scene: Game"));
        switch_scene(&mut a, 5);
        assert_eq!(a.settings.studio.current, 1, "no such scene: nothing changes");
    }

    fn live_snap(outputs: &[(&str, State)], warnings: Vec<Warning>) -> Snapshot {
        use crate::stream::engine::OutputStats;
        use crate::stream::health::{Health, OutHealth};
        let outs = outputs.iter().map(|(s, st)| OutputStats { service: s.to_string(), state: *st, error: Some(format!("{s} said no")), ..Default::default() }).collect();
        let hs = outputs.iter().map(|(s, _)| OutHealth { service: s.to_string(), measured: true, sent_kbps: 4000, ..Default::default() }).collect();
        Snapshot { phase: Phase::Running, stats: Some(Stats { state: State::Live, outputs: outs, health: Health { set_kbps: 6000, outputs: hs, warnings, ..Default::default() }, ..Default::default() }), ..Default::default() }
    }

    #[test]
    fn the_health_line_says_the_warning_in_a_few_words_with_its_advice() {
        use crate::stream::health::Problem;
        let up = Warning { problem: Problem::Upload, level: Level::Bad, service: Some("Twitch".into()) };
        let snap = live_snap(&[("Twitch", State::Live)], vec![up.clone()]);
        assert_eq!(health_line(&snap, None), None, "nothing shown yet");
        let l = health_line(&snap, Some(&up)).unwrap();
        assert_eq!((l.short.as_str(), l.bad), ("Upload to Twitch too slow", true));
        assert_eq!(l.advice, "Your upload to Twitch can't keep up: lower the bitrate or the resolution.");
        let pc = Warning { problem: Problem::Computer, level: Level::Warning, service: None };
        let l = health_line(&snap, Some(&pc)).unwrap();
        assert_eq!((l.short.as_str(), l.bad), ("Computer too slow to encode", false));
        // Multistreaming: a service that is reconnecting is bad, with its own error.
        let snap = live_snap(&[("Twitch", State::Live), ("YouTube", State::Reconnecting)], vec![]);
        let l = health_line(&snap, None).unwrap();
        assert_eq!((l.short.as_str(), l.advice.as_str(), l.bad), ("YouTube: Reconnecting…", "YouTube said no", true));
        // Off the air: nothing.
        assert_eq!(health_line(&Snapshot::default(), Some(&up)), None);
    }

    #[test]
    fn rates_and_shares_read_well() {
        assert_eq!(percent(0.004), "0.4%");
        assert_eq!(percent(0.0), "0.0%");
        assert_eq!(percent(0.123), "12%");
        assert_eq!(percent(-1.0), "0.0%");
        let snap = live_snap(&[("Twitch", State::Live)], vec![]);
        let s = snap.stats.unwrap();
        assert_eq!(sent_of_set(&s), "4.0 Mbit/s of 6.0 Mbit/s");
        let two = live_snap(&[("Twitch", State::Live), ("Kick", State::Live)], vec![]).stats.unwrap();
        assert_eq!(sent_of_set(&two), "8.0 Mbit/s of 12.0 Mbit/s", "every service it goes to");
        let mut early = s.clone();
        early.health.outputs[0].measured = false;
        assert_eq!(sent_of_set(&early), "Measuring…");
    }

    /// The window's tick: the warning is held steady, and a new bad one goes to the status
    /// line once (screen readers and the panel see it there).
    #[test]
    fn a_new_bad_warning_reaches_the_status_line_once() {
        use crate::stream::health::Problem;
        let mut a = SnagApp::blank(Settings::default(), false, None);
        let warn = Warning { problem: Problem::Upload, level: Level::Warning, service: Some("Twitch".into()) };
        let snap = live_snap(&[("Twitch", State::Live)], vec![warn.clone()]);
        a.status = String::new();
        a.health_tick(0.0, &snap);
        assert!(a.x.stream.health_line.is_none(), "a warning waits three seconds");
        a.health_tick(3.0, &snap);
        assert_eq!(a.x.stream.health_line.as_ref().map(|l| l.bad), Some(false));
        assert!(a.status.is_empty(), "a warning doesn't take the status line: {}", a.status);
        let bad = Warning { level: Level::Bad, ..warn };
        a.health_tick(3.5, &live_snap(&[("Twitch", State::Live)], vec![bad]));
        assert_eq!(a.status, "Your upload to Twitch can't keep up: lower the bitrate or the resolution.");
        a.status = "something else".into();
        a.health_tick(4.0, &live_snap(&[("Twitch", State::Live)], vec![Warning { problem: Problem::Upload, level: Level::Bad, service: Some("Twitch".into()) }]));
        assert_eq!(a.status, "something else", "said once");
        // The stream ends: the line goes at once (and other news in the status line stays).
        a.health_tick(5.0, &Snapshot::default());
        assert!(a.x.stream.health_line.is_none());
        assert_eq!(a.status, "something else");
        // The problem clears while the advice is still in the status line: it says so.
        let bad = Warning { problem: Problem::Computer, level: Level::Bad, service: None };
        a.health_tick(6.0, &live_snap(&[("Twitch", State::Live)], vec![bad]));
        assert_eq!(a.status, "Your computer can't encode fast enough: lower the resolution or the frame rate.");
        a.health_tick(15.0, &live_snap(&[("Twitch", State::Live)], vec![]));
        assert!(a.x.stream.health_line.is_some(), "it stays ten seconds");
        a.health_tick(16.0, &live_snap(&[("Twitch", State::Live)], vec![]));
        assert_eq!(a.status, "The stream is fine again.");
    }

    #[test]
    fn only_custom_server_is_translated() {
        assert_eq!(service_name(Service::Twitch), "Twitch");
        assert_eq!(service_name(Service::Custom), "Custom server");
    }

    #[test]
    fn a_check_run_never_reads_or_writes_the_saved_key() {
        let mut a = SnagApp::blank(Settings::default(), false, None);
        assert_eq!(a.stream_key(), "");
        a.set_stream_key("  in_memory_only ");
        assert_eq!(a.stream_key(), "in_memory_only");
    }

    #[test]
    fn going_live_without_a_key_opens_the_streaming_settings() {
        let ctx = egui::Context::default();
        let mut a = SnagApp::blank(Settings::default(), false, None);
        a.go_live(&ctx);
        assert!(a.settings_open && a.settings_tab == SettingsTab::Streaming);
        assert!(a.status.contains("stream key"), "{}", a.status);
        assert!(!a.x.stream.confirm_open);
        // With a key it asks first, once.
        a.set_stream_key("live_abc");
        a.go_live(&ctx);
        assert!(a.x.stream.confirm_open, "the first time it asks");
    }

    #[test]
    fn a_picked_area_becomes_the_source_only_when_it_was_asked_for() {
        let ctx = egui::Context::default();
        let mut a = SnagApp::blank(Settings::default(), false, None);
        assert!(!a.stream_area_picked(&ctx, (1, 2, 300, 200)));
        assert_eq!(a.settings.stream.source, Source::Screen);
        a.x.stream.picking_area = Some(false);
        assert!(a.stream_area_picked(&ctx, (1, 2, 300, 200)));
        assert_eq!(a.settings.stream.source, Source::Area(1, 2, 300, 200));
        assert!(a.x.stream.picking_area.is_none());
    }
}
