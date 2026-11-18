//! The capture panel: a compact window (opened from the tray icon) with All-in-one, Image and
//! Video tabs, what to capture, an effect, where to send it, a few switches and a big
//! Capture button.

use crate::app::{accent, SnagApp};
use crate::theme::pal;
use crate::capture::{CaptureKind, CutShape, PickPurpose};
use crate::effects::Effect;
use crate::settings::{Action, PanelSelection, PanelSettings, PanelShare, Preset, PresetMode};
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};

pub const PANEL_SIZE: Vec2 = Vec2::new(780.0, 236.0);
const CAPTURE_RED: Color32 = Color32::from_rgb(200, 44, 52);

/// A small on/off switch.
fn switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(40.0, 22.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let p = pal();
    let bg = p.border.lerp_to_gamma(p.accent, t);
    ui.painter().rect_filled(rect, CornerRadius::same(11), bg);
    let x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), t);
    let knob = if *on { p.on_accent } else { p.panel };
    ui.painter().circle_filled(pos2(x, rect.center().y), 8.0, knob);
    resp
}

fn tab_icon(p: &egui::Painter, c: Pos2, tab: u8, color: Color32) {
    match tab {
        0 => {
            p.circle_stroke(c, 8.0, Stroke::new(2.0, color));
            p.circle_filled(c, 4.0, color);
        }
        1 => {
            let r = Rect::from_center_size(c, vec2(18.0, 13.0));
            p.rect_filled(r, CornerRadius::same(3), color);
            p.circle_filled(c, 3.5, pal().bg);
            p.rect_filled(Rect::from_min_size(r.min + vec2(3.0, -3.0), vec2(6.0, 3.0)), CornerRadius::same(1), color);
        }
        3 => {
            p.text(c, Align2::CENTER_CENTER, crate::look::ph::BROADCAST, FontId::new(20.0, crate::look::icons()), color);
        }
        _ => {
            let r = Rect::from_min_size(c - vec2(10.0, 6.0), vec2(13.0, 12.0));
            p.rect_filled(r, CornerRadius::same(3), color);
            p.add(egui::Shape::convex_polygon(
                vec![pos2(r.max.x + 1.0, c.y), pos2(r.max.x + 7.0, c.y - 5.0), pos2(r.max.x + 7.0, c.y + 5.0)],
                color,
                Stroke::NONE,
            ));
        }
    }
}

