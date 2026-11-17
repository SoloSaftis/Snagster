//! Snagster's extra windows: settings (with hotkeys and presets), library, GIF recordings,
//! timer, grabbed text, help, and pinned pictures.

use crate::app::{accent, IntervalJob, SettingsTab, SnagApp, TEXT_DIM, logo, rgb};
use crate::capture::CaptureKind;
use crate::doc::{Doc, Tool};
use crate::effects::Effect;
use crate::hotkeys::{self, Target};
use crate::platform::Choice;
use crate::settings::{Action, Combo, ImageFormat, Preset, PresetMode, Settings, Theme};
use crate::{export, library};
use eframe::egui::{self, Color32, CornerRadius, Key, PointerButton, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureOptions, pos2, vec2};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How a settings box or dialog opens: its own window on the desktop, centered on Snagster's.
pub(crate) struct Win<'a> {
    /// Stays the same while the box is open (the window's identity).
    pub key: &'a str,
    pub title: &'a str,
    pub size: egui::Vec2,
    pub resizable: bool,
}

/// Where a `size` box goes so it sits in the middle of `parent` (Snagster's window), kept on the
/// `monitor` (its top-left at 0,0) when that is known.
pub(crate) fn centered_on(parent: Rect, size: egui::Vec2, monitor: Option<egui::Vec2>) -> Pos2 {
    let mut p = parent.center() - size / 2.0;
    if let Some(m) = monitor {
        p.x = p.x.min(m.x - size.x).max(0.0);
        p.y = p.y.min(m.y - size.y).max(0.0);
    }
    p
}

/// Show a settings box or dialog in a window of its own (not inside Snagster's window), opened
/// in the middle of Snagster's window. Closing it (its X or Escape) sets `open` to false. Check
/// runs and systems without extra windows show it inside the main window as before.
pub(crate) fn own_window<R>(ctx: &egui::Context, w: Win, open: &mut bool, add: impl FnOnce(&mut egui::Ui) -> R) -> Option<R> {
    if !*open {
        ctx.data_mut(|d| d.remove::<Pos2>(egui::Id::unique(("own-window-at", w.key))));
        return None;
    }
    if ctx.embed_viewports() {
        let r = egui::Window::new(w.title)
            .id(egui::Id::unique(("own-window", w.key)))
            .open(open)
            .collapsible(false)
            .resizable(w.resizable)
            .default_size(w.size)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, add);
        return r.and_then(|r| r.inner);
    }
    // Where it opens is worked out once, so moving Snagster's window later doesn't drag it along.
    let at_id = egui::Id::unique(("own-window-at", w.key));
    let at = ctx.data(|d| d.get_temp::<Pos2>(at_id)).unwrap_or_else(|| {
        let (outer, monitor) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().monitor_size));
        let parent = outer.unwrap_or_else(|| ctx.content_rect());
        let p = centered_on(parent, w.size, monitor);
        ctx.data_mut(|d| d.insert_temp(at_id, p));
        p
    });
    let mut builder = egui::ViewportBuilder::default()
        .with_title(w.title)
        .with_inner_size(w.size)
        .with_min_inner_size(vec2(260.0, 120.0))
        .with_resizable(w.resizable)
        .with_maximize_button(w.resizable)
        .with_minimize_button(w.resizable)
        .with_position(at);
    static ICON: std::sync::OnceLock<std::sync::Arc<egui::IconData>> = std::sync::OnceLock::new();
    builder = builder.with_icon(ICON.get_or_init(|| std::sync::Arc::new(crate::icon())).clone());
    let mut add = Some(add);
    let (out, close) = ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(("own-window", w.key)), builder, |ui, _class| {
        let panel = egui::CentralPanel::default().show(ui, |ui| {
            let r = ui.scope(|ui| add.take().map(|f| f(ui)));
            (r.inner, r.response.rect.height())
        });
        let (out, used) = panel.inner;
        // A box that can't be resized grows or shrinks to what it holds.
        if !w.resizable {
            let want = (used + 16.0).max(80.0).ceil();
            let have = ui.ctx().input(|i| i.viewport().inner_rect.map(|r| r.height()));
            if have.is_some_and(|h| (h - want).abs() > 2.0) {
                let width = ui.ctx().input(|i| i.viewport().inner_rect.map_or(w.size.x, |r| r.width()));
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(width, want)));
            }
        }
        // Esc closes a secondary window everywhere; Ctrl+W (⌘W) too, as GNOME, KDE and macOS
        // expect.
        let close = ui.ctx().input(|i| i.viewport().close_requested() || i.key_pressed(Key::Escape) || (i.modifiers.command && i.key_pressed(Key::W)));
        (out, close)
    });
    if close {
        *open = false;
        ctx.data_mut(|d| d.remove::<Pos2>(at_id));
    }
    out
}

/// Modifier boxes and a key list for one hotkey.
fn combo_editor(ui: &mut egui::Ui, id: String, c: &mut Combo) -> bool {
    let before = c.clone();
    ui.checkbox(&mut c.ctrl, crate::i18n::t("Ctrl"));
    ui.checkbox(&mut c.alt, crate::i18n::t("Alt"));
    ui.checkbox(&mut c.shift, crate::i18n::t("Shift"));
    ui.checkbox(&mut c.win, crate::i18n::t("Win"));
    egui::ComboBox::from_id_salt(id)
        .width(110.0)
        .selected_text(if c.key.is_empty() { "Not set".to_string() } else { c.key.clone() })
        .show_ui(ui, |ui| {
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                ui.selectable_value(&mut c.key, String::new(), crate::i18n::t("Not set"));
                for (name, _, _) in hotkeys::KEYS {
                    ui.selectable_value(&mut c.key, name.to_string(), *name);
                }
            });
        });
    *c != before
}

fn color_edit(ui: &mut egui::Ui, c: &mut [u8; 3]) {
    let mut col = rgb(*c);
    if egui::color_picker::color_edit_button_srgba(ui, &mut col, egui::color_picker::Alpha::Opaque).changed() {
        *c = [col.r(), col.g(), col.b()];
    }
}

