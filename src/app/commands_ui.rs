//! Commands sent to the open window by AI apps (MCP `app_command`, through `instance`): each
//! catalogue id runs the same `SnagApp` method its ribbon button or menu item runs.

use super::*;
use crate::catalog;

type Run = fn(&mut SnagApp, &egui::Context);

/// Sent by the MCP server after it saved settings: the window reads them again (not a button,
/// so not in the catalogue).
pub(crate) const RELOAD_SETTINGS: &str = "reload_settings";

/// The window commands that take no value, and what each one's button does.
pub(crate) const SIMPLE: &[(&str, Run)] = &[
    ("import_presets", |a, _| a.import_presets()),
    ("export_presets", |a, _| a.export_presets(None)),
    ("timer", |a, _| a.timer_open = true),
    ("capture_from_clipboard", |a, _| a.paste()),
    ("new_image", |a, _| a.dialog = Some(crate::windows::Dialog::new_image())),
    ("import_pictures", |a, _| {
        if let Some(files) = rfd::FileDialog::new().add_filter("Pictures and Snagster projects", &["png", "jpg", "jpeg", "bmp", "gif", "webp", "tif", "tiff", "ico", "snag"]).pick_files() {
            for f in files {
                a.open_path(&f);
            }
        }
    }),
    ("library", |a, _| a.library_open = true),
    ("save", |a, _| a.save()),
    ("save_as", |a, _| a.save_as()),
    ("save_project_as", |a, _| a.save_project_as()),
    ("quick_save", |a, _| a.quick_save()),
    ("rename", |a, _| {
        if let Some(i) = a.current {
            a.dialog = Some(crate::windows::Dialog::Rename(a.entries[i].name.clone()));
        }
    }),
    ("recycle", |a, _| a.recycle_current()),
    ("export_pictures", |a, _| {
        if !a.entries.is_empty() {
            a.dialog = Some(crate::windows::Dialog::export(a.entries.len(), &a.settings));
        }
    }),
    ("print", |a, _| a.print()),
    ("copy_file_path", |a, _| a.copy_file_path()),
    ("open_save_folder", |a, _| {
        let f = PathBuf::from(&a.settings.save_folder);
        let _ = std::fs::create_dir_all(&f);
        a.open_folder(&f);
    }),
    ("exit", |_, ctx| ctx.send_viewport_cmd(egui::ViewportCommand::Close)),
    ("panel", |a, ctx| a.enter_panel(ctx)),
    ("copy", |a, _| a.copy()),
    ("pin", |a, _| a.pin_current()),
    ("close_pins", |a, _| {
        for p in &mut a.pins {
            p.close = true;
        }
    }),
    ("copy_color", |a, _| a.copy_color()),
    ("undo", |a, _| a.undo()),
    ("redo", |a, _| a.redo()),
    ("record_stop", |a, _| a.stop_job()),
    ("record_whole_screen", |a, ctx| {
        if !a.is_recording()
            && a.capturing.is_none()
            && let Some(r) = a.screen_under_pointer()
        {
            a.start_record(ctx, r);
        }
    }),
    ("record_pause", |a, _| {
        if a.mini == Some(Mini::Video) {
            a.toggle_pause();
        }
    }),
    ("play_last_video", |a, _| {
        if let Some(p) = a.videos.first().map(|v| v.path.clone()) {
            a.open_file(&p);
        }
    }),
    ("show_last_video", |a, _| {
        if let Some(p) = a.videos.first().map(|v| v.path.clone()) {
            a.open_folder(&p);
        }
    }),
    ("recordings_folder", |a, _| {
        let f = PathBuf::from(&a.settings.video_folder);
        let _ = std::fs::create_dir_all(&f);
        a.open_folder(&f);
    }),
    ("gif_editor", |a, _| a.gif_open = true),
    ("gif_slideshow", |a, ctx| {
        if a.entries.len() >= 2 {
            a.slideshow(ctx);
        }
    }),
    ("delete_mark", |a, _| a.delete_selected()),
    ("mark_bigger", |a, _| a.scale_selected(1.15)),
    ("mark_smaller", |a, _| a.scale_selected(1.0 / 1.15)),
    ("clear_marks", |a, _| a.clear_marks()),
    ("cut_mark", |a, _| a.cut_mark()),
    ("copy_mark", |a, _| a.copy_mark()),
    ("paste_mark", |a, _| a.paste_mark()),
    ("paste_onto", |a, _| a.paste_as_piece()),
    ("duplicate", |a, _| a.duplicate()),
    ("remove_from_recent", |a, _| a.remove_current()),
    ("grab_text_picture", |a, _| {
        if let Some(i) = a.current
            && !a.ocr_busy
        {
            let img = render::compose(&a.entries[i].doc);
            a.run_ocr(OcrPurpose::GrabText, img);
        }
    }),
    ("resize", |a, _| {
        if let Some(i) = a.current {
            let (w, h) = a.entries[i].doc.image.dimensions();
            a.dialog = Some(crate::windows::Dialog::Resize { w, h, keep: true, ratio: w as f32 / h.max(1) as f32 });
        }
    }),
    ("crop", |a, _| a.tool = Tool::Crop),
    ("trim", |a, _| a.image_op("Trimmed", effects::trim)),
    ("canvas", |a, _| {
        if let Some(i) = a.current {
            let (w, h) = a.entries[i].doc.image.dimensions();
            a.dialog = Some(crate::windows::Dialog::Canvas { w: w + 80, h: h + 80, color: [255, 255, 255, 255] });
        }
    }),
    ("smart_redact", |a, _| {
        if let Some(i) = a.current
            && !a.ocr_busy
        {
            let img = render::compose(&a.entries[i].doc);
            a.run_ocr(OcrPurpose::Redact(i), img);
        }
    }),
    ("simplify", |a, _| {
        if let Some(i) = a.current
            && !a.ocr_busy
        {
            let img = render::compose(&a.entries[i].doc);
            a.run_ocr(OcrPurpose::Simplify(i), img);
        }
    }),
    ("email", |a, _| a.email()),
    ("word_document", |a, _| a.export_word()),
    ("open_with", |a, _| a.open_externally()),
    ("show_in_folder", |a, _| {
        if let Some(p) = a.current.and_then(|i| a.entries[i].path.clone()) {
            a.open_folder(&p);
        }
    }),
    ("zoom_fit", |a, ctx| a.set_zoom(ctx, None)),
    ("zoom_actual", |a, ctx| a.set_zoom(ctx, Some(0.0))),
    ("zoom_in", |a, ctx| a.set_zoom(ctx, Some(1.25))),
    ("zoom_out", |a, ctx| a.set_zoom(ctx, Some(0.8))),
    ("getting_started", |a, _| a.dialog = Some(crate::windows::Dialog::GettingStarted)),
    ("shortcuts", |a, _| a.shortcuts_open = true),
    ("whats_new", |a, _| a.dialog = Some(crate::windows::Dialog::WhatsNew)),
    ("check_updates", |a, ctx| a.x.update.check(ctx, true)),
    ("ai_apps", |a, _| {
        a.settings_open = true;
        a.settings_tab = SettingsTab::Ai;
    }),
    ("about", |a, _| a.about_open = true),
    ("problem_reports", |a, _| {
        if let Some(p) = crate::crash_log_path()
            && let Some(dir) = p.parent()
        {
            let _ = std::fs::create_dir_all(dir);
            a.open_folder(dir);
        }
    }),
    ("go_live", |a, ctx| a.go_live(ctx)),
    ("stream_stop", |a, _| a.stop_stream()),
    ("stream_edit_in_studio", |a, ctx| a.switch_tab(ctx, super::ribbon::RibbonTab::Studio)),
    ("stream_forget_key", |a, _| {
        // The person forgets the key on the Streaming page (an AI app never handles keys).
        a.settings_open = true;
        a.settings_tab = SettingsTab::Streaming;
    }),
    ("studio", |a, ctx| a.switch_tab(ctx, super::ribbon::RibbonTab::Studio)),
    ("studio_record", |a, ctx| {
        a.switch_tab(ctx, super::ribbon::RibbonTab::Studio);
        if a.x.studio.is_some() && !super::studio_ui::recording(a) {
            super::studio_ui::start(a, ctx);
        }
    }),
    ("studio_stop", |a, _| super::studio_ui::stop(a)),
    ("studio_pause", |a, _| super::studio_ui::toggle_pause(a)),
    ("studio_new_scene", |a, _| super::studio_ui::new_scene(&mut a.settings.studio)),
    ("studio_copy_scene", |a, _| super::studio_ui::copy_scene(&mut a.settings.studio)),
    ("studio_delete_scene", |a, _| super::studio_ui::delete_scene(&mut a.settings.studio)),
];