impl SnagApp {
    pub(crate) fn panel_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let full = ui.max_rect();
        // Drag the panel by any empty spot.
        let bg = ui.interact(full, ui.scope_id().with("panel-bg"), Sense::click_and_drag());
        if bg.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.hide_panel(&ctx);
            return;
        }
        let tab = self.settings.panel.tab.min(3);
        let mut go_live = false;
        let mut capture_now = false;
        let mut open_editor = false;
        let mut reset = false;
        let mut close = false;

        // ---- left: tabs ----
        let tabs_rect = Rect::from_min_size(full.min, vec2(92.0, full.height() - 44.0));
        ui.painter().rect_filled(tabs_rect, CornerRadius { nw: 12, ne: 0, sw: 0, se: 0 }, pal().bg);
        let tab_h = tabs_rect.height() / 4.0;
        for (i, name) in ["All-in-One", "Image", "Video", crate::i18n::t("Stream")].iter().enumerate() {
            let r = Rect::from_min_size(tabs_rect.min + vec2(0.0, i as f32 * tab_h), vec2(92.0, tab_h));
            let resp = ui.interact(r, ui.scope_id().with(("tab", i)), Sense::click());
            let sel = tab as usize == i;
            if sel {
                ui.painter().rect_filled(r.shrink(4.0), CornerRadius::same(8), pal().panel_hi);
            } else if resp.hovered() {
                ui.painter().rect_filled(r.shrink(4.0), CornerRadius::same(8), pal().hover);
            }
            let col = if sel { accent() } else { pal().text };
            tab_icon(ui.painter(), r.center() - vec2(0.0, 7.0), i as u8, col);
            ui.painter().text(r.center() + vec2(0.0, 13.0), Align2::CENTER_CENTER, *name, FontId::proportional(12.0), col);
            if resp.clicked() {
                self.settings.panel.tab = i as u8;
            }
        }

        // ---- middle: what, effect, where ----
        let mid = Rect::from_min_size(full.min + vec2(108.0, 18.0), vec2(330.0, 150.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(mid), |ui| {
            if tab == 3 {
                self.panel_stream_grid(ui);
                return;
            }
            egui::Grid::new("panel-grid").num_columns(2).spacing(vec2(14.0, 14.0)).show(ui, |ui| {
                ui.label(RichText::new(crate::i18n::t("Selection:")).color(pal().strong));
                ui.horizontal(|ui| {
                    let p = &mut self.settings.panel;
                    if tab == 0 {
                        ui.add_enabled(false, egui::Button::new(crate::i18n::t("Area, window, menu or full screen")).min_size(vec2(220.0, 24.0)));
                    } else {
                        let list: &[PanelSelection] = if tab == 1 { &PanelSelection::IMAGE } else { &PanelSelection::VIDEO };
                        let cur = if tab == 1 { &mut p.selection } else { &mut p.video_selection };
                        if !list.contains(cur) {
                            *cur = list[0];
                        }
                        egui::ComboBox::from_id_salt("p-sel").width(196.0).selected_text(cur.label()).show_ui(ui, |ui| {
                            for s in list {
                                ui.selectable_value(cur, *s, s.label());
                            }
                        });
                    }
                    let fixed = tab == 1 && p.selection == PanelSelection::FixedSize;
                    ui.menu_button("...", |ui| {
                        ui.label(RichText::new(crate::i18n::t("Picking on the screen")).strong());
                        ui.checkbox(&mut self.settings.detect_objects, crate::i18n::t("Snap to buttons and menus"));
                        ui.checkbox(&mut self.settings.show_magnifier, crate::i18n::t("Magnifier"));
                        ui.checkbox(&mut self.settings.show_crosshair, crate::i18n::t("Crosshair"));
                        if fixed {
                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.label(crate::i18n::t("Size"));
                                ui.add(egui::DragValue::new(&mut self.settings.panel.fixed_w).range(16..=8000));
                                ui.label("x");
                                ui.add(egui::DragValue::new(&mut self.settings.panel.fixed_h).range(16..=8000));
                            });
                        }
                        if tab == 2 {
                            ui.separator();
                            if self.settings.rec_format == crate::video::VideoFormat::Gif {
                                ui.add(egui::Slider::new(&mut self.settings.gif_fps, 2..=30).text("frames per second"));
                                ui.add(egui::Slider::new(&mut self.settings.gif_max_s, 5..=300).suffix(" s").text("longest"));
                            } else {
                                ui.add(egui::Slider::new(&mut self.settings.video_fps, 1..=120).text("frames per second"));
                                ui.checkbox(&mut self.settings.video_hardware, crate::i18n::t("Use the graphics chip to encode"));
                            }
                        }
                    })
                    .response
                    .on_hover_text("More choices");
                });
                ui.end_row();

                if tab == 2 {
                    ui.label(RichText::new(crate::i18n::t("Format:")).color(pal().strong));
                    let f = &mut self.settings.rec_format;
                    egui::ComboBox::from_id_salt("p-format").width(230.0).selected_text(f.label()).show_ui(ui, |ui| {
                        for v in crate::video::VideoFormat::ALL {
                            if v.available() {
                                ui.selectable_value(f, v, v.label());
                            }
                        }
                    });
                    ui.end_row();
                }
                ui.label(RichText::new(crate::i18n::t("Effects:")).color(pal().strong));
                let p = &mut self.settings.panel;
                let shown = p.effect.map(|e| e.label()).unwrap_or("None");
                ui.add_enabled_ui(tab != 2, |ui| {
                    egui::ComboBox::from_id_salt("p-fx").width(230.0).selected_text(shown).show_ui(ui, |ui| {
                        ui.selectable_value(&mut p.effect, None, crate::i18n::t("None"));
                        for e in Effect::ALL {
                            ui.selectable_value(&mut p.effect, Some(e), e.label());
                        }
                    });
                });
                ui.end_row();

                ui.label(RichText::new(crate::i18n::t("Share:")).color(pal().strong));
                ui.add_enabled_ui(tab != 2, |ui| {
                    egui::ComboBox::from_id_salt("p-share").width(230.0).selected_text(p.share.label()).show_ui(ui, |ui| {
                        for s in PanelShare::ALL {
                            ui.selectable_value(&mut p.share, s, s.label());
                        }
                    });
                });
                ui.end_row();
            });
        });

        // ---- right: switches ----
        let sw = Rect::from_min_size(full.min + vec2(458.0, 14.0), vec2(200.0, 160.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(sw), |ui| {
            ui.spacing_mut().item_spacing.y = 9.0;
            let row = |ui: &mut egui::Ui, label: &str, on: &mut bool| {
                ui.horizontal(|ui| {
                    ui.set_width(196.0);
                    ui.label(RichText::new(label).color(pal().strong));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        switch(ui, on);
                    });
                });
            };
            if tab == 3 {
                row(ui, crate::i18n::t("Show the pointer"), &mut self.settings.stream.cursor);
                row(ui, crate::i18n::t("Record a copy"), &mut self.settings.stream.record_copy);
                return;
            }
            if tab == 2 {
                row(ui, "Preview in Editor", &mut self.settings.after_show_editor);
                row(ui, "Show the pointer", &mut self.settings.gif_cursor);
            } else {
                row(ui, "Preview in Editor", &mut self.settings.after_show_editor);
                row(ui, "Copy to Clipboard", &mut self.settings.after_copy);
                row(ui, "Capture Cursor", &mut self.settings.include_cursor);
            }
            row(ui, "Time Delay", &mut self.settings.panel.delay_on);
            if self.settings.panel.delay_on {
                ui.horizontal(|ui| {
                    ui.add_space(70.0);
                    ui.add(egui::DragValue::new(&mut self.settings.panel.delay_s).range(1..=120));
                    ui.label(RichText::new(crate::i18n::t("seconds")).color(pal().dim));
                });
            }
        });

        // ---- far right: reset, close and the big button ----
        let reset_r = Rect::from_center_size(pos2(full.max.x - 46.0, full.min.y + 20.0), vec2(22.0, 22.0));
        let rr = ui.interact(reset_r, ui.scope_id().with("reset"), Sense::click()).on_hover_text("Reset the panel's choices");
        let rc = if rr.hovered() { pal().strong } else { pal().dim };
        let arc: Vec<Pos2> = (0..=20)
            .map(|i| {
                let a = -0.6 + i as f32 / 20.0 * 5.0;
                reset_r.center() + vec2(a.cos(), a.sin()) * 7.0
            })
            .collect();
        let end = *arc.last().unwrap();
        ui.painter().add(egui::Shape::line(arc, Stroke::new(2.0, rc)));
        ui.painter().add(egui::Shape::convex_polygon(vec![end + vec2(-4.0, -3.0), end + vec2(3.0, -4.0), end + vec2(1.0, 3.0)], rc, Stroke::NONE));
        if rr.clicked() {
            reset = true;
        }
        let close_r = Rect::from_center_size(pos2(full.max.x - 18.0, full.min.y + 20.0), vec2(22.0, 22.0));
        let cr = ui.interact(close_r, ui.scope_id().with("close"), Sense::click()).on_hover_text("Hide the panel (Esc)");
        let cc = if cr.hovered() { pal().strong } else { pal().dim };
        ui.painter().line_segment([close_r.center() - vec2(5.0, 5.0), close_r.center() + vec2(5.0, 5.0)], Stroke::new(2.0, cc));
        ui.painter().line_segment([close_r.center() + vec2(-5.0, 5.0), close_r.center() + vec2(5.0, -5.0)], Stroke::new(2.0, cc));
        if cr.clicked() {
            close = true;
        }
        let big = Rect::from_center_size(pos2(full.max.x - 66.0, full.min.y + 100.0), vec2(100.0, 100.0));
        let br = ui.interact(big, ui.scope_id().with("big"), Sense::click());
        // Reds that keep the white label readable (4.5:1); hover and press go darker.
        let fill = if tab == 3 && self.x.stream.snap.on_air() {
            Color32::from_rgb(150, 30, 38)
        } else if br.is_pointer_button_down_on() {
            Color32::from_rgb(150, 30, 38)
        } else if br.hovered() {
            Color32::from_rgb(176, 36, 44)
        } else {
            CAPTURE_RED
        };
        ui.painter().circle_filled(big.center() + vec2(0.0, 3.0), 50.0, Color32::from_black_alpha(70));
        ui.painter().circle_filled(big.center(), 50.0, fill);
        let label = match tab {
            3 if self.x.stream.snap.phase != crate::stream::live::Phase::Idle => crate::i18n::t("Stop"),
            3 => crate::i18n::t("Go live"),
            2 if self.is_recording() => "Stop",
            2 => "Record",
            _ => "Capture",
        };
        ui.painter().text(big.center(), Align2::CENTER_CENTER, label, FontId::proportional(17.0), Color32::WHITE);
        let tip = if tab == 3 { crate::i18n::t("Start or stop the live stream") } else { "Print Screen does the same" };
        if br.on_hover_text(tip).clicked() {
            if tab == 3 {
                go_live = true;
            } else {
                capture_now = true;
            }
        }

        // ---- bottom row ----
        let bottom = Rect::from_min_max(pos2(full.min.x, full.max.y - 44.0), full.max);
        ui.painter().line_segment([bottom.left_top(), bottom.right_top()], Stroke::new(1.0, pal().line));
        ui.scope_builder(egui::UiBuilder::new().max_rect(bottom.shrink2(vec2(14.0, 8.0))), |ui| {
            ui.horizontal_centered(|ui| {
                let mut run = None;
                ui.menu_button(crate::i18n::t("Presets"), |ui| {
                    if self.settings.presets.is_empty() {
                        ui.label(RichText::new(crate::i18n::t("No presets yet")).color(pal().dim));
                    }
                    for (i, p) in self.settings.presets.iter().enumerate() {
                        let keys = if p.combo.is_set() { p.combo.label() } else { String::new() };
                        if ui.add(egui::Button::new(&p.name).shortcut_text(keys)).clicked() {
                            run = Some(i);
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button(crate::i18n::t("Edit presets…")).clicked() {
                        self.settings_open = true;
                        self.settings_tab = crate::app::SettingsTab::Presets;
                        ui.close();
                    }
                });
                if ui.button(" + ").on_hover_text("Save these choices as a preset").clicked() {
                    let preset = self.panel_as_preset();
                    self.status = format!("Saved the preset \"{}\".", preset.name);
                    self.settings.presets.push(preset);
                }
                if let Some(i) = run {
                    self.leave_panel(&ctx);
                    self.run_preset(&ctx, i);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(crate::i18n::t("Settings")).clicked() {
                        self.settings_open = true;
                        open_editor = true;
                    }
                    if ui.button(crate::i18n::t("Open Editor")).clicked() {
                        open_editor = true;
                    }
                    ui.label(RichText::new(&self.status).color(pal().dim).size(11.5));
                    let snap = self.x.stream.snap.clone();
                    if snap.on_air() && crate::app::stream_ui::live_badge(ui, &snap).clicked() {
                        self.settings.panel.tab = 3;
                    }
                });
            });
        });

        if reset {
            self.settings.panel = Default::default();
            self.settings.after_show_editor = true;
            self.settings.after_copy = false;
            self.settings.include_cursor = false;
        }
        if close {
            self.hide_panel(&ctx);
        } else if open_editor {
            self.leave_panel(&ctx);
            self.show_window(&ctx);
        } else if capture_now {
            self.panel_capture(&ctx);
        } else if go_live {
            self.toggle_stream(&ctx);
        }
    }

    /// The Stream tab's choices: what goes out and where.
    fn panel_stream_grid(&mut self, ui: &mut egui::Ui) {
        use crate::app::stream_ui::service_name;
        use crate::i18n::t;
        use crate::stream::Source;
        let idle = self.x.stream.snap.phase == crate::stream::live::Phase::Idle;
        egui::Grid::new("panel-stream-grid").num_columns(2).spacing(vec2(14.0, 14.0)).show(ui, |ui| {
            ui.label(RichText::new(t("Selection:")).color(pal().strong));
            let src = self.settings.stream.source.clone();
            let shown = match &src {
                Source::Window(w) if !w.is_empty() => w.clone(),
                s => t(s.label()).to_string(),
            };
            let mut pick_area = false;
            ui.add_enabled_ui(idle, |ui| {
                egui::ComboBox::from_id_salt("p-stream-src").width(230.0).selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.settings.stream.source, Source::Screen, t("Whole screen"));
                    if ui.selectable_label(matches!(src, Source::Area(..)), t("Pick an area…")).clicked() {
                        pick_area = true;
                    }
                    ui.selectable_value(&mut self.settings.stream.source, Source::Studio, t("Studio scene"));
                    ui.separator();
                    ui.label(RichText::new(t("A window")).color(pal().dim));
                    let ctx = ui.ctx().clone();
                    self.win_cache.get(Some(&ctx), crate::capture::list_windows);
                    for w in self.win_cache.items() {
                        ui.selectable_value(&mut self.settings.stream.source, Source::Window(w.label.clone()), &w.label);
                    }
                });
            });
            if pick_area {
                let ctx = ui.ctx().clone();
                self.pick_stream_area(&ctx, false);
            }
            ui.end_row();
            ui.label(RichText::new(t("Service:")).color(pal().strong));
            ui.add_enabled_ui(idle, |ui| {
                egui::ComboBox::from_id_salt("p-stream-service").width(230.0).selected_text(service_name(self.settings.stream.service)).show_ui(ui, |ui| self.service_menu(ui));
            });
            ui.end_row();
            ui.label(RichText::new(t("Status:")).color(pal().strong));
            let snap = self.x.stream.snap.clone();
            ui.horizontal(|ui| {
                ui.label(RichText::new(crate::app::stream_ui::state_label(&snap)).color(pal().dim));
                if ui.small_button(t("Stream settings…")).clicked() {
                    self.settings_open = true;
                    self.settings_tab = crate::app::SettingsTab::Streaming;
                    let ctx = ui.ctx().clone();
                    self.leave_panel(&ctx);
                }
            });
            ui.end_row();
        });
    }

    pub(crate) fn panel_as_preset(&self) -> Preset {
        preset_from_panel(&self.settings.panel, self.settings.after_copy)
    }
}

