//! Studio mode's window: scenes and sources on the left, the live picture in the middle (drag a
//! source to move it, its corner to resize it), the selected source's settings and the sound
//! mixer on the right, and the ribbon band with Record, Pause, scenes, sources and output.

use super::*;
use crate::i18n::{t, tf};
use crate::look::{self, ph};
use crate::studio::{Item, SourceKind};

struct Rec {
    stop: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    rx: Receiver<crate::video::VideoDone>,
    started: Instant,
    paused_for: Duration,
    paused_at: Option<Instant>,
    path: PathBuf,
}

#[derive(Clone, Copy, PartialEq)]
enum Grab {
    Move,
    Resize,
}

pub(crate) struct StudioUi {
    engine: crate::studio::Engine,
    tex: Option<TextureHandle>,
    tex_frame: u64,
    selected: Option<usize>,
    grab: Option<(usize, Grab, Pos2, (f32, f32, f32, f32))>,
    /// Sound levels for the mixer's meters: every input the scenes use.
    meters: Option<crate::audio::Capture>,
    meters_for: Vec<crate::audio::SoundInput>,
    rec: Option<Rec>,
    pub last: Option<crate::video::VideoDone>,
    windows: Vec<crate::capture::WindowInfo>,
    monitors: Vec<MonitorInfo>,
    /// The Wayland note on drawing was shown.
    told_preview_only: bool,
    /// The source a stroke on the preview started on (its points go to the desktop).
    ink_from: Option<crate::ink::Placed>,
}

impl StudioUi {
    pub fn start(s: &Settings, ctx: &egui::Context) -> StudioUi {
        let scene = s.studio.scene().cloned().unwrap_or_else(|| crate::studio::StudioSettings::default().scenes[0].clone());
        let engine = crate::studio::Engine::start(scene, s.studio.width, s.studio.height, Some(ctx.clone()));
        StudioUi {
            engine,
            tex: None,
            tex_frame: 0,
            selected: None,
            grab: None,
            meters: None,
            meters_for: Vec::new(),
            rec: None,
            last: None,
            windows: Vec::new(),
            monitors: capture::monitors(),
            told_preview_only: false,
            ink_from: None,
        }
    }

    /// The compositor's shared state (the live stream takes its frames from it).
    pub fn engine_shared(&self) -> Arc<crate::studio::Shared> {
        self.engine.shared.clone()
    }

    pub fn recording(&self) -> bool {
        self.rec.is_some()
    }

    fn elapsed(&self) -> Duration {
        self.rec.as_ref().map(|r| r.started.elapsed().saturating_sub(r.paused_for + r.paused_at.map(|p| p.elapsed()).unwrap_or_default())).unwrap_or_default()
    }
}

/// The ribbon band of the Studio tab.
pub(crate) fn band(app: &mut SnagApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    let recording = app.x.studio.as_ref().is_some_and(|s| s.recording());
    // Ctrl+number switches scenes once a frame, not again while the ribbon's More menu is drawn.
    if !crate::app::ribbon::fit::in_overflow()
        && let Some(i) = crate::app::stream_ui::scene_key(ctx)
        && i < app.settings.studio.scenes.len()
    {
        crate::app::stream_ui::switch_scene(app, i);
    }
    // Go live with the Studio scene, next to Record.
    let snap = app.x.stream.snap.clone();
    look::group(ui, t("Live"), |ui| {
        if snap.on_air() {
            if look::big_tool(ui, ph::STOP, t("Stop"), t("End the stream now"), true, true).clicked() {
                app.stop_stream();
            }
        } else {
            let tip = tf("Stream the Studio scene to {service}", &[("service", &crate::app::stream_ui::services_named(app.settings.stream.service, app.settings.stream.also_on().map(|a| a.service)))]);
            if look::big_tool(ui, ph::BROADCAST, t("Go live"), &tip, snap.phase == crate::stream::live::Phase::Idle, false).clicked() {
                app.settings.stream.source = crate::stream::Source::Studio;
                app.go_live(ctx);
            }
        }
    });
    look::group(ui, t("Record"), |ui| {
        if recording {
            if look::big_tool(ui, ph::STOP, t("Stop"), t("Stop recording and save the video"), true, true).clicked() {
                stop(app);
            }
            let paused = app.x.studio.as_ref().and_then(|s| s.rec.as_ref()).is_some_and(|r| r.pause.load(Ordering::Relaxed));
            if look::big_tool(ui, if paused { ph::PLAY } else { ph::PAUSE }, if paused { t("Resume") } else { t("Pause") }, t("Pause the recording; nothing is recorded meanwhile"), true, paused).clicked() {
                toggle_pause(app);
            }
        } else if look::big_tool(ui, ph::RECORD, t("Start recording"), t("Record the scene as it looks in the preview, with the sound chosen"), true, false).clicked() {
            start(app, ctx);
        }
    });
    ink_group(app, ui);
    look::group(ui, t("Scenes"), |ui| {
        look::column(ui, |ui| {
            if look::small_tool(ui, ph::PLUS, t("New scene"), t("An empty scene to fill with sources"), true).clicked() {
                new_scene(&mut app.settings.studio);
            }
            if look::small_tool(ui, ph::CARDS, t("Copy scene"), t("A copy of this scene"), true).clicked() {
                copy_scene(&mut app.settings.studio);
            }
            let many = app.settings.studio.scenes.len() > 1;
            if look::small_tool(ui, ph::TRASH, t("Delete scene"), t("Remove this scene"), many).clicked() {
                delete_scene(&mut app.settings.studio);
            }
        });
    });
    look::group(ui, t("Sources"), |ui| {
        let mut add = None;
        look::big_menu(ui, ph::STACK_PLUS, t("Add source"), t("Put a screen, window, area, picture, text or colour in the scene"), |ui| {
            add = add_source_menu(app, ui);
        });
        if let Some(item) = add {
            add_item(app, item);
        }
    });
    look::group(ui, t("Output"), |ui| {
        look::column(ui, |ui| {
            let fmt = if app.settings.rec_format == crate::video::VideoFormat::Gif { crate::video::VideoFormat::Mp4 } else { app.settings.rec_format };
            look::small_menu(ui, ph::FILM_STRIP, &fmt.ext().to_uppercase(), t("Video format"), |ui| {
                for v in [crate::video::VideoFormat::Mp4, crate::video::VideoFormat::Hevc, crate::video::VideoFormat::Wmv] {
                    if v.available() && ui.radio(fmt == v, t(v.label())).clicked() {
                        app.settings.rec_format = v;
                    }
                }
            });
            let fps = app.settings.studio.fps;
            look::small_menu(ui, ph::VIDEO, &format!("{fps} fps"), t("Frames per second while recording"), |ui| {
                for v in [10, 15, 24, 30, 50, 60, 90, 120] {
                    if ui.radio(app.settings.studio.fps == v, format!("{v} fps")).clicked() {
                        app.settings.studio.fps = v;
                    }
                }
                ui.add(egui::Slider::new(&mut app.settings.studio.fps, 1..=120).text("fps"));
            });
            let (w, h) = (app.settings.studio.width, app.settings.studio.height);
            let mut size = None;
            look::small_menu(ui, ph::FRAME_CORNERS, &format!("{w} x {h}"), t("The size of the video"), |ui| {
                for (a, b) in [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160), (1080, 1920), (1080, 1080)] {
                    if ui.radio((w, h) == (a, b), format!("{a} x {b}")).clicked() {
                        size = Some((a, b));
                    }
                }
            });
            if let Some((a, b)) = size
                && !recording
            {
                app.settings.studio.width = a;
                app.settings.studio.height = b;
                // A new canvas size needs a new engine.
                app.x.studio = Some(StudioUi::start(&app.settings, ctx));
            }
        });
    });
    look::group(ui, t("Sound"), |ui| {
        look::column(ui, |ui| {
            let mut add = None;
            look::small_menu(ui, ph::PLUS, t("Add sound"), t("Add the computer sound, a microphone or a sound file to this scene (up to 6)"), |ui| add = add_sound_menu(app, ui));
            if let Some(s) = add {
                add_sound(app, s);
            }
            if look::small_tool(ui, ph::FOLDER_OPEN, t("Recordings folder"), t("Open the folder your videos are saved in"), true).clicked() {
                let f = PathBuf::from(&app.settings.video_folder);
                let _ = std::fs::create_dir_all(&f);
                app.open_folder(&f);
            }
        });
    });
}