/// A window command with its value read.
pub(crate) enum Cmd {
    Simple(Run),
    Action(Action),
    Capture(CaptureKind),
    WindowTitle(String),
    Monitor(usize),
    Preset(usize),
    Open(Option<PathBuf>),
    Settings(SettingsTab),
    Guide(&'static str),
    Template(Option<Template>),
    Effect(Effect),
    Combine(bool),
    Cloud(String),
    Language(Option<&'static str>),
    StreamSource(crate::stream::Source, bool),
    Tool(Tool),
    ShowWindow,
}

/// The settings pages by the names `app_command settings` takes.
pub(crate) const PAGES: [(&str, SettingsTab); 12] = [
    ("after_capture", SettingsTab::AfterCapture),
    ("saving", SettingsTab::Saving),
    ("capture", SettingsTab::Capture),
    ("recording", SettingsTab::Recording),
    ("editor", SettingsTab::Editor),
    ("effects", SettingsTab::Effects),
    ("redact", SettingsTab::Redact),
    ("hotkeys", SettingsTab::Hotkeys),
    ("presets", SettingsTab::Presets),
    ("ai", SettingsTab::Ai),
    ("language", SettingsTab::Language),
    ("streaming", SettingsTab::Streaming),
];

fn need<'a>(id: &str, value: Option<&'a str>) -> Result<&'a str, String> {
    value.map(str::trim).filter(|v| !v.is_empty()).ok_or_else(|| {
        let what = catalog::command(id).map(|c| c.value).unwrap_or("a value");
        format!("{id} needs a value: {what}")
    })
}