/// The panel's choices as a preset (`after_copy`: the "copy after capture" switch).
fn preset_from_panel(p: &PanelSettings, after_copy: bool) -> Preset {
    let mode = match (p.tab, p.selection) {
        (0, _) => PresetMode::Area,
        (_, PanelSelection::Window | PanelSelection::Menu) => PresetMode::Window,
        (_, PanelSelection::Screen) => PresetMode::Screen,
        (_, PanelSelection::AllScreens) => PresetMode::AllScreens,
        (_, PanelSelection::Scrolling | PanelSelection::Panorama) => PresetMode::Scrolling,
        (_, PanelSelection::Circle) => PresetMode::Circle,
        (_, PanelSelection::Freehand | PanelSelection::Polygon) => PresetMode::Freehand,
        (_, PanelSelection::Star) => PresetMode::Star,
        (_, PanelSelection::Heart) => PresetMode::Heart,
        (_, PanelSelection::FixedSize) => PresetMode::FixedSize,
        _ => PresetMode::Area,
    };
    let name = format!(
        "{}{}",
        if p.tab == 0 { "All-in-one" } else { p.selection.label() },
        p.effect.map(|e| format!(" + {}", e.label())).unwrap_or_default()
    );
    Preset {
        name,
        mode,
        delay_s: if p.delay_on { p.delay_s } else { 0 },
        width: p.fixed_w,
        height: p.fixed_h,
        effects: p.effect.into_iter().collect(),
        copy: after_copy || p.share == PanelShare::Clipboard,
        save: p.share == PanelShare::SaveFolder,
        pin: p.share == PanelShare::Pin,
        combo: Default::default(),
    }
}