impl SnagApp {
    pub(crate) fn settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.settings_open;
        let mut reset = false;
        let mut run_preset = None;
        own_window(ctx, Win { key: "settings", title: crate::i18n::t("Settings"), size: vec2(720.0, 600.0), resizable: true }, &mut open, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (tab, label) in [
                        (SettingsTab::AfterCapture, crate::i18n::t("After capture")),
                        (SettingsTab::Saving, crate::i18n::t("Saving & library")),
                        (SettingsTab::Capture, crate::i18n::t("Capture")),
                        (SettingsTab::Recording, crate::i18n::t("Recording")),
                        (SettingsTab::Editor, crate::i18n::t("Editor & themes")),
                        (SettingsTab::Effects, crate::i18n::t("Effects")),
                        (SettingsTab::Redact, crate::i18n::t("Smart redact")),
                        (SettingsTab::Hotkeys, crate::i18n::t("Hotkeys")),
                        (SettingsTab::Presets, crate::i18n::t("Presets")),
                        (SettingsTab::Ai, crate::i18n::t("AI apps")),
                        (SettingsTab::Streaming, crate::i18n::t("Streaming")),
                        (SettingsTab::Language, crate::i18n::t("Language & updates")),
                    ] {
                        ui.selectable_value(&mut self.settings_tab, tab, label);
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| match self.settings_tab {
                    SettingsTab::AfterCapture => {
                        let s = &mut self.settings;
                        ui.label(crate::i18n::t("When a capture is taken:"));
                        ui.checkbox(&mut s.after_show_editor, crate::i18n::t("Show Snagster with the new picture"));
                        ui.checkbox(&mut s.after_library, crate::i18n::t("Add it to the library"));
                        ui.checkbox(&mut s.after_copy, crate::i18n::t("Copy the picture to the clipboard"));
                        ui.checkbox(&mut s.after_save, crate::i18n::t("Save it to the save folder"));
                        ui.checkbox(&mut s.after_pin, crate::i18n::t("Pin it on top of other windows"));
                        ui.checkbox(&mut s.include_cursor, crate::i18n::t("Include the mouse pointer as a movable mark"));
                        ui.add_space(8.0);
                        use crate::platform::by_system;
                        ui.label(RichText::new(crate::i18n::t(by_system("Taskbar corner", "Menu bar icon", "Tray icon"))).strong());
                        ui.checkbox(
                            &mut s.tray_icon,
                            crate::i18n::t(by_system(
                                "Show Snagster's icon in the taskbar corner (click: capture panel, right-click: quick menu)",
                                "Show Snagster's icon in the menu bar (click: capture panel, right-click: quick menu)",
                                "Show Snagster's icon in the tray (click: capture panel, right-click: quick menu)",
                            )),
                        );
                        ui.add_enabled(s.tray_icon, egui::Checkbox::new(&mut s.close_to_tray, crate::i18n::t("Closing the window keeps Snagster running there")));
                        if !crate::platform::has_tray() {
                            ui.label(RichText::new(crate::i18n::t("This desktop shows tray icons only with an extension, so closing the window ends Snagster.")).color(TEXT_DIM));
                        }
                        ui.add_space(8.0);
                        ui.label(RichText::new(crate::i18n::t("Starting Snagster")).strong());
                        egui::ComboBox::from_id_salt("startmode").selected_text(format!("Start with {}", s.start_mode.label())).show_ui(ui, |ui| {
                            for m in crate::settings::StartMode::ALL {
                                ui.selectable_value(&mut s.start_mode, m, format!("Start with {}", m.label()));
                            }
                        });
                        let mut on = *self.run_at_startup.get_or_insert_with(runs_at_startup);
                        let label = if cfg!(windows) { crate::i18n::t("Start Snagster when Windows starts") } else { crate::i18n::t("Start Snagster when you log in") };
                        if ui.checkbox(&mut on, label).changed() {
                            match set_run_at_startup(on) {
                                Ok(()) => self.run_at_startup = Some(on),
                                Err(e) => self.status = format!("Couldn't change that: {e}"),
                            }
                        }
                        ui.checkbox(&mut self.settings.allow_many, crate::i18n::t("Allow more than one Snagster at a time"))
                            .on_hover_text(crate::i18n::t("Off: opening Snagster again brings the running one to the front (and opens the file you chose there)."));
                        let s = &mut self.settings;
                        ui.horizontal(|ui| {
                            ui.label(crate::i18n::t("Menu capture waits"));
                            ui.add(egui::DragValue::new(&mut s.menu_delay_s).range(1..=30).suffix(" s"));
                            ui.label(crate::i18n::t("so you can open the menu"));
                        });
                    }
                    SettingsTab::Saving => self.saving_tab(ui),
                    SettingsTab::Capture => {
                        let s = &mut self.settings;
                        ui.horizontal(|ui| {
                            ui.label(crate::i18n::t("Delay before capturing"));
                            ui.add(egui::DragValue::new(&mut s.delay_s).range(0..=120).suffix(" s"));
                        });
                        ui.checkbox(&mut s.hide_window, crate::i18n::t("Hide Snagster while capturing"));
                        ui.add_enabled(s.hide_window, egui::Slider::new(&mut s.hide_settle_ms, 100..=1500).text("Wait for Snagster to hide (ms)"));
                        ui.add_space(6.0);
                        ui.label(RichText::new(crate::i18n::t("Picking on the screen")).strong());
                        ui.checkbox(&mut s.detect_windows, crate::i18n::t("Highlight windows under the mouse"));
                        ui.checkbox(&mut s.detect_objects, crate::i18n::t("Snap to buttons, menus and panels inside windows (Ctrl = whole window)"));
                        ui.checkbox(&mut s.show_magnifier, crate::i18n::t("Show the magnifier"));
                        ui.add_enabled(s.show_magnifier, egui::Slider::new(&mut s.magnifier_zoom, 2..=20).text("Magnifier zoom"));
                        ui.checkbox(&mut s.show_crosshair, crate::i18n::t("Show crosshair lines"));
                        ui.checkbox(&mut s.show_info, crate::i18n::t("Show position and size"));
                        ui.add(egui::Slider::new(&mut s.dim_alpha, 0..=220).text("Darken the rest of the screen"));
                        ui.add_space(6.0);
                        ui.label(RichText::new(crate::i18n::t("Scrolling capture")).strong());
                        ui.add(egui::Slider::new(&mut s.scroll_max_steps, 5..=150).text(crate::i18n::t("Most scroll steps")));
                        ui.add(egui::Slider::new(&mut s.scroll_wait_ms, 100..=1500).text(crate::i18n::t("Wait after each scroll (ms)")));
                        ui.horizontal(|ui| {
                            ui.label(crate::i18n::t("Scroll by"));
                            ui.add(egui::DragValue::new(&mut s.scroll_notches).range(1..=20));
                            ui.label(crate::i18n::t("wheel notches each time"));
                        });
                        ui.horizontal(|ui| {
                            ui.label(crate::i18n::t("Stop at a height of"));
                            ui.add(egui::DragValue::new(&mut s.scroll_max_height).range(1000..=60000).speed(100).suffix(" px"));
                        });
                        ui.checkbox(&mut s.scroll_from_top, crate::i18n::t("Scroll to the top first"));
                    }
                    SettingsTab::Recording => {
                        let encoders = self.x.encoders.get(Some(ui.ctx()), crate::video::encoders).to_vec();
                        let encoders_known = self.x.encoders.loaded();
                        let pickable = self.x.pickable.get(Some(ui.ctx()), crate::encode::available).to_vec();
                        let s = &mut self.settings;
                        ui.label(RichText::new(crate::i18n::t("Record as")).strong());
                        egui::ComboBox::from_id_salt("rec-format").width(240.0).selected_text(s.rec_format.label()).show_ui(ui, |ui| {
                            for f in crate::video::VideoFormat::ALL {
                                if f.available() {
                                    ui.selectable_value(&mut s.rec_format, f, f.label());
                                }
                            }
                        });
                        ui.add_space(6.0);
                        ui.label(RichText::new(crate::i18n::t("Video (MP4, HEVC, WMV)")).strong());
                        ui.horizontal(|ui| {
                            ui.add(egui::Slider::new(&mut s.video_fps, 1..=120).text("Frames per second"));
                        });
                        ui.horizontal(|ui| {
                            for fps in [15, 30, 60, 120] {
                                if ui.selectable_label(s.video_fps == fps, format!("{fps}")).clicked() {
                                    s.video_fps = fps;
                                }
                            }
                        });
                        egui::ComboBox::from_label(crate::i18n::t("Picture quality"))
                            .selected_text(["", "Small file", "Good", "High", "Best"][s.video_quality.clamp(1, 4) as usize])
                            .show_ui(ui, |ui| {
                                for (q, name) in [(1u8, "Small file"), (2, "Good"), (3, "High"), (4, "Best")] {
                                    ui.selectable_value(&mut s.video_quality, q, name);
                                }
                            });
                        ui.checkbox(&mut s.video_hardware, crate::i18n::t("Use the graphics chip to encode (Intel, AMD or NVIDIA)"));
                        encoder_picker(ui, "rec-encoder", &mut s.video_encoder, &pickable);
                        ui.horizontal(|ui| {
                            ui.label(crate::i18n::t("Videos go to"));
                            ui.add(egui::TextEdit::singleline(&mut s.video_folder).desired_width(300.0));
                            if ui.button(crate::i18n::t("Choose…")).clicked()
                                && let Some(p) = rfd::FileDialog::new().set_directory(&s.video_folder).pick_folder()
                            {
                                s.video_folder = p.display().to_string();
                            }
                        });
                        ui.add_space(4.0);
                        ui.label(RichText::new(crate::i18n::t("Sound")).strong());
                        if crate::audio::AudioSource::supported() {
                            ui.horizontal_wrapped(|ui| {
                                for v in crate::audio::AudioSource::ALL {
                                    ui.radio_value(&mut s.audio_source, v, crate::i18n::t(v.label()));
                                }
                            });
                            let mics = self.x.mics.get(Some(ui.ctx()), crate::audio::microphones).to_vec();
                            let shown = mics.iter().find(|m| m.id == s.audio_mic).map(|m| m.name.clone()).unwrap_or_else(|| crate::i18n::t("The computer's default microphone").to_string());
                            egui::ComboBox::from_label(crate::i18n::t("Microphone")).selected_text(shown).show_ui(ui, |ui| {
                                ui.selectable_value(&mut s.audio_mic, String::new(), crate::i18n::t("The computer's default microphone"));
                                for m in mics.iter() {
                                    ui.selectable_value(&mut s.audio_mic, m.id.clone(), &m.name);
                                }
                            });
                            ui.label(RichText::new(crate::i18n::t("Sound is recorded into MP4 videos (AAC, 48 kHz stereo). The volumes are set in Studio's mixer.")).color(TEXT_DIM));
                        } else {
                            ui.label(RichText::new(crate::i18n::t("Recording sound isn't available on this system yet.")).color(TEXT_DIM));
                        }
                        ui.add_space(4.0);
                        ui.add(egui::Slider::new(&mut s.video_max_s, 0..=7200).suffix(" s").text("Stop by itself after (0: never)"));
                        let (w, h) = (1920, 1080);
                        ui.label(
                            RichText::new(format!(
                                "A full-HD screen at these choices takes about {:.0} MB a minute. Frames above your screen's refresh rate repeat the last picture.",
                                crate::video::bitrate(w, h, s.video_fps, s.rec_format, s.video_quality) as f64 * 60.0 / 8.0 / 1_000_000.0
                            ))
                            .color(TEXT_DIM),
                        );
                        if let Some(list) = encoders_known.then_some(&encoders) {
                            let hw: Vec<String> = list.iter().filter(|e| e.hardware).map(|e| format!("{} ({})", e.name, e.codec)).collect();
                            let text = if hw.is_empty() {
                                "No graphics-chip encoder was found, so videos use the processor.".to_string()
                            } else {
                                format!("Graphics-chip encoders here: {}", hw.join(", "))
                            };
                            ui.label(RichText::new(text).color(TEXT_DIM));
                        }
                        ui.add_space(6.0);
                        ui.label(RichText::new(crate::i18n::t("Animated GIF")).strong());
                        let s = &mut self.settings;
                        ui.add(egui::Slider::new(&mut s.gif_fps, 2..=30).text("Frames per second"));
                        ui.add(egui::Slider::new(&mut s.gif_max_s, 5..=300).suffix(" s").text("Longest recording"));
                        ui.add(egui::Slider::new(&mut s.gif_max_width, 320..=1920).suffix(" px").text("Widest frame"));
                        ui.add_space(6.0);
                        ui.checkbox(&mut s.gif_cursor, crate::i18n::t("Show the mouse pointer in recordings"));
                        ui.add(egui::Slider::new(&mut s.slideshow_s, 0.2..=10.0).suffix(" s").text("Slideshow: time per picture"));
                        ui.label(RichText::new(crate::i18n::t("Videos and GIFs go to the Videos folder; pictures to the Pictures folder.")).color(TEXT_DIM));
                    }
                    SettingsTab::Editor => self.editor_tab(ui),
                    SettingsTab::Effects => {
                        let s = &mut self.settings;
                        ui.add(egui::Slider::new(&mut s.resize_percent, 10..=400).suffix(" %").text("Resize to"));
                        ui.add(egui::Slider::new(&mut s.border_width, 1..=100).suffix(" px").text("Border width"));
                        ui.horizontal(|ui| {
                            ui.label(crate::i18n::t("Border color"));
                            color_edit(ui, &mut s.border_color);
                        });
                        ui.add(egui::Slider::new(&mut s.shadow_size, 4..=80).suffix(" px").text("Drop shadow size"));
                        ui.add(egui::Slider::new(&mut s.rounded_radius, 2..=80).suffix(" px").text("Corner radius"));
                        ui.add(egui::Slider::new(&mut s.tilt_percent, 5..=60).suffix(" %").text("Perspective tilt"));
                        ui.add(egui::Slider::new(&mut s.brightness, -100..=100).text("Brightness"));
                        ui.add(egui::Slider::new(&mut s.contrast, -100..=100).text("Contrast"));
                        ui.add(egui::Slider::new(&mut s.saturation, -100..=100).text("Color strength"));
                        ui.horizontal(|ui| {
                            ui.label(crate::i18n::t("Watermark text"));
                            ui.text_edit_singleline(&mut s.watermark_text);
                        });
                    }
                    SettingsTab::Redact => {
                        let s = &mut self.settings;
                        ui.label(crate::i18n::t("Smart redact reads the picture and blurs what looks private:"));
                        ui.checkbox(&mut s.redact_emails, crate::i18n::t("Email addresses"));
                        ui.checkbox(&mut s.redact_phones, crate::i18n::t("Phone numbers"));
                        ui.checkbox(&mut s.redact_cards, crate::i18n::t("Card and account numbers"));
                        ui.checkbox(&mut s.redact_addresses, crate::i18n::t("Street addresses"));
                        ui.checkbox(&mut s.redact_ips, crate::i18n::t("IP addresses"));
                        ui.checkbox(&mut s.redact_urls, crate::i18n::t("Web addresses"));
                        ui.add(egui::Slider::new(&mut s.blur_block, 4..=48).suffix(" px").text("Blur block size"));
                        ui.label(RichText::new(crate::i18n::t("It uses the text recognition built into Windows; check the result before sharing.")).color(TEXT_DIM));
                    }
                    SettingsTab::Hotkeys => self.hotkeys_tab(ui),
                    SettingsTab::Presets => {
                        run_preset = self.presets_tab(ui);
                    }
                    SettingsTab::Ai => self.ai_tab(ui),
                    SettingsTab::Streaming => self.streaming_tab(ui),
                    SettingsTab::Language => self.language_tab(ui),
                });
                ui.add_space(8.0);
                ui.separator();
                if ui.button(crate::i18n::t("Reset everything to defaults")).clicked() {
                    reset = true;
                }
            });
        self.settings_open = open;
        if reset {
            self.settings = Settings::default();
        }
        if let Some(i) = run_preset {
            self.run_preset(ctx, i);
        }
    }

    fn saving_tab(&mut self, ui: &mut egui::Ui) {
        let library_before = self.settings.library_folder.clone();
        let s = &mut self.settings;
        ui.label(crate::i18n::t("Save folder"));
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut s.save_folder).desired_width(380.0));
            if ui.button(crate::i18n::t("Browse…")).clicked()
                && let Some(f) = rfd::FileDialog::new().pick_folder()
            {
                s.save_folder = f.display().to_string();
            }
        });
        ui.add_space(6.0);
        ui.label(crate::i18n::t("File name pattern"));
        ui.add(egui::TextEdit::singleline(&mut s.name_pattern).desired_width(380.0));
        ui.label(RichText::new(crate::i18n::t("%Y year  %m month  %d day  %H hour  %M minute  %S second  %n counter  %t title")).color(TEXT_DIM).size(12.0));
        let mut preview = s.clone();
        ui.label(RichText::new(format!("Example: {}.{}", preview.file_name("Capture"), s.format.ext())).color(TEXT_DIM));
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(crate::i18n::t("Format"));
            for f in ImageFormat::ALL {
                ui.radio_value(&mut s.format, f, f.label());
            }
        });
        ui.add_enabled(matches!(s.format, ImageFormat::Jpeg | ImageFormat::Pdf), egui::Slider::new(&mut s.jpeg_quality, 10..=100).text("JPEG / PDF quality"));
        ui.horizontal(|ui| {
            ui.label(crate::i18n::t("Next counter"));
            ui.add(egui::DragValue::new(&mut s.save_counter).range(1..=99999));
        });
        ui.add_space(10.0);
        ui.label(RichText::new(crate::i18n::t("Library")).strong());
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut s.library_folder).desired_width(380.0));
            if ui.button(crate::i18n::t("Browse…")).clicked()
                && let Some(f) = rfd::FileDialog::new().pick_folder()
            {
                s.library_folder = f.display().to_string();
            }
        });
        ui.label(RichText::new(crate::i18n::t("Keep the library in a cloud folder to have it on every computer:")).color(TEXT_DIM));
        ui.horizontal_wrapped(|ui| {
            let clouds = crate::settings::cloud_folders_now(Some(ui.ctx()));
            if clouds.is_empty() {
                ui.label(RichText::new(crate::i18n::t("No OneDrive, Dropbox, Google Drive, Box or iCloud folder was found.")).color(TEXT_DIM));
            }
            for (name, p) in clouds {
                if ui.button(format!("Use {name}")).clicked() {
                    s.library_folder = p.join("Snagster Library").display().to_string();
                }
            }
        });
        // Read the library again only when its folder changed (not every frame this tab shows).
        if self.settings.library_folder != library_before {
            self.lib_loaded = false;
        }
    }

    fn editor_tab(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.settings;
        ui.horizontal(|ui| {
            ui.label(crate::i18n::t("Drawing color"));
            color_edit(ui, &mut s.color);
        });
        ui.horizontal(|ui| {
            ui.label(crate::i18n::t("Palette"));
            for c in s.palette.iter_mut() {
                color_edit(ui, c);
            }
        });
        ui.add(egui::Slider::new(&mut s.width, 1.0..=24.0).step_by(1.0).text("Line width"));
        ui.add(egui::Slider::new(&mut s.corner_radius, 0.0..=60.0).step_by(1.0).text("Box corner radius"));
        ui.add(egui::Slider::new(&mut s.blur_block, 4..=48).text("Blur block size"));
        ui.add(egui::Slider::new(&mut s.magnify_zoom, 1.5..=6.0).text("Magnify zoom"));
        ui.horizontal(|ui| {
            ui.label(crate::i18n::t("Steps"));
            egui::ComboBox::from_id_salt("st").selected_text(s.step_style.label()).show_ui(ui, |ui| {
                for st in crate::settings::StepStyle::ALL {
                    ui.selectable_value(&mut s.step_style, st, st.label());
                }
            });
            ui.label(crate::i18n::t("start at"));
            ui.add(egui::DragValue::new(&mut s.step_start).range(1..=999));
        });
        ui.checkbox(&mut s.fill, crate::i18n::t("Fill boxes and ovals"));
        ui.checkbox(&mut s.shadow, crate::i18n::t("Drop shadow under marks"));
        ui.checkbox(&mut s.text_background, crate::i18n::t("Text on a colored box"));
        ui.horizontal(|ui| {
            ui.label(crate::i18n::t("Text font"));
            ui.label(RichText::new(if s.font.is_empty() { "Default" } else { &s.font }).strong());
            ui.label(RichText::new(crate::i18n::t("(pick it in the Text tool's panel)")).color(TEXT_DIM));
        });
        ui.add_space(10.0);
        ui.label(RichText::new(crate::i18n::t("Themes")).strong());
        ui.label(RichText::new(crate::i18n::t("A theme keeps the palette, font, text box and shadow choices together.")).color(TEXT_DIM));
        let mut apply = None;
        let mut delete = None;
        for (i, t) in s.themes.iter().enumerate() {
            ui.horizontal(|ui| {
                for c in t.palette {
                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                    ui.painter().rect_filled(r, CornerRadius::same(3), rgb(c));
                }
                ui.label(&t.name);
                if ui.small_button(crate::i18n::t("Use")).clicked() {
                    apply = Some(i);
                }
                if ui.small_button(crate::i18n::t("Delete")).clicked() {
                    delete = Some(i);
                }
            });
        }
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.new_theme_name).hint_text("Theme name").desired_width(180.0));
            if ui.button(crate::i18n::t("Save current as theme")).clicked() && !self.new_theme_name.trim().is_empty() {
                s.themes.push(Theme {
                    name: self.new_theme_name.trim().to_string(),
                    palette: s.palette,
                    font: s.font.clone(),
                    text_background: s.text_background,
                    shadow: s.shadow,
                });
                self.new_theme_name.clear();
            }
        });
        if let Some(i) = apply {
            let t = s.themes[i].clone();
            s.palette = t.palette;
            s.color = t.palette[0];
            s.font = t.font;
            s.text_background = t.text_background;
            s.shadow = t.shadow;
        }
        if let Some(i) = delete {
            s.themes.remove(i);
        }
    }

    fn hotkeys_tab(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.settings.hotkeys_enabled, crate::i18n::t("Use global hotkeys (they work while Snagster is in the background)"));
        ui.checkbox(&mut self.settings.override_hotkeys, crate::i18n::t("Take over keys another program already uses (for example Print Screen)"));
        ui.label(RichText::new(self.hotkeys.summary()).color(TEXT_DIM));
        let dups = hotkeys::duplicates(&self.settings);
        if !dups.is_empty() {
            ui.label(RichText::new(format!("Used twice (only the first one works): {}", dups.join(", "))).color(Color32::from_rgb(255, 170, 60)));
        }
        ui.add_space(4.0);
        let statuses: Vec<Option<bool>> = self.settings.bindings.iter().map(|b| self.hotkeys.status_of(Target::Action(b.action))).collect();
        egui::Grid::new("hk").striped(true).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
            for (i, b) in self.settings.bindings.iter_mut().enumerate() {
                ui.label(b.action.label());
                ui.horizontal(|ui| {
                    combo_editor(ui, format!("hk{i}"), &mut b.combo);
                });
                let (t, c) = match (b.combo.is_set(), statuses[i]) {
                    (false, _) => ("", TEXT_DIM),
                    (true, Some(true)) => ("Active", Color32::from_rgb(48, 164, 108)),
                    (true, Some(false)) => ("Taken by another app", Color32::from_rgb(229, 72, 77)),
                    (true, None) => ("Applying...", TEXT_DIM),
                };
                ui.label(RichText::new(t).color(c));
                ui.end_row();
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button(crate::i18n::t("Clear all")).clicked() {
                for b in &mut self.settings.bindings {
                    b.combo = Combo::default();
                }
            }
            if ui.button(crate::i18n::t("Back to the standard keys")).clicked() {
                self.settings.bindings = crate::settings::default_bindings();
            }
            let taken: Vec<usize> = self
                .settings
                .bindings
                .iter()
                .enumerate()
                .filter(|(_, b)| b.combo.is_set() && self.hotkeys.status_of(Target::Action(b.action)) == Some(false))
                .map(|(i, _)| i)
                .collect();
            if !taken.is_empty()
                && ui
                    .button(format!("Give the {} taken ones free keys", taken.len()))
                    .on_hover_text("Moves them to Ctrl + Shift + F1...F8, skipping keys Snagster already uses")
                    .clicked()
            {
                let mut free: Vec<Combo> = (1..=8)
                    .map(|n| Combo::new(true, false, true, &format!("F{n}")))
                    .filter(|c| !hotkeys::wanted(&self.settings).iter().any(|(_, w)| w == c))
                    .collect();
                for i in taken {
                    if free.is_empty() {
                        break;
                    }
                    self.settings.bindings[i].combo = free.remove(0);
                }
            }
        });
        ui.label(
            RichText::new(
                "Changes apply at once. If Print Screen shows as taken, turn off \"Use the Print screen key to open screen capture\" in Windows Settings > Accessibility > Keyboard.",
            )
            .color(TEXT_DIM),
        );
    }

    fn presets_tab(&mut self, ui: &mut egui::Ui) -> Option<usize> {
        ui.label(RichText::new(crate::i18n::t("A preset remembers how to capture, which effects to add, where it goes, and its own hotkey.")).color(TEXT_DIM));
        let (mut dup, mut export_one, mut import, mut export_all) = (None, None, false, false);
        let mut run = None;
        let mut delete = None;
        let n = self.settings.presets.len();
        for i in 0..n {
            let status = self.hotkeys.status_of(Target::Preset(i));
            let p = &mut self.settings.presets[i];
            egui::Frame::new().stroke(Stroke::new(1.0, Color32::from_gray(70))).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(220.0));
                    egui::ComboBox::from_id_salt(("pm", i)).selected_text(p.mode.label()).show_ui(ui, |ui| {
                        for m in PresetMode::ALL {
                            ui.selectable_value(&mut p.mode, m, m.label());
                        }
                    });
                    if p.mode == PresetMode::FixedSize {
                        ui.add(egui::DragValue::new(&mut p.width).range(16..=8000).suffix(" w"));
                        ui.add(egui::DragValue::new(&mut p.height).range(16..=8000).suffix(" h"));
                    }
                    ui.add(egui::DragValue::new(&mut p.delay_s).range(0..=60).suffix(" s delay"));
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(crate::i18n::t("Effects:"));
                    for e in Effect::ALL {
                        let mut on = p.effects.contains(&e);
                        if ui.checkbox(&mut on, e.label()).changed() {
                            if on {
                                p.effects.push(e);
                            } else {
                                p.effects.retain(|x| *x != e);
                            }
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut p.copy, crate::i18n::t("Copy"));
                    ui.checkbox(&mut p.save, crate::i18n::t("Save to folder"));
                    ui.checkbox(&mut p.pin, crate::i18n::t("Pin"));
                });
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::t("Hotkey"));
                    combo_editor(ui, format!("pk{i}"), &mut p.combo);
                    if p.combo.is_set() {
                        let (t, c) = match status {
                            Some(true) => ("Active", Color32::from_rgb(48, 164, 108)),
                            Some(false) => ("Taken", Color32::from_rgb(229, 72, 77)),
                            None => ("", TEXT_DIM),
                        };
                        ui.label(RichText::new(t).color(c));
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button(crate::i18n::t("Capture with this preset")).clicked() {
                        run = Some(i);
                    }
                    if ui.button(crate::i18n::t("Duplicate")).clicked() {
                        dup = Some(i);
                    }
                    if ui.button(crate::i18n::t("Export…")).clicked() {
                        export_one = Some(i);
                    }
                    if ui.button(crate::i18n::t("Delete")).clicked() {
                        delete = Some(i);
                    }
                });
            });
            ui.add_space(4.0);
        }
        if let Some(i) = dup {
            let mut p = self.settings.presets[i].clone();
            p.name = format!("{} copy", p.name);
            p.combo = Combo::default();
            self.settings.presets.insert(i + 1, p);
        }
        if let Some(i) = export_one {
            self.export_presets(Some(i));
        }
        if let Some(i) = delete {
            self.settings.presets.remove(i);
        }
        if ui.button(crate::i18n::t("Add a preset")).clicked() {
            self.settings.presets.push(Preset::default());
        }
        ui.horizontal(|ui| {
            if ui.button(crate::i18n::t("Import presets…")).clicked() {
                import = true;
            }
            if ui.button(crate::i18n::t("Export all presets…")).clicked() {
                export_all = true;
            }
        });
        if import {
            self.import_presets();
        }
        if export_all {
            self.export_presets(None);
        }
        run
    }

    pub(crate) fn help_windows(&mut self, ctx: &egui::Context) {
        let mut open = self.shortcuts_open;
        let s = self.settings.clone();
        own_window(ctx, Win { key: "shortcuts", title: crate::i18n::t("Keyboard shortcuts"), size: vec2(560.0, 560.0), resizable: true }, &mut open, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.label(RichText::new(crate::i18n::t("Anywhere (global hotkeys, change them in Settings > Hotkeys)")).strong());
                egui::Grid::new("gk").striped(true).spacing(vec2(16.0, 4.0)).show(ui, |ui| {
                    for b in s.bindings.iter().filter(|b| b.combo.is_set()) {
                        ui.label(RichText::new(b.combo.label()).strong());
                        ui.label(b.action.label());
                        ui.end_row();
                    }
                });
                ui.add_space(8.0);
                ui.label(RichText::new(crate::i18n::t("In Snagster")).strong());
                egui::Grid::new("keys").striped(true).spacing(vec2(16.0, 4.0)).show(ui, |ui| {
                    let rows: Vec<(String, String)> = [
                        ("Ctrl+Shift+A / W / F", "All-in-one / window / screen capture"),
                        (if crate::platform::system() == crate::platform::System::Windows { "Ctrl+Z, Ctrl+Y" } else { "Ctrl+Z, Ctrl+Shift+Z" }, "Undo, redo"),
                        ("Ctrl+C, Ctrl+V", "Copy picture, paste a picture"),
                        ("Ctrl+S, Ctrl+Shift+S, Ctrl+Alt+S", "Save, save as, quick save to folder"),
                        ("Ctrl+N, Ctrl+O, Ctrl+L", "New picture, open a picture, open the library"),
                        ("Ctrl+P, Ctrl+Shift+P", "Print, pin picture to screen"),
                        ("Ctrl+,", "Settings"),
                        ("Delete, [ and ]", "Delete, shrink or grow the selected mark"),
                        ("0, 1, +, -", "Fit, actual size, zoom in, zoom out"),
                        ("Ctrl+wheel, middle drag", "Zoom, move around"),
                        ("Shift while drawing", "Straight lines, squares, circles"),
                        ("On the frozen screen", "R E O F P S H T change the shape, Space = full screen, Ctrl = whole window"),
                    ]
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .chain(Tool::ALL.iter().map(|t| (t.key().to_string(), format!("{} tool", t.label()))))
                    .collect();
                    for (k, v) in rows {
                        ui.label(RichText::new(crate::platform::keys(&k)).strong());
                        ui.label(v);
                        ui.end_row();
                    }
                });
            });
        });
        self.shortcuts_open = open;
        let mut open = self.about_open;
        own_window(ctx, Win { key: "about", title: crate::i18n::t("About Snagster"), size: vec2(420.0, 300.0), resizable: false }, &mut open, |ui| {
            ui.horizontal(|ui| {
                logo(ui, 40.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(crate::i18n::t("Snagster")).size(22.0).strong());
                    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                });
            });
            ui.label(crate::i18n::t("Capture your screen, mark it up, share it. Written in Rust."));
            let ef = crate::emoji::font();
            ui.label(RichText::new(format!(
                "{} installed fonts, {} emojis{}.",
                crate::fonts::families().map(|f| f.len()).unwrap_or(0),
                ef.count(),
                if ef.color { " in color" } else { "" }
            ))
            .color(TEXT_DIM));
        });
        self.about_open = open;
    }

    pub(crate) fn text_window(&mut self, ctx: &egui::Context) {
        let Some(mut text) = self.grabbed_text.take() else { return };
        let mut open = true;
        own_window(ctx, Win { key: "grabbed-text", title: crate::i18n::t("Grabbed text"), size: vec2(560.0, 460.0), resizable: true }, &mut open, |ui| {
            ui.label(RichText::new(crate::i18n::t("Already copied. Edit it here if you like, then copy again.")).color(TEXT_DIM));
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut text).desired_width(f32::INFINITY).desired_rows(12));
            });
            if ui.button(crate::i18n::t("Copy")).clicked() {
                let _ = arboard::Clipboard::new().and_then(|mut c| c.set_text(text.clone()));
            }
        });
        if open {
            self.grabbed_text = Some(text);
        }
    }

    pub(crate) fn timer_window(&mut self, ctx: &egui::Context) {
        let mut open = self.timer_open;
        let mut start_interval = None;
        let mut set_schedule = None;
        let mut stop_interval = false;
        let mut cancel_schedule = false;
        own_window(ctx, Win { key: "timer", title: crate::i18n::t("Timer"), size: vec2(420.0, 360.0), resizable: false }, &mut open, |ui| {
            let s = &mut self.settings;
            ui.label(RichText::new(crate::i18n::t("Countdown")).strong());
            ui.horizontal(|ui| {
                ui.label(crate::i18n::t("Wait"));
                ui.add(egui::DragValue::new(&mut s.delay_s).range(0..=120).suffix(" s"));
                ui.label(crate::i18n::t("before every capture"));
            });
            ui.add_space(8.0);
            ui.label(RichText::new(crate::i18n::t("Repeat")).strong());
            ui.horizontal(|ui| {
                ui.label(crate::i18n::t("Every"));
                ui.add(egui::DragValue::new(&mut s.interval_s).range(2..=3600).suffix(" s"));
                ui.label(crate::i18n::t("for"));
                ui.add(egui::DragValue::new(&mut s.interval_count).range(1..=1000).suffix(" captures"));
            });
            let kinds = [
                ("Screen", CaptureKind::Screen),
                ("All screens", CaptureKind::AllScreens),
                ("Window in front", CaptureKind::ActiveWindow),
            ];
            ui.horizontal(|ui| {
                for (label, k) in &kinds {
                    if ui.button(*label).clicked() {
                        start_interval = Some(k.clone());
                    }
                }
                if let Some((x, y, w, h)) = self.last_region
                    && ui.button(crate::i18n::t("Last area")).clicked()
                {
                    start_interval = Some(CaptureKind::LastRegion(x, y, w, h));
                }
            });
            if let Some(job) = &self.interval {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} captures left", job.left)).color(accent()));
                    if ui.button(crate::i18n::t("Stop")).clicked() {
                        stop_interval = true;
                    }
                });
            }
            ui.add_space(8.0);
            ui.label(RichText::new(crate::i18n::t("Schedule")).strong());
            ui.horizontal(|ui| {
                ui.label(crate::i18n::t("At"));
                ui.add(egui::TextEdit::singleline(&mut s.schedule_time).desired_width(60.0));
                ui.label(crate::i18n::t("(hh:mm)"));
                for (label, k) in &kinds {
                    if ui.button(*label).clicked() {
                        set_schedule = Some(k.clone());
                    }
                }
            });
            if let Some((t, _)) = &self.schedule {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Capture scheduled at {}", t.format("%H:%M"))).color(accent()));
                    if ui.button(crate::i18n::t("Cancel")).clicked() {
                        cancel_schedule = true;
                    }
                });
            }
        });
        self.timer_open = open;
        if stop_interval {
            self.interval = None;
        }
        if cancel_schedule {
            self.schedule = None;
        }
        if let Some(kind) = start_interval {
            self.interval = Some(IntervalJob {
                kind,
                every: Duration::from_secs(self.settings.interval_s.max(2)),
                left: self.settings.interval_count.max(1),
                next: Instant::now() + Duration::from_secs(self.settings.delay_s.max(1)),
            });
            self.status = format!("Capturing every {} s, {} times.", self.settings.interval_s, self.settings.interval_count);
        }
        if let Some(kind) = set_schedule {
            match chrono::NaiveTime::parse_from_str(self.settings.schedule_time.trim(), "%H:%M") {
                Ok(t) => {
                    self.schedule = Some((t, kind));
                    self.status = format!("Capture scheduled at {}.", t.format("%H:%M"));
                }
                Err(_) => self.status = "Write the time as hh:mm, for example 14:30.".into(),
            }
        }
    }

    pub(crate) fn library_window(&mut self, ctx: &egui::Context) {
        if !self.library_open {
            return;
        }
        // Reading the library folder (maybe a cloud folder with thousands of files) is done on
        // a worker; the last list stays shown meanwhile.
        if !self.lib_loaded && self.lib_scan_rx.is_none() {
            self.lib_loaded = true;
            self.lib_added.clear();
            let folder = self.settings.library_folder.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            self.lib_scan_rx = Some((folder.clone(), rx));
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(library::scan(std::path::Path::new(&folder)));
                ctx2.request_repaint();
            });
        }
        let mut scanned = None;
        if let Some((folder, rx)) = &self.lib_scan_rx {
            match rx.try_recv() {
                Ok(items) => scanned = Some((folder.clone(), Some(items))),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => scanned = Some((folder.clone(), None)),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some((folder, items)) = scanned {
            self.lib_scan_rx = None;
            let added = std::mem::take(&mut self.lib_added);
            match items {
                Some(items) if folder == self.settings.library_folder => {
                    self.lib_items = library::merge_scan(items, added);
                    self.start_thumbnails(ctx);
                }
                // The folder changed while it was read: read the new one.
                _ => self.lib_loaded = false,
            }
        }
        if let Some(rx) = &self.lib_thumb_rx {
            for _ in 0..24 {
                let Ok((p, img)) = rx.try_recv() else { break };
                let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
                self.lib_thumbs.insert(p, ctx.load_texture("lib-thumb", ci, TextureOptions::LINEAR));
            }
        }
        self.library_ui(ctx);
    }

    /// Make the missing thumbnails on a worker; they're turned into textures a few a frame.
    fn start_thumbnails(&mut self, ctx: &egui::Context) {
        {
            let paths: Vec<PathBuf> =
                self.lib_items.iter().map(|i| i.path.clone()).filter(|p| !self.lib_thumbs.contains_key(p)).collect();
            let (tx, rx) = std::sync::mpsc::channel();
            self.lib_thumb_rx = Some(rx);
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                for p in paths {
                    if let Some(t) = library::thumbnail(&p, 192) {
                        if tx.send((p, t)).is_err() {
                            break;
                        }
                        ctx2.request_repaint();
                    }
                }
            });
        }
    }

    fn library_ui(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut open_item = None;
        let mut delete_item = None;
        let mut show_item = None;
        let mut save_tags = None;
        own_window(ctx, Win { key: "library", title: crate::i18n::t("Library"), size: vec2(900.0, 640.0), resizable: true }, &mut open, |ui| {
            ui.horizontal(|ui| {
                ui.label(crate::i18n::t("Search"));
                ui.add(egui::TextEdit::singleline(&mut self.lib_query).hint_text("title, program, tag, date (2026-11)").desired_width(260.0));
                egui::ComboBox::from_id_salt("libsort")
                    .selected_text(["Newest first", "Oldest first", "By program", "By title"][self.lib_sort])
                    .show_ui(ui, |ui| {
                        for (i, l) in ["Newest first", "Oldest first", "By program", "By title"].iter().enumerate() {
                            ui.selectable_value(&mut self.lib_sort, i, *l);
                        }
                    });
                let mut apps: Vec<String> = self.lib_items.iter().map(|i| i.meta.app.clone()).filter(|a| !a.is_empty()).collect();
                apps.sort_by_key(|a| a.to_lowercase());
                apps.dedup();
                egui::ComboBox::from_id_salt("libapp")
                    .selected_text(if self.lib_app.is_empty() { "All programs".to_string() } else { self.lib_app.clone() })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.lib_app, String::new(), crate::i18n::t("All programs"));
                        for a in apps {
                            ui.selectable_value(&mut self.lib_app, a.clone(), a);
                        }
                    });
                if ui.button(crate::i18n::t("Refresh")).clicked() {
                    self.lib_loaded = false;
                }
                if ui.button(crate::i18n::t("Open folder")).clicked() {
                    let f = PathBuf::from(&self.settings.library_folder);
                    self.open_folder(&f);
                }
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}  ({} captures)", self.settings.library_folder, self.lib_items.len())).color(TEXT_DIM).size(12.0));
                if self.lib_scan_rx.is_some() {
                    ui.spinner();
                }
            });
            ui.separator();
            let mut order: Vec<usize> = (0..self.lib_items.len()).filter(|i| self.lib_items[*i].matches(&self.lib_query)).collect();
            match self.lib_sort {
                1 => order.reverse(),
                2 => order.sort_by(|a, b| self.lib_items[*a].meta.app.to_lowercase().cmp(&self.lib_items[*b].meta.app.to_lowercase())),
                3 => order.sort_by(|a, b| self.lib_items[*a].meta.title.to_lowercase().cmp(&self.lib_items[*b].meta.title.to_lowercase())),
                _ => {}
            }
            if !self.lib_app.is_empty() {
                order.retain(|i| self.lib_items[*i].meta.app == self.lib_app);
            }
            if order.is_empty() {
                ui.label(RichText::new(crate::i18n::t("Nothing here yet. Captures are added automatically (Settings > After capture).")).color(TEXT_DIM));
            }
            // Group by month (date order) or by program.
            let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
            for &i in &order {
                let m = &self.lib_items[i].meta;
                let head = match self.lib_sort {
                    0 | 1 => m.created.get(..7).unwrap_or("Unknown date").to_string(),
                    2 => if m.app.is_empty() { "Unknown program".to_string() } else { m.app.clone() },
                    _ => String::new(),
                };
                match groups.last_mut() {
                    Some((h, v)) if *h == head => v.push(i),
                    _ => groups.push((head, vec![i])),
                }
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (head, members) in &groups {
                    if !head.is_empty() {
                        ui.add_space(4.0);
                        ui.label(RichText::new(format!("{head}   ({})", members.len())).strong().color(Color32::WHITE));
                    }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    for &i in members {
                        let item = &self.lib_items[i];
                        let (rect, resp) = ui.allocate_exact_size(vec2(192.0, 176.0), Sense::click());
                        let p = ui.painter();
                        p.rect_filled(rect, CornerRadius::same(8), if resp.hovered() { Color32::from_rgb(46, 49, 59) } else { Color32::from_rgb(28, 30, 36) });
                        let thumb_area = Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(180.0, 110.0));
                        if let Some(t) = self.lib_thumbs.get(&item.path) {
                            let sz = t.size_vec2();
                            let s = (thumb_area.width() / sz.x).min(thumb_area.height() / sz.y);
                            p.image(t.id(), Rect::from_center_size(thumb_area.center(), sz * s), Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
                        }
                        let title: String = item.meta.title.chars().take(26).collect();
                        let date = item.meta.created.get(..16).unwrap_or(&item.meta.created).replace('T', " ");
                        p.text(pos2(rect.min.x + 8.0, rect.min.y + 122.0), egui::Align2::LEFT_TOP, title, egui::FontId::proportional(12.5), Color32::WHITE);
                        p.text(
                            pos2(rect.min.x + 8.0, rect.min.y + 140.0),
                            egui::Align2::LEFT_TOP,
                            format!("{}  {}", date, item.meta.app.chars().take(14).collect::<String>()),
                            egui::FontId::proportional(11.0),
                            TEXT_DIM,
                        );
                        if !item.meta.tags.is_empty() {
                            p.text(pos2(rect.min.x + 8.0, rect.min.y + 156.0), egui::Align2::LEFT_TOP, format!("# {}", item.meta.tags.join(", ")), egui::FontId::proportional(11.0), accent());
                        }
                        if resp.double_clicked() || resp.clicked_by(PointerButton::Primary) && ui.input(|i| i.modifiers.ctrl) {
                            open_item = Some(i);
                        }
                        let resp = resp.on_hover_text("Double-click to open. Right-click for tags and more.");
                        resp.context_menu(|ui| {
                            if ui.button(crate::i18n::t("Open in Snagster")).clicked() {
                                open_item = Some(i);
                                ui.close();
                            }
                            if ui.button(crate::i18n::t("Edit tags…")).clicked() {
                                self.lib_tag_edit = Some((i, self.lib_items[i].meta.tags.join(", ")));
                                ui.close();
                            }
                            if ui.button(crate::i18n::t("Show in folder")).clicked() {
                                show_item = Some(i);
                                ui.close();
                            }
                            if ui.button(crate::i18n::t(crate::platform::trash_text("Move to Recycle Bin", "Move to Trash"))).clicked() {
                                delete_item = Some(i);
                                ui.close();
                            }
                        });
                    }
                });
                }
            });
            if let Some((i, text)) = &mut self.lib_tag_edit {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::t("Tags (comma separated)"));
                    ui.add(egui::TextEdit::singleline(text).desired_width(300.0));
                    if ui.button(crate::i18n::t("Save tags")).clicked() || ui.input(|inp| inp.key_pressed(Key::Enter)) {
                        save_tags = Some((*i, text.clone()));
                    }
                });
            }
        });
        if let Some((i, text)) = save_tags {
            if let Some(item) = self.lib_items.get_mut(i) {
                item.meta.tags = text.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
                if let Err(e) = item.save_meta() {
                    self.status = format!("Couldn't save the tags: {e}");
                }
            }
            self.lib_tag_edit = None;
        }
        if let Some(i) = open_item {
            let p = self.lib_items[i].path.clone();
            self.open_path(&p);
        }
        if let Some(i) = show_item {
            let p = self.lib_items[i].path.clone();
            self.open_folder(&p);
        }
        if let Some(i) = delete_item {
            // To the Recycle Bin / Trash, where it can be brought back (never deleted for good).
            match library::delete(&self.lib_items[i]) {
                Ok(()) => {
                    let s = self.lib_items[i].path.display().to_string();
                    self.status = crate::i18n::tf(crate::platform::trash_text("Moved {path} to the Recycle Bin.", "Moved {path} to the Trash."), &[("path", &s)]);
                    self.lib_items.remove(i);
                }
                Err(e) => self.status = crate::i18n::tf(crate::platform::trash_text("Couldn't move it to the Recycle Bin: {error}", "Couldn't move it to the Trash: {error}"), &[("error", &e)]),
            }
        }
        self.library_open = open;
    }

    pub(crate) fn gif_window(&mut self, ctx: &egui::Context) {
        if !self.gif_open {
            return;
        }
        let mut open = true;
        let mut load: Option<PathBuf> = None;
        let mut grab = None;
        let mut save_trim = false;
        let mut append = false;
        let mut slideshow = false;
        own_window(ctx, Win { key: "gifs", title: crate::i18n::t("GIF recordings"), size: vec2(740.0, 600.0), resizable: true }, &mut open, |ui| {
            ui.horizontal(|ui| {
                if ui.button(crate::i18n::t("Open a GIF…")).clicked()
                    && let Some(p) = rfd::FileDialog::new().add_filter("GIF", &["gif"]).pick_file()
                {
                    load = Some(p);
                }
                if ui.add_enabled(self.entries.len() >= 2, egui::Button::new(crate::i18n::t("Slideshow from Recent"))).clicked() {
                    slideshow = true;
                }
                ui.label(RichText::new(crate::i18n::t("Record with Record > Record an area as GIF")).color(TEXT_DIM));
            });
            if !self.recordings.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(crate::i18n::t("This session:"));
                    for r in &self.recordings {
                        let name = r.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                        if ui.button(format!("{name} ({} frames, {:.1} s)", r.frames, r.seconds)).clicked() {
                            load = Some(r.path.clone());
                        }
                    }
                });
            }
            ui.separator();
            let Some(ed) = &mut self.gif_editor else {
                ui.label(RichText::new(crate::i18n::t("No GIF open.")).color(TEXT_DIM));
                return;
            };
            let n = ed.frames.len();
            if ed.loading.is_some() {
                // The frames are still being read: show what's here, and that more come.
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(format!("{n}")).color(TEXT_DIM));
                });
            }
            if ed.playing && !ed.frames.is_empty() {
                let ms = ed.frames[ed.index].1.max(20) as u128;
                if ed.shown_at.elapsed().as_millis() >= ms {
                    ed.index = if ed.index >= ed.trim.1 || ed.index < ed.trim.0 { ed.trim.0 } else { ed.index + 1 };
                    ed.shown_at = Instant::now();
                }
                ui.ctx().request_repaint_after(Duration::from_millis(20));
            }
            let avail = vec2(ui.available_width(), 320.0);
            let s = (avail.x / ed.size.x).min(avail.y / ed.size.y).min(1.5);
            let (r, _) = ui.allocate_exact_size(ed.size * s, Sense::hover());
            if let Some((t, _)) = ed.frames.get(ed.index) {
                ui.painter().image(t.id(), r, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
            }
            ui.painter().rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, Color32::from_gray(80)), StrokeKind::Outside);
            ui.horizontal(|ui| {
                if ui.button(if ed.playing { "Pause" } else { "Play" }).clicked() {
                    ed.playing = !ed.playing;
                }
                let mut idx = ed.index;
                if ui.add(egui::Slider::new(&mut idx, 0..=n.saturating_sub(1)).text("frame")).changed() {
                    ed.index = idx;
                    ed.playing = false;
                }
            });
            ui.horizontal(|ui| {
                ui.label(crate::i18n::t("Keep frames"));
                let max = n.saturating_sub(1);
                ui.add(egui::DragValue::new(&mut ed.trim.0).range(0..=max));
                ui.label(crate::i18n::t("to"));
                ui.add(egui::DragValue::new(&mut ed.trim.1).range(0..=max));
                if ui.button(crate::i18n::t("Start here")).clicked() {
                    ed.trim.0 = ed.index;
                }
                if ui.button(crate::i18n::t("End here")).clicked() {
                    ed.trim.1 = ed.index;
                }
                if ed.trim.0 > ed.trim.1 {
                    std::mem::swap(&mut ed.trim.0, &mut ed.trim.1);
                }
            });
            ui.horizontal(|ui| {
                if ui.button(crate::i18n::t("Save trimmed GIF as…")).clicked() {
                    save_trim = true;
                }
                if ui.button(crate::i18n::t("Grab this frame as a picture")).clicked() {
                    grab = Some(ed.index);
                }
                if ui.button(crate::i18n::t("Add another GIF to the end…")).clicked() {
                    append = true;
                }
            });
            ui.label(RichText::new(format!("{}", ed.path.display())).color(TEXT_DIM).size(11.5));
        });
        self.gif_open = open;
        if slideshow {
            self.slideshow(ctx);
        }
        if let Some(p) = load {
            self.open_gif(ctx, p);
        }
        let Some(path) = self.gif_editor.as_ref().map(|e| e.path.clone()) else { return };
        // Reading a whole GIF at full size and writing one take seconds: they run on the job
        // queue and report in the status line.
        if let Some(i) = grab {
            let path = path.clone();
            self.jobs.queue(move || {
                let r = export::gif_frames(&path, 0);
                Box::new(move |app: &mut SnagApp, _: &egui::Context| match r {
                    Ok(frames) => {
                        if let Some((img, _)) = frames.into_iter().nth(i) {
                            let name = app.next_name("Frame");
                            app.add_entry(name, Doc::new(img));
                            app.status = format!("Frame {} is now a picture you can mark up.", i + 1);
                        }
                    }
                    Err(e) => app.status = format!("Couldn't read the frame: {e}"),
                })
            });
        }
        if save_trim {
            let (a, b) = self.gif_editor.as_ref().map(|e| e.trim).unwrap_or((0, 0));
            if let Some(out) = rfd::FileDialog::new().add_filter("GIF", &["gif"]).set_file_name("trimmed.gif").save_file() {
                let path = path.clone();
                self.jobs.queue(move || {
                    let r = export::trim_gif(&path, a, b, &out);
                    Box::new(move |app: &mut SnagApp, _: &egui::Context| {
                        app.status = match r {
                            Ok(_) =>format!("Saved frames {}-{} to {}", a + 1, b + 1, out.display()),
                            Err(e) => format!("Couldn't save: {e}"),
                        };
                    })
                });
            }
        }
        if append && let Some(other) = rfd::FileDialog::new().add_filter("GIF", &["gif"]).pick_file() {
            self.jobs.queue(move || {
                let r = export::combine_gifs(&path, &other);
                Box::new(move |app: &mut SnagApp, ctx: &egui::Context| match r {
                    Ok(out) => {
                        app.status = format!("Combined into {}", out.display());
                        app.open_gif(ctx, out);
                    }
                    Err(e) => app.status = format!("Couldn't combine: {e}"),
                })
            });
        }
    }
}


