//! The ribbon at the top, laid out like Word's: a row of tabs (File, Home, Capture, Record,
//! Studio, Edit, Image, Share, View, Help) over a band of tools in named groups. Every tool has
//! an icon and a tooltip. The Studio tab switches the window to Studio mode. The band folds away.

use super::*;
use crate::i18n::{t, tf};
use crate::look::{self, ph};

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(crate) enum RibbonTab {
    #[default]
    Home,
    Capture,
    Record,
    Studio,
    Stream,
    Edit,
    Image,
    Share,
    View,
    Help,
}

impl RibbonTab {
    pub const ALL: [RibbonTab; 10] =
        [RibbonTab::Home, RibbonTab::Capture, RibbonTab::Record, RibbonTab::Studio, RibbonTab::Stream, RibbonTab::Edit, RibbonTab::Image, RibbonTab::Share, RibbonTab::View, RibbonTab::Help];

    pub fn name(self) -> &'static str {
        match self {
            RibbonTab::Home => t("Home"),
            RibbonTab::Capture => t("Capture"),
            RibbonTab::Record => t("Record"),
            RibbonTab::Studio => t("Studio"),
            RibbonTab::Stream => t("Stream"),
            RibbonTab::Edit => t("Edit"),
            RibbonTab::Image => t("Image"),
            RibbonTab::Share => t("Share"),
            RibbonTab::View => t("View"),
            RibbonTab::Help => t("Help"),
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            RibbonTab::Home => t("The things you use most: capture, record, copy, save"),
            RibbonTab::Capture => t("Every way to capture the screen"),
            RibbonTab::Record => t("Record videos and GIFs, with sound"),
            RibbonTab::Studio => t("Studio mode: scenes made of screens, windows, pictures and text, recorded live"),
            RibbonTab::Stream => t("Go live on Twitch, YouTube, Kick, Facebook or your own server"),
            RibbonTab::Edit => t("Undo, marks and the clipboard"),
            RibbonTab::Image => t("Turn, resize, effects and hiding private details"),
            RibbonTab::Share => t("Copy, save, email, print and export"),
            RibbonTab::View => t("Zoom, panels and language"),
            RibbonTab::Help => t("Help, updates and information about Snagster"),
        }
    }
}

/// Window state added with the ribbon: Studio, languages, updates, the HTTP server for AI apps.
#[derive(Default)]
pub(crate) struct Extra {
    pub tab: RibbonTab,
    pub studio: Option<crate::app::studio_ui::StudioUi>,
    pub lang_open: bool,
    pub lang_query: String,
    /// The language list was opened: it needs every script's font.
    pub all_scripts: bool,
    pub update: crate::app::updates_ui::UpdateUi,
    pub mcp_server: Option<crate::mcp_http::Server>,
    pub mcp_error: Option<String>,
    pub mcp_wanted: Option<(u16, String)>,
    /// Microphones, and this computer's video encoders: both listed on a thread of their own
    /// (asking the sound system or Media Foundation can take a while).
    pub mics: crate::app::work::ListCache<crate::audio::Device>,
    pub encoders: crate::app::work::ListCache<crate::video::EncoderInfo>,
    /// The encoders the person can choose (`encode::available`).
    pub pickable: crate::app::work::ListCache<(crate::encode::Codec, crate::encode::EncoderName)>,
    pub stream: crate::app::stream_ui::StreamUi,
    pub access: crate::app::access_ui::AccessUi,
}

fn kbd(s: &Settings, a: Action) -> String {
    let c = s.binding(a);
    if c.is_set() { format!(" ({})", c.label()) } else { String::new() }
}