/// Studio > Ink: draw on the screen (and the preview), the pen, how long ink stays, Clear.
fn ink_group(app: &mut SnagApp, ui: &mut egui::Ui) {
    let now = Instant::now();
    let (on, any) = crate::ink::with(|i| (i.on, i.any(now)));
    look::group(ui, t("Ink"), |ui| {
        let tip = match crate::ink_overlay::support() {
            crate::ink_overlay::Where::Screen => t("Draw on the screen and the preview; it shows in the stream and the video (Esc to stop)"),
            crate::ink_overlay::Where::PreviewOnly => t("Draw on the preview. This desktop doesn't let apps draw over the screen."),
        };
        let tip = format!("{tip}{}", super::ribbon::kbd(&app.settings, Action::DrawInk));
        if look::big_tool(ui, ph::PEN_NIB, t("Draw"), &tip, true, on).clicked() {
            toggle_ink(app);
        }
        let tip = format!("{}{}", t("Remove all ink"), super::ribbon::kbd(&app.settings, Action::ClearInk));
        if look::big_tool(ui, ph::BROOM, t("Clear"), &tip, any, false).clicked() {
            clear_ink();
        }
        look::column(ui, |ui| {
            let st = &mut app.settings.studio;
            look::small_menu(ui, ph::PALETTE, t("Color"), t("Ink color"), |ui| {
                ui.horizontal(|ui| {
                    for c in crate::ink::COLORS {
                        let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                        let col = Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
                        ui.painter().circle_filled(r.center(), 9.0, col);
                        let edge = if st.ink_color == [c[0], c[1], c[2]] { Stroke::new(2.0, accent()) } else { Stroke::new(1.0, pal().border) };
                        ui.painter().circle_stroke(r.center(), 10.0, edge);
                        if resp.clicked() {
                            st.ink_color = [c[0], c[1], c[2]];
                            ui.close();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    egui::color_picker::color_edit_button_srgb(ui, &mut st.ink_color);
                    ui.label(t("More colors…"));
                });
            });
            let width = st.ink_width;
            look::small_menu(ui, ph::CIRCLE, &tf("Pen {width} px", &[("width", &format!("{width:.0}"))]), t("Pen width"), |ui| {
                for (w, name) in crate::ink::WIDTHS {
                    if ui.radio((st.ink_width - w).abs() < 0.01, tf("{name} ({width} px)", &[("name", t(name)), ("width", &format!("{w:.0}"))])).clicked() {
                        st.ink_width = w;
                    }
                }
                ui.add(egui::Slider::new(&mut st.ink_width, 1.0..=64.0).text("px"));
            });
            let label = ink_hold_label(st.ink_mode, st.ink_fade_s);
            look::small_menu(ui, ph::TIMER, &label, t("How long ink stays"), |ui| {
                for s in crate::ink::HOLDS {
                    let picked = st.ink_mode == crate::ink::InkMode::Fade && (st.ink_fade_s - s).abs() < 0.01;
                    if ui.radio(picked, tf("Fades after {seconds} s", &[("seconds", &format!("{s:.0}"))])).clicked() {
                        st.ink_mode = crate::ink::InkMode::Fade;
                        st.ink_fade_s = s;
                    }
                }
                ui.separator();
                if ui.radio(st.ink_mode == crate::ink::InkMode::Stay, t("Stay until cleared")).clicked() {
                    st.ink_mode = crate::ink::InkMode::Stay;
                }
            });
        });
    });
}

/// The Ink group's hold menu label: "Fades after 3 s" or "Stays".
pub(crate) fn ink_hold_label(mode: crate::ink::InkMode, secs: f32) -> String {
    match mode {
        crate::ink::InkMode::Fade => tf("Fades after {seconds} s", &[("seconds", &format!("{:.0}", secs.max(0.5)))]),
        crate::ink::InkMode::Stay => t("Stays").to_string(),
    }
}

/// Draw on or off (the Draw button, its hotkey and the window command). Studio has to be open:
/// the ink goes into its scene.
pub(crate) fn toggle_ink(app: &mut SnagApp) {
    if app.x.studio.is_none() {
        app.status = t("Open the Studio tab to draw on the screen.").into();
        return;
    }
    let now = Instant::now();
    let on = crate::ink::with(|i| {
        let on = !i.on;
        i.set_on(on, now);
        on
    });
    if on {
        crate::ink_overlay::run();
        if crate::ink_overlay::support() == crate::ink_overlay::Where::PreviewOnly
            && let Some(st) = &mut app.x.studio
            && !st.told_preview_only
        {
            st.told_preview_only = true;
            app.status = t("On this desktop you draw on the preview.").into();
        }
    }
}

/// Remove all ink (Clear, its hotkey, the window command and the MCP server's ink_clear).
pub(crate) fn clear_ink() {
    crate::ink::with(|i| i.clear());
}

fn add_source_menu(app: &mut SnagApp, ui: &mut egui::Ui) -> Option<Item> {
    let (cw, ch) = (app.settings.studio.width as f32, app.settings.studio.height as f32);
    let full = |name: String, kind: SourceKind| Item { name, kind, x: 0.0, y: 0.0, w: cw, h: ch, visible: true };
    let mut out = None;
    // The lists are made on a worker and kept a few seconds, not made again every frame
    // while the menu is open.
    ui.menu_button(format!("{}  {}", ph::MONITOR, t("Screen")), |ui| {
        let ctx = ui.ctx().clone();
        app.mon_cache.get(Some(&ctx), capture::monitors);
        if !app.mon_cache.loaded() {
            ui.spinner();
        }
        for (i, m) in app.mon_cache.items().iter().enumerate() {
            if ui.button(format!("{}: {}  {} x {}", i + 1, m.name, m.width, m.height)).clicked() {
                out = Some(full(tf("Screen {n}", &[("n", &(i + 1).to_string())]), SourceKind::Display { index: i }));
                ui.close();
            }
        }
    });
    ui.menu_button(format!("{}  {}", ph::APP_WINDOW, t("Window")), |ui| {
        let ctx = ui.ctx().clone();
        app.win_cache.get(Some(&ctx), capture::list_windows);
        if !app.win_cache.loaded() {
            ui.spinner();
        }
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            for w in app.win_cache.items() {
                if ui.button(&w.label).clicked() {
                    let aspect = w.width as f32 / w.height.max(1) as f32;
                    let (iw, ih) = if aspect > cw / ch { (cw, cw / aspect) } else { (ch * aspect, ch) };
                    out = Some(Item { name: w.label.chars().take(40).collect(), kind: SourceKind::Window { title: w.label.clone() }, x: (cw - iw) / 2.0, y: (ch - ih) / 2.0, w: iw, h: ih, visible: true });
                    ui.close();
                }
            }
        });
    });
    if ui.button(format!("{}  {}", ph::SELECTION, t("Screen area"))).on_hover_text(t("A rectangle of the desktop (set it on the right)")).clicked() {
        let (x, y, w, h) = app.last_region.unwrap_or((0, 0, 1280, 720));
        out = Some(Item { name: t("Screen area").into(), kind: SourceKind::Area { x, y, w, h }, x: 0.0, y: 0.0, w: w as f32, h: h as f32, visible: true });
        ui.close();
    }
    if ui.button(format!("{}  {}", ph::IMAGE, t("Picture…"))).clicked() {
        ui.close();
        if let Some(p) = rfd::FileDialog::new().add_filter(t("Pictures"), &["png", "jpg", "jpeg", "bmp", "gif", "webp"]).pick_file() {
            let (w, h) = image::image_dimensions(&p).unwrap_or((400, 300));
            out = Some(Item {
                name: p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                kind: SourceKind::Image { path: p.to_string_lossy().to_string() },
                x: 40.0,
                y: 40.0,
                w: (w as f32).min(cw),
                h: (h as f32).min(ch),
                visible: true,
            });
        }
    }
    if ui.button(format!("{}  {}", ph::TEXT_T, t("Text"))).clicked() {
        out = Some(Item { name: t("Text").into(), kind: SourceKind::Text { text: t("Your text").into(), size: 64.0, color: [255, 255, 255, 255] }, x: 60.0, y: ch - 160.0, w: cw - 120.0, h: 120.0, visible: true });
        ui.close();
    }
    if ui.button(format!("{}  {}", ph::PAINT_BUCKET, t("Color"))).clicked() {
        out = Some(full(t("Color").into(), SourceKind::Color { color: [24, 26, 34, 255] }));
        ui.close();
    }
    out
}