impl SnagApp {
    pub(crate) fn show_pins(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        for pin in &mut self.pins {
            if pin.close {
                continue;
            }
            let (iw, ih) = (pin.image.width() as f32, pin.image.height() as f32);
            let fit = (900.0 / iw).min(700.0 / ih).min(1.0);
            let size = vec2(iw * fit / ppp, ih * fit / ppp);
            let mut builder = egui::ViewportBuilder::default()
                .with_title(format!("Snagster pin: {}", pin.title))
                .with_inner_size(size)
                .with_decorations(false)
                .with_taskbar(false);
            if crate::app::UI_TEST.load(std::sync::atomic::Ordering::Relaxed) {
                builder = builder.with_active(false).with_window_level(egui::WindowLevel::AlwaysOnBottom);
            } else {
                builder = builder.with_always_on_top();
            }
            let image = &pin.image;
            let tex_slot = &mut pin.tex;
            let mut close = false;
            ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(("pin", pin.id)), builder, |ui, _class| {
                let tex = tex_slot
                    .get_or_insert_with(|| {
                        let ci = egui::ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw());
                        ui.ctx().load_texture("pin", ci, TextureOptions::LINEAR)
                    })
                    .id();
                let rect = ui.max_rect();
                let resp = ui.allocate_rect(rect, Sense::click_and_drag());
                ui.painter().image(tex, rect, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
                ui.painter().rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, accent()), StrokeKind::Inside);
                if resp.drag_started_by(PointerButton::Primary) {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                if resp.double_clicked() || ui.input(|i| i.key_pressed(Key::Escape) || i.viewport().close_requested()) {
                    close = true;
                }
                resp.on_hover_text("Drag to move. Double-click or Esc closes. Right-click for more.").context_menu(|ui| {
                    if ui.button(crate::i18n::t("Copy")).clicked() {
                        let data = arboard::ImageData {
                            width: image.width() as usize,
                            height: image.height() as usize,
                            bytes: image.as_raw().clone().into(),
                        };
                        let _ = arboard::Clipboard::new().and_then(|mut c| c.set_image(data));
                        ui.close();
                    }
                    if ui.button(crate::i18n::t("Close pin")).clicked() {
                        close = true;
                        ui.close();
                    }
                });
            });
            if close {
                pin.close = true;
            }
        }
        self.pins.retain(|p| !p.close);
    }
}