fn number(id: &str, value: Option<&str>) -> Result<usize, String> {
    need(id, value)?.parse().map_err(|_| format!("{id} takes a number"))
}

/// Reads a command id and its value. Doesn't touch the window, so the MCP server checks a
/// command with it before sending it. Unknown ids give an error starting "unknown command".
pub(crate) fn parse(id: &str, value: Option<&str>) -> Result<Cmd, String> {
    let Some(c) = catalog::command(id) else {
        return Err(format!("unknown command {id} (list_commands shows them all)"));
    };
    if !c.window {
        return Err(format!("{id} isn't a window command: use the MCP tool {}", c.tool));
    }
    if let Some(a) = catalog::command_action(id) {
        return Ok(if a == Action::ShowSnag { Cmd::ShowWindow } else { Cmd::Action(a) });
    }
    if let Some((_, run)) = SIMPLE.iter().find(|s| s.0 == id) {
        return Ok(Cmd::Simple(*run));
    }
    Ok(match id {
        "capture_window_list" => Cmd::WindowTitle(need(id, value)?.to_string()),
        "capture_monitor" => Cmd::Monitor(number(id, value)?),
        "capture_shape" => {
            let v = need(id, value)?;
            let s = CutShape::ALL.into_iter().find(|s| format!("{s:?}").eq_ignore_ascii_case(v) || s.label().eq_ignore_ascii_case(v)).ok_or(format!("{v} isn't a shape"))?;
            Cmd::Capture(CaptureKind::Pick(s, PickPurpose::Capture))
        }
        "capture_fixed_size" => {
            let v = need(id, value)?.to_ascii_lowercase();
            let (w, h) = v.split_once('x').ok_or("capture_fixed_size takes WIDTHxHEIGHT")?;
            let (w, h): (u32, u32) = (w.trim().parse().map_err(|_| "bad width")?, h.trim().parse().map_err(|_| "bad height")?);
            if !(6..=8000).contains(&w) || !(6..=8000).contains(&h) {
                return Err("the size is 6-8000 pixels each way".into());
            }
            Cmd::Capture(CaptureKind::Fixed(w, h))
        }
        "run_preset" => Cmd::Preset(number(id, value)?),
        "open" | "open_recent" => match value.map(str::trim).filter(|v| !v.is_empty()) {
            Some(p) => Cmd::Open(Some(PathBuf::from(p))),
            None if id == "open" => Cmd::Open(None),
            None => return Err(need(id, None).unwrap_err()),
        },
        "settings" => match value.map(str::trim).filter(|v| !v.is_empty()) {
            None => Cmd::Settings(SettingsTab::AfterCapture),
            Some(v) => Cmd::Settings(PAGES.iter().find(|p| p.0.eq_ignore_ascii_case(v)).map(|p| p.1).ok_or(format!("{v} isn't a settings page ({})", PAGES.map(|p| p.0).join(", ")))?),
        },
        "export_guide" => Cmd::Guide(match need(id, value)? {
            "pdf" => "pdf",
            "html" => "html",
            "page" => "page",
            v => return Err(format!("{v} isn't pdf, html or page")),
        }),
        "new_from_template" => Cmd::Template(match need(id, value)? {
            "page" => None,
            "side_by_side" => Some(Template::SideBySide),
            "grid2" => Some(Template::Grid2),
            "grid3" => Some(Template::Grid3),
            "before_after" => Some(Template::BeforeAfter),
            "gallery" => Some(Template::Gallery),
            v => return Err(format!("{v} isn't a template")),
        }),
        "effect" => {
            let v = need(id, value)?;
            Cmd::Effect(Effect::ALL.into_iter().find(|e| format!("{e:?}").eq_ignore_ascii_case(v)).ok_or(format!("{v} isn't an effect (see apply_effect)"))?)
        }
        "combine" => Cmd::Combine(match need(id, value)? {
            "side" => false,
            "top" => true,
            v => return Err(format!("{v} isn't side or top")),
        }),
        "save_to_cloud" => Cmd::Cloud(need(id, value)?.to_string()),
        "language" => match value.map(str::trim).filter(|v| !v.is_empty()) {
            None => Cmd::Language(None),
            Some(v) => Cmd::Language(Some(crate::i18n::CODES.iter().find(|c| c.eq_ignore_ascii_case(v)).copied().ok_or(format!("{v} isn't one of Snagster's languages (see list_languages)"))?)),
        },
        "stream_source" => {
            let v = need(id, value)?;
            use crate::stream::Source;
            match v {
                "screen" => Cmd::StreamSource(Source::Screen, false),
                "studio" => Cmd::StreamSource(Source::Studio, false),
                "area" => Cmd::StreamSource(Source::Screen, true),
                _ => match v.strip_prefix("window:") {
                    Some(t) if !t.trim().is_empty() => Cmd::StreamSource(Source::Window(t.trim().to_string()), false),
                    _ => return Err(format!("{v} isn't screen, window:TITLE, area or studio")),
                },
            }
        }
        "select_tool" => {
            let v = need(id, value)?;
            Cmd::Tool(Tool::ALL.into_iter().find(|t| t.label().eq_ignore_ascii_case(v) || format!("{t:?}").eq_ignore_ascii_case(v)).ok_or(format!("{v} isn't an editor tool"))?)
        }
        _ => return Err(format!("unknown command {id}: the window doesn't run it yet")),
    })
}