/// New scene: an empty "Scene N" (translated), made the current one.
pub(crate) fn new_scene(st: &mut crate::studio::StudioSettings) {
    let n = st.scenes.len() + 1;
    let i = st.add_scene(Some(&tf("Scene {n}", &[("n", &n.to_string())])));
    let _ = st.set_current(i);
}

/// Copy scene: "<name> copy" with the same sources, made the current one.
pub(crate) fn copy_scene(st: &mut crate::studio::StudioSettings) {
    let Some(s) = st.scene().cloned() else { return };
    let i = st.add_scene(Some(&tf("{name} copy", &[("name", &s.name)])));
    st.scenes[i].items = s.items;
    let _ = st.set_current(i);
}

/// Delete scene: removes the current one (never the last); the one before becomes current.
pub(crate) fn delete_scene(st: &mut crate::studio::StudioSettings) {
    let c = st.current.min(st.scenes.len().saturating_sub(1));
    let _ = st.remove_scene(c);
}

fn add_item(app: &mut SnagApp, item: Item) {
    let c = app.settings.studio.current.min(app.settings.studio.scenes.len().saturating_sub(1));
    if let Some(scene) = app.settings.studio.scenes.get_mut(c) {
        scene.items.push(item);
        let n = scene.items.len() - 1;
        if let Some(s) = &mut app.x.studio {
            s.selected = Some(n);
        }
    }
}