#[allow(dead_code)]
fn unused(_a: Action) {}

// ---------- small dialogs ----------

pub(crate) enum Dialog {
    NewImage { w: u32, h: u32, color: [u8; 4] },
    Resize { w: u32, h: u32, keep: bool, ratio: f32 },
    Canvas { w: u32, h: u32, color: [u8; 4] },
    Rename(String),
    Export { pick: Vec<bool>, folder: String, format: ImageFormat },
    GettingStarted,
    WhatsNew,
}

impl Dialog {
    pub fn new_image() -> Self {
        Dialog::NewImage { w: 1280, h: 720, color: [255, 255, 255, 255] }
    }
    pub fn export(n: usize, s: &Settings) -> Self {
        Dialog::Export { pick: vec![true; n], folder: s.save_folder.clone(), format: s.format }
    }
}

/// A dialog's main button (`action`, English) and Cancel, in this system's order.
fn dialog_row(ui: &mut egui::Ui, action: &'static str, enabled: bool) -> Option<Choice> {
    crate::platform::dialog_buttons(ui, Some(crate::i18n::t(action)), crate::i18n::t("Cancel"), crate::platform::Kind::Normal, enabled)
}

fn color4_edit(ui: &mut egui::Ui, c: &mut [u8; 4]) {
    let mut col = Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
    if egui::color_picker::color_edit_button_srgba(ui, &mut col, egui::color_picker::Alpha::OnlyBlend).changed() {
        *c = col.to_srgba_unmultiplied();
    }
}