impl SnagApp {
    fn panel_capture(&mut self, ctx: &egui::Context) {
        let p = self.settings.panel.clone();
        self.settings.delay_s = if p.delay_on { p.delay_s } else { 0 };
        self.leave_panel(ctx);
        if p.tab == 2 {
            if self.is_recording() {
                self.stop_job();
                return;
            }
            match p.video_selection {
                PanelSelection::Screen => {
                    let at = crate::capture::cursor_pos().unwrap_or((0, 0));
                    let mons = crate::capture::monitors();
                    if let Some(m) = mons
                        .iter()
                        .find(|m| at.0 >= m.x && at.1 >= m.y && at.0 < m.x + m.width as i32 && at.1 < m.y + m.height as i32)
                        .or(mons.first())
                    {
                        let r = (m.x, m.y, m.width, m.height);
                        self.start_record(ctx, r);
                    }
                }
                _ => self.start_capture(ctx, CaptureKind::Pick(CutShape::Rect, PickPurpose::RecordGif)),
            }
            return;
        }
        self.pending_panel = Some((p.effect, p.share));
        if p.tab == 0 {
            self.do_action(ctx, Action::AllInOne);
            return;
        }
        let pick = |s| CaptureKind::Pick(s, PickPurpose::Capture);
        match p.selection {
            PanelSelection::Region => self.start_capture(ctx, pick(CutShape::Rect)),
            PanelSelection::Window | PanelSelection::Menu => self.start_capture(ctx, CaptureKind::WindowPick),
            PanelSelection::Screen => self.start_capture(ctx, CaptureKind::Screen),
            PanelSelection::AllScreens => self.start_capture(ctx, CaptureKind::AllScreens),
            PanelSelection::Scrolling => self.do_action(ctx, Action::Scrolling),
            PanelSelection::Panorama => self.do_action(ctx, Action::Panorama),
            PanelSelection::Circle => self.start_capture(ctx, pick(CutShape::Ellipse)),
            PanelSelection::Freehand => self.start_capture(ctx, pick(CutShape::Freehand)),
            PanelSelection::Polygon => self.start_capture(ctx, pick(CutShape::Polygon)),
            PanelSelection::Star => self.start_capture(ctx, pick(CutShape::Star)),
            PanelSelection::Heart => self.start_capture(ctx, pick(CutShape::Heart)),
            PanelSelection::FixedSize => self.start_capture(ctx, CaptureKind::Fixed(p.fixed_w.max(8), p.fixed_h.max(8))),
            PanelSelection::Multi => self.do_action(ctx, Action::MultiArea),
            PanelSelection::MenuDelayed => self.do_action(ctx, Action::MenuCapture),
            PanelSelection::ScrollYourself => self.do_action(ctx, Action::ManualScroll),
            PanelSelection::GrabText => {
                self.pending_panel = None;
                self.do_action(ctx, Action::GrabText);
            }
        }
    }