pub(crate) fn start(app: &mut SnagApp, ctx: &egui::Context) {
    let Some(st) = &mut app.x.studio else { return };
    // The same options the MCP server's studio_record uses.
    let mut opts = crate::studio::video_opts(&app.settings);
    let (fps, path) = (opts.fps, opts.path.clone());
    opts.frames = Some(st.engine.feed());
    if let Ok(mut f) = st.engine.shared.recording_fps.lock() {
        *f = Some(fps);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let pause = Arc::new(AtomicBool::new(false));
    let (tx, rx) = channel();
    crate::video::spawn(opts, stop.clone(), pause.clone(), Some(ctx.clone()), tx);
    st.rec = Some(Rec { stop, pause, rx, started: Instant::now(), paused_for: Duration::ZERO, paused_at: None, path });
    app.status = tf("Studio is recording at {fps} fps.", &[("fps", &fps.to_string())]);
}

/// Studio is open and recording.
pub(crate) fn recording(app: &SnagApp) -> bool {
    app.x.studio.as_ref().is_some_and(|s| s.rec.is_some())
}

pub(crate) fn stop(app: &mut SnagApp) {
    if let Some(r) = app.x.studio.as_ref().and_then(|s| s.rec.as_ref()) {
        r.stop.store(true, Ordering::Relaxed);
        r.pause.store(false, Ordering::Relaxed);
    }
}

pub(crate) fn toggle_pause(app: &mut SnagApp) {
    if let Some(r) = app.x.studio.as_mut().and_then(|s| s.rec.as_mut()) {
        let now = !r.pause.load(Ordering::Relaxed);
        r.pause.store(now, Ordering::Relaxed);
        if now {
            r.paused_at = Some(Instant::now());
        } else if let Some(p) = r.paused_at.take() {
            r.paused_for += p.elapsed();
        }
    }
}

impl SnagApp {
    /// Every frame while Studio runs (its tab, or a stream of it from the Stream tab): the
    /// engine draws the current scene as it is now, the mixer follows the current scene, and
    /// the meters listen to every input the scenes use.
    pub(crate) fn studio_tick(&mut self, ctx: &egui::Context) {
        self.settings.studio.apply_mix();
        if self.x.studio.is_some() {
            let now = Instant::now();
            let pen = self.settings.studio.ink_pen();
            let (on, next) = crate::ink::with(|i| {
                i.set_pen(pen, now);
                (i.on, i.next_change(now))
            });
            // The ribbon follows Draw turned off with Esc, and Clear greys out once ink fades.
            if on {
                ctx.request_repaint_after(Duration::from_millis(100));
            } else if let Some(at) = next {
                ctx.request_repaint_after(at.saturating_duration_since(now) + crate::ink::FADE_OUT);
            }
            super::ink_ui::overlays(ctx);
        }
        if let Some(st) = &mut self.x.studio {
            if let Some(scene) = self.settings.studio.scene()
                && let Ok(mut sh) = st.engine.shared.scene.lock()
                && *sh != *scene
            {
                *sh = scene.clone();
            }
            let want = self.settings.studio.sound_inputs();
            if st.meters.is_none() || st.meters_for != want {
                // On the window's thread: never longer than a few frames (the meters only show
                // levels; a stream of Studio starts its own sound and waits properly).
                st.meters = Some(crate::audio::Capture::start_inputs_within(&want, std::time::Duration::from_millis(60)));
                st.meters_for = want;
            } else if let Some(m) = &mut st.meters {
                // A sound that failed after that short wait still gets its note.
                m.drop_failed();
            }
        }
    }

    /// Loudness of `input` in Studio's meters (None: not running).
    pub(crate) fn studio_level(&self, input: &crate::audio::SoundInput) -> Option<f32> {
        self.x.studio.as_ref().and_then(|s| s.meters.as_ref()).and_then(|m| m.level_of(input))
    }

    /// Studio mode's window body (below the ribbon).
    pub(crate) fn studio_view(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.x.studio.is_none() {
            self.x.studio = Some(StudioUi::start(&self.settings, &ctx));
        }
        // Finished recordings.
        let mut finished = None;
        if let Some(st) = &mut self.x.studio
            && let Some(r) = &st.rec
            && let Ok(done) = r.rx.try_recv()
        {
            finished = Some(done);
            st.rec = None;
            if let Ok(mut f) = st.engine.shared.recording_fps.lock() {
                *f = None;
            }
        }
        if let Some(done) = finished {
            match &done.result {
                Ok(()) => {
                    self.status = tf("Saved {file}: {secs} s, {w} x {h}, {enc}.", &[
                        ("file", &done.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
                        ("secs", &format!("{:.1}", done.seconds)),
                        ("w", &done.width.to_string()),
                        ("h", &done.height.to_string()),
                        ("enc", &done.encoder),
                    ]);
                    if !done.note.is_empty() {
                        self.status = format!("{} {}.", self.status, done.note);
                    }
                    self.videos.insert(0, done.clone());
                }
                Err(e) => self.status = tf("Studio recording failed: {e}", &[("e", e)]),
            }
            if let Some(st) = &mut self.x.studio {
                st.last = Some(done);
            }
        }
        self.studio_tick(&ctx);
        egui::Panel::left("studio-left")
            .exact_size(250.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(pal().panel).inner_margin(egui::Margin::symmetric(10, 8)))
            .show(ui, |ui| self.studio_left(ui));
        egui::Panel::right("studio-right")
            .exact_size(270.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(pal().panel).inner_margin(egui::Margin::symmetric(10, 8)))
            .show(ui, |ui| self.studio_right(ui));
        egui::Panel::bottom("studio-status")
            .exact_size(30.0)
            .frame(egui::Frame::new().fill(look::pal().tabs).inner_margin(egui::Margin::symmetric(10, 4)))
            .show(ui, |ui| self.studio_status(ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal().bg)).show(ui, |ui| self.studio_preview(ui));
    }

    fn studio_left(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new(t("Scenes")).strong().color(pal().strong));
        let mut switch = None;
        let recording = self.x.studio.as_ref().is_some_and(|s| s.recording());
        egui::ScrollArea::vertical().id_salt("scenes").max_height(170.0).show(ui, |ui| {
            for (i, sc) in self.settings.studio.scenes.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    let on = i == self.settings.studio.current;
                    if ui.selectable_label(on, format!("{}  ", look::ph::FILM_SLATE)).on_hover_text(t("Show this scene (also while recording)")).clicked() {
                        switch = Some(i);
                    }
                    ui.add(egui::TextEdit::singleline(&mut sc.name).desired_width(170.0)).on_hover_text(t("The scene's name"));
                });
            }
        });
        if let Some(i) = switch
            && self.settings.studio.set_current(i).is_ok()
        {
            if let Some(s) = &mut self.x.studio {
                s.selected = None;
            }
        }
        if recording {
            ui.label(RichText::new(t("Switching scenes while recording cuts to the new scene.")).small().color(pal().dim));
        }
        ui.add_space(8.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(RichText::new(t("Sources")).strong().color(pal().strong));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut add = None;
                let r = look::icon_button(ui, ph::PLUS, t("Add source"));
                egui::Popup::menu(&r).show(|ui| add = add_source_menu(self, ui));
                if let Some(item) = add {
                    add_item(self, item);
                }
            });
        });
        let c = self.settings.studio.current.min(self.settings.studio.scenes.len().saturating_sub(1));
        let selected = self.x.studio.as_ref().and_then(|s| s.selected);
        let mut select = None;
        let mut action: Option<(usize, i32)> = None; // (index, -1 up / 1 down / 0 delete)
        if let Some(scene) = self.settings.studio.scenes.get_mut(c) {
            if scene.items.is_empty() {
                ui.label(RichText::new(t("No sources yet: add a screen, a window or a picture.")).color(pal().dim));
            }
            // Top of the list is drawn on top: show the list in reverse.
            let n = scene.items.len();
            egui::ScrollArea::vertical().id_salt("sources").show(ui, |ui| {
                for i in (0..n).rev() {
                    let it = &mut scene.items[i];
                    ui.horizontal(|ui| {
                        let eye = if it.visible { ph::CHECK } else { ph::EYE_SLASH };
                        if look::icon_button(ui, eye, if it.visible { t("Shown; click to hide") } else { t("Hidden; click to show") }).clicked() {
                            it.visible = !it.visible;
                        }
                        let label = format!("{}  {}", it.kind.icon(), it.name);
                        if ui.selectable_label(selected == Some(i), label).clicked() {
                            select = Some(i);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if look::icon_button(ui, ph::TRASH, t("Remove this source")).clicked() {
                                action = Some((i, 0));
                            }
                            if look::icon_button(ui, ph::CARET_DOWN, t("Move down (behind)")).clicked() {
                                action = Some((i, -1));
                            }
                            if look::icon_button(ui, ph::CARET_UP, t("Move up (in front)")).clicked() {
                                action = Some((i, 1));
                            }
                        });
                    });
                }
            });
            if let Some((i, d)) = action {
                match d {
                    0 => {
                        scene.items.remove(i);
                        select = None;
                    }
                    d => {
                        move_item(&mut scene.items, i, d);
                    }
                }
            }
        }
        if let Some(st) = &mut self.x.studio
            && (select.is_some() || action.is_some_and(|a| a.1 == 0))
        {
            st.selected = select;
        }
    }

    fn studio_right(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(RichText::new(t("Source")).strong().color(pal().strong));
            let c = self.settings.studio.current.min(self.settings.studio.scenes.len().saturating_sub(1));
            let sel = self.x.studio.as_ref().and_then(|s| s.selected);
            let (cw, ch) = (self.settings.studio.width as f32, self.settings.studio.height as f32);
            let monitors = self.x.studio.as_ref().map(|s| s.monitors.clone()).unwrap_or_default();
            let mut refresh_windows = false;
            let mut picker_open = false;
            let windows_loaded = self.win_cache.loaded();
            let windows = self.x.studio.as_ref().map(|s| s.windows.clone()).unwrap_or_default();
            match sel.and_then(|i| self.settings.studio.scenes.get_mut(c).and_then(|s| s.items.get_mut(i))) {
                None => {
                    ui.label(RichText::new(t("Click a source in the list or the preview to change it.")).color(pal().dim));
                }
                Some(it) => {
                    ui.horizontal(|ui| {
                        ui.label(t("Name"));
                        ui.text_edit_singleline(&mut it.name);
                    });
                    ui.checkbox(&mut it.visible, t("Shown"));
                    egui::Grid::new("src-pos").num_columns(4).show(ui, |ui| {
                        ui.label("X");
                        ui.add(egui::DragValue::new(&mut it.x).speed(2.0));
                        ui.label("Y");
                        ui.add(egui::DragValue::new(&mut it.y).speed(2.0));
                        ui.end_row();
                        ui.label(t("Width"));
                        ui.add(egui::DragValue::new(&mut it.w).speed(2.0).range(1.0..=16000.0));
                        ui.label(t("Height"));
                        ui.add(egui::DragValue::new(&mut it.h).speed(2.0).range(1.0..=16000.0));
                        ui.end_row();
                    });
                    ui.horizontal_wrapped(|ui| {
                        if ui.button(t("Fill the video")).on_hover_text(t("Stretch to the whole video")).clicked() {
                            (it.x, it.y, it.w, it.h) = (0.0, 0.0, cw, ch);
                        }
                        if ui.button(t("Fit, keep shape")).on_hover_text(t("As big as fits, without stretching")).clicked() {
                            let aspect = (it.w / it.h.max(1.0)).max(0.01);
                            let (w, h) = if aspect > cw / ch { (cw, cw / aspect) } else { (ch * aspect, ch) };
                            (it.x, it.y, it.w, it.h) = ((cw - w) / 2.0, (ch - h) / 2.0, w, h);
                        }
                        if ui.button(t("Center")).clicked() {
                            it.x = (cw - it.w) / 2.0;
                            it.y = (ch - it.h) / 2.0;
                        }
                    });
                    ui.add_space(6.0);
                    match &mut it.kind {
                        SourceKind::Display { index } => {
                            egui::ComboBox::from_label(t("Screen")).selected_text(monitors.get(*index).map(|m| m.name.clone()).unwrap_or_else(|| format!("{}", *index + 1))).show_ui(ui, |ui| {
                                for (i, m) in monitors.iter().enumerate() {
                                    ui.selectable_value(index, i, format!("{}: {} {} x {}", i + 1, m.name, m.width, m.height));
                                }
                            });
                        }
                        SourceKind::Window { title } => {
                            ui.label(t("Window title contains"));
                            ui.text_edit_singleline(title);
                            let r = ui.button(t("Pick a window…"));
                            if r.clicked() {
                                refresh_windows = true;
                            }
                            egui::Popup::menu(&r).show(|ui| {
                                picker_open = true;
                                if !windows_loaded {
                                    ui.spinner();
                                }
                                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                                    for w in &windows {
                                        if ui.button(&w.label).clicked() {
                                            *title = w.label.clone();
                                            ui.close();
                                        }
                                    }
                                });
                            });
                        }
                        SourceKind::Area { x, y, w, h } => {
                            ui.label(t("Desktop area"));
                            egui::Grid::new("area").num_columns(4).show(ui, |ui| {
                                ui.label("X");
                                ui.add(egui::DragValue::new(x));
                                ui.label("Y");
                                ui.add(egui::DragValue::new(y));
                                ui.end_row();
                                ui.label(t("Width"));
                                ui.add(egui::DragValue::new(w).range(16..=16000));
                                ui.label(t("Height"));
                                ui.add(egui::DragValue::new(h).range(16..=16000));
                                ui.end_row();
                            });
                        }
                        SourceKind::Image { path } => {
                            let shown = std::path::Path::new(path.as_str()).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                            ui.label(RichText::new(shown).small().color(pal().dim)).on_hover_text(path.as_str());
                            if ui.button(t("Choose another picture…")).clicked()
                                && let Some(p) = rfd::FileDialog::new().add_filter(t("Pictures"), &["png", "jpg", "jpeg", "bmp", "gif", "webp"]).pick_file()
                            {
                                *path = p.to_string_lossy().to_string();
                            }
                        }
                        SourceKind::Text { text, size, color } => {
                            ui.add(egui::TextEdit::multiline(text).desired_rows(2));
                            ui.add(egui::Slider::new(size, 8.0..=300.0).text(t("Size")));
                            let mut c = Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]);
                            if ui.color_edit_button_srgba(&mut c).changed() {
                                *color = c.to_srgba_unmultiplied();
                            }
                        }
                        SourceKind::Color { color } => {
                            let mut c = Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]);
                            if ui.color_edit_button_srgba(&mut c).changed() {
                                *color = c.to_srgba_unmultiplied();
                            }
                        }
                    }
                }
            }
            if refresh_windows {
                self.win_cache.expire();
            }
            // While the window picker is open its list is refreshed on a worker.
            if (refresh_windows || picker_open) && let Some(st) = &mut self.x.studio {
                st.windows = self.win_cache.get(Some(ui.ctx()), capture::list_windows).to_vec();
            }
            ui.add_space(10.0);
            ui.separator();
            self.scene_mixer(ui);
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        });
    }

    fn studio_status(&mut self, ui: &mut egui::Ui) {
        let Some(st) = &self.x.studio else { return };
        let rec = st.rec.as_ref().map(|r| (r.started, r.pause.load(Ordering::Relaxed), r.path.clone()));
        let secs = st.elapsed().as_secs();
        let ms = st.engine.shared.draw_ms.load(Ordering::Relaxed);
        let last = st.last.as_ref().filter(|d| d.result.is_ok()).map(|d| d.path.clone());
        let (w, h, fps) = (self.settings.studio.width, self.settings.studio.height, self.settings.studio.fps);
        let mut play = None;
        ui.horizontal_centered(|ui| {
            if let Some((started, paused, path)) = &rec {
                let blink = (started.elapsed().as_millis() / 500) % 2 == 0;
                ui.label(RichText::new(if *paused { "⏸" } else if blink { "●" } else { "○" }).color(look::REC));
                ui.label(RichText::new(format!("{} {}:{:02}:{:02}", if *paused { t("Paused") } else { t("Recording") }, secs / 3600, secs / 60 % 60, secs % 60)).strong().color(pal().strong));
                let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                ui.label(RichText::new(format!("{:.1} MB", size as f64 / 1_048_576.0)).color(pal().dim));
                ui.ctx().request_repaint_after(Duration::from_millis(250));
            } else {
                ui.label(RichText::new(t("Ready")).color(pal().dim));
            }
            ui.separator();
            ui.label(RichText::new(tf("{w} x {h}, {fps} fps; a frame takes {ms} ms to draw", &[("w", &w.to_string()), ("h", &h.to_string()), ("fps", &fps.to_string()), ("ms", &ms.to_string())])).color(pal().dim));
            if let Some(p) = &last {
                ui.separator();
                if ui.link(tf("Last: {file}", &[("file", &p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())])).on_hover_text(t("Play it")).clicked() {
                    play = Some(p.clone());
                }
            }
        });
        if let Some(p) = play {
            self.open_file(&p);
        }
    }

    fn studio_preview(&mut self, ui: &mut egui::Ui) {
        let (cw, ch) = (self.settings.studio.width as f32, self.settings.studio.height as f32);
        let avail = ui.available_rect_before_wrap().shrink(16.0);
        let scale = (avail.width() / cw).min(avail.height() / ch).max(0.01);
        let rect = Rect::from_center_size(avail.center(), vec2(cw * scale, ch * scale));
        let Some(st) = &mut self.x.studio else { return };
        let no = st.engine.shared.frame_no.load(Ordering::Relaxed);
        if no != st.tex_frame
            && let Some(frame) = st.engine.latest()
        {
            st.tex_frame = no;
            let (w, h) = frame.dimensions();
            // The preview never needs more pixels than it shows.
            let max_w = (rect.width() * ui.ctx().pixels_per_point()).ceil() as u32;
            let img = if w > max_w.max(320) * 5 / 4 {
                let nh = (h as f32 * max_w as f32 / w as f32) as u32;
                let mut small = RgbaImage::new(max_w.max(1), nh.max(1));
                crate::studio::blit_scaled(&mut small, &frame, 0, 0, max_w.max(1), nh.max(1), false);
                small
            } else {
                (*frame).clone()
            };
            let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
            match &mut st.tex {
                Some(t) => t.set(ci, egui::TextureOptions::LINEAR),
                None => st.tex = Some(ui.ctx().load_texture("studio-preview", ci, egui::TextureOptions::LINEAR)),
            }
        }
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        // Draw is on: the preview is paper, not sources to move.
        if crate::ink::with(|i| i.on) {
            let placed = st.engine.shared.placed.lock().map(|p| p.clone()).unwrap_or_default();
            self.ink_on_preview(ui, &resp, rect, scale, &placed);
            return;
        }
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::ZERO, Color32::BLACK);
        if let Some(tex) = &st.tex {
            p.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        p.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, pal().border), egui::StrokeKind::Outside);
        if st.recording() {
            p.circle_filled(rect.left_top() + vec2(18.0, 18.0), 7.0, look::REC);
        }
        // Sources: outlines, selection, moving and resizing.
        let c = self.settings.studio.current.min(self.settings.studio.scenes.len().saturating_sub(1));
        let Some(scene) = self.settings.studio.scenes.get_mut(c) else { return };
        let to_screen = |x: f32, y: f32| pos2(rect.left() + x * scale, rect.top() + y * scale);
        let item_rect = |it: &Item| Rect::from_min_max(to_screen(it.x, it.y), to_screen(it.x + it.w, it.y + it.h));
        if let Some(i) = st.selected.filter(|i| *i < scene.items.len()) {
            let r = item_rect(&scene.items[i]);
            p.rect_stroke(r, CornerRadius::ZERO, Stroke::new(2.0, accent()), egui::StrokeKind::Outside);
            p.rect_filled(Rect::from_center_size(r.right_bottom(), vec2(10.0, 10.0)), CornerRadius::same(2), accent());
        }
        if resp.drag_started()
            && let Some(at) = resp.interact_pointer_pos()
        {
            let sel = st.selected.filter(|i| *i < scene.items.len());
            let on_handle = sel.is_some_and(|i| Rect::from_center_size(item_rect(&scene.items[i]).right_bottom(), vec2(16.0, 16.0)).contains(at));
            if on_handle {
                let i = sel.unwrap_or(0);
                let it = &scene.items[i];
                st.grab = Some((i, Grab::Resize, at, (it.x, it.y, it.w, it.h)));
            } else if let Some(i) = (0..scene.items.len()).rev().find(|i| scene.items[*i].visible && item_rect(&scene.items[*i]).contains(at)) {
                st.selected = Some(i);
                let it = &scene.items[i];
                st.grab = Some((i, Grab::Move, at, (it.x, it.y, it.w, it.h)));
            }
        }
        if let Some((i, g, start, (x, y, w, h))) = st.grab
            && let Some(now) = resp.interact_pointer_pos()
            && let Some(it) = scene.items.get_mut(i)
        {
            let d = (now - start) / scale;
            let shift = ui.input(|inp| inp.modifiers.shift);
            match g {
                Grab::Move => {
                    it.x = snap_edge(x + d.x, w, cw);
                    it.y = snap_edge(y + d.y, h, ch);
                }
                Grab::Resize => (it.w, it.h) = resized(w, h, d, shift),
            }
        }
        if resp.drag_stopped() {
            st.grab = None;
        }
        if resp.clicked()
            && let Some(at) = resp.interact_pointer_pos()
        {
            st.selected = (0..scene.items.len()).rev().find(|i| scene.items[*i].visible && item_rect(&scene.items[*i]).contains(at));
        }
        let hint = if st.grab.is_some() { t("Shift: resize freely") } else { t("Drag a source to move it, its corner to resize it") };
        ui.painter().text(pos2(rect.left(), rect.bottom() + 4.0), Align2::LEFT_TOP, hint, FontId::proportional(11.5), pal().dim);
    }

    /// Drawing on the Studio preview. Over a screen, window or area source the ink is kept in
    /// desktop pixels, so it also shows on the screen and on every source showing that place;
    /// elsewhere on the canvas.
    fn ink_on_preview(&mut self, ui: &mut egui::Ui, resp: &egui::Response, rect: Rect, scale: f32, placed: &[crate::ink::Placed]) {
        let pen = self.settings.studio.ink_pen();
        let Some(st) = &mut self.x.studio else { return };
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::ZERO, Color32::BLACK);
        if let Some(tex) = &st.tex {
            p.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        p.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(2.0, accent()), egui::StrokeKind::Outside);
        let now = Instant::now();
        let to_canvas = |at: Pos2| [(at.x - rect.left()) / scale, (at.y - rect.top()) / scale];
        if resp.drag_started()
            && let Some(at) = resp.interact_pointer_pos()
        {
            let c = to_canvas(at);
            let (space, pt) = preview_ink_start(placed, c);
            crate::ink::with(|i| i.begin(space, pt, now));
            st.ink_from = placed.iter().rev().find(|q| q.contains_canvas(c)).copied();
        } else if resp.clicked()
            && let Some(at) = resp.interact_pointer_pos()
        {
            let (space, pt) = preview_ink_start(placed, to_canvas(at));
            crate::ink::with(|i| {
                i.begin(space, pt, now);
                i.end(now);
            });
        }
        if resp.dragged()
            && let Some(at) = resp.interact_pointer_pos()
        {
            let c = to_canvas(at);
            let pt = st.ink_from.map(|q| q.to_desk(c)).unwrap_or(c);
            crate::ink::with(|i| i.extend(pt));
        }
        if resp.drag_stopped() {
            crate::ink::with(|i| i.end(now));
            st.ink_from = None;
        }
        // The pointer is a dot of the pen's colour and width.
        if let Some(at) = resp.hover_pos() {
            let c = Color32::from_rgba_unmultiplied(pen.color[0], pen.color[1], pen.color[2], pen.color[3]);
            ui.painter().circle_filled(at, (pen.width * scale / 2.0).max(2.0), c);
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        }
        // Esc stops drawing, Ctrl+Z takes back the last stroke.
        let (esc, undo) = ui.input(|i| (i.key_pressed(egui::Key::Escape), i.modifiers.command && i.key_pressed(egui::Key::Z)));
        if esc {
            crate::ink::with(|i| i.set_on(false, now));
        }
        if undo {
            crate::ink::with(|i| i.undo(now));
        }
        ui.painter().text(pos2(rect.left(), rect.bottom() + 4.0), Align2::LEFT_TOP, t(crate::ink_overlay::HINT), FontId::proportional(11.5), pal().dim);
    }
}