impl SnagApp {
    pub(crate) fn dialog_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.dialog.take() else { return };
        let mut open = true;
        let mut done = false;
        let title = match &d {
            Dialog::NewImage { .. } => crate::i18n::t("New image"),
            Dialog::Resize { .. } => crate::i18n::t("Resize image"),
            Dialog::Canvas { .. } => crate::i18n::t("Canvas size and color"),
            Dialog::Rename(_) => crate::i18n::t("Rename"),
            Dialog::Export { .. } => crate::i18n::t("Export pictures"),
            Dialog::GettingStarted => crate::i18n::t("Getting started"),
            Dialog::WhatsNew => crate::i18n::t("What's new in Snagster"),
        };
        let mut action: Option<Box<dyn FnOnce(&mut SnagApp)>> = None;
        own_window(ctx, Win { key: "dialog", title, size: vec2(440.0, 220.0), resizable: false }, &mut open, |ui| match &mut d {
            Dialog::NewImage { w, h, color } => {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::t("Width"));
                    ui.add(egui::DragValue::new(w).range(1..=10000));
                    ui.label(crate::i18n::t("Height"));
                    ui.add(egui::DragValue::new(h).range(1..=10000));
                });
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::t("Background"));
                    color4_edit(ui, color);
                    if ui.button(crate::i18n::t("Transparent")).clicked() {
                        *color = [0, 0, 0, 0];
                    }
                });
                let pick = dialog_row(ui, "Create", true);
                done |= pick.is_some();
                if pick == Some(Choice::Action) {
                    let (w, h, c) = (*w, *h, *color);
                    action = Some(Box::new(move |app| {
                        let name = app.next_name("New image");
                        app.add_entry(name, Doc::new(image::RgbaImage::from_pixel(w, h, image::Rgba(c))));
                    }));
                    done = true;
                }
            }
            Dialog::Resize { w, h, keep, ratio } => {
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::t("Width"));
                    if ui.add(egui::DragValue::new(w).range(1..=20000)).changed() && *keep {
                        *h = ((*w as f32 / *ratio).round() as u32).max(1);
                    }
                    ui.label(crate::i18n::t("Height"));
                    if ui.add(egui::DragValue::new(h).range(1..=20000)).changed() && *keep {
                        *w = ((*h as f32 * *ratio).round() as u32).max(1);
                    }
                });
                ui.checkbox(keep, crate::i18n::t("Keep the proportions"));
                let pick = dialog_row(ui, "Resize", true);
                done |= pick.is_some();
                if pick == Some(Choice::Action) {
                    let (w, h) = (*w, *h);
                    action = Some(Box::new(move |app| app.image_op("Resized", |img| crate::effects::resize_exact(img, w, h))));
                    done = true;
                }
            }
            Dialog::Canvas { w, h, color } => {
                ui.label(RichText::new(crate::i18n::t("The picture stays in the middle; the rest gets the color.")).color(TEXT_DIM));
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::t("Width"));
                    ui.add(egui::DragValue::new(w).range(1..=20000));
                    ui.label(crate::i18n::t("Height"));
                    ui.add(egui::DragValue::new(h).range(1..=20000));
                });
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::t("Color"));
                    color4_edit(ui, color);
                    if ui.button(crate::i18n::t("Transparent")).clicked() {
                        *color = [0, 0, 0, 0];
                    }
                });
                let pick = dialog_row(ui, "Apply", true);
                done |= pick.is_some();
                if pick == Some(Choice::Action) {
                    let (w, h, c) = (*w, *h, *color);
                    action = Some(Box::new(move |app| app.image_op("New canvas", |img| crate::effects::canvas(img, w, h, c))));
                    done = true;
                }
            }
            Dialog::Rename(name) => {
                let r = ui.text_edit_singleline(name);
                r.request_focus();
                ui.label(RichText::new(crate::i18n::t("A saved picture's file is renamed too.")).color(TEXT_DIM));
                let mut pick = dialog_row(ui, "Rename", !name.trim().is_empty());
                // The name field has the keyboard, so Return is handled here.
                if pick.is_none() && !name.trim().is_empty() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    pick = Some(Choice::Action);
                }
                done |= pick.is_some();
                if pick == Some(Choice::Action) {
                    let n = name.clone();
                    action = Some(Box::new(move |app| app.rename_current(&n)));
                    done = true;
                }
            }
            Dialog::Export { pick, folder, format } => {
                ui.label(crate::i18n::t("Pictures:"));
                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    for (i, p) in pick.iter_mut().enumerate() {
                        let name = self.entries.get(i).map(|e| e.name.clone()).unwrap_or_default();
                        ui.checkbox(p, name);
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button(crate::i18n::t("All")).clicked() {
                        pick.iter_mut().for_each(|p| *p = true);
                    }
                    if ui.button(crate::i18n::t("None")).clicked() {
                        pick.iter_mut().for_each(|p| *p = false);
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::t("Folder"));
                    ui.add(egui::TextEdit::singleline(folder).desired_width(300.0));
                    if ui.button(crate::i18n::t("Browse…")).clicked()
                        && let Some(f) = rfd::FileDialog::new().pick_folder()
                    {
                        *folder = f.display().to_string();
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label(crate::i18n::t("Format"));
                    for f in ImageFormat::ALL {
                        ui.radio_value(format, f, f.label());
                    }
                });
                let n = pick.iter().filter(|p| **p).count();
                let choice = crate::platform::dialog_buttons(ui, Some(&crate::i18n::tf("Export {n} pictures", &[("n", &n.to_string())])), crate::i18n::t("Cancel"), crate::platform::Kind::Normal, n > 0);
                done |= choice.is_some();
                if choice == Some(Choice::Action) {
                    let (pick, folder, format) = (pick.clone(), PathBuf::from(folder.clone()), *format);
                    action = Some(Box::new(move |app| app.export_many(&pick, &folder, format)));
                    done = true;
                }
            }
            Dialog::GettingStarted => {
                ui.set_max_width(520.0);
                for (head, body) in [
                    ("1. Capture", "Press Print Screen (or the Capture button). Drag an area, or click a window, button or menu. A toolbar then lets you capture, scroll, record a GIF or grab the text."),
                    ("2. Mark it up", "Pick a tool on the left: arrows, text, callouts, steps, stamps, blur, spotlight, magnify... The options for each tool are on the right."),
                    ("3. Share it", "Copy it (Ctrl+C), save it (Ctrl+S), pin it on screen, email it, or put several into a guide (File > Export guide)."),
                    ("Everything is kept", "Every capture goes into the Library (Ctrl+L), searchable by date, program and tags."),
                    ("From anywhere", "Snagster's icon in the taskbar corner opens the capture panel; right-click it for the quick menu."),
                ] {
                    ui.label(RichText::new(head).strong());
                    ui.label(body);
                    ui.add_space(6.0);
                }
                if ui.button(crate::i18n::t("Try it on the sample picture")).clicked() {
                    action = Some(Box::new(|app| {
                        let name = app.next_name("Sample");
                        app.add_entry(name, crate::sample::sample_doc());
                    }));
                    done = true;
                }
            }
            Dialog::WhatsNew => {
                ui.set_max_width(520.0);
                for line in [
                    "Hotkeys can take over keys another program uses (Settings > Hotkeys).",
                    "New tools: Selection (lift part of the picture and move it), Fill and Eraser.",
                    "Arrows with heads at either end, text alignment and outline, callout shapes, blur types.",
                    "Quick styles, a customizable toolbar, cut / copy / paste of marks.",
                    "Capture several areas at once, menus with their sub-menus, and scrolling you do yourself.",
                    "New image, templates, open recent, rename, Recycle Bin, batch export, print, email, Word documents.",
                    "Presets can be imported, exported and duplicated; Snagster can start with Windows.",
                ] {
                    ui.label(format!("- {line}"));
                }
            }
        });
        if let Some(a) = action {
            a(self);
        }
        if open && !done {
            self.dialog = Some(d);
        }
    }

    /// Save the picked pictures into `folder` on the job queue: encoding many big pictures takes
    /// seconds, and the window must keep answering meanwhile (it used to stop answering). Each
    /// gets a file name of its own, also when two pictures have the same title.
    pub(crate) fn export_many(&mut self, pick: &[bool], folder: &std::path::Path, format: ImageFormat) {
        let mut items = Vec::new();
        for (i, p) in pick.iter().enumerate() {
            if !*p || i >= self.entries.len() {
                continue;
            }
            let title = self.entries[i].name.clone();
            let base = self.settings.file_name(&title);
            let path = crate::app::work::free_path(folder.join(format!("{base}.{}", format.ext())), &self.saving);
            self.saving.insert(path.clone());
            items.push((self.entries[i].doc.clone(), title, path));
        }
        let (settings, folder) = (self.settings.clone(), folder.to_path_buf());
        self.status = crate::i18n::tf("Exporting {n} pictures to {folder}…", &[("n", &items.len().to_string()), ("folder", &folder.display().to_string())]);
        self.jobs.queue(move || {
            let (mut ok, mut last_err, mut paths) = (0, None, Vec::new());
            for (doc, title, path) in items {
                match export::save_doc(&doc, &title, &path, &settings) {
                    Ok(()) => ok += 1,
                    Err(e) => last_err = Some(e),
                }
                paths.push(path);
            }
            Box::new(move |app: &mut SnagApp, _: &egui::Context| {
                for p in &paths {
                    app.saving.remove(p);
                }
                app.status = match last_err {
                    None => format!("Exported {ok} pictures to {}", folder.display()),
                    Some(e) => format!("Exported {ok}; some failed: {e}"),
                };
            })
        });
    }

    pub(crate) fn import_presets(&mut self) {
        let Some(p) = rfd::FileDialog::new().add_filter("Snagster presets", &["json"]).pick_file() else { return };
        match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|s| serde_json::from_str::<Vec<Preset>>(&s).map_err(|e| e.to_string())) {
            Ok(list) => {
                let n = list.len();
                for mut pr in list {
                    // Imported hotkeys could clash; they start without one.
                    pr.combo = Combo::default();
                    self.settings.presets.push(pr);
                }
                self.status = format!("Imported {n} presets.");
            }
            Err(e) => self.status = format!("Couldn't import presets: {e}"),
        }
    }

    pub(crate) fn export_presets(&mut self, only: Option<usize>) {
        let list: Vec<Preset> = match only {
            Some(i) => self.settings.presets.get(i).cloned().into_iter().collect(),
            None => self.settings.presets.clone(),
        };
        let name = if only.is_some() { "snag-preset.json" } else { "snag-presets.json" };
        let Some(p) = rfd::FileDialog::new().add_filter("Snagster presets", &["json"]).set_file_name(name).save_file() else { return };
        self.status = match serde_json::to_string_pretty(&list).map_err(|e| e.to_string()).and_then(|s| std::fs::write(&p, s).map_err(|e| e.to_string())) {
            Ok(()) => format!("Exported {} presets to {}", list.len(), p.display()),
            Err(e) => format!("Couldn't export presets: {e}"),
        };
    }
}