    /// Draw the panel's outline (the window has no frame of its own while it shows).
    pub(crate) fn panel_frame() -> egui::Frame {
        egui::Frame::new().fill(pal().panel).stroke(Stroke::new(1.0, pal().border)).corner_radius(CornerRadius::same(12))
    }
}

#[allow(dead_code)]
fn unused(_: StrokeKind) {}

#[cfg(test)]
mod tests {
    #[test]
    fn the_capture_button_label_reads_on_every_red() {
        use crate::theme::contrast;
        use eframe::egui::Color32;
        for red in [super::CAPTURE_RED, Color32::from_rgb(176, 36, 44), Color32::from_rgb(150, 30, 38), crate::app::stream_ui::LIVE_RED] {
            assert!(contrast(Color32::WHITE, red) >= 4.5, "{red:?}");
        }
    }

    use super::*;

    fn image_tab(selection: PanelSelection) -> PanelSettings {
        PanelSettings { tab: 1, selection, ..Default::default() }
    }

    #[test]
    fn all_in_one_tab_is_an_area_preset() {
        let p = PanelSettings { tab: 0, selection: PanelSelection::Star, ..Default::default() };
        let pr = preset_from_panel(&p, false);
        assert_eq!(pr.mode, PresetMode::Area);
        assert_eq!(pr.name, "All-in-one");
    }