/// Where ink drawn on the preview at canvas point `c` is kept: on the desktop when a screen,
/// window or area source is there (the topmost), else on the canvas.
pub(crate) fn preview_ink_start(placed: &[crate::ink::Placed], c: [f32; 2]) -> (crate::ink::Space, [f32; 2]) {
    match placed.iter().rev().find(|q| q.contains_canvas(c)) {
        Some(q) => (crate::ink::Space::Desktop, q.to_desk(c)),
        None => (crate::ink::Space::Canvas, c),
    }
}

/// What a mixer row asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RowAsk {
    Nothing,
    Remove,
    /// Move between "every scene" and "this scene only".
    Share,
}

/// An input's icon (muted or not).
pub(crate) fn sound_icon(input: &crate::audio::SoundInput, muted: bool) -> &'static str {
    use crate::audio::SoundInput::*;
    match (input, muted) {
        (Microphone { .. }, false) => ph::MICROPHONE,
        (Microphone { .. }, true) => ph::MICROPHONE_SLASH,
        (File { .. }, false) => ph::WAVEFORM,
        (_, true) => ph::SPEAKER_SLASH,
        (Computer, false) => ph::SPEAKER,
    }
}

/// One sound in the mixer: mute, name, "every scene" and remove, then the meter and volume.
fn sound_row(ui: &mut egui::Ui, s: &mut crate::studio::SceneSound, shared: bool, level: Option<f32>) -> RowAsk {
    let mut ask = RowAsk::Nothing;
    ui.horizontal(|ui| {
        if look::icon_button(ui, sound_icon(&s.input, s.muted), if s.muted { t("Muted; click to turn on") } else { t("Click to mute") }).clicked() {
            s.muted = !s.muted;
        }
        let name = if s.name.trim().is_empty() { t("Sound").to_string() } else { crate::i18n::tr(&s.name) };
        ui.label(name);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if look::icon_button(ui, ph::TRASH, t("Remove this sound")).clicked() {
                ask = RowAsk::Remove;
            }
            let pin = egui::Button::new(RichText::new(ph::PUSH_PIN).family(look::icons())).selected(shared).frame(shared);
            if ui.add(pin).on_hover_text(if shared { t("In every scene; click to keep it in this scene only") } else { t("In this scene only; click to put it in every scene") }).clicked() {
                ask = RowAsk::Share;
            }
        });
    });
    // The meter: green, then yellow, then red near the top; a dash when the input isn't running.
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::same(3), pal().panel_hi);
    match level {
        Some(level) if !s.muted => {
            let f = meter_fill(level, s.volume);
            ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(r.width() * f, r.height())), CornerRadius::same(3), meter_color_on(f, pal().dark));
        }
        None => {
            ui.painter().line_segment([r.left_center(), r.right_center()], Stroke::new(1.0, pal().err));
        }
        _ => {}
    }
    let pct = format!("{:.0}%", s.volume * 100.0);
    ui.add(egui::Slider::new(&mut s.volume, 0.0..=2.0).show_value(false).text(pct));
    ui.add_space(4.0);
    ask
}