/// The encoders to offer: each name once, in `available`'s order (graphics chip first).
pub(crate) fn encoder_choices(list: &[(crate::encode::Codec, crate::encode::EncoderName)]) -> Vec<crate::encode::EncoderName> {
    let mut out: Vec<crate::encode::EncoderName> = Vec::new();
    for (_, n) in list {
        if !out.iter().any(|o| o.name == n.name) {
            out.push(n.clone());
        }
    }
    out
}

/// The video encoder choice (recording and streaming share it): Automatic, then this
/// computer's encoders. A choice that is gone, or can't start, falls back by itself.
pub(crate) fn encoder_picker(ui: &mut egui::Ui, salt: &str, pick: &mut String, list: &[(crate::encode::Codec, crate::encode::EncoderName)]) {
    use crate::i18n::t;
    let auto = t("Automatic (recommended)");
    let choices = encoder_choices(list);
    ui.horizontal(|ui| {
        ui.label(t("Video encoder"));
        let shown = if pick.is_empty() { auto.to_string() } else { pick.clone() };
        egui::ComboBox::from_id_salt(salt).width(320.0).selected_text(shown).show_ui(ui, |ui| {
            ui.selectable_value(pick, String::new(), auto);
            for n in &choices {
                let kind = if n.hardware { t("graphics chip") } else { t("processor") };
                ui.selectable_value(pick, n.name.clone(), format!("{} ({kind})", n.name));
            }
        });
    })
    .response
    .on_hover_text(t("Automatic uses the best encoder that works. If the one you choose can't start or stops working, Snagster switches to the best one that works and says so."));
    if !pick.is_empty() && !list.is_empty() && !choices.iter().any(|n| n.name == *pick) {
        ui.label(RichText::new(t("The chosen encoder isn't on this computer now, so Automatic is used.")).color(TEXT_DIM));
    }
}