    #[test]
    fn every_selection_maps_to_a_preset_mode() {
        use PanelSelection as S;
        use PresetMode as M;
        let want = [
            (S::Region, M::Area),
            (S::Window, M::Window),
            (S::Menu, M::Window),
            (S::Screen, M::Screen),
            (S::AllScreens, M::AllScreens),
            (S::Scrolling, M::Scrolling),
            (S::Panorama, M::Scrolling),
            (S::Circle, M::Circle),
            (S::Freehand, M::Freehand),
            (S::Polygon, M::Freehand),
            (S::Star, M::Star),
            (S::Heart, M::Heart),
            (S::FixedSize, M::FixedSize),
            (S::Multi, M::Area),
            (S::MenuDelayed, M::Area),
            (S::ScrollYourself, M::Area),
            (S::GrabText, M::Area),
        ];
        assert_eq!(want.len(), PanelSelection::IMAGE.len());
        for (sel, mode) in want {
            let pr = preset_from_panel(&image_tab(sel), false);
            assert_eq!(pr.mode, mode, "{sel:?}");
            assert_eq!(pr.name, sel.label());
        }
    }

    #[test]
    fn the_effect_is_named_and_kept() {
        let p = PanelSettings { effect: Some(Effect::Border), ..image_tab(PanelSelection::Window) };
        let pr = preset_from_panel(&p, false);
        assert_eq!(pr.effects, vec![Effect::Border]);
        assert_eq!(pr.name, format!("Window + {}", Effect::Border.label()));
        assert!(preset_from_panel(&image_tab(PanelSelection::Window), false).effects.is_empty());
    }