impl SnagApp {
    /// Runs a command an AI app sent (it was checked with `parse` before it was sent, and is
    /// checked again here).
    pub(crate) fn run_command(&mut self, ctx: &egui::Context, id: &str, value: Option<&str>) {
        if id == RELOAD_SETTINGS {
            // Only when the window persists its settings (not in tests and check runs).
            if self.persist {
                let fresh = Settings::load(false);
                let lang_changed = fresh.language != self.settings.language;
                self.settings = fresh.clone();
                self.saved_settings = fresh;
                if lang_changed {
                    self.apply_language(ctx);
                }
            }
            return;
        }
        match parse(id, value) {
            Ok(cmd) => self.run_cmd(ctx, cmd),
            // The MCP server checked it before sending; nothing to run.
            Err(_) => {}
        }
    }

    fn run_cmd(&mut self, ctx: &egui::Context, cmd: Cmd) {
        match cmd {
            Cmd::Simple(run) => run(self, ctx),
            Cmd::Action(a) => self.do_action(ctx, a),
            Cmd::ShowWindow => self.unhide(ctx),
            Cmd::Capture(k) => {
                if self.capturing.is_none() {
                    self.start_capture(ctx, k);
                }
            }
            Cmd::WindowTitle(t) => {
                let t = t.to_lowercase();
                match capture::list_windows().into_iter().find(|w| w.label.to_lowercase().contains(&t)) {
                    Some(w) if self.capturing.is_none() => self.start_capture(ctx, CaptureKind::Window(w.id, w.label)),
                    Some(_) => {}
                    None => self.status = crate::i18n::t("No windows found").into(),
                }
            }
            Cmd::Monitor(n) => match capture::monitors().get(n) {
                Some(m) if self.capturing.is_none() => self.start_capture(ctx, CaptureKind::Monitor(m.id)),
                _ => {}
            },
            Cmd::Preset(i) => {
                if i < self.settings.presets.len() {
                    self.run_preset(ctx, i);
                }
            }
            Cmd::Open(Some(p)) => self.open_path(&p),
            Cmd::Open(None) => self.open_dialog(),
            Cmd::Settings(tab) => {
                self.settings_open = true;
                self.settings_tab = tab;
            }
            Cmd::Guide(kind) => self.export_guide(kind),
            Cmd::Template(None) => self.export_guide("page"),
            Cmd::Template(Some(t)) => self.make_template(t),
            Cmd::Effect(e) => self.apply_effect(e),
            Cmd::Combine(vertical) => self.combine(vertical),
            Cmd::Cloud(name) => self.save_to_cloud(&name),
            Cmd::Language(None) => self.x.lang_open = true,
            Cmd::Language(Some(code)) => {
                self.settings.language = code.to_string();
                self.apply_language(ctx);
            }
            Cmd::StreamSource(_, true) => self.pick_stream_area(ctx, false),
            Cmd::StreamSource(s, false) => self.settings.stream.source = s,
            Cmd::Tool(t) => self.tool = t,
        }
        self.unhide(ctx);
    }