/// Where Windows keeps what starts when you sign in (under HKEY_CURRENT_USER).
#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Text values under HKEY_CURRENT_USER through the registry API (microseconds; no `reg.exe`).
#[cfg(windows)]
mod registry {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW};
    use windows::core::HSTRING;

    pub fn get(key: &str, name: &str) -> Option<String> {
        let (k, n) = (HSTRING::from(key), HSTRING::from(name));
        let mut bytes = 0u32;
        unsafe {
            if RegGetValueW(HKEY_CURRENT_USER, &k, &n, RRF_RT_REG_SZ, None, None, Some(&mut bytes)) != ERROR_SUCCESS {
                return None;
            }
            let mut buf = vec![0u16; (bytes as usize).div_ceil(2).max(1)];
            if RegGetValueW(HKEY_CURRENT_USER, &k, &n, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut bytes)) != ERROR_SUCCESS {
                return None;
            }
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..len]))
        }
    }

    pub fn set(key: &str, name: &str, value: &str) -> Result<(), String> {
        let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
        let r = unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name), REG_SZ.0, Some(data.as_ptr().cast()), (data.len() * 2) as u32) };
        r.ok().map_err(|e| e.to_string())
    }

    /// Remove a value; one that isn't there is fine.
    pub fn delete(key: &str, name: &str) -> Result<(), String> {
        let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name)) };
        if r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND { Ok(()) } else { r.ok().map_err(|e| e.to_string()) }
    }

    #[cfg(test)]
    mod tests {
        /// In a key of its own (never the real Run key), removed afterwards.
        #[test]
        fn values_are_written_read_back_and_removed() {
            let key = format!(r"Software\Snagster-test-{}", std::process::id());
            assert_eq!(super::get(&key, "v"), None);
            super::set(&key, "v", "\"C:\\Program Files\\Snagster æøå\\Snagster.exe\"").unwrap();
            assert_eq!(super::get(&key, "v").as_deref(), Some("\"C:\\Program Files\\Snagster æøå\\Snagster.exe\""));
            super::delete(&key, "v").unwrap();
            assert_eq!(super::get(&key, "v"), None);
            super::delete(&key, "v").expect("removing what isn't there is fine");
            unsafe {
                let _ = windows::Win32::System::Registry::RegDeleteKeyW(super::HKEY_CURRENT_USER, &super::HSTRING::from(key.as_str()));
            }
        }
    }
}

/// Start Snagster when Windows starts: the Run key in the registry, or the package's startup
/// task when Snagster was installed from its MSIX (its folder changes with every update).
pub fn set_run_at_startup(on: bool) -> Result<(), String> {
    #[cfg(windows)]
    if crate::packaged::is_packaged() {
        return crate::packaged::set_startup(on);
    }
    #[cfg(windows)]
    {
        // Straight through the registry API: it used to start `reg.exe` (twice) and wait for it
        // on the window's thread.
        // The entry from before the rename.
        let _ = registry::delete(RUN_KEY, "Snag");
        if !on {
            return registry::delete(RUN_KEY, "Snagster");
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        return registry::set(RUN_KEY, "Snagster", &format!("\"{}\"", exe.display()));
    }
    #[cfg(not(windows))]
    {
        let Some(file) = autostart_file() else { return Err("No home folder.".into()) };
        if !on {
            let _ = std::fs::remove_file(&file);
            return Ok(());
        }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        #[cfg(target_os = "macos")]
        let body = launch_agent(&exe);
        #[cfg(not(target_os = "macos"))]
        let body = desktop_entry(&exe);
        std::fs::write(&file, body).map_err(|e| e.to_string())
    }
}

/// The macOS LaunchAgent that starts `exe` at login.
#[cfg(any(target_os = "macos", test))]
fn launch_agent(exe: &std::path::Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>app.snagster</string><key>ProgramArguments</key><array><string>{}</string></array><key>RunAtLoad</key><true/></dict></plist>\n",
        exe.display()
    )
}

/// The Linux autostart entry that starts `exe` at login.
#[cfg(any(all(not(windows), not(target_os = "macos")), test))]
fn desktop_entry(exe: &std::path::Path) -> String {
    format!("[Desktop Entry]\nType=Application\nName=Snagster\nExec=\"{}\"\nX-GNOME-Autostart-enabled=true\n", exe.display())
}

/// Where Linux (autostart entry) or macOS (LaunchAgent) looks for programs to start at login.
#[cfg(not(windows))]
fn autostart_file() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    #[cfg(target_os = "macos")]
    return Some(home.join("Library/LaunchAgents/app.snagster.plist"));
    #[cfg(not(target_os = "macos"))]
    Some(home.join(".config/autostart/snagster.desktop"))
}

pub fn runs_at_startup() -> bool {
    #[cfg(windows)]
    if crate::packaged::is_packaged() {
        return crate::packaged::startup() == Ok(crate::packaged::Startup::On);
    }
    #[cfg(windows)]
    {
        registry::get(RUN_KEY, "Snagster").is_some()
    }
    #[cfg(not(windows))]
    {
        autostart_file().is_some_and(|f| f.exists())
    }
}