/// The sound inputs the "Add sound" menu offers: the computer sound, each microphone, a file.
fn add_sound_menu(app: &mut SnagApp, ui: &mut egui::Ui) -> Option<crate::studio::SceneSound> {
    use crate::audio::SoundInput;
    use crate::studio::SceneSound;
    let mut out = None;
    if !crate::audio::AudioSource::supported() {
        ui.label(RichText::new(t("Computer sound and microphones work on Windows so far; sound files work everywhere.")).color(pal().dim));
    }
    if ui.add_enabled(crate::audio::AudioSource::supported(), egui::Button::new(format!("{}  {}", ph::SPEAKER, t("Computer sound")))).on_hover_text(t("Everything the computer plays")).clicked() {
        out = Some(SceneSound::new(SoundInput::Computer, "Computer sound"));
        ui.close();
    }
    ui.add_enabled_ui(crate::audio::AudioSource::supported(), |ui| {
        ui.menu_button(format!("{}  {}", ph::MICROPHONE, t("Microphone")), |ui| {
            if ui.button(t("The computer's default microphone")).clicked() {
                out = Some(SceneSound::new(SoundInput::Microphone { id: String::new() }, "Microphone"));
                ui.close();
            }
            let mics = app.x.mics.get(Some(ui.ctx()), crate::audio::microphones).to_vec();
            for m in mics {
                if ui.button(&m.name).clicked() {
                    out = Some(SceneSound::new(SoundInput::Microphone { id: m.id.clone() }, &m.name));
                    ui.close();
                }
            }
        });
    });
    if ui.button(format!("{}  {}", ph::WAVEFORM, t("Sound file (WAV)…"))).on_hover_text(t("Music or a jingle, played over and over")).clicked() {
        ui.close();
        if let Some(p) = rfd::FileDialog::new().add_filter(t("WAV sound"), &["wav", "wave"]).pick_file() {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            out = Some(SceneSound::new(SoundInput::File { path: p.to_string_lossy().to_string() }, &name));
        }
    }
    out
}

/// Put `s` in the current scene (when there is room and it isn't there yet).
pub(crate) fn add_sound(app: &mut SnagApp, s: crate::studio::SceneSound) {
    let c = app.settings.studio.current.min(app.settings.studio.scenes.len().saturating_sub(1));
    if let Err(why) = add_sound_to(&mut app.settings.studio, c, s) {
        app.status = why;
    }
}

/// Put `s` in scene `c` (Studio's "Add sound" and the MCP server's studio_edit).
pub(crate) fn add_sound_to(st: &mut crate::studio::StudioSettings, c: usize, s: crate::studio::SceneSound) -> Result<(), String> {
    if c >= st.scenes.len() {
        return Err("there's no such scene".into());
    }
    if st.scene_sounds(c).iter().any(|x| x.input == s.input) {
        return Err(t("That sound is in this scene already.").into());
    }
    if !st.room_for_sound(c) {
        return Err(tf("A scene has at most {n} sounds.", &[("n", &crate::studio::MAX_SOUNDS.to_string())]));
    }
    st.scenes.get_mut(c).ok_or("there's no such scene")?.sounds.push(s);
    Ok(())
}

/// Move source `i` one place: 1 up (in front), -1 down (behind). False when it can't move.
pub(crate) fn move_item<T>(items: &mut [T], i: usize, d: i32) -> bool {
    match d {
        1 if i + 1 < items.len() => items.swap(i, i + 1),
        -1 if i > 0 && i < items.len() => items.swap(i, i - 1),
        _ => return false,
    }
    true
}