    #[test]
    fn delay_counts_only_when_switched_on() {
        let mut p = PanelSettings { delay_s: 7, ..image_tab(PanelSelection::Region) };
        assert_eq!(preset_from_panel(&p, false).delay_s, 0);
        p.delay_on = true;
        assert_eq!(preset_from_panel(&p, false).delay_s, 7);
    }

    #[test]
    fn fixed_size_is_carried() {
        let p = PanelSettings { fixed_w: 640, fixed_h: 480, ..image_tab(PanelSelection::FixedSize) };
        let pr = preset_from_panel(&p, false);
        assert_eq!((pr.width, pr.height), (640, 480));
    }

    #[test]
    fn where_it_goes_sets_copy_save_and_pin() {
        let go = |share, after_copy| {
            let pr = preset_from_panel(&PanelSettings { share, ..image_tab(PanelSelection::Region) }, after_copy);
            (pr.copy, pr.save, pr.pin)
        };
        assert_eq!(go(PanelShare::None, false), (false, false, false));
        assert_eq!(go(PanelShare::Clipboard, false), (true, false, false));
        assert_eq!(go(PanelShare::SaveFolder, false), (false, true, false));
        assert_eq!(go(PanelShare::Pin, false), (false, false, true));
        assert_eq!(go(PanelShare::OpenWith, false), (false, false, false));
        // "Copy after capture" copies whatever else happens.
        assert_eq!(go(PanelShare::SaveFolder, true), (true, true, false));
    }

    #[test]
    fn a_preset_from_the_panel_has_no_hotkey() {
        let pr = preset_from_panel(&PanelSettings::default(), true);
        assert_eq!(pr.combo, Preset::default().combo);
    }
}