impl SnagApp {
    /// How to let AI apps (Claude Code, Claude Desktop, Cursor...) use Snagster over MCP.
    fn ai_tab(&mut self, ui: &mut egui::Ui) {
        use crate::i18n::{t, tf};
        let exe = crate::packaged::mcp_command();
        ui.label(t("Snagster is an MCP server: any AI app that speaks MCP can capture the screen, mark up pictures, read text, record videos and run Studio. Apps on this computer only; nothing goes online."));
        ui.add_space(6.0);
        let copy_row = |ui: &mut egui::Ui, label: &str, text: String| {
            ui.label(RichText::new(label).strong());
            ui.horizontal(|ui| {
                let mut tx = text.clone();
                let rows = tx.lines().count().max(1);
                ui.add(egui::TextEdit::multiline(&mut tx).code_editor().desired_rows(rows).desired_width(470.0));
                if ui.button(t("Copy")).clicked() {
                    let _ = arboard::Clipboard::new().and_then(|mut c| c.set_text(text.clone()));
                }
            });
            ui.add_space(6.0);
        };
        egui::CollapsingHeader::new(RichText::new(t("Apps that start Snagster themselves (stdio)")).strong()).default_open(true).show(ui, |ui| {
            ui.label(RichText::new(t("Claude Desktop, Claude Code, Cursor, VS Code, Windsurf, Zed, LM Studio, Jan, Cline, Continue, Goose and others.")).color(TEXT_DIM));
            copy_row(ui, t("Claude Code (run in a terminal)"), format!("claude mcp add snagster -- \"{exe}\" --mcp"));
            let json = serde_json::to_string_pretty(&serde_json::json!({
                "mcpServers": {"snagster": {"command": exe, "args": ["--mcp"]}}
            }))
            .unwrap_or_default();
            copy_row(ui, t("Claude Desktop, Cursor, LM Studio (mcp.json), Jan and most others"), json);
            let vscode = serde_json::to_string_pretty(&serde_json::json!({
                "servers": {"snagster": {"type": "stdio", "command": exe, "args": ["--mcp"]}}
            }))
            .unwrap_or_default();
            copy_row(ui, t("VS Code (.vscode/mcp.json)"), vscode);
        });
        ui.add_space(4.0);
        egui::CollapsingHeader::new(RichText::new(t("Apps that connect to a server (HTTP)")).strong()).default_open(true).show(ui, |ui| {
            ui.label(RichText::new(t("Open WebUI, AnythingLLM, LibreChat, n8n, LM Studio, your own agent: local AI models (Ollama, llama.cpp, LM Studio) work through these apps.")).color(TEXT_DIM));
            ui.checkbox(&mut self.settings.mcp_http, t("Let AI apps on this computer connect while Snagster runs"));
            ui.horizontal(|ui| {
                ui.label(t("Port"));
                ui.add(egui::DragValue::new(&mut self.settings.mcp_port).range(1024..=65535));
                ui.label(t("Password (optional)"));
                ui.add(egui::TextEdit::singleline(&mut self.settings.mcp_token).password(true).desired_width(140.0));
            });
            match (&self.x.mcp_server, &self.x.mcp_error) {
                (Some(s), _) => {
                    ui.label(RichText::new(tf("Running: {url}", &[("url", &s.url())])).color(Color32::from_rgb(90, 200, 130)));
                }
                (None, Some(e)) => {
                    ui.label(RichText::new(tf("Couldn't start: {e}", &[("e", e)])).color(Color32::from_rgb(240, 120, 110)));
                }
                _ => {
                    ui.label(RichText::new(t("Off.")).color(TEXT_DIM));
                }
            }
            let port = self.settings.mcp_port;
            copy_row(ui, t("Streamable HTTP address (Open WebUI, LibreChat, n8n, most new apps)"), format!("http://127.0.0.1:{port}/mcp"));
            copy_row(ui, t("Older apps (HTTP + SSE)"), format!("http://127.0.0.1:{port}/sse"));
            if !self.settings.mcp_token.is_empty() {
                copy_row(ui, t("Header the app must send"), format!("Authorization: Bearer {}", self.settings.mcp_token));
            }
            copy_row(ui, t("Or run the server without the window"), format!("\"{exe}\" mcp-http --port {port}"));
        });
        ui.add_space(4.0);
        copy_row(ui, t("From a terminal"), format!("\"{exe}\" help"));
        ui.label(RichText::new(tf("{n} tools an AI can use", &[("n", &crate::mcp::tool_names().len().to_string())])).strong());
        egui::Grid::new("mcp-tools").striped(true).spacing(vec2(12.0, 3.0)).show(ui, |ui| {
            for (n, d) in crate::mcp::tool_names() {
                ui.label(RichText::new(n).monospace());
                ui.add(egui::Label::new(RichText::new(d).color(TEXT_DIM).size(12.0)).wrap());
                ui.end_row();
            }
        });
    }

    /// Language and updates.
    fn language_tab(&mut self, ui: &mut egui::Ui) {
        use crate::i18n::{t, tf};
        ui.label(RichText::new(t("Language")).strong());
        let cur = crate::i18n::current();
        ui.horizontal(|ui| {
            let shown = if self.settings.language.is_empty() {
                tf("Automatic: the computer's language ({name})", &[("name", &crate::i18n::name(crate::i18n::system()))])
            } else {
                format!("{}  ·  {}", crate::i18n::name(cur), crate::i18n::native_name(cur))
            };
            ui.label(shown);
            if ui.button(t("Change…")).clicked() {
                self.x.lang_open = true;
            }
        });
        ui.label(RichText::new(t("Snagster speaks 86 languages. Language names are shown in the language you pick.")).color(TEXT_DIM));
        ui.add_space(10.0);
        ui.label(RichText::new(t("Updates")).strong());
        ui.label(tf("This is Snagster {v}.", &[("v", env!("CARGO_PKG_VERSION"))]));
        if let Some(who) = self.x.update.managed() {
            ui.label(RichText::new(crate::app::updates_ui::managed_note(who)).color(TEXT_DIM));
        } else {
            ui.checkbox(&mut self.settings.update_check, t("Look for a new version on GitHub when Snagster starts (once a day)"));
            ui.horizontal(|ui| {
                let checking = self.x.update.checking();
                if ui.add_enabled(!checking, egui::Button::new(if checking { t("Checking…") } else { t("Check now") })).clicked() {
                    self.x.update.check(ui.ctx(), true);
                }
                if let Some(r) = self.x.update.available().cloned()
                    && ui.button(tf("Install {v}", &[("v", &r.version)])).clicked()
                {
                    self.x.update.dialog = true;
                }
            });
        }
        if !self.settings.update_skip.is_empty() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(tf("Skipping version {v}.", &[("v", &self.settings.update_skip)])).color(TEXT_DIM));
                if ui.small_button(t("Don't skip")).clicked() {
                    self.settings.update_skip.clear();
                }
            });
        }
        ui.add_space(10.0);
        ui.label(RichText::new(t("Problems")).strong());
        if let Some(p) = crate::crash_log_path() {
            ui.label(RichText::new(tf("If Snagster ever stops unexpectedly, what happened is written to {file}.", &[("file", &p.display().to_string())])).color(TEXT_DIM));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A box opens in the middle of Snagster's window, but never off the screen.
    #[test]
    fn boxes_open_in_the_middle_of_the_window_and_on_the_screen() {
        let main = Rect::from_min_size(pos2(100.0, 50.0), vec2(1280.0, 800.0));
        assert_eq!(centered_on(main, vec2(400.0, 200.0), None), pos2(540.0, 350.0));
        assert_eq!(centered_on(main, vec2(400.0, 200.0), Some(vec2(1920.0, 1080.0))), pos2(540.0, 350.0));
        // Snagster pushed far to the right: the box stays on the monitor.
        let right = Rect::from_min_size(pos2(1700.0, 900.0), vec2(600.0, 400.0));
        assert_eq!(centered_on(right, vec2(700.0, 600.0), Some(vec2(1920.0, 1080.0))), pos2(1220.0, 480.0));
        // A box bigger than the monitor starts at its corner.
        assert_eq!(centered_on(main, vec2(3000.0, 2000.0), Some(vec2(1920.0, 1080.0))), pos2(0.0, 0.0));
    }

    /// In check runs (no extra windows) a box shows inside the main window, and its content runs.
    #[test]
    fn a_box_shows_inside_the_window_when_windows_are_embedded() {
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(true);
        let mut open = true;
        let mut ran = false;
        let mut out = ctx.run_ui(Default::default(), |ui| {
            let r = own_window(ui.ctx(), Win { key: "t", title: "Test", size: vec2(300.0, 200.0), resizable: false }, &mut open, |ui| {
                ui.label("hello");
                ran = true;
                7
            });
            assert_eq!(r, Some(7));
        });
        out.textures_delta.clear();
        assert!(ran && open);
        let mut closed = false;
        let mut out = ctx.run_ui(Default::default(), |ui| {
            assert_eq!(own_window(ui.ctx(), Win { key: "t", title: "Test", size: vec2(300.0, 200.0), resizable: false }, &mut closed, |_| 1), None);
        });
        out.textures_delta.clear();
    }

    #[test]
    fn new_image_dialog_starts_white_720p() {
        match Dialog::new_image() {
            Dialog::NewImage { w, h, color } => assert_eq!((w, h, color), (1280, 720, [255, 255, 255, 255])),
            _ => panic!("wrong dialog"),
        }
    }

    #[test]
    fn export_dialog_picks_everything_and_uses_the_settings() {
        let mut s = Settings::default();
        s.save_folder = "/tmp/pictures".into();
        match Dialog::export(3, &s) {
            Dialog::Export { pick, folder, format } => {
                assert_eq!(pick, vec![true, true, true]);
                assert_eq!(folder, "/tmp/pictures");
                assert_eq!(format, s.format);
            }
            _ => panic!("wrong dialog"),
        }
        match Dialog::export(0, &s) {
            Dialog::Export { pick, .. } => assert!(pick.is_empty()),
            _ => panic!("wrong dialog"),
        }
    }

    #[test]
    fn linux_autostart_entry_runs_the_program() {
        let e = desktop_entry(Path::new("/opt/Snag ster/snagster"));
        assert!(e.starts_with("[Desktop Entry]\n"));
        assert!(e.contains("\nType=Application\n"));
        assert!(e.contains("\nName=Snagster\n"));
        // Quoted, so a folder with a space still works.
        assert!(e.contains("\nExec=\"/opt/Snag ster/snagster\"\n"));
        assert!(e.ends_with('\n'));
    }

    #[test]
    fn mac_launch_agent_runs_the_program_at_login() {
        let p = launch_agent(Path::new("/Applications/Snagster.app/Contents/MacOS/snagster"));
        assert!(p.starts_with("<?xml version=\"1.0\""));
        assert!(p.contains("<key>Label</key><string>app.snagster</string>"));
        assert!(p.contains("<array><string>/Applications/Snagster.app/Contents/MacOS/snagster</string></array>"));
        assert!(p.contains("<key>RunAtLoad</key><true/>"));
        assert!(p.trim_end().ends_with("</plist>"));
    }

    #[cfg(not(windows))]
    #[test]
    fn autostart_file_is_in_the_home_folder() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let f = autostart_file().unwrap();
        assert!(f.starts_with(home));
        #[cfg(target_os = "macos")]
        assert!(f.ends_with("Library/LaunchAgents/app.snagster.plist"));
        #[cfg(not(target_os = "macos"))]
        assert!(f.ends_with(".config/autostart/snagster.desktop"));
    }
}