impl SnagApp {
    /// The tab row and the band under it.
    pub(crate) fn ribbon(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // ---- tab row ----
        let tabs_rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(Rect::from_min_size(tabs_rect.min, vec2(tabs_rect.width(), 34.0)), CornerRadius::ZERO, look::TABS);
        ui.horizontal(|ui| {
            ui.set_height(34.0);
            // The title bar shows Snagster's icon and name, so the tabs start at the edge.
            ui.add_space(6.0);
            // File: a menu, like Word's.
            let file = look::tab(ui, t("File"), t("New, open, save, export and settings"), false);
            egui::Popup::menu(&file).show(|ui| {
                    ui.set_min_width(260.0);
                    self.file_menu(ui);
                });
            for tab in RibbonTab::ALL {
                let on = self.x.tab == tab && !self.settings.ribbon_folded;
                if look::tab(ui, tab.name(), tab.tip(), on || (tab == RibbonTab::Studio && self.x.tab == RibbonTab::Studio)).clicked() {
                    if self.x.tab == tab {
                        self.settings.ribbon_folded = !self.settings.ribbon_folded;
                    } else {
                        self.settings.ribbon_folded = false;
                        self.switch_tab(&ctx, tab);
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(6.0);
                let snap = self.x.stream.snap.clone();
                if snap.on_air() && crate::app::stream_ui::live_badge(ui, &snap).clicked() {
                    self.switch_tab(&ctx, RibbonTab::Stream);
                }
                if look::icon_button(ui, ph::GEAR, &format!("{} (Ctrl+,)", t("Settings"))).clicked() {
                    self.settings_open = true;
                }
                if look::icon_button(ui, ph::GLOBE, &tf("Language: {name}", &[("name", &crate::i18n::name(crate::i18n::current()))])).clicked() {
                    self.x.lang_open = true;
                }
                let fold = if self.settings.ribbon_folded { ph::CARET_DOWN } else { ph::CARET_UP };
                if look::icon_button(ui, fold, if self.settings.ribbon_folded { t("Show the ribbon") } else { t("Hide the ribbon") }).clicked() {
                    self.settings.ribbon_folded = !self.settings.ribbon_folded;
                }
                if self.x.update.available().is_some() && look::icon_button(ui, ph::DOWNLOAD, t("A new version of Snagster is out")).clicked() {
                    self.x.update.dialog = true;
                }
            });
        });
        if self.settings.ribbon_folded {
            return;
        }
        // ---- band ----
        egui::Frame::new().fill(look::BAND).inner_margin(egui::Margin { left: 8, right: 8, top: 4, bottom: 2 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::horizontal().id_salt("ribbon-band").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    match self.x.tab {
                    RibbonTab::Home => self.band_home(ui, &ctx),
                    RibbonTab::Capture => self.band_capture(ui, &ctx),
                    RibbonTab::Record => self.band_record(ui, &ctx),
                    RibbonTab::Studio => self.band_studio(ui, &ctx),
                    RibbonTab::Stream => crate::app::stream_ui::band(self, ui, &ctx),
                    RibbonTab::Edit => self.band_edit(ui),
                    RibbonTab::Image => self.band_image(ui),
                    RibbonTab::Share => self.band_share(ui),
                    RibbonTab::View => self.band_view(ui, &ctx),
                    RibbonTab::Help => self.band_help(ui),
                    }
                    self.more_group(ui, &ctx);
                });
            });
        });
    }

    pub(crate) fn switch_tab(&mut self, ctx: &egui::Context, tab: RibbonTab) {
        if tab == RibbonTab::Studio && self.x.studio.is_none() {
            self.x.studio = Some(crate::app::studio_ui::StudioUi::start(&self.settings, ctx));
        }
        if tab != RibbonTab::Studio
            && let Some(s) = &self.x.studio
            && !s.recording()
            && !(self.x.stream.uses_studio && self.x.stream.snap.phase != crate::stream::live::Phase::Idle)
        {
            // Leaving Studio stops its preview (and the screen copies behind it).
            self.x.studio = None;
        }
        self.x.tab = tab;
    }

    fn capture_tool(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, big: bool, icon: &str, a: Action, label: &str, tip: &str) {
        let busy = self.capturing.is_some();
        let tip = format!("{tip}{}", kbd(&self.settings, a));
        let clicked = if big { look::big_tool(ui, icon, label, &tip, !busy, false).clicked() } else { look::small_tool(ui, icon, label, &tip, !busy).clicked() };
        if clicked {
            self.do_action(ctx, a);
        }
    }

    /// Window: a list of the open windows to pick from (as ShareX has), and "click a window on
    /// the screen" first. The Window hotkey still lets you click one.
    fn window_tool(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, big: bool) {
        let tip = format!("{}{}", t("Pick a window from the list, or click one on the screen"), kbd(&self.settings, Action::Window));
        let (mut click, mut pick) = (false, None);
        let body = |ui: &mut egui::Ui| {
            ui.set_min_width(320.0);
            if ui.button(format!("{}  {}", ph::CURSOR_CLICK, t("Click a window on the screen"))).clicked() {
                click = true;
                ui.close();
            }
            ui.separator();
            pick = self.window_list(ui);
        };
        if big {
            look::big_menu(ui, ph::APP_WINDOW, t("Window"), &tip, body);
        } else {
            look::small_menu(ui, ph::APP_WINDOW, t("Window"), &tip, body);
        }
        if click {
            self.do_action(ctx, Action::Window);
        } else if let Some(k) = pick
            && self.capturing.is_none()
        {
            self.start_capture(ctx, k);
        }
    }

    fn band_home(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        look::group(ui, t("Capture"), |ui| {
            self.capture_tool(ui, ctx, true, ph::CAMERA, Action::AllInOne, t("Capture"), t("Drag an area, or click a window, button or menu; then capture, scroll, record or grab text"));
            self.capture_tool(ui, ctx, true, ph::SELECTION, Action::Area, t("Area"), t("Drag a rectangle to capture"));
            look::column(ui, |ui| {
                self.window_tool(ui, ctx, false);
                self.capture_tool(ui, ctx, false, ph::MONITOR, Action::Screen, t("Screen"), t("Capture the screen the mouse is on"));
                self.capture_tool(ui, ctx, false, ph::SCROLL, Action::Scrolling, t("Scrolling"), t("Capture a whole long page by scrolling it"));
            });
        });
        look::group(ui, t("Record"), |ui| {
            self.record_button(ui, ctx);
            look::column(ui, |ui| self.record_choices(ui));
        });
        let has = self.current.is_some();
        look::group(ui, t("Clipboard"), |ui| {
            look::column(ui, |ui| {
                if look::small_tool(ui, ph::COPY, t("Copy"), &format!("{} (Ctrl+C)", t("Copy the picture with its marks")), has).clicked() {
                    self.copy();
                }
                if look::small_tool(ui, ph::CLIPBOARD, t("Paste"), &format!("{} (Ctrl+V)", t("A new picture from the clipboard")), true).clicked() {
                    self.paste();
                }
                if look::small_tool(ui, ph::PUSH_PIN, t("Pin"), t("Keep the picture on top of every window"), has).clicked() {
                    self.pin_current();
                }
            });
        });
        let (can_undo, can_redo) = self.current.map(|i| (!self.entries[i].undo.is_empty(), !self.entries[i].redo.is_empty())).unwrap_or((false, false));
        look::group(ui, t("History"), |ui| {
            look::column(ui, |ui| {
                if look::small_tool(ui, ph::UNDO, t("Undo"), "Ctrl+Z", can_undo).clicked() {
                    self.undo();
                }
                if look::small_tool(ui, ph::REDO, t("Redo"), "Ctrl+Y", can_redo).clicked() {
                    self.redo();
                }
            });
        });
        look::group(ui, t("File"), |ui| {
            if look::big_tool(ui, ph::FLOPPY_DISK, t("Save"), &format!("{} (Ctrl+S)", t("Save as a picture or a project")), has, false).clicked() {
                self.save_as();
            }
            look::column(ui, |ui| {
                if look::small_tool(ui, ph::FOLDER_OPEN, t("Open"), &format!("{} (Ctrl+O)", t("Open a picture or a Snagster project")), true).clicked() {
                    self.open_dialog();
                }
                if look::small_tool(ui, ph::IMAGES, t("Library"), &format!("{} (Ctrl+L)", t("Every capture, searchable")), true).clicked() {
                    self.library_open = true;
                }
                if look::small_tool(ui, ph::LAYOUT, t("Panel"), t("The compact capture panel (also from the icon in the taskbar corner)"), true).clicked() {
                    self.enter_panel(ui.ctx());
                }
            });
        });
        look::group(ui, t("Delay"), |ui| {
            look::column(ui, |ui| self.delay_menu(ui));
        });
    }

    fn delay_menu(&mut self, ui: &mut egui::Ui) {
        let d = self.settings.delay_s;
        let label = if d == 0 { t("No delay").to_string() } else { tf("{n} s delay", &[("n", &d.to_string())]) };
        look::small_menu(ui, ph::TIMER, &label, t("Wait before capturing (to open a menu first)"), |ui| {
            for d in [0u64, 1, 2, 3, 5, 10, 15, 30] {
                let label = if d == 0 { t("No delay").to_string() } else { tf("{n} seconds", &[("n", &d.to_string())]) };
                if ui.radio(self.settings.delay_s == d, label).clicked() {
                    self.settings.delay_s = d;
                    ui.close();
                }
            }
        });
        if look::small_tool(ui, ph::CLOCK, t("Timer"), t("Scheduled and repeated captures"), true).clicked() {
            self.timer_open = true;
        }
    }

    fn record_button(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.is_recording() {
            if look::big_tool(ui, ph::STOP, t("Stop"), t("Stop recording and save"), true, true).clicked() {
                self.stop_job();
            }
        } else {
            let tip = format!(
                "{}{}",
                tf("Drag the area to record ({format}, {fps} fps, {sound})", &[
                    ("format", &self.settings.rec_format.ext().to_uppercase()),
                    ("fps", &if self.settings.rec_format == crate::video::VideoFormat::Gif { self.settings.gif_fps } else { self.settings.video_fps }.to_string()),
                    ("sound", t(self.settings.audio_source.label())),
                ]),
                kbd(&self.settings, Action::RecordGif)
            );
            if look::big_tool(ui, ph::RECORD, t("Record"), &tip, self.capturing.is_none(), false).clicked() {
                self.do_action(ctx, Action::RecordGif);
            }
        }
    }

    /// Format, frame rate and sound pickers (small menus).
    fn record_choices(&mut self, ui: &mut egui::Ui) {
        let f = self.settings.rec_format;
        look::small_menu(ui, ph::FILM_STRIP, &f.ext().to_uppercase(), t("What to record as: MP4, HEVC, WMV or GIF"), |ui| {
            for v in crate::video::VideoFormat::ALL {
                if v.available() && ui.radio(self.settings.rec_format == v, t(v.label())).clicked() {
                    self.settings.rec_format = v;
                    ui.close();
                }
            }
        });
        let fps = self.settings.video_fps;
        look::small_menu(ui, ph::VIDEO, &format!("{fps} fps"), t("Frames per second (1-120)"), |ui| {
            for v in [1, 5, 10, 15, 24, 30, 50, 60, 90, 120] {
                if ui.radio(self.settings.video_fps == v, format!("{v} fps")).clicked() {
                    self.settings.video_fps = v;
                }
            }
            ui.add(egui::Slider::new(&mut self.settings.video_fps, 1..=120).text("fps"));
        });
        let src = self.settings.audio_source;
        let icon = if src == crate::audio::AudioSource::None { ph::SPEAKER_SLASH } else if src.mic() { ph::MICROPHONE } else { ph::SPEAKER };
        look::small_menu(ui, icon, t(src.label()), t("Sound to record with videos"), |ui| self.sound_menu(ui));
    }

    pub(crate) fn sound_menu(&mut self, ui: &mut egui::Ui) {
        if !crate::audio::AudioSource::supported() {
            ui.label(RichText::new(t("Recording sound isn't available on this system yet.")).color(TEXT_DIM));
            return;
        }
        for v in crate::audio::AudioSource::ALL {
            if ui.radio(self.settings.audio_source == v, t(v.label())).clicked() {
                self.settings.audio_source = v;
            }
        }
        ui.separator();
        ui.label(RichText::new(t("Microphone")).strong());
        let mics = self.x.mics.get(Some(ui.ctx()), crate::audio::microphones).to_vec();
        if ui.radio(self.settings.audio_mic.is_empty(), t("The computer's default microphone")).clicked() {
            self.settings.audio_mic.clear();
        }
        for m in mics {
            if ui.radio(self.settings.audio_mic == m.id, &m.name).clicked() {
                self.settings.audio_mic = m.id.clone();
            }
        }
        if ui.small_button(t("Look for microphones again")).clicked() {
            self.x.mics.expire();
        }
        if self.settings.rec_format == crate::video::VideoFormat::Wmv || self.settings.rec_format == crate::video::VideoFormat::Gif {
            ui.label(RichText::new(t("Sound is recorded into MP4 videos.")).color(TEXT_DIM));
        }
    }

    fn band_capture(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        look::group(ui, t("Capture"), |ui| {
            self.capture_tool(ui, ctx, true, ph::CAMERA, Action::AllInOne, t("All-in-one"), t("Drag an area, or click a window, button or menu"));
            self.capture_tool(ui, ctx, true, ph::SELECTION, Action::Area, t("Area"), t("Drag a rectangle to capture"));
            self.window_tool(ui, ctx, true);
            self.capture_tool(ui, ctx, true, ph::MONITOR, Action::Screen, t("Screen"), t("Capture the screen the mouse is on"));
            look::column(ui, |ui| {
                self.capture_tool(ui, ctx, false, ph::SQUARES_FOUR, Action::AllScreens, t("All screens"), t("Every screen in one picture"));
                self.capture_tool(ui, ctx, false, ph::BROWSER, Action::ActiveWindow, t("Window in front"), t("The window you're working in"));
                let has_last = self.last_region.is_some();
                if look::small_tool(ui, ph::ARROWS_CLOCKWISE, t("Last area"), &format!("{}{}", t("The same area as last time"), kbd(&self.settings, Action::LastArea)), has_last).clicked() {
                    self.do_action(ctx, Action::LastArea);
                }
            });
        });
        look::group(ui, t("Long pages"), |ui| {
            self.capture_tool(ui, ctx, true, ph::SCROLL, Action::Scrolling, t("Scrolling"), t("Scroll down a page and stitch it into one picture"));
            self.capture_tool(ui, ctx, true, ph::PANORAMA, Action::Panorama, t("Panorama"), t("Scroll sideways and stitch"));
            self.capture_tool(ui, ctx, true, ph::CURSOR_CLICK, Action::ManualScroll, t("Manual"), t("Scroll it yourself while Snagster stitches"));
        });
        look::group(ui, t("Shapes"), |ui| {
            look::column(ui, |ui| {
                self.capture_tool(ui, ctx, false, ph::CIRCLE, Action::Circle, t("Circle"), t("Capture a round area"));
                self.capture_tool(ui, ctx, false, ph::LASSO, Action::Freehand, t("Freehand"), t("Draw around what to capture"));
                self.capture_tool(ui, ctx, false, ph::POLYGON, Action::Polygon, t("Polygon"), t("Click corners around what to capture"));
            });
            look::column(ui, |ui| {
                self.capture_tool(ui, ctx, false, ph::SELECTION_PLUS, Action::MultiArea, t("Several areas"), t("Pick several areas into one picture"));
                self.capture_tool(ui, ctx, false, ph::LIST, Action::MenuCapture, t("Menu"), t("Open a menu; it's captured after the delay"));
                let mut pick = None;
                look::small_menu(ui, ph::STAR, t("More shapes"), t("Star, heart and other fun shapes, and fixed sizes"), |ui| {
                    for s in CutShape::ALL {
                        if ui.button(t(s.label())).clicked() {
                            pick = Some(CaptureKind::Pick(s, PickPurpose::Capture));
                            ui.close();
                        }
                    }
                    ui.separator();
                    for (w, h) in [(640, 480), (800, 600), (1024, 768), (1280, 720), (1920, 1080), (500, 500)] {
                        if ui.button(format!("{w} x {h}")).clicked() {
                            pick = Some(CaptureKind::Fixed(w, h));
                            ui.close();
                        }
                    }
                });
                if let Some(k) = pick {
                    self.start_capture(ctx, k);
                }
            });
        });
        look::group(ui, t("Tools"), |ui| {
            look::column(ui, |ui| {
                self.capture_tool(ui, ctx, false, ph::TEXT_AA, Action::GrabText, t("Grab text"), t("Copy the text out of part of the screen"));
                self.capture_tool(ui, ctx, false, ph::EYEDROPPER, Action::ColorPicker, t("Color picker"), t("Take a colour from the screen"));
                self.capture_tool(ui, ctx, false, ph::RULER, Action::Ruler, t("Ruler"), t("Measure distances on the screen"));
            });
        });
        look::group(ui, t("Timing"), |ui| {
            look::column(ui, |ui| {
                self.delay_menu(ui);
                let mut run = None;
                look::small_menu(ui, ph::STACK, t("Presets"), t("Your saved capture recipes"), |ui| {
                    for (i, p) in self.settings.presets.iter().enumerate() {
                        if ui.button(&p.name).clicked() {
                            run = Some(i);
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button(t("Edit presets...")).clicked() {
                        self.settings_open = true;
                        self.settings_tab = SettingsTab::Presets;
                        ui.close();
                    }
                });
                if let Some(i) = run {
                    self.run_preset(ctx, i);
                }
            });
        });
        look::group(ui, t("After capture"), |ui| {
            look::column(ui, |ui| {
                ui.checkbox(&mut self.settings.after_copy, t("Copy to clipboard")).on_hover_text(t("Every capture goes to the clipboard too"));
                ui.checkbox(&mut self.settings.after_save, t("Save to folder")).on_hover_text(t("Every capture is saved in the save folder"));
                ui.checkbox(&mut self.settings.include_cursor, t("Mouse pointer")).on_hover_text(t("Include the mouse pointer (you can move it afterwards)"));
            });
        });
    }

    fn band_record(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        look::group(ui, t("Record"), |ui| {
            self.record_button(ui, ctx);
            let busy = self.is_recording() || self.capturing.is_some();
            if look::big_tool(ui, ph::MONITOR_PLAY, t("Whole screen"), t("Record the screen the mouse is on"), !busy, false).clicked()
                && let Some(r) = self.screen_under_pointer()
            {
                self.start_record(ctx, r);
            }
            let paused = self.paused();
            if look::big_tool(ui, if paused { ph::PLAY } else { ph::PAUSE }, if paused { t("Resume") } else { t("Pause") }, t("Pause the video; nothing is recorded meanwhile"), self.mini == Some(Mini::Video), paused).clicked() {
                self.toggle_pause();
            }
        });
        look::group(ui, t("Video"), |ui| {
            look::column(ui, |ui| self.record_choices(ui));
            look::column(ui, |ui| {
                let q = self.settings.video_quality.clamp(1, 4) as usize;
                let names = ["", t("Small file"), t("Good"), t("High"), t("Best")];
                look::small_menu(ui, ph::SLIDERS, names[q], t("Picture quality (and file size)"), |ui| {
                    for (v, name) in names.iter().enumerate().skip(1) {
                        if ui.radio(q == v, *name).clicked() {
                            self.settings.video_quality = v as u8;
                        }
                    }
                });
                ui.checkbox(&mut self.settings.video_hardware, t("Graphics chip")).on_hover_text(t("Encode on the graphics chip (Intel, AMD or NVIDIA): fast and light on the battery"));
                ui.checkbox(&mut self.settings.gif_cursor, t("Mouse pointer")).on_hover_text(t("Show the mouse pointer in recordings"));
            });
        });
        look::group(ui, t("Guides"), |ui| {
            let on = self.mini == Some(Mini::Steps);
            if look::big_tool(ui, ph::FOOTPRINTS, if on { t("Stop steps") } else { t("Steps") }, &format!("{}{}", t("Every click takes a numbered picture, for a step-by-step guide"), kbd(&self.settings, Action::RecordSteps)), true, on).clicked() {
                self.do_action(ctx, Action::RecordSteps);
            }
        });
        look::group(ui, t("Recordings"), |ui| {
            let last = self.videos.first().map(|v| v.path.clone());
            look::column(ui, |ui| {
                if look::small_tool(ui, ph::PLAY, t("Play last video"), t("Open the last video in your video player"), last.is_some()).clicked()
                    && let Some(p) = &last
                {
                    self.open_file(p);
                }
                if look::small_tool(ui, ph::FOLDER_OPEN, t("Recordings folder"), t("Open the folder your videos are saved in"), true).clicked() {
                    let f = PathBuf::from(&self.settings.video_folder);
                    let _ = std::fs::create_dir_all(&f);
                    self.open_folder(&f);
                }
                if look::small_tool(ui, ph::GIF, t("GIF editor"), t("Trim GIF recordings, take frames out, combine them"), true).clicked() {
                    self.gif_open = true;
                }
            });
            look::column(ui, |ui| {
                if look::small_tool(ui, ph::GEAR, t("Video settings"), t("Formats, quality, sound and where videos go"), true).clicked() {
                    self.settings_open = true;
                    self.settings_tab = SettingsTab::Recording;
                }
                if look::small_tool(ui, ph::IMAGES_SQUARE, t("GIF slideshow"), t("Make a GIF slideshow from the pictures in Recent"), self.entries.len() >= 2).clicked() {
                    self.slideshow(ctx);
                }
            });
        });
    }

    fn band_edit(&mut self, ui: &mut egui::Ui) {
        let (can_undo, can_redo, has_marks) = self.current.map(|i| (!self.entries[i].undo.is_empty(), !self.entries[i].redo.is_empty(), !self.entries[i].doc.shapes.is_empty())).unwrap_or((false, false, false));
        let has = self.current.is_some();
        let sel = self.selected.is_some();
        look::group(ui, t("History"), |ui| {
            if look::big_tool(ui, ph::UNDO, t("Undo"), "Ctrl+Z", can_undo, false).clicked() {
                self.undo();
            }
            if look::big_tool(ui, ph::REDO, t("Redo"), "Ctrl+Y", can_redo, false).clicked() {
                self.redo();
            }
        });
        look::group(ui, t("Marks"), |ui| {
            look::column(ui, |ui| {
                if look::small_tool(ui, ph::TRASH, t("Delete mark"), "Del", sel).clicked() {
                    self.delete_selected();
                }
                if look::small_tool(ui, ph::PLUS, t("Bigger"), "]", sel).clicked() {
                    self.scale_selected(1.15);
                }
                if look::small_tool(ui, ph::ARROWS_IN, t("Smaller"), "[", sel).clicked() {
                    self.scale_selected(1.0 / 1.15);
                }
            });
            look::column(ui, |ui| {
                if look::small_tool(ui, ph::X, t("Clear all marks"), t("Remove every mark from the picture"), has_marks).clicked() {
                    self.clear_marks();
                }
                if look::small_tool(ui, ph::SCISSORS, t("Cut mark"), "Ctrl+X", sel).clicked() {
                    self.cut_mark();
                }
                if look::small_tool(ui, ph::COPY, t("Copy mark"), "Ctrl+C", sel).clicked() {
                    self.copy_mark();
                }
            });
            look::column(ui, |ui| {
                if look::small_tool(ui, ph::CLIPBOARD, t("Paste mark"), "Ctrl+V", self.mark_clip.is_some() && has).clicked() {
                    self.paste_mark();
                }
                if look::small_tool(ui, ph::IMAGE, t("Paste picture onto"), t("Paste the clipboard's picture as a piece you can move"), has).clicked() {
                    self.paste_as_piece();
                }
            });
        });
        look::group(ui, t("Picture"), |ui| {
            look::column(ui, |ui| {
                if look::small_tool(ui, ph::CARDS, t("Duplicate"), t("A copy of this picture in Recent"), has).clicked() {
                    self.duplicate();
                }
                if look::small_tool(ui, ph::X, t("Remove from Recent"), t("Take it out of the Recent row (the file stays)"), has).clicked() {
                    self.remove_current();
                }
                if look::small_tool(ui, ph::TEXT_AA, t("Grab text"), t("Copy the text out of this picture"), has && !self.ocr_busy).clicked()
                    && let Some(i) = self.current
                {
                    let img = render::compose(&self.entries[i].doc);
                    self.run_ocr(OcrPurpose::GrabText, img);
                }
            });
        });
    }

    fn effect_tool(&mut self, ui: &mut egui::Ui, icon: &str, e: Effect, has: bool) {
        if look::small_tool(ui, icon, t(e.label()), t(e.label()), has).clicked() {
            self.apply_effect(e);
        }
    }

    fn band_image(&mut self, ui: &mut egui::Ui) {
        let has = self.current.is_some();
        look::group(ui, t("Turn"), |ui| {
            look::column(ui, |ui| {
                self.effect_tool(ui, ph::ARROW_COUNTER_CLOCKWISE, Effect::RotateLeft, has);
                self.effect_tool(ui, ph::ARROW_CLOCKWISE, Effect::RotateRight, has);
            });
            look::column(ui, |ui| {
                self.effect_tool(ui, ph::FLIP_HORIZONTAL, Effect::FlipHorizontal, has);
                self.effect_tool(ui, ph::ROWS, Effect::FlipVertical, has);
            });
        });
        look::group(ui, t("Size"), |ui| {
            if look::big_tool(ui, ph::ARROWS_OUT, t("Resize"), t("Change the picture's size"), has, false).clicked()
                && let Some(i) = self.current
            {
                let (w, h) = self.entries[i].doc.image.dimensions();
                self.dialog = Some(crate::windows::Dialog::Resize { w, h, keep: true, ratio: w as f32 / h.max(1) as f32 });
            }
            look::column(ui, |ui| {
                if look::small_tool(ui, ph::CROP, t("Crop"), t("Drag the part to keep (the Crop tool)"), has).clicked() {
                    self.tool = Tool::Crop;
                }
                if look::small_tool(ui, ph::FRAME_CORNERS, t("Trim edges"), t("Cut away flat-coloured edges"), has).clicked() {
                    self.image_op("Trimmed", effects::trim);
                }
                if look::small_tool(ui, ph::SQUARE, t("Canvas"), t("Canvas size and colour"), has).clicked()
                    && let Some(i) = self.current
                {
                    let (w, h) = self.entries[i].doc.image.dimensions();
                    self.dialog = Some(crate::windows::Dialog::Canvas { w: w + 80, h: h + 80, color: [255, 255, 255, 255] });
                }
            });
        });
        look::group(ui, t("Effects"), |ui| {
            let mut chosen = None;
            look::big_menu(ui, ph::MAGIC_WAND, t("Effects"), t("Borders, shadows, edges, colours and more"), |ui| {
                for e in Effect::ALL {
                    if e.starts_group() {
                        ui.separator();
                    }
                    if ui.add_enabled(has, egui::Button::new(t(e.label()))).clicked() {
                        chosen = Some(e);
                        ui.close();
                    }
                }
            });
            if let Some(e) = chosen {
                self.apply_effect(e);
            }
            look::column(ui, |ui| {
                self.effect_tool(ui, ph::SQUARE, Effect::Border, has);
                self.effect_tool(ui, ph::STACK, Effect::DropShadow, has);
                self.effect_tool(ui, ph::SCISSORS, Effect::TornEdge, has);
            });
            look::column(ui, |ui| {
                self.effect_tool(ui, ph::DROP, Effect::Grayscale, has);
                self.effect_tool(ui, ph::FRAME_CORNERS, Effect::RoundedCorners, has);
                if look::small_tool(ui, ph::GEAR, t("Effect settings"), t("Border width, shadow and the other effect choices"), true).clicked() {
                    self.settings_open = true;
                    self.settings_tab = SettingsTab::Effects;
                }
            });
        });
        look::group(ui, t("Privacy"), |ui| {
            let busy = self.ocr_busy;
            if look::big_tool(ui, ph::EYE_SLASH, t("Smart redact"), t("Find and blur emails, phone numbers, card numbers and more"), has && !busy, false).clicked()
                && let Some(i) = self.current
            {
                let img = render::compose(&self.entries[i].doc);
                self.run_ocr(OcrPurpose::Redact(i), img);
            }
            if look::big_tool(ui, ph::TEXTBOX, t("Simplify"), t("Turn text into plain bars, for a clean diagram"), has && !busy, false).clicked()
                && let Some(i) = self.current
            {
                let img = render::compose(&self.entries[i].doc);
                self.run_ocr(OcrPurpose::Simplify(i), img);
            }
        });
    }

    fn band_share(&mut self, ui: &mut egui::Ui) {
        let has = self.current.is_some();
        look::group(ui, t("Send"), |ui| {
            if look::big_tool(ui, ph::COPY, t("Copy"), &format!("{} (Ctrl+C)", t("Copy the picture with its marks")), has, false).clicked() {
                self.copy();
            }
            if look::big_tool(ui, ph::ENVELOPE, t("Email"), t("Attach the picture to a new email"), has, false).clicked() {
                self.email();
            }
            if look::big_tool(ui, ph::PRINTER, t("Print"), "Ctrl+P", has, false).clicked() {
                self.print();
            }
        });
        look::group(ui, t("Save"), |ui| {
            if look::big_tool(ui, ph::FLOPPY_DISK, t("Save as"), t("Save as a picture or a project"), has, false).clicked() {
                self.save_as();
            }
            look::column(ui, |ui| {
                if look::small_tool(ui, ph::DOWNLOAD, t("Quick save"), &format!("{} (Ctrl+Shift+S)", t("Save to the save folder at once")), has).clicked() {
                    self.quick_save();
                }
                if look::small_tool(ui, ph::ARROW_SQUARE_OUT, t("Open with"), t("Open in the computer's picture program"), has).clicked() {
                    self.open_externally();
                }
                if look::small_tool(ui, ph::FOLDER_OPEN, t("Save folder"), t("Open the folder pictures are saved in"), true).clicked() {
                    let f = PathBuf::from(&self.settings.save_folder);
                    let _ = std::fs::create_dir_all(&f);
                    self.open_folder(&f);
                }
            });
        });
        look::group(ui, t("Documents"), |ui| {
            look::column(ui, |ui| {
                if look::small_tool(ui, ph::FILE_DOC, t("Word document"), t("All pictures in Recent in a Word document"), !self.entries.is_empty()).clicked() {
                    self.export_word();
                }
                if look::small_tool(ui, ph::FILE_PDF, t("PDF guide"), t("A step-by-step PDF from recorded steps or Recent"), has).clicked() {
                    self.export_guide("pdf");
                }
                if look::small_tool(ui, ph::BROWSER, t("Web page guide"), t("A step-by-step web page"), has).clicked() {
                    self.export_guide("html");
                }
            });
        });
        look::group(ui, t("Cloud"), |ui| {
            let clouds = crate::settings::cloud_folders_now(Some(ui.ctx()));
            look::small_menu(ui, ph::CLOUD_UP, t("Save to the cloud"), t("OneDrive, Dropbox, Google Drive or iCloud folders on this computer"), |ui| {
                if clouds.is_empty() {
                    ui.label(RichText::new(t("No OneDrive, Dropbox, Google Drive or iCloud folder found.")).color(TEXT_DIM));
                }
                for (name, p) in clouds {
                    if ui.add_enabled(has, egui::Button::new(tf("Save to {name}", &[("name", &name)]))).clicked()
                        && let Some(i) = self.current
                    {
                        let title = self.entries[i].name.clone();
                        let base = self.settings.file_name(&title);
                        let path = crate::settings::unique_path(&p.join("Snagster"), &base, self.settings.format.ext());
                        self.save_in_background(i, path);
                        ui.close();
                    }
                }
            });
        });
    }

    fn band_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        look::group(ui, t("Zoom"), |ui| {
            if look::big_tool(ui, ph::ARROWS_IN, t("Fit"), "0", true, false).clicked() {
                self.set_zoom(ctx, None);
            }
            look::column(ui, |ui| {
                if look::small_tool(ui, ph::FRAME_CORNERS, t("Actual size"), "1", true).clicked() {
                    self.set_zoom(ctx, Some(0.0));
                }
                if look::small_tool(ui, ph::ZOOM_IN, t("Zoom in"), "+", true).clicked() {
                    self.set_zoom(ctx, Some(1.25));
                }
                if look::small_tool(ui, ph::ZOOM_OUT, t("Zoom out"), "-", true).clicked() {
                    self.set_zoom(ctx, Some(0.8));
                }
            });
        });
        look::group(ui, t("Show"), |ui| {
            look::column(ui, |ui| {
                ui.checkbox(&mut self.settings.show_side_panel, t("Options panel")).on_hover_text(t("Colours, sizes and styles on the right"));
                ui.checkbox(&mut self.settings.show_tray, t("Recent pictures")).on_hover_text(t("The row of recent pictures at the bottom"));
            });
        });
        look::group(ui, t("Windows"), |ui| {
            if look::big_tool(ui, ph::LAYOUT, t("Capture panel"), t("The compact capture panel"), true, false).clicked() {
                self.enter_panel(ctx);
            }
            if look::big_tool(ui, ph::IMAGES, t("Library"), "Ctrl+L", true, false).clicked() {
                self.library_open = true;
            }
        });
        look::group(ui, t("Language"), |ui| {
            if look::big_tool(ui, ph::TRANSLATE, &crate::i18n::name(crate::i18n::current()), t("Change the language of Snagster's buttons and menus"), true, false).clicked() {
                self.x.lang_open = true;
            }
        });
    }

    fn band_help(&mut self, ui: &mut egui::Ui) {
        look::group(ui, t("Help"), |ui| {
            if look::big_tool(ui, ph::QUESTION, t("Getting started"), t("How to capture, mark up and share"), true, false).clicked() {
                self.dialog = Some(crate::windows::Dialog::GettingStarted);
            }
            if look::big_tool(ui, ph::KEYBOARD, t("Shortcuts"), "F1", true, false).clicked() {
                self.shortcuts_open = true;
            }
            if look::big_tool(ui, ph::SPARKLE, t("What's new"), t("What changed in this version"), true, false).clicked() {
                self.dialog = Some(crate::windows::Dialog::WhatsNew);
            }
        });
        look::group(ui, t("Updates"), |ui| {
            let checking = self.x.update.checking();
            if look::big_tool(ui, ph::DOWNLOAD, if checking { t("Checking...") } else { t("Check for updates") }, t("Ask GitHub whether a newer Snagster is out"), !checking, false).clicked() {
                self.x.update.check(ui.ctx(), true);
            }
        });
        look::group(ui, t("Snagster"), |ui| {
            if look::big_tool(ui, ph::ROBOT, t("AI apps"), t("Let Claude, ChatGPT, Cursor and local AI apps use Snagster"), true, false).clicked() {
                self.settings_open = true;
                self.settings_tab = SettingsTab::Ai;
            }
            if look::big_tool(ui, ph::INFO, t("About"), t("Version and licence"), true, false).clicked() {
                self.about_open = true;
            }
            look::column(ui, |ui| {
                if look::small_tool(ui, ph::WARNING, t("Problem reports"), t("Open the folder with Snagster's crash log, to send it with a bug report"), true).clicked()
                    && let Some(p) = crate::crash_log_path()
                    && let Some(dir) = p.parent()
                {
                    let _ = std::fs::create_dir_all(dir);
                    self.open_folder(dir);
                }
            });
        });
    }

    /// The last group of every tab: the tab's full menu, so every command stays one click away
    /// (window and screen lists, presets, templates, recent files...).
    fn more_group(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let tip = t("Every command of this tab in one menu");
        match self.x.tab {
            RibbonTab::Studio | RibbonTab::Help => {}
            tab => look::group(ui, t("More"), |ui| {
                look::column(ui, |ui| {
                    look::small_menu(ui, ph::LIST, t("All commands"), tip, |ui| match tab {
                        RibbonTab::Home => self.file_menu(ui),
                        RibbonTab::Capture => self.capture_menu(ui, ctx),
                        RibbonTab::Record => self.record_menu(ui, ctx),
                        RibbonTab::Edit => self.edit_menu(ui),
                        RibbonTab::Image => self.image_menu(ui),
                        RibbonTab::Share => self.share_menu(ui),
                        RibbonTab::Stream => self.stream_menu(ui, ctx),
                        _ => self.view_menu(ui, ctx),
                    });
                    if matches!(tab, RibbonTab::Image | RibbonTab::Capture | RibbonTab::Edit) {
                        look::small_menu(ui, ph::SLIDERS, t("Tools"), t("Grab text, colour picker, ruler, pins, combining pictures"), |ui| self.tools_menu(ui, ctx));
                    }
                });
            }),
        }
    }

    fn band_studio(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        crate::app::studio_ui::band(self, ui, ctx);
    }

    /// The language list: every language named in the current one, with its own name beside.
    pub(crate) fn language_window(&mut self, ctx: &egui::Context) {
        if !self.x.lang_open {
            return;
        }
        self.x.all_scripts = true;
        let mut open = true;
        let current = crate::i18n::current();
        crate::windows::own_window(ctx, crate::windows::Win { key: "language", title: t("Language"), size: vec2(440.0, 600.0), resizable: true }, &mut open, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(look::ph::GLOBE).family(look::icons()).size(16.0));
                    ui.add(egui::TextEdit::singleline(&mut self.x.lang_query).hint_text(t("Find a language")).desired_width(f32::INFINITY));
                });
                ui.add_space(4.0);
                let auto = self.settings.language.is_empty();
                let sys = crate::i18n::system();
                if ui.selectable_label(auto, tf("Automatic: the computer's language ({name})", &[("name", &crate::i18n::name(sys))])).clicked() {
                    self.settings.language.clear();
                }
                ui.separator();
                let q = self.x.lang_query.to_lowercase();
                let mut rows: Vec<(String, String, &str)> = crate::i18n::CODES
                    .iter()
                    .map(|c| (crate::i18n::name(c), crate::i18n::native_name(c), *c))
                    .filter(|(n, native, c)| q.is_empty() || n.to_lowercase().contains(&q) || native.to_lowercase().contains(&q) || c.to_lowercase().contains(&q))
                    .collect();
                rows.sort_by_key(|r| r.0.to_lowercase());
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (name, native, code) in rows {
                        let on = !auto && current == code;
                        let label = if name == native { name.clone() } else { format!("{name}  ·  {native}") };
                        if ui.selectable_label(on, label).on_hover_text(code).clicked() {
                            self.settings.language = code.to_string();
                        }
                    }
                });
            });
        if !open {
            self.x.lang_open = false;
        }
    }

    /// Keep the interface language, fonts and text direction in step with the setting.
    pub(crate) fn apply_language(&mut self, ctx: &egui::Context) {
        let lang = crate::i18n::resolve(&self.settings.language);
        if crate::i18n::current() != lang || ctx.language().as_deref() != Some(lang) {
            crate::i18n::set(lang);
            ctx.set_language(Some(lang.to_string()));
            let rtl = crate::i18n::is_rtl(lang);
            ctx.global_style_mut(|s| s.right_to_left = rtl);
        }
        let cjk = crate::i18n::is_cjk(lang) || self.x.all_scripts;
        self.ui_fonts.ensure_base(ctx, lang, cjk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_lists_every_tab_once_starting_at_home() {
        // A match with no wildcard: adding a tab without listing it here fails to compile.
        let index = |t: RibbonTab| match t {
            RibbonTab::Home => 0,
            RibbonTab::Capture => 1,
            RibbonTab::Record => 2,
            RibbonTab::Studio => 3,
            RibbonTab::Stream => 4,
            RibbonTab::Edit => 5,
            RibbonTab::Image => 6,
            RibbonTab::Share => 7,
            RibbonTab::View => 8,
            RibbonTab::Help => 9,
        };
        for (i, t) in RibbonTab::ALL.iter().enumerate() {
            assert_eq!(index(*t), i, "{t:?} out of place");
        }
        assert_eq!(RibbonTab::default(), RibbonTab::Home);
        assert_eq!(RibbonTab::ALL[0], RibbonTab::default());
    }

    #[test]
    fn every_tab_has_a_unique_name_and_a_tip() {
        let mut names = std::collections::HashSet::new();
        for t in RibbonTab::ALL {
            assert!(!t.name().trim().is_empty(), "{t:?}");
            assert!(names.insert(t.name()), "two tabs are called {}", t.name());
            assert!(t.tip().len() > t.name().len(), "{t:?} tip should say more than its name");
        }
    }

    #[test]
    fn switching_between_ordinary_tabs_starts_no_studio() {
        let ctx = egui::Context::default();
        let mut a = SnagApp::blank(Settings::default(), false, None);
        for t in RibbonTab::ALL.into_iter().filter(|t| *t != RibbonTab::Studio) {
            a.switch_tab(&ctx, t);
            assert_eq!(a.x.tab, t);
            assert!(a.x.studio.is_none());
        }
    }

    #[test]
    fn kbd_shows_a_set_shortcut_in_brackets() {
        let s = Settings::default();
        for b in &s.bindings {
            let k = kbd(&s, b.action);
            if b.combo.is_set() {
                assert!(k.starts_with(" (") && k.ends_with(')'), "{k:?}");
            } else {
                assert!(k.is_empty());
            }
        }
    }
}