impl SnagApp {
    /// The current scene's sound: its shared inputs (in every scene), then its own, with
    /// "Add sound" while there is room.
    fn scene_mixer(&mut self, ui: &mut egui::Ui) {
        let c = self.settings.studio.current.min(self.settings.studio.scenes.len().saturating_sub(1));
        let n = self.settings.studio.scene_sounds(c).len();
        ui.horizontal(|ui| {
            ui.label(RichText::new(t("Sound of this scene")).strong().color(pal().strong));
            ui.label(RichText::new(format!("{n}/{}", crate::studio::MAX_SOUNDS)).color(pal().dim));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let r = look::icon_button(ui, ph::PLUS, t("Add sound"));
                let mut add = None;
                egui::Popup::menu(&r).show(|ui| add = add_sound_menu(self, ui));
                if let Some(s) = add {
                    add_sound(self, s);
                }
            });
        });
        let level = |input: &crate::audio::SoundInput| self.studio_level(input);
        let shared_levels: Vec<Option<f32>> = self.settings.studio.shared_sounds.iter().map(|s| level(&s.input)).collect();
        let own_levels: Vec<Option<f32>> = self.settings.studio.scenes.get(c).map(|sc| sc.sounds.iter().map(|s| level(&s.input)).collect()).unwrap_or_default();
        let mut ask: Option<(bool, usize, RowAsk)> = None;
        let st = &mut self.settings.studio;
        for (i, s) in st.shared_sounds.iter_mut().enumerate() {
            let a = sound_row(ui, s, true, shared_levels.get(i).copied().flatten());
            if a != RowAsk::Nothing {
                ask = Some((true, i, a));
            }
        }
        if let Some(scene) = st.scenes.get_mut(c) {
            for (i, s) in scene.sounds.iter_mut().enumerate() {
                let a = sound_row(ui, s, false, own_levels.get(i).copied().flatten());
                if a != RowAsk::Nothing {
                    ask = Some((false, i, a));
                }
            }
        }
        if let Some((shared, i, a)) = ask
            && let Err(why) = sound_change(st, c, shared, i, a)
        {
            self.status = why;
        }
        if n == 0 {
            ui.label(RichText::new(t("No sound in this scene: add the computer sound, a microphone or a sound file.")).color(pal().dim));
        }
        let notes = self.x.studio.as_ref().and_then(|s| s.meters.as_ref()).map(|m| m.notes().to_vec()).unwrap_or_default();
        for note in notes {
            ui.label(RichText::new(note).small().color(pal().warn));
        }
    }
}

/// Remove sound `i` (of the shared list or scene `c`'s own), or move it between them. A sound
/// put in every scene leaves the scenes' own lists; one kept to this scene only leaves the others.
pub(crate) fn sound_change(st: &mut crate::studio::StudioSettings, c: usize, shared: bool, i: usize, a: RowAsk) -> Result<(), String> {
    // Putting a sound in every scene must leave room in each of them.
    if let (RowAsk::Share, false) = (a, shared)
        && let Some(s) = st.scenes.get(c).and_then(|sc| sc.sounds.get(i))
        && !st.scenes.iter().all(|sc| st.shared_sounds.len() + 1 + sc.sounds.iter().filter(|x| x.input != s.input).count() <= crate::studio::MAX_SOUNDS)
    {
        return Err(tf("Another scene already has {n} sounds: make room there first.", &[("n", &crate::studio::MAX_SOUNDS.to_string())]));
    }
    match (a, shared) {
        (RowAsk::Remove, true) => {
            if i < st.shared_sounds.len() {
                st.shared_sounds.remove(i);
            }
        }
        (RowAsk::Remove, false) => {
            if let Some(scene) = st.scenes.get_mut(c)
                && i < scene.sounds.len()
            {
                scene.sounds.remove(i);
            }
        }
        (RowAsk::Share, true) => {
            if i < st.shared_sounds.len() {
                let s = st.shared_sounds.remove(i);
                if let Some(scene) = st.scenes.get_mut(c) {
                    scene.sounds.push(s);
                }
            }
        }
        (RowAsk::Share, false) => {
            if let Some(scene) = st.scenes.get_mut(c)
                && i < scene.sounds.len()
            {
                let s = scene.sounds.remove(i);
                st.shared_sounds.push(s);
            }
        }
        (RowAsk::Nothing, _) => {}
    }
    st.tidy_sounds();
    Ok(())
}

/// One coordinate of a source being dragged: snapped to the video's near or far edge when
/// within 12 pixels of it.
fn snap_edge(v: f32, len: f32, edge: f32) -> f32 {
    if v.abs() < 12.0 {
        0.0
    } else if (v + len - edge).abs() < 12.0 {
        edge - len
    } else {
        v
    }
}

/// A source's new size when its corner is dragged by `d`: at least 8 pixels wide, keeping its
/// shape unless `free` (Shift) is held.
fn resized(w: f32, h: f32, d: Vec2, free: bool) -> (f32, f32) {
    let nw = (w + d.x).max(8.0);
    let nh = if free { (h + d.y).max(8.0) } else { nw * h / w.max(1.0) };
    (nw, nh)
}

/// How full a mixer meter is (0..=1): the level times the volume on a -60..0 dB scale.
pub(crate) fn meter_fill(level: f32, volume: f32) -> f32 {
    let l = (level * volume).clamp(0.0, 1.0);
    let db = if l > 0.0 { (20.0 * l.log10()).max(-60.0) } else { -60.0 };
    (db + 60.0) / 60.0
}

/// The meter's colour on the palette in use: on a light track the green and yellow are darkened
/// so the bar stands 3:1 against it.
pub(crate) fn meter_color_on(f: f32, dark: bool) -> Color32 {
    let c = meter_color(f);
    if dark { c } else { crate::theme::readable_on(c, crate::theme::LIGHT.panel_hi, 3.0) }
}

/// Green, then yellow, then red near the top.
pub(crate) fn meter_color(f: f32) -> Color32 {
    if f > 0.92 {
        Color32::from_rgb(240, 80, 70)
    } else if f > 0.75 {
        Color32::from_rgb(240, 200, 70)
    } else {
        Color32::from_rgb(80, 200, 120)
    }
}

impl StudioUi {
    /// The engine has drawn a frame (for the button test).
    pub fn preview_ready(&self) -> bool {
        self.engine.latest().is_some()
    }

    /// Seconds since recording started (for the button test).
    pub fn started_secs(&self) -> f32 {
        self.rec.as_ref().map(|r| r.started.elapsed().as_secs_f32()).unwrap_or(0.0)
    }
}

impl StudioUi {
    /// No source selected (after a scene change).
    pub fn select_none(&mut self) {
        self.selected = None;
    }