    /// Share > Save to the cloud: the current picture into `<cloud folder>/Snagster`.
    pub(crate) fn save_to_cloud(&mut self, name: &str) {
        let Some(i) = self.current else { return };
        let clouds = crate::settings::cloud_folders_now(None);
        let Some((_, folder)) = clouds.into_iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) else {
            self.status = crate::i18n::t("No OneDrive, Dropbox, Google Drive or iCloud folder found.").into();
            return;
        };
        let title = self.entries[i].name.clone();
        let path = self.settings.cloud_save_path(&folder, &title);
        self.save_in_background(i, path);
    }

    /// File > Copy file path.
    pub(crate) fn copy_file_path(&mut self) {
        if let Some(p) = self.current.and_then(|i| self.entries[i].path.clone()) {
            let _ = arboard::Clipboard::new().and_then(|mut c| c.set_text(p.display().to_string()));
            self.status = crate::i18n::tf("Copied {path}", &[("path", &p.display().to_string())]);
        }
    }

    /// Tools > Copy color: the colour now chosen for marks, as #RRGGBB.
    pub(crate) fn copy_color(&mut self) {
        let c = self.settings.color;
        let hex = format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
        let _ = arboard::Clipboard::new().and_then(|mut cb| cb.set_text(hex.clone()));
        self.status = crate::i18n::tf("Copied {path}", &[("path", &hex)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_window_command_in_the_catalogue_is_run_by_the_window() {
        for c in catalog::COMMANDS.iter().filter(|c| c.window) {
            if let Err(e) = parse(c.id, None) {
                assert!(!e.starts_with("unknown command"), "{}: {e}", c.id);
                assert!(!c.value.is_empty(), "{} needs a value but the catalogue doesn't say which: {e}", c.id);
            }
        }
        for (id, _) in SIMPLE {
            let c = catalog::command(id).unwrap_or_else(|| panic!("{id} runs in the window but isn't in the catalogue"));
            assert!(c.window && c.value.is_empty(), "{id}");
        }
    }

    #[test]
    fn commands_read_their_values() {
        assert!(parse("nope", None).err().unwrap().starts_with("unknown command"));
        assert!(parse("after_copy", None).err().unwrap().contains("set_setting"));
        assert!(matches!(parse("capture_area", None), Ok(Cmd::Action(Action::Area))));
        assert!(matches!(parse("show_window", None), Ok(Cmd::ShowWindow)));
        assert!(matches!(parse("capture_fixed_size", Some("1280x720")), Ok(Cmd::Capture(CaptureKind::Fixed(1280, 720)))));
        assert!(parse("capture_fixed_size", Some("9x99999")).is_err());
        assert!(parse("capture_fixed_size", None).err().unwrap().contains("WIDTHxHEIGHT"));
        assert!(matches!(parse("capture_shape", Some("star")), Ok(Cmd::Capture(CaptureKind::Pick(CutShape::Star, _)))));
        assert!(matches!(parse("settings", Some("hotkeys")), Ok(Cmd::Settings(SettingsTab::Hotkeys))));
        assert!(parse("settings", Some("nope")).is_err());
        assert!(matches!(parse("effect", Some("border")), Ok(Cmd::Effect(Effect::Border))));
        assert!(matches!(parse("combine", Some("top")), Ok(Cmd::Combine(true))));
        assert!(matches!(parse("export_guide", Some("html")), Ok(Cmd::Guide("html"))));
        assert!(matches!(parse("open", None), Ok(Cmd::Open(None))));
        assert!(parse("open_recent", None).is_err());
        assert!(matches!(parse("stream_source", Some("window:Notepad")), Ok(Cmd::StreamSource(crate::stream::Source::Window(_), false))));
        assert!(matches!(parse("stream_source", Some("area")), Ok(Cmd::StreamSource(_, true))));
        assert!(matches!(parse("select_tool", Some("Text")), Ok(Cmd::Tool(Tool::Text))));
        assert!(matches!(parse("run_preset", Some("2")), Ok(Cmd::Preset(2))));
        assert!(parse("run_preset", Some("two")).is_err());
        assert!(matches!(parse("language", Some("DE")), Ok(Cmd::Language(Some("de")))));
        assert!(parse("language", Some("xx-nope")).is_err());
    }

    #[test]
    fn window_commands_do_what_their_buttons_do() {
        let ctx = egui::Context::default();
        let mut a = crate::app::tests::app();
        a.run_command(&ctx, "duplicate", None);
        assert_eq!(a.entries.len(), 2);
        a.run_command(&ctx, "remove_from_recent", None);
        assert_eq!(a.entries.len(), 1);
        a.run_command(&ctx, "effect", Some("RotateLeft"));
        assert_eq!(a.entries[0].doc.image.dimensions(), (100, 200));
        a.run_command(&ctx, "undo", None);
        assert_eq!(a.entries[0].doc.image.dimensions(), (200, 100));
        a.run_command(&ctx, "select_tool", Some("Text"));
        assert_eq!(a.tool, Tool::Text);
        a.run_command(&ctx, "settings", Some("streaming"));
        assert!(a.settings_open && a.settings_tab == SettingsTab::Streaming);
        a.run_command(&ctx, "nope", None);
        assert!(a.settings_open, "an unknown command changes nothing");
    }
}