    pub fn select(&mut self, i: usize) {
        self.selected = Some(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio::Scene;

    #[test]
    fn sources_move_one_place_and_sounds_are_added_once() {
        let mut v = vec![1, 2, 3];
        assert!(move_item(&mut v, 0, 1));
        assert_eq!(v, vec![2, 1, 3]);
        assert!(move_item(&mut v, 2, -1));
        assert_eq!(v, vec![2, 3, 1]);
        assert!(!move_item(&mut v, 2, 1) && !move_item(&mut v, 0, -1) && !move_item(&mut v, 9, -1) && !move_item(&mut v, 1, 0));
        let mut st = crate::studio::StudioSettings { scenes: vec![Scene { sounds: Vec::new(), name: "A".into(), items: Vec::new() }], current: 0, ..Default::default() };
        let wav = || crate::studio::SceneSound::new(crate::audio::SoundInput::File { path: "a.wav".into() }, "a");
        add_sound_to(&mut st, 0, wav()).unwrap();
        assert!(add_sound_to(&mut st, 0, wav()).unwrap_err().contains("already"));
        assert!(add_sound_to(&mut st, 5, wav()).is_err());
    }

    #[test]
    fn scene_buttons_use_the_shared_scene_rules() {
        let mut st = crate::studio::StudioSettings { scenes: vec![Scene { sounds: Vec::new(), name: "A".into(), items: Vec::new() }], current: 0, ..Default::default() };
        new_scene(&mut st);
        assert_eq!((st.scenes.len(), st.current, st.scenes[1].name.as_str()), (2, 1, "Scene 2"));
        st.scenes[1].items.push(Item { name: "x".into(), kind: SourceKind::Color { color: [0, 0, 0, 255] }, x: 0.0, y: 0.0, w: 1.0, h: 1.0, visible: true });
        copy_scene(&mut st);
        assert_eq!((st.scenes.len(), st.current, st.scenes[2].name.as_str(), st.scenes[2].items.len()), (3, 2, "Scene 2 copy", 1));
        // Deleting moves to the scene before, as the button always did.
        delete_scene(&mut st);
        assert_eq!((st.scenes.len(), st.current), (2, 1));
        st.current = 0;
        delete_scene(&mut st);
        assert_eq!((st.scenes.len(), st.current, st.scenes[0].name.as_str()), (1, 0, "Scene 2"));
        // The last scene stays.
        delete_scene(&mut st);
        assert_eq!(st.scenes.len(), 1);
    }

    #[test]
    fn studio_recording_uses_the_same_options_as_the_mcp_server() {
        let mut s = Settings::default();
        s.rec_format = crate::video::VideoFormat::Gif;
        s.studio.fps = 500;
        s.video_quality = 9;
        let o = crate::studio::video_opts(&s);
        assert_eq!(o.format, crate::video::VideoFormat::Mp4, "a GIF setting records MP4, as before");
        assert_eq!((o.fps, o.quality), (120, 4));
        assert_eq!(o.rect, (0, 0, s.studio.width, s.studio.height));
        assert!(o.path.file_name().unwrap().to_string_lossy().starts_with("Studio_"));
    }

    #[test]
    fn dragged_sources_snap_to_the_video_edges() {
        assert_eq!(snap_edge(8.0, 100.0, 1920.0), 0.0);
        assert_eq!(snap_edge(-11.0, 100.0, 1920.0), 0.0);
        assert_eq!(snap_edge(1815.0, 100.0, 1920.0), 1820.0);
        assert_eq!(snap_edge(500.0, 100.0, 1920.0), 500.0);
        assert_eq!(snap_edge(12.0, 100.0, 1920.0), 12.0);
    }

    #[test]
    fn resizing_keeps_the_shape_unless_shift() {
        assert_eq!(resized(160.0, 90.0, vec2(160.0, 0.0), false), (320.0, 180.0));
        assert_eq!(resized(160.0, 90.0, vec2(40.0, 10.0), true), (200.0, 100.0));
        // Never smaller than 8 pixels.
        assert_eq!(resized(160.0, 90.0, vec2(-500.0, -500.0), true), (8.0, 8.0));
        let (w, h) = resized(160.0, 90.0, vec2(-500.0, 0.0), false);
        assert_eq!(w, 8.0);
        assert!((h - 4.5).abs() < 1e-4);
        // A zero-width source doesn't divide by zero.
        assert!(resized(0.0, 90.0, vec2(10.0, 0.0), false).1.is_finite());
    }

    #[test]
    fn meters_use_a_60_db_scale() {
        assert_eq!(meter_fill(0.0, 1.0), 0.0);
        assert_eq!(meter_fill(1.0, 1.0), 1.0);
        assert_eq!(meter_fill(1.0, 2.0), 1.0, "volume can't push it past full");
        assert!((meter_fill(0.1, 1.0) - 40.0 / 60.0).abs() < 1e-4, "-20 dB");
        assert_eq!(meter_fill(0.000_001, 1.0), 0.0, "below -60 dB is empty");
        assert_eq!(meter_fill(0.5, 0.0), 0.0, "volume 0 is silent");
        assert_eq!(meter_color(0.5), Color32::from_rgb(80, 200, 120));
        assert_eq!(meter_color(0.8), Color32::from_rgb(240, 200, 70));
        assert_eq!(meter_color(0.95), Color32::from_rgb(240, 80, 70));
        // On the light palette's track every colour stands 3:1; on the dark one they are kept.
        for f in [0.5, 0.8, 0.95] {
            let light = meter_color_on(f, false);
            assert!(crate::theme::contrast(light, crate::theme::LIGHT.panel_hi) >= 3.0, "{f}: {light:?}");
            assert_eq!(meter_color_on(f, true), meter_color(f));
        }
    }

    #[test]
    fn added_sources_go_into_the_current_scene() {
        let mut a = SnagApp::blank(Settings::default(), false, None);
        let item = |name: &str| Item { name: name.into(), kind: SourceKind::Color { color: [0, 0, 0, 255] }, x: 0.0, y: 0.0, w: 10.0, h: 10.0, visible: true };
        a.settings.studio.scenes = vec![Scene { sounds: Vec::new(), name: "A".into(), items: Vec::new() }, Scene { sounds: Vec::new(), name: "B".into(), items: Vec::new() }];
        a.settings.studio.current = 1;
        add_item(&mut a, item("one"));
        assert!(a.settings.studio.scenes[0].items.is_empty());
        assert_eq!(a.settings.studio.scenes[1].items.len(), 1);
        // A stale index past the end falls back to the last scene.
        a.settings.studio.current = 9;
        add_item(&mut a, item("two"));
        assert_eq!(a.settings.studio.scenes[1].items.len(), 2);
        // No scenes at all: nothing happens.
        a.settings.studio.scenes.clear();
        add_item(&mut a, item("three"));
        assert!(a.settings.studio.scenes.is_empty());
    }

    /// Adding a sound goes into the current scene, never twice and never past six; sharing
    /// moves it between "every scene" and "this scene", and refuses when a scene is full.
    #[test]
    fn scene_sounds_are_added_shared_and_removed() {
        use crate::audio::SoundInput;
        use crate::studio::{MAX_SOUNDS, SceneSound};
        let mut a = SnagApp::blank(Settings::default(), false, None); // the computer sound is shared
        let file = |n: usize| SceneSound::new(SoundInput::File { path: format!("/s/{n}.wav") }, "f.wav");
        add_sound(&mut a, file(1));
        add_sound(&mut a, file(1));
        assert!(a.status.contains("already"), "{}", a.status);
        assert_eq!(a.settings.studio.scene_sounds(0).len(), 2);
        for n in 2..10 {
            add_sound(&mut a, file(n));
        }
        assert_eq!(a.settings.studio.scene_sounds(0).len(), MAX_SOUNDS);
        assert!(a.status.contains("at most 6"), "{}", a.status);
        // A second scene: the shared computer sound is there too.
        let st = &mut a.settings.studio;
        let s2 = st.add_scene(Some("Two"));
        assert_eq!(st.scene_sounds(s2).len(), 1);
        // Scene 1 is full, so a sound of scene 2 can't go into every scene.
        st.scenes[s2].sounds.push(file(50));
        assert!(sound_change(st, s2, false, 0, RowAsk::Share).unwrap_err().contains("make room"));
        // Make room, then it can; it shows in scene 1 as shared.
        st.scenes[0].sounds.pop();
        sound_change(st, s2, false, 0, RowAsk::Share).unwrap();
        assert_eq!(st.shared_sounds.len(), 2);
        assert!(st.scene_sounds(0).iter().any(|s| s.input == SoundInput::File { path: "/s/50.wav".into() }));
        // Back to scene 2 only, then removed.
        sound_change(st, s2, true, 1, RowAsk::Share).unwrap();
        assert_eq!(st.shared_sounds.len(), 1);
        assert_eq!(st.scenes[s2].sounds.len(), 1);
        sound_change(st, s2, false, 0, RowAsk::Remove).unwrap();
        sound_change(st, s2, true, 0, RowAsk::Remove).unwrap();
        assert!(st.scene_sounds(s2).is_empty());
        assert!(sound_change(st, s2, false, 7, RowAsk::Remove).is_ok(), "a row that is gone changes nothing");
    }

    #[test]
    fn every_input_has_an_icon_that_shows_muting() {
        use crate::audio::SoundInput;
        assert_eq!(sound_icon(&SoundInput::Computer, false), ph::SPEAKER);
        assert_eq!(sound_icon(&SoundInput::Computer, true), ph::SPEAKER_SLASH);
        assert_eq!(sound_icon(&SoundInput::Microphone { id: String::new() }, true), ph::MICROPHONE_SLASH);
        assert_eq!(sound_icon(&SoundInput::File { path: String::new() }, false), ph::WAVEFORM);
    }

    #[test]
    fn stop_and_pause_without_studio_do_nothing() {
        let mut a = SnagApp::blank(Settings::default(), false, None);
        stop(&mut a);
        toggle_pause(&mut a);
        start(&mut a, &egui::Context::default()); // no Studio open: doesn't record
        assert!(a.x.studio.is_none());
    }
}
